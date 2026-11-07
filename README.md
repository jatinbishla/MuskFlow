# Umless

**Local, open-source voice dictation that types what you *meant*, not just what you said.** Hold a key, talk the way you actually talk ("um, can you look at my auth middleware, no wait, the session handler…"), let go, and clean text appears at your cursor, written for the app you're in. Speech recognition, the cleanup LLM and the screen reading all run on your laptop. No audio or text ever leaves it.

Rust · Parakeet-TDT / Whisper via sherpa-onnx · Qwen via llama.cpp (Metal) · a LoRA fine-tuned 0.8B cleanup model · macOS Accessibility. Apache-2.0.

| | **Lite** | **Standard** | **Max** |
|---|---|---|---|
| Speech model | Parakeet-TDT 110M (int8) | Parakeet-TDT 0.6B v3 (int8) | Whisper large-v3-turbo |
| Cleanup model | **Umless-Lite-0.8B r3** (our LoRA fine-tune of Qwen3.5-0.8B, Q4_K_M) | Qwen3.5-2B (Q4_K_M) | Qwen3-4B-Instruct-2507 (Q4_K_M) |
| On disk | 678 MB | 1.9 GB | 3.5 GB |
| **Word error rate, typed text** (11-clip suite) | **5.7%** | **3.6%** | **1.8%** |
| Word error rate, speech model alone | 9.2% | 6.3% | 3.7% |
| Key-up → text, p50 / max (M5 MacBook Air, idle) | 0.36 / 0.75 s¹ | 0.76 / 1.53 s | 1.25 / 3.35 s |
| Languages | English | 25 European | 99, incl. Hindi |

¹ measured with the base 0.8B cleanup model; the fine-tune's prompt is ~6× shorter (~70 vs ~450 tokens).

Details, methodology and the full history are in [Benchmarks](#benchmarks) and [docs/benchmarks.md](docs/benchmarks.md); the design is in [ARCHITECTURE.md](ARCHITECTURE.md).

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

## How it works

```
 mic ─► ring buffer ─► 16 kHz ─► streaming ASR ───────────► rules ─► cleanup LLM ─► guard ─► typed at cursor
 (cpal, RT-safe)     (resample)  (decodes while you talk)  (fillers,   (style from  (rejects     (clipboard + ⌘V,
                                  + Silero VAD              self-       the app +    invented     clipboard restored)
                                                            corrections,screen text) names/numbers)
                                                            digits, "?")
```

**1. Streaming speech recognition, so key-up is cheap.** Parakeet is an offline model, so Umless runs it in a LocalAgreement-2 loop: every second it re-decodes the uncommitted window (plus 1.5 s of already-committed audio as left context, snapped to a word start) and commits the words two consecutive decodes agree on, ignoring filler flips like "uh"↔"a". At key-up only the short uncommitted tail is left to decode, so latency doesn't grow with how long you spoke (a 62 s take finishes in 85 ms of ASR on Lite). Whisper has no reliable word timestamps, so Max decodes VAD-cut phrases instead, capped at 4 s so the phrase still open at key-up stays small (8 s → 4 s halved Max's p50: 2.9 s → 1.25 s, WER 1.5% → 1.8%).

**2. A safety net for lost words.** A decode that starts mid-word can make Parakeet drop seconds of speech for good once the window slides past it. A per-session Silero VAD marks speech as it arrives; at key-up, any stretch VAD heard as speech that no committed word covers is decoded again and spliced back in by timestamp, skipping words already committed and hesitation-only re-decodes. It costs nothing on normal takes (VAD runs while you talk).

**3. Deterministic rules before any model.** Fillers and stammers, explicit self-corrections ("Tuesday, no wait, Wednesday", "at ten. Actually, make it eleven" → the replaced word must be the same kind: day, number, determiner…), spoken numbers to digits ($18,750, 15%, 2:30, +91, while "two blocking bugs" stays words), question marks small models drop, misheard work phrases ("let's sing" → sync, "deploy field" → failed). These run on every tier and are what's typed when a model's output is rejected.

**4. A cleanup LLM that writes for the app you're in.** The focused app (and, in browsers, the site in the window title) picks the style: a concise, structured **prompt** in Claude, ChatGPT, Cursor, Codex or a terminal; **formal** in Mail, Gmail, Outlook, Teams; **casual** in Slack, WhatsApp, Messages; minimal **clean** elsewhere. The conversation's tone can override it (a jokey Teams chat gets casual). llama.cpp runs on Metal with the fixed system prompt's KV state cached and prompt-lookup speculative decoding (~3.5 tokens accepted per pass, since cleanup output mostly copies its input).

**5. A guard against rewrites that invent things.** Every LLM output is checked against what was said: new names or numbers (with tolerance for things said in pieces: "my sql" → MySQL, "nine one" → 91, "2000-line"), dropped names, answering instead of rewriting (code fences, much longer output), and for plain cleanup, words dropped that weren't taken back. A rejected output falls back to the rules-only text, so the worst case is less polish, never a wrong fact.

**6. Screen context, locally.** While you speak, a thread walks the focused window's Accessibility tree (≤4,000 nodes, 300 ms budget): text before the cursor, the selection, and the visible thread. Names on screen join the dictionary for that dictation ("pria" → Priya, chat senders like "Meera Shah" included), and command mode (Fn + Control) uses the thread as context ("reply saying Thursday at 4 works"). Chromium and Electron build their tree only on request (`AXEnhancedUserInterface` for Chrome, `AXManualAccessibility` for Electron, ~2 s on the first read per app launch, overlapped with speech); in browsers only the page's `AXWebArea` text is kept.

**7. A dictionary that respells, not biases.** Speech models usually hear a rare name right and only spell it wrong, so instead of boosting the decoder (which makes it hallucinate the term), Umless respells near misses that sound alike (a phonetic key plus a per-length edit budget), and exact "heard as" entries handle the rest (`Gaurav = Gorov, Barraf`). If you fix a name Umless typed, it reads the field back and learns your spelling.

## Benchmarks

All numbers from `umless bench`, which replays recordings through the same session code the app uses, then scores against two references: what was *said* (ASR WER) and what was *meant* (final WER, with fillers dropped and self-corrections resolved). WER ignores case, punctuation, number formatting and fillers. Reproduce with `scripts/make-bench-clips.sh && ./target/release/umless bench --dictionary bench/dictionary.txt`.

**11-clip suite** (synthesised voices, en-US/en-GB/en-IN accents; dictation with corrections, fast speech, names, jargon, numbers, pauses), final WER (ASR WER):

| Clip | Lite | Standard | Max |
|---|---|---|---|
| dictation (fillers + "no wait") | 7.7% (6.8%) | 5.1% (4.5%) | 2.6% (2.3%) |
| email-gb | 3.1% (2.5%) | 0.0% (7.5%) | 0.0% (0.0%) |
| fast (~200 wpm) | 10.8% (12.2%) | 4.1% (5.4%) | 5.4% (5.4%) |
| founder-in | 0.0% (4.8%) | 0.0% (9.5%) | 0.0% (0.0%) |
| jargon-gb | 0.0% (9.1%) | 4.5% (4.5%) | 0.0% (0.0%) |
| names-in | 8.0% (16.0%) | 0.0% (16.0%) | 0.0% (12.0%) |
| names-us | 4.2% (25.0%) | 4.2% (12.5%) | 0.0% (16.7%) |
| numbers-in | 0.0% (0.0%) | 16.0% (0.0%) | 0.0% (0.0%) |
| pauses | 4.5% (4.5%) | 4.5% (4.5%) | 4.5% (4.5%) |
| tech-gb | 12.5% (16.7%) | 0.0% (0.0%) | 0.0% (0.0%) |
| **All** | **5.7% (9.2%)** | **3.6% (6.3%)** | **1.8% (3.7%)** |

The gap between the two columns is what cleanup buys: names the ASR misspells get fixed by the dictionary and screen context, and resolved self-corrections count as correct only in the final text.

**A real voice** (the author reading five passages on a MacBook Air mic, Indian-English accent, natural restarts included; `scripts/record-passages.sh`), final WER: Lite 10.2%, Standard 11.7%, Max 8.7%. One email passage ("March third at two thirty… actually, no, let's make it Thursday morning") dominates; without it, Standard is 5.2%, Lite 8.5%, Max 8.0%. Hindi/Hinglish isn't supported well on any tier yet.

**AI-prompt mode** (12 rambling spoken requests to an AI assistant: debugging, SQL, refactors, regexes, research, emails; tokens counted with `o200k_base`):

| | Rewritten (not fallen back) | Tokens, said → typed | Requirements kept |
|---|---|---|---|
| Standard | 11/12 | 435 → 367 (−16%) | 11/12 |
| Max | 11/12 | 435 → 375 (−14%) | 12/12 |

A blind A/B test then sent each raw and each Umless prompt to a separate Claude instance, and a third judged the answer pairs against what the speaker actually asked for: Umless 4 wins, raw 3, 5 ties. **Takeaway: the win is shorter, cleaner prompts with every requirement intact, not smarter answers.** Strong models already cope with messy speech; both losses were rewrites that changed meaning ("plus nine one" → "91", "I'll paste the transcript" → "Paste the transcript"), now fixed in the rules and prompt.

**Latency** (key-up → text in the target app, real-time replay, M5 MacBook Air, idle): p50 0.36 s Lite, 0.76 s Standard, 1.25 s Max. Max's floor is Whisper on CPU (~1.5 s per decode); the cleanup LLM itself takes ~0.4–0.9 s.

## Fine-tuning the Lite cleanup model

Lite's job is the hardest: a 0.8B model cleaning up the noisiest transcripts (Parakeet 110M). Few-shot prompting the base Qwen3.5-0.8B needed ~450-token prompts and still missed self-corrections, so I fine-tuned it. The pipeline is one script, [`finetune/lite_cleanup.py`](finetune/lite_cleanup.py), built to run end to end on a free Colab T4 ([notebook](finetune/Umless_Lite_finetune.ipynb)):

1. **Targets first, inputs derived.** A teacher (Qwen3-4B-Instruct) writes the *clean* text: messages across ~40 everyday themes, spoken AI requests → prompts, dictation → formal mail, and command-mode examples (screen + instruction → reply). The messy input is then generated *from* the target the way speech + Parakeet-110M corrupts it: fillers, stammers, phrase-level self-corrections, numbers as words, case loss, and Parakeet's typical confusions ("uh" heard as "all"). Targets are exact by construction, and misheard names never appear in inputs, so the model never learns to invent words.
2. **Filters on the teacher**, so bad targets never train: enumerations must stay numbered lists, questions must stay questions, qualifiers ("open source") must survive.
3. **LoRA** (r = 16, α = 32, lr 2e-4, ~1.5 epochs, effective batch 32) on the full-attention and linear-attention projections of Qwen3.5's hybrid layers, plus the MLPs; merged back into the original checkpoint (keeping every non-LM tensor) and quantised to GGUF Q4_K_M for llama.cpp.
4. **Trained on compact prompts**: the fine-tune needs a ~70-token prompt instead of ~450 tokens of few-shot examples, which halves command-mode latency.

| Round | What changed | Final WER (11 clips) | Text suite |
|---|---|---|---|
| Base Qwen3.5-0.8B, few-shot | — | 6.6% | 21/30 |
| r1 | 4.8k synthetic pairs, 71 min on a T4 | 6.9% | 19/30 |
| r2 | phrase-level corrections, contrastive "I'm/like" pairs, formal + prompt styles | 6.3% | 18/30 |
| **r3 (shipped)** | command mode, list/question filters, case-loss noise | **5.7%**¹ | 23/30¹ |

¹ with the current rules (digits, corrections, "?") in front of it; base with the same rules: 5.7%, 25/30.

**What the fine-tune did and didn't buy.** On the synthetic suite it ties the base model. Where it wins is real use: on my own recorded voice it averages 10.2% vs 11.8% (it resolves the "actually, no, make it Thursday" correction the base model doesn't: 17% vs 38% on that passage), it's the only Lite model that writes command-mode replies without inventing people, and it rewrites more long requests instead of answering them (6/12 vs 4/12). What it didn't fix: it still flattens "first… second… third" into prose and drops trailing question marks, which is why Lite keeps plain cleanup in AI apps and a deterministic rule restores the "?". Lite loads the newest `Umless-Lite-*-rN.gguf` in `models/`, so a new round is a drop-in file. The r3 weights aren't published yet: until they are, `umless models install lite` fetches the base Qwen3.5-0.8B, and you can train your own with the notebook.

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
