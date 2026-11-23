//! 落雨和落雪：天气页上铺一层，岛体里也铺一层。
//! 素材来自 LottieFiles，许可证是 Lottie Simple License，见 `assets/weather/LICENSE.txt`。
//! 预览图自带蓝色底，解码时把蓝底抠掉，只留下雨丝和雪花。
use crate::model::Dock;
use crate::paint::Card;
use crate::shape::{self, EdgeMap};
use crate::weather::Kind;
use eframe::egui::{self, Color32, Pos2, Rect};
use image::AnimationDecoder;
use std::io::Cursor;
use std::sync::OnceLock;

const RAIN: &[u8] = include_bytes!("../assets/weather/rain.gif");
const SNOW: &[u8] = include_bytes!("../assets/weather/snow.gif");

struct Clip {
    frames: Vec<image::RgbaImage>,
    delays: Vec<f32>,
    total: f32,
}

fn keyed(src: image::RgbaImage) -> image::RgbaImage {
    let mut out = src;
    for pixel in out.pixels_mut() {
        let (r, g, b) = (pixel[0], pixel[1], pixel[2]);
        // 蓝底：蓝色明显压过红绿。雨丝和雪花是浅色，红通道不会这么低。
        if b > r.saturating_add(18) && b > g.saturating_add(8) && r < 150 && g < 190 {
            *pixel = image::Rgba([0, 0, 0, 0]);
        } else {
            pixel[3] = 230;
        }
    }
    image::imageops::resize(&out, 256, 256, image::imageops::FilterType::Triangle)
}

fn decode(bytes: &'static [u8]) -> Clip {
    let decoder = image::codecs::gif::GifDecoder::new(Cursor::new(bytes)).expect("天气动效应能解码");
    let mut frames = Vec::new();
    let mut delays = Vec::new();
    for frame in decoder.into_frames() {
        let frame = frame.expect("天气动效帧应完整");
        let (num, den) = frame.delay().numer_denom_ms();
        let seconds = if den == 0 { 0.04 } else { num as f32 / den as f32 / 1000.0 };
        delays.push(seconds.max(0.03));
        frames.push(keyed(frame.into_buffer()));
    }
    let total = delays.iter().sum::<f32>().max(0.03);
    Clip { frames, delays, total }
}

fn clip(snow: bool) -> &'static Clip {
    if snow {
        static SNOW_CLIP: OnceLock<Clip> = OnceLock::new();
        SNOW_CLIP.get_or_init(|| decode(SNOW))
    } else {
        static RAIN_CLIP: OnceLock<Clip> = OnceLock::new();
        RAIN_CLIP.get_or_init(|| decode(RAIN))
    }
}

/// 雨、阵雨、雷用落雨；雪用落雪。晴、夜间和雾仍走原来的矢量闪光。
pub(crate) fn covers(kind: Kind) -> bool {
    matches!(kind, Kind::Drizzle | Kind::Rain | Kind::Shower | Kind::Thunder | Kind::Snow)
}

fn frame_index(clip: &Clip, t: f32) -> usize {
    let mut time = t.rem_euclid(clip.total);
    for (index, delay) in clip.delays.iter().enumerate() {
        if time < *delay { return index; }
        time -= delay;
    }
    clip.frames.len() - 1
}

fn texture(ctx: &egui::Context, kind: Kind, t: f32) -> egui::TextureHandle {
    let snow = kind == Kind::Snow;
    let clip = clip(snow);
    let index = frame_index(clip, t);
    let frame = &clip.frames[index];
    let image = egui::ColorImage::from_rgba_unmultiplied(
        [frame.width() as usize, frame.height() as usize],
        frame.as_raw(),
    );
    let id = egui::Id::new(("weather-scene-frame", snow));
    let existing = ctx.data(|data| data.get_temp::<(usize, egui::TextureHandle)>(id));
    if let Some((shown, mut texture)) = existing {
        if shown != index {
            texture.set(image, texture_options());
            ctx.data_mut(|data| data.insert_temp(id, (index, texture.clone())));
        }
        texture
    } else {
        let texture = ctx.load_texture(if snow { "weather-snow" } else { "weather-rain" }, image, texture_options());
        crate::skin_paint::mark_colored(texture.id());
        ctx.data_mut(|data| data.insert_temp(id, (index, texture.clone())));
        texture
    }
}

fn texture_options() -> egui::TextureOptions {
    egui::TextureOptions {
        magnification: egui::TextureFilter::Linear,
        minification: egui::TextureFilter::Linear,
        wrap_mode: egui::TextureWrapMode::Repeat,
        mipmap_mode: None,
    }
}

pub(crate) fn paint(c: &Card, kind: Kind, t: f32, area: (f32, f32, f32, f32)) {
    let texture = texture(c.ui.ctx(), kind, t);
    let (x, y, w, h) = area;
    let scene = c.clipped(x, y, w, h);
    scene.painter.image(
        texture.id(),
        scene.rect(x, y, w, h),
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        Color32::WHITE,
    );
}

/// 岛体轮廓里铺一层。按岛的短边取纹理比例，雨丝保持竖直，不压成横线。
pub(crate) fn paint_island(painter: &egui::Painter, ctx: &egui::Context, kind: Kind, t: f32, rect: Rect, pill: bool, dock: Dock, flare: f32, flare_h: f32, radius: f32) {
    if !covers(kind) || rect.width() < 16.0 || rect.height() < 8.0 { return; }
    let texture = texture(ctx, kind, t);
    let (rows, map, frame) = island_rows(rect, pill, dock, flare, flare_h, radius);
    if rows.len() < 2 { return; }
    let span = frame.height().clamp(96.0, 220.0);
    let mut mesh = egui::Mesh::default();
    mesh.texture_id = texture.id();
    let tint = Color32::from_white_alpha(210);
    for pair in rows.windows(2) {
        let (ya, la, ra) = pair[0];
        let (yb, lb, rb) = pair[1];
        let id = mesh.vertices.len() as u32;
        for (x, y) in [(la, ya), (ra, ya), (rb, yb), (lb, yb)] {
            mesh.vertices.push(egui::epaint::Vertex {
                pos: map.apply(Pos2::new(x, y)),
                uv: egui::pos2((x - frame.left()) / span, (y - frame.top()) / span),
                color: tint,
            });
        }
        mesh.add_triangle(id, id + 1, id + 2);
        mesh.add_triangle(id, id + 2, id + 3);
    }
    painter.add(egui::Shape::mesh(mesh));
}

fn island_rows(rect: Rect, pill: bool, dock: Dock, flare: f32, flare_h: f32, radius: f32) -> (Vec<(f32, f32, f32)>, EdgeMap, Rect) {
    if pill {
        let radius = rect.height().min(rect.width()) * 0.5;
        let mid = rect.center().y;
        let mut rows = Vec::with_capacity(25);
        for i in 0..=24 {
            let y = rect.top() + rect.height() * i as f32 / 24.0;
            let dy = (y - mid).abs().min(radius);
            let inset = radius - (radius * radius - dy * dy).max(0.0).sqrt();
            rows.push((y, rect.left() + inset, rect.right() - inset));
        }
        (rows, EdgeMap::TOP, rect)
    } else {
        let (map, frame) = EdgeMap::of(rect, dock);
        (shape::sections(frame, flare, flare_h, radius), map, frame)
    }
}

#[cfg(test)]
mod tests {
    use super::clip;

    #[test]
    fn scenes_keep_the_flakes_and_remove_the_blue_plate() {
        for snow in [false, true] {
            let clip = clip(snow);
            assert!(clip.frames.len() >= 8, "动效应有完整循环");
            let frame = &clip.frames[0];
            let opaque = frame.pixels().filter(|pixel| pixel[3] > 20).count();
            let clear = frame.pixels().filter(|pixel| pixel[3] == 0).count();
            assert!(opaque > 80, "应留下雨丝或雪花");
            assert!(clear > opaque * 4, "蓝底应被抠掉");
        }
    }
}
