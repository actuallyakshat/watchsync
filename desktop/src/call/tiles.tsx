// One tile in the expanded call: a person's camera (or their initial), or
// a screen they share. Hovering shows what you can do with it: pin it to
// the big spot (or unpin it), and set how loud it is.
import { useEffect, useRef } from "react";
import { MicIcon, PinIcon, ShareScreenIcon, VolumeIcon } from "../components/Icons";
import type { TileKind } from "./layout";
import type { Remote, Size } from "./mesh";

export interface Person {
  id: string;
  name: string;
  cam: boolean;
  mic: boolean;
  /** Sharing their screen. */
  screen: boolean;
  remote: Remote | null;
}

export function initial(name: string): string {
  return [...name.trim()][0]?.toUpperCase() ?? "?";
}

/** "Riya's screen", "Your screen". */
export function screenName(p: Person): string {
  return p.id === "" ? "Your screen" : `${p.name}'s screen`;
}

export function Tile({
  person,
  kind,
  stream,
  version = 0,
  talking,
  mirrored,
  relayed,
  pinned,
  onPin,
  onPick,
  volume,
  onVolume,
  onSize,
  style,
}: {
  person: Person;
  kind: TileKind;
  /** Their camera or screen (with its sound, which plays elsewhere). */
  stream: MediaStream | null;
  /** Bumped when the stream's tracks change. */
  version?: number;
  talking: boolean;
  /** Your own camera, shown like a mirror. */
  mirrored?: boolean;
  /** Their media comes through the iroh relay. */
  relayed?: boolean;
  /** This tile is pinned to the big spot. */
  pinned: boolean;
  /** Pin it, or unpin it if it's pinned. */
  onPin?: () => void;
  /** Clicking the tile: bring it to the big spot. */
  onPick?: () => void;
  /** Their voice, or their screen's sound (0–1). */
  volume?: number;
  onVolume?: (v: number) => void;
  /** How big this is shown, in device pixels (0×0 once gone). */
  onSize?: (id: string, size: Size) => void;
  style?: React.CSSProperties;
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

  const screen = kind === "screen";
  const on = screen ? person.screen : person.cam;
  const hasVideo = on && !!stream && stream.getVideoTracks().length > 0;
  const video = useRef<HTMLVideoElement>(null);
  useEffect(() => {
    const v = video.current;
    if (!v) return;
    if (v.srcObject !== stream) v.srcObject = stream;
    void v.play().catch(() => {});
  }, [stream, version, hasVideo]);

  const mine = person.id === "";
  const label = screen ? screenName(person) : person.name;
  // Dragging the window by its tiles, except where a click means something.
  const drag = onPick ? undefined : true;
  return (
    <div
      ref={box}
      className={`tile ${screen ? "screen" : ""} ${talking ? "talking" : ""} ${onPick ? "pickable" : ""}`}
      onClick={onPick}
      data-tauri-drag-region={drag}
      style={style}
    >
      {hasVideo ? (
        <video ref={video} className={mirrored && !screen ? "mirrored" : ""} autoPlay playsInline muted aria-label={label} data-tauri-drag-region={drag} />
      ) : screen ? (
        <span className="call-note" data-tauri-drag-region={drag}>
          {mine ? "Starting your share…" : `Waiting for ${person.name}'s screen…`}
        </span>
      ) : (
        <span className={`avatar ${talking ? "talking" : ""}`} data-tauri-drag-region={drag}>
          {initial(person.name)}
        </span>
      )}
      <span className="tile-name">
        {pinned && (
          <span className="tile-pinned" aria-label="Pinned">
            <PinIcon />
          </span>
        )}
        {screen && <ShareScreenIcon />}
        {label}
        {!screen && !person.mic && <MicIcon off />}
        {relayed && (
          <span className="tile-path" data-tip="No direct connection: relayed through iroh">
            via iroh
          </span>
        )}
      </span>
      {(onPin || onVolume) && (
        <div className="tile-actions" onClick={(e) => e.stopPropagation()}>
          {onVolume && (
            <label className="tile-volume" data-tip={screen ? `Sound from ${label}` : `${person.name}'s voice`}>
              <VolumeIcon />
              <input
                type="range"
                min={0}
                max={1}
                step={0.05}
                value={volume ?? 1}
                onChange={(e) => onVolume(Number(e.target.value))}
                aria-label={screen ? `${label} sound volume` : `${person.name}'s volume`}
              />
            </label>
          )}
          {onPin && (
            <button
              className={`tile-btn ${pinned ? "on" : ""}`}
              onClick={onPin}
              aria-label={pinned ? `Unpin ${label}` : `Pin ${label}`}
              aria-pressed={pinned}
              data-tip={pinned ? "Unpin: the big spot follows the call again" : `Pin ${mine && !screen ? "yourself" : label} to the big spot`}
            >
              <PinIcon />
            </button>
          )}
        </div>
      )}
    </div>
  );
}
