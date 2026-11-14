#!/usr/bin/env bash
# Publish a WatchSync release: GitHub release + in-app update feed + Homebrew cask.
#
#   scripts/release.sh
#
# Bump the version first (it must match in all four places below), commit and
# push main. Needs the update-signing key at ~/.tauri/watchsync.key.
#
# Uploads to the GitHub release:
#   WatchSync-<v>-macos-universal.zip          what Homebrew installs
#   WatchSync-<v>-macos-universal.app.tar.gz   what the in-app updater installs
#   latest.json                                the in-app update feed (signed)
# then writes Casks/watchsync.rb in actuallyakshat/homebrew-tap.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

REPO=actuallyakshat/watchsync
TAP=actuallyakshat/homebrew-tap
KEY="$HOME/.tauri/watchsync.key"

# One version, everywhere.
V=$(python3 -c "import json;print(json.load(open('desktop/src-tauri/tauri.conf.json'))['version'])")
for f in Cargo.toml desktop/src-tauri/Cargo.toml; do
  grep -q "^version = \"$V\"" "$f" || { echo "$f isn't at version $V" >&2; exit 1; }
done
grep -q "\"version\": \"$V\"" desktop/package.json || { echo "desktop/package.json isn't at $V" >&2; exit 1; }
TAG="v$V"

[[ -f "$KEY" ]] || { echo "Missing update-signing key: $KEY" >&2; exit 1; }
[[ -z "$(git status --porcelain)" ]] || { echo "Commit your changes first." >&2; exit 1; }
git fetch --quiet origin main
[[ "$(git rev-parse HEAD)" == "$(git rev-parse origin/main)" ]] || {
  echo "Push main first (HEAD must match origin/main)." >&2; exit 1; }
if gh release view "$TAG" --repo "$REPO" >/dev/null 2>&1; then
  echo "Release $TAG already exists. Bump the version." >&2; exit 1
fi

# Build one app for Apple Silicon + Intel, plus the signed updater archive.
(cd desktop && npm ci --silent && \
  TAURI_SIGNING_PRIVATE_KEY="$(cat "$KEY")" TAURI_SIGNING_PRIVATE_KEY_PASSWORD="" \
  npx tauri build --target universal-apple-darwin --bundles app)

BUNDLE=target/universal-apple-darwin/release/bundle/macos
OUT=dist && rm -rf "$OUT" && mkdir -p "$OUT"
ZIP="WatchSync-$V-macos-universal.zip"
TGZ="WatchSync-$V-macos-universal.app.tar.gz"
codesign -v "$BUNDLE/WatchSync.app"
(cd "$BUNDLE" && ditto -c -k --keepParent WatchSync.app "$OLDPWD/$OUT/$ZIP")
cp "$BUNDLE/WatchSync.app.tar.gz" "$OUT/$TGZ"
SIG=$(cat "$BUNDLE/WatchSync.app.tar.gz.sig")

URL="https://github.com/$REPO/releases/download/$TAG/$TGZ"
python3 - "$V" "$SIG" "$URL" > "$OUT/latest.json" <<'PY'
import json, sys, datetime
v, sig, url = sys.argv[1:4]
entry = {"signature": sig, "url": url}
print(json.dumps({
    "version": v,
    "notes": f"WatchSync {v}",
    "pub_date": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "platforms": {"darwin-aarch64": entry, "darwin-x86_64": entry},
}, indent=2))
PY

gh release create "$TAG" "$OUT/$ZIP" "$OUT/$TGZ" "$OUT/latest.json" --repo "$REPO" \
  --target main --title "WatchSync $V" --notes-file scripts/release-notes.md

SHA=$(shasum -a 256 "$OUT/$ZIP" | cut -d' ' -f1)
CASK=$(cat <<RUBY
cask "watchsync" do
  version "$V"
  sha256 "$SHA"

  url "https://github.com/$REPO/releases/download/v#{version}/WatchSync-#{version}-macos-universal.zip"
  name "WatchSync"
  desc "Watch local movies in sync with friends on separate Macs"
  homepage "https://github.com/$REPO"

  depends_on formula: "mpv"
  depends_on formula: "croc"
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
existing=$(gh api "repos/$TAP/contents/Casks/watchsync.rb" --jq .sha 2>/dev/null || true)
args=(-X PUT "repos/$TAP/contents/Casks/watchsync.rb" -f message="watchsync $V"
      -f content="$(printf '%s\n' "$CASK" | base64)")
[[ -n "$existing" ]] && args+=(-f sha="$existing")
gh api "${args[@]}" --jq .commit.html_url

echo "Released $TAG. Install: brew install --cask actuallyakshat/tap/watchsync"
