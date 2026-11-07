//! Windows: a low-level keyboard hook on its own message-loop thread.
//!
//! The hook only listens: every key still reaches the focused app. When the hold key
//! uses Win, releasing it would open the Start menu, so a no-op key (VK 0xE8, which
//! Windows leaves unassigned) is sent while it's held, the trick AutoHotkey uses.

use std::cell::RefCell;

use anyhow::{Result, anyhow};
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VK_LCONTROL, VK_LMENU,
    VK_LSHIFT, VK_LWIN, VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_RWIN,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG,
    SetWindowsHookExW, TranslateMessage, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN,
    WM_SYSKEYUP,
};

use crate::Event;
use crate::chord::{Chord, HoldKey, Key};

/// Unassigned virtual key: pressing it makes Windows treat Win as part of a chord.
const VK_MASK: u16 = 0xE8;

struct Ctx {
    chord: Chord,
    tx: Sender<Event>,
}

thread_local! {
    static CTX: RefCell<Option<Ctx>> = const { RefCell::new(None) };
}

pub fn has_permission() -> bool {
    true
}

pub fn request_permission() -> bool {
    true
}

pub fn start() -> Result<Receiver<Event>> {
    let hold = HoldKey::from_env();
    log::info!("hold key: {}", hold.label());
    let (tx, rx) = unbounded();
    let (ready_tx, ready_rx) = bounded::<Result<(), String>>(1);
    std::thread::Builder::new()
        .name("umless-hotkey".into())
        .spawn(move || run(hold, tx, ready_tx))?;
    ready_rx
        .recv()
        .map_err(|_| anyhow!("hotkey thread exited"))?
        .map_err(|e| anyhow!(e))?;
    Ok(rx)
}

fn run(hold: HoldKey, tx: Sender<Event>, ready: Sender<Result<(), String>>) {
    CTX.with(|c| {
        *c.borrow_mut() = Some(Ctx {
            chord: Chord::new(hold),
            tx,
        })
    });
    // SAFETY: a process-wide low-level hook; the callback runs on this thread's message loop.
    let hook =
        unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(callback), std::ptr::null_mut(), 0) };
    if hook.is_null() {
        let _ = ready.send(Err(format!(
            "Windows refused the keyboard hook (error {})",
            std::io::Error::last_os_error()
        )));
        return;
    }
    let _ = ready.send(Ok(()));
    // SAFETY: a standard message loop; MSG is plain data.
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn key_of(vk: u32) -> Key {
    match vk as u16 {
        VK_LCONTROL => Key::LCtrl,
        VK_RCONTROL => Key::RCtrl,
        VK_LWIN => Key::LMeta,
        VK_RWIN => Key::RMeta,
        VK_LMENU => Key::LAlt,
        VK_RMENU => Key::RAlt,
        VK_LSHIFT => Key::LShift,
        VK_RSHIFT => Key::RShift,
        _ => Key::Other,
    }
}

unsafe extern "system" fn callback(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        // SAFETY: for WH_KEYBOARD_LL, lparam points at a KBDLLHOOKSTRUCT for this call.
        let info = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        let injected = info.flags & LLKHF_INJECTED != 0;
        let pressed = match wparam as u32 {
            WM_KEYDOWN | WM_SYSKEYDOWN => Some(true),
            WM_KEYUP | WM_SYSKEYUP => Some(false),
            _ => None,
        };
        // Our own Ctrl+V and mask key are injected: ignore them.
        if let (Some(pressed), false) = (pressed, injected) {
            let key = key_of(info.vkCode);
            CTX.with(|c| {
                if let Some(ctx) = c.borrow_mut().as_mut() {
                    let event = ctx.chord.key(key, pressed);
                    // Win is held as part of the chord: "use" it now, so releasing it
                    // later (in any order) doesn't open the Start menu.
                    if ctx.chord.hold().uses_meta() && matches!(event, Some(Event::Down(_))) {
                        send_mask_key();
                    }
                    if let Some(e) = event {
                        let _ = ctx.tx.send(e);
                    }
                }
            });
        }
    }
    // SAFETY: passing the event on, as every hook must.
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

/// Presses and releases the unassigned mask key, so the coming Win-up doesn't open Start.
fn send_mask_key() {
    let input = |flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VK_MASK,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let inputs = [input(0), input(KEYEVENTF_KEYUP)];
    // SAFETY: two well-formed keyboard inputs.
    unsafe { SendInput(2, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
}
