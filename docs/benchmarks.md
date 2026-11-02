# Benchmarks log

Machine: MacBook (Apple M5, 4P + 6E cores, 16 GB), macOS 27, CPU execution via sherpa-onnx 2.0.1.

Test clip: 17.2 s synthetic messy dictation (`say -v Samantha`), containing fillers, a repeated word, a self-correction, names and a number:

> "Um, so hey Priya, can we uh push the the Q3 roadmap review to Thursday, no wait, Friday? And also, uh, tell Gaurav that the Kubernetes migration is basically done, like ninety percent, and we just need to, um, test the staging cluster before we ship it."

The ASR is *supposed* to keep fillers and corrections: removing them is the cleanup LLM's job.

## 2026-11-03 — ASR, streaming replay (`muskflow stream`, real time)

| Model | Settings | Transcript quality | Decode CPU while speaking (% of audio time) | Key-up → text |
|---|---|---|---|---|
| Parakeet-TDT 0.6B v3 int8 | step 1.0 s, context 1.0 s, 8 threads | **Near-perfect**; only "Gaurav" → "Garov", occasionally "Cubernetes" | 30 % | **183 ms** |
| Parakeet-TDT 0.6B v3 int8 | step 0.5 s, context 2.0 s | same | 71 % | 408 ms |
| Parakeet-TDT 0.6B v3 int8 | step 0.5 s, no context | duplicated words at chunk seams, spurious capitals | 25 % | 215 ms |
| Parakeet-TDT 110M int8 | step 1.0 s, context 1.0 s, 4 threads | names wrong ("Kridaya"), a seam duplicate | 22 % | 84 ms |
| Parakeet unified 0.6B streaming 560 ms | native streaming | dropped first sentence; re-encodes everything | 836 % | 7 s ❌ (config issue, not investigated yet) |

One-shot (`muskflow transcribe`): Parakeet 0.6B v3 decodes 17 s in ~1.6 s (≈10× real time, 8 threads); CoreML provider gave no speed-up for the int8 model.

### Takeaways
- **Default: Parakeet 0.6B v3 + committer, step 1 s, context 1 s.** Already inside the 350 ms end-to-end budget for the ASR share.
- Left context is essential for stitching quality; 1 s is enough.
- 110M is the Lite-tier candidate: 2× faster, but needs the LLM + screen context to fix names.
- Hotwords (`--hotwords`, beam search) had **no effect** and doubled the decode time. sherpa-onnx likely needs BPE-tokenised hotwords (`modeling_unit=bpe` + `bpe_vocab`). TODO.
- Biggest remaining win: run Parakeet on the Apple Neural Engine / GPU (CoreML fp16 build) instead of int8 on CPU. That would cut CPU use while speaking from 30 % to a few % (battery).

## 2026-11-03 — live mic (`muskflow listen`, MacBook Air M5, built-in mic)

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

## 2026-11-03 — all three tiers, ASR only (`muskflow bench`, real-time replay)

Full report with transcripts: [bench/2026-11-03-asr-tiers.md](bench/2026-11-03-asr-tiers.md). MacBook Air M5, CPU only, 8 clips / 100 s, US + UK + Indian English voices.

| Tier | WER | Gap to best | Key-up → text p50 / max | CPU while speaking | Size |
|---|---|---|---|---|---|
| Lite (Parakeet 110M) | 6.9% | +5.4 pts | 84 / 171 ms | 10% | 136 MB |
| Standard (Parakeet 0.6B) | 4.3% | +2.9 pts | 293 / 413 ms | 37% | 670 MB |
| Max (Whisper turbo, by phrase) | **1.4%** | — | 3,556 / 5,243 ms | 80% | 1,036 MB |

- Max is the most accurate, but on a CPU-only Air it's too slow to use. It needs the GPU/ANE.
- Lite's errors are names and rare words ("Rediscon", "postger's", "Joden", "pria"), plus fillers misheard as words. That's exactly what hotwords, the context-aware cleanup LLM and distilling Max into Lite are meant to fix.
- Gap to close: **5.4 points of WER between Lite and Max**.
