# FreeFlow

Local, open-source voice dictation that writes what you *meant*, not just what you said.
Fast · Accurate · Creative · Light. Runs entirely on your laptop.

> Early development. Today: streaming speech-to-text from a file or the microphone, via the `freeflow` CLI.
> See [ARCHITECTURE.md](ARCHITECTURE.md) for the full design and [docs/benchmarks.md](docs/benchmarks.md) for measurements.

## Quick start (macOS, Apple Silicon)

```bash
brew install rustup cmake && rustup default stable
./scripts/fetch-models.sh          # Silero VAD + Parakeet-TDT 0.6B v3 (~650 MB)
cargo build --release
```

Transcribe a file:

```bash
./target/release/freeflow transcribe some.wav
```

Replay a file as live speech and measure key-up → text latency:

```bash
./target/release/freeflow stream some.wav
```

Dictate from your mic (speak, then press Enter):

```bash
./target/release/freeflow listen
```

Useful flags: `--asr <model dir>`, `--threads N`, `--step` / `--context` (streaming tuning), `--hotwords a,b,c`.

## Layout

| Path | What |
|---|---|
| `crates/ff-audio` | mic capture (real-time safe), 16 kHz resampling, Silero VAD, WAV loading |
| `crates/ff-asr` | `Transcriber` / `Session` traits, Parakeet backends, LocalAgreement streaming committer |
| `apps/cli` | the `freeflow` binary |
| `scripts/` | model download |

## License

Apache-2.0. Models keep their own licences (Parakeet: CC-BY-4.0).
