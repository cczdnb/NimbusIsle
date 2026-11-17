//! 歌词增强：逐字时间轴（yrc / krc）与译文第二行。
//! 解析算法按网易云 yrc、酷狗 krc 的公开格式在 Rust 中重写。

use crate::lyrics::{Lyrics, LyricsLine};

/// 一个字（或词）的起止时间，单位秒。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LyricWord {
    pub t: f32,
    pub dur: f32,
    pub text: String,
}

/// 给 `LyricsLine` 附上的增强数据；与行按索引对齐。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LineEnhance {
    pub translation: String,
    pub words: Vec<LyricWord>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LyricsEnhance {
    pub lines: Vec<LineEnhance>,
}

impl LyricsEnhance {
    pub fn for_line(&self, index: usize) -> Option<&LineEnhance> {
        self.lines.get(index)
    }
    pub fn has_words(&self) -> bool {
        self.lines.iter().any(|l| !l.words.is_empty())
    }
    pub fn has_translation(&self) -> bool {
        self.lines.iter().any(|l| !l.translation.is_empty())
    }
}

/// 当前行在 `secs` 时刻的扫光进度：0..1。有逐字时按已唱宽度比例；否则按整行时间。
pub fn scan_progress(line_t: f32, next_t: Option<f32>, words: &[LyricWord], secs: f32) -> f32 {
    if !words.is_empty() {
        let total: f32 = words.iter().map(|w| w.text.chars().count().max(1) as f32).sum();
        if total <= 0.0 { return 0.0; }
        let mut done = 0.0;
        for w in words {
            let weight = w.text.chars().count().max(1) as f32;
            if secs >= w.t + w.dur { done += weight; }
            else if secs > w.t {
                let p = if w.dur > 0.02 { ((secs - w.t) / w.dur).clamp(0.0, 1.0) } else { 1.0 };
                done += weight * p;
                break;
            } else {
                break;
            }
        }
        return (done / total).clamp(0.0, 1.0);
    }
    let end = next_t.unwrap_or(line_t + 5.0);
    if end <= line_t { return if secs >= line_t { 1.0 } else { 0.0 }; }
    ((secs - line_t) / (end - line_t)).clamp(0.0, 1.0)
}

/// 解析网易云 / 同类 yrc。`timing_first` 为 true 时是 `(start,dur,0)文字`；false 时允许 `文字(start,dur,0)`。
pub fn parse_yrc(raw: &str, timing_first: bool) -> Vec<(f32, String, Vec<LyricWord>)> {
    let mut out = Vec::new();
    for raw_line in raw.lines() {
        let line = raw_line.trim();
        if line.is_empty() { continue; }
        if line.starts_with('{') {
            if let Some(row) = parse_yrc_json(line) { out.push(row); }
            continue;
        }
        if let Some(row) = parse_yrc_tagged(line, timing_first) { out.push(row); }
    }
    out
}

fn parse_yrc_json(line: &str) -> Option<(f32, String, Vec<LyricWord>)> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    // {"t":147,"c":[{"tx":"字","dt":240}]}  或 {"t":14700,"ly":[{"c":"字","d":240}]}
    let t_ms = v.get("t")?.as_f64()?;
    // 网易云 t 是毫秒，通常 >= 50。
    let t = if t_ms >= 1000.0 || v.get("t").and_then(|x| x.as_i64()).unwrap_or(0) >= 50 {
        (t_ms as f32) / 1000.0
    } else {
        t_ms as f32
    };
    let cells = v.get("c").or_else(|| v.get("ly")).and_then(|x| x.as_array())?;
    let mut words = Vec::new();
    let mut text = String::new();
    let mut cursor = t;
    for cell in cells {
        let tx = cell.get("tx").or_else(|| cell.get("c")).or_else(|| cell.get("text")).and_then(|x| x.as_str()).unwrap_or("");
        if tx.is_empty() { continue; }
        let dt = cell.get("dt").or_else(|| cell.get("d")).or_else(|| cell.get("duration")).and_then(|x| x.as_f64()).unwrap_or(200.0);
        let dur = (dt as f32) / 1000.0;
        words.push(LyricWord { t: cursor, dur, text: tx.to_string() });
        text.push_str(tx);
        cursor += dur;
    }
    if text.is_empty() { return None; }
    Some((t, text, words))
}

fn parse_yrc_tagged(line: &str, timing_first: bool) -> Option<(f32, String, Vec<LyricWord>)> {
    // [startMs,durationMs](t,d,0)字(t,d,0)词
    if !line.starts_with('[') { return None; }
    let end = line.find(']')?;
    let tag = &line[1..end];
    let mut parts = tag.split(',');
    let start: u32 = parts.next()?.trim().parse().ok()?;
    let _dur: u32 = parts.next().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    let rest = &line[end + 1..];
    let t = start as f32 / 1000.0;
    let mut words = Vec::new();
    let mut text = String::new();
    let bytes = rest.as_bytes();
    let mut i = 0;
    let mut pending_text = String::new();
    while i < rest.len() {
        if bytes[i] == b'(' {
            if let Some((rel, close)) = parse_paren_timing(&rest[i..]) {
                if !timing_first && !pending_text.is_empty() {
                    words.push(LyricWord { t: rel.0, dur: rel.1, text: pending_text.clone() });
                    text.push_str(&pending_text);
                    pending_text.clear();
                }
                i += close;
                if timing_first {
                    // 后面紧跟文字，直到下一个 ( 或行尾
                    let start_txt = i;
                    while i < rest.len() && rest.as_bytes()[i] != b'(' {
                        i += rest[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
                    }
                    let w = rest[start_txt..i].to_string();
                    if !w.is_empty() {
                        words.push(LyricWord { t: rel.0, dur: rel.1, text: w.clone() });
                        text.push_str(&w);
                    }
                }
                continue;
            }
        }
        if !timing_first {
            let ch = rest[i..].chars().next()?;
            pending_text.push(ch);
            i += ch.len_utf8();
        } else {
            // 格式不对时把剩余当整行
            break;
        }
    }
    if !timing_first && !pending_text.is_empty() && words.is_empty() {
        text.push_str(&pending_text);
    }
    if text.is_empty() && !rest.trim().is_empty() {
        // 只有行时间没有逐字
        let plain = strip_timing_tokens(rest);
        if plain.is_empty() { return None; }
        return Some((t, plain, Vec::new()));
    }
    if text.is_empty() { return None; }
    Some((t, text, words))
}

fn parse_paren_timing(s: &str) -> Option<((f32, f32), usize)> {
    if !s.starts_with('(') { return None; }
    let close = s.find(')')?;
    let inner = &s[1..close];
    let mut it = inner.split(',');
    let a: i64 = it.next()?.trim().parse().ok()?;
    let b: i64 = it.next()?.trim().parse().ok()?;
    let t = (a.max(0) as f32) / 1000.0;
    let dur = (b.max(0) as f32) / 1000.0;
    Some(((t, dur), close + 1))
}

fn strip_timing_tokens(s: &str) -> String {
    let mut out = String::new();
    let mut i = 0;
    let b = s.as_bytes();
    while i < s.len() {
        if b[i] == b'(' {
            if let Some((_, n)) = parse_paren_timing(&s[i..]) { i += n; continue; }
        }
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out.trim().to_string()
}

/// 从酷狗/汽水 krc 行里提取逐字：`[start,dur]<off,dur,0>字`
pub fn words_from_krc_line(line: &str) -> Option<(f32, String, Vec<LyricWord>)> {
    if !line.starts_with('[') { return None; }
    let end = line.find(']')?;
    let tag = &line[1..end];
    let mut parts = tag.split(',');
    let start: u32 = parts.next()?.trim().parse().ok()?;
    let _ldur: u32 = parts.next()?.trim().parse().ok()?;
    let rest = &line[end + 1..];
    let t0 = start as f32 / 1000.0;
    let mut words = Vec::new();
    let mut text = String::new();
    let mut i = 0;
    let b = rest.as_bytes();
    while i < rest.len() {
        if b[i] == b'<' {
            if let Some((off, dur, n)) = parse_angle(&rest[i..]) {
                i += n;
                let start_txt = i;
                while i < rest.len() && rest.as_bytes()[i] != b'<' {
                    i += rest[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
                }
                let w = rest[start_txt..i].to_string();
                if !w.is_empty() {
                    words.push(LyricWord { t: t0 + off, dur, text: w.clone() });
                    text.push_str(&w);
                }
                continue;
            }
        }
        let ch = rest[i..].chars().next()?;
        text.push(ch);
        i += ch.len_utf8();
    }
    if text.is_empty() { return None; }
    Some((t0, text, words))
}

fn parse_angle(s: &str) -> Option<(f32, f32, usize)> {
    if !s.starts_with('<') { return None; }
    let close = s.find('>')?;
    let inner = &s[1..close];
    let mut it = inner.split(',');
    let a: i64 = it.next()?.trim().parse().ok()?;
    let b: i64 = it.next()?.trim().parse().ok()?;
    Some(((a.max(0) as f32) / 1000.0, (b.max(0) as f32) / 1000.0, close + 1))
}

/// 把译文 LRC 按时间贴近匹配到已有行。
pub fn attach_translation(enhance: &mut LyricsEnhance, lines: &[LyricsLine], trans: &Lyrics) {
    if enhance.lines.len() < lines.len() {
        enhance.lines.resize(lines.len(), LineEnhance::default());
    }
    for (i, line) in lines.iter().enumerate() {
        if line.text.trim().is_empty() { continue; }
        let Some(j) = nearest_line(&trans.lines, line.t, 0.12) else { continue; };
        let text = trans.lines[j].text.trim();
        if text.is_empty() || looks_credit(text) { continue; }
        enhance.lines[i].translation = text.to_string();
    }
}

fn nearest_line(lines: &[LyricsLine], t: f32, max_dt: f32) -> Option<usize> {
    let mut best = None;
    let mut best_d = max_dt;
    for (i, l) in lines.iter().enumerate() {
        let d = (l.t - t).abs();
        if d <= best_d { best_d = d; best = Some(i); }
    }
    best
}

fn looks_credit(text: &str) -> bool {
    let t = text.trim();
    t.starts_with("作词") || t.starts_with("作曲") || t.starts_with("编曲") || t.starts_with("词：") || t.starts_with("曲：")
}

/// 把 yrc 逐字对齐到已有同步行；对不上的行保持逐行。
pub fn attach_yrc(enhance: &mut LyricsEnhance, lines: &[LyricsLine], yrc: &[(f32, String, Vec<LyricWord>)]) {
    if enhance.lines.len() < lines.len() {
        enhance.lines.resize(lines.len(), LineEnhance::default());
    }
    for (i, line) in lines.iter().enumerate() {
        if line.text.trim().is_empty() { continue; }
        let Some(&(t, ref text, ref words)) = nearest_yrc(yrc, line.t, 0.18) else { continue; };
        let _ = (t, text);
        if words.is_empty() { continue; }
        enhance.lines[i].words = words.clone();
    }
}

fn nearest_yrc(yrc: &[(f32, String, Vec<LyricWord>)], t: f32, max_dt: f32) -> Option<&(f32, String, Vec<LyricWord>)> {
    let mut best = None;
    let mut best_d = max_dt;
    for row in yrc {
        let d = (row.0 - t).abs();
        if d <= best_d { best_d = d; best = Some(row); }
    }
    best
}

/// 若没有逐行 LRC、只有 yrc：从 yrc 生成歌词行。
pub fn lyrics_from_yrc(yrc: &[(f32, String, Vec<LyricWord>)]) -> (Lyrics, LyricsEnhance) {
    let mut lines = Vec::new();
    let mut enh = Vec::new();
    for (t, text, words) in yrc {
        if text.trim().is_empty() { continue; }
        lines.push(LyricsLine { t: *t, text: text.clone() });
        enh.push(LineEnhance { translation: String::new(), words: words.clone() });
    }
    (Lyrics { lines, synced: true, enhance: LyricsEnhance { lines: enh.clone() } }, LyricsEnhance { lines: enh })
}


/// 把 yrc / 译文附加到已有歌词上。
pub fn enrich(lyrics: &mut Lyrics, yrc: Option<&str>, tlyric: Option<&str>) {
    if let Some(raw) = yrc.filter(|s| !s.trim().is_empty()) {
        let rows = parse_yrc(raw, true);
        if !rows.is_empty() {
            if lyrics.synced && !lyrics.lines.is_empty() {
                attach_yrc(&mut lyrics.enhance, &lyrics.lines, &rows);
                // 若逐行歌词缺少正文、yrc 更完整，用 yrc 重建并保留译文位置。
                if !lyrics.enhance.has_words() {
                    let (l, e) = lyrics_from_yrc(&rows);
                    let _ = (l, e);
                }
            } else {
                let (l, e) = lyrics_from_yrc(&rows);
                *lyrics = l;
                lyrics.enhance = e;
            }
        }
    }
    if let Some(raw) = tlyric.filter(|s| !s.trim().is_empty()) {
        let trans = crate::lyrics::parse_lrc(raw);
        attach_translation(&mut lyrics.enhance, &lyrics.lines, &trans);
    }
}

/// 把增强数据写成可缓存的 JSON。
pub fn encode_cache(enh: &LyricsEnhance) -> String {
    let lines: Vec<serde_json::Value> = enh.lines.iter().map(|l| {
        serde_json::json!({
            "tr": l.translation,
            "w": l.words.iter().map(|w| serde_json::json!([w.t, w.dur, w.text])).collect::<Vec<_>>(),
        })
    }).collect();
    serde_json::json!({ "v": 1, "lines": lines }).to_string()
}

pub fn decode_cache(raw: &str) -> Option<LyricsEnhance> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    let arr = v.get("lines")?.as_array()?;
    let mut lines = Vec::new();
    for l in arr {
        let tr = l.get("tr").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let mut words = Vec::new();
        if let Some(ws) = l.get("w").and_then(|x| x.as_array()) {
            for w in ws {
                let a = w.as_array()?;
                words.push(LyricWord {
                    t: a.first()?.as_f64()? as f32,
                    dur: a.get(1)?.as_f64()? as f32,
                    text: a.get(2)?.as_str()?.to_string(),
                });
            }
        }
        lines.push(LineEnhance { translation: tr, words });
    }
    Some(LyricsEnhance { lines })
}


/// 单行比槽宽更长时的横向偏移（像素，向左为正）。
/// 开头停留、滑到末尾、末尾停留，然后回到开头。短句返回 0。
pub fn marquee_offset(text_width: f32, slot: f32, age: f32) -> f32 {
    let overflow = text_width - slot;
    if overflow <= 1.0 || slot <= 1.0 || !overflow.is_finite() { return 0.0; }
    let pause = 0.7;
    let speed = 28.0;
    let travel = overflow / speed;
    let cycle = pause + travel + pause;
    let t = if age.is_finite() { age.rem_euclid(cycle) } else { 0.0 };
    if t < pause { 0.0 }
    else if t < pause + travel { (t - pause) * speed }
    else { overflow }
}

/// 绘制带扫光的一行歌词：先画暗色全文，再按进度裁出亮色。
pub fn paint_scanned(
    painter: &eframe::egui::Painter,
    pos: eframe::egui::Pos2,
    text: &str,
    font: eframe::egui::FontId,
    dim: eframe::egui::Color32,
    lit: eframe::egui::Color32,
    progress: f32,
    max_width: f32,
    max_rows: usize,
    line_age: f32,
) -> eframe::egui::Vec2 {
    paint_scanned_aligned(painter, pos, text, font, dim, lit, progress, max_width, max_rows, crate::model::LyricAlign::Left, line_age)
}

pub fn paint_scanned_aligned(
    painter: &eframe::egui::Painter,
    pos: eframe::egui::Pos2,
    text: &str,
    font: eframe::egui::FontId,
    dim: eframe::egui::Color32,
    lit: eframe::egui::Color32,
    progress: f32,
    max_width: f32,
    max_rows: usize,
    align: crate::model::LyricAlign,
    line_age: f32,
) -> eframe::egui::Vec2 {
    // Always lay out left-aligned, then shift the origin so Center sits inside
    // [pos.x, pos.x + max_width]. Using job.halign = Center makes egui treat
    // `pos` as the midline, which would overflow left into the cover art.
    // 单行先按不换行测量：放得下就按对齐画；放不下就在槽内裁切并横向滚动。
    let single = max_rows <= 1;
    let slot = max_width.max(0.0);
    let layout = |color: eframe::egui::Color32| {
        let wrap = if single { f32::INFINITY } else { slot.max(1.0) };
        let mut job = eframe::egui::text::LayoutJob::simple(text.to_string(), font.clone(), color, wrap);
        job.wrap.max_rows = max_rows.max(1);
        job.wrap.break_anywhere = !single;
        job.halign = eframe::egui::Align::LEFT;
        painter.layout_job(job)
    };
    let galley = layout(dim);
    let size = galley.size();
    let scrolling = size.x > slot + 1.0;
    let shift = if scrolling { marquee_offset(size.x, slot, line_age) } else { 0.0 };
    let origin = if scrolling {
        pos - eframe::egui::vec2(shift, 0.0)
    } else {
        match align {
            crate::model::LyricAlign::Left => pos,
            crate::model::LyricAlign::Center => {
                let w = size.x.min(slot);
                pos + eframe::egui::vec2(((slot - w) * 0.5).max(0.0), 0.0)
            }
        }
    };
    let slot_clip = eframe::egui::Rect::from_min_size(pos, eframe::egui::vec2(slot.max(1.0), size.y.max(font.size)));
    let base = if scrolling { painter.with_clip_rect(slot_clip.intersect(painter.clip_rect())) } else { painter.clone() };
    base.galley(origin, galley, dim);
    let p = progress.clamp(0.0, 1.0);
    if p > 0.002 && p < 0.999 {
        let galley = layout(lit);
        let scan = eframe::egui::Rect::from_min_size(origin, eframe::egui::vec2(size.x * p + 0.5, size.y));
        let clip = if scrolling { scan.intersect(slot_clip) } else { scan };
        let clipped = base.with_clip_rect(clip.intersect(painter.clip_rect()));
        clipped.galley(origin, galley, lit);
    } else if p >= 0.999 {
        base.galley(origin, layout(lit), lit);
    }
    if scrolling { painter.ctx().request_repaint(); }
    eframe::egui::vec2(if scrolling { slot } else { size.x }, size.y)
}

fn with_alpha(color: eframe::egui::Color32, a: f32) -> eframe::egui::Color32 {
    let a = (a.clamp(0.0, 1.0) * color.a() as f32).round() as u8;
    eframe::egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a)
}

/// 按设置绘制一行歌词：扫光、整句淡入或滚动。`fade` 为换句进度 0..1（1 已稳定）。
pub fn paint_lyric_line(
    painter: &eframe::egui::Painter,
    pos: eframe::egui::Pos2,
    text: &str,
    prev: Option<&str>,
    font: eframe::egui::FontId,
    dim: eframe::egui::Color32,
    lit: eframe::egui::Color32,
    karaoke: f32,
    fade: f32,
    anim: crate::model::LyricAnim,
    max_width: f32,
    max_rows: usize,
    align: crate::model::LyricAlign,
    line_age: f32,
) -> eframe::egui::Vec2 {
    let fade = fade.clamp(0.0, 1.0);
    match anim {
        crate::model::LyricAnim::Scan => {
            paint_scanned_aligned(painter, pos, text, font, dim, lit, karaoke, max_width, max_rows, align, line_age)
        }
        crate::model::LyricAnim::Fade => {
            let color = with_alpha(lit, fade.max(0.08));
            paint_scanned_aligned(painter, pos, text, font.clone(), color, color, 1.0, max_width, max_rows, align, line_age)
        }
        crate::model::LyricAnim::Scroll => {
            let h = font.size * 1.35;
            let clip = eframe::egui::Rect::from_min_size(pos, eframe::egui::vec2(max_width, h.max(font.size + 2.0)));
            let clipped = painter.with_clip_rect(clip.intersect(painter.clip_rect()));
            if fade < 0.995 {
                if let Some(prev) = prev.filter(|s| !s.is_empty()) {
                    let up = pos - eframe::egui::vec2(0.0, fade * h);
                    let c = with_alpha(dim, 1.0 - fade);
                    paint_scanned_aligned(&clipped, up, prev, font.clone(), c, c, 1.0, max_width, max_rows, align, 0.0);
                }
            }
            let down = pos + eframe::egui::vec2(0.0, (1.0 - fade) * h);
            paint_scanned_aligned(&clipped, down, text, font, with_alpha(lit, fade.max(0.2)), with_alpha(lit, fade.max(0.2)), 1.0, max_width, max_rows, align, line_age)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_line_does_not_scroll_and_long_line_resets_with_age() {
        assert_eq!(marquee_offset(80.0, 120.0, 3.0), 0.0);
        assert_eq!(marquee_offset(200.0, 100.0, 0.2), 0.0, "开头应停留");
        let mid = marquee_offset(200.0, 100.0, 0.7 + 1.0);
        assert!((mid - 28.0).abs() < 0.1, "停留结束后按固定速度横移，实际 {mid}");
        assert!((marquee_offset(200.0, 100.0, 0.0)).abs() < 0.01, "换句后的年龄归零，滚动从头开始");
        let end = marquee_offset(200.0, 100.0, 30.0);
        assert!(end <= 100.0 + 0.1, "偏移不超过超出的宽度");
    }

    #[test]
    fn yrc_timing_first_splits_words() {
        let raw = "[147,3460](147,240,0)想(387,200,0)你\n[4000,1000](4000,500,0)啊";
        let rows = parse_yrc(raw, true);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].1, "想你");
        assert_eq!(rows[0].2.len(), 2);
        assert!((rows[0].2[0].t - 0.147).abs() < 0.001);
        assert!((rows[0].2[1].t - 0.387).abs() < 0.001);
        assert_eq!(rows[0].2[0].text, "想");
    }

    #[test]
    fn yrc_json_line_is_accepted() {
        let raw = r#"{"t":1470,"c":[{"tx":"前","dt":240},{"tx":"言","dt":180}]}"#;
        let rows = parse_yrc(raw, true);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, "前言");
        assert_eq!(rows[0].2.len(), 2);
        assert!((rows[0].0 - 1.47).abs() < 0.01);
    }

    #[test]
    fn krc_words_use_offsets() {
        let line = "[1000,2000]<0,200,0>你<200,300,0>好";
        let (t, text, words) = words_from_krc_line(line).unwrap();
        assert!((t - 1.0).abs() < 0.001);
        assert_eq!(text, "你好");
        assert_eq!(words.len(), 2);
        assert!((words[1].t - 1.2).abs() < 0.001);
    }

    #[test]
    fn scan_progress_follows_words_then_falls_back_to_line() {
        let words = vec![
            LyricWord { t: 1.0, dur: 0.5, text: "啊".into() },
            LyricWord { t: 1.5, dur: 0.5, text: "哦".into() },
        ];
        assert!((scan_progress(1.0, Some(2.0), &words, 0.5) - 0.0).abs() < 0.01);
        assert!((scan_progress(1.0, Some(2.0), &words, 1.25) - 0.25).abs() < 0.05);
        assert!((scan_progress(1.0, Some(2.0), &words, 2.1) - 1.0).abs() < 0.01);
        let p = scan_progress(1.0, Some(3.0), &[], 2.0);
        assert!((p - 0.5).abs() < 0.01);
    }

    #[test]
    fn translation_attaches_by_nearby_time() {
        let lyrics = Lyrics {
            synced: true,
            lines: vec![LyricsLine { t: 1.0, text: "hello".into() }, LyricsLine { t: 2.0, text: "world".into() }],
            enhance: LyricsEnhance::default(),
        };
        let trans = Lyrics {
            synced: true,
            lines: vec![LyricsLine { t: 1.02, text: "你好".into() }, LyricsLine { t: 2.05, text: "世界".into() }],
            enhance: LyricsEnhance::default(),
        };
        let mut enh = LyricsEnhance::default();
        attach_translation(&mut enh, &lyrics.lines, &trans);
        assert_eq!(enh.lines[0].translation, "你好");
        assert_eq!(enh.lines[1].translation, "世界");
    }

    #[test]
    fn cache_round_trips() {
        let enh = LyricsEnhance { lines: vec![LineEnhance {
            translation: "你好".into(),
            words: vec![LyricWord { t: 1.0, dur: 0.2, text: "嗨".into() }],
        }]};
        let raw = encode_cache(&enh);
        let back = decode_cache(&raw).unwrap();
        assert_eq!(back, enh);
    }
}
