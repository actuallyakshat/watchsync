Watch the same movie with friends on separate Macs, in sync. Everyone plays their own copy; pause, play or seek on any Mac and it happens for everyone. No accounts.

## New in 0.5.1: a better call

- **Short invite codes**, like `abc-defg-hij`, instead of a long link. Long links still work.
- **See friends right away**: when someone turns their camera on, the call expands to show them. Dragging the strip bigger shows video too, and a green dot marks who has their camera on.
- **⌥⌘M / ⌥⌘V** turn your mic and camera on or off from any app, even with the movie fullscreen, while you're in a room. Change them in Settings → Call shortcuts.
- **Quality to match**: friends send you video only as big as you show it (up to 1080p30), and nothing while the call is shrunk.
- **Your own mix**: a Voices volume and one per friend. Pick your camera and mic in the call window's settings (the gear).
- **Clean mic out of the box**: typing, mouse clicks and background noise stay out, with nothing to set up. Your voice opens the mic; the call settings have a live meter, sensitivity and input volume if you want to tune it.
- **Headphones**: on speakers with your mic on, WatchSync asks for headphones, since friends would hear the movie twice.
- **Gets through**: if two Macs can't connect directly, the call relays through iroh. Call settings → Troubleshooting can force it and shows each friend's path.
- The call window comes back where you left it.
- **Subtitle size is the same for everyone**: change it in the app or with F and G in the movie window.

Everyone in the call needs 0.5.1.

## New in 0.5.0: video calls

Being in a room is now being in a call. Mic and camera start off; turn them on from the small call window that floats on top of everything, including the movie in fullscreen. Use headphones, or your friends will hear the movie twice.

## Install

With Homebrew:

```bash
brew install --cask actuallyakshat/tap/watchsync
```

Without Homebrew, paste this into Terminal (installs Homebrew, then WatchSync):

```bash
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/actuallyakshat/watchsync/main/scripts/install.sh)"
```

Both also install mpv, the video player WatchSync uses. Works on Apple Silicon and Intel Macs.

## Watch together

1. One person clicks **Host a room**, then **Copy room code**, and sends the code to the group.
2. Everyone else copies the code and clicks **Join with copied code**.
3. Everyone clicks **Choose movie…** and picks their copy. The **Watching** list shows **same file** in green when your copy matches the host's.
4. Press Play. Space, arrow keys and the seek bar in the movie window work for everyone too.

## Good to know

- Everyone needs the exact same file (same rip). Otherwise the timing will be off.
- Use headphones on the call.
- If the host quits, the next person who joined takes over.
