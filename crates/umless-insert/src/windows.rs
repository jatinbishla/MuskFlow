//! Windows: the same clipboard paste as macOS. Snapshot the clipboard, put our text
//! there, send Ctrl+V with `SendInput`, then restore the snapshot unless something
//! else wrote to the clipboard meanwhile (`GetClipboardSequenceNumber`).
//! No permission is needed, but Windows won't let a normal process type into an
//! elevated (Run as administrator) window.

use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use windows_sys::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
    GetClipboardSequenceNumber, OpenClipboard, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY, VK_CONTROL,
};

const CF_UNICODETEXT: u32 = 13;
/// Formats whose handle is a GDI object rather than global memory: can't be copied as bytes.
const GDI_FORMATS: &[u32] = &[2, 3, 9, 14, 0x80, 0x82, 0x83, 0x8E];
const VK_C: VIRTUAL_KEY = 0x43;
const VK_V: VIRTUAL_KEY = 0x56;
const COPY_WAIT: Duration = Duration::from_millis(300);
const PASTE_SETTLE: Duration = Duration::from_millis(250);

type Snapshot = Vec<(u32, Vec<u8>)>;

static PENDING_RESTORE: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);

pub fn has_permission() -> bool {
    true
}

pub fn request_permission() -> bool {
    true
}

/// The clipboard, open for this scope. Another app may hold it for a moment: retry.
struct Clip;

impl Clip {
    fn open() -> Result<Self> {
        let t = Instant::now();
        loop {
            // SAFETY: no owner window; closed in Drop.
            if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
                return Ok(Clip);
            }
            if t.elapsed() > Duration::from_millis(500) {
                bail!("another app is holding the clipboard");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn snapshot(&self) -> Snapshot {
        let mut out = Vec::new();
        let mut fmt = 0;
        loop {
            // SAFETY: the clipboard is open.
            fmt = unsafe { EnumClipboardFormats(fmt) };
            if fmt == 0 {
                break;
            }
            if GDI_FORMATS.contains(&fmt) {
                continue;
            }
            // SAFETY: as above; the handle stays owned by the clipboard.
            let h = unsafe { GetClipboardData(fmt) };
            if let Some(bytes) = read_global(h) {
                out.push((fmt, bytes));
            }
        }
        out
    }

    fn set(&self, items: &[(u32, Vec<u8>)]) {
        // SAFETY: the clipboard is open; ownership of each handle passes to it on success.
        unsafe {
            EmptyClipboard();
            for (fmt, bytes) in items {
                if let Some(h) = alloc_global(bytes)
                    && SetClipboardData(*fmt, h as HANDLE).is_null()
                {
                    GlobalFree(h);
                }
            }
        }
    }

    fn text(&self) -> Option<String> {
        // SAFETY: the clipboard is open.
        let bytes = read_global(unsafe { GetClipboardData(CF_UNICODETEXT) })?;
        let wide: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&c| c != 0)
            .collect();
        Some(String::from_utf16_lossy(&wide))
    }
}

impl Drop for Clip {
    fn drop(&mut self) {
        // SAFETY: we opened it.
        unsafe { CloseClipboard() };
    }
}

fn read_global(h: HANDLE) -> Option<Vec<u8>> {
    if h.is_null() {
        return None;
    }
    // SAFETY: h is a global memory handle owned by the clipboard; locked only while copying.
    unsafe {
        let size = GlobalSize(h as HGLOBAL);
        if size == 0 {
            return None;
        }
        let p = GlobalLock(h as HGLOBAL) as *const u8;
        if p.is_null() {
            return None;
        }
        let v = std::slice::from_raw_parts(p, size).to_vec();
        GlobalUnlock(h as HGLOBAL);
        Some(v)
    }
}

fn alloc_global(bytes: &[u8]) -> Option<HGLOBAL> {
    // SAFETY: a fresh movable block of the right size, filled while locked.
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1));
        if h.is_null() {
            return None;
        }
        let p = GlobalLock(h) as *mut u8;
        if p.is_null() {
            GlobalFree(h);
            return None;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        GlobalUnlock(h);
        Some(h)
    }
}

fn utf16z(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(|c| c.to_le_bytes())
        .collect()
}

fn sequence() -> u32 {
    // SAFETY: reads a counter.
    unsafe { GetClipboardSequenceNumber() }
}

fn wait_for_restore() {
    if let Some(prev) = PENDING_RESTORE.lock().unwrap().take() {
        let _ = prev.join();
    }
}

pub fn insert(text: &str) -> Result<()> {
    wait_for_restore();
    let saved = {
        let clip = Clip::open()?;
        let saved = clip.snapshot();
        clip.set(&[(CF_UNICODETEXT, utf16z(text))]);
        saved
    };
    let ours = sequence();
    press_ctrl(VK_V);
    *PENDING_RESTORE.lock().unwrap() = Some(std::thread::spawn(move || {
        std::thread::sleep(PASTE_SETTLE);
        if sequence() != ours {
            log::info!("clipboard changed during paste; not restoring it");
            return;
        }
        match Clip::open() {
            Ok(clip) => clip.set(&saved),
            Err(e) => log::warn!("could not restore the clipboard: {e:#}"),
        }
    }));
    Ok(())
}

pub fn copy(text: &str) -> Result<()> {
    Clip::open()?.set(&[(CF_UNICODETEXT, utf16z(text))]);
    Ok(())
}

pub fn copy_selection() -> Option<String> {
    wait_for_restore();
    let saved = Clip::open().ok()?.snapshot();
    let before = sequence();
    press_ctrl(VK_C);
    let t = Instant::now();
    while sequence() == before && t.elapsed() < COPY_WAIT {
        std::thread::sleep(Duration::from_millis(10));
    }
    if sequence() == before {
        return None;
    }
    let clip = Clip::open().ok()?;
    let text = clip.text();
    clip.set(&saved);
    text.filter(|t| !t.trim().is_empty())
}

fn press_ctrl(key: VIRTUAL_KEY) {
    let input = |vk, flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let inputs = [
        input(VK_CONTROL, 0),
        input(key, 0),
        input(key, KEYEVENTF_KEYUP),
        input(VK_CONTROL, KEYEVENTF_KEYUP),
    ];
    // SAFETY: four well-formed keyboard inputs.
    unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        )
    };
}
