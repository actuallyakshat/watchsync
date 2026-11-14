# WatchSync

Watch a movie with friends from separate Macs, in sync. Everyone plays their
own local copy; WatchSync keeps the players together. Pause, play or seek on
any Mac and it happens for everyone. No accounts.

## Install

With [Homebrew](https://brew.sh):

```bash
brew install --cask actuallyakshat/tap/watchsync
```

No Homebrew? Paste this into Terminal (installs Homebrew, then WatchSync):

```bash
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/actuallyakshat/watchsync/main/scripts/install.sh)"
```

Both also install mpv, the video player WatchSync uses, and open without a
security warning. Update with `brew upgrade --cask watchsync`.

## Features

- **Global play, pause and seek** from the app or the movie window (Space,
  arrow keys, seek bar).
- **Stays in sync**: players drift-correct by nudging speed ±5% (inaudible),
  and jump only when more than 1.5 s off.
- **Same picture for everyone**: pick a crop (16:9, 1.85:1, 2.39:1, 4:3) and
  every player shows exactly the same part of the frame.
- **File check**: shows whether everyone's copy matches the host's.
- **In-app updates**: a signed update feed; click *Update* in the sidebar.

## Build a release

Bump the version in `Cargo.toml`, `desktop/src-tauri/Cargo.toml`,
`desktop/src-tauri/tauri.conf.json` and `desktop/package.json`, commit and
push, then:

```bash
./scripts/release.sh   # universal app, GitHub release, update feed, Homebrew cask
```

Updates are signed with the key at `~/.tauri/watchsync.key` (not in the repo;
keep a backup, or installed apps can't be updated). The public half is in
`tauri.conf.json`.

## Run from source

```bash
cd desktop && npm install
npx tauri dev
```

## Tests

```bash
cargo test                 # engine: sync math, real mpv, real network, e2e
cd desktop && npm test     # UI
```

The Rust tests need `mpv` and `ffmpeg` (`brew install ffmpeg`) and internet
(iroh relay). The end-to-end test runs a three-person watch party with
headless players: play, pause from a guest's movie window, seeks both ways,
drift recovery, a shared 2.39:1 crop and a late joiner.

## How sync works

- The host owns a `PlaybackState { playing, position, anchor_ms, version }`.
  From it, any peer can compute where the movie should be at any moment.
- Guests send *intents* (play/pause/seek/crop) to the host, which orders them
  and broadcasts the result. Play starts 500 ms in the future, so everyone
  starts at the same instant.
- Guests estimate the host's clock offset NTP-style (lowest-RTT ping wins).
- Crops are computed from the video's own size and sent as an aspect ratio, so
  they match on every screen; local zoom/pan keys are reset when applied.
- Networking is iroh (QUIC, peer-to-peer with NAT hole punching and a public
  relay fallback). The invite code encodes the host's address.

## Code map

| Path | What |
|---|---|
| `src/sync.rs` | Pure sync math: state, drift correction, clock offset, crop geometry |
| `src/engine.rs` | The coordinator: room, intents, player control loop |
| `src/mpv.rs` | Launches mpv and talks to its JSON IPC socket |
| `src/net.rs` | iroh transport, wire messages, invite codes, version check |
| `src/fingerprint.rs` | Quick file identity check |
| `desktop/src-tauri` | The macOS app: bridges the engine to the window |
| `desktop/src` | The UI (React), styled with Tars's design tokens |

## Known limits

- If the host leaves, the room ends (no host migration yet).
- Friends on 0.1.x need `brew upgrade --cask watchsync` once; in-app updates
  start with 0.2.0.
- Use headphones if you're also on a call, or you'll hear each other's audio echo.
