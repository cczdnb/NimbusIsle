//! 云屿表情：按场景播放，隐藏时暂停，日常动作之间留出安静时间。
//! 每个表情单独一张 15 帧的竖图。尺寸可以不同，播放时按这一张自己的格子往下切，只解码正在播放的那一张，播完即释放。
use crate::pet::PetState;
use crate::paint::Card;
use crate::skin_paint::Palette;
use eframe::egui::{self, Color32, Rect, TextureHandle};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Expression {
    Wink, Smug, Blush, Delighted, SideGlance, Praised,
    Heart, Giggle, ShyPeek, Scan, Peek, Search,
}

impl Expression {
    pub const ALL:[Self;12]=[Self::Wink,Self::Smug,Self::Blush,Self::Delighted,Self::SideGlance,Self::Praised,
        Self::Heart,Self::Giggle,Self::ShyPeek,Self::Scan,Self::Peek,Self::Search];
    fn times(self)->[f32;15] {
        if self as usize>=9 {[0.35,0.13,0.15,0.18,0.35,0.28,0.16,0.11,0.11,0.16,0.35,0.28,0.16,0.14,0.20]}
        else {[0.35,0.12,0.12,0.12,0.13,0.13,0.14,0.40,0.18,0.14,0.12,0.12,0.12,0.14,0.14]}
    }
    pub fn duration(self)->f32 {self.times().iter().sum()}
    /// 每张表情一张竖图，顺序与 `ALL` 一致。
    fn sheet(self)->&'static [u8] {
        const SHEETS:[&[u8];12]=[
            include_bytes!("../assets/pet/expressions/00-wink.png"),include_bytes!("../assets/pet/expressions/01-smug.png"),
            include_bytes!("../assets/pet/expressions/02-blush.png"),include_bytes!("../assets/pet/expressions/03-delighted.png"),
            include_bytes!("../assets/pet/expressions/04-side-glance.png"),include_bytes!("../assets/pet/expressions/05-praised.png"),
            include_bytes!("../assets/pet/expressions/06-heart.png"),include_bytes!("../assets/pet/expressions/07-giggle.png"),
            include_bytes!("../assets/pet/expressions/08-shy-peek.png"),include_bytes!("../assets/pet/expressions/09-scan.png"),
            include_bytes!("../assets/pet/expressions/10-peek.png"),include_bytes!("../assets/pet/expressions/11-search.png"),
        ];
        SHEETS[self as usize]
    }
    fn frame(self,time:f32)->usize {
        let mut end=0.0;
        for (index,seconds) in self.times().into_iter().enumerate() {end+=seconds;if time<end{return index;}}
        14
    }
}

pub(crate) struct Expressions {
    clip:Option<(Expression,f32)>,
    wait:f32,
    cooldown:f32,
    hovered:bool,
    next:[usize;4],
    /// 正在播放的表情。整张竖图留在内存里，显卡上只放当前这一格。
    sheet:Option<LoadedExpression>,
    /// 没有表情在播时，常驻显示的那张设计稿。
    rest:Option<TextureHandle>,
    /// 听歌时循环的 16 帧唱歌图，和 12 个表情分开加载。
    sing:Option<LoadedSing>,
    /// 下班倒计时专属序列。编号 1–13，对应 `assets/pet/offwork/{n}.png`。
    offwork_id:u8,
    offwork_time:f32,
    offwork_sheet:Option<LoadedOffwork>,
}

struct LoadedExpression {
    expression:Expression,
    palette:Option<Palette>,
    image:image::RgbaImage,
    edges:[f32;16],
    texture:TextureHandle,
    frame:usize,
}
struct LoadedSing { image:image::RgbaImage, texture:TextureHandle, frame:usize }
struct LoadedOffwork { id:u8, image:image::RgbaImage, texture:TextureHandle, frame:usize }

fn color_image(image:&image::RgbaImage)->egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied([image.width() as usize,image.height() as usize],image.as_raw())
}
fn crop_rows(image:&image::RgbaImage,y0:u32,y1:u32)->image::RgbaImage {
    let y0=y0.min(image.height().saturating_sub(1));
    let y1=y1.max(y0+1).min(image.height());
    image::imageops::crop_imm(image,0,y0,image.width(),y1-y0).to_image()
}
fn band_rows(height:u32,start:f32,end:f32)->(u32,u32) {
    let scale=height as f32;
    ((start*scale).round() as u32,(end*scale).round() as u32)
}

pub const SING_FRAME_SECS:f32=0.1;
pub const SING_FRAMES:usize=16;

const OFFWORK_FRAMES:[f32;15]=[0.35,0.12,0.12,0.12,0.13,0.13,0.14,0.40,0.18,0.14,0.12,0.12,0.12,0.14,0.14];

pub(crate) fn offwork_clip_duration()->f32 {OFFWORK_FRAMES.iter().sum()}

fn offwork_frame(time:f32)->usize {
    let mut end=0.0;
    for (index,seconds) in OFFWORK_FRAMES.into_iter().enumerate() {end+=seconds;if time<end{return index;}}
    14
}

fn offwork_sheet(id:u8)->&'static [u8] {
    match id {
        1=>include_bytes!("../assets/pet/offwork/1.png"),
        2=>include_bytes!("../assets/pet/offwork/2.png"),
        3=>include_bytes!("../assets/pet/offwork/3.png"),
        4=>include_bytes!("../assets/pet/offwork/4.png"),
        5=>include_bytes!("../assets/pet/offwork/5.png"),
        6=>include_bytes!("../assets/pet/offwork/6.png"),
        7=>include_bytes!("../assets/pet/offwork/7.png"),
        8=>include_bytes!("../assets/pet/offwork/8.png"),
        9=>include_bytes!("../assets/pet/offwork/9.png"),
        10=>include_bytes!("../assets/pet/offwork/10.png"),
        11=>include_bytes!("../assets/pet/offwork/11.png"),
        12=>include_bytes!("../assets/pet/offwork/12.png"),
        _=>include_bytes!("../assets/pet/offwork/13.png"),
    }
}

/// 一张表情竖图的 16 条切线（15 格）。能整除时按高度均分；探头这种格子上方留了空的，改用刚好包住每一帧的等距格子。
fn expression_frame_edges(image:&image::RgbaImage)->[f32;16] {
    let height=image.height() as f32;
    let equal=|index:usize| index as f32/15.0;
    let faces=ink_faces(image);
    if faces.len()!=15 {return std::array::from_fn(equal);}
    let fits_equal=faces.iter().enumerate().all(|(index,&(top,bottom))|{
        let start=equal(index)*height;
        let end=equal(index+1)*height;
        top as f32+1.0>=start && bottom as f32<=end+1.0
    });
    if fits_equal {return std::array::from_fn(equal);}
    let Some((origin,pitch))=fitted_cell(image.height(),&faces) else {return std::array::from_fn(equal);};
    std::array::from_fn(|index| ((origin+index as u32*pitch) as f32/height).min(1.0))
}

fn ink_faces(image:&image::RgbaImage)->Vec<(u32,u32)> {
    let (width,height)=(image.width(),image.height());
    let mut spans:Vec<(u32,u32)>=Vec::new();
    let mut start=None;
    for y in 0..height {
        let mut ink=false;
        let mut x=0;
        while x<width {
            if image.get_pixel(x,y).0[3]>=8 {ink=true;break;}
            x+=2;
        }
        if ink {if start.is_none(){start=Some(y);}}
        else if let Some(from)=start {spans.push((from,y-1));start=None;}
    }
    if let Some(from)=start {spans.push((from,height-1));}
    let mut merged:Vec<(u32,u32)>=Vec::new();
    for (top,bottom) in spans {
        if let Some(previous)=merged.last_mut() {
            if top-previous.1-1<=6 {*previous=(previous.0,bottom);continue;}
        }
        merged.push((top,bottom));
    }
    merged.retain(|&(top,bottom)| bottom+1-top>6);
    merged
}

fn fitted_cell(height:u32,faces:&[(u32,u32)])->Option<(u32,u32)> {
    let mut best:Option<(u32,u32,u32)>=None;
    let max_pitch=height/15+40;
    let min_pitch=(height/15).min(160);
    for pitch in min_pitch..=max_pitch {
        let span=15*pitch;
        if span>height {continue;}
        for origin in 0..=height-span {
            let mut slack=u32::MAX;
            let mut ok=true;
            for (index,&(top,bottom)) in faces.iter().enumerate() {
                let start=origin+index as u32*pitch;
                let end=start+pitch;
                if top<start || bottom+1>end {ok=false;break;}
                slack=slack.min(top-start).min(end-(bottom+1));
            }
            if ok && best.is_none_or(|(best_slack,_,_)| slack>best_slack) {best=Some((slack,origin,pitch));}
        }
    }
    best.map(|(_,origin,pitch)| (origin,pitch))
}

/// 唱歌序列的帧号。系统关闭动画时停在张嘴的中间帧。
pub fn sing_frame(age:f32)->usize {
    if !crate::bar_layout::animations_enabled() {7}
    else {(age.max(0.0)/SING_FRAME_SECS).floor() as usize%SING_FRAMES}
}

impl Default for Expressions {
    fn default()->Self {Self{clip:None,wait:8.0,cooldown:0.0,hovered:false,next:[0;4],sheet:None,rest:None,sing:None,offwork_id:0,offwork_time:0.0,offwork_sheet:None}}
}

impl Expressions {
    pub fn current(&self)->Option<Expression> {self.clip.map(|clip|clip.0)}
    pub fn shown_sing_frame(&self)->Option<usize> {self.sing.as_ref().map(|loaded| loaded.frame)}
    pub fn play(&mut self,expression:Expression) {self.clip=Some((expression,0.0));}
    pub fn play_at(&mut self,expression:Expression,time:f32) {self.clip=Some((expression,time));}
    pub fn clear(&mut self) {self.clip=None;self.wait=8.0;self.cooldown=0.0;self.hovered=false;self.sheet=None;}
    /// 专属表情占用吉祥物时，清掉正在播的日常序列，避免两套叠在一起。
    pub fn suppress_clip(&mut self) {self.clip=None;self.sheet=None;self.wait=8.0;}
    pub fn show_offwork(&mut self,id:u8,time:f32) {self.offwork_id=id;self.offwork_time=time;}
    pub fn clear_offwork(&mut self) {self.offwork_id=0;self.offwork_time=0.0;}
    fn choose(&mut self,bucket:usize,pool:&[Expression])->Expression {
        let expression=pool[self.next[bucket]%pool.len()];self.next[bucket]=self.next[bucket].wrapping_add(1);expression
    }
    pub fn react(&mut self,state:PetState)->f32 {
        use Expression::*;
        let expression=match state {
            PetState::Happy=>self.choose(2,&[Praised,Smug,Delighted,Giggle]),
            PetState::Satisfied=>self.choose(3,&[Heart,Delighted]),
            _=>return 2.6,
        };
        self.play(expression);expression.duration()
    }
    pub fn step(&mut self,state:PetState,dt:f32,visible:bool) {
        use Expression::*;
        if !visible {return;}
        if !matches!(state,PetState::Awake|PetState::Curious|PetState::Happy|PetState::Satisfied) {self.clear();return;}
        self.cooldown=(self.cooldown-dt).max(0.0);
        if let Some((expression,time))=&mut self.clip {
            *time+=dt;
            if *time>=expression.duration() {self.clip=None;self.sheet=None;self.wait=22.0+(self.next[0]%4) as f32*3.0;}
        }
        let hovered=state==PetState::Curious;
        if hovered && !self.hovered && self.cooldown<=0.0 && self.clip.is_none() {
            let expression=self.choose(1,&[Wink,Blush,SideGlance,ShyPeek]);self.play(expression);self.cooldown=18.0;
        }
        self.hovered=hovered;
        if state==PetState::Awake && self.clip.is_none() && crate::bar_layout::animations_enabled() {
            self.wait-=dt;
            if self.wait<=0.0 {
                let expression=self.choose(0,&[Scan,Delighted,Peek,Smug,Search,Giggle,Praised,ShyPeek]);self.play(expression);
            }
        }
    }
    pub fn draw(&mut self,c:&Card,cx:f32,palette:Option<Palette>)->bool {
        let Some((expression,time))=self.clip else{return false;};
        let frame=if crate::bar_layout::animations_enabled(){expression.frame(time)}else{7};
        if self.sheet.as_ref().is_none_or(|loaded| loaded.expression!=expression) {
            let decoded=image::load_from_memory(expression.sheet()).expect("内置表情序列图").into_rgba8();
            let edges=expression_frame_edges(&decoded);
            let (y0,y1)=band_rows(decoded.height(),edges[frame],edges[frame+1]);
            let image=color_image(&crop_rows(&decoded,y0,y1));
            if let Some(loaded)=&mut self.sheet {
                loaded.expression=expression;loaded.palette=palette;loaded.image=decoded;loaded.edges=edges;loaded.frame=frame;
                loaded.texture.set(image,egui::TextureOptions::LINEAR);
            } else {
                let texture=c.ui.ctx().load_texture("yunyu-expressions",image,egui::TextureOptions::LINEAR);
                crate::skin_paint::mark_colored(texture.id());
                self.sheet=Some(LoadedExpression{expression,palette,image:decoded,edges,texture,frame});
            }
        } else if self.sheet.as_ref().unwrap().frame!=frame {
            let loaded=self.sheet.as_mut().unwrap();
            let (y0,y1)=band_rows(loaded.image.height(),loaded.edges[frame],loaded.edges[frame+1]);
            loaded.texture.set(color_image(&crop_rows(&loaded.image,y0,y1)),egui::TextureOptions::LINEAR);
            loaded.frame=frame;loaded.palette=palette;
        }
        let uv=Rect::from_min_max(egui::pos2(0.0,0.0),egui::pos2(1.0,1.0));
        c.painter.image(self.sheet.as_ref().unwrap().texture.id(),c.rect(cx-24.0,0.0,48.0,30.0),uv,Color32::WHITE);
        true
    }
    /// 下班专属表情。15 帧竖图，画进和日常表情一样的 48×30 槽。
    pub fn draw_offwork(&mut self,c:&Card,cx:f32)->bool {
        let id=self.offwork_id;
        if !(1..=13).contains(&id) {return false;}
        let frame=if crate::bar_layout::animations_enabled(){offwork_frame(self.offwork_time)}else{7};
        if self.offwork_sheet.as_ref().is_none_or(|loaded| loaded.id!=id) {
            let decoded=image::load_from_memory(offwork_sheet(id)).expect("下班专属表情").into_rgba8();
            let (y0,y1)=band_rows(decoded.height(),frame as f32/15.0,(frame+1) as f32/15.0);
            let image=color_image(&crop_rows(&decoded,y0,y1));
            if let Some(loaded)=&mut self.offwork_sheet {loaded.id=id;loaded.image=decoded;loaded.frame=frame;loaded.texture.set(image,egui::TextureOptions::LINEAR);}
            else {
                let texture=c.ui.ctx().load_texture(format!("yunyu-offwork-{id}"),image,egui::TextureOptions::LINEAR);
                crate::skin_paint::mark_colored(texture.id());
                self.offwork_sheet=Some(LoadedOffwork{id,image:decoded,texture,frame});
            }
        } else if self.offwork_sheet.as_ref().unwrap().frame!=frame {
            let loaded=self.offwork_sheet.as_mut().unwrap();
            let (y0,y1)=band_rows(loaded.image.height(),frame as f32/15.0,(frame+1) as f32/15.0);
            loaded.texture.set(color_image(&crop_rows(&loaded.image,y0,y1)),egui::TextureOptions::LINEAR);
            loaded.frame=frame;
        }
        let uv=Rect::from_min_max(egui::pos2(0.0,0.0),egui::pos2(1.0,1.0));
        c.painter.image(self.offwork_sheet.as_ref().unwrap().texture.id(),c.rect(cx-24.0,0.0,48.0,30.0),uv,Color32::WHITE);
        true
    }
    /// 待机、专注、睡觉这些没有序列在播的时候，显示设计稿里睁眼的那一帧。
    pub fn draw_rest(&mut self,c:&Card,cx:f32) {
        if self.rest.is_none() {
            let decoded=image::load_from_memory(Expression::Wink.sheet()).expect("内置吉祥物").into_rgba8();
            let cell=(decoded.height()/15).max(1);
            let frame=image::imageops::crop_imm(&decoded,0,0,decoded.width(),cell).to_image();
            let image=egui::ColorImage::from_rgba_unmultiplied([frame.width() as usize,frame.height() as usize],frame.as_raw());
            let texture=c.ui.ctx().load_texture("yunyu-mascot",image,egui::TextureOptions::LINEAR);
            crate::skin_paint::mark_colored(texture.id());
            self.rest=Some(texture);
        }
        c.painter.image(self.rest.as_ref().unwrap().id(),c.rect(cx-24.0,0.0,48.0,30.0),Rect::from_min_max(egui::pos2(0.0,0.0),egui::pos2(1.0,1.0)),Color32::WHITE);
    }
    /// 听歌且没有插入表情时，循环播放唱歌序列。
    pub fn draw_sing(&mut self,c:&Card,cx:f32,age:f32) {
        let frame=sing_frame(age);
        let step=1.0/SING_FRAMES as f32;
        if self.sing.is_none() {
            let decoded=image::load_from_memory(include_bytes!("../assets/pet/expressions/12-sing.png")).expect("唱歌序列图").into_rgba8();
            let (y0,y1)=band_rows(decoded.height(),frame as f32*step,(frame+1) as f32*step);
            let texture=c.ui.ctx().load_texture("yunyu-sing",color_image(&crop_rows(&decoded,y0,y1)),egui::TextureOptions::LINEAR);
            crate::skin_paint::mark_colored(texture.id());
            self.sing=Some(LoadedSing{image:decoded,texture,frame});
        } else if self.sing.as_ref().unwrap().frame!=frame {
            let loaded=self.sing.as_mut().unwrap();
            let (y0,y1)=band_rows(loaded.image.height(),frame as f32*step,(frame+1) as f32*step);
            loaded.texture.set(color_image(&crop_rows(&loaded.image,y0,y1)),egui::TextureOptions::LINEAR);
            loaded.frame=frame;
        }
        let uv=Rect::from_min_max(egui::pos2(0.0,0.0),egui::pos2(1.0,1.0));
        c.painter.image(self.sing.as_ref().unwrap().texture.id(),c.rect(cx-24.0,0.0,48.0,30.0),uv,Color32::WHITE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expressions_follow_real_scenarios_and_system_animation_setting(){
        let mut expressions=Expressions::default();let mut seen=std::collections::HashSet::new();
        let idle_animations=crate::bar_layout::animations_enabled();
        if idle_animations {
            for _ in 0..8 {
                for _ in 0..400 {
                    expressions.step(PetState::Awake,0.1,true);
                    if expressions.current().is_some(){break;}
                }
                seen.insert(expressions.current().expect("日常表情应在等待时间内触发") as usize);
                for _ in 0..100 {
                    expressions.step(PetState::Awake,0.1,true);
                    if expressions.current().is_none(){break;}
                }
                assert!(expressions.current().is_none(),"日常表情应按时结束");
                for _ in 0..210 {expressions.step(PetState::Awake,0.1,true);assert!(expressions.current().is_none(),"日常动作之间应有安静间隔");}
            }
        }else{
            for _ in 0..400 {
                expressions.step(PetState::Awake,0.1,true);
                assert!(expressions.current().is_none(),"系统关闭动画时应保持安静待机");
            }
        }
        for _ in 0..4 {
            expressions.clear();expressions.step(PetState::Curious,0.1,true);seen.insert(expressions.current().unwrap() as usize);
        }
        for state in [PetState::Happy,PetState::Happy,PetState::Happy,PetState::Happy,PetState::Satisfied,PetState::Satisfied] {
            expressions.react(state);seen.insert(expressions.current().unwrap() as usize);
        }
        let expected=Expression::ALL.into_iter().filter(|expression|idle_animations || !matches!(expression,Expression::Scan|Expression::Peek|Expression::Search))
            .map(|expression|expression as usize).collect::<std::collections::HashSet<_>>();
        assert_eq!(seen,expected,"应覆盖当前系统动画设置下可触发的全部表情");
    }
    #[test]
    fn hidden_feedback_pauses_and_hover_has_a_cooldown(){
        let mut expressions=Expressions::default();expressions.react(PetState::Happy);
        expressions.step(PetState::Happy,30.0,false);assert_eq!(expressions.clip.unwrap().1,0.0);
        expressions.step(PetState::Happy,1.0,true);assert_eq!(expressions.clip.unwrap().1,1.0);
        expressions.clear();expressions.step(PetState::Curious,0.1,true);let first=expressions.current().unwrap();
        for _ in 0..50 {expressions.step(PetState::Curious,0.1,true);}
        expressions.step(PetState::Awake,0.1,true);expressions.step(PetState::Curious,0.1,true);
        assert!(expressions.current().is_none(),"快速反复经过不能一直重播互动");
        for _ in 0..180 {expressions.step(PetState::Curious,0.1,true);}
        expressions.step(PetState::Awake,0.1,true);expressions.step(PetState::Curious,0.1,true);
        assert_ne!(expressions.current().unwrap(),first);
    }
    #[test]
    fn focus_music_sleep_and_warning_keep_priority(){
        let mut expressions=Expressions::default();
        for state in [PetState::Focused,PetState::Music,PetState::Worried,PetState::Waking,PetState::Dozing,PetState::Sleeping] {
            expressions.play(Expression::Wink);expressions.step(state,0.1,true);assert!(expressions.current().is_none());
        }
    }
    #[test]
    fn sheets_keep_alpha_and_the_looking_eyes_really_move(){
        for expression in Expression::ALL {
            let image=image::load_from_memory(expression.sheet()).unwrap().into_rgba8();
            assert_eq!(image.get_pixel(0,0).0[3],0,"{expression:?}");
            let edges=expression_frame_edges(&image);
            let height=image.height() as f32;
            let mut centers=Vec::new();
            for frame in 0..15 {
                let y0=(edges[frame]*height).floor() as u32;
                let y1=(edges[frame+1]*height).ceil() as u32;
                let mut xs=Vec::new();
                for y in y0..y1.min(image.height()) {
                    let mut x=0;
                    while x<image.width() {
                        if image.get_pixel(x,y).0[3]>150 {xs.push(x as f32);}
                        x+=2;
                    }
                }
                assert!(!xs.is_empty(),"{expression:?} 第 {frame} 帧是空的");
                centers.push(xs.iter().sum::<f32>()/xs.len() as f32);
            }
            if expression as usize>=9 {assert!(centers.iter().copied().fold(f32::NEG_INFINITY,f32::max)-centers.iter().copied().fold(f32::INFINITY,f32::min)>6.0,"{expression:?} 应真的左右移动");}
            assert_eq!(expression.frame(0.0),0);assert_eq!(expression.frame(expression.duration()),14);
        }
    }
    #[test]
    fn only_the_playing_sheet_is_uploaded_and_light_skins_keep_dark_eyes(){
        let ctx=egui::Context::default();let mut expressions=Expressions::default();expressions.play(Expression::Wink);
        let frame=|expressions:&mut Expressions,palette|ctx.run(Default::default(),|ctx|{
            egui::CentralPanel::default().show(ctx,|ui|{
                let card=Card::new(ui,egui::Pos2::ZERO,ui.max_rect());assert!(expressions.draw(&card,24.0,palette));
            });
        });
        let output=frame(&mut expressions,None);let id=expressions.sheet.as_ref().unwrap().texture.id();
        let delta=&output.textures_delta.set.iter().find(|(texture,_)|*texture==id).unwrap().1;
        let egui::ImageData::Color(image)=&delta.image else{panic!("表情应为彩色纹理")};
        assert_eq!(image.size,[336,213],"只上传眨眼的当前一帧");
        assert!(image.pixels.iter().any(|p|p.a()>200&&p.r()<70&&p.g()<55&&p.b()<45),"表情应带着深色眼睛");
        assert!(!frame(&mut expressions,None).textures_delta.set.iter().any(|(texture,_)|*texture==id),"同一表情、同一皮肤不能重复解码或上传");
        for palette in [crate::skin_paint::CREAM,crate::skin_paint::GLASS_LIGHT] {
            let output=frame(&mut expressions,Some(palette));assert_eq!(expressions.sheet.as_ref().unwrap().texture.id(),id);
            assert!(!output.textures_delta.set.iter().any(|(texture,_)|*texture==id),"彩色表情按原图显示，不随浅色皮肤反相");
        }
        expressions.play(Expression::Search);
        let output=frame(&mut expressions,None);assert_eq!(expressions.sheet.as_ref().unwrap().texture.id(),id,"换表情原地替换，不新建纹理");
        assert!(output.textures_delta.set.iter().any(|(texture,_)|*texture==id));
        for _ in 0..100 {expressions.step(PetState::Awake,0.1,true);if expressions.current().is_none(){break;}}
        assert!(expressions.current().is_none() && expressions.sheet.is_none(),"播完释放纹理，不常驻显存");
        let output=ctx.run(Default::default(),|_|{});assert!(output.textures_delta.free.contains(&id));
    }

    #[test]
    fn every_frame_stays_inside_its_cell(){
        for expression in Expression::ALL {
            let image=image::load_from_memory(expression.sheet()).unwrap().into_rgba8();
            let faces=ink_faces(&image);
            let edges=expression_frame_edges(&image);
            assert_eq!(faces.len(),15,"{expression:?}");
            let height=image.height() as f32;
            for (index,(top,bottom)) in faces.iter().enumerate() {
                let start=edges[index]*height;
                let end=edges[index+1]*height;
                assert!(*top as f32+0.5>=start && *bottom as f32<=end+0.5,"{expression:?} 第 {index} 帧越出格子");
            }
        }
        let sing=image::load_from_memory(include_bytes!("../assets/pet/expressions/12-sing.png")).unwrap().into_rgba8();
        assert_eq!(sing.dimensions(),(672,6704));
        assert_eq!(sing.height()%SING_FRAMES as u32,0);
    }
}
