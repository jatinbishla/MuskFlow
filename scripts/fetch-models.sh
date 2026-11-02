#!/usr/bin/env bash
# Downloads models into ./models (gitignored), by tier.
#
#   scripts/fetch-models.sh                 # standard (the default tier)
#   scripts/fetch-models.sh lite standard max
#
# The desktop app will use mf-models + a manifest instead; this is for local development.
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

model() { # sherpa archive name
  if [ ! -d "$1" ]; then
    fetch "$BASE/$1.tar.bz2" "$1.tar.bz2"
    tar xjf "$1.tar.bz2"
    rm "$1.tar.bz2"
  fi
  echo "✓ $1"
}

# Every tier uses Silero VAD.
fetch "$BASE/silero_vad.onnx" silero_vad.onnx

for tier in "${@:-standard}"; do
  case "$tier" in
    lite) model sherpa-onnx-nemo-parakeet_tdt_transducer_110m-en-36000-int8 ;;    # ~110 MB
    standard) model sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8 ;;                 # ~650 MB
    max) model sherpa-onnx-whisper-turbo ;;                                       # ~1 GB
    *) echo "unknown tier '$tier' (lite, standard, max)" >&2; exit 1 ;;
  esac
done
