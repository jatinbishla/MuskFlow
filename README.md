# Umless

Local, open-source voice dictation that writes what you *meant*, not just what you said.
Fast · Accurate · Creative · Light. Runs entirely on your laptop.

> Early development. Today: streaming speech-to-text from a file or the microphone, via the `umless` CLI.
> See [ARCHITECTURE.md](ARCHITECTURE.md) for the full design and [docs/benchmarks.md](docs/benchmarks.md) for measurements.

## Install (one command)

Needs macOS 13 or later, 1–6 GB free (by tier) and an admin account. On a brand-new Mac, run `xcode-select --install` first: `git clone` needs Apple's command line tools.

```bash
git clone https://github.com/jatinbishla/Umless.git && cd Umless
./install.sh
```

That's all. The installer builds Umless (installing Rust and cmake first if you don't have them), suggests a tier for your machine, downloads exactly the models that tier needs, adds a local signing certificate to your login keychain (so updates keep their permissions), installs **/Applications/Umless.app** and opens it. Nothing else to clone or download. The app reads its models from this folder's `models/`, so keep the folder where it is (or set `UMLESS_MODELS` before installing). To skip the questions, pass the tier: `./install.sh lite`, `./install.sh standard`, `./install.sh max`, or `./install.sh all`.

| Tier | Download | Best for |
|---|---|---|
| **lite** | ~650 MB | any laptop; fastest (English) |
| **standard** | ~1.8 GB | the default; 25 European languages |
| **max** | ~3.1 GB | most accurate, 99 languages incl. Hindi; wants Apple Silicon / a GPU |

Add or check tiers any time:

```bash
./target/release/umless models install max      # or: lite, standard, all
./target/release/umless models list
```

## Use

**Dictate into any app (macOS):** hold **Fn**, speak, let go, and the cleaned text is typed at your cursor.

`install.sh` already set up the app. After pulling updates or building by hand, rebuild it with:

```bash
scripts/make-signing-cert.sh      # once: a local signing certificate, so updates keep their permissions
scripts/make-app.sh standard      # builds Umless.app and installs it in /Applications
```

Open **Umless** from Applications or Spotlight. It lives in the menu bar (no Dock icon). A pill at the bottom of the screen shows "Listening" while you hold Fn, with a sound at the start and when the text is typed. The menu has **Edit Dictionary…** (names and terms spelled your way, applied as soon as you save) and **Launch at Login**.

The first time, allow Umless under System Settings → Privacy & Security, then quit and reopen it:
- **Input Monitoring**, to see the Fn key.
- **Accessibility**, to type the text.
- **Microphone**, asked automatically.

Also set System Settings → Keyboard → "Press 🌐 key to" → **Do Nothing**, so Fn doesn't open the emoji picker too. Pressing another key while holding Fn (Fn+←, Fn+F5) cancels, so Fn's shortcuts keep working. **Fn + Control is command mode:** say what to write instead of the text itself. With nothing selected, Umless writes at the cursor using what's on screen ("reply saying thanks, I'll send it Thursday" in a Gmail thread). With text selected, it rewrites the selection ("make this more polite", "turn this into bullet points").

**It reads the screen, locally:** while you speak, Umless reads the text around your cursor through Accessibility. It uses it to spell names the way the thread does (you say "pria", it types Priya), to match the conversation's tone (a jokey Teams chat gets casual text, not formal), and as command mode's context. That text never leaves your Mac and is never saved.

**It learns from your fixes:** if you correct a name Umless typed, it adds your spelling to the dictionary. Turn either off in Settings. **It writes for where you are:** an AI prompt in Claude, ChatGPT, Codex, Cursor or a terminal; polished sentences in Mail, Gmail, Outlook and Teams; your own casual voice in Slack, WhatsApp and Messages; plain cleanup everywhere else. Open Umless from the menu bar to see your history, edit your dictionary or fix one style. Your clipboard is restored after each dictation. Models stay loaded; the mic is on only while Fn is held. Log: `~/Library/Logs/Umless.log`.

To remove Umless completely (app, permission entries, history, dictionary, settings, log and signing certificate), run `scripts/uninstall.sh`. Add `--keep-data` to keep your history and dictionary.

From a terminal, `./target/release/umless app` does the same. Grant the permissions to the terminal app instead.

**Try it in the terminal** (speak, then press Enter):

```bash
./target/release/umless listen --tier standard
```

`--tier` works on every command. Other commands (`umless` is `./target/release/umless`; to type just `umless`, run `ln -s "$PWD/target/release/umless" /usr/local/bin/umless`):

| Command | What it does |
|---|---|
| `umless transcribe file.wav` | one-shot transcription of a file |
| `umless stream file.wav` | replays a file as live speech and measures key-up → text latency |
| `umless devices` / `listen --mic "<name>"` | list mics / pick one |
| `umless listen --save recordings/x.wav` | keep the recording for replay benchmarks |
| `umless clean "text"` | runs the cleanup LLM on any text (`--style clean, formal, casual or prompt`) |
| `umless command "reply saying yes" --screen thread.txt` | command mode without the keyboard |
| `umless app` | the Fn-key app, run from a terminal |
| `umless models install <tier>` / `models list` | download or list models |
| `umless bench --show` | runs every clip through every installed tier and compares accuracy, latency, CPU and size (make the clips first with `scripts/make-bench-clips.sh`) |

### Your dictionary

Names, products and jargon get spelled your way on every tier. Put one term per line in a text file and pass it with `--dictionary my-words.txt` (or set `UMLESS_DICTIONARY`), or list a few inline with `--hotwords "SubFlo,Jatin,lien waivers"`:

```
# my-words.txt
Jatin Bishla
SubFlo
lien waivers
```

The speech model usually *hears* a rare name right and only *spells* it wrong ("Subflow", "firman", "Zirodhaa"), so Umless respells near misses that sound the same, before and after the cleanup LLM. Ordinary words that merely look similar ("former", "subway") are left alone. `--hotword-boost 1.5` additionally biases the Parakeet speech model (Lite, Standard) toward the terms; above ~2 it starts inventing them.

Benchmark clips: `./scripts/make-bench-clips.sh` (macOS) synthesises 11 clips with references, and `./scripts/record-passages.sh` records *you* reading 6 passages, so `umless bench --clips recordings` tests your real voice.

Tuning flags: `--threads N`, `--step` / `--context` (streaming), `--asr <model dir>` (override the tier's model).

## Layout

| Path | What |
|---|---|
| `crates/umless-audio` | mic capture (real-time safe), 16 kHz resampling, Silero VAD, WAV loading |
| `crates/umless-asr` | `Transcriber` / `Session` traits, Parakeet backends, LocalAgreement streaming committer |
| `crates/umless-models` | the Lite / Standard / Max tiers |
| `crates/umless-llm` | llama.cpp runtime: prefix-state cache, prompt-lookup speculative decoding |
| `crates/umless-cleanup` | rules, LLM prompt, faithfulness guard |
| `crates/umless-eval` | WER and text normalisation for benchmarks |
| `apps/cli` | the `umless` binary (`app`, `listen`, `command`, `clean`, `transcribe`, `stream`, `devices`, `models`, `bench`) |
| `install.sh` | build + model download, by tier |
| `scripts/` | benchmark clip synthesis and passage recording |

## License

[Apache-2.0](LICENSE). Models keep their own licences (Parakeet: CC-BY-4.0, Whisper: MIT, Qwen3/Qwen3.5: Apache-2.0).
