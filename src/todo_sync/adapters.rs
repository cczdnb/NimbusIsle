//! 各数据源的拉取协议（在 Rust 中按原 API 重写，不搬原 C# 代码）。

use super::Source;
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Bucket { Due = 0, Soon = 1, Later = 2, Unscheduled = 3 }
impl Bucket {
    pub fn label(self) -> &'static str {
        match self {
            Self::Due => "到期",
            Self::Soon => "即将",
            Self::Later => "其他",
            Self::Unscheduled => "未排期",
        }
    }
    /// 进入粘性提醒的紧急档：即将=0，到期=1。其他分组不提醒。
    pub fn nudge_level(self) -> Option<u8> {
        match self {
            Self::Due => Some(1),
            Self::Soon => Some(0),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RemoteItem {
    pub key: String,
    pub summary: String,
    pub due: Option<String>,
    pub status: Option<String>,
    pub url: Option<String>,
    pub priority: u8, // 0=P0, 1=P1, 2=P2
    pub ext_id: Option<String>,
    pub list_id: Option<String>,
    pub source: Source,
    pub bucket: Bucket,
    pub done: bool,
    /// 最近更新时间（Unix 秒）。未排期分组用它按新到旧排序。
    pub updated_at: Option<i64>,
}

impl RemoteItem {
    pub fn done_key(&self) -> String {
        format!("{}:{}", self.source.key(), self.ext_id.as_deref().unwrap_or(&self.key))
    }
    pub fn priority_label(&self) -> &'static str {
        match self.priority { 0 => "P0", 1 => "P1", _ => "P2" }
    }
    pub fn priority_color(&self) -> (u8, u8, u8) {
        match self.priority {
            0 => (0xFF, 0x5F, 0x5F),
            1 => (0xFF, 0xB0, 0x40),
            _ => (0x96, 0xA0, 0xAF),
        }
    }
}

pub struct SyncOutcome {
    pub ok: bool,
    pub message: String,
    pub items: Vec<RemoteItem>,
}

impl SyncOutcome {
    fn fail(msg: impl Into<String>) -> Self {
        Self { ok: false, message: msg.into(), items: Vec::new() }
    }
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(45))
        .user_agent(&format!("Daozai/{} (Windows; TodoSync)", env!("CARGO_PKG_VERSION")))
        .build()
}

fn short(s: &str) -> String {
    let t = s.replace(['\r', '\n'], " ").trim().to_string();
    if t.chars().count() <= 180 { t } else { t.chars().take(180).collect::<String>() + "…" }
}

fn read_body(resp: ureq::Response) -> Result<String, String> {
    use std::io::Read;
    let mut t = String::new();
    resp.into_reader().take(2 * 1024 * 1024).read_to_string(&mut t)
        .map_err(|e| format!("读取响应失败：{e}"))?;
    Ok(t)
}

fn call(req: ureq::Request) -> Result<(u16, String), String> {
    match req.call() {
        Ok(resp) => {
            let status = resp.status();
            let body = read_body(resp)?;
            Ok((status, body))
        }
        Err(ureq::Error::Status(code, resp)) => {
            let body = read_body(resp).unwrap_or_default();
            Err(format!("HTTP {code}: {}", short(&body)))
        }
        Err(ureq::Error::Transport(e)) => Err(format!("网络错误：{e}")),
    }
}

fn send(mut req: ureq::Request, body: Option<&str>) -> Result<Value, String> {
    let (status, text) = if let Some(body) = body {
        match req.set("Content-Type", "application/json").send_string(body) {
            Ok(resp) => (resp.status(), read_body(resp)?),
            Err(ureq::Error::Status(code, resp)) => {
                let b = read_body(resp).unwrap_or_default();
                return Err(format!("HTTP {code}: {}", short(&b)));
            }
            Err(ureq::Error::Transport(e)) => return Err(format!("网络错误：{e}")),
        }
    } else {
        call(req)?
    };
    if status >= 400 { return Err(format!("HTTP {status}: {}", short(&text))); }
    if text.trim().is_empty() { return Ok(json!({})); }
    serde_json::from_str(&text).map_err(|e| format!("JSON 解析失败：{e}"))
}

fn get_json(url: &str, headers: &[(&str, &str)]) -> Result<Value, String> {
    let mut req = agent().get(url);
    for (k, v) in headers { req = req.set(k, v); }
    send(req, None)
}

fn post_json(url: &str, headers: &[(&str, &str)], body: &str) -> Result<Value, String> {
    let mut req = agent().post(url);
    for (k, v) in headers { req = req.set(k, v); }
    send(req, Some(body))
}

pub fn put_json(url: &str, headers: &[(&str, &str)], body: &str) -> Result<Value, String> {
    let mut req = agent().put(url);
    for (k, v) in headers { req = req.set(k, v); }
    send(req, Some(body))
}

pub fn patch_json(url: &str, headers: &[(&str, &str)], body: &str) -> Result<Value, String> {
    let mut req = agent().request("PATCH", url);
    for (k, v) in headers { req = req.set(k, v); }
    send(req, Some(body))
}

pub fn sync(source: Source, address: &str, token: &str, soon_days: u32, interactive: bool) -> SyncOutcome {
    let soon_days = soon_days.clamp(1, 30);
    let result = match source {
        Source::Jira => sync_jira(address, token, soon_days),
        Source::Linear => sync_linear(token, soon_days),
        Source::GitHub => sync_github(address, token, soon_days),
        Source::Notion => sync_notion(address, token, soon_days),
        Source::Feishu => sync_feishu(address, token, soon_days),
        Source::WindowsTodo => sync_windows(address, soon_days, interactive),
        Source::CustomJson => sync_json(address, token),
    };
    match result {
        Ok(items) => {
            let (due, soon, none, later) = count_buckets(&items);
            SyncOutcome {
                ok: true,
                message: format!("已同步 · 到期 {due} · 即将 {soon} · 未排期 {none} · 其他 {later}"),
                items,
            }
        }
        Err(msg) => SyncOutcome::fail(msg),
    }
}

fn count_buckets(items: &[RemoteItem]) -> (usize, usize, usize, usize) {
    let mut d = 0; let mut s = 0; let mut n = 0; let mut later = 0;
    for it in items {
        if it.done { continue; }
        match it.bucket {
            Bucket::Due => d += 1,
            Bucket::Soon => s += 1,
            Bucket::Unscheduled => n += 1,
            Bucket::Later => later += 1,
        }
    }
    (d, s, n, later)
}

fn today() -> chrono::NaiveDate { chrono::Local::now().date_naive() }

pub fn assign_bucket(due: Option<&str>, soon_days: u32) -> Bucket {
    let Some(raw) = due else { return Bucket::Unscheduled; };
    let Some(d) = parse_date(raw) else { return Bucket::Unscheduled; };
    let t = today();
    if d <= t { Bucket::Due }
    else if d <= t + chrono::Duration::days(soon_days as i64) { Bucket::Soon }
    else { Bucket::Later }
}

pub fn parse_date(raw: &str) -> Option<chrono::NaiveDate> {
    let raw = raw.trim();
    if raw.is_empty() { return None; }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(&raw[..raw.len().min(10)], "%Y-%m-%d") { return Some(d); }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(raw, "%Y/%m/%d") { return Some(d); }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(raw) { return Some(dt.with_timezone(&chrono::Local).date_naive()); }
    chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S").ok()
        .or_else(|| chrono::NaiveDateTime::parse_from_str(&raw.replace('T', " "), "%Y-%m-%d %H:%M:%S").ok().map(|d| d.date()))
}

pub fn format_date(d: chrono::NaiveDate) -> String { d.format("%Y-%m-%d").to_string() }

/// 把各源的更新时间解析成 Unix 秒，供未排期分组按新到旧排序。
pub fn parse_updated_stamp(raw: &str) -> Option<i64> {
    let raw = raw.trim();
    if raw.is_empty() { return None; }
    if let Ok(n) = raw.parse::<i64>() {
        return Some(if n > 10_000_000_000 { n / 1000 } else { n });
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(raw) { return Some(dt.timestamp()); }
    let norm = normalize_tz_colon(raw);
    if norm != raw {
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&norm) { return Some(dt.timestamp()); }
    }
    parse_date(raw).and_then(|d| d.and_hms_opt(0, 0, 0)).map(|t| t.and_utc().timestamp())
}

fn normalize_tz_colon(raw: &str) -> String {
    if raw.len() >= 5 {
        let tail = &raw[raw.len()-5..];
        if (tail.starts_with('+') || tail.starts_with('-')) && tail[1..].bytes().all(|b| b.is_ascii_digit()) {
            return format!("{}{}:{}", &raw[..raw.len()-5], &tail[..3], &tail[3..]);
        }
    }
    raw.to_string()
}


fn bucket_items(source: Source, raw: Vec<RawItem>, soon_days: u32) -> Vec<RemoteItem> {
    let mut due = Vec::new();
    let mut soon = Vec::new();
    let mut later = Vec::new();
    let mut none = Vec::new();
    for it in raw {
        let parsed = it.due.as_deref().and_then(parse_date).map(format_date);
        let bucket = assign_bucket(parsed.as_deref(), soon_days);
        let priority = it.priority.unwrap_or(match bucket {
            Bucket::Due => 0, Bucket::Soon => 1, Bucket::Later | Bucket::Unscheduled => 2,
        }).min(2);
        let item = RemoteItem {
            key: it.key,
            summary: it.summary,
            due: parsed,
            status: it.status,
            url: it.url,
            priority,
            ext_id: it.ext_id,
            list_id: it.list_id,
            source,
            bucket,
            done: false,
            updated_at: it.updated.as_deref().and_then(parse_updated_stamp),
        };
        match bucket {
            Bucket::Due => due.push(item),
            Bucket::Soon => soon.push(item),
            Bucket::Later => later.push(item),
            Bucket::Unscheduled => none.push(item),
        }
    }
    let mut out = due;
    out.extend(soon);
    out.extend(later);
    out.extend(none);
    out
}

#[derive(Debug)]
struct RawItem {
    key: String,
    summary: String,
    due: Option<String>,
    status: Option<String>,
    url: Option<String>,
    priority: Option<u8>,
    ext_id: Option<String>,
    list_id: Option<String>,
    updated: Option<String>,
}

fn jira_auth(token: &str) -> Result<(String, String), String> {
    let token = token.trim();
    if token.is_empty() { return Err("请先填写 API Token".into()); }
    if token.contains('@') && token.contains(':') {
        use base64::Engine;
        let enc = base64::engine::general_purpose::STANDARD.encode(token.as_bytes());
        Ok(("Authorization".into(), format!("Basic {enc}")))
    } else {
        let t = token.strip_prefix("Bearer ").unwrap_or(token).trim();
        Ok(("Authorization".into(), format!("Bearer {t}")))
    }
}

/// 校验 Jira 平台地址：接受 http:// 与 https://（含端口、路径），要求是带主机的完整地址。
/// 自建站点常用 HTTP（例如 http://主机:端口），不再仅因协议是 http 而拒绝。
pub fn validate_jira_base(base: &str) -> Result<String, String> {
    let base = base.trim().trim_end_matches('/').to_string();
    if base.is_empty() { return Err("请先填写平台 URL".into()); }
    let parsed = url::Url::parse(&base).map_err(|_| {
        "请填写完整的平台地址，例如 http://主机:端口 或 https://主机".to_string()
    })?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err("请填写完整的平台地址，例如 http://主机:端口 或 https://主机".into());
    }
    if parsed.host_str().unwrap_or("").is_empty() {
        return Err("请填写完整的平台地址，例如 http://主机:端口 或 https://主机".into());
    }
    Ok(base)
}

/// 从 Jira 搜索响应取出 issues。错误体、非搜索结果都返回 Err，不能当成「成功但为空」。
fn parse_jira_search(doc: &Value, base: &str) -> Result<(Vec<RawItem>, usize), String> {
    if let Some(msg) = jira_error_message(doc) {
        return Err(msg);
    }
    let Some(issues) = doc.get("issues").and_then(|v| v.as_array()) else {
        return Err("Jira 返回的不是搜索结果，请检查平台地址、权限或接口是否可用".into());
    };
    let total = doc.get("total").and_then(|v| v.as_u64()).unwrap_or(issues.len() as u64) as usize;
    let mut list = Vec::new();
    let base = base.trim().trim_end_matches('/');
    for issue in issues {
        let key = issue.get("key").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if key.is_empty() { continue; }
        let fields = issue.get("fields").cloned().unwrap_or(json!({}));
        let summary = fields.get("summary").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let due = fields.get("duedate").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(|s| s.to_string());
        let status = fields.get("status").and_then(|s| s.get("name")).and_then(|v| v.as_str()).map(|s| s.to_string());
        let pname = fields.get("priority").and_then(|s| s.get("name")).and_then(|v| v.as_str());
        let priority = map_jira_priority(pname);
        let url = format!("{base}/browse/{key}");
        let updated = fields.get("updated").and_then(|v| v.as_str()).map(|s| s.to_string());
        list.push(RawItem { key, summary, due, status, url: Some(url), priority, ext_id: None, list_id: None, updated });
    }
    Ok((list, total))
}

fn jira_error_message(doc: &Value) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(arr) = doc.get("errorMessages").and_then(|v| v.as_array()) {
        for e in arr {
            if let Some(s) = e.as_str().filter(|s| !s.is_empty()) { parts.push(s.to_string()); }
        }
    }
    if let Some(obj) = doc.get("errors").and_then(|v| v.as_object()) {
        for (k, v) in obj {
            if let Some(s) = v.as_str().filter(|s| !s.is_empty()) { parts.push(format!("{k}: {s}")); }
        }
    }
    if parts.is_empty() && doc.get("issues").is_none() {
        if let Some(s) = doc.get("message").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
            parts.push(s.to_string());
        }
    }
    if parts.is_empty() { None } else { Some(format!("Jira：{}", parts.join("；"))) }
}

fn search_jira_all(base: &str, auth: &str, jql: &str) -> Result<Vec<RawItem>, String> {
    let mut start = 0u32;
    let page = 50u32;
    let mut all = Vec::new();
    let mut total = 0usize;
    loop {
        let url = format!(
            "{base}/rest/api/2/search?jql={}&fields=summary,duedate,status,priority,updated&startAt={start}&maxResults={page}",
            urlencoding(jql)
        );
        let doc = get_json(&url, &[("Authorization", auth), ("Accept", "application/json")])?;
        let (items, t) = parse_jira_search(&doc, base)?;
        total = t;
        let n = items.len() as u32;
        all.extend(items);
        if n == 0 || all.len() >= total || start >= 1000 { break; }
        start = start.saturating_add(page);
        if start > 1000 { break; }
    }
    Ok(all)
}

fn sync_jira(base: &str, token: &str, soon_days: u32) -> Result<Vec<RemoteItem>, String> {
    let base = validate_jira_base(base)?;
    let auth = jira_auth(token)?;
    // 一次拉回当前用户全部未解决指派，含晚于「即将到期」窗口的事项；分组在本地完成。
    let jql = "assignee = currentUser() AND resolution = Unresolved ORDER BY duedate ASC";
    let raw = search_jira_all(&base, &auth.1, jql)?;
    Ok(bucket_items(Source::Jira, raw, soon_days))
}


pub fn map_jira_priority(name: Option<&str>) -> Option<u8> {
    let n = name?.trim();
    if n.is_empty() { return None; }
    if contains_any(n, &["Highest", "Blocker", "Critical", "紧急", "最高", "P0"]) { return Some(0); }
    if contains_any(n, &["High", "高", "重要", "P1"]) { return Some(1); }
    if contains_any(n, &["Medium", "Low", "Lowest", "中", "低", "P2", "Minor", "Trivial"]) { return Some(2); }
    None
}

fn contains_any(hay: &str, needles: &[&str]) -> bool {
    let lower = hay.to_ascii_lowercase();
    needles.iter().any(|n| hay.contains(n) || lower.contains(&n.to_ascii_lowercase()))
}

fn urlencoding(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

fn sync_linear(token: &str, soon_days: u32) -> Result<Vec<RemoteItem>, String> {
    let token = token.trim().strip_prefix("Bearer ").unwrap_or(token.trim());
    if token.is_empty() { return Err("请先填写 Linear API Key".into()); }
    const QUERY: &str = r#"query {
      issues(first: 50, filter: { assignee: { isMe: { eq: true } } }) {
        nodes { identifier title dueDate url priority state { name } updatedAt }
      }
    }"#;
    let body = json!({ "query": QUERY }).to_string();
    let doc = post_json(
        "https://api.linear.app/graphql",
        &[("Authorization", token), ("Accept", "application/json")],
        &body,
    )?;
    if let Some(errs) = doc.get("errors").and_then(|v| v.as_array()).filter(|a| !a.is_empty()) {
        let msg = errs[0].get("message").and_then(|v| v.as_str()).unwrap_or("未知错误");
        return Err(format!("Linear GraphQL：{}", short(msg)));
    }
    let nodes = doc.pointer("/data/issues/nodes").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let mut raw = Vec::new();
    for node in nodes {
        let key = node.get("identifier").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if key.is_empty() { continue; }
        let summary = node.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let due = node.get("dueDate").and_then(|v| v.as_str()).map(|s| s.to_string());
        let url = node.get("url").and_then(|v| v.as_str()).map(|s| s.to_string());
        let status = node.pointer("/state/name").and_then(|v| v.as_str()).map(|s| s.to_string());
        let priority = match node.get("priority") {
            Some(v) if v.is_number() => map_linear_priority(v.as_i64().unwrap_or(0) as i32),
            Some(v) if v.is_string() => map_loose_priority(v.as_str()),
            _ => None,
        };
        let updated = node.get("updatedAt").and_then(|v| v.as_str()).map(|s| s.to_string());
        raw.push(RawItem { key: key.clone(), summary, due, status, url, priority, ext_id: Some(key), list_id: None, updated });
    }
    Ok(bucket_items(Source::Linear, raw, soon_days))
}

fn map_linear_priority(v: i32) -> Option<u8> {
    match v { 1 => Some(0), 2 => Some(1), 3 | 4 => Some(2), _ => None }
}

pub fn try_parse_github_repo(raw: &str) -> Option<(String, String)> {
    let raw = raw.trim();
    if raw.is_empty() { return None; }
    let trimmed = raw
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("github.com/")
        .trim_start_matches("www.github.com/")
        .trim_end_matches('/')
        .trim_end_matches(".git");
    let mut parts = trimmed.split('/');
    let owner = parts.next()?.trim();
    let repo = parts.next()?.trim();
    if owner.is_empty() || repo.is_empty() || parts.next().is_some() && trimmed.matches('/').count() > 1 && !raw.contains("github.com") {
        // owner/repo only, or github.com/owner/repo[/...]
    }
    let repo = repo.split('/').next().unwrap_or(repo);
    if owner.is_empty() || repo.is_empty() { return None; }
    if !owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') { return None; }
    Some((owner.to_string(), repo.to_string()))
}

fn sync_github(address: &str, token: &str, soon_days: u32) -> Result<Vec<RemoteItem>, String> {
    let token = token.trim();
    if token.is_empty() { return Err("请先填写 GitHub PAT（Token）".into()); }
    let (owner, repo) = try_parse_github_repo(address).ok_or("请填写 owner/repo，或完整 GitHub 仓库地址")?;
    let url = format!(
        "https://api.github.com/repos/{}/{}/issues?state=open&per_page=50&sort=updated",
        urlencoding(&owner), urlencoding(&repo)
    );
    let t = token.strip_prefix("Bearer ").unwrap_or(token);
    let doc = get_json(&url, &[
        ("Authorization", &format!("Bearer {t}")),
        ("Accept", "application/vnd.github+json"),
        ("X-GitHub-Api-Version", "2022-11-28"),
    ])?;
    let arr = doc.as_array().ok_or("GitHub 返回不是列表")?;
    let mut raw = Vec::new();
    for issue in arr {
        // 跳过 pull request（GitHub issues API 会混入 PR）
        if issue.get("pull_request").is_some() { continue; }
        let number = issue.get("number").and_then(|v| v.as_i64()).unwrap_or(0);
        if number <= 0 { continue; }
        let key = format!("{owner}/{repo}#{number}");
        let summary = issue.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let html = issue.get("html_url").and_then(|v| v.as_str()).map(|s| s.to_string());
        let status = issue.get("state").and_then(|v| v.as_str()).map(|s| s.to_string());
        let due = issue.get("milestone")
            .and_then(|m| m.get("due_on"))
            .and_then(|v| v.as_str())
            .and_then(parse_date)
            .map(format_date);
        let priority = map_github_priority(issue);
        let updated = issue.get("updated_at").and_then(|v| v.as_str()).map(|s| s.to_string());
        raw.push(RawItem {
            key, summary, due, status, url: html,
            priority, ext_id: Some(number.to_string()), list_id: Some(format!("{owner}/{repo}")),
            updated,
        });
    }
    Ok(bucket_items(Source::GitHub, raw, soon_days))
}

fn map_github_priority(issue: &Value) -> Option<u8> {
    let labels = issue.get("labels")?.as_array()?;
    for lab in labels {
        let name = if lab.is_string() { lab.as_str() } else { lab.get("name").and_then(|v| v.as_str()) };
        if let Some(p) = map_loose_priority(name) { return Some(p); }
    }
    None
}

pub fn try_parse_notion_id(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() { return None; }
    // 先去掉查询串，避免 ?v=1 这类参数把十六进制拼进 ID。
    let base = raw.split('?').next().unwrap_or(raw).split('#').next().unwrap_or(raw);
    // 32 位无连字符，或 UUID，或 URL 路径中的 32 hex。
    let compact: String = base.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if compact.len() >= 32 {
        let id = compact[compact.len()-32..].to_ascii_lowercase();
        return Some(format_uuid(&id));
    }
    None
}

fn format_uuid(compact32: &str) -> String {
    if compact32.len() != 32 { return compact32.to_string(); }
    format!("{}-{}-{}-{}-{}", &compact32[0..8], &compact32[8..12], &compact32[12..16], &compact32[16..20], &compact32[20..32])
}

fn sync_notion(address: &str, token: &str, soon_days: u32) -> Result<Vec<RemoteItem>, String> {
    let token = token.trim();
    if token.is_empty() { return Err("请先填写 Notion Integration Token".into()); }
    let db = try_parse_notion_id(address).ok_or("请填写 Notion 数据库 ID，或数据库页面链接")?;
    let t = token.strip_prefix("Bearer ").unwrap_or(token);
    let url = format!("https://api.notion.com/v1/databases/{db}/query");
    let doc = post_json(
        &url,
        &[("Authorization", &format!("Bearer {t}")), ("Notion-Version", "2022-06-28")],
        "{\"page_size\":50}",
    )?;
    let results = doc.get("results").and_then(|v| v.as_array()).ok_or("Notion 返回无 results")?;
    let mut raw = Vec::new();
    for page in results {
        let id = page.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if id.is_empty() { continue; }
        let page_url = page.get("url").and_then(|v| v.as_str()).map(|s| s.to_string());
        let props = page.get("properties").cloned().unwrap_or(json!({}));
        let mut title = String::new();
        let mut due = None;
        let mut status = None;
        if let Some(obj) = props.as_object() {
            for (name, prop) in obj {
                if title.is_empty() {
                    if let Some(t) = extract_notion_title(prop) { if !t.is_empty() { title = t; } }
                }
                let lname = name.to_ascii_lowercase();
                if due.is_none() && (lname.contains("date") || lname.contains("到期") || lname.contains("截止") || lname.contains("due")) {
                    due = extract_notion_date(prop);
                }
                if status.is_none() && (lname.contains("status") || lname.contains("状态") || lname == "state") {
                    status = prop.get("status").and_then(|s| s.get("name")).and_then(|v| v.as_str())
                        .or_else(|| prop.get("select").and_then(|s| s.get("name")).and_then(|v| v.as_str()))
                        .map(|s| s.to_string());
                }
            }
        }
        // 跳过已完成
        if status.as_deref().is_some_and(|s| {
            let l = s.to_ascii_lowercase();
            l == "done" || l == "完成" || l == "closed" || l.contains("complete")
        }) { continue; }
        if title.is_empty() { title = "(无标题)".into(); }
        let key = format!("NT-{}", if id.len() <= 8 { id.clone() } else { id[id.len()-8..].to_string() });
        let priority = map_notion_priority(&props);
        let updated = page.get("last_edited_time").and_then(|v| v.as_str()).map(|s| s.to_string());
        raw.push(RawItem { key, summary: title, due, status, url: page_url, priority, ext_id: Some(id), list_id: None, updated });
    }
    Ok(bucket_items(Source::Notion, raw, soon_days))
}

fn extract_notion_title(prop: &Value) -> Option<String> {
    let arr = prop.get("title")?.as_array()?;
    let mut s = String::new();
    for part in arr {
        if let Some(t) = part.get("plain_text").and_then(|v| v.as_str()) { s.push_str(t); }
    }
    Some(s)
}

fn extract_notion_date(prop: &Value) -> Option<String> {
    let start = prop.get("date")?.get("start")?.as_str()?;
    Some(parse_date(start).map(format_date).unwrap_or_else(|| start.chars().take(10).collect()))
}

fn map_notion_priority(props: &Value) -> Option<u8> {
    let obj = props.as_object()?;
    for (name, prop) in obj {
        let l = name.to_ascii_lowercase();
        if !(name.contains("优先") || l.contains("priority") || l == "p") { continue; }
        let text = prop.get("select").and_then(|s| s.get("name")).and_then(|v| v.as_str())
            .or_else(|| prop.get("status").and_then(|s| s.get("name")).and_then(|v| v.as_str()));
        if let Some(p) = map_loose_priority(text) { return Some(p); }
    }
    None
}

pub fn try_parse_feishu_table(raw: &str) -> Option<(String, String)> {
    let raw = raw.trim().replace('\\', "/");
    // app_token/table_id 或 URL
    if let Some((a, b)) = raw.split_once('/') {
        let a = a.trim(); let b = b.trim().trim_matches('/');
        // 如果是 URL，找 /app_token/ 和 table=
        if a.starts_with("http") || raw.contains("feishu") || raw.contains("larksuite") {
            // 尝试从路径提取
        }
        if !a.is_empty() && !b.is_empty() && !a.starts_with("http") {
            let table = b.split('/').next().unwrap_or(b).split('?').next().unwrap_or(b);
            if !table.is_empty() { return Some((a.to_string(), table.to_string())); }
        }
    }
    // URL: .../base/{app_token}?table={table}
    if let Ok(url) = url::Url::parse(&raw) {
        let mut app = None;
        for seg in url.path_segments()?.rev() {
            if !seg.is_empty() && seg != "base" && seg != "app" { app = Some(seg.to_string()); break; }
        }
        let table = url.query_pairs().find(|(k, _)| k == "table" || k == "tableId" || k == "table_id").map(|(_, v)| v.to_string());
        if let (Some(a), Some(t)) = (app, table) { return Some((a, t)); }
    }
    None
}

fn sync_feishu(address: &str, token_or_creds: &str, soon_days: u32) -> Result<Vec<RemoteItem>, String> {
    let creds = token_or_creds.trim();
    if creds.is_empty() { return Err("请填写 飞书 Token：tenant_access_token，或 app_id:app_secret".into()); }
    let (app_token, table_id) = try_parse_feishu_table(address).ok_or("请填写 app_token/table_id（多维表格）")?;
    let access = if creds.contains(':') && !creds.to_ascii_lowercase().starts_with("t-") {
        let (id, secret) = creds.split_once(':').unwrap();
        let id = id.trim(); let secret = secret.trim();
        if id.is_empty() || secret.is_empty() { return Err("app_id:app_secret 格式不正确".into()); }
        fetch_feishu_tenant(id, secret)?
    } else {
        creds.strip_prefix("Bearer ").unwrap_or(creds).trim().to_string()
    };
    let url = format!(
        "https://open.feishu.cn/open-apis/bitable/v1/apps/{}/tables/{}/records?page_size=50",
        urlencoding(&app_token), urlencoding(&table_id)
    );
    let doc = get_json(&url, &[("Authorization", &format!("Bearer {access}"))])?;
    let code = doc.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code != 0 {
        let msg = doc.get("msg").and_then(|v| v.as_str()).unwrap_or("未知错误");
        return Err(format!("飞书 API：{}", short(msg)));
    }
    let arr = doc.pointer("/data/items").or_else(|| doc.pointer("/data/records")).and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let mut raw = Vec::new();
    for rec in arr {
        let id = rec.get("record_id").or_else(|| rec.get("id")).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let fields = rec.get("fields").cloned().unwrap_or(json!({}));
        let mut title = String::new();
        let mut due = None;
        let mut status = None;
        let mut prio = None;
        if let Some(obj) = fields.as_object() {
            for (name, val) in obj {
                let l = name.to_ascii_lowercase();
                let text = extract_feishu_text(val);
                if title.is_empty() && (l.contains("title") || l.contains("标题") || l.contains("任务") || l.contains("name") || l.contains("名称")) {
                    if let Some(t) = text.clone().filter(|s| !s.is_empty()) { title = t; }
                }
                if due.is_none() && (l.contains("date") || l.contains("到期") || l.contains("截止") || l.contains("due")) {
                    due = extract_feishu_date(val);
                }
                if status.is_none() && (l.contains("status") || l.contains("状态")) {
                    status = text.clone();
                }
                if prio.is_none() && (l.contains("优先") || l.contains("priority") || l == "p") {
                    prio = map_loose_priority(text.as_deref());
                }
            }
            if title.is_empty() {
                // 取第一个文本字段
                for val in obj.values() {
                    if let Some(t) = extract_feishu_text(val).filter(|s| !s.is_empty()) { title = t; break; }
                }
            }
        }
        if title.is_empty() { title = "(无标题)".into(); }
        if status.as_deref().is_some_and(|s| {
            let l = s.to_ascii_lowercase();
            l == "done" || l.contains("完成") || l.contains("关闭")
        }) { continue; }
        let key = if id.is_empty() { title.chars().take(16).collect() } else { format!("FS-{}", if id.len()<=10 { id.clone() } else { id[id.len()-10..].to_string() }) };
        let updated = rec.get("last_modified_time").and_then(|v| v.as_str().map(|s| s.to_string()).or_else(|| v.as_i64().map(|n| n.to_string())));
        raw.push(RawItem { key, summary: title, due, status, url: None, priority: prio, ext_id: Some(id), list_id: Some(table_id.clone()), updated });
    }
    Ok(bucket_items(Source::Feishu, raw, soon_days))
}

fn fetch_feishu_tenant(app_id: &str, app_secret: &str) -> Result<String, String> {
    let body = json!({ "app_id": app_id, "app_secret": app_secret }).to_string();
    let doc = post_json(
        "https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal",
        &[("Accept", "application/json")],
        &body,
    )?;
    let code = doc.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code != 0 {
        let msg = doc.get("msg").and_then(|v| v.as_str()).unwrap_or("未知错误");
        return Err(format!("获取 tenant_access_token 失败：{}", short(msg)));
    }
    doc.get("tenant_access_token").and_then(|v| v.as_str()).filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .ok_or_else(|| "获取 tenant_access_token 失败：响应无令牌".to_string())
}

fn extract_feishu_text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Array(arr) => {
            let mut s = String::new();
            for part in arr {
                if let Some(t) = part.get("text").and_then(|v| v.as_str()) { s.push_str(t); }
                else if let Some(t) = part.as_str() { s.push_str(t); }
            }
            if s.is_empty() { None } else { Some(s) }
        }
        Value::Object(o) => o.get("text").and_then(|v| v.as_str()).map(|s| s.to_string())
            .or_else(|| o.get("name").and_then(|v| v.as_str()).map(|s| s.to_string())),
        _ => None,
    }
}

fn extract_feishu_date(value: &Value) -> Option<String> {
    if let Some(n) = value.as_i64() {
        let secs = if n > 10_000_000_000 { n / 1000 } else { n };
        return chrono::DateTime::from_timestamp(secs, 0).map(|d| format_date(d.with_timezone(&chrono::Local).date_naive()));
    }
    if let Some(s) = value.as_str() { return parse_date(s).map(format_date); }
    if let Some(arr) = value.as_array().filter(|a| !a.is_empty()) { return extract_feishu_date(&arr[0]); }
    if let Some(o) = value.as_object() {
        if let Some(v) = o.get("value").or_else(|| o.get("start")).or_else(|| o.get("date")) {
            return extract_feishu_date(v);
        }
    }
    None
}

fn sync_windows(client_id: &str, soon_days: u32, interactive: bool) -> Result<Vec<RemoteItem>, String> {
    let token = super::windows_auth::acquire_token(client_id, interactive)?;
    let lists = get_json(
        "https://graph.microsoft.com/v1.0/me/todo/lists",
        &[("Authorization", &format!("Bearer {token}")), ("Accept", "application/json")],
    )?;
    let arr = lists.get("value").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let mut raw = Vec::new();
    for list in arr {
        if raw.len() >= 100 { break; }
        let list_id = list.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if list_id.is_empty() { continue; }
        let mut next = Some(format!(
            "https://graph.microsoft.com/v1.0/me/todo/lists/{}/tasks?$filter=status ne 'completed'&$top=50",
            urlencoding(&list_id)
        ));
        while let Some(url) = next.take() {
            if raw.len() >= 100 { break; }
            let doc = get_json(&url, &[("Authorization", &format!("Bearer {token}")), ("Accept", "application/json")])?;
            if let Some(tasks) = doc.get("value").and_then(|v| v.as_array()) {
                for task in tasks {
                    if raw.len() >= 100 { break; }
                    let id = task.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    if id.is_empty() { continue; }
                    let title = task.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let status = task.get("status").and_then(|v| v.as_str()).map(|s| s.to_string());
                    let due = task.pointer("/dueDateTime/dateTime").and_then(|v| v.as_str()).and_then(parse_date).map(format_date);
                    let prio = match task.get("importance").and_then(|v| v.as_str()) {
                        Some(s) if s.eq_ignore_ascii_case("high") => Some(0),
                        Some(s) if s.eq_ignore_ascii_case("normal") => Some(1),
                        Some(s) if s.eq_ignore_ascii_case("low") => Some(2),
                        _ => None,
                    };
                    let key = format!("TODO-{}", if id.len() <= 16 { id.clone() } else { id[id.len()-16..].to_string() });
                    let updated = task.get("lastModifiedDateTime").and_then(|v| v.as_str()).map(|s| s.to_string());
                    raw.push(RawItem {
                        key,
                        summary: if title.is_empty() { "(无标题)".into() } else { title },
                        due, status, url: None, priority: prio,
                        ext_id: Some(id), list_id: Some(list_id.clone()),
                        updated,
                    });
                }
            }
            next = doc.get("@odata.nextLink").and_then(|v| v.as_str()).map(|s| s.to_string());
        }
    }
    Ok(bucket_items(Source::WindowsTodo, raw, soon_days))
}

fn sync_json(url: &str, token: &str) -> Result<Vec<RemoteItem>, String> {
    let url = url.trim();
    if url.is_empty() { return Err("请先填写 JSON 地址".into()); }
    if !(url.starts_with("https://") || (cfg!(test) && url.starts_with("http://"))) {
        return Err("JSON 地址需使用 https://".into());
    }
    let doc = if token.trim().is_empty() {
        get_json(url, &[("Accept", "application/json")])?
    } else {
        let t = token.trim().strip_prefix("Bearer ").unwrap_or(token.trim());
        get_json(url, &[("Accept", "application/json"), ("Authorization", &format!("Bearer {t}"))])?
    };
    if !doc.is_object() { return Err("JSON 根节点必须是对象".into()); }
    for name in ["due", "soon", "unscheduled"] {
        match doc.get(name) {
            Some(v) if v.is_array() || v.is_null() => {}
            _ => return Err("JSON 需含 due / soon / unscheduled 数组（可为空）".into()),
        }
    }
    let map = |it: &Value, default_p: u8, bucket: Bucket| -> Option<RemoteItem> {
        let key = it.get("key").and_then(|v| v.as_str()).filter(|s| !s.is_empty())?.to_string();
        let summary = it.get("summary").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let due = it.get("due").and_then(|v| v.as_str()).and_then(parse_date).map(format_date)
            .or_else(|| it.get("due").and_then(|v| v.as_str()).map(|s| s.chars().take(16).collect()));
        let status = it.get("status").and_then(|v| v.as_str()).map(|s| s.to_string());
        let url = it.get("url").and_then(|v| v.as_str()).map(|s| s.to_string());
        let priority = it.get("priority").and_then(|v| v.as_u64()).map(|n| n.min(2) as u8).unwrap_or(default_p);
        let ext = it.get("extId").or_else(|| it.get("ext_id")).and_then(|v| v.as_str()).map(|s| s.to_string());
        let list = it.get("listId").or_else(|| it.get("list_id")).and_then(|v| v.as_str()).map(|s| s.to_string());
        let updated = it.get("updated").or_else(|| it.get("updated_at")).or_else(|| it.get("updatedAt"))
            .and_then(|v| v.as_str().map(|s| s.to_string()).or_else(|| v.as_i64().or_else(|| v.as_u64().map(|n| n as i64)).map(|n| n.to_string())));
        Some(RemoteItem { key, summary, due, status, url, priority, ext_id: ext, list_id: list, source: Source::CustomJson, bucket, done: false, updated_at: updated.as_deref().and_then(parse_updated_stamp) })
    };
    let mut items = Vec::new();
    if let Some(arr) = doc.get("due").and_then(|v| v.as_array()) {
        for it in arr { if let Some(x) = map(it, 0, Bucket::Due) { items.push(x); } }
    }
    if let Some(arr) = doc.get("soon").and_then(|v| v.as_array()) {
        for it in arr { if let Some(x) = map(it, 1, Bucket::Soon) { items.push(x); } }
    }
    if let Some(arr) = doc.get("unscheduled").and_then(|v| v.as_array()) {
        for it in arr.iter().take(20) { if let Some(x) = map(it, 2, Bucket::Unscheduled) { items.push(x); } }
    }
    Ok(items)
}

pub fn map_loose_priority(text: Option<&str>) -> Option<u8> {
    let n = text?.trim();
    if n.is_empty() { return None; }
    if contains_any(n, &["P0", "Highest", "Urgent", "Critical", "紧急", "最高", "blocker"]) { return Some(0); }
    if contains_any(n, &["P1", "High", "高", "重要"]) { return Some(1); }
    if contains_any(n, &["P2", "Medium", "Low", "Lowest", "中", "低", "Minor", "Trivial", "普通"]) { return Some(2); }
    None
}

/// 供写回使用：把 ureq 的请求帮手公开给 writeback。
pub fn http_get(url: &str, headers: &[(&str, &str)]) -> Result<Value, String> { get_json(url, headers) }
pub fn http_post(url: &str, headers: &[(&str, &str)], body: &str) -> Result<Value, String> { post_json(url, headers, body) }
pub fn http_patch(url: &str, headers: &[(&str, &str)], body: &str) -> Result<Value, String> { patch_json(url, headers, body) }
pub fn shorten(s: &str) -> String { short(s) }
pub fn encode(s: &str) -> String { urlencoding(s) }
pub fn jira_authorization(token: &str) -> Result<String, String> { Ok(jira_auth(token)?.1) }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_repo_parses_url_and_slug() {
        assert_eq!(try_parse_github_repo("octocat/hello-world").unwrap(), ("octocat".into(), "hello-world".into()));
        assert_eq!(try_parse_github_repo("https://github.com/octocat/hello-world.git").unwrap(), ("octocat".into(), "hello-world".into()));
        assert!(try_parse_github_repo("just-one").is_none());
        assert!(try_parse_github_repo("").is_none());
    }

    #[test]
    fn notion_id_from_url_and_uuid() {
        let id = try_parse_notion_id("https://www.notion.so/myworkspace/0123456789abcdef0123456789abcdef?v=1").unwrap();
        assert_eq!(id, "01234567-89ab-cdef-0123-456789abcdef");
        assert_eq!(try_parse_notion_id("01234567-89ab-cdef-0123-456789abcdef").unwrap(), "01234567-89ab-cdef-0123-456789abcdef");
        assert!(try_parse_notion_id("short").is_none());
    }

    #[test]
    fn feishu_table_parses_slash_and_url() {
        assert_eq!(try_parse_feishu_table("bascnXXX/tblYYY").unwrap(), ("bascnXXX".into(), "tblYYY".into()));
        let u = try_parse_feishu_table("https://example.feishu.cn/base/bascnXXX?table=tblYYY").unwrap();
        assert_eq!(u, ("bascnXXX".into(), "tblYYY".into()));
        assert!(try_parse_feishu_table("only-one").is_none());
    }

    #[test]
    fn buckets_follow_today_and_soon_window() {
        let t = today();
        assert_eq!(assign_bucket(Some(&format_date(t)), 3), Bucket::Due);
        assert_eq!(assign_bucket(Some(&format_date(t + chrono::Duration::days(2))), 3), Bucket::Soon);
        assert_eq!(assign_bucket(Some(&format_date(t + chrono::Duration::days(10))), 3), Bucket::Later);
        assert_eq!(assign_bucket(None, 3), Bucket::Unscheduled);
    }

    fn raw(key: &str, summary: &str, due: Option<String>) -> RawItem {
        RawItem { key: key.into(), summary: summary.into(), due, status: None, url: None, priority: None, ext_id: None, list_id: None, updated: None }
    }

    #[test]
    fn parse_updated_stamp_accepts_common_formats() {
        let rfc = parse_updated_stamp("2026-11-13T10:00:00Z").unwrap();
        let jira = parse_updated_stamp("2026-11-13T18:00:00.000+0800").unwrap();
        assert_eq!(rfc, jira);
        assert_eq!(parse_updated_stamp("1763020800000").unwrap(), 1763020800);
        assert!(parse_updated_stamp("2026-11-13").is_some());
        assert!(parse_updated_stamp("").is_none());
    }

    #[test]
    fn later_due_issues_are_kept_and_listed() {
        let t = today();
        let raw = vec![
            raw("A-1", "今天到期", Some(format_date(t))),
            raw("A-2", "即将到期", Some(format_date(t + chrono::Duration::days(1)))),
            raw("A-3", "更晚的任务", Some(format_date(t + chrono::Duration::days(10)))),
            raw("A-4", "没有日期", None),
        ];
        let items = bucket_items(Source::Jira, raw, 1);
        assert_eq!(items.len(), 4, "晚于即将窗口的事项不能被丢掉");
        assert_eq!(items.iter().find(|i| i.key=="A-3").unwrap().bucket, Bucket::Later);
        assert_eq!(items.iter().find(|i| i.key=="A-1").unwrap().bucket, Bucket::Due);
        assert_eq!(items.iter().find(|i| i.key=="A-2").unwrap().bucket, Bucket::Soon);
        assert_eq!(items.iter().find(|i| i.key=="A-4").unwrap().bucket, Bucket::Unscheduled);
        let (d, s, n, l) = count_buckets(&items);
        assert_eq!((d, s, n, l), (1, 1, 1, 1));
        let outcome = {
            let (due, soon, none, later) = count_buckets(&items);
            format!("已同步 · 到期 {due} · 即将 {soon} · 未排期 {none} · 其他 {later}")
        };
        assert_eq!(outcome, "已同步 · 到期 1 · 即将 1 · 未排期 1 · 其他 1");
        assert!(items.iter().any(|i| i.summary=="更晚的任务"));
    }

    #[test]
    fn jira_error_bodies_are_not_successful_empty_searches() {
        let err = serde_json::json!({"errorMessages":["The requested API has been removed."],"errors":{}});
        let r = parse_jira_search(&err, "http://jira.example.invalid");
        assert!(r.is_err(), "错误体不能当成成功的空列表");
        let msg = r.unwrap_err();
        assert!(msg.contains("The requested API has been removed."), "{msg}");
        assert!(!msg.contains("已同步"));

        let field_err = serde_json::json!({"errorMessages":[],"errors":{"jql":"Invalid JQL"}});
        let msg = parse_jira_search(&field_err, "http://x").unwrap_err();
        assert!(msg.contains("Invalid JQL"), "{msg}");

        let unauthorized = serde_json::json!({"message":"unauthorized"});
        assert!(parse_jira_search(&unauthorized, "http://x").is_err());

        let not_search = serde_json::json!({"foo":1});
        assert!(parse_jira_search(&not_search, "http://x").is_err());

        let empty_ok = serde_json::json!({"startAt":0,"maxResults":50,"total":0,"issues":[]});
        let (items, total) = parse_jira_search(&empty_ok, "http://jira.example.invalid").unwrap();
        assert!(items.is_empty());
        assert_eq!(total, 0);

        let ok = serde_json::json!({
            "startAt":0,"maxResults":50,"total":1,
            "issues":[{"key":"EX-9","fields":{"summary":"以后的任务","duedate":"2026-12-31","status":{"name":"Open"},"priority":{"name":"Low"}}}]
        });
        let (items, total) = parse_jira_search(&ok, "http://jira.example.invalid").unwrap();
        assert_eq!(total, 1);
        assert_eq!(items[0].key, "EX-9");
        assert_eq!(items[0].summary, "以后的任务");
        assert_eq!(items[0].due.as_deref(), Some("2026-12-31"));
        assert_eq!(items[0].url.as_deref(), Some("http://jira.example.invalid/browse/EX-9"));
    }

    #[test]
    fn priority_maps_for_common_labels() {
        assert_eq!(map_jira_priority(Some("Highest")), Some(0));
        assert_eq!(map_jira_priority(Some("High")), Some(1));
        assert_eq!(map_jira_priority(Some("Low")), Some(2));
        assert_eq!(map_loose_priority(Some("P0 紧急")), Some(0));
        assert_eq!(map_loose_priority(Some("P1")), Some(1));
        assert_eq!(map_linear_priority(1), Some(0));
        assert_eq!(map_linear_priority(4), Some(2));
    }

    #[test]
    fn custom_json_requires_three_arrays() {
        let err = sync_json("not-https", "").unwrap_err();
        assert!(err.contains("https"), "{err}");
    }

    #[test]
    fn jira_accepts_http_and_https_platform_urls() {
        assert_eq!(validate_jira_base("http://jira.example.invalid").unwrap(), "http://jira.example.invalid");
        assert_eq!(validate_jira_base("  http://jira.example.invalid/  ").unwrap(), "http://jira.example.invalid");
        assert_eq!(validate_jira_base("https://your-site.atlassian.net").unwrap(), "https://your-site.atlassian.net");
        assert_eq!(validate_jira_base("HTTPS://your-site.atlassian.net/").unwrap(), "HTTPS://your-site.atlassian.net");
        assert_eq!(validate_jira_base("http://127.0.0.1:8080").unwrap(), "http://127.0.0.1:8080");
        assert!(validate_jira_base("").unwrap_err().contains("请先填写"));
        for raw in ["http://", "https://", "ftp://jira.example.invalid", "jira.example.invalid", "not a url"] {
            let err = validate_jira_base(raw).unwrap_err();
            assert!(!err.contains("需使用 https://"), "http 地址不应再报仅允许 https：{raw} -> {err}");
            assert!(!err.is_empty());
        }
    }
}
