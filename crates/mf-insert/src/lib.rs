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

/// Inserts `text` at the cursor of the focused app.
pub fn insert(text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    imp::insert(text)
}

#[cfg(target_os = "macos")]
mod imp {
    use std::time::Duration;

    use anyhow::{Result, bail};
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::{
        NSPasteboard, NSPasteboardItem, NSPasteboardType, NSPasteboardTypeString,
        NSPasteboardWriting,
    };
    use objc2_core_graphics::{
        CGEvent, CGEventFlags, CGEventTapLocation, CGPreflightPostEventAccess,
        CGRequestPostEventAccess,
    };
    use objc2_foundation::{NSArray, NSData, NSString};

    /// kVK_ANSI_V.
    const KEY_V: u16 = 9;
    /// How long the target app gets to read the pasteboard before we restore it.
    const PASTE_SETTLE: Duration = Duration::from_millis(250);

    pub fn has_permission() -> bool {
        CGPreflightPostEventAccess()
    }

    pub fn request_permission() -> bool {
        CGRequestPostEventAccess()
    }

    type Snapshot = Vec<Vec<(Retained<NSPasteboardType>, Retained<NSData>)>>;

    pub fn insert(text: &str) -> Result<()> {
        if !has_permission() {
            bail!(
                "MuskFlow can't type into other apps yet. Allow it in System Settings → Privacy & Security → Accessibility."
            );
        }
        let pb = NSPasteboard::generalPasteboard();
        let saved = snapshot(&pb);

        pb.clearContents();
        // SAFETY: NSPasteboardTypeString is an AppKit constant.
        pb.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString });
        let ours = pb.changeCount();

        press_cmd_v();
        std::thread::sleep(PASTE_SETTLE);

        if pb.changeCount() == ours {
            restore(&pb, &saved);
        } else {
            log::info!("clipboard changed during paste; not restoring it");
        }
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
                    .filter_map(|t| item.dataForType(&t).map(|d| (t, d)))
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
                    item.setData_forType(d, t);
                }
                ProtocolObject::from_retained(item)
            })
            .collect();
        pb.writeObjects(&NSArray::from_retained_slice(&items));
    }

    fn press_cmd_v() {
        for down in [true, false] {
            if let Some(ev) = CGEvent::new_keyboard_event(None, KEY_V, down) {
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
}
