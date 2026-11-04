//! How long from opening the mic to the first audio arriving: `cargo run --release --example mic_latency -- [mic]`.
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let device = std::env::args().nth(1);
    for _ in 0..5 {
        let (tx, rx) = crossbeam_channel::unbounded::<Vec<f32>>();
        let t = Instant::now();
        let mic = umless_audio::MicCapture::start(device.as_deref(), tx)?;
        let opened = t.elapsed();
        let _ = rx.recv()?;
        println!("opened in {opened:?}, first audio after {:?}", t.elapsed());
        mic.stop();
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    Ok(())
}
