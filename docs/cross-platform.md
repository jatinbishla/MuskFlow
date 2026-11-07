# Umless on macOS, Windows and Linux

The speech, cleanup, model and benchmark crates (`umless-audio`, `-asr`, `-llm`, `-cleanup`,
`-models`, `-eval`) are platform-neutral. Four crates talk to the OS, each behind the same
public API on every platform, so `apps/cli` has almost no `cfg`s:

| Crate | macOS | Windows | Linux |
|---|---|---|---|
| `umless-hotkey` | Fn via a listen-only CGEventTap (Input Monitoring) | hold key via `SetWindowsHookExW(WH_KEYBOARD_LL)` | hold key via evdev (`/dev/input`, `input` group); XRecord on X11 as fallback |
| `umless-insert` | NSPasteboard snapshot + ⌘V (Accessibility) | clipboard snapshot (all memory formats) + `SendInput` Ctrl+V | `wl-copy`/`wl-paste` + `wtype` or `ydotool` (Wayland); `xclip` + `xdotool` (X11) |
| `umless-context` | frontmost app, window title, screen text via AX | foreground window, exe name, title | Hyprland, Sway/i3, KDE (`kdotool`), X11 `_NET_ACTIVE_WINDOW`; title only |
| `umless-shell` | AppKit menu bar, WKWebView window, NSPanel pill | tao + tray-icon + wry (WebView2) | tao + tray-icon (AppIndicator) + wry (WebKitGTK) |
| `umless-audio` permission | AVCaptureDevice | none needed | none needed |

## What works where

| | macOS | Windows | Linux |
|---|---|---|---|
| `umless listen / transcribe / stream / clean / command / bench / models` | ✅ | ✅ CI | ✅ CI |
| All three tiers (Lite / Standard / Max) | ✅ | ✅ CI (`Tiers` workflow) | ✅ CI (`Tiers` workflow) |
| LLM on GPU | Metal | CPU; `--features cuda` / `vulkan` | CPU; `--features vulkan` / `cuda` / `rocm` |
| Hold-to-dictate key | Fn (+ Control: command) | Ctrl + Win (+ Alt) | Ctrl + Super (+ Alt) |
| Typing at the cursor, clipboard restored | ✅ | ✅ (not into elevated windows) | ✅ (terminals get Ctrl+Shift+V) |
| Per-app style (AI prompt / formal / casual) | ✅ | ✅ (exe name + title) | ✅ except GNOME Wayland |
| Reading screen text (names, tone, command context) | ✅ | ❌ TODO (UI Automation) | ❌ TODO (AT-SPI) |
| Selection for command mode | AX, else ⌘C | Ctrl+C | PRIMARY selection |
| Tray/menu-bar icon, window, pill, sounds | ✅ | ✅ | ✅ (pill position is the compositor's on Wayland) |
| Launch at login | SMAppService | `HKCU\…\Run` | `~/.config/autostart/umless.desktop` |
| Learning from corrections | ✅ | ❌ (needs field text) | ❌ (needs field text) |
| Installer | `install.sh` → Umless.app | `install.ps1` → `%LOCALAPPDATA%\Programs\Umless` + Start menu | `install.sh` → `~/.local/bin/umless` + app menu |

"CI" means GitHub Actions builds and runs it on that OS (`ci.yml`: build, unit tests,
headless check, installer syntax, CLI smoke test; `tiers.yml`: downloads Lite, Standard and
Max and runs `umless bench` on the synthetic clips on every OS). The desktop pieces on
Windows and Linux (hook, paste, tray, window, pill) compile in CI but have not yet been
exercised on a real Windows or Linux desktop: see "Not yet tested" below.

## The hold key on Windows and Linux

Neither OS lets software see Fn, so Umless uses a chord, parsed by `HoldKey` from
`UMLESS_HOLD_KEY` (default `ctrl+win`; also `right-ctrl`, `right-alt`, `ctrl+alt`,
`ctrl+shift`). Holding Alt with it is command mode (Shift if the hold key uses Alt). Any
other key while holding cancels, as on macOS. `chord.rs` turns key presses into the same
signals the macOS tap feeds `Tracker`, and is unit-tested.

Windows-specific: releasing Win would open the Start menu, so when the chord goes down
Umless sends an unassigned virtual key (0xE8), which makes Windows treat Win as part of a
shortcut. Injected keys (our own Ctrl+V) are ignored by the hook.

Linux-specific: Wayland compositors don't offer global key hooks, so Umless opens every
keyboard under `/dev/input` (rescanning every 5 s for new ones). That requires membership of
the `input` group. If no device is readable and `DISPLAY` is set, it uses the X11 RECORD
extension instead (works on X11 sessions, and for XWayland windows only on Wayland).

## Remaining work, per crate

**`umless-context`, screen text.**
- Windows: `IUIAutomation::GetFocusedElement` (the `windows` crate, `Win32_UI_Accessibility`),
  then `TextPattern2::GetCaretRange` / `TextPattern::GetSelection` for the selection and text
  before the cursor, `ValuePattern` for plain fields, and a bounded `TreeWalker` walk of the
  window for visible text, mirroring the AX walk limits in `lib.rs` (4,000 nodes, 300 ms).
  `field_text` then comes from the same focused element, which turns on learning.
- Linux: the `atspi` crate over D-Bus: focused accessible via `org.a11y.atspi.Event.Focus`
  events (the registry has no "get focused" call), `Text.GetText` / `GetSelection` /
  `GetCaretOffset`, and a bounded child walk for the window. Chromium and Electron apps
  expose AT-SPI only when `ACCESSIBILITY_ENABLED=1` or a screen reader is detected.
- GNOME Wayland focus: no public API. Options: a tiny GNOME Shell extension exposing
  `global.display.focus_window` over D-Bus (as `window-calls` does), or the AT-SPI focused
  application's name.

**`umless-hotkey`.**
- Linux: evdev devices are read but not grabbed, which is right (keys must reach apps), but
  a hold-key setting in the Settings page (instead of only `UMLESS_HOLD_KEY`) is still to do.
- Optionally the `org.freedesktop.portal.GlobalShortcuts` portal (KDE 6, GNOME 48+) as a
  no-`input`-group path; it gives press/release of a user-bound shortcut, which is enough.

**`umless-insert`.**
- Windows: `UIA ValuePattern::SetValue` / `TextPattern` insertion first (no clipboard),
  paste as the fallback, as planned for AX on macOS.
- Linux: restoring keeps one representation (image or text), not every MIME type.

**`umless-shell`.**
- Windows: an app icon resource (`.ico` via `winres`) and an installer-free MSIX/MSI
  package; today `install.ps1` builds from source.
- Linux: a `.deb`/Flatpak. A Flatpak needs `--device=input` (or the portal above) and
  `--talk-name=org.a11y.Bus`.
- The pill is a small transparent web view; on Wayland the compositor places it.

## Not yet tested (no Windows or Linux desktop was available)

Built and unit-tested in CI on all three OSes, and the CLI (all tiers) runs there. Not yet
run by hand on a real desktop:

- Windows: the keyboard hook and Start-menu suppression, Ctrl+V paste and clipboard
  restore, the tray icon/menu, the WebView2 window and its IPC bridge, the pill, the tones,
  the Run-key login item, `install.ps1` end to end (CI only parses it).
- Linux: evdev and XRecord key capture, paste with wtype/ydotool/xdotool, focus detection on
  each compositor, the AppIndicator tray (GNOME needs the AppIndicator extension), the
  WebKitGTK window, the pill, autostart, `install.sh` on Linux end to end.

## Build notes

- Windows: sherpa-onnx's prebuilt *static* libraries use the static C runtime (/MT) while
  llama.cpp builds with the dynamic one (/MD), which fails to link (LNK2038). So on Windows
  sherpa-onnx is linked as DLLs (its `shared` feature; `umless-asr` / `umless-audio` target
  sections). Its build script copies `sherpa-onnx-c-api.dll` and `onnxruntime.dll` into
  `target\release`, and `install.ps1` ships them next to `umless.exe`. (`LLAMA_STATIC_CRT=1`
  doesn't help: llama.cpp's CMake runtime-library policy overrides cc's `/MT`.)
- Linux: the desktop app needs `libgtk-3-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev`
  (and `libasound2-dev`, `clang`, `cmake` for the core). `--no-default-features` drops the GUI.
- GitHub retired the `macos-14` runners; CI uses `macos-15`.
