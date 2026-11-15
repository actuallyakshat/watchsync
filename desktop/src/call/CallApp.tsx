// The floating call window. It owns the call: camera, mic and every
// connection to the others. While you're in a room it is always open, either
// as a small strip (who's here, mic, camera) or expanded to show video. It
// stays on top of everything, including the movie in fullscreen.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { EMPTY, onCallExpand, onSignal, onSnapshot, send, type Snapshot } from "../api";
import { CamIcon, CollapseIcon, ExpandIcon, GridIcon, MicIcon, PinIcon, SelfIcon, SpeakerIcon } from "../components/Icons";
import { Mesh, type Kind, type Remote, type SignalData } from "./mesh";
import { loadPrefs, savePrefs, type CallPrefs } from "./prefs";
import { isTalking, pickSpeaker, track, type Talk } from "./speaker";
import { PILL_HEIGHT, pillWidth } from "./sizes";
import { onResized, setWindowSize, startResize } from "./win";

const AUDIO: MediaTrackConstraints = { echoCancellation: true, noiseSuppression: true, autoGainControl: true };
const VIDEO: MediaTrackConstraints = {
  width: { ideal: 1920 },
  height: { ideal: 1080 },
  frameRate: { ideal: 30, max: 30 },
};

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
  const talkRef = useRef(new Map<string, Talk>());
  const inRoom = snap.phase === "room" && snap.my_id !== "";
  const inRoomRef = useRef(inRoom);
  inRoomRef.current = inRoom;
  const modeRef = useRef(mode);
  modeRef.current = mode;
  /** You shrank the call yourself in this room: cameras don't expand it. */
  const shrunk = useRef(false);

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
    ];
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
          const stream = await navigator.mediaDevices.getUserMedia(kind === "audio" ? { audio: AUDIO } : { video: VIDEO });
          track = (kind === "audio" ? stream.getAudioTracks() : stream.getVideoTracks())[0] ?? null;
          if (!inRoomRef.current || m.track(kind)) {
            // Left the room (or turned on twice) while asking.
            track?.stop();
            return;
          }
          if (track) {
            track.onended = () => void setDevice(kind, false, false); // unplugged
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
        m.setTrack(kind, null);
        track = null;
      }
      const isOn = track !== null;
      if (kind === "audio") {
        setMic(isOn);
        if (!isOn || localMeter.current === SILENT) {
          localMeter.current.close();
          localMeter.current = isOn && track ? meter(track) : SILENT;
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
    }
  }, [inRoom, setDevice]);

  useEffect(() => {
    if (inRoom) void send({ type: "set_call", mic, cam });
  }, [inRoom, mic, cam]);

  // Keep a connection to everyone in the room.
  const peerIds = snap.peers.map((p) => p.id).join(",");
  useEffect(() => {
    const m = mesh.current!;
    if (!inRoom) return;
    m.setMyId(snap.my_id);
    m.sync(peerIds.split(",").filter(Boolean));
  }, [inRoom, snap.my_id, peerIds]);

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
    else void setWindowSize(pillWidth(people.length), PILL_HEIGHT);
    // The saved size only matters when expanding; resizes save it below.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode, people.length]);

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
  const hearCall = () => {
    // Inside the click, so WebKit counts it as permission to play sound.
    for (const a of audios.current) void a.play().catch(() => {});
    setBlocked(false);
  };

  const now = performance.now();
  const talking = (id: string) => isTalking(talk, id, now);
  // Everyone connected is heard, even someone briefly missing from the room
  // list while their connection to the host blinks.
  const sound = remotes.map((r) => <RemoteAudio key={r.id} remote={r} audios={audios.current} onBlocked={onBlocked} />);

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
            <Tile key={p.id} person={p} talking={talking(p.id)} onClick={() => (setPinned(p.id), setPrefs({ layout: "speaker" }))} />
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
          <Tile person={shown} talking={talking(shown.id)} />
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
}: {
  person: Person;
  stream?: MediaStream | null;
  talking: boolean;
  mirrored?: boolean;
  onClick?: () => void;
}) {
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
    <div className={`tile ${talking ? "talking" : ""} ${onClick ? "pickable" : ""}`} onClick={onClick} data-tauri-drag-region={onClick ? undefined : true}>
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
      </span>
    </div>
  );
}

/** Plays one person's voice. */
function RemoteAudio({ remote, audios, onBlocked }: { remote: Remote; audios: Set<HTMLAudioElement>; onBlocked: () => void }) {
  const el = useRef<HTMLAudioElement>(null);
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
