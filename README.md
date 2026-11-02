# MuskFlow

Local, open-source voice dictation that writes what you *meant*, not just what you said.
Fast · Accurate · Creative · Light. Runs entirely on your laptop.

> Early development. Today: streaming speech-to-text from a file or the microphone, via the `muskflow` CLI.
> See [ARCHITECTURE.md](ARCHITECTURE.md) for the full design and [docs/benchmarks.md](docs/benchmarks.md) for measurements.

## Quick start (macOS, Apple Silicon)

```bash
brew install rustup cmake && rustup default stable
./scripts/fetch-models.sh          # Standard tier: Silero VAD + Parakeet-TDT 0.6B v3 (~650 MB)
# or: ./scripts/fetch-models.sh lite standard max   (all three tiers, ~1.8 GB)
cargo build --release
```

Transcribe a file:

```bash
./target/release/muskflow transcribe some.wav
```

Replay a file as live speech and measure key-up → text latency:

```bash
./target/release/muskflow stream some.wav
```

Dictate from your mic (speak, then press Enter):

```bash
./target/release/muskflow listen
```

Compare all three tiers (accuracy, latency, CPU, size) on the benchmark set:

```bash
./scripts/make-bench-clips.sh      # macOS: synthesises 8 clips with references into bench/clips
./target/release/muskflow bench --show
```

Pick a tier for any command with `--asr <model dir>`. Tiers: **Lite** (Parakeet 110M), **Standard** (Parakeet 0.6B, default), **Max** (Whisper large-v3-turbo).

Useful flags: `--asr <model dir>`, `--threads N`, `--step` / `--context` (streaming tuning), `--hotwords a,b,c`.

## Layout

| Path | What |
|---|---|
| `crates/mf-audio` | mic capture (real-time safe), 16 kHz resampling, Silero VAD, WAV loading |
| `crates/mf-asr` | `Transcriber` / `Session` traits, Parakeet backends, LocalAgreement streaming committer |
| `crates/mf-models` | the Lite / Standard / Max tiers |
| `crates/mf-eval` | WER and text normalisation for benchmarks |
| `apps/cli` | the `muskflow` binary (`transcribe`, `stream`, `listen`, `devices`, `bench`) |
| `scripts/` | model download |

## License

[Apache-2.0](LICENSE). Models keep their own licences (Parakeet: CC-BY-4.0).
