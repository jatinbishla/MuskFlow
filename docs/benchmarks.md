# Benchmarks log

Machine: MacBook (Apple M5, 4P + 6E cores, 16 GB), macOS 27, CPU execution via sherpa-onnx 2.0.1.

Test clip: 17.2 s synthetic messy dictation (`say -v Samantha`), containing fillers, a repeated word, a self-correction, names and a number:

> "Um, so hey Priya, can we uh push the the Q3 roadmap review to Thursday, no wait, Friday? And also, uh, tell Gaurav that the Kubernetes migration is basically done, like ninety percent, and we just need to, um, test the staging cluster before we ship it."

The ASR is *supposed* to keep fillers and corrections: removing them is the cleanup LLM's job.

## 2026-11-03 — ASR, streaming replay (`umless stream`, real time)

| Model | Settings | Transcript quality | Decode CPU while speaking (% of audio time) | Key-up → text |
|---|---|---|---|---|
| Parakeet-TDT 0.6B v3 int8 | step 1.0 s, context 1.0 s, 8 threads | **Near-perfect**; only "Gaurav" → "Garov", occasionally "Cubernetes" | 30 % | **183 ms** |
| Parakeet-TDT 0.6B v3 int8 | step 0.5 s, context 2.0 s | same | 71 % | 408 ms |
| Parakeet-TDT 0.6B v3 int8 | step 0.5 s, no context | duplicated words at chunk seams, spurious capitals | 25 % | 215 ms |
| Parakeet-TDT 110M int8 | step 1.0 s, context 1.0 s, 4 threads | names wrong ("Kridaya"), a seam duplicate | 22 % | 84 ms |
| Parakeet unified 0.6B streaming 560 ms | native streaming | dropped first sentence; re-encodes everything | 836 % | 7 s ❌ (config issue, not investigated yet) |

One-shot (`umless transcribe`): Parakeet 0.6B v3 decodes 17 s in ~1.6 s (≈10× real time, 8 threads); CoreML provider gave no speed-up for the int8 model.

### Takeaways
- **Default: Parakeet 0.6B v3 + committer, step 1 s, context 1 s.** Already inside the 350 ms end-to-end budget for the ASR share.
- Left context is essential for stitching quality; 1 s is enough.
- 110M is the Lite-tier candidate: 2× faster, but needs the LLM + screen context to fix names.
- Hotwords (`--hotwords`, beam search) had **no effect** and doubled the decode time. (Fixed later: see the dictionary section below.)
- Biggest remaining win: run Parakeet on the Apple Neural Engine / GPU (CoreML fp16 build) instead of int8 on CPU. That would cut CPU use while speaking from 30 % to a few % (battery).

## 2026-11-03 — live mic (`umless listen`, MacBook Air M5, built-in mic)

| Run | Speech | Enter → text | Notes |
|---|---|---|---|
| v1 (4 threads, window grew through pauses) | 20 s | 7,645 ms | decoder fell behind real time during silence |
| v2 (silence slides window, coalesced backlog, auto threads) | 46 s | 1,051 ms | tail decode only 312 ms; the rest was waiting for an in-flight decode + one catch-up decode |
| v3 (no catch-up decode after Enter) | 59 s | **515 ms** | tail 0.81 s decoded in 270 ms; remainder is waiting on the in-flight decode |

Real dictation (v2) kept every self-correction ("in the European no no no in the US market"); misses were the brand name ("subflow" for SubFlo) and "building app" (likely "billing app"). Both are dictionary/context jobs.

v3 dictation: "billing" was heard as "building" every time, even when the speaker corrected it out loud ("billing software, not building"). That's a consistent Indian-English vowel confusion, so it's the first concrete candidate for the ASR fine-tuning policy (ARCHITECTURE §12.1). "SubFlo" came out as "subflow / sub flow / Subflow", which needs the dictionary.

## 2026-11-03 — live mic, after first-word fix

| Run | Speech | Final decode | Enter → text | Notes |
|---|---|---|---|---|
| v4 (`--mic MacBook`, before fix) | 64 s | 0.62 s tail in 178 ms | 682 ms | first words lost ("is Jatan Bushla") |
| v5 (first-word fix) | 52 s | **4.11 s tail in 776 ms** | 1,483 ms | first words kept ✓; last ~4 s never stabilised (continuous fast speech), and decoding ran at ~5× real time (Air thermals?) |

Open issue: Enter → text depends on how much text is still unstable at key-up and on the in-flight decode. Ideas: ignore fillers when checking agreement; cancel or parallelise the in-flight decode on key-up; ANE/GPU decoding.

## 2026-11-03 — latency spike fix (fast, filler-heavy speech)

New clip `fast.wav`: 21 s at 215 wpm, continuous, lots of "uh/um". Before the fix: a 5.7 s unconfirmed tail at key-up, ~600 ms key-up → text.

Root causes, found by logging every agreement decision and every raw decode:
1. Fillers flip between decodes ("uh" ↔ "a", "um" ↔ "I'm"), breaking prefix agreement.
2. Parakeet drops words at the start of a window, so two decodes disagree on where they begin.
3. **Starting a decode mid-word makes Parakeet emit nothing for seconds of clear speech.** The silence rule then discarded that audio, so words were lost.

Fixes: agreement ignores fillers and tolerates ≤3 dropped lead words (keeping the fuller version); left context snaps to a committed word's start; context 1.5 s; silence is confirmed by Silero VAD rather than by an empty decode; the final decode restores lead words it dropped.

| Clip | Key-up → text (3 runs) | CPU while speaking | Transcript |
|---|---|---|---|
| fast (21 s) | 291 / 415 / 294 ms | 36–41 % | complete |
| dictation (17 s) | 283 / 302 / 391 ms | 35–36 % | complete |
| pauses (16 s) | 231 / 330 / 337 ms | 29–30 % | complete |

`listen --save recordings/x.wav` now records real sessions so they can be replayed with `stream` (recordings/ is gitignored).

Live check (jatin-01, 52 s, MacBook Air): transcript complete with first words kept, but Enter → text 1,442 ms = final decode 563 ms + ~880 ms waiting for the in-flight decode. Replaying the same recording: average decode ≈ 370 ms, final ≈ 500 ms. **The remaining latency is decode speed on CPU**, so the next lever is GPU/ANE inference.

## 2026-11-03 — all three tiers, ASR only (`umless bench`, real-time replay)

Full report with transcripts: [bench/2026-11-03-asr-tiers.md](bench/2026-11-03-asr-tiers.md). MacBook Air M5, CPU only, 8 clips / 100 s, US + UK + Indian English voices.

| Tier | WER | Gap to best | Key-up → text p50 / max | CPU while speaking | Size |
|---|---|---|---|---|---|
| Lite (Parakeet 110M) | 6.9% | +5.4 pts | 84 / 171 ms | 10% | 136 MB |
| Standard (Parakeet 0.6B) | 4.3% | +2.9 pts | 293 / 413 ms | 37% | 670 MB |
| Max (Whisper turbo, by phrase) | **1.4%** | — | 3,556 / 5,243 ms | 80% | 1,036 MB |

- Max is the most accurate, but on a CPU-only Air it's too slow to use. It needs the GPU/ANE.
- Lite's errors are names and rare words ("Rediscon", "postger's", "Joden", "pria"), plus fillers misheard as words. That's exactly what hotwords, the context-aware cleanup LLM and distilling Max into Lite are meant to fix.
- Gap to close: **5.4 points of WER between Lite and Max**.

## 2026-11-03 — cleanup LLM, all three tiers (speech → text → cleaned)

Full report: [bench/2026-11-03-cleanup-tiers.md](bench/2026-11-03-cleanup-tiers.md) (fast replay, so the ASR latency column there isn't realistic; cleanup timings are). Final WER is scored against what the speaker *meant* (`<id>.clean.txt`): fillers gone, self-corrections resolved.

| Tier | Cleanup LLM | Final WER | Rules-only WER | Cleanup p50 | Guard fallbacks |
|---|---|---|---|---|---|
| Lite | Qwen3.5 0.8B | 8.7% | 12.2% | 331 ms | 1/8 |
| Standard | Qwen3.5 2B | 6.1% | 9.9% | 651 ms | 1/8 |
| Max | Qwen3 4B Instruct 2507 | **2.3%** | 6.1% | 892 ms | 0/8 |

Model selection (same clips, same prompt):

| Tier | Qwen3.5 (hybrid) | Qwen3 (transformer) | Picked |
|---|---|---|---|
| Lite | **8.7%**, 334 ms (0.8B) | 9.5%, 392 ms (0.6B) | Qwen3.5 |
| Standard | **6.1%**, 659 ms (2B) | 8.0%, 351 ms (1.7B) | Qwen3.5 (accuracy first) |
| Max | 4.6%, 1,473 ms (4B) | **2.3%, 825 ms** (4B-Instruct-2507) | Qwen3 |

What made the difference:
- **Prefix-state cache**: instructions and examples are computed once and the state is snapshotted; each dictation only prefills its own ~50 tokens. Standard went from 1,145 ms to 403 ms per call.
- **Persistent context** instead of one per call.
- **Guard v2**: rejects dropped clauses, but allows words retracted by a self-correction ("Wednesday at 3 pm? No, actually, make that Thursday").
- **Prompt**: change as few words as possible, keep word order, and treat ASR-misheard fillers ("uh" → "I'll"/"all", "um" → "I'm") as fillers.

Open issues:
- Qwen3.5 is hybrid (gated delta net), so prompt-lookup speculation is off for Lite/Standard. Speculation for hybrids needs state snapshots per draft.
- Lite's 0.8B still misses some self-corrections, so it's the first fine-tuning target. Gap to Max: 6.5 pts.
- No fine-tuning yet: everything above is zero-shot plus examples.

## 2026-11-03: dictionary (names and jargon), all tiers

Full report: [bench/2026-11-03-dictionary-tiers.md](bench/2026-11-03-dictionary-tiers.md). 11 synthetic clips. Three of them are new and name-heavy and were not used for tuning (`names-us`, `names-in`, `jargon-gb`). `bench/dictionary.txt` has 28 terms, including some that appear in no clip.

| Tier | Final WER, no dictionary | **Final WER, with dictionary** | Rules-only, with dictionary |
|---|---|---|---|
| Lite | 10.5% | **6.6%** | 9.3% |
| Standard | 7.2% | **3.6%** | 6.9% |
| Max | 4.2% | **1.5%** | 4.5% |

- **Decoder hotwords were fixed and are opt-in.** sherpa-onnx rejected whole words for NeMo models, so every hotword was silently dropped. Umless now spells them into the model's own BPE pieces. Even so, a boost of 3/token barely helps, and 6+ makes Parakeet hallucinate the terms. That's why it's `--hotword-boost`, off by default.
- **Dictionary respelling** does the work, on every tier including Whisper. Examples: Subflow→SubFlo, firman→foreman, Zirodhaa→Zerodha, Garv→Gaurav, Nick Hill→Nikhil, "turn a construction"→Turner Construction.
- **No regressions** on clips without dictionary terms.
- **Still missed:** "Siobhan" (heard as "Chavon"/"Shivan"; the Irish spelling doesn't match the sound) and "Razape"→Razorpay on Lite.
- **Lite still trails Max by 5.1 pts.** Most of the gap is self-corrections in `dictation` and fillers in `fast`, which is the fine-tuning target.

## 2026-11-04: Lite cleanup fine-tune, round 1

Full report: [bench/2026-11-04-lite-finetune-r1.md](bench/2026-11-04-lite-finetune-r1.md). The model was trained on a free Colab T4: 4,826 synthetic pairs, 2 epochs, 71 minutes. On 200 held-out synthetic pairs it scored 0.4% WER, with 170/200 exact.

| Lite cleanup model | Final WER (11 real-speech clips) | Guard fallbacks |
|---|---|---|
| Qwen3.5-0.8B, few-shot (current) | **6.6%** | 2/11 |
| Umless-Lite-Cleanup-0.8B, round 1 | 6.9% | 1/11 |

**It's a tie, not a win, so Lite keeps the base model.**

- **Better:** self-corrections (`dictation` 15.4% → 7.7%) and `tech-gb` (12.5% → 8.3%).
- **Worse, where the synthetic data was too narrow:**
  1. **Phrase corrections.** "Wednesday at three PM, no, actually, make that Thursday morning" became "Thursday morning at 3 PM". The data only swapped single values.
  2. **Real words deleted.** The data turned "um" into "I'm", so the model learned to delete real ones: "Hi, I'm Jatin" became "Hi Jatin". It also dropped a real "like".

**Round 2 data:**
- phrase-level corrections;
- contrastive pairs that keep real "I'm/and/like/on";
- the formal and AI-prompt styles, so Lite can use them too.

## 2026-11-04: Lite cleanup fine-tune, round 2

Full report: [bench/2026-11-04-lite-finetune-r2.md](bench/2026-11-04-lite-finetune-r2.md). This round adds the phrase-correction, contrastive and style data listed above, and uses compact prompts (about 110 tokens instead of 450).

| Lite cleanup model | Final WER (11 clips) | Cleanup p50 | Guard fallbacks |
|---|---|---|---|
| Qwen3.5-0.8B, few-shot (current) | 6.6% | 602 ms | 2/11 |
| Umless-Lite-0.8B, round 2 | **6.3%** | 705 ms | 2/11 |
| *Standard / Max, for reference* | *3.6% / 1.5%* | | |

**Clean dictation:** a small win. Round 2 resolves "Thursday, no wait, Friday" in `dictation` (15.4% → 10.3%), which the base model never does, and "Hi, I'm Jatin" is kept.

**Styles (formal, AI prompt), hand-checked on 6 dictations:** not good enough to turn on for Lite.

| | Base 0.8B | Round 2 |
|---|---|---|
| "first … second … third" as a prompt | numbered list ✓ | one sentence, adds "web" ✗ |
| "create an adobe software open source" | ✓ | drops "open source" ✗ |
| Stripe-webhook bug report as a prompt | ✓ | ✓ |
| Invoice email to Sarah, formal | ✓ | flips it into "could you send the invoice you sent" ✗ |
| "thursday no wait friday", formal | keeps Thursday ✗ | Friday ✓ |
| Clean output capitalisation | ✓ | "friday", "q3" lower-case ✗ |

**Decision:** Lite keeps the base model and keeps falling back to Clean for formal and prompt destinations. Round 3 needs:
- prompt targets that keep enumerations as lists and never drop qualifiers;
- formal pairs where a question about something stays that question;
- properly capitalised clean targets, so case isn't learned from the noisy input;
- the six real recorded passages (`scripts/record-passages.sh`) as the held-out test.

## 2026-11-06: Lite cleanup fine-tune, round 3

Round 3 added command-mode examples, list/question-preserving style filters and case-loss noise. Lite now loads the newest `Umless-Lite-*-rN` file in `models/` by default. These runs also include the new deterministic digits and self-correction rules (which apply to every tier).

**Dictation, 11 synthetic clips (with `bench/dictionary.txt`):**

| Lite cleanup model | Final WER | Guard fallbacks |
|---|---|---|
| Qwen3.5-0.8B base | 6.6% | 2/11 |
| Round 1 | 6.9% | 1/11 |
| Round 2 | **6.3%** | 2/11 |
| Round 3 | 6.6% | 3/11 |
| *Standard / Max, for reference* | *3.6% / 1.5%* | *0 / 0* |

**30-case text suite** (fillers, corrections, numbers, questions, no-answer, names, prompt, formal, casual; automatic checks), passes:

| | Base | r1 | r2 | r3 | Standard | Max |
|---|---|---|---|---|---|---|
| All | 21 | 19 | 18 | 20 | 21 | 23 |
| Questions keep "?" | 2/3 | 0/3 | 0/3 | 1/3 | 2/3 | 2/3 |
| Prompt style | 4/6 | 3/6 | 3/6 | 2/6 | 4/6 | 5/6 |

**Command mode (6 hand-checked tasks):** round 3 is the best Lite model so far: the Gmail reply addresses the right person and invents nothing (round 2 invented "Priya from Turner Construction"), the Slack reply keeps 1:30, and it runs about twice as fast because it needs no few-shot examples. Hindi translation is still poor on every tier.

**Decision:** round 3 becomes Lite's default for dictation and command mode. Formal and prompt destinations keep falling back to Clean on Lite: numbered lists are still flattened, and "fix it" was dropped from one bug report. The fine-tunes also lose trailing question marks more often than the base model, which a deterministic rule can fix.
