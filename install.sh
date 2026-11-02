#!/usr/bin/env bash
# MuskFlow installer: builds the app and downloads the model tiers you choose.
# Everything comes through this one script; there's nothing else to clone or download.
#
#   ./install.sh                 # asks which tier, recommending one for this machine
#   ./install.sh lite            # lite | standard | max | all (several allowed)
set -euo pipefail
cd "$(dirname "$0")"

say() { printf '\033[1m%s\033[0m\n' "$*"; }

# ── 1. Rust toolchain ────────────────────────────────────────────────────────
for dir in "$HOME/.cargo/bin" /opt/homebrew/opt/rustup/bin /usr/local/opt/rustup/bin; do
  [ -x "$dir/cargo" ] && PATH="$dir:$PATH"
done
if ! command -v cargo >/dev/null; then
  say "MuskFlow is built with Rust, which isn't installed."
  read -r -p "Install it now with rustup (https://rustup.rs)? [Y/n] " yn
  case "${yn:-y}" in
    [Yy]*) curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
           PATH="$HOME/.cargo/bin:$PATH" ;;
    *) echo "Install Rust from https://rustup.rs and run ./install.sh again."; exit 1 ;;
  esac
fi

# ── 2. Pick tiers ────────────────────────────────────────────────────────────
ram_gb() {
  case "$(uname -s)" in
    Darwin) echo $(( $(sysctl -n hw.memsize) / 1073741824 )) ;;
    Linux)  echo $(( $(awk '/MemTotal/ {print $2}' /proc/meminfo) / 1048576 )) ;;
    *)      echo 8 ;;
  esac
}
TIERS=("$@")
if [ ${#TIERS[@]} -eq 0 ]; then
  RAM=$(ram_gb)
  if [ "$RAM" -lt 8 ]; then REC=lite; else REC=standard; fi
  say "Which model tier? (this machine: ${RAM} GB RAM → recommended: $REC)"
  echo "  1) lite      ~650 MB  fastest, any laptop (English)"
  echo "  2) standard  ~1.8 GB  best balance (25 European languages)"
  echo "  3) max       ~3.1 GB  most accurate, 99 languages incl. Hindi; needs a fast GPU/Apple Silicon to feel instant"
  echo "  4) all       ~5.5 GB  all three (to compare them with \`muskflow bench\`)"
  read -r -p "Choose 1-4 [$REC]: " choice
  case "${choice:-}" in
    1) TIERS=(lite) ;; 2) TIERS=(standard) ;; 3) TIERS=(max) ;; 4) TIERS=(all) ;;
    "") TIERS=("$REC") ;;
    *) TIERS=("$choice") ;;
  esac
fi

# ── 3. Build ─────────────────────────────────────────────────────────────────
say "Building MuskFlow (first build takes a few minutes)…"
cargo build --release --quiet
BIN="$PWD/target/release/muskflow"

# ── 4. Models ────────────────────────────────────────────────────────────────
say "Downloading models: ${TIERS[*]}"
"$BIN" models install "${TIERS[@]}"

TIER="${TIERS[0]}"; [ "$TIER" = all ] && TIER=standard
say "Done. Try it:"
echo "  $BIN listen --tier $TIER        # dictate; press Enter to stop"
echo "  $BIN models list                  # what's installed"
echo "  $BIN models install max          # add another tier any time"
[ "$(uname -s)" = Darwin ] && echo "  (macOS will ask for microphone access the first time.)"
