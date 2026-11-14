#!/bin/bash
# WatchSync installer. Run in Terminal:
#   /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/actuallyakshat/watchsync/main/scripts/install.sh)"
# Installs WatchSync into Applications, makes sure mpv (the video player) is
# installed, and opens the app.
set -euo pipefail

URL="${WATCHSYNC_URL:-https://github.com/actuallyakshat/watchsync/releases/latest/download/WatchSync-macos.zip}"
bold() { printf '\n\033[1m%s\033[0m\n' "$1"; }

[ "$(uname -s)" = "Darwin" ] || { echo "WatchSync is for macOS only."; exit 1; }

# 1. The app
DEST="${WATCHSYNC_DEST:-/Applications}"
[ -w "$DEST" ] || DEST="$HOME/Applications"
mkdir -p "$DEST"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

bold "Downloading WatchSync…"
curl -fL --progress-bar -o "$TMP/WatchSync.zip" "$URL"
ditto -x -k "$TMP/WatchSync.zip" "$TMP"
osascript -e 'quit app "WatchSync"' >/dev/null 2>&1 || true
rm -rf "$DEST/WatchSync.app"
mv "$TMP/WatchSync.app" "$DEST/"
echo "Installed to $DEST/WatchSync.app"

# 2. The video player
find_brew() {
  for b in /opt/homebrew/bin/brew /usr/local/bin/brew; do
    [ -x "$b" ] && { echo "$b"; return; }
  done
  command -v brew || true
}

if [ -x /opt/homebrew/bin/mpv ] || [ -x /usr/local/bin/mpv ] || command -v mpv >/dev/null; then
  echo "mpv is already installed."
else
  BREW=$(find_brew)
  if [ -z "$BREW" ]; then
    bold "Installing Homebrew (needed for the video player)…"
    echo "It will ask for your Mac password. Nothing shows while you type; that's normal."
    /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
    BREW=$(find_brew)
    [ -n "$BREW" ] || { echo "Homebrew install failed. Run this script again."; exit 1; }
  fi
  bold "Installing mpv (the video player)… this takes a minute or two."
  HOMEBREW_NO_AUTO_UPDATE=1 "$BREW" install mpv
fi

bold "All set! Opening WatchSync."
[ -n "${WATCHSYNC_NO_OPEN:-}" ] || open "$DEST/WatchSync.app"
