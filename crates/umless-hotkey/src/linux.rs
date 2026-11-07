//! Linux: evdev first, XRecord on X11 as the fallback.
//!
//! Wayland doesn't let apps watch the keyboard globally, so Umless reads the keyboards
//! themselves (`/dev/input/event*`). That works under Wayland and X11 alike but needs
//! read access to the devices: `sudo usermod -aG input $USER`, then log out and in.
//! Without it, on an X11 session, the XRecord extension reports keys instead.
//! Both only listen; keys still reach the focused app.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use crossbeam_channel::{Receiver, Sender, unbounded};

use crate::Event;
use crate::chord::{Chord, HoldKey, Key};

// Linux input-event-codes.h
const KEY_LEFTCTRL: u16 = 29;
const KEY_LEFTSHIFT: u16 = 42;
const KEY_RIGHTSHIFT: u16 = 54;
const KEY_LEFTALT: u16 = 56;
const KEY_RIGHTCTRL: u16 = 97;
const KEY_RIGHTALT: u16 = 100;
const KEY_LEFTMETA: u16 = 125;
const KEY_RIGHTMETA: u16 = 126;
/// Codes from here on are mouse and joystick buttons, not keys.
const BTN_MISC: u16 = 0x100;
/// X keycodes are evdev codes + 8.
const X_KEYCODE_OFFSET: u16 = 8;

fn key_of(code: u16) -> Key {
    match code {
        KEY_LEFTCTRL => Key::LCtrl,
        KEY_RIGHTCTRL => Key::RCtrl,
        KEY_LEFTMETA => Key::LMeta,
        KEY_RIGHTMETA => Key::RMeta,
        KEY_LEFTALT => Key::LAlt,
        KEY_RIGHTALT => Key::RAlt,
        KEY_LEFTSHIFT => Key::LShift,
        KEY_RIGHTSHIFT => Key::RShift,
        _ => Key::Other,
    }
}

/// Keyboards we can read (devices with letter keys).
fn keyboards() -> Vec<(PathBuf, evdev::Device)> {
    evdev::enumerate()
        .filter(|(_, d)| {
            d.supported_keys().is_some_and(|k| {
                k.contains(evdev::KeyCode::KEY_A) && k.contains(evdev::KeyCode::KEY_SPACE)
            })
        })
        .collect()
}

fn on_x11() -> bool {
    std::env::var_os("DISPLAY").is_some_and(|d| !d.is_empty())
}

pub fn has_permission() -> bool {
    !keyboards().is_empty() || x11_record_available()
}

/// Linux has no permission prompt: the user joins the `input` group instead.
pub fn request_permission() -> bool {
    has_permission()
}

pub fn start() -> Result<Receiver<Event>> {
    let hold = HoldKey::from_env();
    log::info!("hold key: {}", hold.label());
    let (keys_tx, keys_rx) = unbounded::<(u16, bool)>();
    if !keyboards().is_empty() {
        std::thread::Builder::new()
            .name("umless-evdev-scan".into())
            .spawn(move || scan(keys_tx))?;
    } else if on_x11() {
        log::warn!("no readable keyboards in /dev/input; using XRecord (X11 only)");
        let tx = keys_tx.clone();
        let (ready_tx, ready_rx) = crossbeam_channel::bounded(1);
        std::thread::Builder::new()
            .name("umless-xrecord".into())
            .spawn(move || {
                if let Err(e) = xrecord(tx, &ready_tx) {
                    let _ = ready_tx.send(Err(format!("{e:#}")));
                }
            })?;
        ready_rx
            .recv()
            .map_err(|_| anyhow!("XRecord thread exited"))?
            .map_err(|e| anyhow!(e))?;
    } else {
        bail!(
            "Umless can't read the keyboard. Add yourself to the input group \
             (sudo usermod -aG input $USER), log out and back in, then start Umless again."
        );
    }
    let (tx, rx) = unbounded();
    std::thread::Builder::new()
        .name("umless-hotkey".into())
        .spawn(move || track(hold, keys_rx, tx))?;
    Ok(rx)
}

fn track(hold: HoldKey, keys: Receiver<(u16, bool)>, tx: Sender<Event>) {
    let mut chord = Chord::new(hold);
    for (code, pressed) in keys {
        if code >= BTN_MISC {
            continue;
        }
        if let Some(e) = chord.key(key_of(code), pressed)
            && tx.send(e).is_err()
        {
            return;
        }
    }
}

/// Starts a reader for every keyboard, and for keyboards plugged in later.
fn scan(tx: Sender<(u16, bool)>) {
    let mut seen = HashSet::new();
    loop {
        for (path, dev) in keyboards() {
            if seen.insert(path.clone()) {
                log::info!(
                    "reading keys from {} ({})",
                    path.display(),
                    dev.name().unwrap_or("?")
                );
                let tx = tx.clone();
                let _ = std::thread::Builder::new()
                    .name("umless-evdev".into())
                    .spawn(move || {
                        read(dev, &tx);
                        log::info!("stopped reading {}", path.display());
                    });
            }
        }
        std::thread::sleep(Duration::from_secs(5));
    }
}

fn read(mut dev: evdev::Device, tx: &Sender<(u16, bool)>) {
    loop {
        let Ok(events) = dev.fetch_events() else {
            return; // unplugged
        };
        for ev in events {
            if let evdev::EventSummary::Key(_, code, value) = ev.destructure() {
                // 1 = press, 2 = auto-repeat, 0 = release.
                if tx.send((code.code(), value != 0)).is_err() {
                    return;
                }
            }
        }
    }
}

fn x11_record_available() -> bool {
    use x11rb::connection::RequestConnection;
    on_x11()
        && x11rb::connect(None).is_ok_and(|(c, _)| {
            c.extension_information(x11rb::protocol::record::X11_EXTENSION_NAME)
                .ok()
                .flatten()
                .is_some()
        })
}

/// X11 fallback: the RECORD extension delivers every key press and release.
fn xrecord(
    tx: Sender<(u16, bool)>,
    ready: &crossbeam_channel::Sender<Result<(), String>>,
) -> Result<()> {
    use x11rb::connection::{Connection, RequestConnection};
    use x11rb::protocol::record::{self, ConnectionExt as _};
    use x11rb::protocol::xproto;
    use x11rb::wrapper::ConnectionExt as _;

    let (ctrl, _) = x11rb::connect(None).context("could not connect to the X server")?;
    let (data, _) = x11rb::connect(None).context("could not connect to the X server")?;
    ctrl.extension_information(record::X11_EXTENSION_NAME)?
        .context("the X server has no RECORD extension")?;
    let id = ctrl.generate_id()?;
    let range = record::Range {
        device_events: record::Range8 {
            first: xproto::KEY_PRESS_EVENT,
            last: xproto::KEY_RELEASE_EVENT,
        },
        ..Default::default()
    };
    ctrl.record_create_context(id, 0, &[record::CS::ALL_CLIENTS.into()], &[range])?
        .check()
        .context("XRecord refused the context")?;
    ctrl.sync()?;
    let _ = ready.send(Ok(()));
    for reply in data.record_enable_context(id)? {
        let reply = reply?;
        if reply.category != 0 {
            continue; // only FromServer carries events
        }
        for ev in reply.data.chunks_exact(32) {
            let kind = ev[0] & 0x7f;
            let pressed = match kind {
                xproto::KEY_PRESS_EVENT => true,
                xproto::KEY_RELEASE_EVENT => false,
                _ => continue,
            };
            let code = (ev[1] as u16).saturating_sub(X_KEYCODE_OFFSET);
            if tx.send((code, pressed)).is_err() {
                return Ok(());
            }
        }
    }
    Ok(())
}
