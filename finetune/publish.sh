#!/usr/bin/env bash
# Publishes the fine-tuned Lite cleanup model to Hugging Face as its own repo.
# Run it yourself after the benchmark shows the model wins; it needs your HF login
# (`finetune/.venv/bin/hf auth login`, with a write token).
#
#   finetune/publish.sh [repo]      # default: jatinbishla/Umless-Lite-Cleanup-0.8B-GGUF
set -euo pipefail
cd "$(dirname "$0")/.."

REPO="${1:-jatinbishla/Umless-Lite-Cleanup-0.8B-GGUF}"
GGUF=models/Umless-Lite-Cleanup-0.8B-Q4_K_M.gguf
HF=finetune/.venv/bin/hf

[ -f "$GGUF" ] || { echo "missing $GGUF (download it from Google Drive → umless/)"; exit 1; }
if grep -q "_TBD_" finetune/MODEL_CARD.md; then
  echo "finetune/MODEL_CARD.md still has _TBD_ results; fill them in from the benchmark first"; exit 1
fi

"$HF" repos create "$REPO" --type model --public --exist-ok
"$HF" upload "$REPO" finetune/MODEL_CARD.md README.md
"$HF" upload "$REPO" "$GGUF" "$(basename "$GGUF")"
echo "published https://huggingface.co/$REPO"
