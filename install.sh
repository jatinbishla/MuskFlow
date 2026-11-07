#!/usr/bin/env bash
# Umless installer: builds the app, downloads the model tiers you choose and installs it:
# Umless.app on a Mac, `umless` plus an app-menu entry on Linux. Everything comes through
# this one script; there's nothing else to clone or download. (Windows: install.ps1.)
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
  say "Umless is built with Rust, which isn't installed."
  read -r -p "Install it now with rustup (https://rustup.rs)? [Y/n] " yn
  case "${yn:-y}" in
    [Yy]*) curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
           PATH="$HOME/.cargo/bin:$PATH" ;;
    *) echo "Install Rust from https://rustup.rs and run ./install.sh again."; exit 1 ;;
  esac
fi

# ── 1b. C/C++ build tools (llama.cpp is compiled from source) ────────────────
if [ "$(uname -s)" = Darwin ] && ! xcode-select -p >/dev/null 2>&1; then
  say "Umless needs Apple's Command Line Tools (the C compiler). A macOS window will open to install them."
  xcode-select --install 2>/dev/null || true
  echo "When that install finishes, run ./install.sh again."
  exit 1
fi
# Linux: the C/C++ toolchain and the libraries the desktop app links (GTK, WebKitGTK,
# ALSA), plus the tools it types with. Installed with the system package manager.
if [ "$(uname -s)" = Linux ]; then
  if [ -n "${WAYLAND_DISPLAY:-}" ]; then TYPING="wl-clipboard wtype ydotool"; else TYPING="xclip xdotool"; fi
  if command -v apt-get >/dev/null; then
    PKGS="build-essential cmake clang libclang-dev pkg-config libasound2-dev libgtk-3-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev $TYPING"
    MISSING=$(for p in $PKGS; do dpkg -s "$p" >/dev/null 2>&1 || echo "$p"; done)
    INSTALL="sudo apt-get install -y"
  elif command -v dnf >/dev/null; then
    PKGS="gcc-c++ cmake clang clang-devel pkgconf alsa-lib-devel gtk3-devel webkit2gtk4.1-devel libayatana-appindicator-gtk3 $TYPING"
    MISSING=$(for p in $PKGS; do rpm -q "$p" >/dev/null 2>&1 || echo "$p"; done)
    INSTALL="sudo dnf install -y"
  elif command -v pacman >/dev/null; then
    PKGS="base-devel cmake clang pkgconf alsa-lib gtk3 webkit2gtk-4.1 libayatana-appindicator $TYPING"
    MISSING=$(for p in $PKGS; do pacman -Q "$p" >/dev/null 2>&1 || echo "$p"; done)
    INSTALL="sudo pacman -S --needed --noconfirm"
  else
    MISSING=""
    say "Unknown package manager: make sure a C++ compiler, cmake, clang, ALSA, GTK 3 and WebKitGTK 4.1 dev packages are installed."
  fi
  MISSING=$(echo $MISSING)
  if [ -n "$MISSING" ]; then
    say "Umless needs these system packages: $MISSING"
    read -r -p "Install them now ($INSTALL $MISSING)? [Y/n] " yn
    case "${yn:-y}" in
      # shellcheck disable=SC2086
      [Yy]*) $INSTALL $MISSING ;;
      *) echo "Install them and run ./install.sh again."; exit 1 ;;
    esac
  fi
fi
PATH="$PWD/.tools/cmake/bin:$PATH"
if ! command -v cmake >/dev/null; then
  if command -v brew >/dev/null; then
    say "Installing cmake with Homebrew…"
    brew install cmake
  elif [ "$(uname -s)" = Darwin ]; then
    # No Homebrew: Kitware's official build, kept inside this folder (.tools/).
    CMAKE_VERSION=3.31.6
    say "Downloading cmake $CMAKE_VERSION (needed to build the AI engine, ~75 MB)…"
    TMP="$(mktemp -d)"
    curl -fL --progress-bar -o "$TMP/cmake.tar.gz" \
      "https://github.com/Kitware/CMake/releases/download/v$CMAKE_VERSION/cmake-$CMAKE_VERSION-macos-universal.tar.gz"
    tar -xzf "$TMP/cmake.tar.gz" -C "$TMP"
    rm -rf .tools/cmake && mkdir -p .tools
    mv "$TMP/cmake-$CMAKE_VERSION-macos-universal/CMake.app/Contents" .tools/cmake
    rm -rf "$TMP"
  else
    echo "Install cmake (e.g. sudo apt install cmake build-essential) and run ./install.sh again."
    exit 1
  fi
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
  echo "  4) all       ~5.5 GB  all three (to compare them with \`umless bench\`)"
  read -r -p "Choose 1-4 [$REC]: " choice
  case "${choice:-}" in
    1) TIERS=(lite) ;; 2) TIERS=(standard) ;; 3) TIERS=(max) ;; 4) TIERS=(all) ;;
    "") TIERS=("$REC") ;;
    *) TIERS=("$choice") ;;
  esac
fi

# ── 3. Build ─────────────────────────────────────────────────────────────────
say "Building Umless (first build takes a few minutes)…"
cargo build --release --quiet
BIN="$PWD/target/release/umless"

# ── 4. Models ────────────────────────────────────────────────────────────────
say "Downloading models: ${TIERS[*]}"
"$BIN" models install "${TIERS[@]}"

TIER="${TIERS[0]}"; [ "$TIER" = all ] && TIER=standard

# ── 5. The app (macOS) ───────────────────────────────────────────────────────
if [ "$(uname -s)" = Darwin ]; then
  say "Installing Umless.app…"
  # A local signing certificate keeps macOS permissions across future updates.
  scripts/make-signing-cert.sh
  scripts/make-app.sh "$TIER"
  open /Applications/Umless.app
  say "Done. Umless is open: follow the setup screen (about a minute), then hold fn anywhere to dictate."
  echo "  It lives in the menu bar. Re-run ./install.sh any time to update; your permissions stay."
elif [ "$(uname -s)" = Linux ]; then
  say "Installing umless…"
  MODELS="${UMLESS_MODELS:-$PWD/models}"
  # The binary finds its speech libraries (sherpa-onnx, onnxruntime) next to itself
  # ($ORIGIN rpath), so both go in one folder, linked into ~/.local/bin.
  LIBDIR="$HOME/.local/lib/umless"
  mkdir -p "$HOME/.local/bin" "$HOME/.local/share/applications" "$LIBDIR"
  pkill -x umless 2>/dev/null || true
  install -m 755 "$BIN" "$LIBDIR/umless"
  cp -P target/release/*.so* "$LIBDIR/" 2>/dev/null || true
  ln -sf "$LIBDIR/umless" "$HOME/.local/bin/umless"
  {
    echo "[Desktop Entry]"
    echo "Type=Application"
    echo "Name=Umless"
    echo "Comment=Local voice dictation: hold a key, speak, and the text is typed"
    echo "Exec=\"$HOME/.local/bin/umless\" --models \"$MODELS\" --tier $TIER app --window"
    echo "Icon=audio-input-microphone"
    echo "Terminal=false"
    echo "Categories=Utility;Accessibility;"
  } > "$HOME/.local/share/applications/umless.desktop"
  if ! id -nG | tr ' ' '\n' | grep -qx input; then
    say "One more step, so Umless can see the hold key (Ctrl + Super):"
    echo "  sudo usermod -aG input \$USER     # then log out and back in"
    echo "  (On X11 Umless can watch the keyboard without it.)"
  fi
  say "Done. Start Umless from your app menu, or run:"
  echo "  ~/.local/bin/umless --models \"$MODELS\" --tier $TIER app --window"
  echo "  Hold Ctrl + Super anywhere and speak; let go to type. Ctrl + Super + Alt is command mode."
  echo "  UMLESS_HOLD_KEY=right-ctrl (or right-alt) picks another hold key."
  echo "  $BIN listen --tier $TIER        # or dictate in the terminal; press Enter to stop"
else
  say "Done. Try it:"
  echo "  $BIN listen --tier $TIER        # dictate; press Enter to stop"
fi
echo "  $BIN models install max          # add another tier any time, then pick it in Settings"
