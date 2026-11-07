//! Global push-to-talk hotkey.
//!
//! Hold **Fn** to dictate; hold **Fn + Control** for command mode. Releasing Fn
//! ends the dictation. Pressing any other key while Fn is held (Fn+←, Fn+F5…)
//! cancels it, so Fn's usual shortcuts keep working.
//!
//! macOS: a listen-only CGEventTap on a background run-loop thread. It needs the
//! Input Monitoring permission, and it can't stop the system's own Fn action, so
//! users should set *System Settings → Keyboard → Press 🌐 key to → Do Nothing*.
//!
//! Windows and Linux have no Fn key that software can see, so they use a [`HoldKey`]
//! instead (default **Ctrl + Win**, like Wispr Flow; `UMLESS_HOLD_KEY=right-ctrl` and
//! others pick another). Adding **Alt** while holding it is command mode (**Shift** when
//! the hold key itself uses Alt).
//!
//! - Windows: a low-level keyboard hook (`SetWindowsHookExW(WH_KEYBOARD_LL)`). No permission.
//! - Linux: evdev (`/dev/input/event*`), which works on Wayland and X11 but needs the user
//!   in the `input` group. Without it, on X11 it falls back to the XRecord extension.

use anyhow::Result;
use crossbeam_channel::Receiver;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Fn: dictate, clean up, insert.
    Dictate,
    /// Fn + Control: speak an instruction; Umless writes the text.
    Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Fn went down: start listening.
    Down(Mode),
    /// Fn came up: finish. The mode is Command if Control was held at any point.
    Up(Mode),
    /// Another key was pressed with Fn: drop this dictation.
    Cancel,
}

/// Whether the process may watch the keyboard (Input Monitoring).
pub fn has_permission() -> bool {
    imp::has_permission()
}

/// Asks macOS for Input Monitoring; shows the system prompt the first time.
pub fn request_permission() -> bool {
    imp::request_permission()
}

/// Starts watching the keyboard on a background thread.
pub fn start() -> Result<Receiver<Event>> {
    imp::start()
}

/// What to say in the UI: "Fn" on macOS, "Ctrl + Win" (or the configured key) elsewhere.
pub fn hold_key_label() -> String {
    if cfg!(target_os = "macos") {
        "Fn".to_string()
    } else {
        HoldKey::from_env().label()
    }
}

/// Held with the hold key for command mode: "control" on macOS, "Alt" (or "Shift") elsewhere.
pub fn command_key_label() -> String {
    if cfg!(target_os = "macos") {
        "control".to_string()
    } else {
        HoldKey::from_env().command_label().to_string()
    }
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
mod chord;
pub use chord::HoldKey;

/// Pure state machine, kept apart from the OS hook so it can be tested.
#[derive(Debug, Default)]
struct Tracker {
    state: State,
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
enum State {
    #[default]
    Idle,
    Held(Mode),
    /// Another key was pressed; ignore everything until Fn comes up.
    Cancelled,
}

impl Tracker {
    /// The Fn key or a modifier changed. `fn_down`/`ctrl_down` are the new states.
    fn flags(&mut self, fn_changed: bool, fn_down: bool, ctrl_down: bool) -> Option<Event> {
        match self.state {
            State::Idle if fn_changed && fn_down => {
                let mode = if ctrl_down {
                    Mode::Command
                } else {
                    Mode::Dictate
                };
                self.state = State::Held(mode);
                Some(Event::Down(mode))
            }
            State::Held(mode) if fn_changed && !fn_down => {
                self.state = State::Idle;
                Some(Event::Up(mode))
            }
            State::Held(Mode::Dictate) if ctrl_down => {
                self.state = State::Held(Mode::Command);
                None
            }
            State::Cancelled if fn_changed && !fn_down => {
                self.state = State::Idle;
                None
            }
            _ => None,
        }
    }

    fn key_down(&mut self) -> Option<Event> {
        if let State::Held(_) = self.state {
            self.state = State::Cancelled;
            return Some(Event::Cancel);
        }
        None
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::c_void;
    use std::ptr::NonNull;

    use anyhow::{Result, anyhow};
    use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
    use objc2_core_foundation::{CFMachPort, CFRetained, CFRunLoop, kCFRunLoopCommonModes};
    use objc2_core_graphics::{
        CGEvent, CGEventField, CGEventFlags, CGEventTapLocation, CGEventTapOptions,
        CGEventTapPlacement, CGEventTapProxy, CGEventType, CGPreflightListenEventAccess,
        CGRequestListenEventAccess,
    };

    use super::{Event, Tracker};

    /// kVK_Function: the Fn / 🌐 key.
    const FN_KEYCODE: i64 = 63;

    struct Ctx {
        tracker: Tracker,
        tx: Sender<Event>,
        port: Option<CFRetained<CFMachPort>>,
    }

    pub fn has_permission() -> bool {
        CGPreflightListenEventAccess()
    }

    pub fn request_permission() -> bool {
        CGRequestListenEventAccess()
    }

    pub fn start() -> Result<Receiver<Event>> {
        let (tx, rx) = unbounded();
        let (ready_tx, ready_rx) = bounded::<Result<(), String>>(1);
        std::thread::Builder::new()
            .name("umless-hotkey".into())
            .spawn(move || run(tx, ready_tx))?;
        ready_rx
            .recv()
            .map_err(|_| anyhow!("hotkey thread exited"))?
            .map_err(|e| anyhow!(e))?;
        Ok(rx)
    }

    fn run(tx: Sender<Event>, ready: Sender<Result<(), String>>) {
        // Leaked on purpose: the tap lives as long as the process.
        let ctx: &'static mut Ctx = Box::leak(Box::new(Ctx {
            tracker: Tracker::default(),
            tx,
            port: None,
        }));
        let mask = (1u64 << CGEventType::FlagsChanged.0) | (1u64 << CGEventType::KeyDown.0);
        // SAFETY: `callback` matches CGEventTapCallBack and `ctx` outlives the tap.
        let port = unsafe {
            CGEvent::tap_create(
                CGEventTapLocation::SessionEventTap,
                CGEventTapPlacement::HeadInsertEventTap,
                CGEventTapOptions::ListenOnly,
                mask,
                Some(callback),
                (ctx as *mut Ctx).cast::<c_void>(),
            )
        };
        let Some(port) = port else {
            let _ = ready.send(Err(
                "macOS refused to let Umless watch the keyboard. Allow it in System Settings → Privacy & Security → Input Monitoring, then restart Umless."
                    .into(),
            ));
            return;
        };
        let Some(source) = CFMachPort::new_run_loop_source(None, Some(&port), 0) else {
            let _ = ready.send(Err(
                "could not create a run-loop source for the hotkey".into()
            ));
            return;
        };
        let Some(run_loop) = CFRunLoop::current() else {
            let _ = ready.send(Err("no run loop on the hotkey thread".into()));
            return;
        };
        // SAFETY: kCFRunLoopCommonModes is a constant provided by CoreFoundation.
        run_loop.add_source(Some(&source), unsafe { kCFRunLoopCommonModes });
        CGEvent::tap_enable(&port, true);
        ctx.port = Some(port);
        let _ = ready.send(Ok(()));
        CFRunLoop::run();
    }

    unsafe extern "C-unwind" fn callback(
        _proxy: CGEventTapProxy,
        kind: CGEventType,
        event: NonNull<CGEvent>,
        user_info: *mut c_void,
    ) -> *mut CGEvent {
        // SAFETY: user_info is the leaked Ctx from `run`; only this thread touches it.
        let ctx = unsafe { &mut *user_info.cast::<Ctx>() };
        // SAFETY: CoreGraphics hands us a valid event for the duration of the call.
        let ev = unsafe { event.as_ref() };
        let out = match kind {
            CGEventType::FlagsChanged => {
                let flags = CGEvent::flags(Some(ev));
                let code =
                    CGEvent::integer_value_field(Some(ev), CGEventField::KeyboardEventKeycode);
                ctx.tracker.flags(
                    code == FN_KEYCODE,
                    flags.contains(CGEventFlags::MaskSecondaryFn),
                    flags.contains(CGEventFlags::MaskControl),
                )
            }
            CGEventType::KeyDown => ctx.tracker.key_down(),
            // macOS turns a tap off if a callback is slow or on user input; turn it back on.
            CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
                if let Some(port) = &ctx.port {
                    CGEvent::tap_enable(port, true);
                }
                None
            }
            _ => None,
        };
        if let Some(e) = out {
            let _ = ctx.tx.send(e);
        }
        event.as_ptr() // listen-only: pass the event through untouched
    }
}

#[cfg(windows)]
#[path = "windows.rs"]
mod imp;

#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod imp;

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
mod imp {
    use anyhow::{Result, bail};
    use crossbeam_channel::Receiver;

    use super::Event;

    pub fn has_permission() -> bool {
        false
    }

    pub fn request_permission() -> bool {
        false
    }

    pub fn start() -> Result<Receiver<Event>> {
        bail!("the push-to-talk hotkey isn't supported on this OS yet")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fn_hold_dictates_and_control_upgrades_to_command() {
        let mut t = Tracker::default();
        assert_eq!(t.flags(true, true, false), Some(Event::Down(Mode::Dictate)));
        assert_eq!(t.flags(false, true, true), None);
        assert_eq!(t.flags(true, false, true), Some(Event::Up(Mode::Command)));

        assert_eq!(t.flags(true, true, true), Some(Event::Down(Mode::Command)));
        assert_eq!(t.flags(true, false, false), Some(Event::Up(Mode::Command)));
    }

    #[test]
    fn another_key_with_fn_cancels_until_release() {
        let mut t = Tracker::default();
        t.flags(true, true, false);
        assert_eq!(t.key_down(), Some(Event::Cancel));
        assert_eq!(t.key_down(), None);
        assert_eq!(t.flags(true, false, false), None);
        assert_eq!(t.flags(true, true, false), Some(Event::Down(Mode::Dictate)));
    }

    #[test]
    fn modifiers_alone_do_nothing() {
        let mut t = Tracker::default();
        assert_eq!(t.flags(false, false, true), None);
        assert_eq!(t.key_down(), None);
    }
}
