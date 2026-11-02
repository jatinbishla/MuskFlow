#!/usr/bin/env bash
# Record yourself reading the benchmark passages, so `muskflow bench --clips recordings` tests real voices.
#
#   scripts/record-passages.sh            # all passages
#   scripts/record-passages.sh read-03    # just the ones matching
#
# Each passage is saved as recordings/<id>.wav with its reference text as recordings/<id>.txt.
# recordings/ is gitignored; your voice stays on your machine.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$ROOT/target/release/muskflow"
OUT="$ROOT/recordings"
mkdir -p "$OUT"
[ -x "$BIN" ] || { echo "build first: cargo build --release" >&2; exit 1; }

for txt in "$ROOT"/bench/passages/*.txt; do
  id="$(basename "$txt" .txt)"
  [[ -n "${1:-}" && "$id" != *"$1"* ]] && continue
  clear
  echo "━━━ $id ━━━"
  echo
  fold -s -w 90 "$txt"
  echo
  echo "Read it naturally, as you'd dictate (fillers and corrections included)."
  read -r -p "Press Enter to start recording… "
  "$BIN" listen --save "$OUT/$id.wav" || true
  cp "$txt" "$OUT/$id.txt"
  echo
  read -r -p "Saved $id. Press Enter for the next one (Ctrl-C to stop)… "
done
echo "Done. Compare tiers on your voice with: $BIN bench --clips recordings --show"
