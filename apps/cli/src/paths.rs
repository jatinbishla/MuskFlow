//! Where Umless keeps the user's data and log on each OS, and how it opens files.
//!
//! - macOS: `~/Library/Application Support/Umless`, log in `~/Library/Logs/Umless.log`
//! - Windows: `%APPDATA%\Umless`, log in `%LOCALAPPDATA%\Umless\Umless.log`
//! - Linux: `$XDG_DATA_HOME/umless` (`~/.local/share/umless`), log in
//!   `$XDG_STATE_HOME/umless/umless.log` (`~/.local/state/umless/umless.log`)

use std::path::{Path, PathBuf};

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

#[allow(dead_code)]
fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// The folder for history, dictionary and settings.
pub fn data_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    return home().join("Library/Application Support/Umless");
    #[cfg(windows)]
    return env_dir("APPDATA")
        .unwrap_or_else(|| home().join("AppData/Roaming"))
        .join("Umless");
    #[cfg(not(any(target_os = "macos", windows)))]
    return env_dir("XDG_DATA_HOME")
        .unwrap_or_else(|| home().join(".local/share"))
        .join("umless");
}

/// Umless's log file, written when it runs without a terminal.
pub fn log_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    return home().join("Library/Logs/Umless.log");
    #[cfg(windows)]
    return env_dir("LOCALAPPDATA")
        .unwrap_or_else(|| home().join("AppData/Local"))
        .join("Umless/Umless.log");
    #[cfg(not(any(target_os = "macos", windows)))]
    return env_dir("XDG_STATE_HOME")
        .unwrap_or_else(|| home().join(".local/state"))
        .join("umless/umless.log");
}

/// How to open a path.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Open {
    /// In the user's text editor.
    Edit,
    /// Show it in the file manager.
    Reveal,
    /// A URL or anything with a default handler.
    Default,
}

/// Opens `path` the way the OS does when you double-click it.
pub fn open(path: &Path, how: Open) -> std::io::Result<()> {
    use std::process::Command;
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = Command::new("open");
        match how {
            Open::Edit => c.arg("-t"),
            Open::Reveal => c.arg("-R"),
            Open::Default => &mut c,
        };
        c.arg(path);
        c
    };
    #[cfg(windows)]
    let mut cmd = match how {
        Open::Edit => {
            let mut c = Command::new("notepad.exe");
            c.arg(path);
            c
        }
        Open::Reveal => {
            let mut c = Command::new("explorer.exe");
            c.arg(format!("/select,{}", path.display()));
            c
        }
        Open::Default => {
            let mut c = Command::new("explorer.exe");
            c.arg(path);
            c
        }
    };
    #[cfg(not(any(target_os = "macos", windows)))]
    let mut cmd = {
        // xdg-open has no "reveal": open the folder instead.
        let target = match how {
            Open::Reveal => path.parent().unwrap_or(path),
            _ => path,
        };
        let mut c = Command::new("xdg-open");
        c.arg(target);
        c
    };
    cmd.spawn().map(|_| ())
}
