//! Linux: clipboard paste through the standard command-line tools, so it works on
//! any desktop without linking GTK or Qt.
//!
//! - Wayland: `wl-copy`/`wl-paste` (wl-clipboard) and `wtype`, or `ydotool` where the
//!   compositor has no virtual-keyboard protocol (GNOME). ydotool needs `ydotoold` running.
//! - X11: `xclip` (or `xsel`) and `xdotool`.
//!
//! As on macOS the user's clipboard is put back afterwards, unless it changed meanwhile.
//! One representation is kept (an image, or else the text). Terminals get Ctrl+Shift+V.
//! The selection comes from the PRIMARY selection, so no key press is needed for it.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context, Result, bail};

const PASTE_SETTLE: Duration = Duration::from_millis(250);

static PENDING_RESTORE: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);

/// Window classes / app ids of terminals, where Ctrl+V doesn't paste.
const TERMINALS: &[&str] = &[
    "gnome-terminal",
    "gnome-terminal-server",
    "org.gnome.terminal",
    "org.gnome.console",
    "org.gnome.ptyxis",
    "kgx",
    "konsole",
    "org.kde.konsole",
    "xfce4-terminal",
    "kitty",
    "alacritty",
    "org.wezfurlong.wezterm",
    "wezterm",
    "foot",
    "footclient",
    "tilix",
    "terminator",
    "xterm",
    "urxvt",
    "st",
    "com.mitchellh.ghostty",
    "ghostty",
    "warp",
    "dev.warp.warp",
];

pub fn is_terminal(app_id: &str) -> bool {
    TERMINALS.iter().any(|t| app_id.eq_ignore_ascii_case(t))
}

fn wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
}

fn have(cmd: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(cmd).is_file()))
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Typer {
    Wtype,
    Ydotool,
    Xdotool,
}

fn typer() -> Option<Typer> {
    if wayland() {
        // wtype needs the virtual-keyboard protocol (wlroots, KDE); GNOME lacks it.
        let gnome = std::env::var("XDG_CURRENT_DESKTOP")
            .is_ok_and(|d| d.to_ascii_lowercase().contains("gnome"));
        if have("wtype") && !gnome {
            return Some(Typer::Wtype);
        }
        if have("ydotool") {
            return Some(Typer::Ydotool);
        }
        // XWayland apps still take xdotool's keys.
        if have("xdotool") {
            return Some(Typer::Xdotool);
        }
        None
    } else if have("xdotool") {
        Some(Typer::Xdotool)
    } else {
        None
    }
}

fn clipboard_tools() -> bool {
    if wayland() {
        have("wl-copy") && have("wl-paste")
    } else {
        have("xclip") || have("xsel")
    }
}

/// Whether Umless can type here: the clipboard and key tools are installed.
pub fn has_permission() -> bool {
    clipboard_tools() && typer().is_some()
}

pub fn request_permission() -> bool {
    if !has_permission() {
        log::warn!("{}", missing_tools());
    }
    has_permission()
}

fn missing_tools() -> String {
    if wayland() {
        "Umless needs wl-clipboard and wtype (or ydotool with ydotoold running on GNOME) to type. \
         Install them with your package manager, e.g. sudo apt install wl-clipboard wtype ydotool"
            .into()
    } else {
        "Umless needs xclip and xdotool to type. Install them, e.g. sudo apt install xclip xdotool"
            .into()
    }
}

fn run_with_input(mut cmd: Command, input: &[u8]) -> Result<()> {
    let name = format!("{:?}", cmd.get_program());
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("could not run {name}"))?;
    child.stdin.take().context("no stdin")?.write_all(input)?;
    // wl-copy and xclip fork a server that keeps the selection; the parent exits.
    let status = child.wait()?;
    if !status.success() {
        bail!("{name} failed");
    }
    Ok(())
}

fn output(cmd: &mut Command) -> Option<Vec<u8>> {
    let out = cmd.stderr(Stdio::null()).output().ok()?;
    out.status.success().then_some(out.stdout)
}

#[derive(Clone, Copy)]
enum Sel {
    Clipboard,
    Primary,
}

fn set_clipboard(mime: &str, bytes: &[u8]) -> Result<()> {
    if wayland() {
        let mut c = Command::new("wl-copy");
        c.args(["--type", mime]);
        run_with_input(c, bytes)
    } else if have("xclip") {
        let mut c = Command::new("xclip");
        c.args(["-selection", "clipboard", "-t", mime]);
        run_with_input(c, bytes)
    } else {
        let mut c = Command::new("xsel");
        c.args(["--clipboard", "--input"]);
        run_with_input(c, bytes)
    }
}

fn get(sel: Sel, mime: Option<&str>) -> Option<Vec<u8>> {
    if wayland() {
        let mut c = Command::new("wl-paste");
        c.arg("--no-newline");
        if let Sel::Primary = sel {
            c.arg("--primary");
        }
        if let Some(m) = mime {
            c.args(["--type", m]);
        }
        output(&mut c)
    } else if have("xclip") {
        let mut c = Command::new("xclip");
        c.args([
            "-selection",
            match sel {
                Sel::Clipboard => "clipboard",
                Sel::Primary => "primary",
            },
            "-o",
        ]);
        if let Some(m) = mime {
            c.args(["-t", m]);
        }
        output(&mut c)
    } else {
        let mut c = Command::new("xsel");
        c.arg(match sel {
            Sel::Clipboard => "--clipboard",
            Sel::Primary => "--primary",
        });
        c.arg("--output");
        output(&mut c)
    }
}

/// The types on offer, e.g. `image/png`, `text/plain;charset=utf-8`.
fn types() -> Vec<String> {
    let out = if wayland() {
        output(Command::new("wl-paste").arg("--list-types"))
    } else if have("xclip") {
        output(Command::new("xclip").args(["-selection", "clipboard", "-t", "TARGETS", "-o"]))
    } else {
        None
    };
    out.map(|o| {
        String::from_utf8_lossy(&o)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect()
    })
    .unwrap_or_default()
}

/// The clipboard's most useful representation: an image if there is one, else text.
fn snapshot() -> Option<(String, Vec<u8>)> {
    let types = types();
    let pick = types
        .iter()
        .find(|t| t.starts_with("image/png"))
        .or_else(|| types.iter().find(|t| t.starts_with("image/")))
        .or_else(|| {
            types
                .iter()
                .find(|t| t.as_str() == "text/plain;charset=utf-8")
        })
        .or_else(|| types.iter().find(|t| t.as_str() == "UTF8_STRING"))
        .or_else(|| types.iter().find(|t| t.starts_with("text/plain")))
        .cloned();
    match pick {
        Some(mime) => get(Sel::Clipboard, Some(&mime)).map(|b| (mime, b)),
        // xsel can't list types: keep the text.
        None => get(Sel::Clipboard, None).map(|b| ("text/plain;charset=utf-8".into(), b)),
    }
}

const TEXT: &str = "text/plain;charset=utf-8";

fn press_paste(terminal: bool) -> Result<()> {
    let Some(t) = typer() else {
        bail!("{}", missing_tools());
    };
    let status = match (t, terminal) {
        (Typer::Wtype, false) => Command::new("wtype")
            .args(["-M", "ctrl", "v", "-m", "ctrl"])
            .status(),
        (Typer::Wtype, true) => Command::new("wtype")
            .args([
                "-M", "ctrl", "-M", "shift", "v", "-m", "shift", "-m", "ctrl",
            ])
            .status(),
        // Linux key codes: 29 = left Ctrl, 42 = left Shift, 47 = V.
        (Typer::Ydotool, false) => Command::new("ydotool")
            .args(["key", "29:1", "47:1", "47:0", "29:0"])
            .status(),
        (Typer::Ydotool, true) => Command::new("ydotool")
            .args(["key", "29:1", "42:1", "47:1", "47:0", "42:0", "29:0"])
            .status(),
        (Typer::Xdotool, false) => Command::new("xdotool")
            .args(["key", "--clearmodifiers", "ctrl+v"])
            .status(),
        (Typer::Xdotool, true) => Command::new("xdotool")
            .args(["key", "--clearmodifiers", "ctrl+shift+v"])
            .status(),
    }
    .with_context(|| format!("could not run {t:?}"))?;
    if !status.success() {
        bail!("{t:?} could not press the paste keys");
    }
    Ok(())
}

pub fn insert(text: &str, app_id: &str) -> Result<()> {
    if !clipboard_tools() {
        bail!("{}", missing_tools());
    }
    if let Some(prev) = PENDING_RESTORE.lock().unwrap().take() {
        let _ = prev.join();
    }
    let saved = snapshot();
    set_clipboard(TEXT, text.as_bytes())?;
    press_paste(is_terminal(app_id))?;
    let ours = text.to_string();
    *PENDING_RESTORE.lock().unwrap() = Some(std::thread::spawn(move || {
        std::thread::sleep(PASTE_SETTLE);
        let now = get(Sel::Clipboard, Some(TEXT)).unwrap_or_default();
        if now != ours.as_bytes() {
            log::info!("clipboard changed during paste; not restoring it");
            return;
        }
        if let Some((mime, bytes)) = saved
            && let Err(e) = set_clipboard(&mime, &bytes)
        {
            log::warn!("could not restore the clipboard: {e:#}");
        }
    }));
    Ok(())
}

pub fn copy(text: &str) -> Result<()> {
    set_clipboard(TEXT, text.as_bytes())
}

pub fn copy_selection() -> Option<String> {
    let bytes = get(Sel::Primary, None)?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    (!text.trim().is_empty()).then_some(text)
}
