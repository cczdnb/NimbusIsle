//! 本机 Agent 会话：各家读取器产出同一种行，页面、常驻标记和通知都只认这一行。
//! 只读本地记录，不把整段对话读进来。Kiro、OpenCode、ZCode 的记录格式尚未核对，这里不解析。

use serde_json::Value;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const KEEP_MIN_DEFAULT: u32 = 10;
pub const KEEP_MIN_RANGE: std::ops::RangeInclusive<u32> = 1..=60;
const FRESH_SECS: i64 = 5 * 60;
const TAIL_BYTES: u64 = 256 * 1024;
const CANDIDATE_SECS: i64 = 36 * 3600;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Cursor = 0,
    Claude = 1,
    Codex = 2,
    WorkBuddy = 3,
    DeepSeekHarness = 4,
    Antigravity = 5,
}

impl Source {
    pub const ALL: [Self; 6] = [Self::Cursor, Self::Claude, Self::Codex, Self::WorkBuddy, Self::DeepSeekHarness, Self::Antigravity];
    pub fn bit(self) -> u8 { 1 << (self as u8) }
    pub const ALL_BITS: u8 = (1 << 6) - 1;
    pub fn name(self) -> &'static str {
        match self {
            Self::Cursor => "Cursor",
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::WorkBuddy => "WorkBuddy",
            Self::DeepSeekHarness => "DeepSeek",
            Self::Antigravity => "Antigravity",
        }
    }
    pub fn enabled(self, bits: u8) -> bool { bits & self.bit() != 0 }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase { NeedsYou, Errored, Running, JustFinished }

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Self::NeedsYou => "等你处理",
            Self::Errored => "报错停了",
            Self::Running => "正在跑",
            Self::JustFinished => "刚跑完",
        }
    }
    pub fn rank(self) -> u8 {
        match self { Self::NeedsYou => 0, Self::Errored => 1, Self::Running => 2, Self::JustFinished => 3 }
    }
    /// 正在跑只改列表，不弹通知。
    pub fn notifies(self) -> bool { !matches!(self, Self::Running) }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Launch {
    Cursor { folder: PathBuf },
    Url(String),
    Program { program: String, args: Vec<String>, cwd: Option<PathBuf> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct LaunchPlan { pub program: String, pub args: Vec<String>, pub cwd: Option<PathBuf> }

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: String,
    pub source: Source,
    pub title: String,
    pub phase: Phase,
    pub step: String,
    pub usage: Option<String>,
    pub project: String,
    pub launch: Launch,
    pub children: u32,
    pub updated: i64,
}

#[derive(Clone, Debug)]
pub struct Roots {
    pub home: PathBuf,
    pub cursor_db: PathBuf,
    pub cursor_projects: PathBuf,
}

impl Roots {
    pub fn system() -> Self {
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default();
        let roaming = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| home.join("AppData\\Roaming"));
        Self {
            cursor_db: roaming.join("Cursor\\User\\globalStorage\\state.vscdb"),
            cursor_projects: home.join(".cursor\\projects"),
            home,
        }
    }
}

/// 归类用的信号。刚跑完和报错都看保留时间。软件里已经读过不算看过，岛上点开才撤。
#[derive(Clone, Debug)]
pub struct Signals<'a> {
    pub agent: bool,
    pub archived: bool,
    pub subagent: bool,
    pub draft: bool,
    pub needs_approval: bool,
    pub pending_plan: bool,
    /// 软件里的未读标记。不再决定这条留不留，岛上点开才算看过。
    #[allow(dead_code)]
    pub unread: bool,
    pub unfinished: bool,
    pub turn: Turn,
    pub age_secs: i64,
    pub keep_secs: i64,
    pub step: &'a str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Turn { Tool, Success, Error, Idle }

pub fn phase_of(signals: &Signals<'_>) -> Option<Phase> {
    if signals.archived || signals.subagent || signals.draft || !signals.agent { return None; }
    if signals.needs_approval || signals.pending_plan { return Some(Phase::NeedsYou); }
    let fresh = signals.age_secs <= FRESH_SECS;
    // 头上标着还没跑完，或者末尾已经是新的一轮，都算正在跑。上一轮的结束记录不能把这一轮盖掉。
    if fresh && (signals.unfinished || signals.turn == Turn::Tool) { return Some(Phase::Running); }
    if signals.turn == Turn::Error && signals.age_secs <= signals.keep_secs { return Some(Phase::Errored); }
    // 软件里标成已读也先留着。岛上点开由 Watch::dismiss 撤掉，到了保留时间这里直接不返回。
    if signals.turn == Turn::Success && signals.age_secs <= signals.keep_secs {
        return Some(Phase::JustFinished);
    }
    None
}

pub fn sort_rows(rows: &mut [Row]) {
    rows.sort_by(|a, b| a.phase.rank().cmp(&b.phase.rank()).then(b.updated.cmp(&a.updated)));
}

pub fn scan(roots: &Roots, now: i64, keep_mins: u32, sources: u8) -> Vec<Row> {
    let keep = i64::from(keep_mins.clamp(1, 60)) * 60;
    let mut rows = Vec::new();
    if Source::Cursor.enabled(sources) { rows.extend(cursor_rows(roots, now, keep)); }
    if Source::Claude.enabled(sources) { rows.extend(claude_rows(roots, now, keep)); }
    if Source::Codex.enabled(sources) { rows.extend(codex_rows(roots, now, keep)); }
    if Source::WorkBuddy.enabled(sources) { rows.extend(jsonl_tree(roots, Source::WorkBuddy, now, keep, &roots.home.join(".workbuddy\\projects"))); }
    if Source::DeepSeekHarness.enabled(sources) { rows.extend(dsh_rows(roots, now, keep)); }
    if Source::Antigravity.enabled(sources) { rows.extend(antigravity_rows(roots, now, keep)); }
    sort_rows(&mut rows);
    rows
}

pub fn cursor_slug(path: &str) -> String {
    let mut out = String::new();
    for c in path.chars() {
        if !c.is_ascii() || c == ':' { continue; }
        let c = if c == '\\' || c == '/' { '-' } else { c };
        if c == '-' && (out.is_empty() || out.ends_with('-')) { continue; }
        if out.is_empty() { out.push(c.to_ascii_lowercase()); } else { out.push(c); }
    }
    out.trim_matches('-').to_string()
}

fn cursor_rows(roots: &Roots, now: i64, keep: i64) -> Vec<Row> {
    let headers = match cursor_headers(&roots.cursor_db) { Ok(headers) => headers, Err(_) => return Vec::new() };
    headers.into_iter().filter_map(|header| cursor_row(roots, &header, now, keep)).collect()
}

struct CursorHeader {
    id: String,
    archived: bool,
    subagent: bool,
    updated: i64,
    value: Value,
}

#[cfg(not(windows))]
fn cursor_headers(_db: &Path) -> Result<Vec<CursorHeader>, String> { Ok(Vec::new()) }

#[cfg(windows)]
fn cursor_headers(db: &Path) -> Result<Vec<CursorHeader>, String> {
    if !db.is_file() { return Ok(Vec::new()); }
    sqlite::query_headers(db)
}

#[cfg(windows)]
mod sqlite {
    use super::{CursorHeader, Value};
    use std::ffi::{CStr, CString};
    use std::os::raw::{c_char, c_int, c_void};
    use std::path::Path;
    const OPEN_READONLY: c_int = 1;
    const OPEN_READWRITE: c_int = 2;
    const OPEN_CREATE: c_int = 4;
    const ROW: c_int = 100;
    #[link(name = "winsqlite3")]
    extern "system" {
        fn sqlite3_open_v2(filename: *const c_char, db: *mut *mut c_void, flags: c_int, vfs: *const c_char) -> c_int;
        fn sqlite3_close(db: *mut c_void) -> c_int;
        fn sqlite3_prepare_v2(db: *mut c_void, sql: *const c_char, nbytes: c_int, stmt: *mut *mut c_void, tail: *mut *const c_char) -> c_int;
        fn sqlite3_step(stmt: *mut c_void) -> c_int;
        fn sqlite3_column_text(stmt: *mut c_void, col: c_int) -> *const u8;
        fn sqlite3_column_int64(stmt: *mut c_void, col: c_int) -> i64;
        fn sqlite3_finalize(stmt: *mut c_void) -> c_int;
        fn sqlite3_busy_timeout(db: *mut c_void, ms: c_int) -> c_int;
        fn sqlite3_exec(db: *mut c_void, sql: *const c_char, cb: *mut c_void, arg: *mut c_void, err: *mut *mut c_char) -> c_int;
    }
    struct Db(*mut c_void);
    impl Drop for Db { fn drop(&mut self) { if !self.0.is_null() { unsafe { sqlite3_close(self.0); } } } }
    fn open(path: &Path, flags: c_int) -> Result<Db, String> {
        let name = CString::new(path.to_string_lossy().as_bytes()).map_err(|_| "路径无效".to_string())?;
        let mut db = std::ptr::null_mut();
        let rc = unsafe { sqlite3_open_v2(name.as_ptr(), &mut db, flags, std::ptr::null()) };
        if rc != 0 || db.is_null() { return Err("无法打开会话数据库".into()); }
        unsafe { sqlite3_busy_timeout(db, 200); }
        Ok(Db(db))
    }
    fn text(stmt: *mut c_void, col: c_int) -> String {
        let ptr = unsafe { sqlite3_column_text(stmt, col) };
        if ptr.is_null() { return String::new(); }
        unsafe { CStr::from_ptr(ptr.cast()) }.to_string_lossy().into_owned()
    }
    pub fn query_headers(path: &Path) -> Result<Vec<CursorHeader>, String> {
        let db = open(path, OPEN_READONLY)?;
        let sql = CString::new("SELECT composerId, ifnull(isArchived,0), ifnull(isSubagent,0), ifnull(lastUpdatedAt,0), ifnull(value,'') FROM composerHeaders WHERE ifnull(isSubagent,0)=0").unwrap();
        let mut stmt = std::ptr::null_mut();
        let rc = unsafe { sqlite3_prepare_v2(db.0, sql.as_ptr(), -1, &mut stmt, std::ptr::null_mut()) };
        if rc != 0 { return Err("Cursor 会话表暂不可读".into()); }
        let mut rows = Vec::new();
        while unsafe { sqlite3_step(stmt) } == ROW {
            let id = text(stmt, 0);
            if id.is_empty() { continue; }
            rows.push(CursorHeader {
                id,
                archived: unsafe { sqlite3_column_int64(stmt, 1) } != 0,
                subagent: unsafe { sqlite3_column_int64(stmt, 2) } != 0,
                updated: unsafe { sqlite3_column_int64(stmt, 3) } / 1000,
                value: serde_json::from_str(&text(stmt, 4)).unwrap_or(Value::Null),
            });
        }
        unsafe { sqlite3_finalize(stmt); }
        Ok(rows)
    }
    pub fn exec(path: &Path, sql: &str) -> Result<(), String> {
        let db = open(path, OPEN_READWRITE | OPEN_CREATE)?;
        let sql = CString::new(sql).map_err(|_| "语句无效".to_string())?;
        let rc = unsafe { sqlite3_exec(db.0, sql.as_ptr(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
        if rc == 0 { Ok(()) } else { Err("无法写入测试数据库".into()) }
    }
}

fn cursor_row(roots: &Roots, header: &CursorHeader, now: i64, keep: i64) -> Option<Row> {
    let value = &header.value;
    let mode = value.get("unifiedMode").and_then(Value::as_str).unwrap_or("agent");
    let folder = value.get("workspaceIdentifier").and_then(|id| id.get("uri")).and_then(|uri| uri.get("fsPath")).and_then(Value::as_str).unwrap_or("").to_string();
    let transcript = roots.cursor_projects.join(cursor_slug(&folder)).join("agent-transcripts").join(&header.id).join(format!("{}.jsonl", header.id));
    let unread = value.get("hasUnreadMessages").and_then(Value::as_bool).unwrap_or(false);
    let unfinished = value.get("unfinishedRunAt").and_then(|v| if v.is_null() { None } else { Some(v) }).is_some();
    let header_age = (now - header.updated).max(0);
    let needs = value.get("hasBlockingPendingActions").and_then(Value::as_bool).unwrap_or(false)
        || value.get("hasPendingPlan").and_then(Value::as_bool).unwrap_or(false);
    if !needs && !unfinished && header_age > keep { return None; }
    let (turn, step, _tokens) = if transcript.is_file() { tail_turn(&read_tail(&transcript)) } else { (Turn::Idle, String::new(), None) };
    let age = file_age(&transcript, now).map(|file| file.min(header_age)).unwrap_or(header_age);
    let signals = Signals {
        agent: mode == "agent",
        archived: header.archived,
        subagent: header.subagent,
        draft: value.get("isDraft").and_then(Value::as_bool).unwrap_or(false),
        needs_approval: value.get("hasBlockingPendingActions").and_then(Value::as_bool).unwrap_or(false),
        pending_plan: value.get("hasPendingPlan").and_then(Value::as_bool).unwrap_or(false),
        unread,
        unfinished,
        turn,
        age_secs: age,
        keep_secs: keep,
        step: "",
    };
    let phase = phase_of(&signals)?;
    let title = value.get("name").and_then(Value::as_str).filter(|name| !name.is_empty()).unwrap_or("未命名会话").to_string();
    let subtitle = value.get("subtitle").and_then(Value::as_str).unwrap_or("");
    let step = if step.is_empty() { subtitle.to_string() } else { step };
    if title == "未命名会话" && step.is_empty() && phase == Phase::JustFinished { return None; }
    let usage = value.get("contextUsagePercent").and_then(Value::as_f64).filter(|n| n.is_finite() && *n > 0.0).map(|n| format!("{n:.0}%"));
    let children = value.get("numSubComposers").and_then(Value::as_u64).unwrap_or(0) as u32;
    let project = Path::new(&folder).file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    Some(Row {
        id: format!("cursor:{}", header.id),
        source: Source::Cursor,
        title,
        phase,
        step,
        usage,
        project,
        launch: Launch::Cursor { folder: PathBuf::from(folder) },
        children,
        updated: header.updated.max(now.saturating_sub(age)),
    })
}

fn claude_rows(roots: &Roots, now: i64, keep: i64) -> Vec<Row> {
    let mut rows = Vec::new();
    let projects = roots.home.join(".claude\\projects");
    rows.extend(jsonl_tree_launch(Source::Claude, now, keep, &projects, true));
    rows
}

fn codex_rows(roots: &Roots, now: i64, keep: i64) -> Vec<Row> {
    let root = roots.home.join(".codex\\sessions");
    if !root.is_dir() { return Vec::new(); }
    let titles = codex_titles(&roots.home.join(".codex\\session_index.jsonl"));
    let mut rows = Vec::new();
    let _ = visit_files(&root, &mut |path| {
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") { return; }
        let age = file_age(path, now).unwrap_or(i64::MAX);
        if age > CANDIDATE_SECS { return; }
        let id = codex_id(path);
        let text = read_tail(path);
        let (turn, step, tokens) = tail_turn(&text);
        let cwd = json_string(&text, &["cwd"]).unwrap_or_default();
        let unread = true;
        let signals = log_signals(turn, age, keep, unread, false);
        let Some(phase) = phase_of(&signals) else { return; };
        let title = titles.get(&id).cloned().or_else(|| first_user_title(&text)).unwrap_or_else(|| "Codex 会话".into());
        rows.push(log_row(Source::Codex, id, title, phase, step, tokens, &cwd, age, now, Launch::Program {
            program: "codex".into(),
            args: vec!["resume".into(), codex_id(path)],
            cwd: (!cwd.is_empty()).then(|| PathBuf::from(cwd.clone())),
        }));
    });
    rows
}

fn jsonl_tree(roots: &Roots, source: Source, now: i64, keep: i64, dir: &Path) -> Vec<Row> {
    let _ = roots;
    jsonl_tree_launch(source, now, keep, dir, false)
}

fn jsonl_tree_launch(source: Source, now: i64, keep: i64, dir: &Path, claude: bool) -> Vec<Row> {
    if !dir.is_dir() { return Vec::new(); }
    let mut rows = Vec::new();
    let _ = visit_files(dir, &mut |path| {
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") { return; }
        let age = file_age(path, now).unwrap_or(i64::MAX);
        if age > CANDIDATE_SECS { return; }
        let text = read_tail(path);
        let (turn, step, tokens) = tail_turn(&text);
        let signals = log_signals(turn, age, keep, true, false);
        let Some(phase) = phase_of(&signals) else { return; };
        let cwd = json_string(&text, &["cwd"]).unwrap_or_default();
        let id = path.file_stem().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let title = first_user_title(&text).unwrap_or_else(|| format!("{} 会话", source.name()));
        let desktop = path.to_string_lossy().contains("claude-code-sessions");
        let launch = if claude && desktop {
            Launch::Url(format!("claude://resume?session={id}"))
        } else if claude {
            Launch::Program { program: "claude".into(), args: vec!["--resume".into(), id.clone()], cwd: (!cwd.is_empty()).then(|| PathBuf::from(&cwd)) }
        } else {
            Launch::Program { program: source.name().to_string(), args: if cwd.is_empty() { Vec::new() } else { vec![cwd.clone()] }, cwd: (!cwd.is_empty()).then(|| PathBuf::from(&cwd)) }
        };
        rows.push(log_row(source, id, title, phase, step, tokens, &cwd, age, now, launch));
    });
    rows
}

fn dsh_rows(roots: &Roots, now: i64, keep: i64) -> Vec<Row> {
    let dir = roots.home.join(".dsh\\sessions");
    if !dir.is_dir() { return Vec::new(); }
    let archived = dsh_archived(&roots.home.join(".dsh\\storages\\workspace.json"));
    let mut rows = Vec::new();
    let _ = visit_files(&dir, &mut |path| {
        let age = file_age(path, now).unwrap_or(i64::MAX);
        if age > CANDIDATE_SECS { return; }
        let id = path.file_stem().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        if archived.iter().any(|item| item == &id) { return; }
        let text = if path.extension().and_then(|ext| ext.to_str()) == Some("zst") { decode_zst_tail(path) } else { read_tail(path) };
        if text.is_empty() { return; }
        let (turn, step, tokens) = tail_turn(&text);
        let signals = log_signals(turn, age, keep, true, false);
        let Some(phase) = phase_of(&signals) else { return; };
        let cwd = json_string(&text, &["cwd"]).unwrap_or_default();
        let title = first_user_title(&text).unwrap_or_else(|| "DeepSeek 会话".into());
        let folder = (!cwd.is_empty()).then(|| PathBuf::from(cwd.clone()));
        rows.push(log_row(Source::DeepSeekHarness, id, title, phase, step, tokens, &cwd, age, now, Launch::Program {
            program: "dsh".into(), args: Vec::new(), cwd: folder,
        }));
    });
    rows
}

fn antigravity_rows(roots: &Roots, now: i64, keep: i64) -> Vec<Row> {
    let brain = roots.home.join(".gemini\\antigravity\\brain");
    if !brain.is_dir() { return Vec::new(); }
    let mut rows = Vec::new();
    let entries = std::fs::read_dir(&brain).map(|dir| dir.flatten().map(|entry| entry.path()).collect::<Vec<_>>()).unwrap_or_default();
    for session in entries {
        let path = session.join(".system_generated\\logs\\transcript.jsonl");
        if !path.is_file() { continue; }
        let age = file_age(&path, now).unwrap_or(i64::MAX);
        if age > CANDIDATE_SECS { continue; }
        let text = read_tail(&path);
        let (turn, step, tokens) = tail_turn(&text);
        let signals = log_signals(turn, age, keep, true, false);
        let Some(phase) = phase_of(&signals) else { continue; };
        let id = session.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let title = first_user_title(&text).unwrap_or_else(|| "Antigravity 会话".into());
        rows.push(log_row(Source::Antigravity, id, title, phase, step, tokens, "", age, now, Launch::Program {
            program: "antigravity".into(), args: Vec::new(), cwd: None,
        }));
    }
    rows
}

fn log_signals(turn: Turn, age: i64, keep: i64, unread: bool, unfinished: bool) -> Signals<'static> {
    Signals { agent: true, archived: false, subagent: false, draft: false, needs_approval: false, pending_plan: false, unread, unfinished, turn, age_secs: age, keep_secs: keep, step: "" }
}

fn log_row(source: Source, id: String, title: String, phase: Phase, step: String, tokens: Option<u64>, cwd: &str, age: i64, now: i64, launch: Launch) -> Row {
    Row {
        id: format!("{}:{id}", source.name()),
        source,
        title,
        phase,
        step,
        usage: tokens.map(token_label),
        project: Path::new(cwd).file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default(),
        launch,
        children: 0,
        updated: now.saturating_sub(age.max(0)),
    }
}

pub fn tail_turn(text: &str) -> (Turn, String, Option<u64>) {
    let mut turn = Turn::Idle;
    let mut step = String::new();
    let mut ended: Option<bool> = None;
    let mut tokens: Option<u64> = None;
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else { continue; };
        if let Some(sum) = usage_tokens(&value) { tokens = Some(sum); }
        if value.get("type").and_then(Value::as_str) == Some("turn_ended") {
            let ok = value.get("status").and_then(Value::as_str).unwrap_or("") == "success";
            ended = Some(ok);
            turn = if ok { Turn::Success } else { Turn::Error };
            continue;
        }
        let role = value.get("role").and_then(Value::as_str).or_else(|| value.pointer("/message/role").and_then(Value::as_str));
        if role == Some("user") || role == Some("assistant") {
            // 结束行后面又有新的发言，说明上一轮已经过去，这一轮才刚开始。
            if ended.is_some() { ended = None; turn = Turn::Tool; step.clear(); }
        }
        if value.get("is_error").and_then(Value::as_bool) == Some(true) || value.get("type").and_then(Value::as_str) == Some("error") {
            ended = Some(false);
            turn = Turn::Error;
            continue;
        }
        if let Some(name) = tool_name(&value) {
            ended = None;
            turn = Turn::Tool;
            step = name;
        }
    }
    if ended == Some(true) { turn = Turn::Success; }
    if ended == Some(false) { turn = Turn::Error; }
    (turn, step, tokens)
}

fn tool_name(value: &Value) -> Option<String> {
    let blocks = value.pointer("/message/content").or_else(|| value.pointer("/payload/content")).and_then(Value::as_array)?;
    let mut name = None;
    for block in blocks {
        let kind = block.get("type").and_then(Value::as_str).unwrap_or("");
        if kind == "tool_use" || kind == "tool_call" || kind == "function_call" {
            name = block.get("name").or_else(|| block.get("tool")).and_then(Value::as_str).map(str::to_string);
        }
    }
    name
}

fn usage_tokens(value: &Value) -> Option<u64> {
    let usage = value.pointer("/message/usage").or_else(|| value.get("usage")).or_else(|| value.pointer("/payload/usage"))?;
    let input = usage.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
    let output = usage.get("output_tokens").and_then(Value::as_u64).unwrap_or(0);
    let fresh = usage.get("input_tokens").is_some() || usage.get("output_tokens").is_some();
    fresh.then_some(input.saturating_add(output)).filter(|n| *n > 0)
}

fn token_label(tokens: u64) -> String {
    if tokens >= 1_000_000 { format!("{:.1}M", tokens as f64 / 1_000_000.0) }
    else if tokens >= 1_000 { format!("{}k", ((tokens as f64) / 1000.0).round() as u64) }
    else { tokens.to_string() }
}

fn first_user_title(text: &str) -> Option<String> {
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else { continue; };
        let role = value.get("role").and_then(Value::as_str).or_else(|| value.pointer("/message/role").and_then(Value::as_str)).or_else(|| value.pointer("/payload/role").and_then(Value::as_str));
        if role != Some("user") && value.get("type").and_then(Value::as_str) != Some("user") { continue; }
        let raw = value.pointer("/message/content").or_else(|| value.get("content")).or_else(|| value.pointer("/payload/content"));
        let text = match raw {
            Some(Value::String(text)) => text.clone(),
            Some(Value::Array(blocks)) => blocks.iter().find_map(|block| block.get("text").and_then(Value::as_str).or_else(|| block.get("content").and_then(Value::as_str))).unwrap_or("").to_string(),
            _ => String::new(),
        };
        let text = text.lines().find(|line| !line.trim().is_empty()).unwrap_or("").trim();
        if text.is_empty() || text.starts_with('<') { continue; }
        let mut title: String = text.chars().take(24).collect();
        if text.chars().count() > 24 { title.push('…'); }
        return Some(title);
    }
    None
}

fn json_string(text: &str, keys: &[&str]) -> Option<String> {
    for line in text.lines().rev() {
        let Ok(value) = serde_json::from_str::<Value>(line) else { continue; };
        for key in keys {
            if let Some(found) = value.get(*key).and_then(Value::as_str).filter(|s| !s.is_empty()) { return Some(found.to_string()); }
            if let Some(found) = value.pointer(&format!("/payload/{key}")).and_then(Value::as_str).filter(|s| !s.is_empty()) { return Some(found.to_string()); }
        }
    }
    None
}

fn codex_titles(path: &Path) -> std::collections::HashMap<String, String> {
    let mut titles = std::collections::HashMap::new();
    let Ok(text) = std::fs::read_to_string(path) else { return titles; };
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else { continue; };
        let id = value.get("id").or_else(|| value.get("session_id")).and_then(Value::as_str).unwrap_or("");
        let title = value.get("title").or_else(|| value.get("name")).and_then(Value::as_str).unwrap_or("");
        if !id.is_empty() && !title.is_empty() { titles.insert(id.to_string(), title.to_string()); }
    }
    titles
}

fn codex_id(path: &Path) -> String {
    let name = path.file_stem().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    name.rsplit('-').collect::<Vec<_>>().into_iter().rev().take(5).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("-")
}

fn dsh_archived(path: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new(); };
    let Ok(value) = serde_json::from_str::<Value>(&text) else { return Vec::new(); };
    let mut ids = Vec::new();
    fn walk(value: &Value, ids: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                if map.get("archived").and_then(Value::as_bool) == Some(true) {
                    if let Some(id) = map.get("id").and_then(Value::as_str) { ids.push(id.to_string()); }
                }
                for child in map.values() { walk(child, ids); }
            }
            Value::Array(items) => { for child in items { walk(child, ids); } }
            _ => {}
        }
    }
    walk(&value, &mut ids);
    ids
}

fn decode_zst_tail(path: &Path) -> String {
    let Ok(file) = File::open(path) else { return String::new(); };
    let Ok(meta) = file.metadata() else { return String::new(); };
    if meta.len() > 4 * 1024 * 1024 { return String::new(); }
    let mut raw = Vec::new();
    let mut file = file;
    if file.read_to_end(&mut raw).is_err() { return String::new(); }
    zstd::stream::decode_all(raw.as_slice()).ok().and_then(|bytes| String::from_utf8(bytes).ok()).unwrap_or_default()
}

fn read_tail(path: &Path) -> String {
    let Ok(mut file) = File::open(path) else { return String::new(); };
    let Ok(len) = file.metadata().map(|meta| meta.len()) else { return String::new(); };
    let start = len.saturating_sub(TAIL_BYTES);
    if file.seek(SeekFrom::Start(start)).is_err() { return String::new(); }
    let mut buf = String::new();
    if file.read_to_string(&mut buf).is_err() { return String::new(); }
    if start > 0 { if let Some(pos) = buf.find('\n') { buf.drain(..=pos); } }
    buf
}

fn file_age(path: &Path, now: i64) -> Option<i64> {
    let modified = path.metadata().ok()?.modified().ok()?;
    let stamp = modified.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    Some((now - stamp).max(0))
}

fn visit_files(dir: &Path, visit: &mut dyn FnMut(&Path)) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue; };
        let path = entry.path();
        if path.is_dir() { let _ = visit_files(&path, visit); }
        else { visit(&path); }
    }
    Ok(())
}

pub fn launch_plan(launch: &Launch) -> LaunchPlan {
    match launch {
        Launch::Cursor { folder } => LaunchPlan {
            program: cursor_executable().unwrap_or_else(|| "Cursor.exe".into()),
            args: if folder.as_os_str().is_empty() { Vec::new() } else { vec![folder.display().to_string()] },
            cwd: None,
        },
        Launch::Url(url) => LaunchPlan { program: "cmd".into(), args: vec!["/C".into(), "start".into(), "".into(), url.clone()], cwd: None },
        Launch::Program { program, args, cwd } => LaunchPlan { program: program.clone(), args: args.clone(), cwd: cwd.clone() },
    }
}

pub fn launch(launch: &Launch) -> Result<(), String> {
    let plan = launch_plan(launch);
    if matches!(launch, Launch::Cursor { .. }) && cursor_executable().is_none() { return Err("未找到该软件".into()); }
    let mut command = std::process::Command::new(&plan.program);
    command.args(&plan.args);
    if let Some(cwd) = &plan.cwd { command.current_dir(cwd); }
    crate::child_process::spawn(&mut command).map(|_| ()).map_err(|_| "未找到该软件".into())
}

fn cursor_executable() -> Option<String> {
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?;
    for candidate in [local.join("Programs\\cursor\\Cursor.exe"), local.join("Cursor\\Cursor.exe")] {
        if candidate.is_file() { return Some(candidate.display().to_string()); }
    }
    None
}

pub struct Watch {
    pub rows: Vec<Row>,
    pub first: usize,
    /// 测试注入的目录。正式运行留空，表示读本机。
    pub roots: Option<Roots>,
    incoming: Option<std::sync::mpsc::Receiver<Vec<Row>>>,
    scanned_at: f64,
    noticed: std::collections::HashMap<String, Phase>,
    /// 点开之后先撤下。同一状态、同一次更新不再冒出来；重新跑起来或状态变了再出现。
    hidden: std::collections::HashMap<String, (Phase, i64)>,
    primed: bool,
}

impl Default for Watch {
    fn default() -> Self {
        Self { rows: Vec::new(), first: 0, roots: None, incoming: None, scanned_at: -1.0e9, noticed: std::collections::HashMap::new(), hidden: std::collections::HashMap::new(), primed: false }
    }
}

impl Watch {
    pub fn poll(&mut self, now_tick: f64, keep_mins: u32, sources: u8, enabled: bool) -> Vec<(Phase, String, String)> {
        if !enabled {
            self.rows.clear();
            self.incoming = None;
            return Vec::new();
        }
        let mut notices = Vec::new();
        if let Some(incoming) = &self.incoming {
            if let Ok(rows) = incoming.try_recv() {
                notices = self.install(rows);
                self.incoming = None;
            }
        }
        if self.incoming.is_none() && now_tick - self.scanned_at >= 3.0 {
            if cfg!(test) && self.roots.is_none() { return notices; }
            let roots = self.roots.clone().unwrap_or_else(Roots::system);
            let (tx, rx) = std::sync::mpsc::channel();
            self.incoming = Some(rx);
            self.scanned_at = now_tick;
            std::thread::spawn(move || {
                let _ = tx.send(scan(&roots, unix_now(), keep_mins, sources));
            });
        }
        notices
    }

    pub fn dismiss(&mut self, id: &str) {
        if let Some(row) = self.rows.iter().find(|row| row.id == id) {
            self.hidden.insert(id.to_string(), (row.phase, row.updated));
        }
        self.rows.retain(|row| row.id != id);
    }

    fn install(&mut self, mut rows: Vec<Row>) -> Vec<(Phase, String, String)> {
        rows.retain(|row| self.hidden.get(&row.id) != Some(&(row.phase, row.updated)));
        let mut notices = Vec::new();
        if self.primed {
            for row in &rows {
                let previous = self.noticed.get(&row.id).copied();
                if row.phase.notifies() && previous != Some(row.phase) {
                    notices.push((row.phase, row.source.name().to_string(), row.title.clone()));
                }
            }
        }
        self.noticed.clear();
        for row in &rows { self.noticed.insert(row.id.clone(), row.phase); }
        self.primed = true;
        let max = rows.len().saturating_sub(1);
        self.first = self.first.min(max);
        self.rows = rows;
        notices
    }
}

pub fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signals(turn: Turn, age: i64, keep_mins: i64) -> Signals<'static> {
        Signals { agent: true, archived: false, subagent: false, draft: false, needs_approval: false, pending_plan: false, unread: true, unfinished: false, turn, age_secs: age, keep_secs: keep_mins * 60, step: "" }
    }

    #[test]
    fn four_states_follow_the_record_and_the_keep_window() {
        let mut needs = signals(Turn::Success, 30, 10);
        needs.needs_approval = true;
        assert_eq!(phase_of(&needs), Some(Phase::NeedsYou));
        let mut plan = signals(Turn::Idle, 10, 10);
        plan.pending_plan = true;
        assert_eq!(phase_of(&plan), Some(Phase::NeedsYou));
        assert_eq!(phase_of(&signals(Turn::Error, 30, 10)), Some(Phase::Errored));
        assert_eq!(phase_of(&signals(Turn::Error, 11 * 60, 10)), None, "过了保留时间的报错不再占着列表");
        let mut still = signals(Turn::Success, 20, 10);
        still.unfinished = true;
        assert_eq!(phase_of(&still), Some(Phase::Running), "头上还标着没跑完，上一轮结束也不能把它藏起来");
        let mut running = signals(Turn::Tool, 20, 10);
        running.unfinished = true;
        assert_eq!(phase_of(&running), Some(Phase::Running));
        assert_eq!(phase_of(&signals(Turn::Success, 9 * 60, 10)), Some(Phase::JustFinished));
        assert_eq!(phase_of(&signals(Turn::Success, 11 * 60, 10)), None, "超过保留时间就撤掉");
        let mut seen = signals(Turn::Success, 60, 10);
        seen.unread = false;
        assert_eq!(phase_of(&seen), Some(Phase::JustFinished), "软件里已经看过，岛上还没点开，保留时间内仍留着");
        let mut stalled = signals(Turn::Tool, 20 * 60, 10);
        stalled.unfinished = true;
        assert_eq!(phase_of(&stalled), None, "好几分钟没再写记录就不算正在跑");
    }

    #[test]
    fn cursor_icon_rasterizes_to_visible_pixels() {
        let image = egui_extras::image::load_svg_bytes_with_size(include_bytes!("../assets/ai-icons/cursor.svg"), Some(eframe::egui::load::SizeHint::Size(32, 32))).expect("Cursor 图标应能栅格化");
        assert!(image.pixels.iter().any(|pixel| pixel.a() > 0), "Cursor 图标不能是空白");
    }

    #[test]
    fn cursor_slug_drops_drive_colon_and_non_ascii() {
        assert_eq!(cursor_slug(r"e:\灵动岛2"), "e-2");
        assert_eq!(cursor_slug(r"C:\Users\Administrator\Desktop\APP"), "c-Users-Administrator-Desktop-APP");
    }

    #[test]
    fn tail_marks_a_tool_as_running_and_a_failed_turn_as_error() {
        let running = r#"{"role":"assistant","message":{"content":[{"type":"tool_use","name":"Read"}]}}"#;
        assert_eq!(tail_turn(running).0, Turn::Tool);
        assert_eq!(tail_turn(running).1, "Read");
        let failed = "{\"type\":\"turn_ended\",\"status\":\"error\"}\n";
        assert_eq!(tail_turn(failed).0, Turn::Error);
        let done = "{\"type\":\"turn_ended\",\"status\":\"success\"}\n";
        assert_eq!(tail_turn(done).0, Turn::Success);
        let resumed = "{\"type\":\"turn_ended\",\"status\":\"success\"}\n{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"name\":\"Shell\"}]}}\n";
        assert_eq!(tail_turn(resumed).0, Turn::Tool);
        let asked = "{\"type\":\"turn_ended\",\"status\":\"success\"}\n{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\"}]}}\n";
        assert_eq!(tail_turn(asked).0, Turn::Tool, "新的提问要盖过上一轮的结束");
    }

    #[test]
    fn usage_is_output_plus_fresh_input_and_context_percent_stays_on_the_header() {
        let line = r#"{"role":"assistant","message":{"usage":{"input_tokens":1200,"output_tokens":800,"cache_read_input_tokens":9000}}}"#;
        assert_eq!(tail_turn(line).2, Some(2000));
        assert_eq!(token_label(2000), "2k");
    }

    #[test]
    fn launch_plan_opens_the_client_for_that_row() {
        let cursor = launch_plan(&Launch::Cursor { folder: PathBuf::from(r"E:\work") });
        assert!(cursor.args == vec![r"E:\work"] || cursor.program.ends_with("Cursor.exe"));
        assert!(cursor.args.iter().any(|arg| arg.contains("work")) || cursor.program.contains("Cursor"));
        let resume = launch_plan(&Launch::Url("claude://resume?session=abc".into()));
        assert_eq!(resume.args, vec!["/C", "start", "", "claude://resume?session=abc"]);
        let codex = launch_plan(&Launch::Program { program: "codex".into(), args: vec!["resume".into(), "abc".into()], cwd: Some(PathBuf::from(r"E:\work")) });
        assert_eq!(codex.program, "codex");
        assert_eq!(codex.args, vec!["resume", "abc"]);
        assert_eq!(codex.cwd.unwrap(), PathBuf::from(r"E:\work"));
    }

    #[test]
    #[cfg(windows)]
    fn cursor_header_and_transcript_become_one_row() {
        let dir = std::env::temp_dir().join(format!("daozai-session-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let db = dir.join("state.vscdb");
        let projects = dir.join("projects");
        let id = "11111111-1111-1111-1111-111111111111";
        let folder = projects.join("e-work").join("agent-transcripts").join(id);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join(format!("{id}.jsonl")), "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"name\":\"Shell\"}]}}\n").unwrap();
        let value = serde_json::json!({
            "name": "改首页",
            "unifiedMode": "agent",
            "unfinishedRunAt": 10,
            "hasUnreadMessages": false,
            "contextUsagePercent": 21.2,
            "subtitle": "Edited app.rs",
            "numSubComposers": 2,
            "workspaceIdentifier": {"uri": {"fsPath": r"e:\work"}}
        });
        let stamp = unix_now() * 1000;
        let escaped = value.to_string().replace('\'', "''");
        super::sqlite::exec(&db, &format!("CREATE TABLE composerHeaders (composerId TEXT, isArchived INTEGER, isSubagent INTEGER, lastUpdatedAt INTEGER, value TEXT); INSERT INTO composerHeaders VALUES ('{id}', 0, 0, {stamp}, '{escaped}');")).unwrap();
        let roots = Roots { home: dir.clone(), cursor_db: db, cursor_projects: projects };
        let rows = scan(&roots, unix_now(), 10, Source::Cursor.bit());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].phase, Phase::Running);
        assert_eq!(rows[0].title, "改首页");
        assert_eq!(rows[0].step, "Shell");
        assert_eq!(rows[0].usage.as_deref(), Some("21%"));
        assert_eq!(rows[0].children, 2);
        assert!(matches!(rows[0].launch, Launch::Cursor { .. }));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn sort_puts_errors_above_running_and_newer_first() {
        let row = |phase: Phase, updated: i64| Row { id: updated.to_string(), source: Source::Codex, title: String::new(), phase, step: String::new(), usage: None, project: String::new(), launch: Launch::Url(String::new()), children: 0, updated };
        let mut rows = vec![row(Phase::JustFinished, 30), row(Phase::Running, 10), row(Phase::Running, 20), row(Phase::Errored, 5), row(Phase::NeedsYou, 1)];
        sort_rows(&mut rows);
        let order: Vec<_> = rows.iter().map(|row| (row.phase, row.updated)).collect();
        assert_eq!(order, vec![(Phase::NeedsYou, 1), (Phase::Errored, 5), (Phase::Running, 20), (Phase::Running, 10), (Phase::JustFinished, 30)]);
    }

    #[test]
    fn opening_a_row_hides_it_until_the_state_changes() {
        let row = |phase, updated| Row { id: "cursor:1".into(), source: Source::Cursor, title: "改首页".into(), phase, step: String::new(), usage: None, project: String::new(), launch: Launch::Url(String::new()), children: 0, updated };
        let mut watch = Watch::default();
        watch.install(vec![row(Phase::Errored, 5)]);
        watch.dismiss("cursor:1");
        assert!(watch.rows.is_empty(), "点开后这一条先撤掉");
        watch.install(vec![row(Phase::Errored, 5)]);
        assert!(watch.rows.is_empty(), "同一场还停在原来的状态，不再冒出来");
        let again = watch.install(vec![row(Phase::Running, 9)]);
        assert_eq!(watch.rows.len(), 1);
        assert_eq!(watch.rows[0].phase, Phase::Running, "重新跑起来要再出现");
        assert!(again.is_empty(), "正在跑不弹通知");
    }
}
