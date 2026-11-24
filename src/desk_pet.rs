//! 岛外桌宠。画面来自 MIT 许可的 7paws 场景，窗口和设置都属于云屿。
//!
//! 开关关闭时不创建窗口。打开后是一扇独立的透明置顶窗：贴在岛的内侧，或自由放在桌面上。

use crate::model::Dock;

/// 屏幕上的物理像素矩形，右下角不包含在内。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Px {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// 这一帧岛的位置。`show` 为假时只藏窗口，不销毁。
#[derive(Clone, Copy, Debug)]
pub struct Anchor {
    pub origin: (i32, i32),
    pub ppp: f32,
    pub dock: Dock,
    pub canvas: (f32, f32),
    pub pill: (f32, f32),
    pub edge_gap: f32,
    pub monitor: Px,
    pub island: isize,
    pub show: bool,
}

#[derive(Clone, Debug)]
pub struct Prefs {
    pub enabled: bool,
    pub free: bool,
    pub scale: u32,
    pub opacity: u32,
    pub coat: String,
    pub layout_104: bool,
    pub labels: bool,
    pub privacy: bool,
    pub left_mouse: bool,
    pub sleep: bool,
    pub sleep_mins: u32,
    pub purr: bool,
    pub purr_sleep: bool,
    /// 打得太快、乱按、摸太久或被吵醒时生气，并用尾巴捶桌。
    pub annoy: bool,
    pub click_through: bool,
    pub x: Option<i32>,
    pub y: Option<i32>,
}

pub const COATS: [(&str, &str); 6] = [
    ("tuxedo", "奶牛"),
    ("orange", "橘猫"),
    ("tabby", "虎斑"),
    ("calico", "三花"),
    ("white", "白猫"),
    ("blue", "蓝猫"),
];

impl Default for Prefs {
    fn default() -> Self {
        Self {
            enabled: false,
            free: false,
            scale: 40,
            opacity: 100,
            coat: "tuxedo".into(),
            layout_104: false,
            labels: true,
            privacy: false,
            left_mouse: false,
            sleep: true,
            sleep_mins: 5,
            purr: true,
            purr_sleep: false,
            annoy: true,
            click_through: false,
            x: None,
            y: None,
        }
    }
}

impl Prefs {
    pub fn normalize(&mut self) {
        self.scale = snap_percent(self.scale);
        self.opacity = self.opacity.clamp(10, 100);
        self.coat = normalize_coat(&self.coat).to_string();
        self.sleep_mins = self.sleep_mins.clamp(1, 120);
    }

    pub fn apply_kv(&mut self, key: &str, value: &str) -> bool {
        match key {
            "desk_pet" => self.enabled = value != "0",
            "desk_pet_free" => self.free = value == "1",
            "desk_pet_scale" => self.scale = value.parse().unwrap_or(40),
            "desk_pet_opacity" => self.opacity = value.parse().unwrap_or(100),
            "desk_pet_coat" => self.coat = value.to_string(),
            "desk_pet_layout" => self.layout_104 = value == "104",
            "desk_pet_labels" => self.labels = value != "0",
            "desk_pet_privacy" => self.privacy = value == "1",
            "desk_pet_left_mouse" => self.left_mouse = value == "1",
            "desk_pet_sleep" => self.sleep = value != "0",
            "desk_pet_sleep_mins" => self.sleep_mins = value.parse().unwrap_or(5),
            "desk_pet_purr" => self.purr = value != "0",
            "desk_pet_purr_sleep" => self.purr_sleep = value == "1",
            "desk_pet_annoy" => self.annoy = value != "0",
            "desk_pet_click_through" => self.click_through = value == "1",
            "desk_pet_x" => self.x = value.parse().ok(),
            "desk_pet_y" => self.y = value.parse().ok(),
            _ => return false,
        }
        true
    }

    pub fn kv(&self) -> [(&'static str, String); 17] {
        let on = |value: bool| if value { "1".to_string() } else { "0".to_string() };
        [
            ("desk_pet", on(self.enabled)),
            ("desk_pet_free", on(self.free)),
            ("desk_pet_scale", snap_percent(self.scale).to_string()),
            ("desk_pet_opacity", self.opacity.clamp(10, 100).to_string()),
            ("desk_pet_coat", normalize_coat(&self.coat).to_string()),
            ("desk_pet_layout", if self.layout_104 { "104".into() } else { "65".into() }),
            ("desk_pet_labels", on(self.labels)),
            ("desk_pet_privacy", on(self.privacy)),
            ("desk_pet_left_mouse", on(self.left_mouse)),
            ("desk_pet_sleep", on(self.sleep)),
            ("desk_pet_sleep_mins", self.sleep_mins.clamp(1, 120).to_string()),
            ("desk_pet_purr", on(self.purr)),
            ("desk_pet_purr_sleep", on(self.purr_sleep)),
            ("desk_pet_annoy", on(self.annoy)),
            ("desk_pet_click_through", on(self.click_through)),
            ("desk_pet_x", self.x.map(|v| v.to_string()).unwrap_or_default()),
            ("desk_pet_y", self.y.map(|v| v.to_string()).unwrap_or_default()),
        ]
    }
}

pub fn snap_percent(value: u32) -> u32 {
    let value = value.clamp(10, 150);
    (((value + 2) / 5) * 5).clamp(10, 150)
}

pub fn coat_index(id: &str) -> usize {
    COATS.iter().position(|(coat, _)| *coat == id).unwrap_or(0)
}

pub fn normalize_coat(id: &str) -> &'static str {
    COATS[coat_index(id)].0
}

/// 逻辑像素。40% 的 65% 键盘是 264×192，用来贴在 30 像素高的药丸旁边。
pub fn logical_size(layout_104: bool, percent: u32) -> (f32, f32) {
    let scale = snap_percent(percent) as f32 / 100.0;
    let (width, height) = if layout_104 { (740.0, 520.0) } else { (660.0, 480.0) };
    (width * scale, height * scale)
}

pub fn pill_px(origin: (i32, i32), ppp: f32, dock: Dock, canvas: (f32, f32), size: (f32, f32), gap: f32) -> Px {
    let ppp = if ppp.is_finite() && ppp > 0.0 { ppp } else { 1.0 };
    let (canvas_w, canvas_h) = canvas;
    let (width, height) = size;
    let (x, y) = match dock {
        Dock::Left => (gap, (canvas_h - height) / 2.0),
        Dock::Right => (canvas_w - gap - width, (canvas_h - height) / 2.0),
        Dock::Top => ((canvas_w - width) / 2.0, gap),
    };
    let px = |value: f32| origin.0 + (value * ppp).round() as i32;
    let py = |value: f32| origin.1 + (value * ppp).round() as i32;
    Px { left: px(x), top: py(y), right: px(x + width), bottom: py(y + height) }
}

fn fits(x: i32, y: i32, size: (i32, i32), monitor: Px) -> bool {
    x >= monitor.left && y >= monitor.top && x.saturating_add(size.0) <= monitor.right && y.saturating_add(size.1) <= monitor.bottom
}

fn clamp_origin(x: i32, y: i32, size: (i32, i32), monitor: Px) -> (i32, i32) {
    let max_x = (monitor.right - size.0).max(monitor.left);
    let max_y = (monitor.bottom - size.1).max(monitor.top);
    (x.clamp(monitor.left, max_x), y.clamp(monitor.top, max_y))
}

/// 贴在岛的内侧。顶部停靠先放左侧；左右停靠放屏幕内侧。这一侧放不下就换到另一侧。
pub fn docked_origin(island: Px, size: (i32, i32), monitor: Px, gap: i32, prefer_right: bool) -> (i32, i32) {
    let beside = |right: bool| {
        let x = if right { island.right.saturating_add(gap) } else { island.left.saturating_sub(gap).saturating_sub(size.0) };
        (x, island.top)
    };
    let primary = beside(prefer_right);
    let secondary = beside(!prefer_right);
    if fits(primary.0, primary.1, size, monitor) { primary }
    else if fits(secondary.0, secondary.1, size, monitor) { secondary }
    else { clamp_origin(primary.0, primary.1, size, monitor) }
}

/// 算出这一帧窗口的物理位置。自由放置第一次出现时，把当时贴岛的位置记下来。
pub fn place(prefs: &mut Prefs, anchor: &Anchor) -> (i32, i32, i32, i32, bool) {
    let ppp = if anchor.ppp.is_finite() && anchor.ppp > 0.0 { anchor.ppp } else { 1.0 };
    let logical = logical_size(prefs.layout_104, prefs.scale);
    let size = ((logical.0 * ppp).round() as i32, (logical.1 * ppp).round() as i32);
    let island = pill_px(anchor.origin, ppp, anchor.dock, anchor.canvas, anchor.pill, anchor.edge_gap);
    let gap = (8.0 * ppp).round() as i32;
    let prefer_right = anchor.dock == Dock::Left;
    let docked = docked_origin(island, size, anchor.monitor, gap, prefer_right);
    let mut changed = false;
    let (x, y) = if prefs.free {
        if prefs.x.is_none() || prefs.y.is_none() {
            prefs.x = Some(docked.0);
            prefs.y = Some(docked.1);
            changed = true;
        }
        let clamped = clamp_origin(prefs.x.unwrap_or(docked.0), prefs.y.unwrap_or(docked.1), size, anchor.monitor);
        if prefs.x != Some(clamped.0) || prefs.y != Some(clamped.1) {
            prefs.x = Some(clamped.0);
            prefs.y = Some(clamped.1);
            changed = true;
        }
        clamped
    } else {
        docked
    };
    (x, y, size.0.max(1), size.1.max(1), changed)
}

/// 命中区消息。页面可能把 JSON 再包成一层字符串。
pub fn parse_regions(text: &str) -> Option<Vec<[f64; 4]>> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let value = match value {
        serde_json::Value::String(inner) => serde_json::from_str(&inner).ok()?,
        other => other,
    };
    if value.get("t").and_then(|item| item.as_str()) != Some("hit") { return None; }
    serde_json::from_value(value.get("r")?.clone()).ok()
}

/// 头以外的区域（身体、尾巴、键盘、鼠标）可以拖。头和身体重叠时仍算身体，避免只能点到键盘才拖得动。
pub fn can_drag(regions: &[[f64; 4]], x: f64, y: f64) -> bool {
    let inside = |region: &[f64; 4]| {
        x >= region[0] && y >= region[1] && x <= region[0] + region[2] && y <= region[1] + region[3]
    };
    regions.iter().skip(1).any(inside)
}

/// 扫描码对应的 DOM code，以及这一键算左手还是右手。扫不到的键仍让猫抬一下爪。
pub fn key_name(scan: u32, extended: bool) -> Option<(&'static str, bool)> {
    if scan == 0 { return None; }
    let (code, left) = match (extended, scan) {
        (false, 0x29) => ("Backquote", true),
        (false, 0x02) => ("Digit1", true),
        (false, 0x03) => ("Digit2", true),
        (false, 0x04) => ("Digit3", true),
        (false, 0x05) => ("Digit4", true),
        (false, 0x06) => ("Digit5", true),
        (false, 0x07) => ("Digit6", false),
        (false, 0x08) => ("Digit7", false),
        (false, 0x09) => ("Digit8", false),
        (false, 0x0A) => ("Digit9", false),
        (false, 0x0B) => ("Digit0", false),
        (false, 0x0C) => ("Minus", false),
        (false, 0x0D) => ("Equal", false),
        (false, 0x0E) => ("Backspace", false),
        (false, 0x0F) => ("Tab", true),
        (false, 0x10) => ("KeyQ", true),
        (false, 0x11) => ("KeyW", true),
        (false, 0x12) => ("KeyE", true),
        (false, 0x13) => ("KeyR", true),
        (false, 0x14) => ("KeyT", true),
        (false, 0x15) => ("KeyY", false),
        (false, 0x16) => ("KeyU", false),
        (false, 0x17) => ("KeyI", false),
        (false, 0x18) => ("KeyO", false),
        (false, 0x19) => ("KeyP", false),
        (false, 0x1A) => ("BracketLeft", false),
        (false, 0x1B) => ("BracketRight", false),
        (false, 0x2B) => ("Backslash", false),
        (false, 0x1C) => ("Enter", false),
        (true, 0x1C) => ("NumpadEnter", false),
        (false, 0x3A) => ("CapsLock", true),
        (false, 0x1E) => ("KeyA", true),
        (false, 0x1F) => ("KeyS", true),
        (false, 0x20) => ("KeyD", true),
        (false, 0x21) => ("KeyF", true),
        (false, 0x22) => ("KeyG", true),
        (false, 0x23) => ("KeyH", false),
        (false, 0x24) => ("KeyJ", false),
        (false, 0x25) => ("KeyK", false),
        (false, 0x26) => ("KeyL", false),
        (false, 0x27) => ("Semicolon", false),
        (false, 0x28) => ("Quote", false),
        (false, 0x2A) => ("ShiftLeft", true),
        (false, 0x2C) => ("KeyZ", true),
        (false, 0x2D) => ("KeyX", true),
        (false, 0x2E) => ("KeyC", true),
        (false, 0x2F) => ("KeyV", true),
        (false, 0x30) => ("KeyB", true),
        (false, 0x31) => ("KeyN", false),
        (false, 0x32) => ("KeyM", false),
        (false, 0x33) => ("Comma", false),
        (false, 0x34) => ("Period", false),
        (false, 0x35) => ("Slash", false),
        (true, 0x35) => ("NumpadDivide", false),
        (false, 0x36) => ("ShiftRight", false),
        (false, 0x1D) => ("ControlLeft", true),
        (true, 0x1D) => ("ControlRight", false),
        (false, 0x38) => ("AltLeft", true),
        (true, 0x38) => ("AltRight", false),
        (false, 0x39) => ("Space", false),
        (true, 0x5B) => ("MetaLeft", true),
        (true, 0x5C) => ("MetaRight", false),
        (false, 0x3B) => ("F1", true),
        (false, 0x3C) => ("F2", true),
        (false, 0x3D) => ("F3", true),
        (false, 0x3E) => ("F4", true),
        (false, 0x3F) => ("F5", true),
        (false, 0x40) => ("F6", false),
        (false, 0x41) => ("F7", false),
        (false, 0x42) => ("F8", false),
        (false, 0x43) => ("F9", false),
        (false, 0x44) => ("F10", false),
        (false, 0x57) => ("F11", false),
        (false, 0x58) => ("F12", false),
        (true, 0x48) => ("ArrowUp", false),
        (true, 0x50) => ("ArrowDown", false),
        (true, 0x4B) => ("ArrowLeft", false),
        (true, 0x4D) => ("ArrowRight", false),
        (true, 0x47) => ("Home", false),
        (true, 0x4F) => ("End", false),
        (true, 0x49) => ("PageUp", false),
        (true, 0x51) => ("PageDown", false),
        (true, 0x52) => ("Insert", false),
        (true, 0x53) => ("Delete", false),
        (false, 0x01) => ("Escape", true),
        _ => ("", false),
    };
    Some((code, left))
}

/// 同步桌宠窗口。返回真表示记住了新的自由位置，调用方应保存设置。
pub fn sync(prefs: &mut Prefs, anchor: Option<&Anchor>) -> bool {
    #[cfg(windows)]
    { host::sync(prefs, anchor) }
    #[cfg(not(windows))]
    { let _ = (prefs, anchor); false }
}

pub fn error() -> Option<String> {
    #[cfg(windows)]
    { host::error() }
    #[cfg(not(windows))]
    { None }
}

#[cfg(windows)]
#[path = "desk_pet_host.rs"]
mod host;

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor() -> Px { Px { left: 0, top: 0, right: 1920, bottom: 1080 } }
    fn anchor(dock: Dock) -> Anchor {
        Anchor { origin: (800, 0), ppp: 1.0, dock, canvas: (640.0, 270.0), pill: (163.0, 30.0), edge_gap: 6.0, monitor: monitor(), island: 0, show: true }
    }

    #[test]
    fn default_size_sits_beside_a_thirty_pixel_pill() {
        assert_eq!(logical_size(false, 40), (264.0, 192.0));
        assert_eq!(logical_size(true, 40), (296.0, 208.0));
        assert_eq!(snap_percent(41), 40);
        assert_eq!(snap_percent(43), 45);
        assert_eq!(normalize_coat("nope"), "tuxedo");
    }

    #[test]
    fn top_dock_puts_the_cat_on_the_left_and_flips_at_the_screen_edge() {
        let mut prefs = Prefs::default();
        let (x, y, w, h, changed) = place(&mut prefs, &anchor(Dock::Top));
        let island = pill_px((800, 0), 1.0, Dock::Top, (640.0, 270.0), (163.0, 30.0), 6.0);
        assert!(!changed && !prefs.free);
        assert_eq!((w, h), (264, 192));
        assert_eq!(x + w + 8, island.left);
        assert_eq!(y, island.top);
        let mut edge = anchor(Dock::Top);
        edge.origin = (0, 0);
        let (x, _, w, _, _) = place(&mut Prefs::default(), &edge);
        let island = pill_px((0, 0), 1.0, Dock::Top, (640.0, 270.0), (163.0, 30.0), 6.0);
        assert!(x >= island.right, "左侧放不下时应改到岛的右侧，实际 {x}");
        assert_eq!(x, island.right + 8);
        assert!(x + w <= 1920);
    }

    #[test]
    fn free_placement_pins_once_then_stays_put() {
        let mut prefs = Prefs { free: true, ..Prefs::default() };
        let first = place(&mut prefs, &anchor(Dock::Top));
        assert!(first.4 && prefs.x == Some(first.0) && prefs.y == Some(first.1));
        let mut moved = anchor(Dock::Top);
        moved.origin = (1200, 0);
        let second = place(&mut prefs, &moved);
        assert!(!second.4);
        assert_eq!((second.0, second.1), (first.0, first.1));
    }

    #[test]
    fn prefs_round_trip_unknown_keys_and_blank_position() {
        let mut prefs = Prefs::default();
        assert!(!prefs.apply_kv("pet", "1"));
        assert!(prefs.apply_kv("desk_pet", "1") && prefs.enabled);
        assert!(prefs.apply_kv("desk_pet_layout", "104") && prefs.layout_104);
        assert!(prefs.apply_kv("desk_pet_x", "") && prefs.x.is_none());
        prefs.scale = 43;
        prefs.coat = "nope".into();
        let saved = prefs.kv();
        let mut loaded = Prefs::default();
        for (key, value) in saved { assert!(loaded.apply_kv(key, &value)); }
        loaded.normalize();
        assert!(loaded.enabled && loaded.layout_104);
        assert_eq!(loaded.scale, 45);
        assert_eq!(loaded.coat, "tuxedo");
    }

    #[test]
    fn overlapping_head_still_drags_from_the_body_and_regions_parse() {
        let regions = [[0.39, 0.16, 0.22, 0.31], [0.37, 0.23, 0.26, 0.50], [0.33, 0.68, 0.43, 0.17]];
        assert!(can_drag(&regions, 0.45, 0.40), "头和身体重叠的胸口应能拖");
        assert!(can_drag(&regions, 0.40, 0.72), "键盘应能拖");
        assert!(!can_drag(&regions, 0.45, 0.18), "只有头的位置不拖");
        let parsed = parse_regions(r#"{"t":"hit","r":[[0.1,0.2,0.3,0.4]]}"#).expect("对象");
        assert_eq!(parsed, vec![[0.1, 0.2, 0.3, 0.4]]);
        let wrapped = parse_regions(r#""{\"t\":\"hit\",\"r\":[[0.1,0.2,0.3,0.4]]}""#).expect("字符串再包一层");
        assert_eq!(wrapped, parsed);
    }

    #[test]
    fn physical_keys_name_the_hand_without_recording_characters() {
        assert_eq!(key_name(0x1E, false), Some(("KeyA", true)));
        assert_eq!(key_name(0x19, false), Some(("KeyP", false)));
        assert_eq!(key_name(0x4B, true), Some(("ArrowLeft", false)));
        assert_eq!(key_name(0, false), None);
    }
}
