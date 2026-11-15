Watch the same movie with friends on separate Macs, in sync. Everyone plays their own copy; pause, play or seek on any Mac and it happens for everyone. No accounts.

## New in 0.5.0: video calls

Being in a room is now being in a call. Mic and camera start off; turn them on from the small call window that floats on top of everything, including the movie in fullscreen. Drag it anywhere, or expand it to see whoever's talking (or everyone, in a grid). Space, arrows and F still control the movie from the call window; M and V toggle your mic and camera.

Use headphones, or your friends will hear the movie twice. This is the first version of calls: shortcuts that work from any app, volume per person and camera/mic pickers are coming.

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
