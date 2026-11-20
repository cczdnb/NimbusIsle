//! 天气页上的两段场景：落雨和落雪。
//! 素材来自 LottieFiles，许可证是 Lottie Simple License，见 `assets/weather/LICENSE.txt`。
//! 预览图自带蓝色底，解码时把蓝底抠掉，只留下雨丝和雪花。
use crate::paint::Card;
use crate::weather::Kind;
use eframe::egui::{self, Color32};
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

pub(crate) fn paint(c: &Card, kind: Kind, t: f32, area: (f32, f32, f32, f32)) {
    let snow = kind == Kind::Snow;
    let clip = clip(snow);
    let index = frame_index(clip, t);
    let frame = &clip.frames[index];
    let image = egui::ColorImage::from_rgba_unmultiplied(
        [frame.width() as usize, frame.height() as usize],
        frame.as_raw(),
    );
    let id = egui::Id::new(("weather-scene-frame", snow));
    let ctx = c.ui.ctx();
    let texture = ctx.data(|data| data.get_temp::<(usize, egui::TextureHandle)>(id));
    let texture = if let Some((shown, mut texture)) = texture {
        if shown != index {
            texture.set(image, egui::TextureOptions::LINEAR);
            ctx.data_mut(|data| data.insert_temp(id, (index, texture.clone())));
        }
        texture
    } else {
        let texture = ctx.load_texture(if snow { "weather-snow" } else { "weather-rain" }, image, egui::TextureOptions::LINEAR);
        crate::skin_paint::mark_colored(texture.id());
        ctx.data_mut(|data| data.insert_temp(id, (index, texture.clone())));
        texture
    };
    let (x, y, w, h) = area;
    let scene = c.clipped(x, y, w, h);
    scene.painter.image(
        texture.id(),
        scene.rect(x, y, w, h),
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        Color32::WHITE,
    );
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
