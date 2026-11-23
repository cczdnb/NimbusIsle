//! 下班倒计时。只按本机设置的上班和下班钟点。

use chrono::{Datelike, NaiveTime, Offset as _, TimeZone, Timelike};
use eframe::egui::Color32;

use crate::model::Settings;
use crate::paint::{Card, H};
use crate::theme::{self, W};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Rule {
    pub enabled: bool,
    pub fixed_start: u32,
    pub fixed_end: u32,
    pub weekdays: bool,
    /// 剩余少于此分钟数时，倒计时变色。
    pub soon: u32,
}

impl Rule {
    pub(crate) fn from_settings(s: &Settings) -> Self {
        Self {
            enabled: s.offwork,
            fixed_start: s.offwork_start.min(1439),
            fixed_end: s.offwork_end.min(1439),
            weekdays: s.offwork_weekdays,
            soon: s.offwork_soon.clamp(5, 120),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    Hidden,
    Counting { leave_at: i64, remain: i64 },
    Ready { leave_at: i64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tone {
    Plain,
    Soon,
    Mute,
}

struct Chip {
    text: String,
    tone: Tone,
}

impl Phase {
    fn chip(&self, soon_secs: i64) -> Chip {
        match *self {
            Phase::Hidden => Chip { text: "—".into(), tone: Tone::Mute },
            Phase::Counting { remain, .. } => {
                let remain = remain.max(0);
                Chip {
                    text: format!("{}:{:02}:{:02}", remain / 3600, (remain % 3600) / 60, remain % 60),
                    tone: if remain <= soon_secs { Tone::Soon } else { Tone::Plain },
                }
            }
            Phase::Ready { .. } => Chip { text: String::new(), tone: Tone::Mute },
        }
    }
}

pub(crate) fn phase<Tz: TimeZone>(now: &chrono::DateTime<Tz>, rule: &Rule) -> Phase
where
    Tz::Offset: std::fmt::Display,
{
    if !rule.enabled {
        return Phase::Hidden;
    }
    let weekend = matches!(now.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun);
    if rule.weekdays && weekend {
        return Phase::Hidden;
    }
    let Some(leave_at) = at_minutes(now, rule.fixed_end) else {
        return Phase::Hidden;
    };
    let remain = leave_at - now.timestamp();
    if remain > 0 {
        Phase::Counting { leave_at, remain }
    } else {
        Phase::Ready { leave_at }
    }
}

fn at_minutes<Tz: TimeZone>(now: &chrono::DateTime<Tz>, minutes: u32) -> Option<i64>
where
    Tz::Offset: std::fmt::Display,
{
    let minutes = minutes.min(1439);
    let time = NaiveTime::from_num_seconds_from_midnight_opt(minutes * 60, 0)?;
    let naive = now.date_naive().and_time(time);
    now.offset().fix().from_local_datetime(&naive).single().map(|dt| dt.timestamp())
}

pub(crate) fn describe<Tz: TimeZone>(now: &chrono::DateTime<Tz>, rule: &Rule, phase: &Phase) -> String
where
    Tz::Offset: std::fmt::Display,
{
    match phase {
        Phase::Hidden if !rule.enabled => "下班倒计时关着。".into(),
        Phase::Hidden => "周末不显示。工作日会回到岛上。".into(),
        Phase::Counting { leave_at, remain } => format!("{} 下班，还剩 {}", hm(now, *leave_at), remain_text(*remain)),
        Phase::Ready { leave_at } => format!("{} 已到，倒计时已收起", hm(now, *leave_at)),
    }
}

fn hm<Tz: TimeZone>(now: &chrono::DateTime<Tz>, ts: i64) -> String
where
    Tz::Offset: std::fmt::Display,
{
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|t| t.with_timezone(&now.offset().fix()).format("%H:%M").to_string())
        .unwrap_or_else(|| "--:--".into())
}

fn remain_text(secs: i64) -> String {
    let secs = secs.max(0);
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h} 小时 {m:02} 分")
    } else if m > 0 {
        format!("{m} 分 {s:02} 秒")
    } else {
        format!("{s} 秒")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Band {
    White,
    Yellow,
    Orange,
    Red,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Segment {
    band: Band,
    /// 已过为 1，未到为 0，当前这一格是这一小时里走过的比例。
    fill: f32,
    span: i64,
}

fn band_for(remain_secs: i64) -> Band {
    if remain_secs > 3 * 3600 {
        Band::White
    } else if remain_secs > 2 * 3600 {
        Band::Yellow
    } else if remain_secs > 3600 {
        Band::Orange
    } else {
        Band::Red
    }
}

fn segments<Tz: TimeZone>(now: &chrono::DateTime<Tz>, rule: &Rule) -> Vec<Segment>
where
    Tz::Offset: std::fmt::Display,
{
    let start = rule.fixed_start.min(1439);
    let end = rule.fixed_end.min(1439);
    if start >= end {
        return Vec::new();
    }
    let (Some(start_at), Some(end_at)) = (at_minutes(now, start), at_minutes(now, end)) else {
        return Vec::new();
    };
    if end_at <= start_at {
        return Vec::new();
    }
    let now_at = now.timestamp();
    let mut cells = Vec::new();
    let mut cursor = start_at;
    while cursor < end_at {
        let next = (cursor + 3600).min(end_at);
        let span = next - cursor;
        let fill = if now_at <= cursor {
            0.0
        } else if now_at >= next {
            1.0
        } else {
            (now_at - cursor) as f32 / span as f32
        };
        cells.push(Segment { band: band_for(end_at - cursor), fill, span });
        cursor = next;
    }
    cells
}

fn band_color(band: Band) -> Color32 {
    match band {
        Band::White => theme::WHITE,
        Band::Yellow => theme::YELLOW,
        Band::Orange => theme::ORANGE,
        Band::Red => theme::RED,
    }
}

/// 整条进度的填充宽度。按每格的实际时长加权，不会超出槽宽。
fn progress_width(width: f32, cells: &[Segment]) -> f32 {
    let span: i64 = cells.iter().map(|cell| cell.span).sum();
    if width < 8.0 || span <= 0 {
        return 0.0;
    }
    let done: f32 = cells.iter().map(|cell| cell.fill.clamp(0.0, 1.0) * cell.span as f32).sum();
    (width * done / span as f32).clamp(0.0, width)
}

fn current_band(cells: &[Segment]) -> Option<Band> {
    cells.iter().find(|cell| cell.fill < 1.0).map(|cell| cell.band).or_else(|| cells.last().map(|cell| cell.band))
}

fn paint_hour_bar(view: &Card, x: f32, width: f32, cells: &[Segment]) {
    let filled = progress_width(width, cells);
    if cells.is_empty() || width < 8.0 {
        return;
    }
    let h = 4.0;
    let y = 21.0;
    let left = x - width / 2.0;
    view.fill(left, y, width, h, h / 2.0, Color32::from_white_alpha(48));
    if filled <= 0.0 {
        return;
    }
    let Some(band) = current_band(cells) else { return };
    view.fill(left, y, filled.max(h).min(width), h, h / 2.0, band_color(band));
}

fn tone_color(tone: Tone) -> Color32 {
    match tone {
        Tone::Plain => theme::WHITE,
        Tone::Soon => theme::YELLOW,
        Tone::Mute => theme::GRAY,
    }
}

const FINALE_HOLD: f64 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Finale {
    Idle,
    Seconds,
    Go { until: f64 },
    Done,
}

impl Default for Finale {
    fn default() -> Self { Self::Idle }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum FinaleCaption {
    #[default]
    None,
    Seconds(i64),
    Go,
}

#[derive(Default)]
pub(crate) struct Runtime {
    mood_day: String,
    mood_arrived: bool,
    mood_done: bool,
    soon_beat: i64,
    finale_day: String,
    finale: Finale,
    caption: FinaleCaption,
    face_day: String,
    face_id: u8,
    face_time: f32,
    face_wait: f32,
    face_once: u16,
    face_plays: u32,
    face_showing: bool,
}

impl Runtime {
    pub(crate) fn load() -> Self {
        Self { soon_beat: -1, ..Self::default() }
    }
}

impl crate::app::App {
    pub(crate) fn offwork_phase(&self) -> Phase {
        phase(&chrono::Local::now(), &Rule::from_settings(&self.settings))
    }

    pub(crate) fn offwork_live(&self) -> bool {
        self.settings.offwork && matches!(self.offwork_phase(), Phase::Counting { .. }) && !self.effective_minimal()
    }

    pub(crate) fn offwork_status(&self) -> String {
        let now = chrono::Local::now();
        let rule = Rule::from_settings(&self.settings);
        describe(&now, &rule, &phase(&now, &rule))
    }

    pub(crate) fn offwork_tick(&mut self, ctx: &eframe::egui::Context) {
        if !self.settings.offwork {
            return;
        }
        let wait = match self.offwork_phase() {
            Phase::Counting { .. } => 1.0,
            Phase::Hidden => 120.0,
            Phase::Ready { .. } => 30.0,
        };
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
    }

    pub(crate) fn offwork_sheet_playing(&self) -> bool {
        self.offwork.face_showing
    }

    pub(crate) fn offwork_face(&mut self, dt: f32, visible: bool) {
        if !self.settings.offwork_own_faces {
            self.pet.expressions.clear_offwork();
            self.legacy_offwork_face();
            return;
        }
        let show = self.settings.offwork
            && visible
            && !self.media.is_music()
            && matches!(self.pet.state, crate::pet::PetState::Awake | crate::pet::PetState::Curious)
            && !matches!(self.offwork_phase(), Phase::Hidden);
        if !show {
            self.offwork.face_showing = false;
            if self.media.is_music() || matches!(self.offwork_phase(), Phase::Hidden) || !self.settings.offwork {
                self.pet.expressions.clear_offwork();
                self.offwork.face_id = 0;
            }
            return;
        }
        let now = chrono::Local::now();
        let minute = now.hour() * 60 + now.minute();
        let cue = face_cue(minute, self.settings.offwork_start, self.settings.offwork_end, self.settings.lunch_start, self.settings.lunch_end);
        let day = now.format("%Y-%m-%d").to_string();
        let shown = step_face(&mut self.offwork, cue, dt, &day);
        self.offwork.face_showing = shown.is_some();
        if let Some((id, time)) = shown {
            self.pet.expressions.suppress_clip();
            self.pet.expressions.show_offwork(id, time);
        } else {
            self.pet.expressions.clear_offwork();
        }
    }

    fn legacy_offwork_face(&mut self) {
        // 专属表情开关关掉时，沿用现在的 12 张。
        if self.settings.offwork_own_faces || !self.pet_enabled() || self.media.is_music() || !matches!(self.pet.state, crate::pet::PetState::Awake | crate::pet::PetState::Curious) {
            return;
        }
        let now = chrono::Local::now();
        let day = now.format("%Y-%m-%d").to_string();
        if self.offwork.mood_day != day {
            self.offwork.mood_day = day;
            self.offwork.mood_arrived = false;
            self.offwork.mood_done = false;
            self.offwork.soon_beat = -1;
        }
        let rule = Rule::from_settings(&self.settings);
        let current = phase(&now, &rule);
        let (remain, counting) = match current {
            Phase::Counting { remain, .. } => (remain, true),
            Phase::Ready { .. } => (0, false),
            Phase::Hidden => return,
        };
        let soon = rule.soon as i64 * 60;
        let since = at_minutes(&now, rule.fixed_end).map(|leave| now.timestamp() - (leave - 9 * 3600));
        if remain <= 0 {
            if !self.offwork.mood_done {
                self.pet.expressions.play(crate::pet_expressions::Expression::Smug);
                self.offwork.mood_done = true;
            }
            return;
        }
        if counting && remain <= soon {
            let beat = remain / 600;
            if self.offwork.soon_beat != beat {
                let expression = if beat % 2 == 0 { crate::pet_expressions::Expression::SideGlance } else { crate::pet_expressions::Expression::Peek };
                self.pet.expressions.play(expression);
                self.offwork.soon_beat = beat;
            }
            return;
        }
        if since.is_some_and(|elapsed| (0..=20 * 60).contains(&elapsed)) && !self.offwork.mood_arrived {
            self.pet.expressions.play(crate::pet_expressions::Expression::Delighted);
            self.offwork.mood_arrived = true;
        }
    }

    fn chip_now(&self) -> (Phase, Chip) {
        let current = self.offwork_phase();
        let soon = self.settings.offwork_soon.clamp(5, 120) as i64 * 60;
        let chip = current.chip(soon);
        (current, chip)
    }

    pub(crate) fn paint_offwork(&mut self, c: &Card, x: f32, width: f32, alpha: f32, interactive: bool) {
        let (current, chip) = self.chip_now();
        if !matches!(current, Phase::Counting { .. }) {
            return;
        }
        let mut view = c.faded(alpha);
        if alpha < 0.8 {
            view = view.ghost();
        }
        let color = tone_color(chip.tone);
        let size = 12.0;
        let text_w = view.text_w(&chip.text, size, W::Bold);
        view.text(x - text_w / 2.0, 17.0, &chip.text, size, W::Bold, color, H::Left);
        let now = chrono::Local::now();
        let cells = segments(&now, &Rule::from_settings(&self.settings));
        paint_hour_bar(&view, x, width, &cells);
        if alpha < 0.05 {
            return;
        }
        let tip = describe(&now, &Rule::from_settings(&self.settings), &current);
        let id = if interactive { "offwork-pill" } else { "offwork-preview" };
        let hit = view.hit(x - width / 2.0, 2.0, width, 26.0, id);
        let _ = crate::quota_ui::bar_tooltip(hit, tip);
    }

    pub(crate) fn paint_offwork_side(&mut self, c: &Card, cx: f32, mid: f32) {
        let (current, chip) = self.chip_now();
        if !matches!(current, Phase::Counting { .. }) {
            return;
        }
        let color = tone_color(chip.tone);
        let (head, secs) = split_hms(&chip.text);
        if secs.is_empty() {
            c.text(cx, mid + 4.0, head, 11.0, W::Bold, color, H::Center);
        } else {
            c.text(cx, mid - 2.0, head, 11.0, W::Bold, color, H::Center);
            c.text(cx, mid + 11.0, secs, 10.0, W::Semibold, color, H::Center);
        }
        let now = chrono::Local::now();
        let tip = describe(&now, &Rule::from_settings(&self.settings), &current);
        let hit = c.hit(cx - 16.0, mid - 16.0, 32.0, 32.0, "offwork-side");
        let _ = crate::quota_ui::bar_tooltip(hit, tip);
    }

    pub(crate) fn advance_offwork_finale(&mut self, ctx: &eframe::egui::Context) {
        if !self.settings.offwork {
            self.offwork.finale = Finale::Idle;
            self.offwork.caption = FinaleCaption::None;
            return;
        }
        let visible = !self.open && !self.side() && !self.nudge_active()
            && !(self.notice.is_some() && !self.compact_notices())
            && !self.effective_minimal();
        let now = chrono::Local::now();
        let shown = phase(&now, &Rule::from_settings(&self.settings));
        let day = now.format("%Y-%m-%d").to_string();
        let (state, caption) = step_finale(self.offwork.finale, &self.offwork.finale_day, &day, &shown, self.t, visible);
        self.offwork.finale_day = day;
        self.offwork.finale = state;
        self.offwork.caption = caption;
        if caption != FinaleCaption::None {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
    }

    pub(crate) fn offwork_finale_showing(&self) -> bool {
        self.offwork.caption != FinaleCaption::None
    }

    pub(crate) fn offwork_finale_size(&self) -> Option<(f32, f32)> {
        self.offwork_finale_showing().then_some((FINALE_W, FINALE_IMAGE_H + FINALE_TEXT_H))
    }

    pub(crate) fn paint_offwork_finale(&self, ui: &eframe::egui::Ui, painter: &eframe::egui::Painter, panel: eframe::egui::Rect, flare: f32, flare_h: f32, radius: f32, feather: f32) {
        let label = match self.offwork.caption {
            FinaleCaption::Seconds(remain) => remain.to_string(),
            FinaleCaption::Go => "GOGOGO".to_string(),
            FinaleCaption::None => return,
        };
        crate::shape::fill(painter, panel, flare, flare_h, radius, Color32::WHITE, feather);
        let card = Card::new(ui, panel.min, panel);
        let texture = finale_texture(ui.ctx());
        let image_h = (panel.height() - FINALE_TEXT_H).max(1.0);
        card.painter.image(
            texture.id(),
            card.rect(0.0, 0.0, panel.width(), image_h),
            eframe::egui::Rect::from_min_max(eframe::egui::pos2(0.0, 0.0), eframe::egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        painter.text(
            eframe::egui::pos2(panel.center().x, panel.bottom() - FINALE_TEXT_H / 2.0),
            eframe::egui::Align2::CENTER_CENTER,
            &label,
            theme::font(28.0, W::Bold),
            Color32::BLACK,
        );
    }
}

const FINALE_W: f32 = 240.0;
const FINALE_IMAGE_H: f32 = 240.0 * 393.0 / 566.0;
const FINALE_TEXT_H: f32 = 36.0;

fn finale_texture(ctx: &eframe::egui::Context) -> eframe::egui::TextureHandle {
    let key = eframe::egui::Id::new("offwork-finale");
    if let Some(texture) = ctx.data(|data| data.get_temp::<eframe::egui::TextureHandle>(key)) {
        return texture;
    }
    let decoded = image::load_from_memory(include_bytes!("../assets/pet/offwork-end.png")).expect("下班倒计时结束图").into_rgba8();
    let image = eframe::egui::ColorImage::from_rgba_unmultiplied([decoded.width() as usize, decoded.height() as usize], decoded.as_raw());
    let texture = ctx.load_texture("offwork-finale", image, eframe::egui::TextureOptions::LINEAR);
    crate::skin_paint::mark_colored(texture.id());
    ctx.data_mut(|data| data.insert_temp(key, texture.clone()));
    texture
}

fn step_finale(mut state: Finale, stored_day: &str, day: &str, shown: &Phase, now: f64, visible: bool) -> (Finale, FinaleCaption) {
    if stored_day != day {
        state = Finale::Idle;
    }
    let remain = match shown {
        Phase::Counting { remain, .. } if *remain > 0 => Some(*remain),
        Phase::Ready { .. } => Some(0),
        Phase::Hidden | Phase::Counting { .. } => None,
    };
    match (state, remain) {
        (Finale::Done, _) => (Finale::Done, FinaleCaption::None),
        (Finale::Go { until }, _) if now >= until => (Finale::Done, FinaleCaption::None),
        (Finale::Go { until }, _) => (Finale::Go { until }, if visible { FinaleCaption::Go } else { FinaleCaption::None }),
        (_, None) => (Finale::Idle, FinaleCaption::None),
        (_, Some(remain)) if remain > 60 => (Finale::Idle, FinaleCaption::None),
        (Finale::Seconds, Some(0)) if visible => (Finale::Go { until: now + FINALE_HOLD }, FinaleCaption::Go),
        (Finale::Seconds, Some(0)) => (Finale::Done, FinaleCaption::None),
        (_, Some(0)) => (Finale::Idle, FinaleCaption::None),
        (state, Some(_)) if !visible => (state, FinaleCaption::None),
        (_, Some(remain)) => (Finale::Seconds, FinaleCaption::Seconds(remain)),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum FaceCue {
    Quiet,
    Lunch,
    Once(u8),
    Repeat { id: u8, min_gap: f32, max_gap: f32 },
    Loop(u8),
}

fn repeat_cue(id: u8) -> FaceCue {
    let (min_gap, max_gap) = match id {
        2 | 9 | 10 | 11 => (3.0, 5.0),
        4 | 6 | 7 => (4.0, 6.0),
        _ => (6.0, 8.0),
    };
    FaceCue::Repeat { id, min_gap, max_gap }
}

fn face_gap(min_gap: f32, max_gap: f32, id: u8, plays: u32) -> f32 {
    let t = crate::anim::hash01(((id as u64) << 32) | plays as u64);
    min_gap + (max_gap - min_gap) * t
}

/// 专属表情落在哪一段。时间按上班、午休、下班的钟点算，不写死 9:30。
fn face_cue(now: u32, start: u32, end: u32, lunch_a: u32, lunch_b: u32) -> FaceCue {
    let now = now.min(1439);
    let start = start.min(1439);
    let end = end.min(1439);
    if start >= end {
        return FaceCue::Quiet;
    }
    if now < start {
        return FaceCue::Quiet;
    }
    if now >= end {
        return if now - end < 2 { FaceCue::Once(12) } else { FaceCue::Loop(13) };
    }
    let lunch_ok = lunch_a < lunch_b && lunch_a > start && lunch_b < end;
    if lunch_ok && now >= lunch_a && now < lunch_b {
        return FaceCue::Lunch;
    }
    let morning_end = if lunch_ok { lunch_a } else { end.saturating_sub(180).max(start) };
    if now < morning_end {
        return morning_cue(now, start, morning_end, lunch_ok);
    }
    afternoon_cue(now, if lunch_ok { lunch_b } else { morning_end }, end)
}

fn morning_cue(now: u32, start: u32, morning_end: u32, hope: bool) -> FaceCue {
    let len = morning_end.saturating_sub(start);
    let arrive = len.min(15);
    let rest = len - arrive;
    let perk = rest.min(30);
    let rest = rest - perk;
    let hope_len = if hope { rest.min(30) } else { 0 };
    let work = rest - hope_len;
    let arrive_end = start + arrive;
    let perk_end = arrive_end + perk;
    let work_end = perk_end + work;
    if now < arrive_end {
        FaceCue::Once(1)
    } else if now < perk_end {
        repeat_cue(2)
    } else if now < work_end {
        repeat_cue(3)
    } else if hope {
        repeat_cue(4)
    } else {
        FaceCue::Quiet
    }
}

fn afternoon_cue(now: u32, from: u32, end: u32) -> FaceCue {
    let yawn0 = end.saturating_sub(180);
    let yawn1 = end.saturating_sub(160);
    let stretch1 = end.saturating_sub(140);
    let clock0 = end.saturating_sub(60);
    let fidget0 = end.saturating_sub(40);
    let smirk0 = end.saturating_sub(20);
    let window = |a: u32, b: u32, cue: FaceCue| -> Option<FaceCue> {
        let a = a.max(from);
        let b = b.min(end);
        if a < b && now >= a && now < b { Some(cue) } else { None }
    };
    window(smirk0, end, repeat_cue(11))
        .or_else(|| window(fidget0, smirk0, repeat_cue(10)))
        .or_else(|| window(clock0, fidget0, repeat_cue(9)))
        .or_else(|| window(stretch1, clock0, repeat_cue(8)))
        .or_else(|| window(yawn1, stretch1, repeat_cue(7)))
        .or_else(|| window(yawn0, yawn1, repeat_cue(6)))
        .or_else(|| window(from, yawn0, repeat_cue(5)))
        .unwrap_or(FaceCue::Quiet)
}

fn step_face(play: &mut Runtime, cue: FaceCue, dt: f32, day: &str) -> Option<(u8, f32)> {
    if play.face_day != day {
        play.face_day = day.to_string();
        play.face_id = 0;
        play.face_time = 0.0;
        play.face_wait = 0.0;
        play.face_once = 0;
        play.face_plays = 0;
    }
    let clip = crate::pet_expressions::offwork_clip_duration();
    match cue {
        FaceCue::Quiet | FaceCue::Lunch => {
            play.face_id = 0;
            play.face_time = 0.0;
            play.face_wait = 0.0;
            None
        }
        FaceCue::Once(id) => {
            if play.face_once & (1 << id) != 0 {
                play.face_id = 0;
                return None;
            }
            if play.face_id != id {
                play.face_id = id;
                play.face_time = 0.0;
            }
            play.face_time += dt;
            if play.face_time >= clip {
                play.face_once |= 1 << id;
                play.face_id = 0;
                play.face_time = 0.0;
                None
            } else {
                Some((id, play.face_time))
            }
        }
        FaceCue::Repeat { id, min_gap, max_gap } => {
            if play.face_id != id {
                play.face_id = id;
                play.face_time = 0.0;
                play.face_wait = 0.0;
            }
            if play.face_wait > 0.0 {
                play.face_wait = (play.face_wait - dt).max(0.0);
                return None;
            }
            play.face_time += dt;
            if play.face_time >= clip {
                play.face_plays += 1;
                play.face_time = 0.0;
                play.face_wait = face_gap(min_gap, max_gap, id, play.face_plays);
                None
            } else {
                Some((id, play.face_time))
            }
        }
        FaceCue::Loop(id) => {
            if play.face_id != id {
                play.face_id = id;
                play.face_time = 0.0;
            }
            play.face_time += dt;
            if play.face_time >= clip {
                play.face_time %= clip;
            }
            Some((id, play.face_time))
        }
    }
}

fn split_hms(text: &str) -> (&str, &str) {
    match text.rsplit_once(':') {
        Some((head, secs)) if head.contains(':') => (head, secs),
        _ => (text, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, TimeZone};

    fn rule() -> Rule {
        Rule { enabled: true, fixed_start: 9 * 60, fixed_end: 18 * 60, weekdays: true, soon: 30 }
    }

    fn at(day: u32, h: u32, m: u32, s: u32) -> chrono::DateTime<FixedOffset> {
        FixedOffset::east_opt(8 * 3600).unwrap().with_ymd_and_hms(2026, 11, day, h, m, s).unwrap()
    }

    fn ts(day: u32, h: u32, m: u32) -> i64 {
        at(day, h, m, 0).timestamp()
    }

    #[test]
    fn weekend_stays_hidden_and_weekdays_count_to_the_fixed_end() {
        let rule = rule();
        assert!(matches!(phase(&at(21, 11, 0, 0), &rule), Phase::Hidden));
        let mut everyday = rule.clone();
        everyday.weekdays = false;
        assert!(matches!(phase(&at(21, 11, 0, 0), &everyday), Phase::Counting { .. }));
        match phase(&at(18, 9, 0, 0), &rule) {
            Phase::Counting { leave_at, .. } => assert_eq!(leave_at, ts(18, 18, 0)),
            other => panic!("工作日应倒计时到固定下班时间：{other:?}"),
        }
    }

    #[test]
    fn last_half_hour_shows_minutes_and_seconds() {
        let rule = rule();
        let shown = phase(&at(18, 17, 40, 5), &rule);
        let chip = shown.chip(30 * 60);
        assert_eq!(chip.text, "0:19:55");
        assert_eq!(chip.tone, Tone::Soon);
        assert_eq!(shown.chip(10 * 60).tone, Tone::Plain, "还没进到设定的提前提醒窗口时保持原色");
    }

    #[test]
    fn weekday_without_a_punch_counts_down_to_the_fixed_end() {
        let rule = rule();
        match phase(&at(18, 9, 54, 2), &rule) {
            Phase::Counting { leave_at, remain } => {
                assert_eq!(leave_at, ts(18, 18, 0));
                assert_eq!(remain, 8 * 3600 + 5 * 60 + 58);
                assert_eq!(phase(&at(18, 9, 54, 2), &rule).chip(30 * 60).text, "8:05:58");
            }
            other => panic!("没记下上班时间时按固定下班时间倒计时：{other:?}"),
        }
    }

    #[test]
    fn finished_countdown_leaves_the_island() {
        let rule = rule();
        let shown = phase(&at(18, 18, 0, 1), &rule);
        assert!(matches!(shown, Phase::Ready { .. }));
        let chip = shown.chip(30 * 60);
        assert!(chip.text.is_empty());
        assert!(!chip.text.contains("可走"));
        assert!(!describe(&at(18, 18, 0, 1), &rule, &shown).contains("可走"));
    }

    #[test]
    fn hour_cells_follow_the_configured_shift() {
        let rule = rule();
        let morning = segments(&at(18, 8, 0, 0), &rule);
        assert_eq!(morning.len(), 9);
        assert!(morning.iter().all(|cell| cell.fill == 0.0), "上班前九格都是空槽");
        assert_eq!(morning[6].band, Band::Yellow, "15:00 进入时还剩 3 小时");
        assert_eq!(morning[7].band, Band::Orange, "16:00 进入时还剩 2 小时");
        assert_eq!(morning[8].band, Band::Red, "17:00 进入时还剩 1 小时");
        assert!(morning[..6].iter().all(|cell| cell.band == Band::White));

        let mid = segments(&at(18, 10, 30, 0), &rule);
        assert_eq!(mid[0].fill, 1.0);
        assert!((mid[1].fill - 0.5).abs() < 0.001);
        assert!(mid[2..].iter().all(|cell| cell.fill == 0.0));

        let mut short = rule.clone();
        short.fixed_start = 16 * 60;
        let late = segments(&at(18, 16, 30, 0), &short);
        assert_eq!(late.len(), 2);
        assert_eq!(late[0].band, Band::Orange);
        assert!((late[0].fill - 0.5).abs() < 0.001);
        assert_eq!(late[1].band, Band::Red);
        assert_eq!(late[1].fill, 0.0);
        assert!(!late.iter().any(|cell| matches!(cell.band, Band::White | Band::Yellow)));

        let mut tail = rule.clone();
        tail.fixed_end = 18 * 60 + 30;
        let ending = segments(&at(18, 18, 10, 0), &tail);
        assert_eq!(ending.len(), 10);
        assert_eq!(ending[9].band, Band::Red);
        assert!((ending[9].fill - 10.0 / 30.0).abs() < 0.001, "末尾半小时按自己的长度填充");
    }

    fn hm(hour: u32, minute: u32) -> u32 { hour * 60 + minute }

    fn sample_shift(now: u32) -> FaceCue {
        face_cue(now, hm(9, 30), hm(18, 30), hm(12, 0), hm(13, 30))
    }

    #[test]
    fn offwork_sheets_follow_the_shift_in_file_order() {
        assert!(matches!(sample_shift(hm(9, 0)), FaceCue::Quiet));
        assert!(matches!(sample_shift(hm(9, 30)), FaceCue::Once(1)));
        assert!(matches!(sample_shift(hm(9, 45)), FaceCue::Repeat { id: 2, .. }));
        assert!(matches!(sample_shift(hm(10, 15)), FaceCue::Repeat { id: 3, .. }));
        assert!(matches!(sample_shift(hm(11, 30)), FaceCue::Repeat { id: 4, .. }));
        assert!(matches!(sample_shift(hm(12, 30)), FaceCue::Lunch));
        assert!(matches!(sample_shift(hm(13, 30)), FaceCue::Repeat { id: 5, .. }));
        assert!(matches!(sample_shift(hm(15, 30)), FaceCue::Repeat { id: 6, .. }));
        assert!(matches!(sample_shift(hm(15, 50)), FaceCue::Repeat { id: 7, .. }));
        assert!(matches!(sample_shift(hm(16, 10)), FaceCue::Repeat { id: 8, .. }));
        assert!(matches!(sample_shift(hm(17, 30)), FaceCue::Repeat { id: 9, .. }));
        assert!(matches!(sample_shift(hm(17, 50)), FaceCue::Repeat { id: 10, .. }));
        assert!(matches!(sample_shift(hm(18, 10)), FaceCue::Repeat { id: 11, .. }));
        assert!(matches!(sample_shift(hm(18, 30)), FaceCue::Once(12)));
        assert!(matches!(sample_shift(hm(18, 32)), FaceCue::Loop(13)));
    }

    #[test]
    fn arrival_plays_once_and_overtime_loops() {
        let mut play = Runtime::default();
        let clip = crate::pet_expressions::offwork_clip_duration();
        let shown = step_face(&mut play, FaceCue::Once(1), 0.1, "2026-11-23");
        assert_eq!(shown.unwrap().0, 1);
        let done = step_face(&mut play, FaceCue::Once(1), clip, "2026-11-23");
        assert!(done.is_none());
        let again = step_face(&mut play, FaceCue::Once(1), 0.1, "2026-11-23");
        assert!(again.is_none(), "到岗当天只播一次");

        let mut overtime = Runtime::default();
        let first = step_face(&mut overtime, FaceCue::Loop(13), clip - 0.05, "2026-11-23").unwrap();
        assert_eq!(first.0, 13);
        let wrapped = step_face(&mut overtime, FaceCue::Loop(13), 0.1, "2026-11-23").unwrap();
        assert_eq!(wrapped.0, 13);
        assert!(wrapped.1 < 0.2, "加班播完立刻从头再播");
    }

    #[test]
    fn hour_bar_stays_inside_the_existing_slot() {
        let rule = rule();
        let morning = segments(&at(18, 8, 0, 0), &rule);
        assert_eq!(progress_width(72.0, &morning), 0.0, "上班前进度条是空的");
        let mid = segments(&at(18, 10, 30, 0), &rule);
        let filled = progress_width(72.0, &mid);
        assert!(filled > 72.0 * 1.0 / 9.0 && filled < 72.0 * 2.0 / 9.0, "10:30 应走完一小时再加半小时，实际 {filled}");
        assert!(filled <= 72.0);
        let done = Segment { band: Band::Red, fill: 1.0, span: 3600 };
        assert!((progress_width(72.0, &[done]) - 72.0).abs() < 0.01);
        assert_eq!(progress_width(72.0, &[]), 0.0);
    }

    #[test]
    fn start_that_is_not_before_the_end_draws_no_cells() {
        let mut rule = rule();
        rule.fixed_start = 18 * 60;
        assert!(segments(&at(18, 12, 0, 0), &rule).is_empty());
        rule.fixed_start = 19 * 60;
        assert!(segments(&at(18, 12, 0, 0), &rule).is_empty());
        match phase(&at(18, 12, 0, 0), &rule) {
            Phase::Counting { leave_at, .. } => assert_eq!(leave_at, ts(18, 18, 0)),
            other => panic!("格子画不出来时，数字仍倒计时到下班：{other:?}"),
        }
    }

    fn counting(remain: i64) -> Phase {
        Phase::Counting { leave_at: 0, remain }
    }

    fn finale(state: Finale, shown: &Phase, now: f64, visible: bool) -> (Finale, FinaleCaption) {
        step_finale(state, "2026-11-20", "2026-11-20", shown, now, visible)
    }

    #[test]
    fn last_minute_card_follows_the_remaining_seconds() {
        let (state, caption) = finale(Finale::Idle, &counting(61), 0.0, true);
        assert_eq!(state, Finale::Idle);
        assert_eq!(caption, FinaleCaption::None);

        let (state, caption) = finale(Finale::Idle, &counting(60), 0.0, true);
        assert_eq!(state, Finale::Seconds);
        assert_eq!(caption, FinaleCaption::Seconds(60));

        let (state, caption) = finale(Finale::Idle, &counting(37), 0.0, true);
        assert_eq!((state, caption), (Finale::Seconds, FinaleCaption::Seconds(37)), "中途打开从剩下的秒数开始");

        let (state, caption) = finale(Finale::Idle, &counting(37), 0.0, false);
        assert_eq!((state, caption), (Finale::Idle, FinaleCaption::None), "这一分钟被挡住时先不拉开");

        let (state, caption) = finale(Finale::Seconds, &counting(20), 0.0, true);
        assert_eq!(caption, FinaleCaption::Seconds(20));
        assert_eq!(state, Finale::Seconds);
    }

    #[test]
    fn gogogo_holds_then_ends_once() {
        let ready = Phase::Ready { leave_at: 0 };
        let (state, caption) = finale(Finale::Seconds, &ready, 10.0, true);
        assert_eq!(caption, FinaleCaption::Go);
        let Finale::Go { until } = state else { panic!("走到 0 应进入 GOGOGO") };
        assert!((until - 14.0).abs() < 0.001);

        let (state, caption) = finale(Finale::Go { until }, &ready, 13.9, true);
        assert_eq!((state, caption), (Finale::Go { until }, FinaleCaption::Go));
        let (state, caption) = finale(Finale::Go { until }, &ready, 14.0, true);
        assert_eq!((state, caption), (Finale::Done, FinaleCaption::None));

        let (state, caption) = finale(Finale::Done, &counting(30), 20.0, true);
        assert_eq!((state, caption), (Finale::Done, FinaleCaption::None), "同一天不再重播");

        let (state, caption) = step_finale(Finale::Done, "2026-11-19", "2026-11-20", &counting(8 * 3600), 0.0, true);
        assert_eq!((state, caption), (Finale::Idle, FinaleCaption::None), "第二天重新开始");
    }

    #[test]
    fn missing_the_last_minute_does_not_replay() {
        let ready = Phase::Ready { leave_at: 0 };
        let (state, caption) = finale(Finale::Idle, &ready, 10.0, true);
        assert_eq!((state, caption), (Finale::Idle, FinaleCaption::None), "已经到点再打开不补播");

        let (state, caption) = finale(Finale::Seconds, &counting(8), 10.0, false);
        assert_eq!((state, caption), (Finale::Seconds, FinaleCaption::None));
        let (state, caption) = finale(state, &ready, 11.0, false);
        assert_eq!((state, caption), (Finale::Done, FinaleCaption::None), "被挡住直到结束就不再补 GOGOGO");
        let (state, caption) = finale(state, &ready, 12.0, true);
        assert_eq!(caption, FinaleCaption::None);
    }

    #[test]
    fn offwork_settings_roundtrip_stays_on_this_machine() {
        let _guard = crate::ui_tests::ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let dir = std::env::temp_dir().join(format!("yunyu-offwork-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let old = std::env::var_os("YUNYU_DATA_DIR");
        std::env::set_var("YUNYU_DATA_DIR", &dir);
        struct Restore(Option<std::ffi::OsString>);
        impl Drop for Restore {
            fn drop(&mut self) {
                match self.0.take() {
                    Some(value) => std::env::set_var("YUNYU_DATA_DIR", value),
                    None => std::env::remove_var("YUNYU_DATA_DIR"),
                }
            }
        }
        let _restore = Restore(old);
        let mut settings = Settings::default();
        settings.offwork = true;
        settings.offwork_start = 9 * 60 + 15;
        settings.offwork_end = 18 * 60 + 30;
        settings.offwork_weekdays = false;
        settings.save();
        let loaded = Settings::load();
        assert!(loaded.offwork);
        assert_eq!(loaded.offwork_start, 9 * 60 + 15);
        assert_eq!(loaded.offwork_end, 18 * 60 + 30);
        assert!(!loaded.offwork_weekdays);
        let text = std::fs::read_to_string(dir.join("settings.txt")).unwrap();
        assert!(!text.contains("offwork_mode"));
        assert!(!text.contains("按打卡"));
    }
}
