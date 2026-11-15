// The floating call window. It owns the call: camera, mic and every
// connection to the others. While you're in a room it is always open, either
// as a small strip (who's here, mic, camera) or expanded to show video. It
// stays on top of everything, including the movie in fullscreen.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { EMPTY, onCallExpand, onCallShortcut, onSignal, onSnapshot, outputIsSpeakers, send, type Snapshot } from "../api";
import {
  CamIcon,
  CollapseIcon,
  ExpandIcon,
  GearIcon,
  GridIcon,
  HeadphonesIcon,
  MicIcon,
  PinIcon,
  SelfIcon,
  SpeakerIcon,
  VolumeIcon,
} from "../components/Icons";
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
/** Asks the speakers question this often while your mic is on. */
const OUTPUT_CHECK_MS = 5000;
/** Choosing where call sound plays needs setSinkId, which WebKit may lack. */
const CAN_PICK_SPEAKER = typeof HTMLMediaElement !== "undefined" && "setSinkId" in HTMLMediaElement.prototype;

type Mode = "pill" | "tiles";

interface Person {
  id: string;
  name: string;
  cam: boolean;
  mic: boolean;
  remote: Remote | null;
}

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
    return () => {
      m.close();
      for (const k of ["audio", "video"] as Kind[]) m.track(k)?.stop();
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

  // Entering a room: connect, and start the way you last left things.
  // Leaving: hang up and let go of the camera and mic.
  useEffect(() => {
    const m = mesh.current!;
    if (inRoom) {
      const p = loadPrefs();
      if (p.mic) void setDevice("audio", true, false);
      if (p.cam) void setDevice("video", true, false);
    } else {
      m.close();
      void setDevice("audio", false, false);
      void setDevice("video", false, false);
      setMode("pill");
      shrunk.current = false;
      setPinned(null);
      setError(null);
      setBlocked(false);
      setSettingsOpen(false);
      setSpeakersDismissed(false);
    }
  }, [inRoom, setDevice]);

  useEffect(() => {
    if (inRoom) void send({ type: "set_call", mic, cam });
  }, [inRoom, mic, cam]);

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
        .map((p) => ({ id: p.id, name: p.name, cam: p.cam, mic: p.mic, remote: byId.get(p.id) ?? null })),
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
  const before = useRef({ fullscreen: false, cams: 0 });
  useEffect(() => {
    const was = before.current;
    const fullscreenNow = snap.player_fullscreen && !was.fullscreen && remoteCams > 0;
    const camNow = remoteCams > 0 && was.cams === 0 && (!shrunk.current || snap.player_fullscreen);
    if (fullscreenNow || camNow) setMode("tiles");
    before.current = { fullscreen: snap.player_fullscreen, cams: remoteCams };
  }, [snap.player_fullscreen, remoteCams]);

  // The movie's keys work here too, so clicking this window doesn't take
  // them away from the movie.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
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
  const sound = remotes.map((r) => (
    <RemoteAudio
      key={r.id}
      remote={r}
      audios={audios.current}
      onBlocked={onBlocked}
      volume={prefs.voices * volumeOf(lastNames.current.get(r.id))}
      sinkId={prefs.speakerId}
    />
  ));
  const speakersWarning = onSpeakers && !speakersDismissed;

  if (!inRoom) return <div className="call" />;

  if (mode === "pill") {
    return (
      <div className="call pill" data-tauri-drag-region>
        {sound}
        <div className="pill-people" data-tauri-drag-region>
          {people.length === 0 && (
            <span className="pill-alone" data-tauri-drag-region>
              Just you
            </span>
          )}
          {people.slice(0, 4).map((p) => (
            <span
              key={p.id}
              className={`avatar small ${talking(p.id) ? "talking" : ""} ${p.cam ? "has-cam" : ""}`}
              title={p.cam ? `${p.name} (camera on)` : p.name}
              data-tauri-drag-region
            >
              {initial(p.name)}
            </span>
          ))}
        </div>
        {speakersWarning && (
          <button
            className="call-btn warn"
            onClick={() => setMode("tiles")}
            aria-label="You're on speakers"
            title="You're on speakers: friends hear the movie through your mic. Use headphones."
          >
            <HeadphonesIcon />
          </button>
        )}
        {blocked ? (
          <button className="call-btn wide" onClick={hearCall}>
            Click to hear
          </button>
        ) : (
          <Buttons mic={mic} cam={cam} micTalking={mic && talking("")} onToggle={toggle} />
        )}
        <button
          className={`call-btn ${remoteCams > 0 ? "on" : ""}`}
          onClick={() => setMode("tiles")}
          aria-label="Expand the call"
          title={remoteCams > 0 ? "Expand to see video" : "Expand"}
        >
          <ExpandIcon />
        </button>
      </div>
    );
  }

  const shown = (pinned && people.find((p) => p.id === pinned)) || people.find((p) => p.id === speaker) || people[0];
  const self: Person = { id: "", name: snap.name || "You", cam, mic, remote: null };

  return (
    <div className="call tiles" data-tauri-drag-region>
      {sound}
      {prefs.layout === "grid" || !shown ? (
        <div className="grid" style={{ ["--cols" as string]: Math.ceil(Math.sqrt(people.length + 1)) }} data-tauri-drag-region>
          {people.map((p) => (
            <Tile
              key={p.id}
              person={p}
              talking={talking(p.id)}
              onSize={onSize}
              relayed={paths.get(p.id) === "relay"}
              volume={volumeOf(p.name)}
              onVolume={(v) => setVolume(p.name, v)}
              onClick={() => (setPinned(p.id), setPrefs({ layout: "speaker" }))}
            />
          ))}
          <Tile person={self} stream={selfStream} talking={mic && talking("")} mirrored />
          {people.length === 0 && (
            <p className="call-note" data-tauri-drag-region>
              Waiting for friends to join
            </p>
          )}
        </div>
      ) : (
        <div className="speaker" data-tauri-drag-region>
          <Tile
            key={shown.id}
            person={shown}
            talking={talking(shown.id)}
            onSize={onSize}
            relayed={paths.get(shown.id) === "relay"}
            volume={volumeOf(shown.name)}
            onVolume={(v) => setVolume(shown.name, v)}
          />
          {prefs.selfView && cam && (
            <div className="self-corner">
              <Tile person={self} stream={selfStream} talking={false} mirrored />
            </div>
          )}
        </div>
      )}

      {error && (
        <div className="call-error" role="alert">
          <span>{error}</span>
          <button className="call-btn" onClick={() => setError(null)} aria-label="Dismiss">
            ×
          </button>
        </div>
      )}
      {speakersWarning && !error && (
        <div className="call-error warn" role="status">
          <HeadphonesIcon />
          <span>You're on speakers, so friends hear the movie through your mic. Use headphones.</span>
          <button className="call-btn" onClick={() => setSpeakersDismissed(true)} aria-label="Dismiss">
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
            return mic && c ? { level: c.level(), open: c.open(), thresholdDb: c.thresholdDb() } : null;
          }}
          paths={people.map((p) => ({ name: p.name, path: paths.get(p.id) ?? null }))}
          relayAvailable={!!turn}
          onClose={() => setSettingsOpen(false)}
        />
      )}
      {blocked && (
        <button className="call-hear" onClick={hearCall}>
          Click to hear the call
        </button>
      )}

      <div className="call-controls">
        <Buttons mic={mic} cam={cam} micTalking={mic && talking("")} onToggle={toggle} />
        {prefs.layout === "speaker" && pinned && (
          <button className="call-btn on" onClick={() => setPinned(null)} aria-label="Unpin" title="Unpin: follow whoever's talking">
            <PinIcon />
          </button>
        )}
        <button
          className="call-btn"
          onClick={() => setPrefs({ layout: prefs.layout === "grid" ? "speaker" : "grid" })}
          aria-label={prefs.layout === "grid" ? "Speaker view" : "Grid view"}
          title={prefs.layout === "grid" ? "Show whoever's talking" : "Show everyone"}
        >
          {prefs.layout === "grid" ? <SpeakerIcon /> : <GridIcon />}
        </button>
        {prefs.layout === "speaker" && (
          <button
            className={`call-btn ${prefs.selfView ? "on" : ""}`}
            onClick={() => setPrefs({ selfView: !prefs.selfView })}
            aria-label="Show yourself"
            aria-pressed={prefs.selfView}
            title="Show your own camera in the corner"
          >
            <SelfIcon />
          </button>
        )}
        <button
          className={`call-btn ${settingsOpen ? "on" : ""}`}
          onClick={() => setSettingsOpen((o) => !o)}
          aria-label="Call settings"
          aria-pressed={settingsOpen}
          title="Camera, mic and volume"
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
          title="Shrink"
        >
          <CollapseIcon />
        </button>
      </div>
      <div className="resize-grip" onMouseDown={startResize} aria-hidden="true" />
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
        title={mic ? "Mute (M)" : "Unmute (M)"}
      >
        <MicIcon off={!mic} />
      </button>
      <button
        className={`call-btn ${cam ? "on" : "off"}`}
        onClick={() => onToggle("video")}
        aria-label={cam ? "Turn your camera off" : "Turn your camera on"}
        aria-pressed={cam}
        title={cam ? "Camera off (V)" : "Camera on (V)"}
      >
        <CamIcon off={!cam} />
      </button>
    </>
  );
}

function Tile({
  person,
  stream,
  talking,
  mirrored,
  onClick,
  onSize,
  volume,
  onVolume,
  relayed,
}: {
  person: Person;
  /** Their media comes through the iroh relay. */
  relayed?: boolean;
  stream?: MediaStream | null;
  talking: boolean;
  mirrored?: boolean;
  onClick?: () => void;
  /** Their own volume (0–1), for friends' tiles. */
  volume?: number;
  onVolume?: (v: number) => void;
  /** How big this person is shown, in device pixels (0×0 once gone). */
  onSize?: (id: string, size: Size) => void;
}) {
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = box.current;
    if (!onSize || !el || typeof ResizeObserver === "undefined") return;
    const id = person.id;
    const ro = new ResizeObserver(([e]) => {
      const dpr = window.devicePixelRatio || 1;
      onSize(id, { w: e.contentRect.width * dpr, h: e.contentRect.height * dpr });
    });
    ro.observe(el);
    return () => {
      ro.disconnect();
      onSize(id, { w: 0, h: 0 });
    };
  }, [onSize, person.id]);
  const media = stream ?? person.remote?.stream ?? null;
  const version = person.remote?.version ?? 0;
  const hasVideo = person.cam && !!media && media.getVideoTracks().length > 0;
  const video = useRef<HTMLVideoElement>(null);
  useEffect(() => {
    const v = video.current;
    if (!v) return;
    if (v.srcObject !== media) v.srcObject = media;
    void v.play().catch(() => {});
  }, [media, version, hasVideo]);

  return (
    <div ref={box} className={`tile ${talking ? "talking" : ""} ${onClick ? "pickable" : ""}`} onClick={onClick} data-tauri-drag-region={onClick ? undefined : true}>
      {hasVideo ? (
        <video ref={video} className={mirrored ? "mirrored" : ""} autoPlay playsInline muted data-tauri-drag-region={onClick ? undefined : true} />
      ) : (
        <span className={`avatar ${talking ? "talking" : ""}`} data-tauri-drag-region={onClick ? undefined : true}>
          {initial(person.name)}
        </span>
      )}
      <span className="tile-name">
        {person.name}
        {!person.mic && <MicIcon off />}
        {relayed && (
          <span className="tile-path" title="No direct connection: relayed through iroh">
            via iroh
          </span>
        )}
      </span>
      {onVolume && (
        <label className="tile-volume" onClick={(e) => e.stopPropagation()} title={`${person.name}'s volume`}>
          <VolumeIcon />
          <input
            type="range"
            min={0}
            max={1}
            step={0.05}
            value={volume ?? 1}
            onChange={(e) => onVolume(Number(e.target.value))}
            aria-label={`${person.name}'s volume`}
          />
        </label>
      )}
    </div>
  );
}

/** Plays one person's voice. */
function RemoteAudio({
  remote,
  audios,
  onBlocked,
  volume,
  sinkId,
}: {
  remote: Remote;
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
    if (a.srcObject !== remote.stream) a.srcObject = remote.stream;
    a.play().catch((e: unknown) => {
      if (e instanceof DOMException && e.name === "NotAllowedError") onBlocked();
    });
    return () => void audios.delete(a);
  }, [remote.stream, remote.version, audios, onBlocked]);
  return <audio ref={el} autoPlay />;
}

function initial(name: string): string {
  return [...name.trim()][0]?.toUpperCase() ?? "?";
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
  /** Live input level and gate state, while the mic is on and processed. */
  micMeter: () => { level: number; open: boolean; thresholdDb: number } | null;
  paths: { name: string; path: Path | null }[];
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
            {p.name}: {p.path === "relay" ? "relayed through iroh" : p.path === "direct" ? "direct" : "connecting…"}
          </p>
        ))}
      </details>
      <button className="call-settings-done" onClick={onClose}>
        Done
      </button>
    </div>
  );
}

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
  meter: () => { level: number; open: boolean; thresholdDb: number } | null;
}) {
  const [live, setLive] = useState(meter);
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
        {!live ? "Turn your mic on to test it." : live.open ? "Sending your voice" : "Quiet: nothing sent"}
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
                title="More sensitive picks up quieter speech, and more noise"
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
