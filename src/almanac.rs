//! 离线农历、节气、常见节日和宜忌。不联网，也不依赖外部历法库。
use chrono::{Datelike, NaiveDate};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Almanac {
    /// 农历、节气、节日，放在日期标题旁。算不出时为「无」。
    pub beside: String,
    /// 当日宜忌，放在日程列表上方。算不出时为「无」。
    pub advice: String,
}

pub fn describe(date: NaiveDate) -> Almanac {
    let Some(lunar) = lunar_date(date) else {
        return Almanac { beside: "无".into(), advice: "无".into() };
    };
    let mut parts = vec![lunar.label()];
    if let Some(term) = solar_term(date) { parts.push(term.to_string()); }
    if let Some(festival) = festival(date, &lunar) { parts.push(festival.to_string()); }
    Almanac { beside: parts.join(" · "), advice: advice(date, &lunar) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Lunar {
    year: i32,
    month: u8,
    leap: bool,
    day: u8,
}

impl Lunar {
    fn label(self) -> String {
        format!("{}{}月{}", if self.leap { "闰" } else { "" }, MONTHS[self.month as usize - 1], day_name(self.day))
    }
}

const MONTHS: [&str; 12] = ["正", "二", "三", "四", "五", "六", "七", "八", "九", "十", "冬", "腊"];
const STEMS: [&str; 10] = ["甲", "乙", "丙", "丁", "戊", "己", "庚", "辛", "壬", "癸"];
const BRANCHES: [&str; 12] = ["子", "丑", "寅", "卯", "辰", "巳", "午", "未", "申", "酉", "戌", "亥"];
const TERMS: [(&str, f64); 24] = [
    ("小寒", 285.0), ("大寒", 300.0), ("立春", 315.0), ("雨水", 330.0), ("惊蛰", 345.0), ("春分", 0.0),
    ("清明", 15.0), ("谷雨", 30.0), ("立夏", 45.0), ("小满", 60.0), ("芒种", 75.0), ("夏至", 90.0),
    ("小暑", 105.0), ("大暑", 120.0), ("立秋", 135.0), ("处暑", 150.0), ("白露", 165.0), ("秋分", 180.0),
    ("寒露", 195.0), ("霜降", 210.0), ("立冬", 225.0), ("小雪", 240.0), ("大雪", 255.0), ("冬至", 270.0),
];
const ACTIVITIES: [(&str, &str); 12] = [
    ("出行", "动土"), ("扫除", "求医"), ("祈福", "服药"), ("修造", "诉讼"),
    ("签约", "远行"), ("纳财", "开市"), ("求医", "嫁娶"), ("安床", "登高"),
    ("开市", "诉讼"), ("纳财", "开仓"), ("开业", "安葬"), ("筑堤", "开市"),
];

fn day_name(day: u8) -> String {
    const ONES: [&str; 10] = ["", "一", "二", "三", "四", "五", "六", "七", "八", "九"];
    match day {
        10 => "初十".into(),
        20 => "二十".into(),
        30 => "三十".into(),
        1..=9 => format!("初{}", ONES[day as usize]),
        11..=19 => format!("十{}", ONES[day as usize - 10]),
        21..=29 => format!("廿{}", ONES[day as usize - 20]),
        _ => "无".into(),
    }
}

fn festival(date: NaiveDate, lunar: &Lunar) -> Option<&'static str> {
    let gregorian = match (date.month(), date.day()) {
        (1, 1) => Some("元旦"),
        (2, 14) => Some("情人节"),
        (3, 8) => Some("妇女节"),
        (3, 12) => Some("植树节"),
        (5, 1) => Some("劳动节"),
        (5, 4) => Some("青年节"),
        (6, 1) => Some("儿童节"),
        (9, 10) => Some("教师节"),
        (10, 1) => Some("国庆节"),
        (12, 25) => Some("圣诞节"),
        _ => None,
    };
    if gregorian.is_some() { return gregorian; }
    if lunar.leap { return None; }
    match (lunar.month, lunar.day) {
        (1, 1) => Some("春节"),
        (1, 15) => Some("元宵节"),
        (5, 5) => Some("端午节"),
        (7, 7) => Some("七夕"),
        (7, 15) => Some("中元节"),
        (8, 15) => Some("中秋节"),
        (9, 9) => Some("重阳节"),
        (12, 8) => Some("腊八节"),
        _ => None,
    }.or_else(|| {
        let next = date.succ_opt()?;
        lunar_date(next).filter(|tomorrow| !tomorrow.leap && tomorrow.month == 1 && tomorrow.day == 1).map(|_| "除夕")
    })
}

fn advice(date: NaiveDate, lunar: &Lunar) -> String {
    let branch = day_cycle(date) % 12;
    let build = month_branch(lunar.month);
    let index = (branch + 12 - build) % 12;
    let (good, bad) = ACTIVITIES[index];
    format!("宜 {good}　忌 {bad}")
}

fn year_name(year: i32) -> String {
    let index = (year - 1984).rem_euclid(60) as usize;
    format!("{}{}", STEMS[index % 10], BRANCHES[index % 12])
}

fn month_branch(month: u8) -> usize {
    // 正月建寅，二月建卯，冬月建子，腊月建丑。
    ((month as usize) + 1) % 12
}

fn day_cycle(date: NaiveDate) -> usize {
    // 1970-01-01 为己酉日，干支序号 45。
    (julian_day_number(date.year(), date.month() as i32, date.day() as i32) as i64 + 17).rem_euclid(60) as usize
}

fn lunar_date(date: NaiveDate) -> Option<Lunar> {
    if !(1901..=2099).contains(&date.year()) { return None; }
    let today = china_midnight(date);
    // 朔可能发生在当天较晚的时刻，用这一天结束前的时刻来判断它属于哪一天。
    let (start_k, start_jd) = new_moon_on_or_before(today + 1.0 - 1.0 / 1440.0);
    let start_day = civil_china(start_jd)?;
    if start_day > date { return None; }
    let day = (date.signed_duration_since(start_day).num_days() + 1) as u8;
    if !(1..=30).contains(&day) { return None; }
    let (month, leap, year) = month_of(start_k)?;
    Some(Lunar { year, month, leap, day })
}

fn month_of(start_k: i32) -> Option<(u8, bool, i32)> {
    let start = new_moon(start_k);
    let solstice = winter_solstice_on_or_before(start);
    let next_solstice = winter_solstice_on_or_before(solstice + 370.0);
    let mut k = ((solstice - 40.0 - 2451550.09766) / 29.530588861).floor() as i32;
    let mut moons = Vec::new();
    while moons.len() < 18 {
        let jd = new_moon(k);
        if jd > next_solstice + 35.0 { break; }
        if jd >= solstice - 35.0 { moons.push((k, jd)); }
        k += 1;
    }
    let contains = |jd: f64| {
        let next = new_moon_after(jd);
        crosses(jd, next, 270.0)
    };
    let eleventh = moons.iter().position(|(_, jd)| contains(*jd))?;
    let next_eleventh = moons.iter().rposition(|(_, jd)| contains(*jd)).filter(|index| *index > eleventh)?;
    let span = moons[eleventh..next_eleventh].to_vec();
    if span.len() < 12 { return None; }
    let leap_slot = if span.len() >= 13 {
        span.windows(2).position(|pair| !contains_major(pair[0].1, pair[1].1)).filter(|index| *index > 0)
    } else { None };
    let mut number = 11u8;
    for (index, (k, _)) in span.iter().enumerate() {
        let leap = leap_slot == Some(index);
        if *k == start_k {
            let base = civil_china(solstice).map(|day| day.year()).unwrap_or(1900);
            let year = if number >= 11 { base } else { base + 1 };
            return Some((number, leap, year));
        }
        if !leap { number = if number == 12 { 1 } else { number + 1 }; }
    }
    None
}

fn contains_major(start: f64, end: f64) -> bool {
    (1..=11).any(|step| crosses(start, end, step as f64 * 30.0))
}

fn crosses(start: f64, end: f64, target: f64) -> bool {
    let a = sun_longitude(start);
    let b = sun_longitude(end);
    let travel = (b - a).rem_euclid(360.0);
    let delta = (target - a).rem_euclid(360.0);
    travel > 0.2 && delta > 0.05 && delta < travel - 0.05
}

fn winter_solstice_on_or_before(jd: f64) -> f64 {
    let mut lo = jd - 370.0;
    let mut hi = jd;
    for _ in 0..40 {
        let mid = (lo + hi) / 2.0;
        if crosses(mid, jd.max(mid + 0.01), 270.0) || sun_longitude(mid) > 270.0 && sun_longitude(jd) >= 270.0 {
            lo = mid;
        } else { hi = mid; }
    }
    // 取冬至之前最近的朔之前的一个确定下界：直接返回小于等于 jd、黄经刚过 270 的时刻。
    let mut day = jd.floor();
    while !crosses(day - 1.0, day, 270.0) && jd - day < 370.0 { day -= 1.0; }
    day
}

fn new_moon_after(jd: f64) -> f64 {
    let (_, current) = new_moon_on_or_before(jd);
    let k = ((current - 2451550.09766) / 29.530588861).round() as i32;
    new_moon(k + 1)
}

fn new_moon_on_or_before(jd: f64) -> (i32, f64) {
    let mut k = ((jd - 2451550.09766) / 29.530588861).floor() as i32;
    while new_moon(k + 1) <= jd + 0.0001 { k += 1; }
    while new_moon(k) > jd + 0.0001 { k -= 1; }
    (k, new_moon(k))
}

fn solar_term(date: NaiveDate) -> Option<&'static str> {
    let start = china_midnight(date);
    TERMS.into_iter().find(|(_, longitude)| crosses(start, start + 1.0, *longitude)).map(|(name, _)| name)
}

/// 平朔加上主要均数，结果是力学时儒略日，误差小于半日，足够判断中国历日。
fn new_moon(k: i32) -> f64 {
    let k = k as f64;
    let t = k / 1236.85;
    let t2 = t * t;
    let t3 = t2 * t;
    let t4 = t3 * t;
    let jde = 2451550.09766 + 29.530588861 * k + 0.00015437 * t2 - 0.000000150 * t3 + 0.00000000073 * t4;
    let e = 1.0 - 0.002516 * t - 0.0000074 * t2;
    let sun = (2.5534 + 29.10535670 * k - 0.0000014 * t2).to_radians();
    let moon = (201.5643 + 385.81693528 * k + 0.0107582 * t2 + 0.00001238 * t3).to_radians();
    let arg = (160.7108 + 390.67050284 * k - 0.0016118 * t2 - 0.00000227 * t3).to_radians();
    let mut correction = -0.40720 * arg.sin();
    correction += 0.17241 * e * sun.sin();
    correction += 0.01608 * (2.0 * arg).sin();
    correction += 0.01039 * (2.0 * moon).sin();
    correction += 0.00739 * e * (arg - sun).sin();
    correction -= 0.00514 * e * (arg + sun).sin();
    correction += 0.00208 * e * e * (2.0 * sun).sin();
    correction -= 0.00111 * (arg - 2.0 * moon).sin();
    correction -= 0.00057 * (arg + 2.0 * moon).sin();
    correction += 0.00042 * e * (sun + 2.0 * moon).sin();
    correction -= 0.00042 * (3.0 * arg).sin();
    jde + correction
}

fn sun_longitude(jd: f64) -> f64 {
    let t = (jd - 2451545.0) / 36525.0;
    let mean = 280.46646 + 36000.76983 * t + 0.0003032 * t * t;
    let anomaly = (357.52911 + 35999.05029 * t - 0.0001537 * t * t).to_radians();
    let equation = (1.914602 - 0.004817 * t) * anomaly.sin()
        + (0.019993 - 0.000101 * t) * (2.0 * anomaly).sin()
        + 0.000289 * (3.0 * anomaly).sin();
    (mean + equation).rem_euclid(360.0)
}

fn china_midnight(date: NaiveDate) -> f64 {
    julian_day_number(date.year(), date.month() as i32, date.day() as i32) as f64 - 0.5 - 8.0 / 24.0
}

fn civil_china(jd: f64) -> Option<NaiveDate> {
    let (year, month, day) = civil_from_jd(jd + 8.0 / 24.0);
    NaiveDate::from_ymd_opt(year, month, day)
}

fn julian_day_number(year: i32, month: i32, day: i32) -> i32 {
    let a = (14 - month) / 12;
    let y = year + 4800 - a;
    let m = month + 12 * a - 3;
    day + (153 * m + 2) / 5 + 365 * y + y / 4 - y / 100 + y / 400 - 32045
}

fn civil_from_jd(jd: f64) -> (i32, u32, u32) {
    let z = (jd + 0.5).floor() as i32;
    let a = z + 32044;
    let b = (4 * a + 3) / 146097;
    let c = a - (146097 * b) / 4;
    let d = (4 * c + 3) / 1461;
    let e = c - (1461 * d) / 4;
    let m = (5 * e + 2) / 153;
    let day = (e - (153 * m + 2) / 5 + 1) as u32;
    let month = (m + 3 - 12 * (m / 10)) as u32;
    let year = 100 * b + d - 4800 + m / 10;
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(text: &str) -> NaiveDate { NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap() }

    #[test]
    fn spring_festivals_are_the_first_of_the_first_month() {
        for date in ["2020-01-25", "2023-01-22", "2024-02-10", "2025-01-29", "2026-02-17"] {
            let lunar = lunar_date(day(date)).unwrap_or_else(|| panic!("{date} 应能算出农历"));
            assert_eq!((lunar.month, lunar.day, lunar.leap), (1, 1, false), "{date} 是春节");
            assert_eq!(year_name(lunar.year), match date { "2020-01-25" => "庚子", "2023-01-22" => "癸卯", "2024-02-10" => "甲辰", "2025-01-29" => "乙巳", "2026-02-17" => "丙午", _ => "" }, "{date}");
            assert!(describe(day(date)).beside.contains("春节"), "{date}");
        }
    }

    #[test]
    fn known_solar_terms_and_a_day_without_one() {
        assert_eq!(solar_term(day("2024-02-04")), Some("立春"));
        assert_eq!(solar_term(day("2024-12-21")), Some("冬至"));
        assert_eq!(solar_term(day("2024-06-15")), None);
        assert!(describe(day("2024-06-15")).beside.contains("月"));
    }

    #[test]
    fn out_of_range_dates_say_none() {
        let old = describe(day("1899-12-31"));
        assert_eq!(old.beside, "无");
        assert_eq!(old.advice, "无");
    }

    #[test]
    fn advice_names_one_suitable_and_one_unsuitable_activity() {
        let text = describe(day("2024-02-10")).advice;
        assert!(text.starts_with("宜 ") && text.contains("忌 "), "{text}");
    }
}
