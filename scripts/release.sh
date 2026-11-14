#!/usr/bin/env bash
# Publish a WatchSync release and update the Homebrew cask.
#
#   scripts/release.sh            # releases the version in Cargo.toml
#
# Steps: build the universal app, create GitHub release v<version> with the
# zip, then write Casks/watchsync.rb in actuallyakshat/homebrew-tap.
# Bump `version` in Cargo.toml and push main first.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

REPO=actuallyakshat/watchsync
TAP=actuallyakshat/homebrew-tap
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
TAG="v$VERSION"
ZIP="dist/WatchSync-${VERSION}-macos-universal.zip"

[[ -z "$(git status --porcelain)" ]] || { echo "Commit your changes first." >&2; exit 1; }
git fetch --quiet origin main
[[ "$(git rev-parse HEAD)" == "$(git rev-parse origin/main)" ]] || {
  echo "Push main first (HEAD must match origin/main)." >&2; exit 1; }
if gh release view "$TAG" --repo "$REPO" >/dev/null 2>&1; then
  echo "Release $TAG already exists. Bump the version in Cargo.toml." >&2; exit 1
fi

./scripts/bundle-macos.sh
SHA=$(shasum -a 256 "$ZIP" | cut -d' ' -f1)

gh release create "$TAG" "$ZIP" --repo "$REPO" --target main \
  --title "WatchSync $VERSION" --notes-file scripts/release-notes.md

CASK=$(cat <<RUBY
cask "watchsync" do
  version "$VERSION"
  sha256 "$SHA"

  url "https://github.com/$REPO/releases/download/v#{version}/WatchSync-#{version}-macos-universal.zip"
  name "WatchSync"
  desc "Watch local movies in sync with friends on separate Macs"
  homepage "https://github.com/$REPO"

  depends_on formula: "mpv"
  depends_on :macos

  app "WatchSync.app"

  # Not Apple-notarized; clear quarantine so macOS opens it.
  postflight_steps do
    run "/usr/bin/xattr",
        args:           ["-dr", "com.apple.quarantine", "{{appdir}}/WatchSync.app"],
        writable_paths: ["WatchSync.app"],
        writable_base:  :appdir
  end
end
RUBY
)

existing_sha=$(gh api "repos/$TAP/contents/Casks/watchsync.rb" --jq .sha 2>/dev/null || true)
args=(-X PUT "repos/$TAP/contents/Casks/watchsync.rb"
      -f message="watchsync $VERSION"
      -f content="$(printf '%s\n' "$CASK" | base64)")
[[ -n "$existing_sha" ]] && args+=(-f sha="$existing_sha")
gh api "${args[@]}" --jq .commit.html_url

echo "Released $TAG. Install: brew install --cask actuallyakshat/tap/watchsync"
