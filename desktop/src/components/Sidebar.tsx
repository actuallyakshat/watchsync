import { useState } from "react";
import type { Person, Reaction, Snapshot } from "../api";
import { formatDrift } from "../format";
import type { UpdateState } from "../update";
import { Chat } from "./Chat";
import { ArrowUpIcon, CamIcon, CheckIcon, CopyIcon, GearIcon, Mark, MicIcon, ShareScreenIcon } from "./Icons";

interface Props {
  snap: Snapshot;
  version: string;
  update: UpdateState;
  onCopyInvite: () => Promise<void>;
  onUpdate: () => void;
  settingsOpen: boolean;
  onSettings: () => void;
  onChat: (text: string) => void;
  onReact: (r: Reaction) => void;
  onShowCall: () => void;
}

export function Sidebar({ snap, version, update, onCopyInvite, onUpdate, settingsOpen, onSettings, onChat, onReact, onShowCall }: Props) {
  const [copied, setCopied] = useState(false);
  const inRoom = snap.phase === "room";

  const copy = async () => {
    await onCopyInvite();
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
        <p className="side-empty">{inRoom ? "Just you so far." : "Host or join a room to see who's watching."}</p>
      )}

      {inRoom ? <Chat lines={snap.chat} onSend={onChat} onReact={onReact} /> : <div className="spacer" />}

      <div className="sidebar-foot">
        {inRoom && snap.invite_link && (
          <button className="foot-link" onClick={copy} title="Copy the link friends tap to join">
            {copied ? <CheckIcon /> : <CopyIcon />}
            {copied ? "Copied. Send it to friends" : "Copy invite link"}
          </button>
        )}
        {inRoom && (
          <button className="foot-link" onClick={onShowCall} title="Bring up the call window">
            <CamIcon />
            Show call
          </button>
        )}
        <UpdateLink state={update} onUpdate={onUpdate} />
        <button className={`foot-link ${settingsOpen ? "on" : ""}`} onClick={onSettings} aria-pressed={settingsOpen}>
          <GearIcon />
          Settings
        </button>
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
      <span
        className="person-call"
        aria-label={`${p.name}: mic ${p.mic ? "on" : "off"}, camera ${p.cam ? "on" : "off"}${p.screen ? ", sharing their screen" : ""}`}
      >
        {p.mic && <MicIcon />}
        {p.cam && <CamIcon />}
        {p.screen && <ShareScreenIcon />}
      </span>
      <span className={`person-meta ${meta.tone ?? ""} ${p.is_host || p.outdated ? "" : "mono"}`}>{meta.text}</span>
    </li>
  );
}

/** Only shown when there's something to do; checking lives in Settings. */
function UpdateLink({ state, onUpdate }: { state: UpdateState; onUpdate: () => void }) {
  switch (state.kind) {
    case "available":
      return (
        <button className="foot-link signal" onClick={onUpdate}>
          <ArrowUpIcon />
          Update to {state.version}
          <span className="dot" />
        </button>
      );
    case "installing":
      return (
        <div className="foot-link" aria-live="polite">
          <span className="spinner" />
          {state.percent === null ? "Updating…" : `Downloading ${state.percent}%`}
        </div>
      );
    case "failed":
      return (
        <button className="foot-link" onClick={onUpdate} title={state.message}>
          <ArrowUpIcon />
          Update failed. Try again
        </button>
      );
    default:
      return null;
  }
}
