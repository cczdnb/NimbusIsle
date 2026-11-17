//! 收起态媒体条：按 NotchPeninsula `DrawMediaInline` / `MeasureMediaBlockWidth`
//! 在 Rust 中重写（不搬原 C#，也不捆绑 NpsMediaEnhancement / PlayerCap）。
//!
//! 折叠条：约 22px 圆角封面 + 有同步歌词时只画卡拉 OK 一行（可加译文），
//! 无歌词时才画歌名/歌手；进度条单独在文字下方，不与歌手行叠在一起。
//! 未悬停右侧频谱（可关、可选样式），悬停才出现圆形上一曲 / 播放 / 下一曲。

use crate::app::WIN_W;
use crate::lyric_enhance::{self, LyricWord};
use crate::model::{LyricAlign, LyricAnim, SpectrumStyle};
use crate::paint::{self, Card, H};
use crate::theme::{self, W};
use eframe::egui::{self, Color32, Context, Pos2, Rect};

pub const THUMB: f32 = 22.0;
pub const THUMB_MEASURE: f32 = 30.0;
pub const SPECTRUM_W: f32 = 21.2;
pub const TEXT_TAIL: f32 = 45.0 + 8.0;
pub const BUTTON_BLOCK_LEFT: f32 = 92.0;
pub const SPECTRUM_RESERVE: f32 = 41.0;
pub const PAD: f32 = 22.0;
pub const GAP: f32 = 16.0;
const TITLE_SIZE: f32 = 12.0;
const ARTIST_SIZE: f32 = 10.0;
const TRANS_SCALE: f32 = 0.85;
const CAP: f32 = WIN_W - 32.0;

pub struct IslandPlan {
    pub width: f32,
    pub show_clock: bool,
    pub show_weather: bool,
    pub media_w: f32,
}

pub fn measure_text(ctx: &Context, text: &str, size: f32, weight: W) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    ctx.fonts(|f| {
        f.layout_no_wrap(text.to_owned(), theme::font(size, weight), Color32::WHITE)
            .size()
            .x
    })
}

pub fn measure_clock(ctx: &Context, text: &str, size: f32) -> f32 {
    match text.split_once(' ') {
        Some((period, digits)) => {
            measure_text(ctx, period, size * 0.72, W::Semibold)
                + 4.0
                + measure_text(ctx, digits, size, W::Bold)
        }
        None => measure_text(ctx, text, size, W::Bold),
    }
}

/// 媒体行高度：为第二行原文、译文和独立进度条留出空间。
pub fn row_height(font: f32, second_line: bool, progress: bool, original_second: bool) -> f32 {
    let top = 4.0;
    let main = font + 4.0;
    let second = if original_second { font + 4.0 } else { 0.0 }
        + if second_line { font * TRANS_SCALE + 2.0 } else { 0.0 };
    let prog = if progress { 8.0 } else { 3.0 };
    let cap = if original_second { 72.0 } else { 56.0 };
    (top + main + second + prog).clamp(30.0, cap)
}

pub fn measure_block_width(
    ctx: &Context,
    title: &str,
    artist: &str,
    lyric: Option<&str>,
    trans: &str,
    font: f32,
    spectrum: bool,
) -> f32 {
    let lyric_size = font;
    // 歌词和译文的测量宽度封顶，长句只在槽内滚动，不能把岛宽撑开。
    let cap = 220.0;
    let mut text_w = if let Some(line) = lyric.filter(|s| !s.is_empty()) {
        measure_text(ctx, line, lyric_size, W::Semibold).min(cap)
    } else {
        measure_text(ctx, title, TITLE_SIZE.max(font * 0.95), W::Semibold)
            .max(measure_text(ctx, artist, ARTIST_SIZE, W::Regular))
    };
    if !trans.is_empty() {
        text_w = text_w.max(measure_text(ctx, trans, lyric_size * TRANS_SCALE, W::Regular).min(cap));
    }
    let tail = if spectrum { TEXT_TAIL } else { 16.0 };
    THUMB_MEASURE + text_w + tail
}

pub fn measure_island(media_w: f32, clock_w: f32, weather_w: f32) -> IslandPlan {
    let media_w = media_w.max(80.0);
    let cap = CAP;
    let only = (PAD + media_w + PAD).min(cap);
    if PAD + media_w + PAD > cap {
        return IslandPlan {
            width: only,
            show_clock: false,
            show_weather: false,
            media_w: (cap - PAD - PAD).max(80.0),
        };
    }
    let with_clock = PAD + clock_w + GAP + media_w + PAD;
    if clock_w > 0.0 && with_clock <= cap {
        let with_both = with_clock - PAD + GAP + weather_w + PAD;
        if weather_w > 0.0 && with_both <= cap {
            return IslandPlan {
                width: with_both,
                show_clock: true,
                show_weather: true,
                media_w,
            };
        }
        return IslandPlan {
            width: with_clock,
            show_clock: true,
            show_weather: false,
            media_w,
        };
    }
    IslandPlan {
        width: only,
        show_clock: false,
        show_weather: false,
        media_w,
    }
}

pub fn truncate_to_width(c: &Card, text: &str, size: f32, weight: W, max_w: f32) -> String {
    if text.is_empty() || max_w <= 8.0 {
        return text.to_owned();
    }
    if c.text_w(text, size, weight) <= max_w {
        return text.to_owned();
    }
    let ell = "…";
    let ew = c.text_w(ell, size, weight);
    if ew >= max_w {
        return ell.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        let s: String = chars[..mid].iter().collect();
        if c.text_w(&s, size, weight) + ew <= max_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    if lo == 0 {
        ell.to_owned()
    } else {
        chars[..lo].iter().collect::<String>() + ell
    }
}

pub struct InlinePaint<'a> {
    pub title: &'a str,
    pub artist: &'a str,
    pub lyric: Option<&'a str>,
    pub prev_lyric: Option<&'a str>,
    pub trans: &'a str,
    pub words: &'a [LyricWord],
    pub karaoke: f32,
    pub fade: f32,
    pub anim: LyricAnim,
    pub align: LyricAlign,
    pub font: f32,
    pub timeline: Option<f32>,
    pub position: f64,
    pub duration: f64,
    pub show_progress: bool,
    pub spectrum: Option<SpectrumStyle>,
    pub playing: bool,
    pub artwork: Option<([usize; 2], egui::TextureId)>,
    pub time: f64,
    /// 当前句出现后经过的秒数。换句时归零，长句横滑从头开始。
    /// 当前句出现后经过的秒数。换句时归零，长句横滑从头开始。
    pub line_age: f32,
    /// 原文行数。1 为单行横滑；2 先换到第二行，仍超出槽宽的再横滑。
    pub rows: usize,
}

#[derive(PartialEq)]
pub enum InlineAction {
    Prev,
    Toggle,
    Next,
    Seek(f64),
    /// 双击封面：回到播放器。单击封面不产生这个动作，也不跳进度。
    Activate,
}

fn mmss(secs: f64) -> String {
    let s = secs.max(0.0).round() as u32;
    format!("{:02}:{:02}", s / 60, s % 60)
}

fn text_x(align: LyricAlign, left: f32, width: f32) -> f32 {
    match align {
        LyricAlign::Left => left,
        LyricAlign::Center => left + width * 0.5,
    }
}

fn text_h(align: LyricAlign) -> H {
    match align {
        LyricAlign::Left => H::Left,
        LyricAlign::Center => H::Center,
    }
}

fn paint_spectrum(c: &Card, style: SpectrumStyle, x: f32, height: f32, playing: bool, time: f64) {
    let center_y = height / 2.0;
    match style {
        SpectrumStyle::Bars => {
            let bar_w = 2.0;
            let spacing = 2.8;
            let max_h = 14.0;
            for i in 0..5 {
                let phase = if playing {
                    time as f32 * (5.0 + i as f32 * 0.7)
                } else {
                    0.0
                } + i as f32 * 1.6;
                let h = if playing {
                    (2.0 + (phase.sin() * 0.5 + 0.5) * max_h).max(2.0)
                } else {
                    2.0
                };
                let y = (height - h) / 2.0;
                c.fill(x + i as f32 * (bar_w + spacing), y, bar_w, h, 1.5, theme::WHITE);
            }
        }
        SpectrumStyle::Mirror => {
            let bar_w = 2.0;
            let spacing = 2.8;
            for i in 0..5 {
                let phase = if playing {
                    time as f32 * (5.0 + i as f32 * 0.7)
                } else {
                    0.0
                } + i as f32 * 1.6;
                let h = if playing {
                    (2.0 + (phase.sin() * 0.5 + 0.5) * 6.5).max(1.5)
                } else {
                    1.5
                };
                let bx = x + i as f32 * (bar_w + spacing);
                c.fill(bx, center_y - 1.0 - h, bar_w, h, 1.0, theme::WHITE);
                c.fill(bx, center_y + 1.0, bar_w, h, 1.0, Color32::from_white_alpha(170));
            }
        }
        SpectrumStyle::Dots => {
            for i in 0..5 {
                let phase = if playing {
                    time as f32 * (5.0 + i as f32 * 0.7)
                } else {
                    0.0
                } + i as f32 * 1.6;
                let off = if playing { (phase.sin() * 0.5 + 0.5) * 6.0 } else { 0.0 };
                c.circle(x + 1.0 + i as f32 * 4.8, center_y - off + 3.0, 1.8, theme::WHITE);
            }
        }
        SpectrumStyle::Wave => {
            let w = SPECTRUM_W + 4.0;
            let n = 16;
            let mut prev: Option<(f32, f32)> = None;
            for i in 0..n {
                let t = i as f32 / (n - 1) as f32;
                let phase = if playing { time as f32 * 4.2 + t * 6.2 } else { t * 3.0 };
                let amp = if playing { 6.0 } else { 1.2 };
                let px = x + t * w;
                let py = center_y + phase.sin() * amp;
                if let Some(p) = prev {
                    c.seg(p, (px, py), 1.3, theme::WHITE);
                }
                prev = Some((px, py));
            }
        }
    }
}

pub fn paint_inline(
    c: &Card,
    content_left: f32,
    anchor_right: f32,
    height: f32,
    hovered: bool,
    data: &InlinePaint,
) -> Option<InlineAction> {
    let mut x = content_left;
    let center_y = height / 2.0;

    let thumb_x = x;
    let thumb_y = (height - THUMB) / 2.0;
    if let Some(([iw, ih], id)) = data.artwork {
        let uv_size = if iw > ih {
            egui::vec2(ih as f32 / iw as f32, 1.0)
        } else {
            egui::vec2(1.0, iw as f32 / ih as f32)
        };
        let uv = Rect::from_center_size(Pos2::new(0.5, 0.5), uv_size);
        let r = c.rect(thumb_x, thumb_y, THUMB, THUMB);
        c.painter.add(
            egui::epaint::RectShape::filled(r, 5.0 * c.s, Color32::WHITE).with_texture(id, uv),
        );
    } else {
        c.fill(thumb_x, thumb_y, THUMB, THUMB, 5.0, theme::CARD);
        paint::note(c, thumb_x + THUMB / 2.0, center_y, 12.0, theme::WHITE);
    }
    let cover = c.hit(thumb_x, thumb_y, THUMB, THUMB, "inline-cover");
    x += THUMB + 8.0;

    let right_reserve = if hovered {
        BUTTON_BLOCK_LEFT
    } else if data.spectrum.is_some() {
        SPECTRUM_RESERVE
    } else {
        8.0
    };
    let text_clip_right = anchor_right - right_reserve - 4.0;
    let text_left = x;
    let text_max_w = (text_clip_right - text_left).max(20.0);

    let progress_band = if data.show_progress && data.timeline.is_some() { 8.0 } else { 0.0 };
    let text_h_area = (height - progress_band).max(16.0);
    let clip_w = (text_clip_right - text_left).max(4.0);
    let clipped = c.clipped(text_left, 0.0, clip_w, text_h_area);

    let lyric_size = data.font.clamp(10.0, 18.0);
    let has_lyric = data.lyric.filter(|s| !s.is_empty());
    if let Some(line) = has_lyric {
        let has_trans = !data.trans.is_empty();
        let trans_h = if has_trans { lyric_size * TRANS_SCALE + 2.0 } else { 0.0 };
        let rows = data.rows.max(1) as f32;
        let block = lyric_size * rows + trans_h;
        let top = ((text_h_area - block) * 0.5).max(1.0);
        let text_y = top;
        lyric_enhance::paint_lyric_line(
            &clipped.painter,
            clipped.pos(text_left, text_y),
            line,
            data.prev_lyric,
            clipped.font(lyric_size, W::Semibold),
            Color32::from_gray(150),
            theme::WHITE,
            data.karaoke,
            data.fade,
            data.anim,
            text_max_w * clipped.s,
            data.rows.max(1),
            data.align,
            data.line_age,
        );
        if has_trans {
            // 与歌词一样在槽内裁剪，不把译文改写成省略号（宽度已由岛体固定）。
            clipped.text(
                text_x(data.align, text_left, text_max_w),
                top + lyric_size + lyric_size * TRANS_SCALE + 1.0,
                data.trans,
                lyric_size * TRANS_SCALE,
                W::Regular,
                Color32::from_gray(165),
                text_h(data.align),
            );
        }
    } else {
        // Only title+artist when there are no synced lyrics.
        let title_sz = TITLE_SIZE.max(lyric_size * 0.95).min(lyric_size);
        let artist_sz = ARTIST_SIZE;
        let title = truncate_to_width(&clipped, data.title, title_sz, W::Semibold, text_max_w);
        let artist = truncate_to_width(&clipped, data.artist, artist_sz, W::Regular, text_max_w);
        let has_artist = !artist.is_empty();
        let block_h = if has_artist { title_sz + 3.0 + artist_sz } else { title_sz };
        let title_top = ((text_h_area - block_h) * 0.5).max(2.0);
        // Card::text y is baseline: top + approximate ascender.
        let title_base = title_top + title_sz * 0.82;
        let artist_base = title_base + artist_sz + 3.0;
        let hx = text_x(data.align, text_left, text_max_w);
        let hh = text_h(data.align);
        clipped.text(hx, title_base, &title, title_sz, W::Semibold, theme::WHITE, hh);
        if has_artist {
            // Artist stays inside the text band, clearly above the progress strip.
            clipped.text(hx, artist_base, &artist, artist_sz, W::Regular, theme::GRAY, hh);
        }
    }

    let mut action = if cover.double_clicked() { Some(InlineAction::Activate) } else { None };
    if data.show_progress {
        if let Some(progress) = data.timeline {
            let prog_y = height - 4.5;
            let prog_l = text_left;
            let prog_r = text_clip_right.min(text_left + text_max_w);
            if prog_r > prog_l + 12.0 {
                let w = prog_r - prog_l;
                let bar = c.hit_drag(prog_l - 2.0, prog_y - 6.0, w + 4.0, 14.0, "inline-seek");
                let preview = if data.duration > 0.1
                    && (bar.is_pointer_button_down_on() || bar.clicked() || bar.drag_stopped())
                {
                    bar.interact_pointer_pos()
                        .or_else(|| c.ui.input(|i| i.pointer.interact_pos()))
                        .map(|pointer| {
                            ((c.to_local(pointer).x - prog_l) / w).clamp(0.0, 1.0) as f64
                                * data.duration
                        })
                } else {
                    None
                };
                let frac = if let Some(secs) = preview {
                    (secs / data.duration).clamp(0.0, 1.0) as f32
                } else {
                    progress.clamp(0.0, 1.0)
                };
                c.fill(prog_l, prog_y, w, 2.0, 1.0, Color32::from_white_alpha(40));
                let fw = (w * frac).max(if frac > 0.0 { 2.0 } else { 0.0 });
                if fw > 0.5 {
                    c.fill(prog_l, prog_y, fw, 2.0, 1.0, theme::WHITE);
                }
                if bar.is_pointer_button_down_on() {
                    if let Some(secs) = preview {
                        c.text(
                            prog_l + w * frac,
                            prog_y - 6.0,
                            &mmss(secs),
                            8.0,
                            W::Semibold,
                            theme::WHITE,
                            H::Center,
                        );
                    }
                }
                if action.is_none() && (bar.clicked() || bar.drag_stopped()) && data.duration > 0.1 {
                    if let Some(secs) = preview {
                        action = Some(InlineAction::Seek(secs));
                    }
                }
            }
        }
    }

    if hovered {
        let cy = height / 2.0;
        let r = 11.0;
        let play_r = 13.0;
        let gap = 28.0;
        let cx_play = anchor_right - 30.0;
        let cx_prev = cx_play - gap;
        let cx_next = cx_play + gap;
        let circle = Color32::from_white_alpha(30);
        c.circle(cx_prev, cy, r, circle);
        c.circle(cx_play, cy, play_r, circle);
        c.circle(cx_next, cy, r, circle);
        let prev = c.hit(cx_prev - r, cy - r, r * 2.0, r * 2.0, "inline-prev");
        paint::skip(c, cx_prev, cy, -1.0, theme::WHITE);
        let play = c.hit(
            cx_play - play_r,
            cy - play_r,
            play_r * 2.0,
            play_r * 2.0,
            "inline-play",
        );
        paint::play_pause(
            c,
            cx_play,
            cy,
            11.0,
            if data.playing { 1.0 } else { 0.0 },
            theme::WHITE,
        );
        let next = c.hit(cx_next - r, cy - r, r * 2.0, r * 2.0, "inline-next");
        paint::skip(c, cx_next, cy, 1.0, theme::WHITE);
        if action.is_none() && prev.clicked() {
            action = Some(InlineAction::Prev);
        }
        if action.is_none() && play.clicked() {
            action = Some(InlineAction::Toggle);
        }
        if action.is_none() && next.clicked() {
            action = Some(InlineAction::Next);
        }
    } else if let Some(style) = data.spectrum {
        let spectrum_x = anchor_right - 16.0 - SPECTRUM_W;
        paint_spectrum(c, style, spectrum_x, height, data.playing, data.time);
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_height_grows_for_second_line_and_progress() {
        let base = row_height(12.0, false, false, false);
        assert!((base - 30.0).abs() < 0.1);
        let with = row_height(12.0, true, true, false);
        assert!(with > base + 6.0);
        assert!(with <= 56.0);
        let two = row_height(12.0, false, false, true);
        assert!(two > base + 8.0);
        assert!(two <= 72.0);
    }
}
