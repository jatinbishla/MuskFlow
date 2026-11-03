//! Prints what MuskFlow sees as the focused app, its window and the style it picks.
//! `cargo run -p mf-context --example focus`
fn main() {
    match mf_context::focused() {
        Some(f) => println!(
            "{} · {:?} → {} ({:?})",
            f.label(),
            f.title,
            f.style().label(),
            f.bundle_id
        ),
        None => println!("no frontmost app"),
    }
}
