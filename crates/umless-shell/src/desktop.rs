//! Windows and Linux shell: a tray icon with the same menu as the macOS menu bar, the
//! Umless window (the same HTML hub, in WebView2 on Windows and WebKitGTK on Linux), the
//! listening pill, start/finish sounds and Launch at Login.
//!
//! tao owns the main thread's event loop; the worker thread's [`Ui::set`] calls are sent
//! to it through an event-loop proxy. The page talks to the backend through the same
//! `window.webkit.messageHandlers.umless.postMessage` bridge as on macOS: a small shim
//! forwards it to wry's `window.ipc`.
//!
//! Wayland doesn't let apps place their windows, so there the pill shows wherever the
//! compositor puts it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use tao::dpi::{LogicalSize, PhysicalPosition};
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy, EventLoopWindowTarget};
use tao::window::{Window, WindowBuilder, WindowId};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder, TrayIconEvent};
use wry::{WebView, WebViewBuilder};

use super::{Config, State, Ui};

const PILL_HEIGHT: f64 = 36.0;
const PILL_MARGIN: f64 = 90.0;
const FLASH: Duration = Duration::from_millis(2500);

/// Makes the macOS bridge work on wry.
const SHIM: &str = r#"
window.webkit = window.webkit || {};
window.webkit.messageHandlers = { umless: { postMessage: function (m) { window.ipc.postMessage(m); } } };
"#;

const PILL_HTML: &str = r#"<!doctype html><html><head><meta charset="utf-8"><style>
html,body{margin:0;height:100%;background:transparent;overflow:hidden}
body{display:flex;align-items:center;justify-content:center;font:500 13px system-ui,-apple-system,"Segoe UI",Ubuntu,sans-serif}
#p{background:rgba(20,20,22,.92);color:#fff;border-radius:18px;height:36px;line-height:36px;padding:0 20px;white-space:nowrap}
</style></head><body><div id="p"></div><script>
window.setPill = function (t) { document.getElementById('p').textContent = t; };
</script></body></html>"#;

#[derive(Debug)]
enum UserEvent {
    State(State),
    /// Run this in the Umless window's page.
    Eval(String),
    Menu(MenuEvent),
    Tray(TrayIconEvent),
    /// Hide the pill if nothing newer was shown.
    HidePill(u64),
}

static PROXY: OnceLock<Mutex<EventLoopProxy<UserEvent>>> = OnceLock::new();
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn send(e: UserEvent) {
    if let Some(p) = PROXY.get() {
        let _ = p.lock().unwrap().send_event(e);
    }
}

pub fn set(state: State) {
    if PROXY.get().is_some() {
        send(UserEvent::State(state));
    } else {
        log::info!("{state:?}");
    }
}

struct Shell {
    config: Config,
    /// Lives as long as the process; each window's IPC handler borrows it.
    backend: &'static super::Backend,
    tray: Option<TrayIcon>,
    status: MenuItem,
    open: MenuItem,
    edit: MenuItem,
    login: CheckMenuItem,
    quit: MenuItem,
    window: Option<(Window, WebView)>,
    pill: Option<(Window, WebView)>,
}

pub fn run(mut config: Config, work: impl FnOnce(Ui) + Send + 'static) -> ! {
    #[cfg(target_os = "linux")]
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        // No desktop (SSH, a server): run without a tray.
        log::warn!("no display; running without the tray icon");
        work(Ui(()));
        std::process::exit(0);
    }
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let _ = PROXY.set(Mutex::new(event_loop.create_proxy()));
    MenuEvent::set_event_handler(Some(|e| send(UserEvent::Menu(e))));
    TrayIconEvent::set_event_handler(Some(|e| send(UserEvent::Tray(e))));

    let hold = config.hold_key.clone();
    let mut shell = Shell {
        status: MenuItem::new("Starting…", false, None),
        open: MenuItem::new("Open Umless…", true, None),
        edit: MenuItem::new("Edit Dictionary…", true, None),
        login: CheckMenuItem::new("Launch at Login", true, launch_at_login(), None),
        quit: MenuItem::new("Quit Umless", true, None),
        tray: None,
        window: None,
        pill: None,
        backend: Box::leak(Box::new(std::mem::replace(
            &mut config.backend,
            Box::new(|_| String::new()),
        ))),
        config,
    };
    let hint = MenuItem::new(
        format!("Hold {hold} to dictate · {} tier", shell.config.tier),
        false,
        None,
    );
    let mut work = Some(work);

    event_loop.run(move |event, target, flow| {
        *flow = ControlFlow::Wait;
        match event {
            Event::NewEvents(StartCause::Init) => {
                // Linux needs the tray made once GTK is running, inside the loop.
                let menu = Menu::new();
                let _ = menu.append_items(&[
                    &shell.status,
                    &hint,
                    &PredefinedMenuItem::separator(),
                    &shell.open,
                    &shell.edit,
                    &shell.login,
                    &PredefinedMenuItem::separator(),
                    &shell.quit,
                ]);
                match TrayIconBuilder::new()
                    .with_menu(Box::new(menu))
                    .with_tooltip("Umless")
                    .with_icon(icon(Icon::Working))
                    .build()
                {
                    Ok(t) => shell.tray = Some(t),
                    Err(e) => log::warn!("no tray icon: {e}"),
                }
                shell.pill = build_pill(target);
                apply(&mut shell, State::Loading);
                if shell.config.open_window {
                    open_window(&mut shell, target);
                }
                if let Some(work) = work.take() {
                    std::thread::Builder::new()
                        .name("umless".into())
                        .spawn(move || work(Ui(())))
                        .expect("could not start the dictation thread");
                }
            }
            Event::UserEvent(UserEvent::State(state)) => apply(&mut shell, state),
            Event::UserEvent(UserEvent::Eval(js)) => eval(&shell, &js),
            Event::UserEvent(UserEvent::HidePill(generation)) => {
                if GENERATION.load(Ordering::Relaxed) == generation
                    && let Some((w, _)) = &shell.pill
                {
                    w.set_visible(false);
                }
            }
            Event::UserEvent(UserEvent::Menu(e)) => {
                if e.id == shell.open.id() {
                    open_window(&mut shell, target);
                } else if e.id == shell.edit.id() {
                    open_in_editor(&shell.config.dictionary);
                } else if e.id == shell.login.id() {
                    let on = !launch_at_login();
                    if let Err(err) = set_launch_at_login(on) {
                        log::warn!("launch at login: {err:#}");
                    }
                    shell.login.set_checked(launch_at_login());
                } else if e.id == shell.quit.id() {
                    std::process::exit(0);
                }
            }
            // A left click on the tray icon opens the window (Windows convention).
            Event::UserEvent(UserEvent::Tray(TrayIconEvent::DoubleClick { .. })) => {
                open_window(&mut shell, target);
            }
            Event::UserEvent(UserEvent::Tray(_)) => {}
            Event::WindowEvent {
                window_id,
                event: WindowEvent::CloseRequested,
                ..
            } => {
                if is_window(&shell, window_id) {
                    // Closing only closes the window; Umless keeps running in the tray.
                    shell.window = None;
                }
            }
            _ => {}
        }
    })
}

fn is_window(shell: &Shell, id: WindowId) -> bool {
    shell.window.as_ref().is_some_and(|(w, _)| w.id() == id)
}

fn webview(builder: WebViewBuilder, window: &Window) -> wry::Result<WebView> {
    #[cfg(target_os = "linux")]
    {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        let vbox = window.default_vbox().expect("tao windows have a vbox");
        builder.build_gtk(vbox)
    }
    #[cfg(not(target_os = "linux"))]
    builder.build(window)
}

fn open_window(shell: &mut Shell, target: &EventLoopWindowTarget<UserEvent>) {
    if let Some((w, _)) = &shell.window {
        w.set_visible(true);
        w.set_focus();
        return;
    }
    let window = match WindowBuilder::new()
        .with_title("Umless")
        .with_inner_size(LogicalSize::new(980.0, 660.0))
        .with_min_inner_size(LogicalSize::new(760.0, 520.0))
        .build(target)
    {
        Ok(w) => w,
        Err(e) => {
            log::warn!("could not open the window: {e}");
            return;
        }
    };
    // The backend runs here on the main thread; its reply goes back through the loop.
    let backend = shell.backend;
    let builder = WebViewBuilder::new()
        .with_initialization_script(SHIM)
        .with_html(shell.config.html)
        .with_ipc_handler(move |req| {
            let reply = backend(req.body());
            send(UserEvent::Eval(format!("umless._reply({reply})")));
        });
    match webview(builder, &window) {
        Ok(web) => {
            window.set_focus();
            shell.window = Some((window, web));
        }
        Err(e) => log::warn!("could not create the web view: {e}"),
    }
}

fn eval(shell: &Shell, js: &str) {
    if let Some((_, web)) = &shell.window {
        let _ = web.evaluate_script(js);
    }
}

fn build_pill(target: &EventLoopWindowTarget<UserEvent>) -> Option<(Window, WebView)> {
    let builder = WindowBuilder::new()
        .with_title("Umless")
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top(true)
        .with_resizable(false)
        .with_focused(false)
        .with_visible(false)
        .with_inner_size(LogicalSize::new(240.0, PILL_HEIGHT));
    #[cfg(windows)]
    let builder = {
        use tao::platform::windows::WindowBuilderExtWindows;
        builder.with_skip_taskbar(true)
    };
    #[cfg(target_os = "linux")]
    let builder = {
        use tao::platform::unix::WindowBuilderExtUnix;
        builder.with_skip_taskbar(true)
    };
    let window = builder.build(target).ok()?;
    let web = webview(
        WebViewBuilder::new()
            .with_transparent(true)
            .with_html(PILL_HTML),
        &window,
    )
    .ok()?;
    Some((window, web))
}

fn show_pill(shell: &Shell, text: &str) {
    let Some((w, web)) = &shell.pill else { return };
    let js = format!("setPill({})", serde_json_string(text));
    let _ = web.evaluate_script(&js);
    // A rough fit: ~7.5 px per character at 13 px, plus padding.
    let width = (text.chars().count() as f64 * 7.5 + 48.0).max(120.0);
    w.set_inner_size(LogicalSize::new(width, PILL_HEIGHT));
    if let Some(m) = w.current_monitor().or_else(|| w.primary_monitor()) {
        let scale = m.scale_factor();
        let (pos, size) = (m.position(), m.size());
        let x = pos.x as f64 + (size.width as f64 - width * scale) / 2.0;
        let y = pos.y as f64 + size.height as f64 - (PILL_MARGIN + PILL_HEIGHT) * scale;
        w.set_outer_position(PhysicalPosition::new(x, y));
    }
    w.set_visible(true);
}

/// A JS string literal.
fn serde_json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 || c == '\u{2028}' || c == '\u{2029}' => {
                out.push_str(&format!("\\u{:04x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn apply(shell: &mut Shell, state: State) {
    let generation = GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    let (icon_kind, status, pill, sound): (Icon, String, Option<String>, Option<Sound>) =
        match &state {
            State::Loading => (Icon::Working, "Loading models…".into(), None, None),
            State::Setup => (
                Icon::Error,
                "Finish setup in the Umless window".into(),
                None,
                None,
            ),
            State::Ready => (Icon::Idle, "Ready".into(), None, None),
            State::Listening { command, style } => (
                Icon::Listening,
                format!("Listening · {style}"),
                Some(if *command {
                    "●  Command".to_string()
                } else {
                    format!("●  Listening · {style}")
                }),
                Some(Sound::Start),
            ),
            State::Working => (
                Icon::Working,
                "Writing…".into(),
                Some("Writing…".into()),
                None,
            ),
            State::Typed => (Icon::Idle, "Ready".into(), None, Some(Sound::Done)),
            State::Nothing(why) => (Icon::Idle, "Ready".into(), Some(why.clone()), None),
            State::Error(msg) => (
                Icon::Error,
                format!("⚠ {msg}"),
                Some(format!("⚠ {}", short(msg))),
                Some(Sound::Error),
            ),
        };
    if let Some(tray) = &shell.tray {
        let _ = tray.set_icon(Some(icon(icon_kind)));
        let _ = tray.set_tooltip(Some(format!("Umless · {}", short(&status))));
    }
    shell.status.set_text(short(&status));
    let js = format!(
        "window.umless && umless._event({}, {})",
        serde_json_string(event_name(&state)),
        serde_json_string(&status)
    );
    eval(shell, &js);
    match pill {
        Some(text) => show_pill(shell, &text),
        None => {
            if let Some((w, _)) = &shell.pill {
                w.set_visible(false);
            }
        }
    }
    if let Some(s) = sound {
        crate::sound::play(s.tone());
    }
    if matches!(state, State::Nothing(_) | State::Error(_)) {
        std::thread::spawn(move || {
            std::thread::sleep(FLASH);
            send(UserEvent::HidePill(generation));
        });
    }
}

#[derive(Clone, Copy)]
enum Sound {
    Start,
    Done,
    Error,
}

impl Sound {
    /// (frequency Hz, milliseconds)
    fn tone(self) -> &'static [(f32, u32)] {
        match self {
            Sound::Start => &[(880.0, 45), (1320.0, 55)],
            Sound::Done => &[(1320.0, 40), (990.0, 60)],
            Sound::Error => &[(330.0, 90), (220.0, 140)],
        }
    }
}

fn event_name(state: &State) -> &'static str {
    match state {
        State::Typed => "typed",
        State::Error(_) => "error",
        State::Setup => "setup",
        State::Ready => "ready",
        _ => "state",
    }
}

fn short(msg: &str) -> String {
    let first = msg.split(['.', '\n']).next().unwrap_or(msg);
    if first.chars().count() > 60 {
        format!("{}…", first.chars().take(60).collect::<String>())
    } else {
        first.to_string()
    }
}

#[derive(Clone, Copy)]
enum Icon {
    Idle,
    Listening,
    Working,
    Error,
}

/// Umless's mark, drawn into a 32×32 RGBA image: five rounded bars, hanging from the
/// top like an "M" when idle and centred into a waveform while listening, on a dark
/// rounded square so it reads on light and dark taskbars.
fn icon(kind: Icon) -> tray_icon::Icon {
    const S: usize = 32;
    let (heights, hang): ([f32; 5], bool) = match kind {
        Icon::Idle | Icon::Error => ([26.0, 14.0, 21.0, 14.0, 26.0], true),
        Icon::Listening => ([14.0, 26.0, 10.0, 26.0, 14.0], false),
        Icon::Working => ([18.0, 18.0, 18.0, 18.0, 18.0], false),
    };
    let bg: [u8; 3] = match kind {
        Icon::Listening => [214, 48, 49],
        Icon::Error => [214, 140, 0],
        _ => [32, 32, 36],
    };
    let mut rgba = vec![0u8; S * S * 4];
    let radius = 7.0f32;
    let inside_rounded = |x: f32, y: f32, x0: f32, y0: f32, x1: f32, y1: f32, r: f32| {
        let cx = x.clamp(x0 + r, x1 - r);
        let cy = y.clamp(y0 + r, y1 - r);
        (x - cx).powi(2) + (y - cy).powi(2) <= r * r
    };
    let (bar, gap) = (4.0f32, 1.6f32);
    let x_start = (S as f32 - (5.0 * bar + 4.0 * gap)) / 2.0;
    let tallest = 26.0f32;
    for py in 0..S {
        for px in 0..S {
            let (x, y) = (px as f32 + 0.5, py as f32 + 0.5);
            let i = (py * S + px) * 4;
            if !inside_rounded(x, y, 0.0, 0.0, S as f32, S as f32, radius) {
                continue;
            }
            let mut c = bg;
            for (b, h) in heights.iter().enumerate() {
                let x0 = x_start + b as f32 * (bar + gap);
                let y0 = if hang {
                    (S as f32 - tallest) / 2.0
                } else {
                    (S as f32 - h) / 2.0
                };
                if inside_rounded(x, y, x0, y0, x0 + bar, y0 + h, bar / 2.0) {
                    c = [255, 255, 255];
                }
            }
            rgba[i..i + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
        }
    }
    tray_icon::Icon::from_rgba(rgba, S as u32, S as u32).expect("valid icon size")
}

fn open_in_editor(path: &std::path::Path) {
    #[cfg(windows)]
    let r = std::process::Command::new("notepad.exe").arg(path).spawn();
    #[cfg(not(windows))]
    let r = std::process::Command::new("xdg-open").arg(path).spawn();
    if let Err(e) = r {
        log::warn!("could not open {}: {e}", path.display());
    }
}

// ---- Launch at Login ----------------------------------------------------------------

#[cfg(windows)]
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

pub fn launch_at_login() -> bool {
    #[cfg(windows)]
    {
        std::process::Command::new("reg")
            .args(["query", RUN_KEY, "/v", "Umless"])
            .output()
            .is_ok_and(|o| o.status.success())
    }
    #[cfg(not(windows))]
    autostart_file().is_some_and(|p| p.exists())
}

/// The command a login item runs: this binary with the models folder and tier spelled
/// out, since a process started at login has no useful working directory or environment.
fn login_command() -> anyhow::Result<String> {
    let exe = std::env::current_exe()?;
    let models = std::env::var_os("UMLESS_MODELS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("models"));
    let models = if models.is_absolute() {
        models
    } else {
        std::env::current_dir()?.join(models)
    };
    let mut cmd = format!("\"{}\" --models \"{}\"", exe.display(), models.display());
    if let Ok(tier) = std::env::var("UMLESS_TIER") {
        cmd.push_str(&format!(" --tier {tier}"));
    }
    cmd.push_str(" app");
    Ok(cmd)
}

pub fn set_launch_at_login(on: bool) -> anyhow::Result<()> {
    let line = login_command()?;
    #[cfg(windows)]
    {
        let mut cmd = std::process::Command::new("reg");
        if on {
            cmd.args([
                "add",
                RUN_KEY,
                "/v",
                "Umless",
                "/t",
                "REG_SZ",
                "/d",
                &line,
                "/f",
            ]);
        } else {
            cmd.args(["delete", RUN_KEY, "/v", "Umless", "/f"]);
        }
        let out = cmd.output()?;
        anyhow::ensure!(
            out.status.success(),
            "reg failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let path = autostart_file().ok_or_else(|| anyhow::anyhow!("no home folder"))?;
        if on {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(
                &path,
                format!(
                    "[Desktop Entry]\nType=Application\nName=Umless\nComment=Local voice dictation\nExec={line}\nIcon=umless\nX-GNOME-Autostart-enabled=true\n"
                ),
            )?;
        } else if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }
}

#[cfg(not(windows))]
fn autostart_file() -> Option<std::path::PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config"))
        })?;
    Some(config.join("autostart/umless.desktop"))
}
