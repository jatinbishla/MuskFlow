//! Windows: the foreground window, its title and its process's exe name.
//!
//! `Focus::bundle_id` is the exe name without `.exe`, lowercase ("slack", "msedge"),
//! which is what the style tables list. UWP apps run inside ApplicationFrameHost, so
//! for those the hosted app's own child window is used.
//!
//! TODO: read the selection and the text around the cursor through UI Automation
//! (`IUIAutomation::GetFocusedElement` + `TextPattern`), as macOS does through
//! Accessibility. Until then [`screen`] returns nothing, so command mode falls back
//! to Ctrl+C for the selection and the tone comes from the app alone.

use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId,
};

use super::{Focus, Screen, display_name};

pub fn focused() -> Option<Focus> {
    // SAFETY: plain Win32 queries on the foreground window.
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        return None;
    }
    let mut pid = window_pid(hwnd);
    let mut exe = exe_stem(pid)?;
    if exe == "applicationframehost" {
        if let Some((child_pid, child_exe)) = hosted_app(hwnd, pid) {
            pid = child_pid;
            exe = child_exe;
        }
    }
    Some(Focus {
        pid: pid as i32,
        app: display_name(&exe),
        bundle_id: exe,
        title: window_title(hwnd),
    })
}

fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    // SAFETY: hwnd is a window handle; pid is written.
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    pid
}

fn window_title(hwnd: HWND) -> Option<String> {
    // SAFETY: the buffer is sized from GetWindowTextLengthW.
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return None;
        }
        let mut buf = vec![0u16; len as usize + 1];
        let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        (n > 0).then(|| String::from_utf16_lossy(&buf[..n as usize]))
    }
}

/// "C:\\…\\slack.exe" -> "slack".
fn exe_stem(pid: u32) -> Option<String> {
    // SAFETY: the handle is closed below; the buffer length is passed in and updated.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len);
        CloseHandle(h);
        if ok == 0 {
            return None;
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        let file = path.rsplit(['\\', '/']).next().unwrap_or(&path);
        let stem = file
            .strip_suffix(".exe")
            .or_else(|| file.strip_suffix(".EXE"))
            .unwrap_or(file);
        Some(stem.to_ascii_lowercase())
    }
}

/// The app inside an ApplicationFrameHost window: its first child from another process.
fn hosted_app(hwnd: HWND, host_pid: u32) -> Option<(u32, String)> {
    struct Search {
        host_pid: u32,
        found: Option<u32>,
    }
    unsafe extern "system" fn each(child: HWND, lparam: LPARAM) -> i32 {
        // SAFETY: lparam is the &mut Search passed below, alive for the enumeration.
        let s = unsafe { &mut *(lparam as *mut Search) };
        let pid = window_pid(child);
        if pid != s.host_pid {
            s.found = Some(pid);
            return 0; // stop
        }
        1
    }
    let mut s = Search {
        host_pid,
        found: None,
    };
    // SAFETY: the callback only touches `s`, which outlives the call.
    unsafe { EnumChildWindows(hwnd, Some(each), &mut s as *mut Search as LPARAM) };
    let pid = s.found?;
    Some((pid, exe_stem(pid)?))
}

pub fn screen(_pid: i32) -> Screen {
    Screen::default()
}

pub fn field_text(_pid: i32) -> Option<String> {
    None
}
