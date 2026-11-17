//! Cursor 套餐额度。只读本机登录，向 Cursor 自己的用量接口查询。
//! 令牌不写入云屿配置，也不替 Cursor 续期。
use crate::quota::{Account, Config, Provider, Snapshot, Window};
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};

const USAGE_URL: &str = "https://api2.cursor.sh/aiserver.v1.DashboardService/GetCurrentPeriodUsage";

pub fn db_path() -> Option<PathBuf> {
    let roaming = std::env::var_os("APPDATA").map(PathBuf::from)?;
    let path = roaming.join("Cursor").join("User").join("globalStorage").join("state.vscdb");
    path.is_file().then_some(path)
}

pub fn connect(config: &mut Config, alias: &str) -> Result<(), String> {
    let path = db_path().ok_or("未找到本机 Cursor")?;
    let session = read_session(&path)?;
    if session.token.is_empty() { return Err("请先在 Cursor 中登录".into()); }
    let path = std::fs::canonicalize(&path).unwrap_or(path);
    if let Some(existing) = config.accounts.iter_mut().find(|account| account.provider == Provider::Cursor && account.source == path) {
        existing.enabled = true;
        if !alias.trim().is_empty() { existing.alias = alias.trim().to_owned(); }
        if config.selected.is_empty() { config.selected = existing.id.clone(); }
        return Ok(());
    }
    let id = format!("Cursor-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
    let label = if alias.trim().is_empty() { session.email.filter(|email| !email.is_empty()).unwrap_or_else(|| "Cursor".into()) } else { alias.trim().to_owned() };
    let pinned = config.accounts.iter().filter(|account| account.pinned).count() < 3;
    if config.accounts.is_empty() || config.selected.is_empty() { config.selected = id.clone(); }
    config.accounts.push(Account { id, provider: Provider::Cursor, alias: label, source: path, slot: None, enabled: true, pinned });
    Ok(())
}

pub fn fetch(agent: &ureq::Agent, account: &Account) -> Result<Snapshot, (String, u64)> {
    let session = read_session(&account.source).map_err(|error| (error, 60))?;
    if session.token.is_empty() { return Err(("请先在 Cursor 中登录".into(), 0)); }
    let response = agent.post(USAGE_URL)
        .set("Authorization", &format!("Bearer {}", session.token))
        .set("Content-Type", "application/json")
        .set("Connect-Protocol-Version", "1")
        .set("Accept", "application/json")
        .send_string("{}")
        .map_err(|error| match error {
            ureq::Error::Status(401 | 403, _) => ("登录已失效，请在 Cursor 中重新登录".into(), 0),
            ureq::Error::Status(429, response) => ("请求过于频繁，稍后自动重试".into(), response.header("Retry-After").and_then(|value| value.parse().ok()).unwrap_or(180).clamp(30, 3600)),
            ureq::Error::Status(_, _) => ("暂时读不到套餐额度".into(), 60),
            _ => ("无法连接 Cursor 额度服务，请检查网络或代理".into(), 30),
        })?;
    let mut body = String::new();
    response.into_reader().take(1024 * 1024).read_to_string(&mut body).map_err(|_| ("读取额度响应失败".into(), 30))?;
    let mut value: Value = serde_json::from_str(&body).map_err(|_| ("暂时读不到套餐额度".into(), 60))?;
    if value.get("membershipType").and_then(Value::as_str).is_none_or(str::is_empty) {
        if let Some(plan) = session.plan { value["membershipType"] = Value::String(plan); }
    }
    parse(&value).map_err(|error| (error, 60))
}

pub fn parse(value: &Value) -> Result<Snapshot, String> {
    let usage = &value["planUsage"];
    if !usage.is_object() { return Err("暂时读不到套餐额度".into()); }
    let reset = cycle_end(value);
    let mut windows = Vec::new();
    if let Some(used) = percent(&usage["autoPercentUsed"]) {
        windows.push(pool("Cursor 模型", used, reset));
    }
    if let Some(used) = percent(&usage["apiPercentUsed"]) {
        windows.push(pool("其他模型", used, reset));
    }
    if windows.is_empty() { return Err("暂时读不到套餐额度".into()); }
    Ok(Snapshot { plan: plan_name(value["membershipType"].as_str().unwrap_or("")), windows, at: chrono::Utc::now().timestamp(), ..Default::default() })
}

fn pool(label: &str, used: f32, reset: Option<i64>) -> Window {
    let remaining = (100.0 - used).clamp(0.0, 100.0);
    Window { label: label.into(), seconds: None, remaining, reset, amount: None }
}

fn percent(value: &Value) -> Option<f32> {
    let number = value.as_f64().or_else(|| value.as_str()?.parse().ok()).filter(|number| number.is_finite() && *number >= 0.0)?;
    Some(number.min(100.0) as f32)
}

fn plan_name(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "ultra" | "unified_ultra" => "Ultra".into(),
        "pro" => "Pro".into(),
        "pro_plus" | "proplus" | "pro+" => "Pro+".into(),
        "free" | "free_trial" => "Free".into(),
        "business" | "team" => "Business".into(),
        "enterprise" => "Enterprise".into(),
        "" => String::new(),
        other => other.to_owned(),
    }
}

fn cycle_end(value: &Value) -> Option<i64> {
    let raw = value.get("billingCycleEnd").filter(|item| !item.is_null())?;
    if let Some(millis) = raw.as_i64().or_else(|| raw.as_str().and_then(|text| text.parse().ok())) {
        return Some(if millis > 10_000_000_000 { millis / 1000 } else { millis });
    }
    raw.as_str().and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok().map(|time| time.timestamp()))
}

struct Session { token: String, email: Option<String>, plan: Option<String> }

fn read_session(path: &Path) -> Result<Session, String> {
    if !path.is_file() { return Err("未找到本机 Cursor".into()); }
    let token = item(path, "cursorAuth/accessToken").unwrap_or_default();
    let email = item(path, "cursorAuth/cachedEmail").filter(|value| !value.is_empty());
    let plan = item(path, "cursorAuth/stripeMembershipType").filter(|value| !value.is_empty());
    Ok(Session { token, email, plan })
}

fn plain(raw: &str) -> String {
    let raw = raw.trim();
    serde_json::from_str::<Value>(raw).ok().and_then(|value| value.as_str().map(str::to_owned)).unwrap_or_else(|| raw.trim_matches('"').to_owned())
}

#[cfg(not(windows))]
fn item(_path: &Path, _key: &str) -> Option<String> { None }

#[cfg(windows)]
fn item(path: &Path, key: &str) -> Option<String> {
    let sql = format!("SELECT value FROM ItemTable WHERE key = '{key}' LIMIT 1");
    sqlite::text(path, &sql).ok().map(|value| plain(&value)).filter(|value| !value.is_empty())
}

#[cfg(windows)]
mod sqlite {
    use std::ffi::{CStr, CString};
    use std::os::raw::{c_char, c_int, c_void};
    use std::path::Path;
    const OPEN_READONLY: c_int = 1;
    const ROW: c_int = 100;
    #[link(name = "winsqlite3")]
    extern "system" {
        fn sqlite3_open_v2(filename: *const c_char, db: *mut *mut c_void, flags: c_int, vfs: *const c_char) -> c_int;
        fn sqlite3_close(db: *mut c_void) -> c_int;
        fn sqlite3_prepare_v2(db: *mut c_void, sql: *const c_char, nbytes: c_int, stmt: *mut *mut c_void, tail: *mut *const c_char) -> c_int;
        fn sqlite3_step(stmt: *mut c_void) -> c_int;
        fn sqlite3_column_text(stmt: *mut c_void, col: c_int) -> *const u8;
        fn sqlite3_finalize(stmt: *mut c_void) -> c_int;
        fn sqlite3_busy_timeout(db: *mut c_void, ms: c_int) -> c_int;
    }
    struct Db(*mut c_void);
    impl Drop for Db { fn drop(&mut self) { if !self.0.is_null() { unsafe { sqlite3_close(self.0); } } } }
    pub fn text(path: &Path, sql: &str) -> Result<String, String> {
        let name = CString::new(path.to_string_lossy().as_bytes()).map_err(|_| "路径无效".to_string())?;
        let mut db = std::ptr::null_mut();
        let rc = unsafe { sqlite3_open_v2(name.as_ptr(), &mut db, OPEN_READONLY, std::ptr::null()) };
        if rc != 0 || db.is_null() { return Err("无法读取 Cursor 登录".into()); }
        unsafe { sqlite3_busy_timeout(db, 200); }
        let db = Db(db);
        let sql = CString::new(sql).map_err(|_| "语句无效".to_string())?;
        let mut stmt = std::ptr::null_mut();
        let rc = unsafe { sqlite3_prepare_v2(db.0, sql.as_ptr(), -1, &mut stmt, std::ptr::null_mut()) };
        if rc != 0 { return Err("无法读取 Cursor 登录".into()); }
        let value = if unsafe { sqlite3_step(stmt) } == ROW {
            let ptr = unsafe { sqlite3_column_text(stmt, 0) };
            if ptr.is_null() { String::new() } else { unsafe { CStr::from_ptr(ptr.cast()) }.to_string_lossy().into_owned() }
        } else { String::new() };
        unsafe { sqlite3_finalize(stmt); }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Value {
        serde_json::json!({
            "membershipType": "ultra",
            "billingCycleEnd": "2026-11-25T06:32:13.504Z",
            "planUsage": { "autoPercentUsed": 52.0, "apiPercentUsed": 99.0, "totalPercentUsed": 70.0 }
        })
    }

    #[test]
    fn two_pools_stay_separate_and_the_total_is_not_a_third_ring() {
        let snapshot = parse(&sample()).unwrap();
        assert_eq!(snapshot.plan, "Ultra");
        assert_eq!(snapshot.windows.len(), 2);
        assert_eq!(snapshot.windows[0].label, "Cursor 模型");
        assert!((snapshot.windows[0].remaining - 48.0).abs() < 0.01);
        assert_eq!(snapshot.windows[1].label, "其他模型");
        assert!((snapshot.windows[1].remaining - 1.0).abs() < 0.01);
        assert!(snapshot.windows.iter().all(|window| window.reset == Some(chrono::DateTime::parse_from_rfc3339("2026-11-25T06:32:13.504Z").unwrap().timestamp())));
        assert!(snapshot.rings().len() == 2);
    }

    #[test]
    fn a_missing_pool_is_omitted_and_both_missing_is_an_error() {
        let mut value = sample();
        value["planUsage"]["apiPercentUsed"] = Value::Null;
        value["membershipType"] = serde_json::json!("unified_ultra");
        let snapshot = parse(&value).unwrap();
        assert_eq!(snapshot.plan, "Ultra");
        assert_eq!(snapshot.windows.len(), 1);
        assert_eq!(snapshot.windows[0].label, "Cursor 模型");
        value["planUsage"]["autoPercentUsed"] = Value::Null;
        assert!(parse(&value).is_err());
        assert!(parse(&serde_json::json!({})).is_err());
    }

    #[test]
    fn usage_past_the_cap_shows_nothing_left() {
        let snapshot = parse(&serde_json::json!({"membershipType":"pro","planUsage":{"autoPercentUsed":140,"apiPercentUsed":0}})).unwrap();
        assert_eq!(snapshot.plan, "Pro");
        assert_eq!(snapshot.windows[0].remaining, 0.0);
        assert_eq!(snapshot.windows[1].remaining, 100.0);
    }
}
