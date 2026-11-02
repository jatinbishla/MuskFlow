# MuskFlow — Architecture

> Local, open-source voice dictation that writes what you *meant*, not just what you said.
> Fast · Accurate · Creative · Light. Runs entirely on your laptop.

Status: design v1 · 3 Nov 2026 · Working name "MuskFlow" (placeholder)

---

## 0. Design goals (the bar we hold ourselves to)

| Goal | Concrete target (Standard tier, M1 8 GB) | Wispr Flow (reference) |
|---|---|---|
| **Fast** | key-up → text inserted **p50 ≤ 350 ms, p99 ≤ 600 ms** | p99 < 700 ms, needs network |
| **Accurate** | final-text WER ≤ raw-ASR WER − 30 %; hallucinated content words < 0.5 % | closed, unknown |
| **Faithful** | never answers, never invents; "edit don't rewrite" enforced by a guard | can over-rewrite |
| **Creative** | Polish + Command modes: rewrite, restructure, change tone on request | Command Mode (paid) |
| **Light** | idle RAM < 60 MB, idle CPU ≈ 0 %, app download < 25 MB, models 0.6–2 GB by tier | cloud |
| **Private** | zero network after model download; no telemetry by default; screen context never persisted | cloud-only |
| **Open** | Apache-2.0 code, open models, open dataset recipe, open benchmark | closed |

Non-goals for v1: meeting transcription, speaker diarization, long-form (> 3 min) dictation, mobile.

---

## 1. System overview

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ Desktop app (Tauri shell: tray, settings, overlay pill)                      │
│   ▲ events (state, partial text, levels)        │ commands (config, models)  │
│   │                                             ▼                            │
│ ┌──────────────────────────── muskflow-core (Rust) ─────────────────────────┐│
│ │                                                                           ││
│ │  Hotkey ─► Session State Machine ◄──────────────── Config / Profiles      ││
│ │               │                                                           ││
│ │   ┌───────────┼─────────────┬──────────────────┬──────────────┐           ││
│ │   ▼           ▼             ▼                  ▼              ▼           ││
│ │ Audio ──► VAD ──► ASR ──► Transcript ──► Router ──► Cleanup ──► Guard     ││
│ │ (cpal)  (Silero) (stream) Committer       │       (LLM)        │          ││
│ │                    ▲                      │                    ▼          ││
│ │                    │ hotwords             └──► Rules ─────► Inserter      ││
│ │                    │                                           │          ││
│ │ Context Engine ────┴──── entities ──► Cleanup prompt           ▼          ││
│ │ (AX / UIA / AT-SPI)                                       Learner         ││
│ │                                                          (SQLite)         ││
│ │  Model Manager · Hardware Probe · Resource Governor · Metrics             ││
│ └───────────────────────────────────────────────────────────────────────────┘│
└──────────────────────────────────────────────────────────────────────────────┘
        ▲ downloads once (HF, sha256-pinned)          No other network.
```

One process. The core is a Rust library linked into the Tauri app and also into a headless `muskflow-cli` (for evals, scripting, and Linux users who want no UI).

---

## 2. Repository layout

```
muskflow/
├── ARCHITECTURE.md
├── crates/
│   ├── mf-core/        # session state machine, pipeline orchestration, metrics
│   ├── mf-audio/       # capture, resample, ring buffer, VAD, pre-roll
│   ├── mf-asr/         # Transcriber trait + backends (sherpa-onnx, whisper.cpp), committer
│   ├── mf-context/     # ContextProvider trait + macos/windows/linux impls, entity extraction
│   ├── mf-llm/         # llama.cpp wrapper, KV-prefix cache, prompt-lookup speculative decode
│   ├── mf-cleanup/     # router, prompt builder, rule passes, modes, guard
│   ├── mf-insert/      # Inserter trait + per-OS impls
│   ├── mf-learn/       # edit watcher, dictionary, style stats, SQLite store
│   ├── mf-models/      # manifest, downloader, hardware probe, tier selection
│   └── mf-config/      # TOML config, per-app profiles, hot reload
├── apps/
│   ├── desktop/        # Tauri 2 app (Rust + Svelte/TS UI)
│   └── cli/            # muskflow-cli: record/transcribe/clean/eval/bench
├── models/
│   └── manifest.json   # model registry (urls, sha256, sizes, licenses, tiers)
├── training/           # Python: data generation, fine-tuning, export to GGUF/ONNX
│   ├── datagen/
│   ├── finetune/
│   └── export/
├── eval/               # MuskFlow-Bench: audio + context + gold, scorers, report
└── docs/
```

---

## 3. The session pipeline (the hot path)

### 3.1 State machine (`mf-core`)

```
 Idle ──key down──► Arming ──mic open──► Listening ──key up / VAD end──► Finalizing
  ▲                                         │ (stream ASR, prefill LLM)       │
  │                                     Esc/cancel                            ▼
  │                                         │                       Cleaning (LLM / rules)
  │                                         ▼                                 │
  └──── Learning (≤15 s, background) ◄── Inserting ◄──── Guarding ◄───────────┘
```

- Hotkey modes: **hold-to-talk** (default), **toggle** (tap on / tap off), **hands-free** (toggle + VAD auto-stop after 1.5 s silence).
- Double-tap hotkey = **Command mode** (see §7). Esc = cancel at any point; nothing is inserted.
- Every transition is timestamped → per-session latency breakdown (in-app debug panel + `--bench`).

### 3.2 Critical-path timing (where the speed comes from)

Most of the work happens **while the user is still speaking**. On key-up only the last bit is left.

| While speaking (off critical path) | After key-up (critical path) | Budget |
|---|---|---|
| Audio stream + VAD | ASR on uncommitted tail (≤ ~3 s audio) | 60–150 ms |
| Streaming ASR commits stable text | Append tail to LLM KV cache (prefill) | 15–40 ms |
| Context grab + entity extraction | Decode with prompt-lookup speculation | 120–220 ms |
| **LLM prefills system prompt + context + committed text incrementally** | Guard (word alignment) | < 5 ms |
| Model warm (loaded, KV prefix cached) | Insert | 10–40 ms |
| | **Total** | **~250–450 ms** |

Three tricks make this work. Most dictation tools use none of them:

1. **Streaming commit.** Text that is already stable is finalised while the user talks.
2. **Incremental LLM prefill.** The cleanup prompt is built and fed into the KV cache *as committed text arrives*, so on key-up only a few dozen tokens are left to prefill.
3. **Prompt-lookup speculative decoding.** Cleanup output copies 85–95 % of its input. We draft the next k tokens by matching n-grams from the raw transcript, verify them in one batched forward pass, and accept the longest correct prefix. That gives 2–4× decode speed with no extra model.

---

## 4. Audio (`mf-audio`)

- **Capture:** `cpal`, device default or user-picked; mono; resampled to 16 kHz f32 with `rubato`.
- **Mic warm-up:** opening a device on macOS can take 100–300 ms. Options:
  - `warm_mic = "on_hotkey"` (default): open on key-down and keep a **300 ms pre-roll** once the stream is up (handles "first syllable clipped").
  - `warm_mic = "always"`: keep the stream open, but **discard audio** unless armed. The mic indicator stays on, so this is opt-in.
- **Ring buffer:** lock-free SPSC (`rtrb`), 30 s capacity; the audio thread never allocates or locks.
- **VAD:** Silero VAD v5 (ONNX, ~2 MB) on 32 ms frames. It is used to:
  - trim leading/trailing silence (fewer tokens means faster ASR),
  - auto-stop in hands-free mode,
  - drop sessions with no speech (no empty insertions),
  - pick segment boundaries for the committer.
- **Levels:** RMS per frame is sent to the overlay pill for the waveform.
- **Guards:** max session length 5 min (configurable); macOS Secure Input active means refuse to listen (password fields).

---

## 5. Speech recognition (`mf-asr`)

### 5.1 Interface

```rust
trait Transcriber: Send {
    fn caps(&self) -> AsrCaps;                // streaming?, hotwords?, languages, timestamps
    fn set_hotwords(&mut self, words: &[Hotword]);  // boosted terms from context + dictionary
    fn push_audio(&mut self, pcm: &[f32]);     // streaming backends
    fn partial(&mut self) -> Option<Hypothesis>;   // words + timestamps + confidence
    fn finalize(&mut self) -> Hypothesis;
    fn transcribe(&mut self, pcm: &[f32], opts: &DecodeOpts) -> Hypothesis; // offline backends
}
```

### 5.2 Backends

| Backend | Runtime | Models | Mode |
|---|---|---|---|
| `sherpa` | sherpa-onnx (C API, CoreML/CUDA/DirectML/CPU EPs) | Parakeet-TDT-0.6B-v3 (int8), Nemotron streaming 0.6B, Moonshine | offline + native streaming, **hotword biasing** |
| `whisper` | whisper.cpp (Metal/CUDA/Vulkan/CPU) | large-v3-turbo (q5), small/base for Lite | offline; context via `initial_prompt` |

### 5.3 Streaming committer (for offline models like Parakeet/Whisper)

Re-decode a sliding window every ~500 ms of new audio using a **LocalAgreement-2** policy: a word is *committed* once two consecutive decodes agree on it (with timestamps). The window then slides past the committed audio. This gives:

- live partial text in the overlay (users trust what they can see),
- at key-up, only the uncommitted tail (usually < 3 s) needs decoding,
- committed text streams into the LLM prefill (§3.2).

Native streaming models (Nemotron, Moonshine streaming) skip the committer and emit finals directly.

### 5.4 Context biasing (accuracy before cleanup)

Hotwords = top-N entities from the Context Engine + the user dictionary. They are passed as:

- sherpa-onnx **contextual biasing** (Aho-Corasick boosting during beam search),
- Whisper `initial_prompt` ("Priya, Kubernetes, SubFlo, Q3 roadmap.").

This fixes names *at the source*. Wispr fixes them afterwards with the LLM; we do both.

### 5.5 Language

- Auto-detect on the first 2 s (Whisper) or user-pinned.
- A **Hinglish / code-switch** profile pins the Whisper backend + a Romanised output preference. Hinglish is a first-class eval slice.

---

## 6. Context engine (`mf-context`)

### 6.1 What we capture (only at key-down, only in memory)

```rust
struct Context {
    app: AppId,              // bundle id / exe name, e.g. "com.tinyspeck.slackmacgap"
    app_kind: AppKind,       // Chat | Email | Doc | Code | Terminal | Browser | Notes | Unknown
    window_title: String,
    field_before: String,    // ≤ 1500 chars before caret in the focused field
    field_after: String,     // ≤ 300 chars after caret
    selection: Option<String>,
    url_host: Option<String>,// browsers only, via AX/UIA (e.g. "mail.google.com" → Email)
    is_secure: bool,         // password field ⇒ abort session
}
```

### 6.2 Providers

| OS | API | Notes |
|---|---|---|
| macOS | Accessibility (`AXUIElement`: focused element, `kAXValue`, `kAXSelectedTextRange`) + NSWorkspace frontmost app | Needs Accessibility permission. Electron/Chromium need `AXEnhancedUserInterface` / `AXManualAccessibility`, set once per app |
| Windows | UI Automation (`TextPattern`, `ValuePattern`, focused element) | Works for most Win32/UWP/Chromium |
| Linux | AT-SPI2 over D-Bus | Best-effort; GNOME/KDE fine, some toolkits empty |

**No screenshots or OCR in v1.** They are slower, heavier and creepier. An optional OCR provider (Apple Vision / Windows.Media.Ocr) can come later for apps that expose nothing.

### 6.3 Entity extraction (fast, < 3 ms, no ML)

From `window_title + field_before + field_after`, pull:

- capitalised non-sentence-initial tokens,
- camelCase / snake_case identifiers,
- @handles, emails, URLs,
- acronyms,
- words not in a 50k common-word list.

Score them by recency (closer to caret is better) and frequency. Keep the top 30 as ASR hotwords and the top 15 for the LLM prompt.

### 6.4 Privacy rules

- Context lives only for the session; it is never written to disk or logs.
- **Deny-list** (default): password managers, banking apps, Keychain, any secure field. The user can add apps.
- Per-app toggle: "Use screen context in this app."

---

## 7. Router, modes and cleanup (`mf-cleanup`)

### 7.1 Modes ("creative" without being reckless)

| Mode | Trigger | What the LLM may do | Guard strictness |
|---|---|---|---|
| **Verbatim** | profile (Terminal default) | nothing; rules only (spoken punctuation, casing) | n/a |
| **Clean** *(default)* | normal dictation | fillers, repetitions, self-corrections, punctuation, casing, paragraphs, lists, numbers/dates, name spelling | **strict** |
| **Polish** | profile (e.g. Email) or say "polish" at the end | above + smooth phrasing, merge fragments, fix grammar, match app tone | medium |
| **Command** | double-tap hotkey, or text selected | follow a spoken instruction on the selection/field: "make this friendlier", "turn into bullet points", "reply saying yes but Friday", "translate to Hindi" | none (shows a preview first) |

Inline voice commands, handled by rules before the LLM: "new line", "new paragraph", "bullet point", "scratch that" (deletes the last sentence), "all caps …", "press enter" (at the end: insert, then send Enter, opt-in).

### 7.2 Router (decides the cheapest path that's good enough)

```
raw transcript
  ├─ empty / no speech ───────────────────────────────► drop
  ├─ Verbatim profile ────────────────────────────────► rules → insert
  ├─ ≤ 6 words, no fillers, no correction cues ───────► rules → insert   (instant)
  ├─ Command mode ────────────────────────────────────► Command LLM path (preview)
  └─ otherwise ───────────────────────────────────────► Cleanup LLM (Clean/Polish)
```

Correction cues: *no, wait, actually, I mean, sorry, scratch, rather, make that, not X but Y*.

### 7.3 Rule passes (deterministic, < 1 ms, always run)

- **Pre-LLM:** spoken-punctuation commands, normalising known dictionary spellings, stripping leading "um/uh".
- **Post-LLM:** apply profile formatting (e.g. Code: no trailing period; Chat: optional lowercase start), smart-quote preference, spacing relative to the caret (add a leading space if `field_before` doesn't end in whitespace; capitalise if the caret is at a sentence start).

### 7.4 Cleanup LLM prompt (compact, so prefill stays cheap)

Our fine-tuned model is trained on this exact format, so we don't need a long system prompt:

```
<|sys|>clean v1<|app|>chat<|mode|>clean<|lang|>en
<|ctx|>Priya; Q3 roadmap; Kubernetes; SubFlo
<|prev|>…last 200 chars of field_before…
<|raw|>um so hey priya can we uh push the the q3 roadmap review to thursday no friday
<|out|>
```

→ `Hey Priya, can we push the Q3 roadmap review to Friday?`

- `max_new_tokens = 1.3 × raw_tokens + 24` (stops runaway generations).
- Greedy decoding (temperature 0) for Clean, so output is deterministic and maximally speculative-friendly. Polish/Command use temperature 0.3.
- The system/app/mode/ctx prefix is **KV-cached per app** (prefix cache), so switching back to Slack reuses it.

### 7.5 Faithfulness guard (`mf-cleanup::guard`) — the trust feature

Run on Clean (strict) and Polish (medium):

1. Normalise both texts (lowercase, strip punctuation, expand numerals so "4 pm" ≡ "four pm"), tokenise into words.
2. Word-level alignment (Levenshtein with substitution costs reduced for phonetic matches via Double Metaphone, and for dictionary/context entities).
3. Classify edits:
   - **Allowed deletions:** fillers, immediate repetitions, words inside a span cancelled by a correction cue.
   - **Allowed insertions:** function words (≤ 1 per 8 words), list markers.
   - **Allowed substitutions:** phonetic near-matches, entity spellings, numerals.
4. Compute `content_insertions`, `content_deletions`, `overlap = |kept| / |raw content words|`.
5. Fail if (strict) inserted content words > 0 beyond entities, or overlap < 0.85, or the output looks like an answer (question in, statement out with no overlap). Polish thresholds are looser (overlap ≥ 0.6, no new named entities or numbers).
6. **On fail** → insert the rules-only version and log the failure locally (anonymous counters; it feeds the opt-in dataset).

Target: guard fallback < 2 % of sessions on the benchmark. A high rate means the model needs work, not looser thresholds.

---

## 8. LLM runtime (`mf-llm`)

- **Engine:** llama.cpp via a thin FFI wrapper (`llama-cpp-sys` pinned), GGUF Q4_K_M / Q5_K_M.
  - Backends: Metal (Mac), CUDA / Vulkan (Win/Linux GPU), CPU AVX2/NEON fallback.
  - Optional MLX backend later if it measurably beats Metal llama.cpp on Apple Silicon.
- **Session-scoped KV cache:** the prefix (sys + app + mode + ctx) is cached; committed transcript chunks are appended incrementally during Listening; the tail is appended on key-up.
- **Prompt-lookup speculative decoding** (implemented in our decode loop on llama.cpp's batch API):
  ```
  loop:
    find longest suffix of generated text (n = 3..1) that occurs in raw transcript
    draft = next k (≤ 10) tokens after that occurrence in raw transcript
    run one batch forward over [last_token + draft]
    accept tokens while argmax == draft[i]; append first mismatch's argmax
  ```
  Greedy verification means output is **identical** to normal decoding, only faster.
- **Model lifecycle:** load lazily on first use; keep warm while the app is active; **unload after 10 min idle** (configurable) to return RAM. Loading is mmap, so a re-load costs ~150–400 ms. The first dictation after unload shows a "warming" pill state, and we pre-load on hotkey-down.
- **BYO endpoint:** an OpenAI-compatible HTTP provider (Ollama, LM Studio, vLLM, or a cloud key) for Polish/Command with bigger models. Clearly flagged in the UI when text leaves the device.

---

## 9. Insertion (`mf-insert`)

Strategy chain per app (learned: remember which method worked per app id):

| Order | macOS | Windows | Linux |
|---|---|---|---|
| 1 | AX: set `kAXSelectedText` on the focused element (native apps; clean undo) | UIA `ValuePattern` where safe (simple edits only) | IBus/Fcitx **input-method commit** (cleanest; optional component) |
| 2 | **Clipboard paste:** snapshot *all* pasteboard types → write text → ⌘V → restore after 250 ms (skip restore if the user copied meanwhile; check `changeCount`) | Clipboard paste with full-format snapshot/restore | `wl-copy` + `wtype`/`ydotool` (Wayland) or `xclip` + `xdotool` (X11) |
| 3 | CGEvent unicode typing (slow, last resort) | `SendInput` KEYEVENTF_UNICODE | `wtype`/`xdotool type` |

- Mark our pasteboard writes as **transient/concealed** (`org.nspasteboard.TransientType` / `ConcealedType`) so clipboard managers ignore them.
- Command mode: replace the selection; if the app is not writable, show a copy-able preview card.
- **Undo-friendly:** one insertion = one undo step wherever the OS allows.

---

## 10. Learning (`mf-learn`) — gets better the more you use it, locally

- **Edit watcher:** for 15 s after insertion (or until the focus changes), poll the field value via the context provider every 500 ms. Diff what we inserted against what's there now.
- **Dictionary learning:** a single-token or short-span replacement where old ≈ new phonetically (Double Metaphone / Jaro-Winkler) becomes a candidate. It is promoted after 2 sightings or 1 manual confirm. Entries feed ASR hotwords and LLM `ctx`.
- **Style stats:** per app — do you delete our trailing periods? lowercase starts? strip emoji? These flip profile switches automatically, with a "we noticed…" toast so it's never silent.
- **Snippets:** "insert my calendar link" → exact text expansion. Matched by the router before the LLM (fuzzy match on trigger phrase).
- **Store:** SQLite (`rusqlite`) in the app data dir: `dictionary`, `snippets`, `app_profiles`, `style_stats`, `history` (opt-in, default keeps 0 days), `metrics` (latency/guard counters, no text).
- **Opt-in personal adapter (v2):** export (raw → user-final) pairs and train a tiny LoRA locally overnight (MLX/llama.cpp finetune). The adapter loads alongside the base GGUF.

---

## 11. Models (`mf-models`)

### 11.1 Tiers (auto-picked by the hardware probe; the user can override)

| Tier | Target machine | ASR | Cleanup | Disk | Active RAM |
|---|---|---|---|---|---|
| **Lite** | 8 GB, CPU-only / old Intel | Moonshine base or Parakeet-0.6B int8 | `muskflow-clean-0.6b` Q4 + rules | ~0.8 GB | ~0.9 GB |
| **Standard** | Apple Silicon 8–16 GB, any GPU laptop | Parakeet-TDT-0.6B-v3 int8 | `muskflow-clean-1.5b` Q4_K_M | ~1.4 GB | ~1.6 GB |
| **Max** | 16 GB+ Apple Silicon / 8 GB VRAM | Parakeet + Whisper-turbo (multilingual) | `muskflow-clean-1.5b` (Clean) + `muskflow-polish-4b` (Polish/Command) | ~4.5 GB | ~4 GB |

Base models for our fine-tunes are chosen by MuskFlow-Bench, not by reputation. Candidates: Qwen 3.5 0.8B/2B, Llama 3.2 1B, Gemma edge variants. Check every licence before redistributing.

### 11.2 Manifest (`models/manifest.json`)

```json
{
  "id": "muskflow-clean-1.5b-q4km",
  "kind": "cleanup",
  "format": "gguf",
  "url": "https://huggingface.co/muskflow/clean-1.5b/resolve/<commit>/model-q4_k_m.gguf",
  "sha256": "…",
  "bytes": 1032847360,
  "license": "apache-2.0",
  "base": "…",
  "tiers": ["standard", "max"],
  "min_ram_gb": 8,
  "prompt_format": "clean-v1"
}
```

- Resumable download, sha256 verified, URLs pinned to a commit hash, atomic rename into place.
- **The only network the app ever does**, also available as a manual "import model file" for air-gapped users.

### 11.3 Hardware probe

Total/free RAM, Apple Silicon generation, Metal/CUDA/Vulkan availability, AVX2/AVX-512, battery state. **Resource governor:** on battery, use the Lite cleanup path for short utterances; on low free RAM, skip the warm-keep.

---

## 12. Training & data (`training/`) — the moat, also open

```
clean corpora ──► speechify (big LLM) ──► TTS (multi-voice, accents, noise) ──► our ASR ──► (raw, ctx, gold) pairs
    │                                                                                         │
    └──────── context synthesis (fake field text containing the true entity spellings) ──────┘
```

1. **Clean targets by app kind:** chat, email, docs, notes, code comments/commits, Hinglish chat. Open licensed corpora + synthetic generation.
2. **Speechify:** generate how people *say* it — fillers, restarts, self-corrections, spoken punctuation, spoken lists and numbers, rambling-then-summary. Use a variety of personas and accents (Indian English is a core slice).
3. **Realistic ASR noise:** TTS (several open TTS voices + noise/reverb augmentation) → run our actual ASR tiers → the real ASR output becomes input. The model learns to fix *our* errors.
4. **Context injection:** half the samples get a `ctx` / `prev` that contains the correct spelling of mis-recognised entities; the other half get irrelevant or empty context (so it doesn't hallucinate entities).
5. **Hard negatives:** already-clean inputs (output = input); dictated questions/instructions ("what's the ETA can you send it") that must be transcribed, never answered; profanity kept as said; numbers that must not change.
6. **Fine-tune:** LoRA/QLoRA (Unsloth/axolotl) → merge → GGUF quantise → re-run the benchmark *on the quantised model* (quantisation can break faithfulness; we gate on the quantised scores).
7. **Distill the router/tagger (optional Lite path):** a ~60–100M token-classifier (KEEP / DEL / PUNCT / CAP) for instant filler + punctuation cleanup on weak hardware (ONNX, < 20 ms).

Initial scale: ~30k pairs → v0 model. Then iterate on benchmark failures.

### 12.1 Fine-tuning any model that isn't good enough (policy)

Every model in the pipeline (ASR, VAD, cleanup LLM, tagger) starts as an off-the-shelf open model. We fine-tune one **only when end-to-end testing on MuskFlow-Bench shows it is the bottleneck** for a slice we care about.

1. **Find the bottleneck.** The bench reports each stage's error separately: raw-ASR WER per slice; cleanup errors given a perfect transcript (we feed gold ASR text); entity errors with and without context.
2. **Fine-tune the weakest stage on that slice.**
   - **ASR:** Parakeet via NeMo; Whisper via HF/LoRA. Train on accented and Indian English, Hinglish code-switching, technical vocabulary, laptop-mic and noisy audio, and short utterances. Data comes from contributor recordings (consented, licensed) plus TTS augmentation. We keep the hotword-biasing behaviour and check it with a regression test.
   - **Cleanup LLM:** as in §12 above.
   - **VAD:** only if it clips first syllables or misses soft speech on laptop mics.
3. **Release gate.** The fine-tuned model must beat the base on the target slice **and not regress any other slice by more than 2 % relative**. Latency and size are measured *after* quantisation/export.
4. **Publish.** Each fine-tune ships as its own Hugging Face repo (`muskflow/<base>-<purpose>`, e.g. `muskflow/parakeet-0.6b-indic-en`) with a model card, training recipe, data provenance, bench scores, and the base model's licence. The manifest points at it, and users can switch back to the base model.

---

## 13. MuskFlow-Bench (`eval/`)

- ~2,000 utterances: audio (real recordings from contributors + held-out synthetic), context JSON, gold output, slice tags (accent, app kind, correction, list, entity, Hinglish, noise, short).
- Scorers:
  - final-text WER vs gold,
  - **content-word hallucination rate**,
  - omission rate,
  - correction-resolution accuracy,
  - entity accuracy (± context),
  - "answered instead of transcribed" rate,
  - guard fallback rate,
  - latency p50/p99 per tier on reference machines.
- LLM-as-judge pairwise preference vs other tools' outputs on the same audio (published with methodology).
- Runs in CI on a small subset for every PR touching `mf-asr`, `mf-cleanup`, `mf-llm` or prompts. The full run happens nightly.

---

## 14. Desktop app (`apps/desktop`)

- **Tauri 2**, Svelte + TypeScript UI.
- **Tray:** status, mode, mic picker, pause.
- **Overlay pill** (always-on-top, click-through, near the caret or bottom-centre): waveform → **live partial text** → "cleaning" shimmer → done ✓ / fallback ⚠ (tap to see raw vs cleaned).
- **Settings:** hotkeys, modes per app (auto-detected list), dictionary & snippets editor, model tier & downloads, privacy (context per app, deny-list, history retention), BYO endpoint.
- **First-run:** permissions walkthrough (Mic, Accessibility, Input Monitoring for Fn on macOS), hardware probe → suggested tier → download with progress → 20-second "say this" demo that measures your latency.
- **Debug panel:** per-session timeline (audio / ASR / prefill / decode / guard / insert), raw vs final diff, guard verdict. Great for bug reports, and nothing leaves the device unless the user copies it.

---

## 15. Concurrency & performance engineering

- **Threads:**
  - audio callback (real-time, no alloc/lock),
  - VAD + committer worker,
  - ASR worker,
  - LLM worker (owns the model + KV cache),
  - context/insert on the main thread where the OS requires it (macOS AX is fine off-main; paste events are posted from a dedicated thread),
  - tokio runtime for orchestration + UI events.
- **Message passing:** `crossbeam` channels; the session is an actor. Cancellation via a `CancellationToken` checked between ASR windows and every decode step.
- **Zero-copy audio:** ring buffer slices are passed by reference to ASR.
- **Allocation discipline:** pre-allocated batch buffers in `mf-llm`; reused tokeniser buffers.
- **Startup:** app shows in < 300 ms; models load lazily. Binary size target < 25 MB (llama.cpp + sherpa-onnx statically linked, LTO, stripped).
- **Benchmarks:** `muskflow-cli bench` reports per-stage p50/p99 over the bench set. A regression > 10 % fails CI on the reference runner (M-series Mac mini).

---

## 16. Security & privacy

- No network except the model manifest/download (pinned hosts, sha256) and the optional user-configured BYO endpoint.
- **No telemetry** by default. Opt-in anonymous metrics are only counters/latencies, never text.
- Secure-input detection; password-field abort; app deny-list.
- Context and audio stay in memory only. History is off by default. If enabled, it's stored locally and can be encrypted with an OS keychain key.
- Signed + notarised macOS builds, signed Windows builds, reproducible-build CI. The dependency audit (`cargo deny`) runs in CI.

---

## 17. Configuration (`~/.config/muskflow/config.toml`)

```toml
[hotkey]
talk = "Fn"            # hold
command = "Fn Fn"      # double tap
mode = "hold"          # hold | toggle | handsfree

[models]
tier = "auto"
cleanup_unload_after_min = 10

[privacy]
screen_context = true
deny_apps = ["com.1password.1password", "com.apple.keychainaccess"]
history_days = 0

[profiles."com.tinyspeck.slackmacgap"]
mode = "clean"
tone = "casual"
trailing_period = false

[profiles."com.apple.Terminal"]
mode = "verbatim"

[profiles."com.google.Chrome@mail.google.com"]
mode = "polish"
tone = "professional"
```

---

## 18. Build, release, distribution

- **CI** (GitHub Actions): fmt/clippy/test; bench subset; build matrix — macOS arm64 + x86_64 (universal), Windows x64/arm64, Linux x64 (AppImage, Flatpak, .deb).
- **Releases:** Tauri updater with signed update manifests (opt-in auto-update). Homebrew cask, winget, Flathub.
- **Models:** Hugging Face org `muskflow`, each with a model card + benchmark scores + licence.
- **Licence:** Apache-2.0 for code. Fine-tuned models inherit their base model licences (documented per model).

---

## 19. Build order ("in one go", but in dependency order)

1. `mf-audio` + `mf-asr` + `muskflow-cli transcribe` → measure ASR latency/accuracy on our voices.
2. `mf-llm` with prefix cache + prompt-lookup; `mf-cleanup` rules + guard; `muskflow-cli clean` with a prompted base model.
3. `eval/` bench v0 + `training/` datagen → first fine-tuned `muskflow-clean` → swap it in.
4. `mf-context` + `mf-insert` (macOS first) + `mf-core` state machine → end-to-end dictation from the CLI.
5. `apps/desktop`: tray, overlay, settings, first-run, model manager.
6. `mf-learn`: dictionary, style stats, snippets.
7. Polish + Command modes, BYO endpoint.
8. Windows → Linux ports of context/insert/hotkey.
9. Public bench + model release + v1.0.

---

## 20. Known risks

| Risk | Mitigation |
|---|---|
| Sub-2B model quality for Polish/Command | Keep Clean narrow; Polish/Command use a 4B model on the Max tier or BYO endpoint |
| Quantisation breaks faithfulness | Gate releases on *quantised* bench scores |
| Fn hotkey + permissions friction on macOS | Excellent first-run UX; fallback hotkey (Right ⌥) |
| Electron/Chromium accessibility gaps | Enable `AXManualAccessibility`; fall back to paste with no context |
| Wayland insertion fragmentation | IBus/Fcitx input-method path as the primary on Linux |
| Model licence constraints | Licence matrix in `models/manifest.json`; prefer Apache/MIT bases |
| Existing OSS tools (OpenWhispr, VoiceInk, MuskFlow) | Differentiate on the fine-tuned model, guard, latency and bench; publish the model so the ecosystem standardises on it |
