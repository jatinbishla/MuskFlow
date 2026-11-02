#!/usr/bin/env bash
# Downloads the dev models into ./models (gitignored).
# The desktop app will use ff-models + models/manifest.json instead; this is for local development.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$ROOT/models"
BASE="https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models"
mkdir -p "$DEST"
cd "$DEST"

fetch() { # url out
  if [ -s "$2" ]; then echo "✓ $2"; return; fi
  echo "↓ $2"
  curl -fL --retry 3 -C - -o "$2.part" "$1"
  mv "$2.part" "$2"
}

fetch "$BASE/silero_vad.onnx" silero_vad.onnx

PARAKEET=sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8
if [ ! -d "$PARAKEET" ]; then
  fetch "$BASE/$PARAKEET.tar.bz2" "$PARAKEET.tar.bz2"
  tar xjf "$PARAKEET.tar.bz2"
  rm "$PARAKEET.tar.bz2"
fi
echo "✓ $PARAKEET"
ls -lh "$DEST/$PARAKEET"
