//! 播放器进程身份核实：只用“查询有限信息”权限确认进程名，
//! 确认是目标播放器后，调用方才申请内存读取权限，避免接触其他软件。
use windows::Win32::{
    Foundation::CloseHandle,
    System::{
        Diagnostics::ToolHelp::{CreateToolhelp32Snapshot,Process32FirstW,Process32NextW,PROCESSENTRY32W,TH32CS_SNAPPROCESS},
        Threading::{OpenProcess,QueryFullProcessImageNameW,PROCESS_NAME_WIN32,PROCESS_QUERY_LIMITED_INFORMATION},
    },
};

fn exe_name(path:&str)->&str {path.rsplit(['\\','/']).next().unwrap_or(path)}

/// 从进程快照按可执行文件名筛选进程号；快照不需要打开任何进程。
pub fn pids_named(exe:&str)->Vec<u32> {
    let mut pids=Vec::new();
    unsafe {
        let Ok(snapshot)=CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS,0) else {return pids};
        let mut entry=PROCESSENTRY32W{dwSize:std::mem::size_of::<PROCESSENTRY32W>() as u32,..Default::default()};
        let mut ok=Process32FirstW(snapshot,&mut entry).is_ok();
        while ok {
            let len=entry.szExeFile.iter().position(|&c|c==0).unwrap_or(entry.szExeFile.len());
            if String::from_utf16_lossy(&entry.szExeFile[..len]).eq_ignore_ascii_case(exe){pids.push(entry.th32ProcessID);}
            ok=Process32NextW(snapshot,&mut entry).is_ok();
        }
        let _=CloseHandle(snapshot);
    }
    pids
}

/// 仅以有限查询权限核对进程映像名（进程号可能被复用，每次使用前都要核对）。
pub fn image_is(pid:u32,exe:&str)->bool {
    image_path(pid).is_some_and(|path|exe_name(&path).eq_ignore_ascii_case(exe))
}

/// 该进程可见、无宿主、不是工具窗口里面积最大的一扇，当作主窗口。
#[derive(Clone, Copy, Debug)]
pub(crate) struct WindowFact {
    pub pid: u32,
    pub visible: bool,
    pub owned: bool,
    pub tool: bool,
    pub area: i64,
}

pub(crate) fn pick_main(pid: u32, windows: &[WindowFact]) -> Option<usize> {
    windows.iter().enumerate()
        .filter(|(_, window)| window.pid == pid && window.visible && !window.owned && !window.tool && window.area > 0)
        .max_by_key(|(_, window)| window.area)
        .map(|(index, _)| index)
}

/// 把播放器主窗口还原并置前。找不到可见主窗口时返回 false，由界面给出短提示。
pub fn activate_named(exe: &str) -> bool {
    if exe.trim().is_empty() { return false; }
    pids_named(exe).into_iter().any(activate_pid)
}

fn activate_pid(pid: u32) -> bool {
    let Some(window) = main_window(pid) else { return false };
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{IsIconic, SW_RESTORE, ShowWindow};
        if IsIconic(window).as_bool() { let _ = ShowWindow(window, SW_RESTORE); }
        force_foreground(window)
    }
}

fn main_window(pid: u32) -> Option<windows::Win32::Foundation::HWND> {
    use windows::Win32::{
        Foundation::{BOOL, HWND, LPARAM, RECT},
        UI::WindowsAndMessaging::{EnumWindows, GW_OWNER, GWL_EXSTYLE, GetWindow, GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible, WS_EX_TOOLWINDOW},
    };
    struct Found { facts: Vec<WindowFact>, handles: Vec<HWND> }
    unsafe extern "system" fn visit(hwnd: HWND, data: LPARAM) -> BOOL {
        let found = &mut *(data.0 as *mut Found);
        let mut owner = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut owner));
        let owned = GetWindow(hwnd, GW_OWNER).is_ok();
        let tool = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0;
        let mut rect = RECT::default();
        let area = if GetWindowRect(hwnd, &mut rect).is_ok() {
            (rect.right - rect.left).max(0) as i64 * (rect.bottom - rect.top).max(0) as i64
        } else { 0 };
        found.facts.push(WindowFact { pid: owner, visible: IsWindowVisible(hwnd).as_bool(), owned, tool, area });
        found.handles.push(hwnd);
        true.into()
    }
    let mut found = Found { facts: Vec::new(), handles: Vec::new() };
    unsafe { let _ = EnumWindows(Some(visit), LPARAM(&mut found as *mut Found as isize)); }
    let index = pick_main(pid, &found.facts)?;
    found.handles.get(index).copied()
}

unsafe fn force_foreground(window: windows::Win32::Foundation::HWND) -> bool {
    use windows::Win32::{
        System::Threading::{AttachThreadInput, GetCurrentThreadId},
        UI::WindowsAndMessaging::{BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow},
    };
    let foreground = GetForegroundWindow();
    let foreground_thread = GetWindowThreadProcessId(foreground, None);
    let current = GetCurrentThreadId();
    let attached = foreground_thread != 0 && foreground_thread != current && AttachThreadInput(current, foreground_thread, true).as_bool();
    let _ = BringWindowToTop(window);
    let ok = SetForegroundWindow(window).as_bool();
    if attached { let _ = AttachThreadInput(current, foreground_thread, false); }
    ok
}

/// 用“查询有限信息”权限读取进程的可执行文件路径；只用于核对身份和读取版本号。
pub fn image_path(pid:u32)->Option<String> {
    if pid==0 {return None;}
    unsafe {
        let process=OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION,false,pid).ok()?;
        let mut path=[0u16;1024];let mut length=path.len() as u32;
        let result=QueryFullProcessImageNameW(process,PROCESS_NAME_WIN32,windows::core::PWSTR(path.as_mut_ptr()),&mut length);
        let _=CloseHandle(process);
        result.ok()?;
        Some(String::from_utf16_lossy(&path[..length as usize]))
    }
}

#[test]
fn exe_name_takes_file_part(){
    assert_eq!(exe_name(r"C:\Program Files\KuGou\KuGou.exe"),"KuGou.exe");
    assert_eq!(exe_name("cloudmusic.exe"),"cloudmusic.exe");
}

#[test]
fn largest_ownerless_window_is_the_main_one(){
    let windows = [
        WindowFact { pid: 7, visible: true, owned: true, tool: false, area: 900 },
        WindowFact { pid: 7, visible: true, owned: false, tool: true, area: 800 },
        WindowFact { pid: 7, visible: false, owned: false, tool: false, area: 700 },
        WindowFact { pid: 7, visible: true, owned: false, tool: false, area: 400 },
        WindowFact { pid: 7, visible: true, owned: false, tool: false, area: 650 },
        WindowFact { pid: 9, visible: true, owned: false, tool: false, area: 2000 },
    ];
    assert_eq!(pick_main(7, &windows), Some(4));
    assert_eq!(pick_main(3, &windows), None);
}

#[test]
fn snapshot_finds_own_process(){
    let me=std::env::current_exe().unwrap();
    let exe=me.file_name().unwrap().to_str().unwrap();
    let pid=std::process::id();
    assert!(pids_named(exe).contains(&pid));
    assert!(image_is(pid,exe));
    assert!(!image_is(pid,"KuGou.exe"));
}
