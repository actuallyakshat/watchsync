import { useState } from "react";
import type { Person, Snapshot } from "../api";
import { formatDrift } from "../format";
import type { UpdateState } from "../update";
import { ArrowUpIcon, CheckIcon, CopyIcon, Mark } from "./Icons";

interface Props {
  snap: Snapshot;
  version: string;
  update: UpdateState;
  onCopyCode: () => Promise<void>;
  onUpdate: () => void;
}

export function Sidebar({ snap, version, update, onCopyCode, onUpdate }: Props) {
  const [copied, setCopied] = useState(false);
  const inRoom = snap.phase === "room";

  const copy = async () => {
    await onCopyCode();
    setCopied(true);
    setTimeout(() => setCopied(false), 1800);
  };

  return (
    <nav className="sidebar" aria-label="Room">
      <div className="drag" data-tauri-drag-region />
      <div className="brand" data-tauri-drag-region>
        <Mark size={20} />
        <span>WatchSync</span>
      </div>

      <div className="side-label">{inRoom ? "In this room" : "Room"}</div>
      {inRoom && snap.peers.length > 0 ? (
        <ul className="people">
          {snap.peers.map((p) => (
            <PersonRow key={`${p.name}-${p.is_host}`} person={p} />
          ))}
        </ul>
      ) : (
        <p className="side-empty people">{inRoom ? "Just you so far." : "Host or join a room to see who's watching."}</p>
      )}

      <div className="sidebar-foot">
        {inRoom && snap.is_host && snap.code && (
          <button className="foot-link" onClick={copy} title="Copy the code friends use to join">
            {copied ? <CheckIcon /> : <CopyIcon />}
            {copied ? "Copied. Send it to friends" : "Copy invite code"}
          </button>
        )}
        <UpdateLink state={update} onUpdate={onUpdate} />
        <div className="version mono">WatchSync {version}</div>
      </div>
    </nav>
  );
}

function PersonRow({ person: p }: { person: Person }) {
  const dot = p.file === "match" ? "match" : p.file === "mismatch" ? "mismatch" : p.file === "no_file" ? "none" : "";
  let meta: { text: string; tone?: "warn" | "bad" };
  if (p.outdated) meta = { text: "needs update", tone: "warn" };
  else if (p.file === "mismatch") meta = { text: "different file", tone: "bad" };
  else if (p.is_host) meta = { text: "host" };
  else if (p.file === "no_file") meta = { text: "no movie yet" };
  else meta = { text: formatDrift(p.drift_ms) };

  const title =
    p.file === "match"
      ? "Has the same file as the host"
      : p.file === "mismatch"
        ? "Has a different file from the host. The timing will be off."
        : p.file === "no_file"
          ? "Hasn't chosen a movie yet"
          : "";

  return (
    <li className="person" title={title}>
      <span className={`person-dot ${dot}`} />
      <span className="person-name">{p.name}</span>
      <span className={`person-meta ${meta.tone ?? ""} ${p.is_host || p.outdated ? "" : "mono"}`}>{meta.text}</span>
    </li>
  );
}

function UpdateLink({ state, onUpdate }: { state: UpdateState; onUpdate: () => void }) {
  if (state.kind === "available")
    return (
      <button className="foot-link signal" onClick={onUpdate}>
        <ArrowUpIcon />
        Update to {state.version}
        <span className="dot" />
      </button>
    );
  if (state.kind === "installing")
    return (
      <div className="foot-link" aria-live="polite">
        <span className="spinner" />
        {state.percent === null ? "Updating…" : `Downloading ${state.percent}%`}
      </div>
    );
  if (state.kind === "failed")
    return (
      <button className="foot-link" onClick={onUpdate} title={state.message}>
        <ArrowUpIcon />
        Update failed. Try again
      </button>
    );
  return null;
}
