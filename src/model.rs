//! 持久化与状态模型。
//!
//! 文件布局沿用原来的文件名和键名，默认目录是 `%APPDATA%\Yunyu`：
//!   %APPDATA%\Yunyu\settings.txt   -> pet=0/1, minimal=0/1
//!   %APPDATA%\Yunyu\todo.txt
//!   %APPDATA%\Yunyu\water.txt      -> bottle=<ml> / goal=<ml> / start=YYYY-MM-DD
//!   %APPDATA%\Yunyu\shelf.txt      -> 置物清单（每行一个绝对路径）
//!
//! 设置环境变量 `YUNYU_DATA_DIR` 可把以上目录整体改到别处（跑测试时不碰真实数据）。

use std::fs;
use std::path::{Path, PathBuf};

fn env_dir(key: &str) -> PathBuf {
    PathBuf::from(std::env::var(key).unwrap_or_else(|_| ".".into()))
}

pub fn config_dir() -> PathBuf {
    if let Ok(root) = std::env::var("YUNYU_DATA_DIR") {
        return PathBuf::from(root);
    }
    env_dir("APPDATA").join("Yunyu")
}

fn read_kv(path: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Ok(text) = fs::read_to_string(path) {
        for line in text.trim_start_matches('\u{feff}').lines() {
            if let Some((k, v)) = line.split_once('=') {
                out.push((k.trim().to_string(), v.trim().to_string()));
            }
        }
    }
    out
}

fn write_kv(path: &Path, pairs: &[(String, String)]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let body: String = pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}\n"))
        .collect();
    atomic_write(path, body.as_bytes())
}

pub(crate) fn load_health_pending()->[bool;2] {
    let mut pending=[false;2];
    for (key,value) in read_kv(&config_dir().join("health-pending.txt")) {
        match key.as_str() {"water"=>pending[0]=value=="1","sit"=>pending[1]=value=="1",_=>{}}
    }
    pending
}

pub(crate) fn save_health_pending(pending:[bool;2]) {
    let _=write_kv(&config_dir().join("health-pending.txt"),&[
        ("water".into(),u8::from(pending[0]).to_string()),
        ("sit".into(),u8::from(pending[1]).to_string()),
    ]);
}

pub(crate) fn atomic_write(path:&Path,body:&[u8])->std::io::Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64,Ordering};
    static SERIAL:AtomicU64=AtomicU64::new(0);
    let (tmp,mut file)=loop {
        let mut name=path.file_name().ok_or_else(||std::io::Error::other("缺少文件名"))?.to_os_string();
        name.push(format!(".{}-{}.tmp",std::process::id(),SERIAL.fetch_add(1,Ordering::Relaxed)));
        let tmp=path.with_file_name(name);
        match fs::OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(file)=>break (tmp,file),
            Err(e) if e.kind()==std::io::ErrorKind::AlreadyExists=>continue,
            Err(e)=>return Err(e),
        }
    };
    let result=file.write_all(body).and_then(|_|file.sync_all());
    drop(file);
    let result=result.and_then(|_|replace_file(&tmp,path));
    if result.is_err(){let _=fs::remove_file(&tmp);}
    result
}

fn replace_file(tmp:&Path,path:&Path)->std::io::Result<()> {
    let result=fs::rename(tmp,path);
    #[cfg(windows)]
    if result.as_ref().is_err_and(|e|e.kind()==std::io::ErrorKind::CrossesDevices) {
        use std::os::windows::ffi::OsStrExt;
        use windows::core::PCWSTR;
        use windows::Win32::Storage::FileSystem::{MoveFileExW,MOVEFILE_COPY_ALLOWED,MOVEFILE_REPLACE_EXISTING,MOVEFILE_WRITE_THROUGH};
        // Windows 目录映射可能将同目录替换报告为跨卷，交给系统复制并替换。
        let source:Vec<u16>=tmp.as_os_str().encode_wide().chain(Some(0)).collect();
        let target:Vec<u16>=path.as_os_str().encode_wide().chain(Some(0)).collect();
        return unsafe{MoveFileExW(PCWSTR(source.as_ptr()),PCWSTR(target.as_ptr()),MOVEFILE_REPLACE_EXISTING|MOVEFILE_COPY_ALLOWED|MOVEFILE_WRITE_THROUGH)}.map_err(std::io::Error::other);
    }
    result
}

/// 岛体配色皮肤（与形状独立，可任意组合）。
/// - 经典：纯黑；奶油：暖白底深棕字、橙色强调（参考 Oyen）；
/// - 毛玻璃·浅 / 毛玻璃·深：系统实时模糊 + 浅色或深色玻璃。
/// - 液态玻璃：抓取身后桌面，经高斯模糊后做边缘折射、色散与雾度。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Skin {
    #[default]
    Classic,
    Cream,
    GlassLight,
    GlassDark,
    Fluid,
    LiquidGlass,
}

impl Skin {
    pub const ALL: [Skin; 6] = [Skin::Classic, Skin::Cream, Skin::GlassLight, Skin::GlassDark, Skin::Fluid, Skin::LiquidGlass];
    pub fn key(self) -> &'static str {
        match self { Self::Classic => "classic", Self::Fluid => "fluid", Self::Cream => "cream", Self::GlassLight => "glass_light", Self::GlassDark => "glass", Self::LiquidGlass => "liquid_glass" }
    }
    pub fn parse(value: &str) -> Self {
        match value { "fluid" => Self::Fluid, "cream" => Self::Cream, "glass_light" => Self::GlassLight, "glass" | "glass_dark" => Self::GlassDark, "liquid_glass" | "liquid" => Self::LiquidGlass, _ => Self::Classic }
    }
    pub fn label(self) -> &'static str {
        match self { Self::Classic => "经典", Self::Fluid => "悬浮云", Self::Cream => "奶油", Self::GlassLight => "毛玻璃·浅", Self::GlassDark => "毛玻璃·深", Self::LiquidGlass => "液态玻璃" }
    }
    pub fn description(self) -> &'static str {
        match self {
            Self::Classic => "纯黑岛体，与屏幕刘海融为一体，资源占用最低。",
            Self::Fluid => "半透明灰底随音乐封面换色，光晕跟随鼠标，播放进度带有渐隐光尾。皮肤作者：啵（@hanhb123）。",
            Self::Cream => "暖白底色配深棕文字和橙色强调，柔和明亮。",
            Self::GlassLight => "浅色磨砂玻璃，透出身后桌面的颜色，文字为深色；按背景明暗自动调整，保证清晰。",
            Self::GlassDark => "深色磨砂玻璃，透出身后桌面的颜色，文字为白色；按背景明暗自动调整，保证清晰。",
            Self::LiquidGlass => "抓取岛体身后的桌面，经高斯模糊后做边缘折射、色散与雾度，接近液态玻璃的观感。抓屏失败或机器较吃力时自动改用深色毛玻璃，无需另行开关。",
        }
    }
    pub fn is_glass(self) -> bool { matches!(self, Self::GlassLight | Self::GlassDark) }
    pub fn is_liquid(self) -> bool { matches!(self, Self::LiquidGlass) }
}

/// 岛体形状：贴着屏幕顶边的刘海，或悬浮在顶边下方的胶囊。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IslandShape {
    #[default]
    Notch,
    Pill,
}

impl IslandShape {
    pub const ALL: [IslandShape; 2] = [IslandShape::Notch, IslandShape::Pill];
    pub fn key(self) -> &'static str {
        match self { Self::Notch => "notch", Self::Pill => "pill" }
    }
    pub fn parse(value: &str) -> Self {
        match value { "pill" => Self::Pill, _ => Self::Notch }
    }
    pub fn label(self) -> &'static str {
        match self { Self::Notch => "刘海", Self::Pill => "胶囊" }
    }
    pub fn description(self) -> &'static str {
        match self {
            Self::Notch => "贴着屏幕顶边，顶部两侧向外柔和过渡，像屏幕刘海。",
            Self::Pill => "悬浮在屏幕顶边下方，四角圆润，收起时是完整的胶囊。",
        }
    }
}

/// 岛体停靠的屏幕边缘：顶部为横向刘海 / 胶囊，左右两侧为竖向排列。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dock {
    #[default]
    Top,
    Left,
    Right,
}

impl Dock {
    pub const ALL: [Dock; 3] = [Dock::Top, Dock::Left, Dock::Right];
    pub fn key(self) -> &'static str {
        match self { Self::Top => "top", Self::Left => "left", Self::Right => "right" }
    }
    pub fn parse(value: &str) -> Self {
        match value { "left" => Self::Left, "right" => Self::Right, _ => Self::Top }
    }
    pub fn label(self) -> &'static str {
        match self { Self::Top => "顶部", Self::Left => "左侧", Self::Right => "右侧" }
    }
    pub fn is_side(self) -> bool { self != Self::Top }
}

/// 吉祥物相对岛体的落点。岛头、岛尾沿岛的长边，不按屏幕左右。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PetPlace {
    #[default]
    Island,
    Head,
    Tail,
    Auto,
}

impl PetPlace {
    pub fn key(self) -> &'static str {
        match self { Self::Island => "island", Self::Head => "head", Self::Tail => "tail", Self::Auto => "auto" }
    }
    pub fn parse(value: &str) -> Self {
        match value { "head" => Self::Head, "tail" => Self::Tail, "auto" => Self::Auto, _ => Self::Island }
    }
}

/// 岛体大小（百分比）的范围与步进。这是整体缩放，不是高度。
pub const ISLAND_SIZE_MIN: u32 = 80;
pub const ISLAND_SIZE_MAX: u32 = 140;
pub const ISLAND_SIZE_STEP: u32 = 5;
/// 岛体宽度（逻辑点）。0 = 自动（跟随常驻内容）；手动范围与画布安全宽度 WIN_W-32 一致。
pub const ISLAND_WIDTH_MIN: u32 = 160;
pub const ISLAND_WIDTH_MAX: u32 = 608;
pub const ISLAND_WIDTH_STEP: u32 = 8;
pub const ISLAND_WIDTH_DEFAULT: u32 = 0;
/// 水平位置：-100 = 最左，0 = 居中，100 = 最右（按可移动范围的百分比）。
pub const ISLAND_OFFSET_MAX: i32 = 100;
/// 岛上功能页面数量（含第三方待办页和会话页）。
pub const PAGE_COUNT: usize = 11;

pub fn island_size_value(value: u32) -> u32 {
    let value = value.clamp(ISLAND_SIZE_MIN, ISLAND_SIZE_MAX);
    (value + ISLAND_SIZE_STEP / 2) / ISLAND_SIZE_STEP * ISLAND_SIZE_STEP
}

pub fn island_width_value(value: u32) -> u32 {
    if value == 0 { return 0; } // 自动：不强制最小宽度，跟随常驻内容
    let value = value.clamp(ISLAND_WIDTH_MIN, ISLAND_WIDTH_MAX);
    let snapped = (value + ISLAND_WIDTH_STEP / 2) / ISLAND_WIDTH_STEP * ISLAND_WIDTH_STEP;
    snapped.clamp(ISLAND_WIDTH_MIN, ISLAND_WIDTH_MAX)
}


fn normalize_player_key(source:&str)->String {
    source.rsplit(['\\','/']).next().unwrap_or(source).trim().to_string()
}
fn player_delay_for(list:&[(String,i32)], source:&str)->i32 {
    let key=normalize_player_key(source);
    if key.is_empty(){return 0;}
    list.iter().find(|(s,_)|s.eq_ignore_ascii_case(&key)).map(|(_,d)|*d).unwrap_or(0)
}
fn parse_player_delays(v:&str)->Vec<(String,i32)> {
    v.split(',').filter_map(|part|{
        let (k,val)=part.split_once(':')?;
        let k=normalize_player_key(k);
        if k.is_empty(){return None;}
        Some((k, val.trim().parse::<i32>().ok()?.clamp(-5000,5000)))
    }).collect()
}
fn format_player_delays(list:&[(String,i32)])->String {
    list.iter().map(|(s,d)|format!("{s}:{d}")).collect::<Vec<_>>().join(",")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LyricAlign {
    Left,
    #[default]
    Center,
}
impl LyricAlign {
    pub fn parse(s:&str)->Self { if s=="left" {Self::Left} else {Self::Center} }
    pub fn key(self)->&'static str { match self { Self::Left=>"left", Self::Center=>"center" } }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LyricAnim {
    #[default]
    Scan,
    Fade,
    Scroll,
}
impl LyricAnim {
    pub fn parse(s:&str)->Self { match s { "fade"=>Self::Fade, "scroll"=>Self::Scroll, _=>Self::Scan } }
    pub fn key(self)->&'static str { match self { Self::Scan=>"scan", Self::Fade=>"fade", Self::Scroll=>"scroll" } }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SpectrumStyle {
    #[default]
    Bars,
    Mirror,
    Dots,
    Wave,
}
impl SpectrumStyle {
    pub fn parse(s:&str)->Self { match s { "mirror"=>Self::Mirror, "dots"=>Self::Dots, "wave"=>Self::Wave, _=>Self::Bars } }
    pub fn key(self)->&'static str { match self { Self::Bars=>"bars", Self::Mirror=>"mirror", Self::Dots=>"dots", Self::Wave=>"wave" } }
}

#[derive(Clone, Debug)]
pub struct Settings {
    pub display_id: String,
    /// 岛体皮肤、毛玻璃是否使用系统模糊、大小百分比、宽度（逻辑点）、水平位置。
    pub skin: Skin,
    pub island_shape: IslandShape,
    pub glass_blur: bool,
    pub island_size: u32,
    pub island_width: u32,
    pub island_offset: i32,
    /// OLED 像素偏移：岛体收起静止时沿停靠边缓慢移动几个像素，减轻 OLED 静态显示留下的痕迹。
    /// 默认关闭，开启后也不改动 `island_offset`。
    pub oled_shift: bool,
    /// 停靠边缘；左右停靠时 `island_offset` 表示上下位置。
    pub dock: Dock,
    pub settings_theme: crate::settings_theme::SettingsTheme,
    pub pet: bool,
    /// 吉祥物落在岛上槽位，或贴在岛头、岛尾。默认岛上。
    pub pet_place: PetPlace,
    pub minimal: bool,
    pub browser_minimal: bool,
    pub avoid_browsers: bool,
    pub avoid_apps: Vec<(String,String)>,
    pub visible_pages: [bool; PAGE_COUNT],
    pub page_order: [usize; PAGE_COUNT],
    pub show_clock: bool,
    pub show_battery: bool,
    pub show_weather: bool,
    /// 天气页的落雨、落雪和阳光闪光。关闭后只留静止图标。
    pub weather_scene: bool,
    /// 收起时常驻 CPU、内存占用。默认关闭，不另开面板。
    pub show_cpu: bool,
    pub show_mem: bool,
    /// 收起时显示下班倒计时。默认关闭。
    pub offwork: bool,
    /// 固定上班时间，一天中的分钟数。小时格子从这里起算。
    pub offwork_start: u32,
    /// 固定下班时间，一天中的分钟数。
    pub offwork_end: u32,
    /// 周六日不显示倒计时。
    pub offwork_weekdays: bool,
    /// 下班前多少分钟算「快下班」：倒计时变色。
    pub offwork_soon: u32,
    /// 打开后用班次专属表情。图还没画好时先不播现有表情顶替。
    pub offwork_own_faces: bool,
    /// 微信、QQ 和系统通知横幅及其铃铛的停留秒数。
    pub notification_dwell_secs: f32,
    pub clock_12h: bool,
    pub clock_period: u8,
    pub clock_seconds: bool,
    pub resident_order: Vec<usize>,
    pub system_hud: bool,
    pub bluetooth_notifications: bool,
    pub read_notifications:bool,
    pub wechat_notifications:bool,
    pub wecom_notifications:bool,
    pub feishu_notifications:bool,
    pub dingtalk_notifications:bool,
    /// 微信缩在托盘时，点提醒通过系统托盘按钮唤起微信（默认关闭）。
    pub wechat_tray_open:bool,
    pub qq_only:bool,
    pub notification_preview:bool,
    pub notifications: bool,
    pub sound: bool,
    pub lyrics_enabled:bool,
    pub island_lyrics:bool,
    pub lyric_delay_ms:i32,
    pub lyric_translation:bool,
    pub lyric_align:LyricAlign,
    pub lyric_anim:LyricAnim,
    pub spectrum_enabled:bool,
    pub spectrum_style:SpectrumStyle,
    pub lyric_progress:bool,
    pub lyric_font_size:f32,
    /// 收起媒体条显示第二行原文。默认关闭，译文仍由 `lyric_translation` 单独控制。
    pub lyric_multiline:bool,
    /// 只给收起媒体条铺一层封面主色。默认关闭，不改变流体皮肤的整岛染色。
    pub cover_tint:bool,
    pub lyric_player_delays:Vec<(String,i32)>,
    /// 全局快捷键，空字符串表示不注册。顺序：播放暂停、上一首、下一首、快退、快进、唤出或隐藏。
    pub hotkeys:[String;6],
    pub hide_fullscreen: bool,
    /// 启用顶栏适配，按显示方式避让顶栏或保持岛体在顶栏上方。
    pub avoid_top_bar: bool,
    pub top_bar_on_top: bool,
    pub focus_minutes: u32,
    pub rest_minutes: u32,
    pub water_minutes: u32,
    pub serving_ml: u32,
    pub water_start: u32,
    pub water_end: u32,
    /// 久坐提醒：连续使用电脑达到 sit_minutes 分钟后提醒起身；与喝水共用提醒时段。
    pub sit_remind: bool,
    pub sit_minutes: u32,
    /// 午休免打扰：lunch_start..lunch_end（一天中的分钟数）内喝水和久坐都不提醒。
    pub lunch_rest: bool,
    pub lunch_start: u32,
    pub lunch_end: u32,
    /// v0.5.5 邮件提醒：总开关、验证码高亮、轮询间隔（秒）、会议提前量（分钟）。
    pub mail_enabled: bool,
    pub mail_sync_operations:bool,
    pub mail_code: bool,
    pub mail_code_hide_seconds:u32,
    pub mail_interval: u32,
    pub mail_ics: u32,
    pub ignored_mail_events: Vec<String>,
    /// v0.5.6 日程：还没配置时在岛上给一次「接入飞书日历」的提示；
    /// 用户点开设置或关掉后置 false，不再弹。
    pub calendar_prompt: bool,
    pub calendar_auto_sync: bool,
    /// 记录最近复制的文字（最近记录与固定条目均在本机保存）。
    pub clipboard_history: bool,
    /// 天气页要看的城市；留空表示按网络出口自动定位。
    pub weather_city: String,
    /// 用户确认的具体地点；刷新时直接使用坐标，避免同名城市重新匹配。
    pub weather_position: Option<crate::weather::Position>,
    /// 已保存、可在岛上下拉切换的城市（不含自动定位），最多 `weather::MAX_CITIES` 个。
    pub weather_saved: Vec<crate::weather::Position>,
    /// 兼容渲染：用 Direct3D 12 代替 OpenGL（部分显卡的 OpenGL 驱动会崩溃或画面错乱）。重启后生效。
    pub render_compat: bool,
    /// 自动切换到兼容渲染的原因；手动开启时为空。
    pub render_compat_reason: String,
    /// 没有设置文件时为假，用来打开首次引导。文件里没有这个键时视为已经用过。
    pub first_run_done: bool,
    /// 刚跑完的会话再留几分钟。报错不看这个时间。
    pub session_keep_mins: u32,
    /// 会话读取器开关，每一位对应 `agent_session::Source`。
    pub session_sources: u8,
}

impl Default for Settings {
    fn default() -> Self {
        Self { display_id: String::new(), skin: Skin::Classic, island_shape: IslandShape::Notch, glass_blur: true, island_size: 100, island_width: ISLAND_WIDTH_DEFAULT, island_offset: 0, oled_shift: false, dock: Dock::Top, settings_theme: Default::default(), pet: true, pet_place: PetPlace::Island, minimal: false, browser_minimal: false, avoid_browsers: true, avoid_apps: Vec::new(), visible_pages: [true; PAGE_COUNT], page_order: [0,1,2,3,4,5,6,7,8,9,10], show_clock: true, show_battery: true, show_weather: true, weather_scene: true, show_cpu: false, show_mem: false, offwork: false, offwork_start: 9 * 60, offwork_end: 18 * 60, offwork_weekdays: true, offwork_soon: 30, offwork_own_faces: false, notification_dwell_secs: 4.5, clock_12h: false, clock_period: 0, clock_seconds: false, resident_order: Vec::new(), system_hud: true, bluetooth_notifications: true, read_notifications:false, wechat_notifications:false, wecom_notifications:false, feishu_notifications:false, dingtalk_notifications:false, wechat_tray_open:false, qq_only:false, notification_preview:false, notifications: true, sound: false, lyrics_enabled:true, island_lyrics:false, lyric_delay_ms:-350, lyric_translation:true, lyric_align:LyricAlign::Center, lyric_anim:LyricAnim::Scan, spectrum_enabled:true, spectrum_style:SpectrumStyle::Bars, lyric_progress:true, lyric_font_size:12.0, lyric_multiline:false, cover_tint:false, lyric_player_delays:Vec::new(),hotkeys:[String::new(),String::new(),String::new(),String::new(),String::new(),String::new()], hide_fullscreen: true, avoid_top_bar: true, top_bar_on_top: false, focus_minutes: 25, rest_minutes: 5, water_minutes: 45, serving_ml: 250, water_start: 9, water_end: 22, sit_remind: true, sit_minutes: 60, lunch_rest: true, lunch_start: 720, lunch_end: 810, mail_enabled: true, mail_sync_operations:false, mail_code: true, mail_code_hide_seconds:60, mail_interval: 120, mail_ics: 15, ignored_mail_events: Vec::new(), calendar_prompt: true, calendar_auto_sync: true, clipboard_history: true, weather_city: String::new(), weather_position: None, weather_saved: Vec::new(), render_compat: false, render_compat_reason: String::new(), first_run_done: false, session_keep_mins: 10, session_sources: crate::agent_session::Source::ALL_BITS }
    }
}

impl Settings {
    pub fn avoids_top_bar(&self) -> bool { self.avoid_top_bar && !self.top_bar_on_top }

    /// 微信、QQ 和系统通知横幅及其铃铛的停留时间。
    pub fn notification_dwell(&self) -> f64 { self.notification_dwell_secs.clamp(2.0, 10.0) as f64 }

    /// 主窗口（灵动岛）的界面缩放倍率。
    pub fn island_scale(&self) -> f32 {
        island_size_value(self.island_size) as f32 / 100.0
    }

    /// 水平位置，-1..=1。
    pub fn island_offset_ratio(&self) -> f32 {
        self.island_offset.clamp(-ISLAND_OFFSET_MAX, ISLAND_OFFSET_MAX) as f32 / ISLAND_OFFSET_MAX as f32
    }

    pub fn lyric_font(&self)->f32 { self.lyric_font_size.clamp(10.0,18.0) }

    pub fn effective_lyric_delay_ms(&self, source:&str)->i32 {
        let extra=player_delay_for(&self.lyric_player_delays, source);
        (self.lyric_delay_ms+extra).clamp(-5000,5000)
    }

    pub fn set_player_delay(&mut self, source:&str, ms:i32) {
        let key=normalize_player_key(source);
        if key.is_empty(){return;}
        let ms=ms.clamp(-5000,5000);
        if let Some(slot)=self.lyric_player_delays.iter_mut().find(|(s,_)|s.eq_ignore_ascii_case(&key)) {
            if ms==0 { self.lyric_player_delays.retain(|(s,_)|!s.eq_ignore_ascii_case(&key)); }
            else { slot.1=ms; }
        } else if ms!=0 {
            self.lyric_player_delays.push((key,ms));
        }
    }

    pub fn clock_text(&self, clock: &str) -> String {
        use chrono::Timelike;
        let Ok(time) = chrono::NaiveTime::parse_from_str(clock, "%H:%M:%S").or_else(|_|chrono::NaiveTime::parse_from_str(clock,"%H:%M")) else { return if self.clock_seconds {"--:--:--"}else{"--:--"}.into(); };
        let (pm, hour) = time.hour12();
        let mut digits=if self.clock_12h {format!("{}:{:02}",hour,time.minute())}else{time.format("%H:%M").to_string()};
        if self.clock_seconds {digits.push_str(&format!(":{:02}",time.second()));}
        let period=match self.clock_period {
            1=>if pm {"下午"}else{"上午"},
            2=>match time.hour() {0..=5=>"凌晨",6..=10=>"上午",11..=12=>"中午",13..=16=>"下午",17..=18=>"傍晚",_=>"晚上"},
            _=>return digits,
        };
        format!("{period} {digits}")
    }

    pub fn load() -> Self {
        let path = config_dir().join("settings.txt");
        let file_existed = path.is_file();
        let mut saw_first_run = false;
        let mut s = Self::default();
        for (k, v) in read_kv(&path) {
            match k.as_str() {
                "settings_theme" => s.settings_theme=crate::settings_theme::SettingsTheme::parse(&v),
                "display_id" => s.display_id = v,
                "skin" => {
                    // 早期测试版把胶囊当作皮肤保存。
                    if v == "pill" { s.island_shape = IslandShape::Pill; }
                    s.skin = Skin::parse(&v);
                }
                "island_shape" => s.island_shape = IslandShape::parse(&v),
                "dock" => s.dock = Dock::parse(&v),
                "glass_blur" => s.glass_blur = v != "0",
                "island_size" => s.island_size = island_size_value(v.parse::<u32>().unwrap_or(100)),
                "island_width" => s.island_width = island_width_value(v.parse::<u32>().unwrap_or(ISLAND_WIDTH_DEFAULT)),
                "island_offset" => s.island_offset = v.parse::<i32>().unwrap_or(0).clamp(-ISLAND_OFFSET_MAX, ISLAND_OFFSET_MAX),
                "oled_shift" => s.oled_shift = v != "0",
                "pet" => s.pet = v == "1" || v.eq_ignore_ascii_case("true"),
                "pet_place" => s.pet_place = PetPlace::parse(&v),
                "minimal" => s.minimal = v == "1" || v.eq_ignore_ascii_case("true"),
                "browser_minimal" => s.browser_minimal = v == "1",
                "avoid_browsers" => s.avoid_browsers = v != "0",
                "avoid_apps" => s.avoid_apps = serde_json::from_str(&v).unwrap_or_default(),
                "resident_order" => {
                    let mut order:Vec<usize>=v.split(',').filter_map(|n|n.parse().ok()).collect();
                    if order.len()==6 && !order.contains(&crate::bar_layout::WEATHER) {
                        if let Some(clock)=order.iter().position(|slot|*slot==crate::bar_layout::CLOCK){order.insert(clock+1,crate::bar_layout::WEATHER);}
                    }
                    if order.len()==7 && !order.contains(&crate::bar_layout::REMOTE_TODO) {
                        order.push(crate::bar_layout::REMOTE_TODO);
                    }
                    if order.len()==8 && !order.contains(&crate::bar_layout::CPU) && !order.contains(&crate::bar_layout::MEM) {
                        order.push(crate::bar_layout::CPU);
                        order.push(crate::bar_layout::MEM);
                    }
                    if order.len()==10 && !order.contains(&crate::bar_layout::SESSION) {
                        order.push(crate::bar_layout::SESSION);
                    }
                    if order.len()==11 && !order.contains(&crate::bar_layout::OFFWORK) {
                        let at=order.iter().position(|slot|*slot==crate::bar_layout::WEATHER)
                            .or_else(||order.iter().position(|slot|*slot==crate::bar_layout::CLOCK))
                            .map(|index|index+1)
                            .unwrap_or(order.len());
                        order.insert(at,crate::bar_layout::OFFWORK);
                    }
                    if crate::bar_layout::valid_order(&order){s.resident_order=order;}
                }
                "page_order" => {
                    if let Ok(order)=v.split(',').map(str::parse::<usize>).collect::<Result<Vec<_>,_>>() {
                        if order.len()==PAGE_COUNT && (0..PAGE_COUNT).all(|page|order.contains(&page)) {
                            s.page_order.copy_from_slice(&order);
                        } else if order.len()==10 && (0..10).all(|page|order.contains(&page)) {
                            s.page_order[..10].copy_from_slice(&order);
                            s.page_order[10]=10;
                        } else if order.len()==9 && (0..9).all(|page|order.contains(&page)) {
                            s.page_order[..9].copy_from_slice(&order);
                            s.page_order[9]=9;
                        } else if order.len()==8 && (0..8).all(|page|order.contains(&page)) {
                            s.page_order[..8].copy_from_slice(&order);
                            s.page_order[8]=8;
                            s.page_order[9]=9;
                        }
                    }
                }
                "clock_period" => s.clock_period = v.parse::<u8>().ok().filter(|v|*v<=2).unwrap_or(0),
                "clock_12h" => s.clock_12h = v == "1",
                "clock_seconds" => s.clock_seconds = v == "1",
                "show_battery" => s.show_battery = v != "0",
                "show_clock" => s.show_clock = v != "0",
                "show_weather" => s.show_weather = v != "0",
                "weather_scene" => s.weather_scene = v != "0",
                "show_cpu" => s.show_cpu = v == "1",
                "show_mem" => s.show_mem = v == "1",
                "offwork" => s.offwork = v == "1",
                "offwork_start" => s.offwork_start = v.parse::<u32>().unwrap_or(9 * 60).min(1439),
                "offwork_end" => s.offwork_end = v.parse::<u32>().unwrap_or(18 * 60).min(1439),
                "offwork_weekdays" => s.offwork_weekdays = v != "0",
                "offwork_soon" => s.offwork_soon = v.parse::<u32>().unwrap_or(30).clamp(5, 120),
                "offwork_own_faces" => s.offwork_own_faces = v == "1",
                "notification_dwell_secs" => s.notification_dwell_secs = v.parse::<f32>().unwrap_or(4.5).clamp(2.0, 10.0),
                "visible_pages" if (6..=PAGE_COUNT).contains(&v.len()) && v.bytes().all(|b| b==b'0' || b==b'1') => {
                    for (enabled,value) in s.visible_pages.iter_mut().zip(v.bytes()) { *enabled=value==b'1'; }
                    if v.len()<PAGE_COUNT {
                        for enabled in &mut s.visible_pages[v.len()..]{*enabled=v.bytes().any(|b|b==b'1');}
                    }
                }
                "read_notifications" => s.read_notifications=v=="1",
                "wechat_notifications" => s.wechat_notifications=v=="1",
                "wecom_notifications" => s.wecom_notifications=v=="1",
                "feishu_notifications" => s.feishu_notifications=v=="1",
                "dingtalk_notifications" => s.dingtalk_notifications=v=="1",
                "wechat_tray_open" => s.wechat_tray_open=v=="1",
                "qq_only" => s.qq_only=v=="1",
                "notification_preview" => s.notification_preview=v=="1",
                "system_hud" => s.system_hud = v != "0",
                "bluetooth_notifications" => s.bluetooth_notifications = v != "0",
                "notifications" => s.notifications = v != "0",
                "sound" => s.sound = v == "1",
                "lyrics_enabled" => s.lyrics_enabled=v!="0",
                "island_lyrics" => s.island_lyrics=v=="1",
                "lyric_delay_ms" => s.lyric_delay_ms=v.parse::<i32>().unwrap_or(-350).clamp(-5000,5000),
                "lyric_translation" => s.lyric_translation=v!="0",
                "lyric_align" => s.lyric_align=LyricAlign::parse(&v),
                "lyric_anim" => s.lyric_anim=LyricAnim::parse(&v),
                "spectrum_enabled" => s.spectrum_enabled=v!="0",
                "spectrum_style" => s.spectrum_style=SpectrumStyle::parse(&v),
                "lyric_progress" => s.lyric_progress=v!="0",
                "lyric_font_size" => s.lyric_font_size=v.parse::<f32>().unwrap_or(12.0).clamp(10.0,18.0),
                "lyric_multiline" => s.lyric_multiline=v=="1",
                "cover_tint" => s.cover_tint=v=="1",
                "hotkeys" => {
                    let parts:Vec<String>=v.split('\u{1}').map(|part|part.chars().filter(|c|!c.is_control()).take(40).collect()).collect();
                    for (slot,part) in s.hotkeys.iter_mut().zip(parts) { *slot=part; }
                }
                "lyric_player_delays" => s.lyric_player_delays=parse_player_delays(&v),
                "hide_fullscreen" => s.hide_fullscreen = v != "0",
                "avoid_top_bar" => s.avoid_top_bar = v != "0",
                "top_bar_on_top" => s.top_bar_on_top = v == "1",
                "focus_minutes" => s.focus_minutes = v.parse::<u32>().unwrap_or(25).clamp(1, 180),
                "rest_minutes" | "short_minutes" => s.rest_minutes = v.parse::<u32>().unwrap_or(5).clamp(1, 60),
                "water_minutes" => s.water_minutes = v.parse::<u32>().unwrap_or(45).clamp(5, 180),
                "serving_ml" => s.serving_ml = v.parse::<u32>().unwrap_or(250).clamp(50, 1000),
                "water_start" => s.water_start = v.parse::<u32>().unwrap_or(9).min(23),
                "water_end" => s.water_end = v.parse::<u32>().unwrap_or(22).min(24),
                "sit_remind" => s.sit_remind = v != "0",
                "sit_minutes" => s.sit_minutes = v.parse::<u32>().unwrap_or(60).clamp(15, 180),
                "lunch_rest" => s.lunch_rest = v != "0",
                "lunch_start" => s.lunch_start = v.parse::<u32>().unwrap_or(720).min(1439),
                "lunch_end" => s.lunch_end = v.parse::<u32>().unwrap_or(810).min(1439),
                "mail_enabled" => s.mail_enabled = v != "0",
                "mail_sync_operations" => s.mail_sync_operations=v=="1",
                "mail_code" => s.mail_code = v != "0",
                "mail_code_hide_seconds" => s.mail_code_hide_seconds=v.parse::<u32>().unwrap_or(60).clamp(15,300),
                "mail_interval" => s.mail_interval = match v.parse::<u32>().unwrap_or(120) { 60 => 60, 300 => 300, _ => 120 },
                "ignored_mail_events" => s.ignored_mail_events = serde_json::from_str(&v).unwrap_or_default(),
                "mail_ics" => s.mail_ics = match v.parse::<u32>().unwrap_or(15) { 5 => 5, 30 => 30, _ => 15 },
                "calendar_prompt" => s.calendar_prompt = v != "0",
                "calendar_auto_sync" => s.calendar_auto_sync = v != "0",
                "clipboard_history" => s.clipboard_history = v != "0",
                "weather_city" => s.weather_city = v.trim().to_string(),
                "weather_position" => s.weather_position=serde_json::from_str::<Option<crate::weather::Position>>(&v).ok().flatten().filter(|p|p.valid()),
                "weather_saved" => s.weather_saved=serde_json::from_str::<Vec<crate::weather::Position>>(&v).unwrap_or_default().into_iter().filter(|p|p.valid()).collect(),
                "render_compat" => s.render_compat = v == "1",
                "render_compat_reason" => s.render_compat_reason = v.chars().filter(|c| !c.is_control()).take(200).collect(),
                "first_run_done" => { saw_first_run = true; s.first_run_done = v == "1"; }
                "session_keep_mins" => s.session_keep_mins = v.parse::<u32>().unwrap_or(10).clamp(1, 60),
                "session_sources" => s.session_sources = v.parse::<u8>().unwrap_or(crate::agent_session::Source::ALL_BITS) & crate::agent_session::Source::ALL_BITS,
                _ => {}
            }
        }
        if file_existed && !saw_first_run { s.first_run_done = true; }
        if !s.visible_pages.contains(&true) { s.visible_pages[0]=true; }
        // 旧版只有一个指定城市：迁移进已保存列表，升级后下拉框里能直接看到它。
        if let Some(position)=s.weather_position.clone().filter(|p|p.label==s.weather_city) {
            if !s.weather_saved.contains(&position) {s.weather_saved.insert(0,position);}
        }
        s.weather_saved.dedup();s.weather_saved.truncate(crate::weather::MAX_CITIES);
        s
    }

    pub fn save(&self) {
        let _ = write_kv(
            &config_dir().join("settings.txt"),
            &[
                ("display_id".into(),self.display_id.clone()),
                ("skin".into(),self.skin.key().into()),
                ("island_shape".into(),self.island_shape.key().into()),
                ("dock".into(),self.dock.key().into()),
                ("glass_blur".into(),(self.glass_blur as u8).to_string()),
                ("island_size".into(),island_size_value(self.island_size).to_string()),
                ("island_width".into(),island_width_value(self.island_width).to_string()),
                ("island_offset".into(),self.island_offset.clamp(-ISLAND_OFFSET_MAX,ISLAND_OFFSET_MAX).to_string()),
                ("oled_shift".into(),(self.oled_shift as u8).to_string()),
                ("settings_theme".into(),self.settings_theme.key().into()),
                ("pet".into(), if self.pet { "1".to_string() } else { "0".to_string() }),
                ("pet_place".into(), self.pet_place.key().into()),
                ("minimal".into(), if self.minimal { "1".to_string() } else { "0".to_string() }),
                ("browser_minimal".into(), (self.browser_minimal as u8).to_string()),
                ("avoid_browsers".into(), (self.avoid_browsers as u8).to_string()),
                ("avoid_apps".into(), serde_json::to_string(&self.avoid_apps).unwrap_or_default()),
                ("visible_pages".into(), self.visible_pages.iter().map(|enabled|if *enabled {'1'}else{'0'}).collect()),
                ("page_order".into(),self.page_order.iter().map(usize::to_string).collect::<Vec<_>>().join(",")),
                ("resident_order".into(),self.resident_order.iter().map(usize::to_string).collect::<Vec<_>>().join(",")),
                ("clock_period".into(), self.clock_period.to_string()),
                ("clock_12h".into(), (self.clock_12h as u8).to_string()),
                ("clock_seconds".into(), (self.clock_seconds as u8).to_string()),
                ("show_battery".into(), (self.show_battery as u8).to_string()),
                ("show_clock".into(), (self.show_clock as u8).to_string()),
                ("show_weather".into(), (self.show_weather as u8).to_string()),
                ("weather_scene".into(), (self.weather_scene as u8).to_string()),
                ("show_cpu".into(), (self.show_cpu as u8).to_string()),
                ("show_mem".into(), (self.show_mem as u8).to_string()),
                ("offwork".into(), u8::from(self.offwork).to_string()),
                ("offwork_start".into(), self.offwork_start.min(1439).to_string()),
                ("offwork_end".into(), self.offwork_end.min(1439).to_string()),
                ("offwork_weekdays".into(), u8::from(self.offwork_weekdays).to_string()),
                ("offwork_soon".into(), self.offwork_soon.clamp(5, 120).to_string()),
                ("offwork_own_faces".into(), u8::from(self.offwork_own_faces).to_string()),
                ("notification_dwell_secs".into(), format!("{:.1}", self.notification_dwell_secs.clamp(2.0, 10.0))),
                ("read_notifications".into(),(self.read_notifications as u8).to_string()),
                ("wechat_notifications".into(),(self.wechat_notifications as u8).to_string()),
                ("wecom_notifications".into(),(self.wecom_notifications as u8).to_string()),
                ("feishu_notifications".into(),(self.feishu_notifications as u8).to_string()),
                ("dingtalk_notifications".into(),(self.dingtalk_notifications as u8).to_string()),
                ("wechat_tray_open".into(),(self.wechat_tray_open as u8).to_string()),
                ("qq_only".into(),(self.qq_only as u8).to_string()),
                ("notification_preview".into(),(self.notification_preview as u8).to_string()),
                ("system_hud".into(), (self.system_hud as u8).to_string()),
                ("bluetooth_notifications".into(), (self.bluetooth_notifications as u8).to_string()),
                ("notifications".into(), (self.notifications as u8).to_string()),
                ("sound".into(), (self.sound as u8).to_string()),
                ("lyrics_enabled".into(),(self.lyrics_enabled as u8).to_string()),
                ("island_lyrics".into(),(self.island_lyrics as u8).to_string()),
                ("lyric_delay_ms".into(),self.lyric_delay_ms.to_string()),
                ("lyric_translation".into(),(self.lyric_translation as u8).to_string()),
                ("lyric_align".into(),self.lyric_align.key().into()),
                ("lyric_anim".into(),self.lyric_anim.key().into()),
                ("spectrum_enabled".into(),(self.spectrum_enabled as u8).to_string()),
                ("spectrum_style".into(),self.spectrum_style.key().into()),
                ("lyric_progress".into(),(self.lyric_progress as u8).to_string()),
                ("lyric_font_size".into(),format!("{:.1}",self.lyric_font_size.clamp(10.0,18.0))),
                ("lyric_multiline".into(),(self.lyric_multiline as u8).to_string()),
                ("cover_tint".into(),(self.cover_tint as u8).to_string()),
                ("hotkeys".into(),self.hotkeys.join("\u{1}")),
                ("lyric_player_delays".into(),format_player_delays(&self.lyric_player_delays)),
                ("hide_fullscreen".into(), (self.hide_fullscreen as u8).to_string()),
                ("avoid_top_bar".into(), (self.avoid_top_bar as u8).to_string()),
                ("top_bar_on_top".into(), (self.top_bar_on_top as u8).to_string()),
                ("focus_minutes".into(), self.focus_minutes.to_string()),
                ("rest_minutes".into(), self.rest_minutes.to_string()),
                ("water_minutes".into(), self.water_minutes.to_string()),
                ("serving_ml".into(), self.serving_ml.to_string()),
                ("water_start".into(), self.water_start.to_string()),
                ("water_end".into(), self.water_end.to_string()),
                ("sit_remind".into(), (self.sit_remind as u8).to_string()),
                ("sit_minutes".into(), self.sit_minutes.to_string()),
                ("lunch_rest".into(), (self.lunch_rest as u8).to_string()),
                ("lunch_start".into(), self.lunch_start.to_string()),
                ("lunch_end".into(), self.lunch_end.to_string()),
                ("mail_enabled".into(), (self.mail_enabled as u8).to_string()),
                ("mail_sync_operations".into(),(self.mail_sync_operations as u8).to_string()),
                ("mail_code".into(), (self.mail_code as u8).to_string()),
                ("mail_code_hide_seconds".into(), self.mail_code_hide_seconds.to_string()),
                ("mail_interval".into(), self.mail_interval.to_string()),
                ("mail_ics".into(), self.mail_ics.to_string()),
                ("ignored_mail_events".into(), serde_json::to_string(&self.ignored_mail_events).unwrap()),
                ("calendar_prompt".into(), (self.calendar_prompt as u8).to_string()),
                ("calendar_auto_sync".into(), (self.calendar_auto_sync as u8).to_string()),
                ("clipboard_history".into(), (self.clipboard_history as u8).to_string()),
                ("weather_city".into(), self.weather_city.clone()),
                ("weather_position".into(), serde_json::to_string(&self.weather_position).unwrap()),
                ("weather_saved".into(), serde_json::to_string(&self.weather_saved).unwrap()),
                ("render_compat".into(), (self.render_compat as u8).to_string()),
                ("render_compat_reason".into(), self.render_compat_reason.clone()),
                ("first_run_done".into(), (self.first_run_done as u8).to_string()),
                ("session_keep_mins".into(), self.session_keep_mins.clamp(1, 60).to_string()),
                ("session_sources".into(), (self.session_sources & crate::agent_session::Source::ALL_BITS).to_string()),
            ],
        );
    }
}

#[derive(Clone, Debug, Default)]
pub struct TodoItem {
    pub text: String,
    /// 一次性待办：完成日期；每周重复：最近一次完成的日期（只算当天）。YYYY-MM-DD
    pub done_on: Option<String>,
    /// 本地日期与时间：YYYY-MM-DD HH:MM；每周重复只用其中的时间。
    pub due: Option<String>,
    pub reminded: bool,
    /// 每周重复：bit0 = 周一 … bit6 = 周日；0 = 不重复（一次性待办）。
    pub repeat: u8,
    /// 每周重复：当天已提醒的日期，第二天自动重新提醒。
    pub reminded_on: Option<String>,
}

pub const WEEKDAY_NAMES: [&str; 7] = ["一", "二", "三", "四", "五", "六", "日"];
pub const REPEAT_ALL: u8 = 0b111_1111;
pub const REPEAT_WORKDAYS: u8 = 0b001_1111;
pub const REPEAT_WEEKEND: u8 = 0b110_0000;

fn weekday_bit(date: &str) -> Option<u8> {
    use chrono::Datelike;
    let day = chrono::NaiveDate::parse_from_str(date.get(..10)?, "%Y-%m-%d").ok()?;
    Some(1 << day.weekday().num_days_from_monday())
}

/// 「每天」「工作日」「周末」「每周一、三、五」。
pub fn repeat_label(mask: u8) -> String {
    match mask & REPEAT_ALL {
        0 => "不重复".into(),
        REPEAT_ALL => "每天".into(),
        REPEAT_WORKDAYS => "工作日".into(),
        REPEAT_WEEKEND => "周末".into(),
        mask => {
            let days: Vec<&str> = (0..7).filter(|i| mask & (1 << i) != 0).map(|i| WEEKDAY_NAMES[i]).collect();
            format!("每周{}", days.join("、"))
        }
    }
}

/// 列表里的短标签：「每天」「工作日」「周末」「周一三五」。
pub fn repeat_tag(mask: u8) -> String {
    match mask & REPEAT_ALL {
        0 => String::new(),
        REPEAT_ALL => "每天".into(),
        REPEAT_WORKDAYS => "工作日".into(),
        REPEAT_WEEKEND => "周末".into(),
        mask => format!("周{}", (0..7).filter(|i| mask & (1 << i) != 0).map(|i| WEEKDAY_NAMES[i]).collect::<String>()),
    }
}

impl TodoItem {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into(), ..Self::default() }
    }

    pub fn is_repeating(&self) -> bool { self.repeat & REPEAT_ALL != 0 }

    /// 这一天是否要做：一次性待办每天都显示；重复待办只在选中的星期几显示。
    pub fn scheduled_on(&self, today: &str) -> bool {
        !self.is_repeating() || weekday_bit(today).is_some_and(|bit| self.repeat & bit != 0)
    }

    /// 今天是否已完成：重复待办第二天自动变回未完成。
    pub fn done_for(&self, today: &str) -> bool {
        if self.is_repeating() { self.done_on.as_deref() == Some(today) } else { self.done_on.is_some() }
    }

    /// 勾选 / 取消勾选。
    pub fn toggle(&mut self, today: &str) {
        self.done_on = if self.done_for(today) { None } else { Some(today.to_string()) };
    }

    /// `now` 为 `YYYY-MM-DD HH:MM`。一次性待办按完整时间提醒一次；
    /// 重复待办在每个选中的日子到点提醒一次。
    pub fn reminder_due(&self, now: &str) -> bool {
        let Some(due) = self.due.as_deref() else { return false; };
        let today = now.get(..10).unwrap_or("");
        if self.done_for(today) { return false; }
        if !self.is_repeating() { return !self.reminded && due <= now; }
        let (Some(at), Some(clock)) = (due.get(11..16), now.get(11..16)) else { return false; };
        self.scheduled_on(today) && self.reminded_on.as_deref() != Some(today) && at <= clock
    }

    pub fn mark_reminded(&mut self, now: &str) {
        if self.is_repeating() { self.reminded_on = now.get(..10).map(str::to_string); } else { self.reminded = true; }
    }

    /// 悬停提示里的提醒说明。
    pub fn reminder_text(&self) -> Option<String> {
        let due = self.due.as_deref()?;
        Some(if self.is_repeating() { format!("{} {}", repeat_label(self.repeat), due.get(11..16).unwrap_or(due)) } else { due.to_string() })
    }
}

#[derive(Clone, Debug, Default)]
pub struct TodoList {
    pub items: Vec<TodoItem>,
}

impl TodoList {
    pub fn load() -> Self {
        fs::read_to_string(config_dir().join("todo.txt")).map(|text| Self::parse(&text)).unwrap_or_default()
    }

    pub fn parse(text: &str) -> Self {
        let mut list = Self::default();
        {
            for line in text.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                // 格式：<x 或 o>|<完成日期或 ->|正文；也接受纯文本行（视为未完成）
                if !line.contains('|') {
                    list.items.push(TodoItem::new(line.trim()));
                    continue;
                }
                // 每周重复的标记写成 `r<星期位>`（旧版本读到会当作未完成的普通待办）。
                let mut parts = line.splitn(3, '|');
                let flag = parts.next().unwrap_or("o");
                let when = parts.next().unwrap_or("-");
                let raw = parts.next().unwrap_or("").trim();
                let repeat = flag.strip_prefix('r').and_then(|bits| bits.parse::<u8>().ok()).unwrap_or(0) & REPEAT_ALL;
                let (due, reminded_field, text) = if let Some(rest) = raw.strip_prefix("@due:") {
                    let mut fields = rest.splitn(3, '|');
                    let due = fields.next().unwrap_or("").to_string();
                    let reminded = fields.next().unwrap_or("0").to_string();
                    (Some(due), reminded, fields.next().unwrap_or("").to_string())
                } else { (None, "0".to_string(), raw.to_string()) };
                if text.is_empty() {
                    continue;
                }
                let done = (flag == "x" || repeat != 0) && when != "-";
                list.items.push(TodoItem {
                    text, due, repeat,
                    reminded: repeat == 0 && reminded_field == "1",
                    reminded_on: (repeat != 0 && reminded_field.len() == 10).then_some(reminded_field),
                    done_on: done.then(|| when.to_string()),
                });
            }
        }
        list
    }

    pub fn save(&self) {
        let body = self.serialize();
        let path = config_dir().join("todo.txt");
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = atomic_write(&path, body.as_bytes());
    }

    pub fn serialize(&self) -> String {
        self
            .items
            .iter()
            .map(|i| {
                let state = if i.is_repeating() {
                    format!("r{}|{}", i.repeat & REPEAT_ALL, i.done_on.as_deref().unwrap_or("-"))
                } else {
                    i.done_on.as_ref().map(|d| format!("x|{d}")).unwrap_or_else(|| "o|-".into())
                };
                let text = i.text.replace(['\n', '\r'], " ");
                let reminded = if i.is_repeating() { i.reminded_on.clone().unwrap_or_else(|| "0".into()) } else { (i.reminded as u8).to_string() };
                match &i.due {
                    Some(due) => format!("{state}|@due:{due}|{reminded}|{text}\n"),
                    None => format!("{state}|{text}\n"),
                }
            })
            .collect()
    }

    pub fn done_today(&self, today: &str) -> usize {
        self.items
            .iter()
            .filter(|i| i.done_on.as_deref() == Some(today))
            .count()
    }

    /// 今天要做的待办（不在今天的重复待办除外）中，(已完成, 总数)。
    pub fn progress(&self, today: &str) -> (usize, usize) {
        let scheduled = self.items.iter().filter(|i| i.scheduled_on(today));
        let (mut done, mut total) = (0, 0);
        for item in scheduled { total += 1; if item.done_for(today) { done += 1; } }
        (done, total)
    }

    /// 今天未完成在前，其他日期的未完成其次，已完成沉底；原始存储顺序不变。
    pub fn display_order(&self, today: &str) -> Vec<usize> {
        let mut order:Vec<_>=(0..self.items.len()).collect();
        order.sort_by_key(|&i|{let item=&self.items[i];(item.done_for(today),!item.scheduled_on(today))});
        order
    }
}

#[derive(Clone, Debug)]
pub struct Water {
    pub bottle_ml: u32,
    pub goal_ml: u32,
    pub start_day: String,
    pub drunk_ml: u32,
    pub refills: u32,
    /// 每小时饮水量（ml），给饮水页的柱状图用。
    pub hours: [u32; 24],
    /// 定时提醒开关。
    pub remind: bool,
}

impl Default for Water {
    fn default() -> Self {
        Self { bottle_ml: 2000, goal_ml: 2000, start_day: String::new(), drunk_ml: 0, refills: 0, hours: [0; 24], remind: true }
    }
}

impl Water {
    /// `water.txt` 只写原工具已知的三个键；复刻版新增的进度键写到 `water-extra.txt`，
    /// 避免给原工具的文件引入未知键。
    pub fn load(today: &str) -> Self {
        let mut w = Self::default();
        for (k, v) in read_kv(&config_dir().join("water.txt")) {
            match k.as_str() {
                "bottle" => w.bottle_ml = v.parse::<f32>().map(|x| x as u32).unwrap_or(2000),
                "goal" => w.goal_ml = v.parse().unwrap_or(2000),
                "start" => w.start_day = v,
                _ => {}
            }
        }
        for (k, v) in read_kv(&config_dir().join("water-extra.txt")) {
            match k.as_str() {
                "drunk" => w.drunk_ml = v.parse().unwrap_or(0),
                "refills" => w.refills = v.parse().unwrap_or(0),
                "remind" => w.remind = v != "0",
                "hours" => {
                    for (i, x) in v.split(',').take(24).enumerate() {
                        w.hours[i] = x.trim().parse().unwrap_or(0);
                    }
                }
                _ => {}
            }
        }
        w.goal_ml=w.goal_ml.clamp(500,6000);
        w.bottle_ml=w.bottle_ml.clamp(50,6000);
        if w.start_day != today {
            // 跨天重置，与原工具 start= 字段语义一致
            w.start_day = today.to_string();
            w.drunk_ml = 0;
            w.refills = 0;
            w.hours = [0; 24];
            w.save();
        }
        w
    }

    pub fn save(&self) {
        let _ = write_kv(
            &config_dir().join("water.txt"),
            &[
                ("bottle".into(), self.bottle_ml.to_string()),
                ("goal".into(), self.goal_ml.to_string()),
                ("start".into(), self.start_day.clone()),
            ],
        );
        let _ = write_kv(
            &config_dir().join("water-extra.txt"),
            &[
                ("drunk".into(), self.drunk_ml.to_string()),
                ("refills".into(), self.refills.to_string()),
                ("remind".into(), if self.remind { "1".into() } else { "0".into() }),
                (
                    "hours".into(),
                    self.hours.iter().map(|h| h.to_string()).collect::<Vec<_>>().join(","),
                ),
            ],
        );
    }

    /// 喝一口：计入总量与当前小时。
    pub fn drink(&mut self, ml: u32, hour: usize) {
        let total=self.drunk_ml.saturating_add(ml).min(self.goal_ml.max(ml).saturating_mul(4));
        let added=total.saturating_sub(self.drunk_ml);
        self.drunk_ml=total;
        self.hours[hour.min(23)]=self.hours[hour.min(23)].saturating_add(added);
        self.save();
    }

    pub fn percent(&self) -> f32 {
        if self.goal_ml == 0 {
            0.0
        } else {
            (self.drunk_ml as f32 / self.goal_ml as f32 * 100.0).min(100.0)
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Shelf {
    pub items: Vec<PathBuf>,
}

impl Shelf {
    pub fn load() -> Self {
        let mut s = Self::default();
        if let Ok(text) = fs::read_to_string(config_dir().join("shelf.txt")) {
            for line in text.lines() {
                let p = PathBuf::from(line.trim());
                if !line.trim().is_empty() && !s.items.contains(&p) {
                    s.items.push(p);
                }
            }
        }
        s
    }

    pub fn save(&self) {
        let body: String = self
            .items
            .iter()
            .map(|p| format!("{}\n", p.display()))
            .collect();
        let path = config_dir().join("shelf.txt");
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = atomic_write(&path, body.as_bytes());
    }

    /// 原工具的做法：记录文件所在位置，不复制文件本体（官网说明
    /// "the shelf remembers where your files live"）。
    pub fn add(&mut self, src: &Path) -> std::io::Result<()> {
        if !src.exists() { return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "文件不存在")); }
        if !self.items.iter().any(|p| p == src) {
            self.items.push(src.to_path_buf());
            self.save();
        }
        Ok(())
    }

    pub fn remove(&mut self, p: &Path) {
        self.items.retain(|item| item != p);
        self.save();
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.save();
    }
}

pub fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= KB * KB * KB {
        format!("{:.1} GB", b / (KB * KB * KB))
    } else if b >= KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pet_place_roundtrip_defaults_to_the_island() {
        let _guard=crate::ui_tests::ENV_LOCK.lock().unwrap_or_else(|error|error.into_inner());
        let dir=std::env::temp_dir().join(format!("yunyu-pet-place-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir_all(&dir).unwrap();
        std::env::set_var("YUNYU_DATA_DIR",&dir);
        fs::write(dir.join("settings.txt"),"pet=1\n").unwrap();
        assert_eq!(Settings::load().pet_place,PetPlace::Island,"缺省落点是岛上");
        let mut settings=Settings::default();
        settings.pet_place=PetPlace::Tail;
        settings.save();
        assert_eq!(Settings::load().pet_place,PetPlace::Tail);
        settings.pet_place=PetPlace::Island;
        settings.save();
        let text=fs::read_to_string(dir.join("settings.txt")).unwrap();
        assert!(text.lines().any(|line|line=="pet_place=island"),"写回应是 pet_place=island");
        let _=fs::remove_dir_all(dir);
    }

    #[test]
    fn notification_dwell_stays_between_two_and_ten_seconds() {
        let mut settings = Settings::default();
        assert!((settings.notification_dwell() - 4.5).abs() < 0.01);
        settings.notification_dwell_secs = 1.0;
        assert_eq!(settings.notification_dwell(), 2.0);
        settings.notification_dwell_secs = 30.0;
        assert_eq!(settings.notification_dwell(), 10.0);
    }

    #[test]
    fn island_appearance_values_are_normalized() {
        assert_eq!(island_size_value(100), 100);
        assert_eq!(island_size_value(10), ISLAND_SIZE_MIN);
        assert_eq!(island_size_value(999), ISLAND_SIZE_MAX);
        assert_eq!(island_size_value(113), 115);
        assert_eq!(island_size_value(112), 110);
        assert_eq!(island_width_value(0), 0);
        assert_eq!(island_width_value(ISLAND_WIDTH_DEFAULT), 0);
        assert_eq!(island_width_value(10), ISLAND_WIDTH_MIN);
        assert_eq!(island_width_value(9999), ISLAND_WIDTH_MAX);
        assert_eq!(island_width_value(403), 400);
        assert_eq!(island_width_value(405), 408);
        assert_eq!(Settings::default().island_width, 0);
        for skin in Skin::ALL { assert_eq!(Skin::parse(skin.key()), skin); }
        assert_eq!(Skin::parse("unknown"), Skin::Classic);
        assert_eq!(Skin::parse("glass"), Skin::GlassDark, "旧版本保存的毛玻璃即深色毛玻璃");
        for shape in IslandShape::ALL { assert_eq!(IslandShape::parse(shape.key()), shape); }
        assert_eq!(Settings::default().island_shape, IslandShape::Notch);
        let mut s = Settings::default();
        assert_eq!((s.skin, s.glass_blur, s.island_scale(), s.island_offset_ratio()), (Skin::Classic, true, 1.0, 0.0));
        s.island_size = 125; s.island_offset = -250;
        assert_eq!((s.island_scale(), s.island_offset_ratio()), (1.25, -1.0));
    }

    #[test]
    fn quality_save_preserves_neighbor_and_cleans_failed_writes() {
        let dir=std::env::temp_dir().join(format!("yunyu-quality-save-{}",std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path=dir.join("settings.txt");let neighbor=dir.join("settings.tmp");
        fs::write(&neighbor,b"other data").unwrap();
        let saved=atomic_write(&path,b"settings");
        let untouched=fs::read(&neighbor).ok()==Some(b"other data".to_vec());
        let blocked=dir.join("blocked.txt");fs::create_dir_all(&blocked).unwrap();
        assert!(atomic_write(&blocked,b"cannot replace directory").is_err());
        let leftovers=fs::read_dir(&dir).unwrap().flatten().filter(|e|e.path()!=path && e.path()!=neighbor && e.path()!=blocked).count();
        let failed_tmp=blocked.with_extension("tmp").exists();
        for entry in fs::read_dir(&dir).unwrap().flatten(){if entry.file_type().unwrap().is_file(){fs::remove_file(entry.path()).unwrap();}}
        fs::remove_dir(blocked).unwrap();fs::remove_dir(dir).unwrap();
        assert!(saved.is_ok() && untouched,"保存不能覆盖或移走同名 .tmp 文件");
        assert!(!failed_tmp && leftovers==0,"失败的保存应清除本次临时文件");
    }

    #[test]
    fn atomic_write_replaces_existing_settings() {
        let dir=std::env::temp_dir().join(format!("yunyu-save-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir_all(&dir).unwrap();
        let path=dir.join("settings.txt");
        atomic_write(&path,b"focus_minutes=25\n").unwrap();
        atomic_write(&path,b"focus_minutes=26\n").unwrap();
        assert_eq!(fs::read(&path).unwrap(),b"focus_minutes=26\n");
        assert!(!path.with_extension("tmp").exists(),"保存后不应遗留临时配置");
        fs::remove_file(path).unwrap();
        fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn quality_concurrent_saves_publish_one_complete_value() {
        let dir=std::env::temp_dir().join(format!("yunyu-quality-concurrent-{}",std::process::id()));fs::create_dir_all(&dir).unwrap();
        let path=dir.join("cache.lrc");let gate=std::sync::Arc::new(std::sync::Barrier::new(4));
        let workers:Vec<_>=(0..4u8).map(|byte|{let path=path.clone();let gate=gate.clone();std::thread::spawn(move||{gate.wait();atomic_write(&path,&vec![byte;128*1024])})}).collect();
        for worker in workers{worker.join().unwrap().unwrap();}
        let content=fs::read(&path).unwrap();assert_eq!(content.len(),128*1024);assert!(content.iter().all(|b|*b==content[0]));
        assert_eq!(fs::read_dir(&dir).unwrap().count(),1);fs::remove_file(path).unwrap();fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn weekly_todos_round_trip_and_reset_each_day() {
        // 2026-11-02 是周一。
        let mut weekly = TodoItem::new("周会 | 准备材料");
        weekly.repeat = 0b000_0101; // 周一、周三
        weekly.due = Some("2026-11-02 09:30".into());
        weekly.toggle("2026-11-02");
        weekly.reminded_on = Some("2026-11-02".into());
        let once = TodoItem { done_on: Some("2026-11-01".into()), reminded: true, due: Some("2026-11-01 08:00".into()), ..TodoItem::new("一次性") };
        let list = TodoList { items: vec![weekly.clone(), once.clone()] };
        let loaded = TodoList::parse(&list.serialize());
        assert_eq!(loaded.items.len(), 2);
        let w = &loaded.items[0];
        assert_eq!((w.text.as_str(), w.repeat, w.done_on.as_deref(), w.reminded_on.as_deref()), ("周会 | 准备材料", 0b101, Some("2026-11-02"), Some("2026-11-02")));
        assert!(w.done_for("2026-11-02") && !w.done_for("2026-11-04"), "重复待办第二天自动变回未完成");
        assert!(w.scheduled_on("2026-11-04") && !w.scheduled_on("2026-11-03"));
        assert!(!w.reminder_due("2026-11-02 10:00"), "当天已提醒");
        assert!(!w.reminder_due("2026-11-04 09:00") && w.reminder_due("2026-11-04 09:30"));
        assert!(!w.reminder_due("2026-11-03 10:00"), "不在选中的星期几不提醒");
        let o = &loaded.items[1];
        assert_eq!((o.repeat, o.reminded, o.done_on.as_deref()), (0, true, Some("2026-11-01")));
        assert!(o.done_for("2026-11-09") && o.scheduled_on("2026-11-03"));
        assert_eq!(loaded.progress("2026-11-03"), (1, 1));
        assert_eq!(loaded.progress("2026-11-04"), (1, 2));
        assert_eq!(loaded.display_order("2026-11-03"), vec![0, 1],"已完成事项应排在其他日期的未完成事项后面");
        assert_eq!(repeat_label(REPEAT_WORKDAYS), "工作日");
        assert_eq!(repeat_label(0b101_0101), "每周一、三、五、日");
        assert_eq!(repeat_tag(0b001_0101), "周一三五");
    }

    #[test]
    fn human_size_formats_units() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(1536 * 1024), "1.5 MB");
        assert_eq!(human_size(2 * 1024 * 1024 * 1024), "2.0 GB");
    }

    #[test]
    fn water_percent_clamps_at_100() {
        let w = Water { drunk_ml: 1000, ..Water::default() };
        assert!((w.percent() - 50.0).abs() < 0.01);
        let w = Water { drunk_ml: 5000, ..w };
        assert!((w.percent() - 100.0).abs() < 0.01);
    }
}

/// 仅保存应用标识与开关，不保存通知内容。
#[derive(Clone,Debug,Default)]
pub struct NotificationApps {pub items:Vec<(String,String,bool)>}
impl NotificationApps {
    pub fn load()->Self {
        let path=config_dir().join("notification-apps.json");
        let Ok(raw)=fs::read(path) else{return Self::default()};
        let Ok(json)=serde_json::from_slice::<serde_json::Value>(&raw) else{return Self::default()};
        let items=json.as_array().into_iter().flatten().filter_map(|a|Some((a.get("id")?.as_str()?.to_owned(),a.get("name")?.as_str()?.to_owned(),a.get("enabled")?.as_bool()?))).collect();Self{items}
    }
    pub fn save(&self){
        let list:Vec<_>=self.items.iter().map(|(id,name,enabled)|serde_json::json!({"id":id,"name":name,"enabled":enabled})).collect();
        let path=config_dir().join("notification-apps.json");if let Some(parent)=path.parent(){let _=fs::create_dir_all(parent);}if let Ok(data)=serde_json::to_vec(&list){let _=atomic_write(&path,&data);}
    }
    pub fn observe(&mut self,id:&str,name:&str)->bool{
        if let Some(item)=self.items.iter().find(|a|a.0==id){return item.2;}
        let enabled=true;
        self.items.push((id.to_string(),name.to_string(),enabled));self.save();enabled
    }
}
