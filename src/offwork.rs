//! 下班倒计时。只按本机设置：固定下班钟点，或自己记下的上班时间加工时。

use chrono::{Datelike, NaiveTime, Offset as _, TimeZone};
use eframe::egui::Color32;

use crate::model::Settings;
use crate::paint::{Card, H};
use crate::theme::{self, W};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Rule {
    pub enabled: bool,
    pub fixed_end: u32,
    pub weekdays: bool,
    /// 剩余少于此分钟数时，倒计时变色。
    pub soon: u32,
}

impl Rule {
    pub(crate) fn from_settings(s: &Settings) -> Self {
        Self {
            enabled: s.offwork,
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
    Go,
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
            Phase::Ready { .. } => Chip { text: "可走".into(), tone: Tone::Go },
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
        Phase::Ready { leave_at } => format!("{} 已到，可以走了", hm(now, *leave_at)),
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

fn tone_color(tone: Tone) -> Color32 {
    match tone {
        Tone::Plain => theme::WHITE,
        Tone::Soon => theme::YELLOW,
        Tone::Go => theme::GREEN,
        Tone::Mute => theme::GRAY,
    }
}

#[derive(Default)]
pub(crate) struct Runtime {
    mood_day: String,
    mood_arrived: bool,
    mood_done: bool,
    soon_beat: i64,
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
        self.settings.offwork && !matches!(self.offwork_phase(), Phase::Hidden) && !self.effective_minimal()
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
        self.offwork_face();
    }

    fn offwork_face(&mut self) {
        // 专属表情还没画。开关打开后留在这里，不拿现有的 12 张顶上。
        if self.settings.offwork_own_faces || !self.pet_visible() || self.media.is_music() || !matches!(self.pet.state, crate::pet::PetState::Awake | crate::pet::PetState::Curious) {
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
        let mut view = c.faded(alpha);
        if alpha < 0.8 {
            view = view.ghost();
        }
        let color = tone_color(chip.tone);
        let size = 12.0;
        let text_w = view.text_w(&chip.text, size, W::Bold);
        view.text(x - text_w / 2.0, 20.0, &chip.text, size, W::Bold, color, H::Left);
        if alpha < 0.05 {
            return;
        }
        let now = chrono::Local::now();
        let tip = describe(&now, &Rule::from_settings(&self.settings), &current);
        let id = if interactive { "offwork-pill" } else { "offwork-preview" };
        let hit = view.hit(x - width / 2.0, 2.0, width, 26.0, id);
        let _ = crate::quota_ui::bar_tooltip(hit, tip);
    }

    pub(crate) fn paint_offwork_side(&mut self, c: &Card, cx: f32, mid: f32) {
        let (current, chip) = self.chip_now();
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
        Rule { enabled: true, fixed_end: 18 * 60, weekdays: true, soon: 30 }
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
        settings.offwork_end = 18 * 60 + 30;
        settings.offwork_weekdays = false;
        settings.save();
        let loaded = Settings::load();
        assert!(loaded.offwork);
        assert_eq!(loaded.offwork_end, 18 * 60 + 30);
        assert!(!loaded.offwork_weekdays);
        let text = std::fs::read_to_string(dir.join("settings.txt")).unwrap();
        assert!(!text.contains("offwork_mode"));
        assert!(!text.contains("按打卡"));
    }
}
