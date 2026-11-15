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

Both also install WatchSync's helper apps, mpv (the video player) and croc
(for sending movies), and open without a security warning. Update with `brew upgrade --cask watchsync`.

## Features

- **Global play, pause and seek** from the app or the movie window (Space,
  arrow keys, seek bar).
- **Stays in sync**: players drift-correct by nudging speed ±5% (inaudible),
  and jump only when more than 1.5 s off.
- **Invite links**: *Copy invite link* gives a link friends tap to open
  WatchSync straight into the room.
- **Survives trouble**: if your connection blinks, you rejoin on your own
  while the movie keeps playing. If the host leaves or their Mac goes to
  sleep, the next person who joined takes over as host.
- **Video call built in**: being in a room is being in the call (mic and
  camera off until you turn them on). The call floats in a small
  always-on-top window you can put anywhere, even over the movie in
  fullscreen: a strip showing who's here, or expanded to show whoever's
  talking or everyone. It comes back where you left it.
  - **⌥⌘M / ⌥⌘V** toggle your mic and camera from any app while you're in
    a room (change them in Settings). In the call window, Space, arrows and
    F still control the movie.
  - **Quality to match**: each friend is sent video only as big as you show
    them (up to 1080p30), and none while the call is shrunk.
  - **Your own mix**: a Voices volume and one per friend; pick your camera
    and mic in the call window's settings.
  - **Clean mic out of the box**: a voice gate keeps typing, mouse clicks
    and room noise out (it follows your room, no setting needed), on top of
    the system's echo cancellation and noise suppression. Call settings show
    a live meter, and let you turn it off, set sensitivity by hand, and
    change your input volume.
  - **Headphones**: on speakers with your mic on, it asks for headphones,
    since friends would hear the movie twice.
  - **Gets through**: if two Macs can't connect directly, the call relays
    through iroh, like the rest of WatchSync (Troubleshooting can force it).
- **Chat and reactions**, in the sidebar and on the movie itself.
- **Same picture for everyone**: pick a crop (4:3, IMAX 1.43:1, 1.66:1, 16:9,
  1.85:1, IMAX 1.90:1, 2.00:1, 2.20:1, 2.39:1, 2.76:1) and every player shows
  exactly the same part of the frame.
- **Same subtitle size for everyone**: set it in the app, or with F and G in
  the movie window, and every player follows.
- **Just for you**: your own audio track and subtitles, volume levelling and
  dialogue boost.
- **File check**: shows whether everyone's copy matches the host's.
- **Finds your copy**: if a file with the host's exact name is on your Mac
  (Spotlight), WatchSync suggests opening it.
- **Send the movie**: *Share my copy*, and friends without it click *Get
  Riya's copy*. Sent with [croc](https://github.com/schollz/croc), one
  private code per download, checked on arrival, saved to
  `~/Movies/WatchSync` and opened. Both Macs stay awake while the transfer
  runs (`caffeinate`, tied to croc; on battery only idle sleep is
  prevented, and closing a laptop's lid still sleeps it).
- **In-app updates**: a signed update feed; Settings → Check for updates.

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

The Rust tests need `mpv`, `croc` and `ffmpeg` (`brew install ffmpeg`) and
internet (iroh and croc relays). The end-to-end test runs a three-person watch party with
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
| `src/share.rs` | Sending movies with croc, keeping the Mac awake |
| `desktop/src-tauri` | The macOS app: bridges the engine to the window |
| `desktop/src` | The UI (React), styled with Tars's design tokens |
| `docs/join` | The invite-link page (GitHub Pages) that opens the app |

## Known limits

- After the host changes, the old invite link stops working; the new host's
  *Copy invite link* gives a fresh one.
- Friends on 0.2.x can't take over as host, see chat, or use invite links;
  they lose the room if the host leaves. Everyone should update.
- Friends on 0.1.x need `brew upgrade --cask watchsync` once; in-app updates
  start with 0.2.0.
- Use headphones if you're also on a call, or you'll hear each other's audio echo.
