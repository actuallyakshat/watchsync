# WatchSync

Watch a movie with friends from separate Macs, in sync. Everyone plays their
own local copy; WatchSync keeps the players together. Pause, play or seek on
any Mac and it happens for everyone. No accounts.

## Install

Open **Terminal**, paste this line and press Return:

```bash
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/actuallyakshat/watchsync/main/scripts/install.sh)"
```

It installs WatchSync into Applications, installs the video player (mpv,
plus Homebrew if needed; it may ask for your Mac password), and opens the app.
Run the same line again to update.

Prefer downloading by hand? Get `WatchSync-macos.zip` from the
[latest release](https://github.com/actuallyakshat/watchsync/releases/latest).
The app isn't notarized by Apple (that needs a paid developer account), so a
browser download shows a "can't verify" warning the first time: open it via
System Settings → Privacy & Security → **Open Anyway**. The Terminal installer
avoids this because macOS only checks files downloaded through browsers and
chat apps.

## Build a release

```bash
./scripts/bundle-macos.sh   # → dist/WatchSync-macos.zip (universal)
swift scripts/make-icon.swift  # regenerate assets/WatchSync.icns
```

## Run from source

```bash
cargo run --release
```

1. One person clicks **Host a room**, then **Copy room code**, and sends the code
   (WhatsApp, iMessage…).
2. Friends copy the code and click **Join with copied code**.
3. Everyone clicks **Choose movie…** and picks their copy. The movie opens in an
   mpv window, paused at the room's position.
4. Press Play. Space, arrow keys and the seek bar in the mpv window are shared
   with everyone too.

The **Watching** list shows whether each friend's file matches the host's
(size + hash of the first/last 4 MB) and how far off each player is, in ms.

## Tests

```bash
cargo test
```

Needs `mpv` and `ffmpeg` (`brew install ffmpeg`) plus internet (iroh relay).

- `src/sync.rs`: unit tests for the timing math.
- `tests/mpv.rs`: drives a real headless mpv (start position, real-time play,
  frame-exact seeks, speed, user pause/seek events, exit).
- `tests/net.rs`: real host/guest connection, message ordering, leave detection.
- `tests/e2e.rs`: a full watch party with three engines and players: play,
  pause from the guest's mpv window, seeks both ways, drift recovery, late
  joiner. Prints the measured offsets.

GPUI shaders compile at app start (`runtime_shaders` feature), so Xcode's
Metal toolchain isn't needed to build.

## How sync works

- The host owns a `PlaybackState { playing, position, anchor_ms, version }`.
  From it, any peer can compute where the movie should be at any moment.
- Guests send *intents* (play/pause/seek) to the host, which orders them and
  broadcasts the new state. Play starts 500 ms in the future, so everyone
  starts at the same instant.
- Guests estimate the host's clock offset NTP-style (lowest-RTT ping wins).
- Every 0.5 s each player compares its position to the expected one. Under
  60 ms it does nothing; up to 1.5 s it nudges mpv's speed ±5% (pitch-corrected,
  so you won't hear it); beyond that it seeks.
- Networking is iroh (QUIC, peer-to-peer with NAT hole punching and a public
  relay fallback). The room code encodes the host's address.

## Code map

| File | What |
|---|---|
| `src/sync.rs` | Pure sync math: state, drift correction, clock offset (unit tests) |
| `src/engine.rs` | The coordinator: room, intents, player control loop |
| `src/mpv.rs` | Launches mpv and talks to its JSON IPC socket |
| `src/net.rs` | iroh transport, wire messages, room codes |
| `src/fingerprint.rs` | Quick file identity check |
| `src/lib.rs` | Exposes the modules above (for tests) |
| `src/main.rs` | GPUI window |

## Known limits (v1)

- If the host leaves, the room ends (no host migration yet).
- No "wait until everyone has loaded" barrier after seeks; fine for local files.
- Use headphones if you're also on a call, or you'll hear each other's audio echo.
