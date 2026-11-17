//! 全局快捷键。空绑定不注册；某一条被系统占用时只标记这一条。
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub const COUNT: usize = 6;
pub const LABELS: [&str; COUNT] = ["播放 / 暂停", "上一首", "下一首", "快退 5 秒", "快进 5 秒", "唤出或隐藏"];
const ID_BASE: i32 = 0x4E60;

static PRESSED: AtomicU32 = AtomicU32::new(0);
static HIDDEN: AtomicBool = AtomicBool::new(false);
static SHOW: AtomicBool = AtomicBool::new(false);
static FAILED: AtomicU32 = AtomicU32::new(0);
static APPLIED: std::sync::Mutex<Option<(isize, [String; COUNT])>> = std::sync::Mutex::new(None);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Toggle,
    Prev,
    Next,
    Back,
    Forward,
    Island,
}

impl Action {
    pub fn from_index(index: u32) -> Option<Self> {
        Some(match index {
            0 => Self::Toggle,
            1 => Self::Prev,
            2 => Self::Next,
            3 => Self::Back,
            4 => Self::Forward,
            5 => Self::Island,
            _ => return None,
        })
    }
}

pub fn press(id: u32) {
    let index = id.wrapping_sub(ID_BASE as u32);
    if index < COUNT as u32 { PRESSED.fetch_or(1 << index, Ordering::Relaxed); }
}

pub fn poll() -> Option<Action> {
    let bits = PRESSED.swap(0, Ordering::Relaxed);
    let index = bits.trailing_zeros();
    if index >= COUNT as u32 { return None; }
    let rest = bits & !(1 << index);
    if rest != 0 { PRESSED.fetch_or(rest, Ordering::Relaxed); }
    Action::from_index(index)
}

pub fn hidden() -> bool { HIDDEN.load(Ordering::Relaxed) }

pub fn toggle_hidden() {
    let hide = !HIDDEN.load(Ordering::Relaxed);
    HIDDEN.store(hide, Ordering::Relaxed);
    if !hide { SHOW.store(true, Ordering::Relaxed); }
}

/// 托盘点击：取消热键隐藏，并请求主循环把窗口显示出来。
pub fn release_hidden() {
    if HIDDEN.swap(false, Ordering::Relaxed) { SHOW.store(true, Ordering::Relaxed); }
}

pub fn take_show() -> bool { SHOW.swap(false, Ordering::Relaxed) }

pub fn failed(index: usize) -> bool {
    index < COUNT && FAILED.load(Ordering::Relaxed) & (1 << index) != 0
}

/// 把「Ctrl+Alt+P」解析成 (修饰键, 虚拟键)。空字符串表示不绑定。
pub fn parse(chord: &str) -> Result<Option<(u32, u32)>, ()> {
    let chord = chord.trim();
    if chord.is_empty() { return Ok(None); }
    let mut mods = 0u32;
    let mut key = None;
    for part in chord.split('+').map(str::trim).filter(|part| !part.is_empty()) {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods |= 0x0002,
            "alt" => mods |= 0x0001,
            "shift" => mods |= 0x0004,
            "win" | "windows" => mods |= 0x0008,
            other => {
                if key.is_some() { return Err(()); }
                key = Some(virtual_key(other).ok_or(())?);
            }
        }
    }
    match key {
        Some(vk) if mods != 0 => Ok(Some((mods, vk))),
        _ => Err(()),
    }
}

fn virtual_key(name: &str) -> Option<u32> {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() == 1 {
        let ch = chars[0].to_ascii_uppercase();
        if ch.is_ascii_alphanumeric() { return Some(ch as u32); }
    }
    Some(match name {
        "space" => 0x20,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        "tab" => 0x09,
        "enter" => 0x0D,
        _ => return None,
    })
}

/// 先注销全部编号，再注册非空组合。`register` 返回 false 表示这一条被占用。
pub fn install(chords: &[String; COUNT], mut register: impl FnMut(i32, u32, u32) -> bool) -> [bool; COUNT] {
    let mut failed = [false; COUNT];
    let mut bits = 0u32;
    for index in 0..COUNT {
        let _ = register(-(ID_BASE + index as i32), 0, 0);
        match parse(&chords[index]) {
            Ok(None) => {}
            Ok(Some((mods, vk))) => {
                if !register(ID_BASE + index as i32, mods | 0x4000, vk) {
                    failed[index] = true;
                    bits |= 1 << index;
                }
            }
            Err(()) => { failed[index] = true; bits |= 1 << index; }
        }
    }
    FAILED.store(bits, Ordering::Relaxed);
    failed
}

#[cfg(windows)]
pub fn install_native(hwnd: isize, chords: &[String; COUNT]) -> [bool; COUNT] {
    if let Ok(mut applied)=APPLIED.lock() {
        if applied.as_ref().is_some_and(|(window, saved)| *window==hwnd && saved==chords) {
            return std::array::from_fn(failed);
        }
        *applied=Some((hwnd, chords.clone()));
    }
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::Input::KeyboardAndMouse::{HOT_KEY_MODIFIERS, RegisterHotKey, UnregisterHotKey};
    install(chords, |id, mods, vk| {
        let window = HWND(hwnd as *mut _);
        if id < 0 {
            let _ = unsafe { UnregisterHotKey(window, -id) };
            true
        } else {
            unsafe { RegisterHotKey(window, id, HOT_KEY_MODIFIERS(mods), vk) }.is_ok()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_parse_and_empty_means_unbound() {
        assert_eq!(parse("").unwrap(), None);
        assert_eq!(parse("Ctrl+Alt+P").unwrap(), Some((0x0002 | 0x0001, b'P' as u32)));
        assert_eq!(parse("Alt+Left").unwrap(), Some((0x0001, 0x25)));
        assert!(parse("P").is_err(), "单独一个主键不算组合键");
        assert!(parse("Ctrl+Alt").is_err());
    }

    #[test]
    fn one_occupied_chord_does_not_cancel_the_others_and_empty_is_not_registered() {
        let mut chords = [String::new(), String::new(), String::new(), String::new(), String::new(), String::new()];
        let failed = install(&chords, |id, _mods, _vk| { assert!(id < 0, "默认全空只注销，不注册"); true });
        assert_eq!(failed, [false; 6]);
        chords[0] = "Ctrl+Alt+P".into();
        chords[1] = "Ctrl+Alt+Left".into();
        chords[3] = "Alt+Left".into();
        let mut seen = Vec::new();
        let failed = install(&chords, |id, mods, vk| {
            if id < 0 { return true; }
            seen.push((id, mods, vk));
            id != ID_BASE + 1
        });
        assert!(failed[1], "被占用的那一条单独失败");
        assert!(!failed[0] && !failed[3], "其余绑定仍然生效");
        assert!(seen.iter().any(|(id, _, _)| *id == ID_BASE));
        assert!(seen.iter().any(|(id, _, _)| *id == ID_BASE + 3));
        assert!(!failed[2] && !failed[4] && !failed[5], "空绑定不算失败");
    }
}
