//! Prints what MuskFlow sees: the focused app, its window, the style it picks, and the
//! text on screen (selection, text before the cursor, window text, names).
//! `cargo run -p mf-context --example focus -- 3` waits 3 s first, so you can switch apps.
fn main() {
    let wait: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    std::thread::sleep(std::time::Duration::from_secs(wait));
    let Some(f) = mf_context::focused() else {
        println!("no frontmost app");
        return;
    };
    let t = std::time::Instant::now();
    let screen = mf_context::screen(f.pid);
    println!(
        "{} · {:?} → {} ({:?})",
        f.label(),
        f.title,
        f.style_for(&screen).label(),
        f.bundle_id
    );
    println!("read in {:?}", t.elapsed());
    println!("selection: {:?}", screen.selection);
    println!("before cursor: {:?}", screen.before);
    println!("window ({} chars):\n{}", screen.window.len(), screen.window);
    println!("names: {:?}", mf_context::text::names(&screen.all_text()));
}
