// The floating call window. It owns the call: camera, mic and every
// connection to the others. While you're in a room it is always open, either
// as a small strip (who's here, mic, camera) or expanded to show video. It
// stays on top of everything, including the movie in fullscreen.
//
// Sharing your screen works like Google Meet: pick your entire screen or one
// window, and macOS's own picker opens to choose which. A friend's shared
// screen takes the big spot (and brings the call up to show it).
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { chooseShare, EMPTY, onCallExpand, onCallShortcut, onSignal, onSnapshot, outputIsSpeakers, send, type Snapshot } from "../api";
import {
  CamIcon,
  CollapseIcon,
  ExpandIcon,
  GearIcon,
  GridIcon,
  HeadphonesIcon,
  MicIcon,
  MutedIcon,
  PinIcon,
  SelfIcon,
  ShareScreenIcon,
  SpeakerIcon,
  StopShareIcon,
  VolumeIcon,
} from "../components/Icons";
import { filmstripSide, gridFit, GAP, pickStage, tileKey, type Box, type TileKind, type TileRef } from "./layout";
import { shareSoundAvailable, startShareSound, type SharedSound } from "./shareSound";
import { initial, Tile, type Person } from "./tiles";
import { TooltipLayer, useTooltip } from "./tooltips";
import { ICE_SERVERS, Mesh, type Kind, type Path, type Remote, type SignalData, type Size } from "./mesh";
import { loadPrefs, savePrefs, type CallPrefs } from "./prefs";
import { micConstraints, processMic, type MicChain, type MicSettings } from "./micChain";
import { isTalking, pickSpeaker, track, type Talk } from "./speaker";
import { PILL_HEIGHT, pillWidth } from "./sizes";
import { onResized, rememberPosition, restorePosition, setWindowSize, startResize } from "./win";

const VIDEO: MediaTrackConstraints = {
  width: { ideal: 1920 },
  height: { ideal: 1080 },
  frameRate: { ideal: 30, max: 30 },
};
/** A shared screen: up to 1080p (sharp text) at up to 30 fps. */
const SCREEN: MediaTrackConstraints = {
  width: { max: 1920 },
  height: { max: 1080 },
  frameRate: { max: 30 },
};
/** Turned down faster than anyone could close a picker: macOS refused. */
const REFUSED_MS = 600;
/** The smallest the call window grows to when a friend starts sharing. */
const SCREEN_VIEW = { width: 640, height: 400 };
/** Asks the speakers question this often while your mic is on. */
const OUTPUT_CHECK_MS = 5000;
/** Choosing where call sound plays needs setSinkId, which WebKit may lack. */
const CAN_PICK_SPEAKER = typeof HTMLMediaElement !== "undefined" && "setSinkId" in HTMLMediaElement.prototype;

type Mode = "pill" | "tiles";
type ShareKind = "screen" | "window";

/** A tile's key: the person's id ("self" for you), ":screen" for a screen. */
const keyOf = (p: Person, kind: TileKind) => tileKey(p.id || "self", kind);

export function CallApp() {
  const [snap, setSnap] = useState<Snapshot>(EMPTY);
  const [prefs, setPrefsState] = useState<CallPrefs>(loadPrefs);
  const [mode, setMode] = useState<Mode>("pill");
  const [remotes, setRemotes] = useState<Remote[]>([]);
  const [mic, setMic] = useState(false);
  const [cam, setCam] = useState(false);
  const [selfStream, setSelfStream] = useState<MediaStream | null>(null);
  const [talk, setTalk] = useState<Map<string, Talk>>(new Map());
  const [speaker, setSpeaker] = useState<string | null>(null);
  const [pinned, setPinned] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [blocked, setBlocked] = useState(false);
  /** Your shared screen (shown to you as a preview), and the share menu. */
  const [shareStream, setShareStream] = useState<MediaStream | null>(null);
  const [shareMenu, setShareMenu] = useState(false);
  const [shareKind, setShareKind] = useState<ShareKind>("screen");
  const sharing = shareStream !== null;
  /** Your Mac's sound, going out with the share. */
  const sharedSound = useRef<SharedSound | null>(null);
  const [soundOn, setSoundOn] = useState(false);
  const [canShareSound, setCanShareSound] = useState(false);
  const canShareSoundRef = useRef(false);
  canShareSoundRef.current = canShareSound;

  const mesh = useRef<Mesh | null>(null);
  if (!mesh.current) {
    mesh.current = new Mesh(
      "",
      (to, data) => void send({ type: "signal", to, data }),
      () => setRemotes(mesh.current!.remotes()),
    );
  }
  const audios = useRef(new Set<HTMLAudioElement>());
  const localMeter = useRef<Meter>(SILENT);
  /** The mic as captured, and what cleans it up before it's sent. */
  const rawMic = useRef<MediaStreamTrack | null>(null);
  const micChain = useRef<MicChain | null>(null);
  const talkRef = useRef(new Map<string, Talk>());
  const inRoom = snap.phase === "room" && snap.my_id !== "";
  const inRoomRef = useRef(inRoom);
  inRoomRef.current = inRoom;
  const modeRef = useRef(mode);
  modeRef.current = mode;
  /** You shrank the call yourself in this room: cameras don't expand it. */
  const shrunk = useRef(false);

  const prefsRef = useRef(prefs);
  prefsRef.current = prefs;
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [onSpeakers, setOnSpeakers] = useState(false);
  const [speakersDismissed, setSpeakersDismissed] = useState(false);

  const setPrefs = useCallback((change: Partial<CallPrefs>) => {
    setPrefsState((p) => {
      const next = { ...p, ...change };
      savePrefs(next);
      return next;
    });
  }, []);

  // ------------------------------------------------------------ engine
  useEffect(() => {
    const m = mesh.current!;
    const un = [
      onSnapshot(setSnap),
      onSignal(({ from, data }) => void m.onSignal(from, data as SignalData)),
      onCallExpand(() => setMode("tiles")),
      rememberPosition(),
    ];
    void restorePosition(pillWidth(0));
    void shareSoundAvailable().then((ok) => setCanShareSound(ok === true));
    return () => {
      m.close();
      sharedSound.current?.stop();
      for (const k of ["audio", "video", "screen"] as Kind[]) m.track(k)?.stop();
      for (const u of un) void u.then((f) => f());
    };
  }, []);

  // ----------------------------------------------------------- devices
  /** Turn the mic or camera on or off. `remember`: a choice, not a cleanup. */
  const setDevice = useCallback(
    async (kind: Kind, on: boolean, remember: boolean) => {
      const m = mesh.current!;
      const old = m.track(kind);
      let track: MediaStreamTrack | null = old;
      if (on && !old) {
        try {
          const p = prefsRef.current;
          const stream = await capture(kind, kind === "audio" ? p.micId : p.camId, p.micInput);
          const captured = (kind === "audio" ? stream.getAudioTracks() : stream.getVideoTracks())[0] ?? null;
          const chain = kind === "audio" && captured ? await processMic(captured, p.micInput) : null;
          track = chain?.track ?? captured;
          if (!inRoomRef.current || m.track(kind)) {
            // Left the room (or turned on twice) while asking.
            chain?.close();
            captured?.stop();
            return;
          }
          if (captured && track) {
            captured.onended = () => void setDevice(kind, false, false); // unplugged
            if (kind === "audio") {
              rawMic.current = captured;
              micChain.current = chain;
              // If processing stops mid-call, send the plain mic until it's back.
              chain?.onRunning((running) => {
                if (micChain.current !== chain || !rawMic.current) return;
                m.setTrack("audio", running ? chain.track : rawMic.current);
              });
            }
            m.setTrack(kind, track);
          }
          setError(null);
        } catch (e) {
          setError(deviceError(kind, e));
          setMode("tiles");
          track = null;
        }
      } else if (!on && old) {
        old.onended = null;
        old.stop();
        if (kind === "audio") {
          micChain.current?.close();
          micChain.current = null;
          if (rawMic.current) rawMic.current.onended = null;
          rawMic.current?.stop();
          rawMic.current = null;
        }
        m.setTrack(kind, null);
        track = null;
      }
      const isOn = track !== null;
      if (kind === "audio") {
        setMic(isOn);
        if (!isOn || localMeter.current === SILENT) {
          localMeter.current.close();
          const chain = micChain.current;
          // With the gate, you're "talking" only when your voice is sent.
          localMeter.current = !isOn || !track ? SILENT : chain ? { level: () => (chain.open() ? chain.level() : 0), close: () => {} } : meter(track);
        }
      } else {
        setCam(isOn);
        setSelfStream(track ? new MediaStream([track]) : null);
      }
      if (remember && isOn === on) setPrefs(kind === "audio" ? { mic: on } : { cam: on });
      return isOn;
    },
    [setPrefs],
  );

  /** Change how the mic is cleaned up (live). */
  const setMicInput = useCallback(
    (micInput: MicSettings) => {
      setPrefs({ micInput });
      prefsRef.current = { ...prefsRef.current, micInput };
      micChain.current?.set(micInput);
      void rawMic.current?.applyConstraints?.(micConstraints(micInput)).catch(() => {});
    },
    [setPrefs],
  );

  /** Use a different camera or mic (switching now if it's on). */
  const chooseDevice = useCallback(
    async (kind: Kind, id: string) => {
      setPrefs(kind === "audio" ? { micId: id } : { camId: id });
      prefsRef.current = { ...prefsRef.current, ...(kind === "audio" ? { micId: id } : { camId: id }) };
      if (mesh.current!.track(kind)) {
        await setDevice(kind, false, false);
        await setDevice(kind, true, false);
      }
    },
    [setDevice, setPrefs],
  );

  /** Stop sending your Mac's sound (the screen goes on). */
  const stopSound = useCallback(() => {
    const s = sharedSound.current;
    if (!s) return;
    sharedSound.current = null;
    s.stop();
    mesh.current!.setTrack("screenAudio", null);
    setSoundOn(false);
  }, []);

  /** Send your Mac's sound with the share (not the movie or the call). */
  const startSound = useCallback(async () => {
    if (sharedSound.current) return;
    const s = await startShareSound();
    if (!s) {
      setError("Couldn't share your Mac's sound. Your screen is still shared.");
      return;
    }
    if (!mesh.current!.track("screen") || sharedSound.current) return s.stop(); // stopped meanwhile
    sharedSound.current = s;
    mesh.current!.setTrack("screenAudio", s.track);
    setSoundOn(true);
  }, []);

  /** Stop sharing your screen (also when macOS's menu bar stops it). */
  const stopShare = useCallback((announce: boolean) => {
    const m = mesh.current!;
    stopSound();
    const t = m.track("screen");
    if (!t) return;
    t.onended = null;
    t.stop();
    m.setTrack("screen", null);
    setShareStream(null);
    if (announce) void send({ type: "notify", text: "Stopped sharing your screen" });
  }, [stopSound]);

  /** Share your entire screen or a window: macOS's picker chooses which. */
  const startShare = useCallback(
    async (kind: ShareKind) => {
      setShareMenu(false);
      const m = mesh.current!;
      let track: MediaStreamTrack | null = null;
      const asked = performance.now();
      try {
        if (!navigator.mediaDevices?.getDisplayMedia) throw new DOMException("no getDisplayMedia", "NotSupportedError");
        // Tells the app which macOS picker the request below should open.
        await chooseShare(kind);
        const stream = await navigator.mediaDevices.getDisplayMedia({ video: SCREEN, audio: false });
        track = stream.getVideoTracks()[0] ?? null;
      } catch (e) {
        // Closing the picker is "not allowed" too: that's no error. But no
        // picker closes that fast: then macOS didn't allow it at all.
        const name = e instanceof DOMException ? e.name : "";
        if (name === "NotAllowedError" && performance.now() - asked < REFUSED_MS) {
          setError("macOS didn't let WatchSync share your screen. Allow it in System Settings → Privacy & Security → Screen & System Audio Recording.");
          setMode("tiles");
        } else if (name !== "NotAllowedError" && name !== "AbortError") {
          setError(name === "NotSupportedError" ? "Screen sharing needs a newer macOS." : "Couldn't share your screen.");
          setMode("tiles");
        }
        return;
      }
      if (!track) return;
      if (!inRoomRef.current) {
        track.stop(); // left the room while picking
        return;
      }
      // Sharp text over smooth motion.
      track.contentHint = "detail";
      const old = m.track("screen");
      if (old) {
        old.onended = null;
        old.stop();
      }
      const shared = track;
      shared.onended = () => {
        if (mesh.current!.track("screen") === shared) stopShare(true);
      };
      m.setTrack("screen", shared);
      setShareStream(new MediaStream([shared]));
      setShareKind(kind);
      setError(null);
      void send({ type: "notify", text: kind === "window" ? "Sharing a window" : "Sharing your screen" });
      if (prefsRef.current.shareSound && canShareSoundRef.current) void startSound();
    },
    [stopShare, startSound],
  );

  /** Turn the share's sound on or off (and remember it for next time). */
  const toggleSound = useCallback(() => {
    const on = !sharedSound.current;
    setPrefs({ shareSound: on });
    if (on) void startSound();
    else stopSound();
  }, [setPrefs, startSound, stopSound]);

  /** The share button: stop if sharing, otherwise ask what to share. */
  const onShareButton = useCallback(() => {
    if (sharing) return stopShare(true);
    setMode("tiles");
    setSettingsOpen(false);
    setShareMenu((open) => !open);
  }, [sharing, stopShare]);

  const toggle = useCallback(
    async (kind: Kind) => {
      const on = !(kind === "audio" ? mic : cam);
      const isOn = await setDevice(kind, on, true);
      if (isOn === undefined || !snap.player_open) return;
      const what = kind === "audio" ? (isOn ? "Mic on" : "Mic muted") : isOn ? "Camera on" : "Camera off";
      void send({ type: "notify", text: what });
    },
    [mic, cam, setDevice, snap.player_open],
  );

  // On speakers with the mic on, friends hear the movie twice: say so.
  useEffect(() => {
    if (!inRoom || !mic) return setOnSpeakers(false);
    let live = true;
    const poll = () =>
      void outputIsSpeakers()
        .then((s) => live && setOnSpeakers(s === true))
        .catch(() => {});
    poll();
    const timer = setInterval(poll, OUTPUT_CHECK_MS);
    return () => {
      live = false;
      clearInterval(timer);
    };
  }, [inRoom, mic]);

  // ⌥⌘M / ⌥⌘V (or whatever's set) from any app.
  const toggleRef = useRef(toggle);
  toggleRef.current = toggle;
  useEffect(() => {
    const un = onCallShortcut((action) => void toggleRef.current(action === "mic" ? "audio" : "video"));
    return () => void un.then((f) => f());
  }, []);

  // Entering a room: connect, with the mic the way you last left it. The
  // camera always starts off: nobody appears on camera without choosing to.
  // Leaving: hang up and let go of the camera and mic.
  useEffect(() => {
    const m = mesh.current!;
    if (inRoom) {
      if (loadPrefs().mic) void setDevice("audio", true, false);
    } else {
      m.close();
      void setDevice("audio", false, false);
      void setDevice("video", false, false);
      stopShare(false);
      setShareMenu(false);
      setMode("pill");
      shrunk.current = false;
      setPinned(null);
      setError(null);
      setBlocked(false);
      setSettingsOpen(false);
      setSpeakersDismissed(false);
    }
  }, [inRoom, setDevice, stopShare]);

  useEffect(() => {
    if (inRoom) void send({ type: "set_call", mic, cam, screen: sharing });
  }, [inRoom, mic, cam, sharing]);

  // Keep a connection to everyone in the room. If they can't reach each
  // other directly, WebRTC relays through this Mac's relay, over iroh.
  const peerIds = snap.peers.map((p) => p.id).join(",");
  const turn = snap.turn;
  const relayOnly = prefs.forceRelay && !!turn;
  useEffect(() => {
    const m = mesh.current!;
    if (!inRoom) return;
    m.setConfig({
      iceServers: turn ? [...ICE_SERVERS, { urls: turn.urls, username: turn.username, credential: turn.credential }] : ICE_SERVERS,
      iceTransportPolicy: relayOnly ? "relay" : "all",
    });
    m.setMyId(snap.my_id);
    m.sync(peerIds.split(",").filter(Boolean));
  }, [inRoom, snap.my_id, peerIds, turn?.urls, turn?.username, turn?.credential, relayOnly]);

  // Which path each friend's media takes (shown when it's the relay).
  const [paths, setPaths] = useState<Map<string, Path | null>>(new Map());
  useEffect(() => {
    if (!inRoom) return setPaths(new Map());
    let live = true;
    const poll = () =>
      void mesh.current!.paths().then((p) => {
        if (live) setPaths(p);
      });
    poll();
    const timer = setInterval(poll, 3000);
    return () => {
      live = false;
      clearInterval(timer);
    };
  }, [inRoom]);

  // ------------------------------------------------------------ people
  const byId = useMemo(() => new Map(remotes.map((r) => [r.id, r])), [remotes]);
  const people: Person[] = useMemo(
    () =>
      snap.peers
        .filter((p) => p.id && p.id !== snap.my_id)
        .map((p) => ({ id: p.id, name: p.name, cam: p.cam, mic: p.mic, screen: !!p.screen, remote: byId.get(p.id) ?? null, iroh: p.iroh ?? null })),
    [snap.peers, snap.my_id, byId],
  );
  const ids = people.map((p) => p.id).join(",");

  // Who's talking, from WebRTC's audio levels (and a meter on our own mic).
  useEffect(() => {
    if (!inRoom) return;
    const timer = setInterval(() => {
      const now = performance.now();
      const levels = new Map<string, number>();
      for (const r of mesh.current!.remotes()) {
        const sources = r.audio?.getSynchronizationSources?.() ?? [];
        levels.set(r.id, sources[0]?.audioLevel ?? 0);
      }
      levels.set("", localMeter.current.level());
      const next = track(talkRef.current, levels, now);
      talkRef.current = next;
      setTalk(next);
      setSpeaker((cur) => pickSpeaker(cur, ids.split(",").filter(Boolean), next, now));
    }, 150);
    return () => clearInterval(timer);
  }, [inRoom, ids]);

  // ------------------------------------------------------------ window
  useEffect(() => {
    if (mode === "tiles") void setWindowSize(prefs.size.width, prefs.size.height);
    else void setWindowSize(pillWidth(people.length) + (onSpeakers && !speakersDismissed ? 34 : 0), PILL_HEIGHT);
    // The saved size only matters when expanding; resizes save it below.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode, people.length, onSpeakers, speakersDismissed]);

  useEffect(() => {
    const un = onResized((width, height) => {
      if (width <= 150 || height <= 90) return;
      // Dragging the strip bigger means "show me the video", at that size.
      if (modeRef.current === "pill") setMode("tiles");
      setPrefs({ size: { width, height } });
    });
    return () => void un.then((f) => f());
  }, [setPrefs]);

  // A friend's camera coming on brings their video up (unless you shrank
  // the call yourself); so does going fullscreen with a camera on.
  const remoteCams = people.filter((p) => p.cam).length;
  // A friend starting to share always brings it up, big enough to read.
  const sharers = people.filter((p) => p.screen).map((p) => p.id);
  const sharerIds = sharers.join(",");
  const before = useRef({ fullscreen: false, cams: 0, sharers: [] as string[] });
  useEffect(() => {
    const was = before.current;
    const fullscreenNow = snap.player_fullscreen && !was.fullscreen && remoteCams > 0;
    const camNow = remoteCams > 0 && was.cams === 0 && (!shrunk.current || snap.player_fullscreen);
    const now = sharerIds.split(",").filter(Boolean);
    const newSharer = now.find((id) => !was.sharers.includes(id));
    if (newSharer) {
      setMode("tiles");
      // Show the new share on the big spot, unless you pinned a screen.
      setPinned((p) => (p?.endsWith(":screen") ? p : null));
      const { width, height } = prefsRef.current.size;
      if (width < SCREEN_VIEW.width || height < SCREEN_VIEW.height) {
        const size = { width: Math.max(width, SCREEN_VIEW.width), height: Math.max(height, SCREEN_VIEW.height) };
        // Saved, so expanding from the strip uses it; set now if already expanded.
        prefsRef.current = { ...prefsRef.current, size };
        setPrefs({ size });
        void setWindowSize(size.width, size.height);
      }
    } else if (fullscreenNow || camNow) setMode("tiles");
    before.current = { fullscreen: snap.player_fullscreen, cams: remoteCams, sharers: now };
  }, [snap.player_fullscreen, remoteCams, sharerIds, setPrefs]);

  // The movie's keys work here too, so clicking this window doesn't take
  // them away from the movie.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        setShareMenu(false);
        setSettingsOpen(false);
        return;
      }
      if (e.metaKey || e.ctrlKey || e.altKey || e.repeat) return;
      const player = snap.player_open;
      if (e.code === "Space" && player) {
        e.preventDefault();
        void send({ type: "toggle_play" });
      } else if (e.key === "ArrowLeft" && player) void send({ type: "seek_by", seconds: -10 });
      else if (e.key === "ArrowRight" && player) void send({ type: "seek_by", seconds: 10 });
      else if (e.code === "KeyF" && player) void send({ type: "toggle_fullscreen" });
      else if (e.code === "KeyM") void toggle("audio");
      else if (e.code === "KeyV") void toggle("video");
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [snap.player_open, toggle]);

  const onBlocked = useCallback(() => setBlocked(true), []);

  // Tell everyone how big we show them, so nobody sends more than is seen
  // (and nothing while they're off screen, like with the call shrunk).
  const shownAt = useRef(new Map<string, Size>());
  const onSize = useCallback((id: string, size: Size) => {
    if (size.w > 0 && size.h > 0) shownAt.current.set(id, size);
    else shownAt.current.delete(id);
    mesh.current!.setWanted(id, size);
  }, []);
  useEffect(() => {
    for (const r of remotes) if (!shownAt.current.has(r.id)) mesh.current!.setWanted(r.id, { w: 0, h: 0 });
  });
  const hearCall = () => {
    // Inside the click, so WebKit counts it as permission to play sound.
    for (const a of audios.current) void a.play().catch(() => {});
    setBlocked(false);
  };

  const now = performance.now();
  const talking = (id: string) => isTalking(talk, id, now);
  // Everyone connected is heard, even someone briefly missing from the room
  // list while their connection to the host blinks.
  const names = new Map(snap.peers.map((p) => [p.id, p.name]));
  const lastNames = useRef(new Map<string, string>());
  for (const [id, n] of names) lastNames.current.set(id, n);
  const volumeOf = (name: string | undefined) => (name !== undefined ? (prefs.volumes[name] ?? 1) : 1);
  const setVolume = (name: string, v: number) => setPrefs({ volumes: { ...prefs.volumes, [name]: v } });
  const shareVolumeOf = (name: string | undefined) => (name !== undefined ? (prefs.shareVolumes[name] ?? 1) : 1);
  const setShareVolume = (name: string, v: number) => setPrefs({ shareVolumes: { ...prefs.shareVolumes, [name]: v } });
  const sound = remotes.map((r) => (
    <span key={r.id} hidden>
      <RemoteAudio
        stream={r.stream}
        version={r.version}
        audios={audios.current}
        onBlocked={onBlocked}
        volume={prefs.voices * volumeOf(lastNames.current.get(r.id))}
        sinkId={prefs.speakerId}
      />
      {/* Their shared screen's sound (silent unless they share it). */}
      <RemoteAudio
        stream={r.screen}
        version={r.version}
        audios={audios.current}
        onBlocked={onBlocked}
        volume={shareVolumeOf(lastNames.current.get(r.id))}
        sinkId={prefs.speakerId}
      />
    </span>
  ));
  const speakersWarning = onSpeakers && !speakersDismissed;
  const tip = useTooltip();
  const [box, setBox] = useState<Box | null>(null);
  const area = useRef<ResizeObserver | null>(null);
  const measure = useCallback((el: HTMLDivElement | null) => {
    area.current?.disconnect();
    area.current = null;
    if (!el || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(([e]) => setBox({ w: e.contentRect.width, h: e.contentRect.height }));
    ro.observe(el);
    area.current = ro;
  }, []);

  if (!inRoom) return <div className="call" />;

  if (mode === "pill") {
    return (
      <div className="call pill" data-tauri-drag-region>
        {sound}
        <div className="pill-people" data-tauri-drag-region>
          {tip && !tip.el.closest(".pill-people") ? (
            // No room above the strip for a button's tooltip: it goes here.
            // (Not an avatar's: that would swap the avatar from under you.)
            <span className="pill-hint" role="tooltip">
              {tip.text}
            </span>
          ) : (
            <>
              {people.length === 0 && (
                <span className="pill-alone" data-tauri-drag-region>
                  Just you
                </span>
              )}
              {people.slice(0, 4).map((p) => (
                <span
                  key={p.id}
                  className={`avatar small ${talking(p.id) ? "talking" : ""} ${p.cam || p.screen ? "has-cam" : ""}`}
                  data-tip={p.screen ? `${p.name} is sharing their screen` : p.cam ? `${p.name}, camera on` : p.name}
                  aria-label={p.name}
                  data-tauri-drag-region
                >
                  {initial(p.name)}
                </span>
              ))}
            </>
          )}
        </div>
        {speakersWarning && (
          <button
            className="call-btn warn"
            onClick={() => setMode("tiles")}
            aria-label="You're on speakers"
            data-tip="On speakers, friends hear the movie twice. Use headphones."
          >
            <HeadphonesIcon />
          </button>
        )}
        {blocked ? (
          <button className="call-btn wide" onClick={hearCall} data-tip="macOS held the call's sound back">
            Click to hear
          </button>
        ) : (
          <>
            <Buttons mic={mic} cam={cam} micTalking={mic && talking("")} onToggle={toggle} />
            <ShareButton sharing={sharing} onClick={onShareButton} />
          </>
        )}
        <button
          className={`call-btn ${remoteCams > 0 || sharers.length > 0 ? "on" : ""}`}
          onClick={() => setMode("tiles")}
          aria-label="Expand the call"
          data-tip={sharers.length > 0 ? "Expand to see the shared screen" : remoteCams > 0 ? "Expand to see video" : "Expand"}
        >
          <ExpandIcon />
        </button>
      </div>
    );
  }

  // ------------------------------------------------------------- tiles
  const self: Person = { id: "", name: snap.name || "You", cam, mic, screen: sharing, remote: null };
  interface Shown {
    ref: TileRef;
    person: Person;
  }
  const tile = (person: Person, kind: TileKind): Shown => ({ ref: { key: keyOf(person, kind), kind, self: person.id === "" }, person });
  // Screens first: that's what people share them for.
  const all: Shown[] = [
    ...people.filter((p) => p.screen).map((p) => tile(p, "screen")),
    ...people.map((p) => tile(p, "cam")),
    ...(sharing ? [tile(self, "screen")] : []),
    tile(self, "cam"),
  ];
  const pin = (key: string) => {
    setPinned(key);
    setPrefs({ layout: "speaker" });
  };
  const togglePin = (key: string) => (pinned === key ? setPinned(null) : pin(key));

  const render = (t: Shown, where: "stage" | "strip" | "grid", style?: React.CSSProperties) => {
    const { person: p, ref } = t;
    const screen = ref.kind === "screen";
    const mine = ref.self;
    return (
      <Tile
        key={ref.key}
        person={p}
        kind={ref.kind}
        stream={screen ? (mine ? shareStream : (p.remote?.screen ?? null)) : mine ? selfStream : (p.remote?.stream ?? null)}
        version={p.remote?.version ?? 0}
        talking={!screen && (mine ? mic && talking("") : talking(p.id))}
        mirrored={mine && !screen}
        relayed={!mine && !screen && paths.get(p.id) === "relay"}
        pinned={pinned === ref.key}
        onPin={() => togglePin(ref.key)}
        onPick={where === "stage" ? undefined : () => pin(ref.key)}
        volume={mine ? undefined : screen ? shareVolumeOf(p.name) : volumeOf(p.name)}
        onVolume={mine ? undefined : screen ? (v) => setShareVolume(p.name, v) : (v) => setVolume(p.name, v)}
        onSize={!mine && !screen ? onSize : undefined}
        style={style}
      />
    );
  };

  const stageKey = prefs.layout === "speaker" ? pickStage(all.map((t) => t.ref), pinned, speaker) : null;
  const stage = stageKey !== null ? all.find((t) => t.ref.key === stageKey) : undefined;
  let body: React.ReactNode;
  if (stage) {
    const hideSelfCam = (t: Shown) => t.ref.self && t.ref.kind === "cam" && prefs.hideSelf;
    const others = all.filter((t) => t !== stage && !hideSelfCam(t));
    const side = filmstripSide(box);
    // No room for a filmstrip: one small tile in the corner, the most
    // useful one. Watching a screen: the face of whoever's talking (or the
    // one sharing). Watching a face: the shared screen. Else yourself.
    const camOn = (t: Shown) => t.ref.kind === "cam" && !t.ref.self && t.person.cam;
    const corner = side
      ? undefined
      : stage.ref.kind === "screen"
        ? (others.find((t) => camOn(t) && t.person.id === speaker) ??
          others.find((t) => camOn(t) && keyOf(t.person, "screen") === stage.ref.key) ??
          others.find(camOn) ??
          others.find((t) => t.ref.self && t.ref.kind === "cam" && cam))
        : (others.find((t) => t.ref.kind === "screen" && !t.ref.self) ?? others.find((t) => t.ref.self && t.ref.kind === "cam" && cam));
    body = (
      <div className={`spotlight ${side ? `strip-${side}` : ""}`} data-tauri-drag-region>
        <div className="stage" data-tauri-drag-region>
          {render(stage, "stage")}
        </div>
        {side && others.length > 0 && (
          <div className="filmstrip" data-tauri-drag-region>
            {others.map((t) => render(t, "strip"))}
          </div>
        )}
        {corner && <div className="corner">{render(corner, "strip")}</div>}
      </div>
    );
  } else {
    const fit = gridFit(all.length, box);
    const cols = fit?.cols ?? Math.ceil(Math.sqrt(all.length));
    const rows = Math.ceil(all.length / cols);
    const size: React.CSSProperties = fit
      ? { width: fit.w, height: fit.h }
      : { width: `calc((100% - ${GAP * (cols - 1)}px) / ${cols})`, height: `calc((100% - ${GAP * (rows - 1)}px) / ${rows})` };
    body = (
      <div className="grid" data-tauri-drag-region>
        {all.map((t) => render(t, "grid", size))}
        {people.length === 0 && (
          <p className="call-note waiting" data-tauri-drag-region>
            Waiting for friends to join
          </p>
        )}
      </div>
    );
  }

  return (
    <div className="call tiles" data-tauri-drag-region>
      {sound}
      <div className="call-area" ref={measure} data-tauri-drag-region>
        {body}
      </div>

      {sharing && !error && !speakersWarning && (
        <div className="share-chip" role="status">
          <span className="rec-dot" aria-hidden="true" />
          <span className="share-chip-text">{shareKind === "window" ? "You're sharing a window" : "You're sharing your screen"}</span>
          {canShareSound && (
            <button
              className={`chip-btn ${soundOn ? "on" : ""}`}
              onClick={toggleSound}
              aria-label={soundOn ? "Stop sharing sound" : "Share sound"}
              aria-pressed={soundOn}
              data-tip={soundOn ? "Friends hear your Mac's sound (not the movie or the call). Click to stop." : "Let friends hear your Mac's sound too"}
            >
              {soundOn ? <VolumeIcon /> : <MutedIcon />}
            </button>
          )}
          <button className="chip-stop" onClick={() => stopShare(true)} data-tip="Friends stop seeing your screen">
            Stop sharing
          </button>
        </div>
      )}
      {shareMenu && !sharing && (
        <div className="share-menu" role="menu" aria-label="Share">
          <p className="share-menu-title">Share with everyone in the call</p>
          <button role="menuitem" className="share-option" onClick={() => void startShare("screen")}>
            <span>Your entire screen</span>
            <small>Everything on a display you choose</small>
          </button>
          <button role="menuitem" className="share-option" onClick={() => void startShare("window")}>
            <span>A window</span>
            <small>Just one window. Nothing else shows</small>
          </button>
          {canShareSound && (
            <label className="share-sound">
              <input type="checkbox" checked={prefs.shareSound} onChange={(e) => setPrefs({ shareSound: e.target.checked })} />
              <span>
                Share sound from this Mac
                <small>Not the movie or the call</small>
              </span>
            </label>
          )}
          <button className="share-menu-cancel" onClick={() => setShareMenu(false)}>
            Cancel
          </button>
        </div>
      )}
      {error && (
        <div className="call-error" role="alert">
          <span>{error}</span>
          <button className="call-btn" onClick={() => setError(null)} aria-label="Dismiss" data-tip="Dismiss">
            ×
          </button>
        </div>
      )}
      {speakersWarning && !error && (
        <div className="call-error warn" role="status">
          <HeadphonesIcon />
          <span>You're on speakers, so friends hear the movie through your mic. Use headphones.</span>
          <button className="call-btn" onClick={() => setSpeakersDismissed(true)} aria-label="Dismiss" data-tip="Dismiss">
            ×
          </button>
        </div>
      )}
      {settingsOpen && (
        <CallSettings
          prefs={prefs}
          onVoices={(voices) => setPrefs({ voices })}
          onDevice={(kind, id) => void chooseDevice(kind, id)}
          onSpeaker={(speakerId) => setPrefs({ speakerId })}
          onForceRelay={(forceRelay) => setPrefs({ forceRelay })}
          onMicInput={setMicInput}
          micMeter={() => {
            const c = micChain.current;
            if (!mic) return null;
            if (!c || !c.running()) return "unprocessed";
            return { level: c.level(), open: c.open(), thresholdDb: c.thresholdDb() };
          }}
          paths={people.map((p) => ({ name: p.name, path: paths.get(p.id) ?? null, iroh: p.iroh ?? null }))}
          relayAvailable={!!turn}
          onClose={() => setSettingsOpen(false)}
        />
      )}
      {blocked && (
        <button className="call-hear" onClick={hearCall}>
          Click to hear the call
        </button>
      )}

      <div className={`call-controls ${shareMenu || settingsOpen ? "pinned-open" : ""}`}>
        <Buttons mic={mic} cam={cam} micTalking={mic && talking("")} onToggle={toggle} />
        <ShareButton sharing={sharing} open={shareMenu} onClick={onShareButton} />
        <span className="call-controls-gap" aria-hidden="true" />
        {prefs.layout === "speaker" && pinned && (
          <button className="call-btn on" onClick={() => setPinned(null)} aria-label="Unpin" data-tip="Unpin: the big spot follows the call again">
            <PinIcon />
          </button>
        )}
        <button
          className="call-btn"
          onClick={() => setPrefs({ layout: prefs.layout === "grid" ? "speaker" : "grid" })}
          aria-label={prefs.layout === "grid" ? "Speaker view" : "Grid view"}
          data-tip={prefs.layout === "grid" ? "Spotlight: one big, the rest small" : "Grid: everyone the same size"}
        >
          {prefs.layout === "grid" ? <SpeakerIcon /> : <GridIcon />}
        </button>
        {prefs.layout === "speaker" && (
          <button
            className={`call-btn ${prefs.hideSelf ? "" : "on"}`}
            onClick={() => setPrefs({ hideSelf: !prefs.hideSelf })}
            aria-label="Show yourself"
            aria-pressed={!prefs.hideSelf}
            data-tip={prefs.hideSelf ? "Show yourself" : "Hide yourself (friends still see you)"}
          >
            <SelfIcon />
          </button>
        )}
        <button
          className={`call-btn ${settingsOpen ? "on" : ""}`}
          onClick={() => {
            setShareMenu(false);
            setSettingsOpen((o) => !o);
          }}
          aria-label="Call settings"
          aria-pressed={settingsOpen}
          data-tip="Settings: camera, mic and sound"
        >
          <GearIcon />
        </button>
        <button
          className="call-btn"
          onClick={() => {
            shrunk.current = true;
            setMode("pill");
          }}
          aria-label="Shrink the call"
          data-tip="Shrink to the strip"
        >
          <CollapseIcon />
        </button>
      </div>
      <div className="resize-grip" onMouseDown={startResize} aria-hidden="true" />
      <TooltipLayer tip={tip} />
    </div>
  );
}

function Buttons({
  mic,
  cam,
  micTalking,
  onToggle,
}: {
  mic: boolean;
  cam: boolean;
  micTalking: boolean;
  onToggle: (k: Kind) => void;
}) {
  return (
    <>
      <button
        className={`call-btn ${mic ? "on" : "off"} ${micTalking ? "talking" : ""}`}
        onClick={() => onToggle("audio")}
        aria-label={mic ? "Mute your mic" : "Turn your mic on"}
        aria-pressed={mic}
        data-tip={mic ? "Mute (M)" : "Unmute (M)"}
      >
        <MicIcon off={!mic} />
      </button>
      <button
        className={`call-btn ${cam ? "on" : "off"}`}
        onClick={() => onToggle("video")}
        aria-label={cam ? "Turn your camera off" : "Turn your camera on"}
        aria-pressed={cam}
        data-tip={cam ? "Turn camera off (V)" : "Turn camera on (V)"}
      >
        <CamIcon off={!cam} />
      </button>
    </>
  );
}

function ShareButton({ sharing, open, onClick }: { sharing: boolean; open?: boolean; onClick: () => void }) {
  return (
    <button
      className={`call-btn ${sharing ? "sharing" : open ? "on" : ""}`}
      onClick={onClick}
      aria-label={sharing ? "Stop sharing your screen" : "Share your screen"}
      aria-pressed={sharing}
      aria-expanded={sharing ? undefined : !!open}
      data-tip={sharing ? "Stop sharing" : "Share your screen or a window"}
    >
      {sharing ? <StopShareIcon /> : <ShareScreenIcon />}
    </button>
  );
}

/** Plays one person's voice, or their shared screen's sound. */
function RemoteAudio({
  stream,
  version,
  audios,
  onBlocked,
  volume,
  sinkId,
}: {
  stream: MediaStream;
  version: number;
  audios: Set<HTMLAudioElement>;
  onBlocked: () => void;
  volume: number;
  sinkId: string;
}) {
  const el = useRef<HTMLAudioElement>(null);
  useEffect(() => {
    if (el.current) el.current.volume = Math.min(1, Math.max(0, volume));
  }, [volume]);
  useEffect(() => {
    const a = el.current as (HTMLAudioElement & { setSinkId?: (id: string) => Promise<void> }) | null;
    if (CAN_PICK_SPEAKER && a?.setSinkId) void a.setSinkId(sinkId).catch(() => {});
  }, [sinkId]);
  useEffect(() => {
    const a = el.current;
    if (!a) return;
    audios.add(a);
    if (a.srcObject !== stream) a.srcObject = stream;
    a.play().catch((e: unknown) => {
      if (e instanceof DOMException && e.name === "NotAllowedError") onBlocked();
    });
    return () => void audios.delete(a);
  }, [stream, version, audios, onBlocked]);
  return <audio ref={el} autoPlay />;
}

interface Meter {
  level: () => number;
  close: () => void;
}
const SILENT: Meter = { level: () => 0, close: () => {} };

/** A live level meter on our own mic (0–1, like WebRTC's audioLevel). */
function meter(track: MediaStreamTrack): Meter {
  try {
    const ctx = new AudioContext();
    const analyser = ctx.createAnalyser();
    analyser.fftSize = 512;
    ctx.createMediaStreamSource(new MediaStream([track])).connect(analyser);
    const buf = new Float32Array(analyser.fftSize);
    return {
      level: () => {
        if (track.readyState === "ended") return 0;
        if (ctx.state === "suspended") void ctx.resume();
        analyser.getFloatTimeDomainData(buf);
        let sum = 0;
        for (const v of buf) sum += v * v;
        return Math.sqrt(sum / buf.length);
      },
      close: () => void ctx.close().catch(() => {}),
    };
  } catch {
    return SILENT;
  }
}

function deviceError(kind: Kind, e: unknown): string {
  const what = kind === "audio" ? "microphone" : "camera";
  const name = e instanceof DOMException ? e.name : "";
  if (name === "NotAllowedError" || name === "SecurityError")
    return `WatchSync can't use your ${what}. Allow it in System Settings → Privacy & Security → ${kind === "audio" ? "Microphone" : "Camera"}.`;
  if (name === "NotFoundError" || name === "OverconstrainedError") return `No ${what} found.`;
  if (name === "NotReadableError") return `Your ${what} is busy in another app.`;
  return `Couldn't turn on your ${what}.`;
}

/** Camera or mic, the chosen one if it's still there, else the default. */
async function capture(kind: Kind, deviceId: string, mic: MicSettings): Promise<MediaStream> {
  const base = kind === "audio" ? micConstraints(mic) : VIDEO;
  const request = (c: MediaTrackConstraints) => navigator.mediaDevices.getUserMedia(kind === "audio" ? { audio: c } : { video: c });
  if (!deviceId) return request(base);
  try {
    return await request({ ...base, deviceId: { exact: deviceId } });
  } catch (e) {
    const gone = e instanceof DOMException && (e.name === "OverconstrainedError" || e.name === "NotFoundError");
    if (!gone) throw e;
    return request(base); // unplugged: use the default
  }
}

/** Camera, mic (and speaker, where WebKit allows) and how loud voices are. */
function CallSettings({
  prefs,
  onVoices,
  onDevice,
  onSpeaker,
  onForceRelay,
  onMicInput,
  micMeter,
  paths,
  relayAvailable,
  onClose,
}: {
  prefs: CallPrefs;
  onVoices: (v: number) => void;
  onDevice: (kind: Kind, id: string) => void;
  onSpeaker: (id: string) => void;
  onForceRelay: (on: boolean) => void;
  onMicInput: (s: MicSettings) => void;
  /** Live input level and gate state; "unprocessed" if the mic is sent as
   * it is; null with the mic off. */
  micMeter: () => MeterReading;
  paths: { name: string; path: Path | null; iroh: "direct" | "relayed" | null }[];
  relayAvailable: boolean;
  onClose: () => void;
}) {
  const [devices, setDevices] = useState<MediaDeviceInfo[]>([]);
  useEffect(() => {
    const md = navigator.mediaDevices;
    if (!md?.enumerateDevices) return;
    const load = () => void md.enumerateDevices().then(setDevices).catch(() => {});
    load();
    md.addEventListener?.("devicechange", load);
    return () => md.removeEventListener?.("devicechange", load);
  }, []);
  const of = (kind: MediaDeviceKind) => devices.filter((d) => d.kind === kind && d.deviceId && d.deviceId !== "default");
  const select = (label: string, kind: MediaDeviceKind, value: string, onChange: (id: string) => void) => (
    <label className="call-setting">
      <span>{label}</span>
      <select value={value} onChange={(e) => onChange(e.target.value)} aria-label={label}>
        <option value="">System default</option>
        {of(kind).map((d, i) => (
          <option key={d.deviceId} value={d.deviceId}>
            {d.label || `${label} ${i + 1}`}
          </option>
        ))}
      </select>
    </label>
  );
  return (
    <div className="call-settings" role="dialog" aria-label="Call settings">
      {select("Camera", "videoinput", prefs.camId, (id) => onDevice("video", id))}
      {select("Microphone", "audioinput", prefs.micId, (id) => onDevice("audio", id))}
      <MicInput settings={prefs.micInput} onChange={onMicInput} meter={micMeter} />
      {CAN_PICK_SPEAKER && select("Speaker", "audiooutput", prefs.speakerId, onSpeaker)}
      <label className="call-setting">
        <span>Voices</span>
        <input
          type="range"
          min={0}
          max={1}
          step={0.05}
          value={prefs.voices}
          onChange={(e) => onVoices(Number(e.target.value))}
          aria-label="Voices volume"
        />
      </label>
      <details className="call-trouble">
        <summary>Troubleshooting</summary>
        <label className="call-check">
          <input
            type="checkbox"
            checked={prefs.forceRelay}
            disabled={!relayAvailable}
            onChange={(e) => onForceRelay(e.target.checked)}
          />
          Always relay the call through iroh
        </label>
        <p className="call-hint">
          Normally used only when your Macs can't connect directly. Turn on to test it, or if the call cuts out.
        </p>
        {paths.map((p) => (
          <p key={p.name} className="call-path">
            {p.name}:{" "}
            {p.path === "relay"
              ? p.iroh === "direct"
                ? "through iroh, which is direct (Mac to Mac)"
                : p.iroh === "relayed"
                  ? "through iroh's relay servers (no direct link)"
                  : "relayed through iroh"
              : p.path === "direct"
                ? "direct"
                : "connecting…"}
          </p>
        ))}
      </details>
      <button className="call-settings-done" onClick={onClose}>
        Done
      </button>
    </div>
  );
}

type MeterReading = { level: number; open: boolean; thresholdDb: number } | "unprocessed" | null;

const dbOf = (level: number) => 20 * Math.log10(Math.max(level, 1e-6));
/** Where a level sits on the meter, 0–100 (−70 dB to 0 dB). */
const meterPct = (db: number) => Math.min(100, Math.max(0, ((db + 70) / 70) * 100));

/** Noise suppression, sensitivity, input volume, and a live meter. */
function MicInput({
  settings: s,
  onChange,
  meter,
}: {
  settings: MicSettings;
  onChange: (s: MicSettings) => void;
  meter: () => MeterReading;
}) {
  const [reading, setLive] = useState(meter);
  const live = typeof reading === "object" ? reading : null;
  const read = useRef(meter);
  read.current = meter;
  useEffect(() => {
    const timer = setInterval(() => setLive(read.current()), 100);
    return () => clearInterval(timer);
  }, []);
  const set = (change: Partial<MicSettings>) => onChange({ ...s, ...change });
  const threshold = s.auto ? (live?.thresholdDb ?? null) : s.thresholdDb;

  return (
    <div className="mic-input">
      <div className="mic-meter" aria-hidden="true">
        <i className={live?.open ? "open" : ""} style={{ width: `${live ? meterPct(dbOf(live.level)) : 0}%` }} />
        {s.suppress && threshold !== null && <b style={{ left: `${meterPct(threshold)}%` }} />}
      </div>
      <p className="call-hint" aria-live="polite">
        {reading === "unprocessed"
          ? "Mic processing isn't available right now: sending your mic as it is."
          : !live
            ? "Turn your mic on to test it."
            : live.open
              ? "Sending your voice"
              : "Quiet: nothing sent"}
      </p>
      <label className="call-check">
        <input type="checkbox" checked={s.suppress} onChange={(e) => set({ suppress: e.target.checked })} />
        Noise suppression
      </label>
      <p className="call-hint">Keeps typing, clicks and background noise out. Your voice opens it.</p>
      {s.suppress && (
        <>
          <label className="call-check">
            <input type="checkbox" checked={s.auto} onChange={(e) => set({ auto: e.target.checked })} />
            Set sensitivity automatically
          </label>
          {!s.auto && (
            <label className="call-setting">
              <span>Sensitivity</span>
              <input
                type="range"
                min={-80}
                max={-15}
                step={1}
                value={-s.thresholdDb - 95}
                onChange={(e) => set({ thresholdDb: -95 - Number(e.target.value) })}
                aria-label="Mic sensitivity"
                data-tip="More sensitive picks up quieter speech, and more noise"
              />
            </label>
          )}
        </>
      )}
      <label className="call-setting">
        <span>Input volume</span>
        <input
          type="range"
          min={0}
          max={2}
          step={0.05}
          value={s.volume}
          onChange={(e) => set({ volume: Number(e.target.value) })}
          aria-label="Mic input volume"
        />
      </label>
      <label className="call-check">
        <input type="checkbox" checked={s.autoGain} onChange={(e) => set({ autoGain: e.target.checked })} />
        Adjust mic volume automatically
      </label>
    </div>
  );
}
