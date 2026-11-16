//! 第三方待办：把 Jira / Linear / GitHub / Notion / 飞书 / Windows 待办 / 自建 JSON
//! 同步进岛仔现有待办页，与本地待办并存。令牌只进 Windows 凭据管理器。

use serde::{Deserialize, Serialize};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

pub mod adapters;
pub mod writeback;
pub mod windows_auth;

pub use adapters::{Bucket, RemoteItem, SyncOutcome};

const SERVICE: &str = "Daozai.TodoSync";
const CONFIG_FILE: &str = "todo-sync.json";
const DONE_FILE: &str = "todo-sync-done.json";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    #[default]
    Jira,
    Linear,
    GitHub,
    Notion,
    Feishu,
    WindowsTodo,
    CustomJson,
}

impl Source {
    pub const ALL: [Source; 7] = [
        Source::Jira, Source::Linear, Source::GitHub, Source::Notion,
        Source::Feishu, Source::WindowsTodo, Source::CustomJson,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Jira => "Jira",
            Self::Linear => "Linear",
            Self::GitHub => "GitHub Issues",
            Self::Notion => "Notion",
            Self::Feishu => "飞书",
            Self::WindowsTodo => "Windows 待办",
            Self::CustomJson => "自建 JSON",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Self::Jira => "jira",
            Self::Linear => "linear",
            Self::GitHub => "github",
            Self::Notion => "notion",
            Self::Feishu => "feishu",
            Self::WindowsTodo => "windows",
            Self::CustomJson => "json",
        }
    }
    pub fn address_label(self) -> &'static str {
        match self {
            Self::Jira => "平台 URL",
            Self::Linear => "工作区（可选）",
            Self::GitHub => "仓库",
            Self::Notion => "数据库",
            Self::Feishu => "多维表格",
            Self::WindowsTodo => "Azure 应用 Client ID",
            Self::CustomJson => "JSON 地址",
        }
    }
    pub fn address_hint(self) -> &'static str {
        match self {
            Self::Jira => "例如 https://你的站点.atlassian.net 或 http://主机:端口",
            Self::Linear => "可留空；Linear 按当前账号的指派议题同步",
            Self::GitHub => "owner/repo，或仓库完整网址",
            Self::Notion => "数据库 ID，或数据库页面链接",
            Self::Feishu => "app_token/table_id（飞书多维表格）",
            Self::WindowsTodo => "Azure 公共客户端（移动和桌面）的 Application (client) ID，需允许 http://localhost 回调",
            Self::CustomJson => "返回含 due / soon / unscheduled 数组的 HTTPS 地址",
        }
    }
    pub fn token_label(self) -> &'static str {
        match self {
            Self::Jira => "API Token",
            Self::Linear => "API Key",
            Self::GitHub => "Personal Access Token",
            Self::Notion => "Integration Token",
            Self::Feishu => "飞书凭据",
            Self::WindowsTodo => "（登录微软账号，无需填令牌）",
            Self::CustomJson => "Bearer 令牌（可选）",
        }
    }
    pub fn token_hint(self) -> &'static str {
        match self {
            Self::Jira => "Jira Cloud 可用「邮箱:API 令牌」，或粘贴个人访问令牌；保存后只存放在 Windows 凭据管理器。",
            Self::Linear => "Linear 设置 → API → Personal API Key；请求头直接使用该密钥。",
            Self::GitHub => "需要 repo 或 issues 权限的 PAT（classic 或 fine-grained）。",
            Self::Notion => "Notion Integration Token，并把该 Integration 邀请进目标数据库。",
            Self::Feishu => "填写 tenant_access_token，或 app_id:app_secret（应用身份，只读同步）。",
            Self::WindowsTodo => "点击「登录微软账号」完成一次系统授权（Tasks.ReadWrite）。定时同步不会再弹窗。",
            Self::CustomJson => "若接口需要鉴权，填写 Bearer 令牌；可留空。",
        }
    }
    pub fn needs_token(self) -> bool { self != Self::WindowsTodo }
    pub fn supports_writeback(self) -> bool {
        matches!(self, Self::Jira | Self::Linear | Self::GitHub | Self::Notion | Self::WindowsTodo)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    pub source: Source,
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_interval")]
    pub interval_min: u32,
    #[serde(default = "default_soon")]
    pub soon_days: u32,
    #[serde(default)]
    pub last_sync: String,
    #[serde(default)]
    pub last_message: String,
    #[serde(default)]
    pub last_ok: bool,
    /// 已确认的粘性提醒：(事项 done_key, 当时紧急档)。升档后会再次提醒。
    #[serde(default)]
    pub acked_nudges: Vec<(String, u8)>,
}

fn default_interval() -> u32 { 60 }
fn default_soon() -> u32 { 3 }

impl Default for Config {
    fn default() -> Self {
        Self {
            source: Source::Jira,
            address: String::new(),
            enabled: false,
            interval_min: 60,
            soon_days: 3,
            last_sync: String::new(),
            last_message: String::new(),
            last_ok: false,
            acked_nudges: Vec::new(),
        }
    }
}

impl Config {
    pub fn load() -> Self {
        let path = crate::model::config_dir().join(CONFIG_FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }
    pub fn save(&self) {
        let path = crate::model::config_dir().join(CONFIG_FILE);
        if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = crate::model::atomic_write(&path, text.as_bytes());
        }
    }
    pub fn soon_days(&self) -> u32 { self.soon_days.clamp(1, 30) }
    pub fn interval_min(&self) -> u32 { self.interval_min.clamp(5, 240) }
}

fn token_slot(source: Source, address: &str) -> String {
    let raw = format!("{}:{}", source.key(), address.trim().to_ascii_lowercase());
    let digest = ring::digest::digest(&ring::digest::SHA256, raw.as_bytes());
    let hex: String = digest.as_ref()[..12].iter().map(|b| format!("{b:02x}")).collect();
    format!("token-{}-{hex}", source.key())
}

pub fn save_token(source: Source, address: &str, token: &str) -> Result<(), String> {
    let token = token.trim();
    if token.is_empty() { return delete_token(source, address); }
    #[cfg(windows)]
    {
        let entry = keyring::Entry::new(SERVICE, &token_slot(source, address))
            .map_err(|_| "无法打开 Windows 凭据管理器".to_string())?;
        entry.set_password(token).map_err(|_| "无法把令牌写入 Windows 凭据管理器".to_string())
    }
    #[cfg(not(windows))]
    {
        let _ = (source, address, token);
        Err("当前系统不支持安全保存令牌".into())
    }
}

pub fn load_token(source: Source, address: &str) -> Result<String, String> {
    #[cfg(windows)]
    {
        let entry = keyring::Entry::new(SERVICE, &token_slot(source, address))
            .map_err(|_| "无法打开 Windows 凭据管理器".to_string())?;
        match entry.get_password() {
            Ok(v) if !v.is_empty() => Ok(v),
            Ok(_) | Err(keyring::Error::NoEntry) => Err("尚未保存令牌".into()),
            Err(_) => Err("暂时无法读取 Windows 凭据，请稍后重试".into()),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (source, address);
        Err("当前系统不支持读取已保存的令牌".into())
    }
}

pub fn delete_token(source: Source, address: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        match keyring::Entry::new(SERVICE, &token_slot(source, address)) {
            Ok(entry) => match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(_) => Err("Windows 凭据清理未完成".into()),
            },
            Err(_) => Ok(()),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (source, address);
        Ok(())
    }
}

pub fn token_saved(source: Source, address: &str) -> bool {
    load_token(source, address).is_ok()
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct DoneSet { keys: Vec<String> }

impl DoneSet {
    fn load() -> Self {
        let path = crate::model::config_dir().join(DONE_FILE);
        std::fs::read_to_string(path).ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }
    fn save(&self) {
        let path = crate::model::config_dir().join(DONE_FILE);
        if let Some(p) = path.parent() { let _ = std::fs::create_dir_all(p); }
        if let Ok(t) = serde_json::to_string_pretty(self) {
            let _ = crate::model::atomic_write(&path, t.as_bytes());
        }
    }
    fn contains(&self, key: &str) -> bool { self.keys.iter().any(|k| k == key) }
    fn insert(&mut self, key: String) {
        if !self.contains(&key) { self.keys.push(key); self.save(); }
    }
    fn remove(&mut self, key: &str) {
        let n = self.keys.len();
        self.keys.retain(|k| k != key);
        if self.keys.len() != n { self.save(); }
    }
}

pub struct State {
    pub config: Config,
    pub items: Vec<RemoteItem>,
    pub expanded: Option<String>,
    pub form_token: String,
    pub form_show_token: bool,
    pub form_address: String,
    pub syncing: bool,
    pub writing: bool,
    pub note: String,
    pub note_error: bool,
    pub windows_login: String,
    rx: Option<Receiver<SyncOutcome>>,
    write_rx: Option<Receiver<(bool, String, String, bool)>>, // ok, msg, done_key, completing
    last_auto: Instant,
    done: DoneSet,
    /// 用于界面的「手动同步」防抖。
    last_manual: Instant,
}

impl State {
    pub fn load() -> Self {
        let config = Config::load();
        let form_address = config.address.clone();
        let mut state = Self {
            config,
            items: Vec::new(),
            expanded: None,
            form_token: String::new(),
            form_show_token: false,
            form_address,
            syncing: false,
            writing: false,
            note: String::new(),
            note_error: false,
            windows_login: String::new(),
            rx: None,
            write_rx: None,
            last_auto: Instant::now() - Duration::from_secs(3600),
            done: DoneSet::load(),
            last_manual: Instant::now() - Duration::from_secs(10),
        };
        if !state.config.last_message.is_empty() {
            state.note = state.config.last_message.clone();
            state.note_error = !state.config.last_ok;
        }
        state
    }

    pub fn open_count(&self) -> (usize, usize, usize, usize) {
        let mut due = 0;
        let mut soon = 0;
        let mut none = 0;
        let mut later = 0;
        for it in &self.items {
            if it.done { continue; }
            match it.bucket {
                adapters::Bucket::Due => due += 1,
                adapters::Bucket::Soon => soon += 1,
                adapters::Bucket::Unscheduled => none += 1,
                adapters::Bucket::Later => later += 1,
            }
        }
        (due, soon, none, later)
    }

    /// 待办页在没有第三方事项时的说明：区分尚未同步、同步失败、同步成功但没有匹配。
    pub fn remote_empty_hint(&self) -> Option<String> {
        remote_empty_hint_text(self.items.is_empty(), self.syncing, &self.config.last_sync, self.config.last_ok, &self.config.last_message)
    }

    pub fn pending_nudges(&self) -> Vec<&RemoteItem> {
        pending_nudge_items(&self.items, &self.config.acked_nudges)
    }

    pub fn ack_pending_nudges(&mut self) {
        let updates: Vec<(String, u8)> = self.pending_nudges().into_iter()
            .filter_map(|it| it.bucket.nudge_level().map(|level| (it.done_key(), level)))
            .collect();
        for (key, level) in updates {
            if let Some(slot) = self.config.acked_nudges.iter_mut().find(|(k, _)| k == &key) {
                if level > slot.1 { slot.1 = level; }
            } else {
                self.config.acked_nudges.push((key, level));
            }
        }
        self.config.save();
    }

    pub fn display_items(&self) -> Vec<usize> {
        display_item_order(&self.items)
    }

    pub fn poll(&mut self, auto: bool) {
        if let Some(rx) = &self.rx {
            if let Ok(outcome) = rx.try_recv() {
                self.syncing = false;
                self.rx = None;
                self.apply_outcome(outcome);
            }
        }
        if let Some(rx) = &self.write_rx {
            if let Ok((ok, msg, key, completing)) = rx.try_recv() {
                self.writing = false;
                self.write_rx = None;
                self.note = msg;
                self.note_error = !ok;
                if let Some(item) = self.items.iter_mut().find(|it| it.done_key() == key) {
                    if completing {
                        if ok {
                            item.done = true;
                            self.done.insert(key);
                        } else {
                            item.done = false;
                        }
                    } else if ok {
                        item.done = false;
                        self.done.remove(&key);
                    }
                }
            }
        }
        if auto && self.config.enabled && !self.syncing {
            let gap = Duration::from_secs(self.config.interval_min() as u64 * 60);
            if self.last_auto.elapsed() >= gap {
                let _ = self.start_sync(false);
            }
        }
    }

    fn apply_outcome(&mut self, outcome: SyncOutcome) {
        self.config.last_sync = now_stamp();
        self.config.last_message = outcome.message.clone();
        self.config.last_ok = outcome.ok;
        self.config.save();
        self.note = outcome.message;
        self.note_error = !outcome.ok;
        if outcome.ok {
            let mut items = outcome.items;
            for it in &mut items {
                if self.done.contains(&it.done_key()) {
                    it.done = true;
                }
            }
            self.items = items;
        }
    }

    pub fn persist_settings(&mut self) {
        self.config.address = self.form_address.trim().to_string();
        self.config.save();
        if self.config.source.needs_token() && !self.form_token.trim().is_empty() {
            match save_token(self.config.source, &self.config.address, &self.form_token) {
                Ok(()) => {
                    self.form_token.clear();
                    self.note = "令牌已保存到 Windows 凭据管理器".into();
                    self.note_error = false;
                }
                Err(e) => { self.note = e; self.note_error = true; }
            }
        }
    }

    pub fn start_sync(&mut self, manual: bool) -> bool {
        if self.syncing { return false; }
        if manual && self.last_manual.elapsed() < Duration::from_secs(2) {
            self.note = "同步过于频繁，请稍后再试".into();
            self.note_error = true;
            return false;
        }
        self.config.address = self.form_address.trim().to_string();
        if self.config.source.needs_token() && !self.form_token.trim().is_empty() {
            if let Err(e) = save_token(self.config.source, &self.config.address, &self.form_token) {
                self.note = e; self.note_error = true; return false;
            }
            self.form_token.clear();
        }
        self.config.save();
        let token = if self.config.source.needs_token() {
            match load_token(self.config.source, &self.config.address) {
                Ok(t) => t,
                Err(e) => { self.note = e; self.note_error = true; return false; }
            }
        } else { String::new() };
        let source = self.config.source;
        let address = self.config.address.clone();
        let soon = self.config.soon_days();
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.syncing = true;
        self.last_auto = Instant::now();
        if manual { self.last_manual = Instant::now(); }
        self.note = "正在同步…".into();
        self.note_error = false;
        let _ = std::thread::Builder::new().name("daozai-todo-sync".into()).spawn(move || {
            let result = adapters::sync(source, &address, &token, soon, manual);
            let _ = tx.send(result);
        });
        true
    }

    pub fn toggle(&mut self, index: usize) {
        let Some(item) = self.items.get(index).cloned() else { return; };
        let completing = !item.done;
        if !item.source.supports_writeback() {
            if completing {
                self.items[index].done = true;
                self.done.insert(item.done_key());
                self.note = if item.source == Source::Feishu {
                    "已在本地标记完成（飞书为只读同步，请在飞书中关闭任务）".into()
                } else {
                    "已在本地标记完成（当前源不支持写回）".into()
                };
            } else {
                self.items[index].done = false;
                self.done.remove(&item.done_key());
                self.note = "已取消完成标记".into();
            }
            self.note_error = false;
            return;
        }
        if self.writing { self.note = "正在写回，请稍候".into(); self.note_error = true; return; }
        // 乐观更新，失败再改回。
        self.items[index].done = completing;
        let token = if item.source.needs_token() {
            match load_token(item.source, &self.config.address) {
                Ok(t) => t,
                Err(e) => {
                    self.items[index].done = !completing;
                    self.note = e; self.note_error = true; return;
                }
            }
        } else { String::new() };
        let address = self.config.address.clone();
        let key = item.done_key();
        let (tx, rx) = mpsc::channel();
        self.write_rx = Some(rx);
        self.writing = true;
        self.note = "正在写回…".into();
        self.note_error = false;
        let _ = std::thread::Builder::new().name("daozai-todo-write".into()).spawn(move || {
            let result = if completing {
                writeback::complete(item.source, &address, &token, &item)
            } else {
                Err("取消完成不会写回源，仅本地恢复为未完成".into())
            };
            let (ok, msg) = match result {
                Ok(m) => (true, m),
                Err(m) => {
                    // 取消完成：本地允许，不算失败。
                    if !completing { (true, "已在本地恢复为未完成".into()) } else { (false, m) }
                }
            };
            let _ = tx.send((ok, msg, key, completing));
        });
    }
}

pub fn pending_nudge_items<'a>(items: &'a [RemoteItem], acked: &[(String, u8)]) -> Vec<&'a RemoteItem> {
    items.iter().filter(|it| {
        if it.done { return false; }
        let Some(level) = it.bucket.nudge_level() else { return false; };
        match acked.iter().find(|(k, _)| k == &it.done_key()) {
            Some((_, acked)) if *acked >= level => false,
            _ => true,
        }
    }).collect()
}


/// 分组内按截止日期由近到远；未排期没有日期，按最近更新时间由新到旧。已完成的事项总是放在最后。
pub fn display_item_order(items: &[RemoteItem]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by(|&a, &b| {
        let ia = &items[a];
        let ib = &items[b];
        ia.done.cmp(&ib.done)
            .then(ia.bucket.cmp(&ib.bucket))
            .then(due_sort_key(ia).cmp(&due_sort_key(ib)))
            .then(ib.updated_at.unwrap_or(i64::MIN).cmp(&ia.updated_at.unwrap_or(i64::MIN)))
            .then(ia.priority.cmp(&ib.priority))
            .then(a.cmp(&b))
    });
    order
}

fn due_sort_key(it: &RemoteItem) -> (u8, String) {
    match it.due.as_deref().and_then(adapters::parse_date) {
        Some(d) => (0, d.format("%Y-%m-%d").to_string()),
        None => (1, String::new()),
    }
}

pub fn remote_empty_hint_text(items_empty: bool, syncing: bool, last_sync: &str, last_ok: bool, last_message: &str) -> Option<String> {
    if !items_empty { return None; }
    if syncing { return Some("正在同步第三方待办…".into()); }
    if last_sync.trim().is_empty() {
        Some("还没有同步过。请到设置 → 待办与置物 → 第三方里保存并同步。".into())
    } else if !last_ok {
        let msg = last_message.trim();
        if msg.is_empty() {
            Some("最近一次同步失败，请到设置里查看原因。".into())
        } else {
            Some(format!("同步失败：{msg}"))
        }
    } else {
        Some("已同步，当前账号下没有未完成的指派事项。".into())
    }
}

fn now_stamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_source_has_chinese_guidance() {
        for s in Source::ALL {
            assert!(!s.label().is_empty());
            assert!(!s.address_hint().is_empty());
            assert!(!s.token_hint().is_empty());
            assert_eq!(Source::ALL.iter().filter(|x| x.key() == s.key()).count(), 1);
        }
    }

    #[test]
    fn token_slots_differ_by_source_and_address() {
        let a = token_slot(Source::Jira, "https://A.example");
        let b = token_slot(Source::Jira, "https://a.example");
        let c = token_slot(Source::GitHub, "https://a.example");
        assert_eq!(a, b, "地址大小写不应产生不同槽位");
        assert_ne!(a, c);
    }

    #[test]
    fn jira_http_address_is_saved_and_reloaded() {
        let dir = std::env::temp_dir().join(format!(
            "daozai-jira-http-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("todo-sync.json");
        let mut c = Config::default();
        c.source = Source::Jira;
        c.address = "http://jira.example.invalid".into();
        c.enabled = true;
        let text = serde_json::to_string_pretty(&c).unwrap();
        assert!(text.contains("http://jira.example.invalid"), "保存内容应保留 http 地址");
        std::fs::write(&path, &text).unwrap();
        let loaded: Config = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(loaded.address, "http://jira.example.invalid");
        assert_eq!(loaded.source, Source::Jira);
        assert!(loaded.enabled);
        assert!(adapters::validate_jira_base(&loaded.address).is_ok());
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn remote_empty_hints_distinguish_not_run_failed_and_empty_success() {
        let h = remote_empty_hint_text(true, false, "", false, "").unwrap();
        assert!(h.contains("还没有同步过"), "{h}");
        let h = remote_empty_hint_text(true, false, "2026-11-13 16:59", false, "Jira：The requested API has been removed.").unwrap();
        assert!(h.contains("同步失败"), "{h}");
        assert!(h.contains("The requested API has been removed."), "{h}");
        assert!(!h.starts_with("已同步"), "{h}");
        let h = remote_empty_hint_text(true, false, "2026-11-13 16:59", true, "已同步 · 到期 0 · 即将 0 · 未排期 0 · 其他 0").unwrap();
        assert!(h.contains("已同步"), "{h}");
        assert!(h.contains("没有未完成的指派事项"), "{h}");
        assert!(remote_empty_hint_text(false, false, "2026-11-13 16:59", true, "已同步").is_none());
        assert_eq!(remote_empty_hint_text(true, true, "", false, "").as_deref(), Some("正在同步第三方待办…"));
    }

    fn item(key: &str, bucket: Bucket, done: bool) -> RemoteItem {
        RemoteItem {
            key: key.into(), summary: key.into(), due: None, status: None, url: None,
            priority: 2, ext_id: None, list_id: None, source: Source::Jira, bucket, done,
            updated_at: None,
        }
    }

    #[test]
    fn sticky_nudge_stacks_new_items_and_survives_until_ack() {
        let due = item("UL-1", Bucket::Due, false);
        let soon = item("UL-2", Bucket::Soon, false);
        let later = item("UL-3", Bucket::Later, false);
        let items = [due.clone(), soon.clone(), later.clone()];
        let pending = pending_nudge_items(&items, &[]);
        assert_eq!(pending.iter().map(|i| i.key.as_str()).collect::<Vec<_>>(), ["UL-1", "UL-2"]);
        // 确认即将档后，到期事项仍在；已确认的即将不再出现。
        let acked = vec![("jira:UL-2".into(), 0u8)];
        let items = [due.clone(), soon.clone()];
        let pending = pending_nudge_items(&items, &acked);
        assert_eq!(pending.iter().map(|i| i.key.as_str()).collect::<Vec<_>>(), ["UL-1"]);
        // 新的到期事项会叠加上去。
        let due2 = item("UL-9", Bucket::Due, false);
        let items = [due.clone(), soon.clone(), due2.clone()];
        let pending = pending_nudge_items(&items, &acked);
        let keys: Vec<_> = pending.iter().map(|i| i.key.as_str()).collect();
        assert_eq!(keys, ["UL-1", "UL-9"]);
        // 即将事项升到到期后再次提醒。
        let mut promoted = soon.clone();
        promoted.bucket = Bucket::Due;
        let items = [promoted.clone()];
        let pending = pending_nudge_items(&items, &acked);
        assert_eq!(pending[0].key, "UL-2");
        // 全部确认后清空。
        let all = vec![("jira:UL-1".into(), 1u8), ("jira:UL-2".into(), 1u8), ("jira:UL-9".into(), 1u8)];
        assert!(pending_nudge_items(&[due, promoted, due2], &all).is_empty());
        assert!(pending_nudge_items(&[later], &[]).is_empty(), "「其他」不进入粘性提醒");
    }

    #[test]
    fn display_items_sort_due_near_to_far_and_unscheduled_by_updated() {
        fn with(key: &str, bucket: Bucket, due: Option<&str>, updated: Option<i64>, done: bool) -> RemoteItem {
            let mut it = item(key, bucket, done);
            it.due = due.map(|s| s.into());
            it.updated_at = updated;
            it
        }
        let items = vec![
            with("L-late", Bucket::Later, Some("2027-02-01"), None, false),
            with("U-old", Bucket::Unscheduled, None, Some(100), false),
            with("D-late", Bucket::Due, Some("2026-11-10"), None, false),
            with("U-none", Bucket::Unscheduled, None, None, false),
            with("S-late", Bucket::Soon, Some("2026-11-20"), None, false),
            with("D-done", Bucket::Due, Some("2026-10-01"), None, true),
            with("L-early", Bucket::Later, Some("2026-12-01"), None, false),
            with("D-early", Bucket::Due, Some("2026-11-01"), None, false),
            with("U-new", Bucket::Unscheduled, None, Some(200), false),
            with("S-early", Bucket::Soon, Some("2026-11-16"), None, false),
        ];
        let keys: Vec<_> = display_item_order(&items).into_iter().map(|i| items[i].key.as_str()).collect();
        assert_eq!(keys, ["D-early", "D-late", "S-early", "S-late", "L-early", "L-late", "U-new", "U-old", "U-none", "D-done"]);
    }
}
