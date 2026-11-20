//! 国务院放假和调休。农历、节气、宜忌仍由 almanac 本地计算。
//!
//! 程序内置 holiday-cn 的 2025–2027 年数据，打开日程时再按年更新。
//! 同一天若当年和次年的通知都写到了，用年份更大的那一份。

use chrono::{Datelike, NaiveDate, Weekday};
use std::collections::{HashMap, HashSet};
#[cfg(not(test))]
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

const FRESH: Duration = Duration::from_secs(20 * 60 * 60);
#[cfg(not(test))]
const RETRY: Duration = Duration::from_secs(60 * 60);
const OFF_DAYS: std::ops::RangeInclusive<usize> = 7..=40;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Mark {
    Off { name: Option<String> },
    Work { name: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tint {
    Rest,
    Work,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Day {
    name: String,
    off: bool,
}

struct YearFile {
    year: i32,
    /// 0 是内置数据，1 是后来下载的缓存。同一年缓存覆盖内置。
    rank: u8,
    days: Vec<(NaiveDate, Day)>,
}

struct Store {
    days: HashMap<NaiveDate, Day>,
    checked: Option<Instant>,
    stamps: Vec<(String, u64, i64)>,
}

fn store() -> MutexGuard<'static, Store> {
    static STORE: std::sync::OnceLock<Mutex<Store>> = std::sync::OnceLock::new();
    STORE.get_or_init(|| Mutex::new(Store { days: HashMap::new(), checked: None, stamps: Vec::new() })).lock().unwrap_or_else(|err| err.into_inner())
}

#[cfg(not(test))]
struct Gate {
    running: bool,
    pending: BTreeSet<i32>,
    skip_until: HashMap<i32, Instant>,
}

#[cfg(not(test))]
fn gate() -> MutexGuard<'static, Gate> {
    static GATE: std::sync::OnceLock<Mutex<Gate>> = std::sync::OnceLock::new();
    GATE.get_or_init(|| Mutex::new(Gate { running: false, pending: BTreeSet::new(), skip_until: HashMap::new() })).lock().unwrap_or_else(|err| err.into_inner())
}

pub(crate) fn refresh(year: i32) {
    #[cfg(not(test))]
    refresh_impl(year);
    #[cfg(test)]
    { let _ = year; }
}

pub(crate) fn mark(date: NaiveDate) -> Option<Mark> {
    if let Some(day) = lookup(date) {
        return Some(if day.off { Mark::Off { name: Some(day.name) } } else { Mark::Work { name: day.name } });
    }
    matches!(date.weekday(), Weekday::Sat | Weekday::Sun).then_some(Mark::Off { name: None })
}

pub(crate) fn tint(date: NaiveDate) -> Option<Tint> {
    match mark(date) {
        Some(Mark::Off { .. }) => Some(Tint::Rest),
        Some(Mark::Work { .. }) => Some(Tint::Work),
        None => None,
    }
}

pub(crate) fn hover(date: NaiveDate) -> String {
    let base = format!("{}年{}月{}日", date.year(), date.month(), date.day());
    match note(date) {
        Some((name, label)) => format!("{base} {name} {label}"),
        None => base,
    }
}

pub(crate) fn beside(date: NaiveDate, current: &str) -> String {
    let Some((name, label)) = note(date) else { return current.to_string() };
    let mut text = current.to_string();
    if !contains_name(&text, &name) {
        if !text.is_empty() { text.push_str(" · "); }
        text.push_str(&name);
    }
    if !text.is_empty() { text.push_str(" · "); }
    text.push_str(label);
    text
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Badge { Rest, Work }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaptionKind { Holiday, Makeup, Mark, Lunar }

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Caption {
    pub text: String,
    pub kind: CaptionKind,
}

pub(crate) fn badge(date: NaiveDate) -> Option<Badge> {
    match mark(date) {
        Some(Mark::Off { .. }) => Some(Badge::Rest),
        Some(Mark::Work { .. }) => Some(Badge::Work),
        None => None,
    }
}

/// 格子里的一行小字。法定放假名优先，其次补班、节气、节日，最后才是农历。
pub(crate) fn caption(date: NaiveDate) -> Option<Caption> {
    match mark(date) {
        Some(Mark::Off { name: Some(name) }) => return Some(Caption { text: short_holiday(&name), kind: CaptionKind::Holiday }),
        Some(Mark::Work { .. }) => return Some(Caption { text: "补班".into(), kind: CaptionKind::Makeup }),
        _ => {}
    }
    let mark = crate::almanac::day_mark(date)?;
    if let Some(term) = mark.term { return Some(Caption { text: term.into(), kind: CaptionKind::Mark }); }
    if let Some(festival) = mark.festival { return Some(Caption { text: festival.into(), kind: CaptionKind::Mark }); }
    Some(Caption { text: mark.lunar, kind: CaptionKind::Lunar })
}

fn short_holiday(name: &str) -> String {
    name.split(['、', '，']).next().unwrap_or(name).trim().to_string()
}

fn note(date: NaiveDate) -> Option<(String, &'static str)> {
    match mark(date) {
        Some(Mark::Off { name: Some(name) }) => Some((name, "放假")),
        Some(Mark::Work { name }) => Some((name, "补班")),
        _ => None,
    }
}

fn contains_name(beside: &str, name: &str) -> bool {
    let key = name.trim_end_matches('节');
    !key.is_empty() && beside.split('·').any(|part| part.trim().trim_end_matches('节') == key)
}

fn lookup(date: NaiveDate) -> Option<Day> {
    let mut store = store();
    if store.checked.is_none_or(|at| at.elapsed() >= Duration::from_secs(2)) || store.days.is_empty() {
        store.checked = Some(Instant::now());
        let stamps = current_stamps();
        if stamps != store.stamps || store.days.is_empty() {
            store.stamps = stamps;
            store.days = load_days();
        }
    }
    store.days.get(&date).cloned()
}

#[cfg(test)]
fn load_days() -> HashMap<NaiveDate, Day> { combine(bundled_files()) }

#[cfg(not(test))]
fn load_days() -> HashMap<NaiveDate, Day> {
    let mut files = bundled_files();
    files.extend(read_caches(&cache_dir()));
    combine(files)
}

fn bundled_files() -> Vec<YearFile> {
    [
        include_str!("../assets/holidays/2025.json"),
        include_str!("../assets/holidays/2026.json"),
        include_str!("../assets/holidays/2027.json"),
    ].into_iter().filter_map(parse).collect()
}

fn combine(mut files: Vec<YearFile>) -> HashMap<NaiveDate, Day> {
    files.sort_by_key(|file| (file.year, file.rank));
    let mut days = HashMap::new();
    for file in files {
        for (date, day) in file.days { days.insert(date, day); }
    }
    days
}

fn parse(text: &str) -> Option<YearFile> {
    let text = text.trim().trim_start_matches('\u{feff}');
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let year = value.get("year")?.as_i64()?;
    if !(1900..=2100).contains(&year) { return None; }
    let year = year as i32;
    let mut seen = HashSet::new();
    let mut days = Vec::new();
    let mut off = 0usize;
    for item in value.get("days")?.as_array()? {
        let name = item.get("name")?.as_str()?.trim();
        if !valid_name(name) { return None; }
        let date = NaiveDate::parse_from_str(item.get("date")?.as_str()?, "%Y-%m-%d").ok()?;
        if date.year() != year && date.year() != year - 1 { return None; }
        if !seen.insert(date) { return None; }
        let is_off = item.get("isOffDay")?.as_bool()?;
        if is_off { off += 1; }
        days.push((date, Day { name: name.to_string(), off: is_off }));
    }
    OFF_DAYS.contains(&off).then_some(YearFile { year, rank: 0, days })
}

fn valid_name(name: &str) -> bool {
    let count = name.chars().count();
    (1..=16).contains(&count) && name.chars().all(|ch| !ch.is_whitespace() && ch != '·' && !ch.is_control())
}

fn read_caches(dir: &Path) -> Vec<YearFile> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return files };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") { continue; }
        let Some(year) = path.file_stem().and_then(|stem| stem.to_str()).and_then(|stem| stem.parse::<i32>().ok()) else { continue };
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let Some(mut file) = parse(&text) else { continue };
        if file.year != year { continue; }
        file.rank = 1;
        files.push(file);
    }
    files
}

#[cfg(not(test))]
fn cache_dir() -> std::path::PathBuf { crate::model::config_dir().join("holidays") }

fn current_stamps() -> Vec<(String, u64, i64)> {
    #[cfg(test)]
    { Vec::new() }
    #[cfg(not(test))]
    { stamps(&cache_dir()) }
}

#[cfg(not(test))]
fn stamps(dir: &Path) -> Vec<(String, u64, i64)> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return found };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") { continue; }
        let Some(name) = path.file_name().and_then(|name| name.to_str()).map(str::to_string) else { continue };
        let Ok(meta) = entry.metadata() else { continue };
        let modified = meta.modified().ok().and_then(|time| time.duration_since(std::time::SystemTime::UNIX_EPOCH).ok()).map(|time| time.as_secs() as i64).unwrap_or(0);
        found.push((name, meta.len(), modified));
    }
    found.sort();
    found
}

fn should_fetch(cache_age: Option<Duration>) -> bool {
    cache_age.is_none_or(|age| age >= FRESH)
}

#[cfg(not(test))]
fn refresh_impl(year: i32) {
    if !(1900..=2100).contains(&year) { return; }
    let mut gate = gate();
    gate.pending.insert(year);
    let next = year.saturating_add(1);
    if next <= 2100 { gate.pending.insert(next); }
    if gate.running { return; }
    let now = Instant::now();
    let ready = gate.pending.iter().any(|year| gate.skip_until.get(year).is_none_or(|at| now >= *at));
    if !ready { return; }
    gate.running = true;
    drop(gate);
    std::thread::spawn(worker);
}

#[cfg(not(test))]
fn worker() {
    struct ClearOnPanic;
    impl Drop for ClearOnPanic {
        fn drop(&mut self) {
            if std::thread::panicking() { gate().running = false; }
        }
    }
    let _guard = ClearOnPanic;
    loop {
        let years = {
            let mut gate = gate();
            let now = Instant::now();
            let years: Vec<i32> = gate.pending.iter().copied().filter(|year| gate.skip_until.get(year).is_none_or(|at| now >= *at)).collect();
            for year in &years { gate.pending.remove(year); }
            if years.is_empty() {
                gate.running = false;
                return;
            }
            let until = now + RETRY;
            for year in &years { gate.skip_until.insert(*year, until); }
            years
        };
        for year in years {
            if should_fetch(cache_age(year)) { let _ = fetch_year(year); }
        }
    }
}

#[cfg(not(test))]
fn cache_age(year: i32) -> Option<Duration> {
    std::fs::metadata(cache_dir().join(format!("{year}.json"))).ok()?.modified().ok()?.elapsed().ok()
}

#[cfg(not(test))]
fn fetch_year(year: i32) -> bool {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(8)).timeout_connect(Duration::from_secs(4)).redirects(2).build();
    for url in mirror_urls(year) {
        let Some(text) = http_text(&agent, &url) else { continue };
        let Some(file) = parse(&text) else { continue };
        if file.year != year { continue; }
        return save(year, &text);
    }
    false
}

fn mirror_urls(year: i32) -> [String; 2] {
    [
        format!("https://cdn.jsdelivr.net/gh/NateScarlet/holiday-cn@master/{year}.json"),
        format!("https://fastly.jsdelivr.net/gh/NateScarlet/holiday-cn@master/{year}.json"),
    ]
}

#[cfg(not(test))]
fn http_text(agent: &ureq::Agent, url: &str) -> Option<String> {
    use std::io::Read;
    let response = agent.get(url).set("User-Agent", concat!("Yunyu/", env!("CARGO_PKG_VERSION"))).set("Accept", "application/json").call().ok()?;
    if response.status() != 200 { return None; }
    let mut body = Vec::new();
    response.into_reader().take(256 * 1024).read_to_end(&mut body).ok()?;
    String::from_utf8(body).ok()
}

#[cfg(not(test))]
fn save(year: i32, body: &str) -> bool {
    let dir = cache_dir();
    std::fs::create_dir_all(&dir).is_ok() && crate::model::atomic_write(&dir.join(format!("{year}.json")), body.as_bytes()).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration as DateDuration;

    fn day(text: &str) -> NaiveDate { NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap() }

    fn notice(year: i32, extra: &str) -> String {
        let mut days: Vec<String> = (1..=7).map(|day| format!(r#"{{"name":"元旦","date":"{year}-01-{day:02}","isOffDay":true}}"#)).collect();
        if !extra.is_empty() { days.push(extra.to_string()); }
        format!(r#"{{"year":{year},"days":[{}]}}"#, days.join(","))
    }

    fn assert_off(start: &str, end: &str, name: &str) {
        let mut date = day(start);
        let end = day(end);
        while date <= end {
            assert_eq!(mark(date), Some(Mark::Off { name: Some(name.into()) }), "{date}");
            date += DateDuration::days(1);
        }
    }

    #[test]
    fn bundled_notices_cover_the_published_days() {
        assert_eq!(bundled_files().len(), 3);
        assert_off("2026-01-01", "2026-01-03", "元旦");
        assert_eq!(mark(day("2026-01-04")), Some(Mark::Work { name: "元旦".into() }));
        assert_off("2026-02-15", "2026-02-23", "春节");
        assert_eq!(mark(day("2026-02-14")), Some(Mark::Work { name: "春节".into() }));
        assert_eq!(mark(day("2026-02-28")), Some(Mark::Work { name: "春节".into() }));
        assert_off("2026-04-04", "2026-04-06", "清明节");
        assert_off("2026-05-01", "2026-05-05", "劳动节");
        assert_eq!(mark(day("2026-05-09")), Some(Mark::Work { name: "劳动节".into() }));
        assert_off("2026-06-19", "2026-06-21", "端午节");
        assert_off("2026-09-25", "2026-09-27", "中秋节");
        assert_eq!(mark(day("2026-09-20")), Some(Mark::Work { name: "国庆节".into() }));
        assert_off("2026-10-01", "2026-10-07", "国庆节");
        assert_eq!(mark(day("2026-10-10")), Some(Mark::Work { name: "国庆节".into() }));
        assert_eq!(mark(day("2026-11-18")), None);
        assert_eq!(mark(day("2026-11-21")), Some(Mark::Off { name: None }));
        assert_eq!(mark(day("2026-02-07")), Some(Mark::Off { name: None }));
        assert_eq!(mark(day("2026-02-21")), Some(Mark::Off { name: Some("春节".into()) }));
        assert_eq!(mark(day("2027-01-01")), Some(Mark::Off { name: Some("元旦".into()) }));
        assert_eq!(mark(day("2027-01-31")), Some(Mark::Work { name: "春节".into() }));
        assert_eq!(mark(day("2025-09-28")), Some(Mark::Work { name: "国庆节、中秋节".into() }));
    }

    #[test]
    fn labels_name_the_holiday_once_and_leave_weekends_unchanged() {
        assert_eq!(beside(day("2026-02-15"), "腊月廿八"), "腊月廿八 · 春节 · 放假");
        assert_eq!(beside(day("2026-02-17"), "正月初一 · 春节"), "正月初一 · 春节 · 放假");
        assert_eq!(beside(day("2026-04-04"), "三月初八 · 清明"), "三月初八 · 清明 · 放假");
        assert_eq!(beside(day("2026-04-05"), "三月初九"), "三月初九 · 清明节 · 放假");
        assert_eq!(beside(day("2026-02-14"), "腊月廿七"), "腊月廿七 · 春节 · 补班");
        assert_eq!(beside(day("2026-11-21"), "十月十二"), "十月十二");
        assert_eq!(beside(day("2026-11-18"), "十月初九"), "十月初九");
        assert_eq!(beside(day("2025-09-28"), "八月初七"), "八月初七 · 国庆节、中秋节 · 补班");
        assert_eq!(hover(day("2026-02-15")), "2026年2月15日 春节 放假");
        assert_eq!(hover(day("2026-02-14")), "2026年2月14日 春节 补班");
        assert_eq!(hover(day("2026-11-18")), "2026年11月18日");
        assert_eq!(hover(day("2026-11-21")), "2026年11月21日");
        assert_eq!(tint(day("2026-11-21")), Some(Tint::Rest));
        assert_eq!(tint(day("2026-02-14")), Some(Tint::Work));
        assert_eq!(tint(day("2026-11-18")), None);
        assert_eq!(caption(day("2026-02-15")).unwrap(), Caption { text: "春节".into(), kind: CaptionKind::Holiday });
        assert_eq!(caption(day("2026-02-14")).unwrap(), Caption { text: "补班".into(), kind: CaptionKind::Makeup });
        assert_eq!(caption(day("2026-11-01")).unwrap().text, "万圣节");
        assert_eq!(caption(day("2026-11-11")).unwrap().text, "购物节");
        assert_eq!(caption(day("2026-11-26")).unwrap().text, "感恩节");
        assert_eq!(caption(day("2026-11-18")).unwrap().kind, CaptionKind::Lunar);
        assert_eq!(badge(day("2026-02-14")), Some(Badge::Work));
        assert_eq!(badge(day("2026-11-21")), Some(Badge::Rest));
        assert_eq!(badge(day("2026-11-18")), None);
        assert_eq!(caption(day("2025-09-28")).unwrap().text, "补班");
        let described = crate::almanac::describe(day("2026-02-17")).beside;
        let text = beside(day("2026-02-17"), &described);
        assert!(described.contains("春节"), "{described}");
        assert_eq!(text.matches("春节").count(), 1, "{text}");
        assert!(text.ends_with(" · 放假"), "{text}");
        let qingming = crate::almanac::describe(day("2026-04-04")).beside;
        let text = beside(day("2026-04-04"), &qingming);
        assert!(text.ends_with(" · 放假"), "{qingming} -> {text}");
        let named = qingming.split('·').any(|part| part.trim().trim_end_matches('节') == "清明");
        assert_eq!(text.contains("清明节"), !named, "{qingming} -> {text}");
    }

    #[test]
    fn a_later_notice_and_a_cache_replace_the_same_date() {
        let older = parse(&notice(2026, r#"{"name":"元旦","date":"2026-12-31","isOffDay":true}"#)).unwrap();
        let newer = parse(&notice(2027, r#"{"name":"元旦","date":"2026-12-31","isOffDay":false}"#)).unwrap();
        let days = combine(vec![newer, older]);
        let saved = days.get(&day("2026-12-31")).unwrap();
        assert!(!saved.off);
        assert_eq!(saved.name, "元旦");
        let mut bundled = parse(&notice(2026, "")).unwrap();
        let mut cache = parse(&notice(2026, "")).unwrap();
        cache.rank = 1;
        cache.days[0].1 = Day { name: "春节".into(), off: false };
        bundled.rank = 0;
        let days = combine(vec![cache, bundled]);
        let saved = days.get(&day("2026-01-01")).unwrap();
        assert!(!saved.off);
        assert_eq!(saved.name, "春节");
    }

    #[test]
    fn invalid_notices_and_mismatched_cache_names_are_ignored() {
        assert!(parse(&notice(2026, r#"{"name":"元旦","date":"2026-01-01","isOffDay":true}"#)).is_none(), "重复日期");
        assert!(parse(&notice(2026, r#"{"name":"元旦","date":"2028-01-01","isOffDay":true}"#)).is_none(), "超出通知年份");
        assert!(parse(&notice(2026, r#"{"name":" ","date":"2026-02-01","isOffDay":true}"#)).is_none(), "空名称");
        assert!(parse(&notice(2026, r#"{"name":"元旦","date":"2026-02-01"}"#)).is_none(), "缺少放假标记");
        assert!(parse(r#"{"year":2026,"days":[]}"#).is_none());
        let too_few = (1..=6).map(|day| format!(r#"{{"name":"元旦","date":"2026-01-{day:02}","isOffDay":true}}"#)).collect::<Vec<_>>().join(",");
        assert!(parse(&format!(r#"{{"year":2026,"days":[{too_few}]}}"#)).is_none());
        let too_many = (0..41).map(|offset| {
            let date = day("2026-01-01") + DateDuration::days(offset);
            format!(r#"{{"name":"元旦","date":"{}","isOffDay":true}}"#, date.format("%Y-%m-%d"))
        }).collect::<Vec<_>>().join(",");
        assert!(parse(&format!(r#"{{"year":2026,"days":[{too_many}]}}"#)).is_none());
        let spilled = parse(&notice(2027, r#"{"name":"元旦","date":"2026-12-31","isOffDay":true}"#)).unwrap();
        assert!(spilled.days.iter().any(|(date, _)| *date == day("2026-12-31")));
        let dir = std::env::temp_dir().join(format!("yunyu-holidays-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::SystemTime::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("2026.json"), notice(2026, "")).unwrap();
        std::fs::write(dir.join("2027.json"), r#"{"year":2026,"days":[]}"#).unwrap();
        std::fs::write(dir.join("notes.json"), "{}").unwrap();
        std::fs::write(dir.join("2025.json"), "{").unwrap();
        let files = read_caches(&dir);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].year, 2026);
        assert_eq!(files[0].rank, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refresh_waits_for_a_stale_cache_and_names_both_mirrors() {
        assert!(should_fetch(None));
        assert!(should_fetch(Some(FRESH)));
        assert!(!should_fetch(Some(FRESH - Duration::from_secs(1))));
        assert_eq!(mirror_urls(2026)[0], "https://cdn.jsdelivr.net/gh/NateScarlet/holiday-cn@master/2026.json");
        assert_eq!(mirror_urls(2026)[1], "https://fastly.jsdelivr.net/gh/NateScarlet/holiday-cn@master/2026.json");
    }
}
