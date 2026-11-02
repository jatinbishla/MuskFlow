# Benchmarks log

Machine: MacBook (Apple M5, 4P + 6E cores, 16 GB), macOS 27, CPU execution via sherpa-onnx 2.0.1.

Test clip: 17.2 s synthetic messy dictation (`say -v Samantha`), containing fillers, a repeated word, a self-correction, names and a number:

> "Um, so hey Priya, can we uh push the the Q3 roadmap review to Thursday, no wait, Friday? And also, uh, tell Gaurav that the Kubernetes migration is basically done, like ninety percent, and we just need to, um, test the staging cluster before we ship it."

The ASR is *supposed* to keep fillers and corrections: removing them is the cleanup LLM's job.

## 2026-11-03 — ASR, streaming replay (`freeflow stream`, real time)

| Model | Settings | Transcript quality | Decode CPU while speaking (% of audio time) | Key-up → text |
|---|---|---|---|---|
| Parakeet-TDT 0.6B v3 int8 | step 1.0 s, context 1.0 s, 8 threads | **Near-perfect**; only "Gaurav" → "Garov", occasionally "Cubernetes" | 30 % | **183 ms** |
| Parakeet-TDT 0.6B v3 int8 | step 0.5 s, context 2.0 s | same | 71 % | 408 ms |
| Parakeet-TDT 0.6B v3 int8 | step 0.5 s, no context | duplicated words at chunk seams, spurious capitals | 25 % | 215 ms |
| Parakeet-TDT 110M int8 | step 1.0 s, context 1.0 s, 4 threads | names wrong ("Kridaya"), a seam duplicate | 22 % | 84 ms |
| Parakeet unified 0.6B streaming 560 ms | native streaming | dropped first sentence; re-encodes everything | 836 % | 7 s ❌ (config issue, not investigated yet) |

One-shot (`freeflow transcribe`): Parakeet 0.6B v3 decodes 17 s in ~1.6 s (≈10× real time, 8 threads); CoreML provider gave no speed-up for the int8 model.

### Takeaways
- **Default: Parakeet 0.6B v3 + committer, step 1 s, context 1 s.** Already inside the 350 ms end-to-end budget for the ASR share.
- Left context is essential for stitching quality; 1 s is enough.
- 110M is the Lite-tier candidate: 2× faster, but needs the LLM + screen context to fix names.
- Hotwords (`--hotwords`, beam search) had **no effect** and doubled the decode time. sherpa-onnx likely needs BPE-tokenised hotwords (`modeling_unit=bpe` + `bpe_vocab`). TODO.
- Biggest remaining win: run Parakeet on the Apple Neural Engine / GPU (CoreML fp16 build) instead of int8 on CPU. That would cut CPU use while speaking from 30 % to a few % (battery).

## 2026-11-03 — live mic (`freeflow listen`, MacBook Air M5, built-in mic)

| Run | Speech | Enter → text | Notes |
|---|---|---|---|
| v1 (4 threads, window grew through pauses) | 20 s | 7,645 ms | decoder fell behind real time during silence |
| v2 (silence slides window, coalesced backlog, auto threads) | 46 s | 1,051 ms | tail decode only 312 ms; the rest was waiting for an in-flight decode + one catch-up decode |
| v3 (no catch-up decode after Enter) | 59 s | **515 ms** | tail 0.81 s decoded in 270 ms; remainder is waiting on the in-flight decode |

Real dictation (v2) kept every self-correction ("in the European no no no in the US market"); misses were the brand name ("subflow" for SubFlo) and "building app" (likely "billing app"). Both are dictionary/context jobs.

v3 dictation: "billing" was heard as "building" every time, even when the speaker corrected it out loud ("billing software, not building"). That's a consistent Indian-English vowel confusion, so it's the first concrete candidate for the ASR fine-tuning policy (ARCHITECTURE §12.1). "SubFlo" came out as "subflow / sub flow / Subflow", which needs the dictionary.
