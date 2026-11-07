//! Linux: the focused window's class (or Wayland app id), title and pid.
//!
//! Wayland has no common way to ask which window is focused, so each compositor is
//! asked its own way: Hyprland (`hyprctl`), Sway and other i3-IPC compositors
//! (`swaymsg`), KDE Plasma (`kdotool`, if installed). On X11, `_NET_ACTIVE_WINDOW`.
//! GNOME on Wayland doesn't expose it to apps, so there the style is the default.
//!
//! TODO: read the selection and the text around the cursor through AT-SPI (the
//! `atspi` crate over D-Bus), as macOS does through Accessibility. Until then
//! [`screen`] is empty and command mode uses the PRIMARY selection.

use std::process::{Command, Stdio};

use serde_json::Value;

use super::{Focus, Screen, display_name};

pub fn focused() -> Option<Focus> {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let found = if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        hyprland()
    } else if std::env::var_os("SWAYSOCK").is_some() || std::env::var_os("I3SOCK").is_some() {
        sway()
    } else if wayland
        && std::env::var("XDG_CURRENT_DESKTOP")
            .is_ok_and(|d| d.to_ascii_uppercase().contains("KDE"))
    {
        kde()
    } else {
        None
    };
    // X11 sessions, and Wayland sessions where only XWayland windows can be seen.
    found.or_else(|| if wayland { None } else { x11() })
}

fn json(cmd: &mut Command) -> Option<Value> {
    let out = cmd.stderr(Stdio::null()).output().ok()?;
    if !out.status.success() {
        return None;
    }
    serde_json::from_slice(&out.stdout).ok()
}

fn text(cmd: &mut Command) -> Option<String> {
    let out = cmd.stderr(Stdio::null()).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn focus(class: &str, title: Option<String>, pid: i64) -> Option<Focus> {
    if class.is_empty() {
        return None;
    }
    Some(Focus {
        pid: pid as i32,
        bundle_id: class.to_ascii_lowercase(),
        app: display_name(class),
        title: title.filter(|t| !t.is_empty()),
    })
}

fn hyprland() -> Option<Focus> {
    let v = json(Command::new("hyprctl").args(["activewindow", "-j"]))?;
    focus(
        v["class"].as_str()?,
        v["title"].as_str().map(String::from),
        v["pid"].as_i64().unwrap_or(0),
    )
}

fn sway() -> Option<Focus> {
    let tree = json(Command::new("swaymsg").args(["-t", "get_tree", "-r"]))?;
    let node = find_focused(&tree)?;
    let class = node["app_id"]
        .as_str()
        .or_else(|| node["window_properties"]["class"].as_str())?;
    focus(
        class,
        node["name"].as_str().map(String::from),
        node["pid"].as_i64().unwrap_or(0),
    )
}

/// The focused leaf of a sway/i3 layout tree.
fn find_focused(node: &Value) -> Option<&Value> {
    if node["focused"].as_bool() == Some(true) {
        return Some(node);
    }
    ["nodes", "floating_nodes"]
        .iter()
        .filter_map(|k| node[*k].as_array())
        .flatten()
        .find_map(find_focused)
}

fn kde() -> Option<Focus> {
    let id = text(Command::new("kdotool").arg("getactivewindow"))?;
    let class = text(Command::new("kdotool").args(["getwindowclassname", &id]))?;
    let title = text(Command::new("kdotool").args(["getwindowname", &id]));
    let pid = text(Command::new("kdotool").args(["getwindowpid", &id]))
        .and_then(|p| p.parse().ok())
        .unwrap_or(0);
    focus(&class, title, pid)
}

fn x11() -> Option<Focus> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, Window};

    let (conn, screen) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots.get(screen)?.root;
    let atom = |name: &str| -> Option<u32> {
        Some(
            conn.intern_atom(false, name.as_bytes())
                .ok()?
                .reply()
                .ok()?
                .atom,
        )
    };
    let (active, net_name, utf8, net_pid) = (
        atom("_NET_ACTIVE_WINDOW")?,
        atom("_NET_WM_NAME")?,
        atom("UTF8_STRING")?,
        atom("_NET_WM_PID")?,
    );
    let prop = |w: Window, p: u32, t: u32| -> Option<Vec<u8>> {
        let r = conn
            .get_property(false, w, p, t, 0, 4096)
            .ok()?
            .reply()
            .ok()?;
        (!r.value.is_empty()).then_some(r.value)
    };
    let win = prop(root, active, AtomEnum::WINDOW.into())?;
    let win = u32::from_ne_bytes(win.get(..4)?.try_into().ok()?);
    if win == 0 {
        return None;
    }
    // WM_CLASS is "instance\0Class\0": the class is the second part.
    let class_raw = prop(win, AtomEnum::WM_CLASS.into(), AtomEnum::STRING.into())?;
    let parts: Vec<&[u8]> = class_raw
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .collect();
    let class = String::from_utf8_lossy(parts.get(1).or(parts.first())?).into_owned();
    let title = prop(win, net_name, utf8)
        .or_else(|| prop(win, AtomEnum::WM_NAME.into(), AtomEnum::STRING.into()))
        .map(|t| String::from_utf8_lossy(&t).into_owned());
    let pid = prop(win, net_pid, AtomEnum::CARDINAL.into())
        .and_then(|p| Some(u32::from_ne_bytes(p.get(..4)?.try_into().ok()?)))
        .unwrap_or(0);
    focus(&class, title, pid as i64)
}

pub fn screen(_pid: i32) -> Screen {
    Screen::default()
}

pub fn field_text(_pid: i32) -> Option<String> {
    None
}
