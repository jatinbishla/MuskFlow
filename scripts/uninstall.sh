#!/usr/bin/env bash
# Removes Umless from this Mac: the app, its macOS permission entries (Microphone,
# Accessibility, Input Monitoring), Launch at Login, your history, dictionary and
# settings, its log, and the local signing certificate.
#
# This folder and its downloaded models stay. Delete the folder to remove those too.
#
#   scripts/uninstall.sh              # remove everything
#   scripts/uninstall.sh --keep-data  # keep history, dictionary and settings
set -uo pipefail
cd "$(dirname "$0")/.."

ID="io.github.jatinbishla.umless"
OLD_ID="io.github.jatinbishla.muskflow"   # Umless's name before the rename: MuskFlow
KEEP_DATA=0
[ "${1:-}" = "--keep-data" ] && KEEP_DATA=1

say() { printf '\033[1m%s\033[0m\n' "$*"; }

say "Quitting Umless…"
pkill -x Umless 2>/dev/null && sleep 1
pkill -x MuskFlow 2>/dev/null && sleep 1

say "Removing Umless's permission entries…"
# Clears Umless's rows in System Settings → Privacy & Security, so the next
# install is asked afresh. Other apps' permissions are untouched.
for id in "$ID" "$OLD_ID"; do
  for service in Microphone Accessibility ListenEvent PostEvent; do
    tccutil reset "$service" "$id" >/dev/null 2>&1 && echo "  $service ($id): cleared"
  done
done

say "Removing the app…"
for app in /Applications/Umless.app "$HOME/Applications/Umless.app" dist/Umless.app \
           /Applications/MuskFlow.app "$HOME/Applications/MuskFlow.app" dist/MuskFlow.app; do
  [ -e "$app" ] && rm -rf "$app" && echo "  removed $app"
done
# Launch at Login goes with the app: macOS drops a login item whose app is gone.

if [ "$KEEP_DATA" = 0 ]; then
  say "Removing history, dictionary, settings and log…"
  for name in Umless MuskFlow; do
    [ -e "$HOME/Library/Application Support/$name" ] && rm -rf "$HOME/Library/Application Support/$name" \
      && echo "  removed ~/Library/Application Support/$name"
    [ -e "$HOME/Library/Logs/$name.log" ] && rm -f "$HOME/Library/Logs/$name.log" && echo "  removed ~/Library/Logs/$name.log"
  done
fi

say "Removing the local signing certificate…"
for cert in "Umless Local Signing" "MuskFlow Local Signing"; do
  security delete-identity -c "$cert" >/dev/null 2>&1 && echo "  removed \"$cert\""
done

say "Umless is uninstalled. Reinstall any time with ./install.sh"
