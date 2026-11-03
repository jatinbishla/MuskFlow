---
license: apache-2.0
base_model: Qwen/Qwen3.5-0.8B
language:
- en
pipeline_tag: text-generation
library_name: gguf
tags:
- dictation
- speech-to-text
- text-cleanup
- disfluency-removal
- muskflow
- gguf
- llama.cpp
---

# MuskFlow Lite Cleanup 0.8B (GGUF)

**Raw dictation in, the text you meant out.** This is the cleanup model behind the Lite tier of [MuskFlow](https://github.com/jatinbishla/MuskFlow), an open-source, fully local voice-typing app. It is Qwen3.5-0.8B fine-tuned for one job:

| Speech recognizer heard | Model writes |
|---|---|
| `Can we move the review to Thursday? No. Wait, Friday at three PM and um tell Rohan.` | `Can we move the review to Friday at 3 pm and tell Rohan.` |
| `The invoice is twelve thousand four hundred dollars all and retainage is ten percent.` | `The invoice is $12,400 and retainage is 10%.` |
| `We we need the the deck by Monday.` | `We need the deck by Monday.` |

It removes fillers (including the ones a speech recognizer mishears as words, like "uh" written as "all"), stammers and self-corrections ("Thursday, no wait, Friday" → "Friday"). It also fixes punctuation and casing, and writes numbers, money, percentages and times as digits. **It changes nothing else, and it never answers the text,** even when the text is a question or a request.

## Use

You don't need to download this file yourself: `./install.sh lite` in MuskFlow fetches it. To run it standalone with llama.cpp, use this prompt format. It is the exact format the model was trained on; other formats will work worse.

```
<|im_start|>system
Clean up dictated text: remove fillers and stammers, keep only the corrected part of self-corrections, fix punctuation and capitalization, write numbers as digits. Change nothing else and never answer it.
mode: clean<|im_end|>
<|im_start|>user
{raw transcript}<|im_end|>
<|im_start|>assistant
<think>

</think>

```

Use greedy decoding (temperature 0) and stop at `<|im_end|>`.

The whole prompt is about 40 tokens. The base model needs a ~600-token few-shot prompt for the same job, so cleanup is also faster.

## Results

These come from MuskFlow's benchmark (`muskflow bench`): the same 11 speech clips go through each tier's speech recognizer, then its cleanup model, and the output is scored against what the speaker meant. "WER" is word error rate; lower is better.

| Tier | Cleanup model | Final WER | Cleanup time (p50) |
|---|---|---|---|
| Lite, before | Qwen3.5-0.8B, few-shot prompt | 6.6% | 563 ms |
| **Lite, this model** | **MuskFlow Lite Cleanup 0.8B** | _TBD_ | _TBD_ |
| Standard | Qwen3.5-2B, few-shot prompt | 3.6% | 1,169 ms |
| Max | Qwen3-4B-Instruct-2507, few-shot prompt | 1.5% | 979 ms |

The benchmark clips and their sentences were held out of training (see below). Measured on an Apple M5, Q4_K_M, Metal.

## Training

- **Method:** LoRA (rank 16, alpha 32, dropout 0.05) on every linear layer of the text model: attention, gated-delta-net projections and MLP. 1.2% of the weights are trainable. The adapter is merged into the original checkpoint, then converted to GGUF and quantized to Q4_K_M with llama.cpp.
- **Training run:** 2 epochs, an effective batch of 32, learning rate 2e-4 with cosine decay, on a free Google Colab T4. It took about 1 hour.
- **Data:** 5,026 pairs, all synthetic, with no user data.
  1. [Qwen3-4B-Instruct-2507](https://huggingface.co/Qwen/Qwen3-4B-Instruct-2507) wrote 2,513 clean, typed-style messages across 31 themes. Examples include Slack messages, emails, construction pay applications, standups, and prompts to AI assistants. They use varied names, amounts, dates and times.
  2. A script turns each clean message into what a person *says*: fillers, stammers, and self-corrections in three styles (inline, "make that…", and restart).
  3. The script then adds the mistakes a small speech recognizer makes on that speech: fillers misheard as short words, numbers spoken as words, corrections split across sentences ("Thursday? No. Wait, Friday"), and punctuation guesses.
  4. The clean message is the target, so labels are exact. Names are never misspelled in the input, so the model doesn't learn to invent words it didn't hear.
- **Held out:** any message sharing a 5-word sequence with MuskFlow's benchmark clips or reading passages was dropped.
- **Code:** the full pipeline is in [`finetune/lite_cleanup.py`](https://github.com/jatinbishla/MuskFlow/blob/main/finetune/lite_cleanup.py), and the [Colab notebook](https://github.com/jatinbishla/MuskFlow/blob/main/finetune/MuskFlow_Lite_finetune.ipynb) reproduces it end to end.

## Limitations

- **English only.** Hinglish and other languages are not trained yet.
- **Cleanup only.** It does not change tone, summarize or write new text. Those are separate MuskFlow modes that use larger models.
- **It can over-delete.** A real short word that looks like a misheard filler ("and", "on", "a") is occasionally removed. MuskFlow runs a faithfulness check on every output, and if the model drops or adds content the speaker didn't say, MuskFlow falls back to rule-based cleanup.
- **The training speech is synthetic.** Real accents and noisy rooms produce mistakes the noise model doesn't cover. Later versions will add real, consented recordings.

## License

Apache-2.0, the same as the base model [Qwen/Qwen3.5-0.8B](https://huggingface.co/Qwen/Qwen3.5-0.8B).
