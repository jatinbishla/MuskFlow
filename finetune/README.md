# Fine-tuning MuskFlow's models

MuskFlow fine-tunes a model when end-to-end benchmarks show it falling short, and publishes each fine-tune as its own Hugging Face repo. `install.sh` downloads it, so users never fetch models by hand.

## Lite cleanup model (Qwen3.5-0.8B)

| File | What it does |
|---|---|
| `lite_cleanup.py` | The whole pipeline: `gen` (a teacher writes clean messages) → `pairs` (derive the messy speech Lite's ASR produces) → `train` (LoRA) → `export` (merge into the original checkpoint) → `check` (held-out score) |
| `MuskFlow_Lite_finetune.ipynb` | Runs it on a free Colab T4 in about 1.5 hours, saving to Google Drive as it goes. Rebuild with `python3 build_notebook.py` after editing `lite_cleanup.py` |
| `MODEL_CARD.md` | The Hugging Face README for the published model |
| `publish.sh` | Uploads the model and card (needs `hf auth login`) |

**Run it:** open [the notebook in Colab](https://colab.research.google.com/github/jatinbishla/MuskFlow/blob/main/finetune/MuskFlow_Lite_finetune.ipynb), choose a T4 GPU, then *Run all*.

**Ship it:**

1. Put `MuskFlow-Lite-Cleanup-0.8B-Q4_K_M.gguf` in `models/`.
2. Compare it against the current Lite model and the other tiers:
   ```
   muskflow bench --tiers lite --dictionary bench/dictionary.txt --llm MuskFlow-Lite-Cleanup-0.8B-Q4_K_M.gguf --show
   ```
   MuskFlow-* models automatically get the compact prompt they were trained on.
3. If it wins, fill in the results in `MODEL_CARD.md` and run `finetune/publish.sh`.
4. Point `Tier::Lite` in `crates/mf-models/src/lib.rs` at the new repo and file.

The training data is synthetic, and the benchmark clips and reading passages are held out by 5-gram overlap, so `muskflow bench` stays a fair test.
