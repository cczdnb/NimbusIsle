//! Cursor 套餐额度。只读本机登录，向 Cursor 自己的用量接口查询。
//! 令牌不写入云屿配置，也不替 Cursor 续期。
use crate::quota::{Account, Config, Provider, Snapshot, Window};
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};

const USAGE_URL: &str = "https://api2.cursor.sh/aiserver.v1.DashboardService/GetCurrentPeriodUsage";
const SUMMARY_URL: &str = "https://cursor.com/api/usage-summary";

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
    let session = read_session(&account.source).map_err(|error| (format!("读取登录：{error}"), 60))?;
    if session.token.is_empty() { return Err(("读取登录：库里没有 accessToken".into(), 0)); }
    // TUN 打开时，Windows 仍可能留着 127.0.0.1 的 HTTP 代理。那个端口经常不听，或者和虚拟网卡绕圈。
    // 代理这条连不上就改直连，流量仍由 TUN 转发；没配代理时本来就是直连，不再重试。
    let direct = configured_proxy().then(direct_agent);
    let primary = with_direct_fallback(agent, direct.as_ref(), |client| usage_rpc(client, &session.token));
    if let Ok(value) = &primary {
        if let Ok(snapshot) = parse(&with_plan(value.clone(), session.plan.as_deref())) { return Ok(snapshot); }
    }
    let summary = with_direct_fallback(agent, direct.as_ref(), |client| usage_summary(client, &session));
    if let Ok(snapshot) = summary { return Ok(snapshot); }
    let first = match &primary {
        Ok(value) => format!("用量接口：{}", parse(&with_plan(value.clone(), session.plan.as_deref())).unwrap_err()),
        Err((message, _)) => message.clone(),
    };
    let second = match summary { Err((message, _)) => message, Ok(_) => String::new() };
    Err((format!("{first}；{second}"), 60))
}

fn usage_rpc(agent: &ureq::Agent, token: &str) -> Result<Value, (String, u64)> {
    let response = agent.post(USAGE_URL)
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/json")
        .set("Connect-Protocol-Version", "1")
        .set("Accept", "application/json")
        .send_string("{}")
        .map_err(|error| match error {
            ureq::Error::Status(code @ (401 | 403), response) => {
                let (reason, wait) = rejected(token);
                (format!("用量接口 HTTP {code}：{reason}{}", brief_error(&response_text(response))), wait)
            }
            ureq::Error::Status(429, response) => ("用量接口 HTTP 429：请求过于频繁，稍后自动重试".into(), response.header("Retry-After").and_then(|value| value.parse().ok()).unwrap_or(180).clamp(30, 3600)),
            ureq::Error::Status(code, response) => (format!("用量接口 HTTP {code}{}", brief_error(&response_text(response))), 60),
            ureq::Error::Transport(transport) => (format!("用量接口：连不上 api2.cursor.sh{}", transport_note(&transport)), 30),
        })?;
    read_json(response).map_err(|error| (format!("用量接口：{error}"), 60))
}

/// 团队席和旧仪表盘没有 `planUsage`。改用网页同一条用量摘要，登录态用本机会话拼出来。
fn usage_summary(agent: &ureq::Agent, session: &Session) -> Result<Snapshot, (String, u64)> {
    let Some(cookie) = session_cookie(cookie_token(session)) else {
        return Err(("用量摘要：登录令牌里没有用户编号".into(), 60));
    };
    let response = agent.get(SUMMARY_URL).set("Accept", "application/json").set("Cookie", &cookie).call().map_err(|error| match error {
        ureq::Error::Status(code, response) => (format!("用量摘要 HTTP {code}{}", brief_error(&response_text(response))), 60),
        ureq::Error::Transport(transport) => (format!("用量摘要：连不上 cursor.com{}", transport_note(&transport)), 30),
    })?;
    let value = read_json(response).map_err(|error| (format!("用量摘要：{error}"), 60))?;
    parse(&with_plan(value, session.plan.as_deref())).map_err(|error| (format!("用量摘要：{error}"), 60))
}

fn with_direct_fallback<T>(agent: &ureq::Agent, direct: Option<&ureq::Agent>, mut call: impl FnMut(&ureq::Agent) -> Result<T, (String, u64)>) -> Result<T, (String, u64)> {
    match call(agent) {
        Ok(value) => Ok(value),
        Err((message, _)) if direct.is_some() && message.contains("连不上") => match call(direct.unwrap()) {
            Ok(value) => Ok(value),
            Err((again, wait)) => Err((format!("{message}；直连：{again}"), wait)),
        },
        Err(error) => Err(error),
    }
}

fn configured_proxy() -> bool {
    ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy", "HTTP_PROXY", "http_proxy"]
        .iter()
        .any(|key| std::env::var(key).ok().is_some_and(|value| !value.trim().is_empty()))
        || super::system_proxy().is_some()
}

fn direct_agent() -> ureq::Agent {
    ureq::AgentBuilder::new().try_proxy_from_env(false).timeout(std::time::Duration::from_secs(12)).timeout_connect(std::time::Duration::from_secs(8)).redirects(0).build()
}

fn transport_note(transport: &ureq::Transport) -> String {
    let kind = match transport.kind() {
        ureq::ErrorKind::Dns => "域名没有解析出来",
        ureq::ErrorKind::ConnectionFailed => "连接被拒绝或超时",
        ureq::ErrorKind::ProxyConnect => "系统代理没连上",
        ureq::ErrorKind::ProxyUnauthorized => "系统代理要求登录",
        ureq::ErrorKind::InvalidProxyUrl => "系统代理地址无效",
        ureq::ErrorKind::Io => "网络中断",
        _ => "网络失败",
    };
    format!("（{kind}）")
}

fn read_json(response: ureq::Response) -> Result<Value, String> {
    let body = response_text(response);
    serde_json::from_str(&body).map_err(|_| "返回的不是 JSON".into())
}

fn response_text(response: ureq::Response) -> String {
    let mut body = String::new();
    let _ = response.into_reader().take(4096).read_to_string(&mut body);
    body
}

fn brief_error(body: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(body) else { return String::new(); };
    let text = value.get("error").or_else(|| value.get("message")).and_then(Value::as_str).unwrap_or("");
    let text: String = text.chars().filter(|ch| !ch.is_control()).take(48).collect();
    if text.len() < 2 || text.contains("eyJ") { return String::new(); }
    format!("，{text}")
}

fn with_plan(mut value: Value, plan: Option<&str>) -> Value {
    if value.get("membershipType").and_then(Value::as_str).is_none_or(str::is_empty) {
        if let Some(plan) = plan.filter(|plan| !plan.is_empty()) { value["membershipType"] = Value::String(plan.to_owned()); }
    }
    value
}

pub fn parse(value: &Value) -> Result<Snapshot, String> {
    let Some(usage) = usage_bucket(value) else { return Err(format!("返回里没有套餐字段（{}）", field_names(value))); };
    let reset = cycle_end(value);
    let mut windows = Vec::new();
    if let Some(used) = percent(&usage["autoPercentUsed"]) {
        windows.push(pool("Cursor 模型", used, reset));
    }
    if let Some(used) = percent(&usage["apiPercentUsed"]) {
        windows.push(pool("其他模型", used, reset));
    }
    if windows.is_empty() {
        if let Some(used) = percent(&usage["totalPercentUsed"]) {
            windows.push(pool("套餐", used, reset));
        } else if let Some(used) = ratio_percent(usage) {
            windows.push(pool("套餐", used, reset));
        }
    }
    if windows.is_empty() { return Err(format!("套餐字段里没有百分比（{}）", field_names(usage))); }
    Ok(Snapshot { plan: plan_name(value["membershipType"].as_str().unwrap_or("")), windows, at: chrono::Utc::now().timestamp(), ..Default::default() })
}

fn field_names(value: &Value) -> String {
    let Some(object) = value.as_object() else { return if value.is_null() { "空".into() } else { "不是对象".into() }; };
    let mut names: Vec<&str> = object.keys().map(String::as_str).filter(|key| !key.to_ascii_lowercase().contains("token")).take(8).collect();
    if names.is_empty() { return "没有字段".into(); }
    names.sort_unstable();
    names.join(", ")
}

fn usage_bucket(value: &Value) -> Option<&Value> {
    let plan = &value["planUsage"];
    if plan.is_object() { return Some(plan); }
    let individual = &value["individualUsage"];
    if individual["plan"].is_object() { return Some(&individual["plan"]); }
    if individual["overall"].is_object() { return Some(&individual["overall"]); }
    None
}

fn ratio_percent(usage: &Value) -> Option<f32> {
    let used = usage.get("used").and_then(number)?;
    let limit = usage.get("limit").and_then(number).filter(|limit| *limit > 0.0)?;
    Some((used / limit * 100.0).clamp(0.0, 100.0) as f32)
}

fn number(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| value.as_str()?.parse().ok()).filter(|number| number.is_finite() && *number >= 0.0)
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

struct Session { token: String, refresh: String, email: Option<String>, plan: Option<String> }

fn cookie_token(session: &Session) -> &str {
    if jwt_text(&session.refresh, "type").as_deref() == Some("session") && jwt_text(&session.token, "type").as_deref() != Some("session") {
        &session.refresh
    } else {
        &session.token
    }
}

fn session_cookie(token: &str) -> Option<String> {
    let sub = jwt_text(token, "sub")?;
    let user = workos_user(&sub);
    if user.is_empty() || token.is_empty() { return None; }
    Some(format!("WorkosCursorSessionToken={user}%3A%3A{token}"))
}

fn workos_user(sub: &str) -> &str {
    sub.rfind("user_").map(|index| &sub[index..]).filter(|user| !user.is_empty()).unwrap_or(sub)
}

fn jwt_text(token: &str, key: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let bytes = b64url(payload)?;
    serde_json::from_slice::<Value>(&bytes).ok()?.get(key)?.as_str().map(str::to_owned)
}

/// 接口拒绝时，先看本机令牌是否还在有效期内。没过期就不要提示去重新登录。
fn rejected(token: &str) -> (String, u64) {
    match jwt_expiry(token) {
        Some(exp) if exp > chrono::Utc::now().timestamp() => ("Cursor 仍是登录状态，这次没查到额度，稍后会再试".into(), 60),
        _ => ("登录已失效，请在 Cursor 中重新登录".into(), 0),
    }
}

fn jwt_expiry(token: &str) -> Option<i64> {
    let payload = token.split('.').nth(1)?;
    let bytes = b64url(payload)?;
    serde_json::from_slice::<Value>(&bytes).ok()?.get("exp")?.as_i64()
}

fn b64url(input: &str) -> Option<Vec<u8>> {
    fn val(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'-' | b'+' => Some(62),
            b'_' | b'/' => Some(63),
            _ => None,
        }
    }
    let bytes: Vec<u8> = input.bytes().filter(|byte| *byte != b'=').collect();
    if bytes.len() % 4 == 1 { return None; }
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    let mut index = 0;
    while index + 1 < bytes.len() {
        let a = val(bytes[index])?;
        let b = val(bytes[index + 1])?;
        out.push((a << 2) | (b >> 4));
        if index + 2 >= bytes.len() { break; }
        let c = val(bytes[index + 2])?;
        out.push((b << 4) | (c >> 2));
        if index + 3 >= bytes.len() { break; }
        let d = val(bytes[index + 3])?;
        out.push((c << 6) | d);
        index += 4;
    }
    Some(out)
}

fn read_session(path: &Path) -> Result<Session, String> {
    if !path.is_file() { return Err("未找到本机 Cursor".into()); }
    let token = item(path, "cursorAuth/accessToken")?;
    let refresh = item(path, "cursorAuth/refreshToken").unwrap_or_default();
    let email = item(path, "cursorAuth/cachedEmail").ok().filter(|value| !value.is_empty());
    let plan = item(path, "cursorAuth/stripeMembershipType").ok().filter(|value| !value.is_empty());
    Ok(Session { token, refresh, email, plan })
}

fn plain(raw: &str) -> String {
    let raw = raw.trim();
    serde_json::from_str::<Value>(raw).ok().and_then(|value| value.as_str().map(str::to_owned)).unwrap_or_else(|| raw.trim_matches('"').to_owned())
}

#[cfg(not(windows))]
fn item(_path: &Path, _key: &str) -> Result<String, String> { Err("暂时读不到 Cursor 的登录状态".into()) }

#[cfg(windows)]
fn item(path: &Path, key: &str) -> Result<String, String> {
    let sql = format!("SELECT value FROM ItemTable WHERE key = '{key}' LIMIT 1");
    sqlite::text(path, &sql).map(|value| plain(&value))
}

#[cfg(windows)]
mod sqlite {
    use std::ffi::{CStr, CString};
    use std::os::raw::{c_char, c_int, c_void};
    use std::path::Path;
    const OPEN_READONLY: c_int = 1;
    const OPEN_URI: c_int = 0x40;
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
        match query(path, sql, None) {
            Err(error) if error.contains("正在使用") => query(path, sql, uri_name(path)).or(Err(error)),
            other => other,
        }
    }
    fn query(path: &Path, sql: &str, uri: Option<CString>) -> Result<String, String> {
        let owned;
        let name = if let Some(uri) = uri.as_ref() { uri } else {
            owned = CString::new(path.to_string_lossy().as_bytes()).map_err(|_| "路径无效".to_string())?;
            &owned
        };
        let flags = if uri.is_some() { OPEN_READONLY | OPEN_URI } else { OPEN_READONLY };
        let mut db = std::ptr::null_mut();
        let rc = unsafe { sqlite3_open_v2(name.as_ptr(), &mut db, flags, std::ptr::null()) };
        if rc != 0 || db.is_null() {
            if !db.is_null() { unsafe { sqlite3_close(db); } }
            return Err(sqlite_error(rc));
        }
        unsafe { sqlite3_busy_timeout(db, 1500); }
        let db = Db(db);
        let sql = CString::new(sql).map_err(|_| "语句无效".to_string())?;
        let mut stmt = std::ptr::null_mut();
        let rc = unsafe { sqlite3_prepare_v2(db.0, sql.as_ptr(), -1, &mut stmt, std::ptr::null_mut()) };
        if rc != 0 { return Err(sqlite_error(rc)); }
        let step = unsafe { sqlite3_step(stmt) };
        let value = if step == ROW {
            let ptr = unsafe { sqlite3_column_text(stmt, 0) };
            if ptr.is_null() { String::new() } else { unsafe { CStr::from_ptr(ptr.cast()) }.to_string_lossy().into_owned() }
        } else if step == 101 {
            String::new()
        } else {
            unsafe { sqlite3_finalize(stmt); }
            return Err(sqlite_error(step));
        };
        unsafe { sqlite3_finalize(stmt); }
        Ok(value)
    }
    fn sqlite_error(code: c_int) -> String {
        if code == 5 || code == 6 { "Cursor 正在使用登录数据，稍后再试".into() } else { "暂时读不到 Cursor 的登录状态".into() }
    }
    fn uri_name(path: &Path) -> Option<CString> {
        let text = path.to_string_lossy();
        let text = text.trim_start_matches(r"\\?\");
        let mut encoded = String::from("file:");
        for byte in text.replace('\\', "/").bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b':' | b'.' | b'_' | b'-' => encoded.push(byte as char),
                _ => encoded.push_str(&format!("%{byte:02X}")),
            }
        }
        encoded.push_str("?mode=ro&nolock=1");
        CString::new(encoded).ok()
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
        value["planUsage"]["totalPercentUsed"] = Value::Null;
        assert!(parse(&value).is_err());
        let team = parse(&serde_json::json!({"billingCycleEnd":"2026-05-02T00:00:00Z","spendLimitUsage":{"limitType":"team"}})).unwrap_err();
        assert!(team.contains("没有套餐字段") && team.contains("billingCycleEnd") && team.contains("spendLimitUsage"), "{team}");
        assert!(parse(&serde_json::json!({})).is_err());
    }

    #[test]
    fn team_summary_uses_the_personal_plan_pools() {
        let snapshot = parse(&serde_json::json!({
            "membershipType": "enterprise",
            "billingCycleEnd": "2026-05-02T14:11:55.000Z",
            "individualUsage": { "plan": { "autoPercentUsed": 0, "apiPercentUsed": 100, "totalPercentUsed": 100 } }
        })).unwrap();
        assert_eq!(snapshot.plan, "Enterprise");
        assert_eq!(snapshot.windows.len(), 2);
        assert!((snapshot.windows[0].remaining - 100.0).abs() < 0.01);
        assert!((snapshot.windows[1].remaining - 0.0).abs() < 0.01);
        let overall = parse(&serde_json::json!({
            "membershipType": "team",
            "individualUsage": { "overall": { "used": 25, "limit": 100 } }
        })).unwrap();
        assert_eq!(overall.plan, "Business");
        assert_eq!(overall.windows[0].label, "套餐");
        assert!((overall.windows[0].remaining - 75.0).abs() < 0.01);
    }

    #[test]
    fn session_cookie_uses_the_user_id_inside_the_subject() {
        let header = "eyJhbGciOiJub25lIn0";
        let payload = b64url_test(br#"{"sub":"auth0|user_01TEST","exp":4102444800,"type":"session"}"#);
        let token = format!("{header}.{payload}.sig");
        let cookie = session_cookie(&token).unwrap();
        assert!(cookie.starts_with("WorkosCursorSessionToken=user_01TEST%3A%3A"));
        assert!(cookie.ends_with(&token));
        assert!(!cookie.contains("auth0|"));
    }

    #[test]
    fn a_live_session_is_not_reported_as_logged_out_when_the_api_rejects_it() {
        let header = "eyJhbGciOiJub25lIn0";
        let live = format!("{header}.{}.sig", b64url_test(br#"{"exp":4102444800}"#));
        let dead = format!("{header}.{}.sig", b64url_test(br#"{"exp":1000}"#));
        let (message, wait) = rejected(&live);
        assert_eq!(message, "Cursor 仍是登录状态，这次没查到额度，稍后会再试");
        assert_eq!(wait, 60);
        assert!(jwt_expiry(&live) == Some(4102444800));
        let (message, wait) = rejected(&dead);
        assert_eq!(message, "登录已失效，请在 Cursor 中重新登录");
        assert_eq!(wait, 0);
        let (message, _) = rejected("not-a-jwt");
        assert_eq!(message, "登录已失效，请在 Cursor 中重新登录");
    }

    fn b64url_test(data: &[u8]) -> String {
        const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        for chunk in data.chunks(3) {
            let a = chunk[0];
            let b = chunk.get(1).copied().unwrap_or(0);
            let c = chunk.get(2).copied().unwrap_or(0);
            out.push(TABLE[(a >> 2) as usize] as char);
            out.push(TABLE[(((a & 3) << 4) | (b >> 4)) as usize] as char);
            if chunk.len() > 1 { out.push(TABLE[(((b & 15) << 2) | (c >> 6)) as usize] as char); }
            if chunk.len() > 2 { out.push(TABLE[(c & 63) as usize] as char); }
        }
        out
    }

    #[test]
    fn usage_past_the_cap_shows_nothing_left() {
        let snapshot = parse(&serde_json::json!({"membershipType":"pro","planUsage":{"autoPercentUsed":140,"apiPercentUsed":0}})).unwrap();
        assert_eq!(snapshot.plan, "Pro");
        assert_eq!(snapshot.windows[0].remaining, 0.0);
        assert_eq!(snapshot.windows[1].remaining, 100.0);
    }

    #[test]
    fn a_failed_proxy_connection_retries_directly() {
        let agent = ureq::AgentBuilder::new().redirects(0).middleware(|_: ureq::Request, _: ureq::MiddlewareNext| {
            Err(std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "proxy down").into())
        }).build();
        let direct = ureq::AgentBuilder::new().redirects(0).middleware(|request: ureq::Request, _: ureq::MiddlewareNext| {
            assert_eq!(request.header("Authorization"), Some("Bearer synthetic-token"));
            ureq::Response::new(200, "OK", r#"{"membershipType":"pro","planUsage":{"autoPercentUsed":12}}"#)
        }).build();
        let value = super::with_direct_fallback(&agent, Some(&direct), |client| super::usage_rpc(client, "synthetic-token")).unwrap();
        let snapshot = parse(&value).unwrap();
        assert_eq!(snapshot.windows[0].label, "Cursor 模型");
        assert!((snapshot.windows[0].remaining - 88.0).abs() < 0.01);
    }

    #[test]
    fn an_http_rejection_does_not_retry_directly() {
        let agent = ureq::AgentBuilder::new().redirects(0).middleware(|_: ureq::Request, _: ureq::MiddlewareNext| {
            ureq::Response::new(401, "Unauthorized", "{}")
        }).build();
        let direct = ureq::AgentBuilder::new().redirects(0).middleware(|_: ureq::Request, _: ureq::MiddlewareNext| {
            panic!("HTTP 401 不应改走直连");
        }).build();
        let (message, _) = super::with_direct_fallback(&agent, Some(&direct), |client| super::usage_rpc(client, "synthetic-token")).unwrap_err();
        assert!(message.contains("HTTP 401") && !message.contains("直连"), "{message}");
    }
}
