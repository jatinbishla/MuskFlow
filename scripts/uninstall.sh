#!/usr/bin/env bash
# Removes MuskFlow from this Mac: the app, its macOS permission entries (Microphone,
# Accessibility, Input Monitoring), Launch at Login, your history, dictionary and
# settings, its log, and the local signing certificate.
#
# This folder and its downloaded models stay. Delete the folder to remove those too.
#
#   scripts/uninstall.sh              # remove everything
#   scripts/uninstall.sh --keep-data  # keep history, dictionary and settings
set -uo pipefail
cd "$(dirname "$0")/.."

ID="io.github.jatinbishla.muskflow"
KEEP_DATA=0
[ "${1:-}" = "--keep-data" ] && KEEP_DATA=1

say() { printf '\033[1m%s\033[0m\n' "$*"; }

say "Quitting MuskFlow…"
pkill -x MuskFlow 2>/dev/null && sleep 1

say "Removing MuskFlow's permission entries…"
# Clears MuskFlow's rows in System Settings → Privacy & Security, so the next
# install is asked afresh. Other apps' permissions are untouched.
for service in Microphone Accessibility ListenEvent PostEvent; do
  tccutil reset "$service" "$ID" >/dev/null 2>&1 && echo "  $service: cleared"
done

say "Removing the app…"
for app in /Applications/MuskFlow.app "$HOME/Applications/MuskFlow.app" dist/MuskFlow.app; do
  [ -e "$app" ] && rm -rf "$app" && echo "  removed $app"
done
# Launch at Login goes with the app: macOS drops a login item whose app is gone.

if [ "$KEEP_DATA" = 0 ]; then
  say "Removing history, dictionary, settings and log…"
  rm -rf "$HOME/Library/Application Support/MuskFlow" && echo "  removed ~/Library/Application Support/MuskFlow"
  rm -f "$HOME/Library/Logs/MuskFlow.log" && echo "  removed ~/Library/Logs/MuskFlow.log"
fi

say "Removing the local signing certificate…"
security delete-identity -c "MuskFlow Local Signing" >/dev/null 2>&1 \
  && echo "  removed \"MuskFlow Local Signing\"" || echo "  none found"

say "MuskFlow is uninstalled. Reinstall any time with ./install.sh"
