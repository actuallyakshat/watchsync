import { useRef } from "react";
import type { Comfort, Crop, Snapshot, TrackKind } from "../api";
import { formatTime, frameSize, movieTitle } from "../format";
import { PauseIcon, PlayIcon, Skip10 } from "./Icons";
import { Sound } from "./Sound";
import { Files } from "./Files";

/** Tallest to widest. Must match `Crop::ALL` in src/sync.rs. */
export const CROPS: { id: Crop; label: string; aspect: number | null; imax?: boolean }[] = [
  { id: "original", label: "Original", aspect: null },
  { id: "classic", label: "4:3", aspect: 4 / 3 },
  { id: "imax_143", label: "1.43:1", aspect: 1.43, imax: true },
  { id: "euro_166", label: "1.66:1", aspect: 1.66 },
  { id: "wide", label: "16:9", aspect: 16 / 9 },
  { id: "flat", label: "1.85:1", aspect: 1.85 },
  { id: "imax_190", label: "1.90:1", aspect: 1.9, imax: true },
  { id: "univisium_200", label: "2.00:1", aspect: 2 },
  { id: "todd_ao_220", label: "2.20:1", aspect: 2.2 },
  { id: "scope", label: "2.39:1", aspect: 2.39 },
  { id: "ultra_276", label: "2.76:1", aspect: 2.76 },
];

interface Props {
  snap: Snapshot;
  onChoose: () => void;
  onToggle: () => void;
  onSeekBy: (seconds: number) => void;
  onSeekTo: (position: number) => void;
  onCrop: (crop: Crop) => void;
  onTrack: (kind: TrackKind, id: number | null) => void;
  onComfort: (c: Comfort) => void;
  onSubScale: (scale: number) => void;
  files: Omit<React.ComponentProps<typeof Files>, "snap">;
  onOpenFound: (path: string) => void;
}

export function Room({ snap, onChoose, onToggle, onSeekBy, onSeekTo, onCrop, onTrack, onComfort, onSubScale, files, onOpenFound }: Props) {
  const host = snap.peers.find((p) => p.is_host);

  return (
    <>
      {snap.file_name ? (
        <MovieHeader fileName={snap.file_name} file={snap.is_host ? undefined : snap.my_file} hostHasFile={!!host && host.file !== "no_file"} onChoose={onChoose} />
      ) : (
        <ChooseMovie onChoose={onChoose} found={snap.found} onOpen={onOpenFound} />
      )}

      <Player snap={snap} onToggle={onToggle} onSeekBy={onSeekBy} onSeekTo={onSeekTo} />

      <Files snap={snap} {...files} />

      <section className="section" aria-labelledby="picture-head">
        <div className="section-head">
          <h2 id="picture-head">Picture</h2>
          <span>{snap.crop_by && snap.crop !== "original" ? `Set by ${snap.crop_by}, for everyone` : "Applies to everyone in the room"}</span>
        </div>
        <div className="chips" role="radiogroup" aria-label="Picture crop">
          {CROPS.map((c) => {
            const f = frameSize(c.aspect);
            return (
              <button
                key={c.id}
                role="radio"
                aria-checked={snap.crop === c.id}
                aria-label={c.imax ? `IMAX ${c.label}` : c.label}
                className={`chip ${snap.crop === c.id ? "on" : ""}`}
                title={c.aspect === null ? "The movie as it is in the file" : `Crop to ${c.imax ? "IMAX " : ""}${c.label}`}
                onClick={() => onCrop(c.id)}
              >
                <span className={`frame ${c.aspect === null ? "full" : ""}`}>
                  <i style={{ width: f.w, height: f.h }} />
                </span>
                {c.imax && <span className="imax">IMAX</span>}
                {c.label}
              </button>
            );
          })}
        </div>
      </section>

      <Sound
        tracks={snap.tracks}
        comfort={snap.comfort}
        playerOpen={snap.player_open}
        onTrack={onTrack}
        onComfort={onComfort}
        subScale={snap.sub_scale}
        subScaleBy={snap.sub_scale_by}
        onSubScale={onSubScale}
      />
    </>
  );
}

function MovieHeader({
  fileName,
  file,
  hostHasFile,
  onChoose,
}: {
  fileName: string;
  file?: string;
  hostHasFile: boolean;
  onChoose: () => void;
}) {
  const { title, year } = movieTitle(fileName);
  let note: { text: string; tone: string } | null = null;
  if (file === "match" && hostHasFile) note = { text: "Same file as the host", tone: "match" };
  if (file === "mismatch") note = { text: "Different file from the host", tone: "mismatch" };

  return (
    <header>
      <h1 className="movie-title">
        {title}
        {year && <span className="year"> {year}</span>}
      </h1>
      <div className="movie-sub">
        {note && <span className={`match-note ${note.tone}`}>{note.text}</span>}
        <span className="file-name mono" title={fileName}>
          {fileName}
        </span>
        <button className="link" onClick={onChoose}>
          Change
        </button>
      </div>
    </header>
  );
}

function ChooseMovie({
  onChoose,
  found,
  onOpen,
}: {
  onChoose: () => void;
  found: Snapshot["found"];
  onOpen: (path: string) => void;
}) {
  if (found) {
    const name = found.path.split("/").pop() ?? found.path;
    const { title } = movieTitle(name);
    return (
      <header>
        <h1 className="movie-title">Found {title} on your Mac</h1>
        <div className="choose">
          <p>
            It's in {found.folder}, with the same file name as the host's copy.
          </p>
          <p className="file-name mono found-name" title={found.path}>
            {name}
          </p>
          <div className="row-actions">
            <button className="primary" onClick={() => onOpen(found.path)}>
              Open it
            </button>
            <button className="ghost" onClick={onChoose}>
              Choose a different file…
            </button>
          </div>
        </div>
      </header>
    );
  }
  return (
    <header>
      <h1 className="movie-title">Choose your copy of the movie</h1>
      <div className="choose">
        <p>Everyone opens the same movie file from their own Mac. WatchSync checks that the copies match.</p>
        <button className="primary" onClick={onChoose}>
          Choose movie…
        </button>
      </div>
    </header>
  );
}

function Player({
  snap,
  onToggle,
  onSeekBy,
  onSeekTo,
}: {
  snap: Snapshot;
  onToggle: () => void;
  onSeekBy: (s: number) => void;
  onSeekTo: (p: number) => void;
}) {
  const track = useRef<HTMLDivElement>(null);
  const ready = snap.player_open && snap.duration > 0;
  const pct = ready ? Math.min(100, (snap.position / snap.duration) * 100) : 0;

  const seekFromClick = (e: React.MouseEvent) => {
    if (!ready || !track.current) return;
    const r = track.current.getBoundingClientRect();
    onSeekTo(((e.clientX - r.left) / r.width) * snap.duration);
  };

  return (
    <div className={`player ${snap.player_open ? "" : "off"}`}>
      <div
        ref={track}
        className={`track ${ready ? "" : "disabled"}`}
        onClick={seekFromClick}
        role="slider"
        aria-label="Position"
        aria-valuemin={0}
        aria-valuemax={Math.round(snap.duration)}
        aria-valuenow={Math.round(snap.position)}
      >
        <div className="track-fill" style={{ width: `${pct}%` }} />
        <div className="track-knob" style={{ left: `${pct}%` }} />
      </div>
      <div className="times mono">
        <span className="now">{formatTime(snap.position)}</span>
        <span>{ready ? formatTime(snap.duration) : "--:--"}</span>
      </div>
      <div className="controls">
        <button className="skip" onClick={() => onSeekBy(-10)} disabled={!ready} aria-label="Back 10 seconds">
          <Skip10 back />
        </button>
        <button className="play" onClick={onToggle} disabled={!snap.player_open} aria-label={snap.playing ? "Pause for everyone" : "Play for everyone"}>
          {snap.playing ? <PauseIcon /> : <PlayIcon />}
        </button>
        <button className="skip" onClick={() => onSeekBy(10)} disabled={!ready} aria-label="Forward 10 seconds">
          <Skip10 />
        </button>
      </div>
      <p className="activity" aria-live="polite">
        {snap.last_action ? (
          <>
            <span className="live" />
            {snap.last_action}
          </>
        ) : snap.player_open ? (
          "Space pauses for everyone, here or in the movie window."
        ) : (
          "Controls work once you've chosen your copy of the movie."
        )}
      </p>
    </div>
  );
}
