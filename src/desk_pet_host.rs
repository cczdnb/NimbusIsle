//! WebView2 透明窗口。7paws 的 three.js 场景只负责画猫，位置和穿透由这里管。

use std::cell::Cell;
use std::ffi::c_void;
use std::sync::Mutex;

use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{CoreWebView2EnvironmentOptions, CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler, NavigationCompletedEventHandler, WebMessageReceivedEventHandler};
use windows::core::{w, Interface, PCWSTR, PWSTR};
use windows::Win32::System::WinRT::EventRegistrationToken;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::DwmExtendFrameIntoClientArea;
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, HBRUSH, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, MapVirtualKeyW, MAPVK_VK_TO_VSC_EX, VK_LBUTTON, VK_MENU, VK_RBUTTON};
use windows::Win32::UI::WindowsAndMessaging::*;

use super::{place, Anchor, Prefs, Px};

struct Host {
    hwnd: HWND,
    controller: Option<ICoreWebView2Controller>,
    webview: Option<ICoreWebView2>,
    ready: bool,
    shown: bool,
    free: bool,
    click_through: bool,
    privacy: bool,
    dragging: bool,
    was_down: bool,
    drag: (i32, i32),
    cursor: (i32, i32),
    buttons: u8,
    ignore: bool,
    rect: (i32, i32, i32, i32),
    options: String,
    island: isize,
    moved: Option<(i32, i32)>,
    want_show: bool,
}

struct Keys {
    held: Vec<u32>,
    queue: Vec<Queued>,
    seeded: bool,
}
struct Queued {
    scan: u32,
    extended: bool,
    down: bool,
    repeat: bool,
}

static KEYS: Mutex<Keys> = Mutex::new(Keys { held: Vec::new(), queue: Vec::new(), seeded: false });
static REGIONS: Mutex<Vec<[f64; 4]>> = Mutex::new(Vec::new());
static ERROR: Mutex<String> = Mutex::new(String::new());

thread_local! {
    static HOST: Cell<*mut Host> = const { Cell::new(std::ptr::null_mut()) };
    static BUSY: Cell<bool> = const { Cell::new(false) };
    static FAILED: Cell<bool> = const { Cell::new(false) };
}

pub(super) fn error() -> Option<String> {
    let text = ERROR.lock().ok()?.clone();
    (!text.is_empty()).then_some(text)
}

pub(super) fn sync(prefs: &mut Prefs, anchor: Option<&Anchor>) -> bool {
    if !prefs.enabled {
        FAILED.with(|failed| failed.set(false));
        set_error("");
        destroy();
        return false;
    }
    let Some(anchor) = anchor.filter(|anchor| anchor.show) else {
        with_host(|host| host.hide());
        return false;
    };
    if host_ptr().is_null() {
        if FAILED.with(|failed| failed.get()) { return false; }
        if let Err(message) = create(anchor) {
            set_error(&message);
            FAILED.with(|failed| failed.set(true));
            return false;
        }
    }
    let mut changed = false;
    with_host(|host| {
        let (x, y, w, h, pinned) = place(prefs, anchor);
        changed = pinned;
        host.free = prefs.free;
        host.click_through = prefs.click_through;
        host.privacy = prefs.privacy;
        host.island = anchor.island;
        let options = options_json(prefs);
        if host.ready && options != host.options {
            if let Some(webview) = &host.webview { post(webview, &options); }
            host.options = options;
        }
        host.place_window(x, y, w, h, true);
        // 这扇窗口的 WM_TIMER 到不了自己的过程（样式从创建后就不再更新）。
        // 按键队列和拖动都放在岛的刷新里处理。
        host.tick();
        if let Some(pos) = host.moved.take() {
            prefs.x = Some(pos.0);
            prefs.y = Some(pos.1);
            changed = true;
        }
    });
    changed
}

fn options_json(prefs: &Prefs) -> String {
    serde_json::json!({
        "t": "opt",
        "coat": super::normalize_coat(&prefs.coat),
        "opacity": prefs.opacity.clamp(10, 100) as f32 / 100.0,
        "layout": if prefs.layout_104 { "104" } else { "65" },
        "sleep": prefs.sleep,
        "sleepMinutes": prefs.sleep_mins.clamp(1, 120),
        "labels": prefs.labels,
        "privacy": prefs.privacy,
        "leftMouse": prefs.left_mouse,
        "purrPet": prefs.purr,
        "purrSleep": prefs.purr_sleep,
        "annoy": prefs.annoy,
    }).to_string()
}

fn create(anchor: &Anchor) -> std::result::Result<(), String> {
    unsafe { let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED); }
    let class = w!("YunyuDeskPet");
    let instance = unsafe { GetModuleHandleW(None) }.map_err(|error| error.to_string())?;
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: instance.into(),
        lpszClassName: class,
        hbrBackground: HBRUSH::default(),
        ..Default::default()
    };
    unsafe {
        if RegisterClassW(&wc) == 0 {
            let code = GetLastError();
            if code != ERROR_CLASS_ALREADY_EXISTS { return Err(format!("桌宠窗口类注册失败（{code:?}）")); }
        }
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class, w!("云屿桌宠"), WS_POPUP,
            0, 0, 64, 64, None, None, instance, None,
        ).map_err(|error| error.to_string())?;
        let margins = MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 };
        let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
        let host = Box::new(Host {
            hwnd, controller: None, webview: None, ready: false, shown: false,
            free: false, click_through: false, privacy: false, dragging: false, was_down: false,
            drag: (0, 0), cursor: (0, 0), buttons: 0, ignore: false, rect: (0, 0, 64, 64),
            options: String::new(), island: anchor.island, moved: None, want_show: false,
        });
        let ptr = Box::into_raw(host);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as isize);
        HOST.with(|slot| slot.set(ptr));
        let _ = SetTimer(hwnd, 1, 33, None);
        let folder = match assets_dir() {
            Ok(path) => path,
            Err(error) => { destroy_hwnd(hwnd); return Err(error.to_string()); }
        };
        let data = crate::model::config_dir().join("webview2-deskpet");
        if let Err(error) = std::fs::create_dir_all(&data) {
            destroy_hwnd(hwnd);
            return Err(error.to_string());
        }
        let folder_w = wide(&folder.to_string_lossy());
        let data_w = wide(&data.to_string_lossy());
        if let Err(error) = start_environment(hwnd, folder_w, data_w) {
            destroy_hwnd(hwnd);
            return Err(error.to_string());
        }
    }
    Ok(())
}

fn start_environment(hwnd: HWND, folder: Vec<u16>, data: Vec<u16>) -> windows::core::Result<()> {
    let options = CoreWebView2EnvironmentOptions::default();
    unsafe { options.set_additional_browser_arguments("--autoplay-policy=no-user-gesture-required".into()); }
    let options: ICoreWebView2EnvironmentOptions = options.into();
    unsafe {
        CreateCoreWebView2EnvironmentWithOptions(
            PCWSTR::null(),
            PCWSTR(data.as_ptr()),
            &options,
            &CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(move |hr, environment| {
                if hr.is_err() {
                    fail(hwnd, "没有找到 WebView2 运行时，桌宠无法显示。");
                    return Ok(());
                }
                let Some(environment) = environment else { return Ok(()) };
                environment.CreateCoreWebView2Controller(hwnd, &CreateCoreWebView2ControllerCompletedHandler::create(Box::new(move |hr, controller| {
                    if hr.is_err() {
                        fail(hwnd, "桌宠画面没有创建出来。");
                        return Ok(());
                    }
                    if let Some(controller) = controller { let _ = attach(hwnd, controller, &folder); }
                    Ok(())
                })))?;
                Ok(())
            })),
        )
    }
}

fn attach(hwnd: HWND, controller: ICoreWebView2Controller, folder: &[u16]) -> windows::core::Result<()> {
    unsafe {
        if let Ok(controller2) = controller.cast::<ICoreWebView2Controller2>() {
            let _ = controller2.SetDefaultBackgroundColor(COREWEBVIEW2_COLOR { A: 0, R: 0, G: 0, B: 0 });
        }
        let webview = controller.CoreWebView2()?;
        if let Ok(settings) = webview.Settings() {
            let _ = settings.SetAreDefaultContextMenusEnabled(false);
            let _ = settings.SetIsStatusBarEnabled(false);
            let _ = settings.SetAreDevToolsEnabled(false);
            let _ = settings.SetIsZoomControlEnabled(false);
        }
        if let Ok(webview3) = webview.cast::<ICoreWebView2_3>() {
            let _ = webview3.SetVirtualHostNameToFolderMapping(
                w!("deskpet.yunyu"),
                PCWSTR(folder.as_ptr()),
                COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_ALLOW,
            );
        }
        let mut token = EventRegistrationToken::default();
        let _ = webview.add_WebMessageReceived(&WebMessageReceivedEventHandler::create(Box::new(|_, args| {
            let Some(args) = args else { return Ok(()) };
            if let Some(text) = message_text(&args) { store_regions(&text); }
            Ok(())
        })), &mut token);
        let _ = webview.add_NavigationCompleted(&NavigationCompletedEventHandler::create(Box::new(move |_, _| {
            with_host(|host| {
                if host.hwnd == hwnd {
                    host.ready = true;
                    if host.want_show { host.show(); }
                }
            });
            Ok(())
        })), &mut token);
        webview.Navigate(w!("https://deskpet.yunyu/index.html"))?;
        controller.SetIsVisible(true)?;
        if let Some((_, _, width, height)) = with_host(|host| host.rect) {
            let _ = controller.SetBounds(RECT { left: 0, top: 0, right: width, bottom: height });
        }
        with_host(|host| {
            if host.hwnd == hwnd {
                host.controller = Some(controller);
                host.webview = Some(webview);
            }
        });
    }
    Ok(())
}

fn message_text(args: &ICoreWebView2WebMessageReceivedEventArgs) -> Option<String> {
    unsafe {
        let mut raw = PWSTR::null();
        // 页面发的是对象。TryGetWebMessageAsString 只接受字符串，对象会直接失败，命中区就一直是空的。
        args.WebMessageAsJson(&mut raw).ok()?;
        if raw.is_null() { return None; }
        let mut len = 0;
        while *raw.0.add(len) != 0 { len += 1; }
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(raw.0, len));
        CoTaskMemFree(Some(raw.0 as *const c_void));
        Some(text)
    }
}

fn store_regions(text: &str) {
    let Some(regions) = super::parse_regions(text) else { return };
    if let Ok(mut slot) = REGIONS.lock() { *slot = regions; }
}

fn fail(hwnd: HWND, message: &str) {
    set_error(message);
    FAILED.with(|failed| failed.set(true));
    destroy_hwnd(hwnd);
}

fn destroy() {
    let ptr = host_ptr();
    if ptr.is_null() { return; }
    unsafe { if let Some(host) = ptr.as_ref() { destroy_hwnd(host.hwnd); } }
}

fn destroy_hwnd(hwnd: HWND) {
    if let Ok(mut keys) = KEYS.lock() {
        keys.held.clear();
        keys.queue.clear();
        keys.seeded = false;
    }
    unsafe {
        let _ = KillTimer(hwnd, 1);
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Host;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        HOST.with(|slot| if slot.get() == ptr { slot.set(std::ptr::null_mut()) });
        if let Some(host) = ptr.as_mut() {
            if let Some(controller) = host.controller.take() { let _ = controller.Close(); }
            host.webview = None;
        }
        let _ = DestroyWindow(hwnd);
        if !ptr.is_null() { drop(Box::from_raw(ptr)); }
    }
    if let Ok(mut regions) = REGIONS.lock() { regions.clear(); }
}

impl Host {
    fn hide(&mut self) {
        if self.shown {
            unsafe { let _ = ShowWindow(self.hwnd, SW_HIDE); }
            self.shown = false;
        }
    }

    fn show(&mut self) {
        if !self.shown {
            unsafe { let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE); }
            self.shown = true;
            self.raise_island();
        }
    }

    fn place_window(&mut self, x: i32, y: i32, w: i32, h: i32, show: bool) {
        self.want_show = show;
        if self.dragging { return; }
        let next = (x, y, w, h);
        if next != self.rect || (show && !self.shown && self.ready) {
            unsafe {
                let _ = SetWindowPos(self.hwnd, HWND_TOPMOST, x, y, w, h, SWP_NOACTIVATE | if show && self.ready { SWP_SHOWWINDOW } else { SWP_NOACTIVATE });
                if let Some(controller) = &self.controller {
                    let _ = controller.SetBounds(RECT { left: 0, top: 0, right: w, bottom: h });
                    let _ = controller.NotifyParentWindowPositionChanged();
                }
            }
            self.rect = next;
            if show && self.ready { self.shown = true; self.raise_island(); }
        }
    }

    fn raise_island(&self) {
        if self.island == 0 { return; }
        unsafe {
            let _ = SetWindowPos(HWND(self.island as *mut c_void), HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    fn tick(&mut self) {
        let cursor = cursor_pos();
        let down = unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) < 0 };
        let alt = unsafe { GetAsyncKeyState(VK_MENU.0 as i32) < 0 };
        let right = unsafe { GetAsyncKeyState(VK_RBUTTON.0 as i32) < 0 };
        let over = hit(cursor, self.hwnd, None);
        let body = body_hit(cursor, self.hwnd);
        let locked = self.click_through && !alt;
        if down && !self.was_down && self.free && body && !locked {
            self.dragging = true;
            if let Some(rect) = window_rect(self.hwnd) {
                self.drag = (cursor.0 - rect.left, cursor.1 - rect.top);
            }
        }
        if self.dragging && down {
            let x = cursor.0 - self.drag.0;
            let y = cursor.1 - self.drag.1;
            let size = (self.rect.2, self.rect.3);
            let monitor = monitor_at(cursor);
            let (x, y) = clamp_to(x, y, size, monitor);
            unsafe { let _ = SetWindowPos(self.hwnd, HWND_TOPMOST, x, y, size.0, size.1, SWP_NOACTIVATE); }
            self.rect.0 = x;
            self.rect.1 = y;
        }
        if !down && self.dragging {
            self.dragging = false;
            self.moved = Some((self.rect.0, self.rect.1));
        }
        self.was_down = down;
        let ignore = !self.dragging && (locked || !over);
        if ignore != self.ignore {
            unsafe {
                let ex = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
                let transparent = WS_EX_TRANSPARENT.0 as isize;
                let next = if ignore { ex | transparent } else { ex & !transparent };
                if next != ex { SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, next); }
            }
            self.ignore = ignore;
        }
        let buttons = u8::from(down) | (u8::from(right) << 1);
        let moved = cursor != self.cursor;
        if (moved || buttons != self.buttons) && self.ready {
            if let Some(webview) = &self.webview {
                let (nx, ny) = normalize(cursor, monitor_at(cursor));
                let dx = ((cursor.0 - self.cursor.0) as f64 * 0.004).clamp(-0.16, 0.16);
                let dy = ((cursor.1 - self.cursor.1) as f64 * 0.004).clamp(-0.12, 0.12);
                post(webview, &serde_json::json!({"t":"mouse","nx":nx,"ny":ny,"dx":dx,"dy":dy,"btn":buttons}).to_string());
            }
        }
        self.cursor = cursor;
        self.buttons = buttons;
        self.flush_keys();
    }

    fn flush_keys(&self) {
        poll_keys();
        let Some(webview) = &self.webview else {
            if let Ok(mut keys) = KEYS.lock() { keys.queue.clear(); }
            return;
        };
        let queue = KEYS.lock().ok().map(|mut keys| std::mem::take(&mut keys.queue)).unwrap_or_default();
        for key in queue {
            let named = super::key_name(key.scan, key.extended);
            let (code, left) = named.unwrap_or(("", false));
            post(webview, &serde_json::json!({
                "t": "key",
                "code": code,
                "down": key.down,
                "repeat": key.repeat,
                "side": if left { "L" } else { "R" },
            }).to_string());
        }
    }
}

fn pointer_in(cursor: (i32, i32), hwnd: HWND) -> Option<(f64, f64, Vec<[f64; 4]>)> {
    let rect = window_rect(hwnd)?;
    let width = (rect.right - rect.left).max(1) as f64;
    let height = (rect.bottom - rect.top).max(1) as f64;
    let regions = REGIONS.lock().ok()?.clone();
    if regions.is_empty() { return None; }
    Some((((cursor.0 - rect.left) as f64 / width), ((cursor.1 - rect.top) as f64 / height), regions))
}

fn hit(cursor: (i32, i32), hwnd: HWND, only: Option<usize>) -> bool {
    let Some((x, y, regions)) = pointer_in(cursor, hwnd) else { return false };
    let inside = |region: &[f64; 4]| x >= region[0] && y >= region[1] && x <= region[0] + region[2] && y <= region[1] + region[3];
    match only {
        Some(index) => regions.get(index).is_some_and(inside),
        None => regions.iter().any(inside),
    }
}

fn body_hit(cursor: (i32, i32), hwnd: HWND) -> bool {
    pointer_in(cursor, hwnd).is_some_and(|(x, y, regions)| super::can_drag(&regions, x, y))
}

fn cursor_pos() -> (i32, i32) {
    let mut point = POINT::default();
    unsafe { let _ = GetCursorPos(&mut point); }
    (point.x, point.y)
}

fn window_rect(hwnd: HWND) -> Option<RECT> {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect).ok()? };
    Some(rect)
}

fn monitor_at(cursor: (i32, i32)) -> Px {
    unsafe {
        let monitor = MonitorFromPoint(POINT { x: cursor.0, y: cursor.1 }, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..MONITORINFO::default() };
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            return Px { left: info.rcMonitor.left, top: info.rcMonitor.top, right: info.rcMonitor.right, bottom: info.rcMonitor.bottom };
        }
    }
    Px { left: 0, top: 0, right: 1920, bottom: 1080 }
}

fn normalize(cursor: (i32, i32), monitor: Px) -> (f64, f64) {
    let width = (monitor.right - monitor.left).max(1) as f64;
    let height = (monitor.bottom - monitor.top).max(1) as f64;
    (((cursor.0 - monitor.left) as f64 / width).clamp(0.0, 1.0), ((cursor.1 - monitor.top) as f64 / height).clamp(0.0, 1.0))
}

fn clamp_to(x: i32, y: i32, size: (i32, i32), monitor: Px) -> (i32, i32) {
    let max_x = (monitor.right - size.0).max(monitor.left);
    let max_y = (monitor.bottom - size.1).max(monitor.top);
    (x.clamp(monitor.left, max_x), y.clamp(monitor.top, max_y))
}

fn post(webview: &ICoreWebView2, json: &str) {
    let wide = wide(json);
    unsafe { let _ = webview.PostWebMessageAsJson(PCWSTR(wide.as_ptr())); }
}

fn wide(text: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(text).encode_wide().chain(std::iter::once(0)).collect()
}

fn assets_dir() -> std::io::Result<std::path::PathBuf> {
    let dir = crate::model::config_dir().join("deskpet");
    std::fs::create_dir_all(&dir)?;
    write_same(&dir.join("three.min.js"), include_bytes!("../assets/deskpet/three.min.js"))?;
    write_same(&dir.join("7paws.js"), include_bytes!("../assets/deskpet/7paws.js"))?;
    write_same(&dir.join("host.js"), include_bytes!("../assets/deskpet/host.js"))?;
    write_same(&dir.join("index.html"), include_bytes!("../assets/deskpet/index.html"))?;
    write_same(&dir.join("LICENSE-7paws.txt"), include_bytes!("../assets/deskpet/LICENSE-7paws.txt"))?;
    Ok(dir)
}

fn write_same(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    if std::fs::read(path).ok().as_deref() != Some(bytes) { std::fs::write(path, bytes)?; }
    Ok(())
}

fn set_error(message: &str) {
    if let Ok(mut error) = ERROR.lock() { *error = message.to_string(); }
}

fn host_ptr() -> *mut Host {
    HOST.with(|slot| slot.get())
}

fn with_host<T>(f: impl FnOnce(&mut Host) -> T) -> Option<T> {
    if BUSY.with(|busy| busy.get()) { return None; }
    let ptr = host_ptr();
    if ptr.is_null() { return None; }
    BUSY.with(|busy| busy.set(true));
    let value = unsafe { f(&mut *ptr) };
    BUSY.with(|busy| busy.set(false));
    Some(value)
}

/// 全局键盘钩子挂在界面线程上，这一帧一旦卡住超过系统时限，钩子会被卸掉，之后按键全部丢失。
/// 每帧直接看按键状态，按下和松开各记一次。
fn poll_keys() {
    let mut now = Vec::new();
    let mut consider = |vk: u16| {
        if unsafe { GetAsyncKeyState(vk as i32) } < 0 { now.push(u32::from(vk)); }
    };
    for vk in [0x08, 0x09, 0x0D, 0x14, 0x1B, 0x20] { consider(vk); }
    for vk in 0x21..=0x28 { consider(vk); }
    for vk in 0x2D..=0x2E { consider(vk); }
    for vk in 0x30..=0x39 { consider(vk); }
    for vk in 0x41..=0x5A { consider(vk); }
    for vk in 0x5B..=0x5C { consider(vk); }
    for vk in 0x60..=0x6F { consider(vk); }
    for vk in 0x70..=0x7B { consider(vk); }
    for vk in 0xA0..=0xA5 { consider(vk); }
    for vk in [0xBA, 0xBB, 0xBC, 0xBD, 0xBE, 0xBF, 0xC0, 0xDB, 0xDC, 0xDD, 0xDE] { consider(vk); }
    let Ok(mut keys) = KEYS.lock() else { return };
    if !keys.seeded {
        keys.held = now;
        keys.seeded = true;
        return;
    }
    let previous = keys.held.clone();
    for vk in &previous {
        if !now.contains(vk) {
            if let Some((scan, extended)) = scan_of(*vk) {
                keys.queue.push(Queued { scan, extended, down: false, repeat: false });
            }
        }
    }
    for vk in &now {
        if !previous.contains(vk) {
            if let Some((scan, extended)) = scan_of(*vk) {
                keys.queue.push(Queued { scan, extended, down: true, repeat: false });
            }
        }
    }
    if keys.queue.len() > 64 {
        let drop = keys.queue.len() - 32;
        keys.queue.drain(0..drop);
    }
    keys.held = now;
}

fn scan_of(vk: u32) -> Option<(u32, bool)> {
    let mapped = unsafe { MapVirtualKeyW(vk, MAPVK_VK_TO_VSC_EX) };
    let scan = mapped & 0xFF;
    if scan == 0 { return None; }
    Some((scan, mapped & 0xFF00 != 0))
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_TIMER => { with_host(|host| { if host.hwnd == hwnd { host.tick(); } }); }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}
