# MuskFlow

Local, open-source voice dictation that writes what you *meant*, not just what you said.
Fast · Accurate · Creative · Light. Runs entirely on your laptop.

> Early development. Today: streaming speech-to-text from a file or the microphone, via the `muskflow` CLI.
> See [ARCHITECTURE.md](ARCHITECTURE.md) for the full design and [docs/benchmarks.md](docs/benchmarks.md) for measurements.

## Install (one command)

```bash
git clone https://github.com/jatinbishla/MuskFlow.git && cd MuskFlow
./install.sh
```

That's all. The installer builds MuskFlow (installing Rust first if you don't have it), suggests a tier for your machine, and downloads exactly the models that tier needs. Nothing else to clone or download. To skip the questions, pass the tier: `./install.sh lite`, `./install.sh standard`, `./install.sh max`, or `./install.sh all`.

| Tier | Download | Best for |
|---|---|---|
| **lite** | ~650 MB | any laptop; fastest (English) |
| **standard** | ~1.8 GB | the default; 25 European languages |
| **max** | ~3.1 GB | most accurate, 99 languages incl. Hindi; wants Apple Silicon / a GPU |

Add or check tiers any time:

```bash
./target/release/muskflow models install max      # or: lite, standard, all
./target/release/muskflow models list
```

## Use

Dictate from your mic (speak, then press Enter):

```bash
./target/release/muskflow listen --tier standard
```

`--tier` works on every command. Other commands:

| Command | What it does |
|---|---|
| `muskflow transcribe file.wav` | one-shot transcription of a file |
| `muskflow stream file.wav` | replays a file as live speech and measures key-up → text latency |
| `muskflow devices` / `listen --mic "<name>"` | list mics / pick one |
| `muskflow listen --save recordings/x.wav` | keep the recording for replay benchmarks |
| `muskflow clean "text"` | runs the cleanup LLM on any text |
| `muskflow bench --show` | runs every clip through every installed tier and compares accuracy, latency, CPU and size |

Benchmark clips: `./scripts/make-bench-clips.sh` (macOS) synthesises 8 clips with references, and `./scripts/record-passages.sh` records *you* reading 6 passages, so `muskflow bench --clips recordings` tests your real voice.

Tuning flags: `--threads N`, `--step` / `--context` (streaming), `--hotwords a,b,c`, `--asr <model dir>` (override the tier's model).

## Layout

| Path | What |
|---|---|
| `crates/mf-audio` | mic capture (real-time safe), 16 kHz resampling, Silero VAD, WAV loading |
| `crates/mf-asr` | `Transcriber` / `Session` traits, Parakeet backends, LocalAgreement streaming committer |
| `crates/mf-models` | the Lite / Standard / Max tiers |
| `crates/mf-llm` | llama.cpp runtime: prefix-state cache, prompt-lookup speculative decoding |
| `crates/mf-cleanup` | rules, LLM prompt, faithfulness guard |
| `crates/mf-eval` | WER and text normalisation for benchmarks |
| `apps/cli` | the `muskflow` binary (`transcribe`, `stream`, `listen`, `devices`, `bench`) |
| `install.sh` | build + model download, by tier |
| `scripts/` | benchmark clip synthesis and passage recording |

## License

[Apache-2.0](LICENSE). Models keep their own licences (Parakeet: CC-BY-4.0, Whisper: MIT, Qwen3/Qwen3.5: Apache-2.0).
