//! Puts text at the cursor in whatever app is focused.
//!
//! macOS, v1: clipboard paste. Snapshot every item and type on the pasteboard,
//! put our text there, press ⌘V, then restore the snapshot so the user's
//! clipboard is unchanged. If something else writes to the clipboard in the
//! meantime (the user copied), we don't restore over it. Posting ⌘V needs the
//! Accessibility permission.
//!
//! Later: AX `kAXSelectedText` first (clean undo, no clipboard at all), with
//! paste as the fallback (ARCHITECTURE.md §9).

use anyhow::Result;

/// Whether the process may type into other apps (Accessibility).
pub fn has_permission() -> bool {
    imp::has_permission()
}

/// Asks macOS for Accessibility; shows the system prompt the first time.
pub fn request_permission() -> bool {
    imp::request_permission()
}

/// Puts `text` on the clipboard (the MuskFlow window's Copy buttons).
pub fn copy(text: &str) -> Result<()> {
    imp::copy(text)
}

/// The focused app's selected text, fetched with ⌘C, for apps that don't expose it through
/// Accessibility. The user's clipboard is put back. `None` if nothing was copied.
pub fn copy_selection() -> Option<String> {
    imp::copy_selection()
}

/// Inserts `text` at the cursor of the focused app.
pub fn insert(text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    imp::insert(text)
}

#[cfg(target_os = "macos")]
mod imp {
    use std::sync::Mutex;
    use std::thread::JoinHandle;
    use std::time::Duration;

    use anyhow::{Result, bail};
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::{
        NSPasteboard, NSPasteboardItem, NSPasteboardTypeString, NSPasteboardWriting,
    };
    use objc2_core_graphics::{
        CGEvent, CGEventFlags, CGEventTapLocation, CGPreflightPostEventAccess,
        CGRequestPostEventAccess,
    };
    use objc2_foundation::{NSArray, NSData, NSString};

    /// kVK_ANSI_V, kVK_ANSI_C.
    const KEY_V: u16 = 9;
    const KEY_C: u16 = 8;
    /// How long an app gets to answer ⌘C.
    const COPY_WAIT: Duration = Duration::from_millis(300);
    /// How long the target app gets to read the pasteboard before we restore it.
    const PASTE_SETTLE: Duration = Duration::from_millis(250);

    pub fn has_permission() -> bool {
        CGPreflightPostEventAccess()
    }

    pub fn request_permission() -> bool {
        CGRequestPostEventAccess()
    }

    /// Pasteboard items as (type, bytes) pairs: plain data, so it can move to the restore thread.
    type Snapshot = Vec<Vec<(String, Vec<u8>)>>;

    /// The restore still running from the previous insert, if any.
    static PENDING_RESTORE: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);

    pub fn insert(text: &str) -> Result<()> {
        if !has_permission() {
            bail!(
                "MuskFlow can't type into other apps yet. Allow it in System Settings → Privacy & Security → Accessibility."
            );
        }
        // A back-to-back dictation must not snapshot our own text as "the user's clipboard".
        if let Some(prev) = PENDING_RESTORE.lock().unwrap().take() {
            let _ = prev.join();
        }
        let pb = NSPasteboard::generalPasteboard();
        let saved = snapshot(&pb);

        pb.clearContents();
        // SAFETY: NSPasteboardTypeString is an AppKit constant.
        pb.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString });
        let ours = pb.changeCount();
        press_cmd_v();

        // The text is in; give the app time to read the pasteboard, then put the user's
        // clipboard back without making the caller wait.
        *PENDING_RESTORE.lock().unwrap() = Some(std::thread::spawn(move || {
            std::thread::sleep(PASTE_SETTLE);
            let pb = NSPasteboard::generalPasteboard();
            if pb.changeCount() == ours {
                restore(&pb, &saved);
            } else {
                log::info!("clipboard changed during paste; not restoring it");
            }
        }));
        Ok(())
    }

    pub fn copy(text: &str) -> Result<()> {
        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        // SAFETY: NSPasteboardTypeString is an AppKit constant.
        pb.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString });
        Ok(())
    }

    fn snapshot(pb: &NSPasteboard) -> Snapshot {
        let Some(items) = pb.pasteboardItems() else {
            return Vec::new();
        };
        items
            .iter()
            .map(|item| {
                item.types()
                    .iter()
                    .filter_map(|t| item.dataForType(&t).map(|d| (t.to_string(), d.to_vec())))
                    .collect()
            })
            .collect()
    }

    fn restore(pb: &NSPasteboard, saved: &Snapshot) {
        pb.clearContents();
        if saved.is_empty() {
            return;
        }
        let items: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = saved
            .iter()
            .map(|types| {
                let item = NSPasteboardItem::new();
                for (t, d) in types {
                    item.setData_forType(&NSData::with_bytes(d), &NSString::from_str(t));
                }
                ProtocolObject::from_retained(item)
            })
            .collect();
        pb.writeObjects(&NSArray::from_retained_slice(&items));
    }

    pub fn copy_selection() -> Option<String> {
        if !has_permission() {
            return None;
        }
        if let Some(prev) = PENDING_RESTORE.lock().unwrap().take() {
            let _ = prev.join();
        }
        let pb = NSPasteboard::generalPasteboard();
        let saved = snapshot(&pb);
        let before = pb.changeCount();
        press_cmd(KEY_C);
        let t = std::time::Instant::now();
        while pb.changeCount() == before && t.elapsed() < COPY_WAIT {
            std::thread::sleep(Duration::from_millis(10));
        }
        if pb.changeCount() == before {
            return None; // nothing selected
        }
        // SAFETY: NSPasteboardTypeString is an AppKit constant.
        let text = pb
            .stringForType(unsafe { NSPasteboardTypeString })
            .map(|s| s.to_string());
        restore(&pb, &saved);
        text.filter(|t| !t.trim().is_empty())
    }

    fn press_cmd_v() {
        press_cmd(KEY_V);
    }

    fn press_cmd(key: u16) {
        for down in [true, false] {
            if let Some(ev) = CGEvent::new_keyboard_event(None, key, down) {
                CGEvent::set_flags(Some(&ev), CGEventFlags::MaskCommand);
                CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&ev));
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use anyhow::{Result, bail};

    pub fn has_permission() -> bool {
        false
    }

    pub fn request_permission() -> bool {
        false
    }

    pub fn insert(_text: &str) -> Result<()> {
        bail!("inserting text is macOS-only for now")
    }

    pub fn copy(_text: &str) -> Result<()> {
        bail!("the clipboard is macOS-only for now")
    }

    pub fn copy_selection() -> Option<String> {
        None
    }
}
