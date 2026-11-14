#!/bin/bash
# WatchSync installer for Macs without Homebrew. Run in Terminal:
#   /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/actuallyakshat/watchsync/main/scripts/install.sh)"
# Installs Homebrew if needed, then: brew install --cask actuallyakshat/tap/watchsync
# (which also installs mpv, the video player, and croc, for sending movies),
# and opens the app.
set -euo pipefail

bold() { printf '\n\033[1m%s\033[0m\n' "$1"; }
[ "$(uname -s)" = "Darwin" ] || { echo "WatchSync is for macOS only."; exit 1; }

find_brew() {
  for b in /opt/homebrew/bin/brew /usr/local/bin/brew; do
    [ -x "$b" ] && { echo "$b"; return; }
  done
  command -v brew || true
}

BREW=$(find_brew)
if [ -z "$BREW" ]; then
  bold "Installing Homebrew (the Mac package manager)…"
  echo "It will ask for your Mac password. Nothing shows while you type; that's normal."
  /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
  BREW=$(find_brew)
  [ -n "$BREW" ] || { echo "Homebrew install failed. Run this script again."; exit 1; }
fi

bold "Installing WatchSync, mpv (the video player) and croc (for sending movies)…"
if "$BREW" list --cask watchsync >/dev/null 2>&1; then
  "$BREW" upgrade --cask actuallyakshat/tap/watchsync || true
else
  "$BREW" install --cask actuallyakshat/tap/watchsync
fi

bold "All set! Opening WatchSync."
open -a WatchSync
