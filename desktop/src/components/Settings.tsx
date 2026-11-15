import { useEffect, useState } from "react";
import { cleanName, MAX_NAME } from "../name";
import type { PlayerSetup } from "../api";
import type { UpdateState } from "../update";
import { CheckIcon } from "./Icons";
import { DEFAULT_SHORTCUTS, fromEvent, label, type Shortcuts } from "../shortcuts";

interface Props {
  name: string;
  version: string;
  update: UpdateState;
  onRename: (name: string) => void;
  onCheck: () => void;
  onUpdate: () => void;
  setup: PlayerSetup | null;
  onInstallHelpers: () => Promise<void>;
  shortcuts: Shortcuts;
  /** Rejects with a message if the shortcuts can't be used. */
  onShortcuts: (s: Shortcuts) => Promise<void>;
}

export function Settings({ name, version, update, onRename, onCheck, onUpdate, setup, onInstallHelpers, shortcuts, onShortcuts }: Props) {
  return (
    <div className="settings">
      <h1 className="page-title">Settings</h1>
      <NameSection name={name} onRename={onRename} />
      <ShortcutsSection shortcuts={shortcuts} onChange={onShortcuts} />
      {setup && <HelpersSection setup={setup} onInstall={onInstallHelpers} />}
      <UpdatesSection version={version} update={update} onCheck={onCheck} onUpdate={onUpdate} />
    </div>
  );
}

function NameSection({ name, onRename }: { name: string; onRename: (name: string) => void }) {
  const [draft, setDraft] = useState(name);
  const [saved, setSaved] = useState(false);
  // Follow the engine's name until the person starts typing.
  useEffect(() => setDraft(name), [name]);

  const clean = cleanName(draft);
  const changed = clean !== null && clean !== name;

  const save = (e: React.FormEvent) => {
    e.preventDefault();
    if (!clean || !changed) return;
    onRename(clean);
    setSaved(true);
    setTimeout(() => setSaved(false), 2000);
  };

  return (
    <section className="setting" aria-labelledby="name-head">
      <div className="setting-text">
        <h2 id="name-head">Display name</h2>
        <p>What friends see in the room and in messages like "{clean ?? name} paused".</p>
      </div>
      <form className="setting-control join" onSubmit={save}>
        <input
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          maxLength={MAX_NAME}
          aria-label="Display name"
          spellCheck={false}
          autoCorrect="off"
          className="name-input"
        />
        <button className="primary" type="submit" disabled={!changed}>
          {saved ? "Saved" : "Save"}
        </button>
      </form>
    </section>
  );
}

function UpdatesSection({
  version,
  update,
  onCheck,
  onUpdate,
}: {
  version: string;
  update: UpdateState;
  onCheck: () => void;
  onUpdate: () => void;
}) {
  let status: React.ReactNode = null;
  let action: React.ReactNode = (
    <button className="ghost" onClick={onCheck}>
      Check for updates
    </button>
  );
  switch (update.kind) {
    case "checking":
      status = (
        <>
          <span className="spinner" /> Checking for updates…
        </>
      );
      action = null;
      break;
    case "latest":
      status = (
        <>
          <CheckIcon /> You're up to date
        </>
      );
      break;
    case "check_failed":
      status = <span className="bad">Couldn't reach GitHub. Check your internet connection.</span>;
      action = (
        <button className="ghost" onClick={onCheck}>
          Try again
        </button>
      );
      break;
    case "available":
      status = <span className="signal-text">WatchSync {update.version} is available</span>;
      action = (
        <button className="primary" onClick={onUpdate}>
          Update to {update.version}
        </button>
      );
      break;
    case "installing":
      status = (
        <>
          <span className="spinner" /> {update.percent === null ? "Updating…" : `Downloading ${update.percent}%`}
        </>
      );
      action = null;
      break;
    case "failed":
      status = <span className="bad" title={update.message}>The update didn't install: {update.message}</span>;
      action = (
        <button className="primary" onClick={onUpdate}>
          Try again
        </button>
      );
      break;
  }

  return (
    <section className="setting" aria-labelledby="updates-head">
      <div className="setting-text">
        <h2 id="updates-head">Updates</h2>
        <p>
          You have WatchSync <span className="mono">{version}</span>. It looks for new versions on its own and shows
          them in the sidebar.
        </p>
        {status && (
          <p className="setting-status" aria-live="polite">
            {status}
          </p>
        )}
      </div>
      <div className="setting-control">{action}</div>
    </section>
  );
}

/** mpv and croc, which WatchSync uses under the hood. */
function HelpersSection({ setup, onInstall }: { setup: PlayerSetup; onInstall: () => Promise<void> }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const missing = [!setup.installed && "mpv", !setup.croc && "croc"].filter(Boolean) as string[];

  const install = async () => {
    setBusy(true);
    setError(null);
    try {
      await onInstall();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="setting" aria-labelledby="helpers-head">
      <div className="setting-text">
        <h2 id="helpers-head">Helper apps</h2>
        <ul className="helpers">
          <li>
            <span className={`person-dot ${setup.installed ? "match" : "mismatch"}`} />
            mpv plays the movie {setup.installed ? "" : "(not installed)"}
          </li>
          <li>
            <span className={`person-dot ${setup.croc ? "match" : "mismatch"}`} />
            croc sends movies to friends {setup.croc ? "" : "(not installed)"}
          </li>
        </ul>
        {missing.length > 0 && !setup.has_brew && (
          <p>Install with Homebrew in Terminal: brew install {missing.join(" ")}</p>
        )}
        {error && <p className="error-text">{error}</p>}
      </div>
      <div className="setting-control">
        {missing.length === 0 ? (
          <span className="setting-status">
            <CheckIcon /> All set
          </span>
        ) : (
          setup.has_brew && (
            <button className="primary" onClick={install} disabled={busy}>
              {busy ? "Installing…" : `Install ${missing.join(" and ")}`}
            </button>
          )
        )}
      </div>
    </section>
  );
}

function ShortcutsSection({ shortcuts, onChange }: { shortcuts: Shortcuts; onChange: (s: Shortcuts) => Promise<void> }) {
  const [recording, setRecording] = useState<keyof Shortcuts | null>(null);
  const [error, setError] = useState<string | null>(null);

  const change = async (next: Shortcuts) => {
    try {
      await onChange(next);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  };

  const record = (which: keyof Shortcuts) => (e: React.KeyboardEvent) => {
    if (recording !== which) return;
    e.preventDefault();
    if (e.key === "Escape") return setRecording(null);
    const shortcut = fromEvent(e.nativeEvent);
    if (!shortcut) return; // still holding modifiers
    setRecording(null);
    void change({ ...shortcuts, [which]: shortcut });
  };

  const field = (which: keyof Shortcuts, what: string) => (
    <button
      className={`shortcut ${recording === which ? "recording" : ""}`}
      onClick={() => setRecording(recording === which ? null : which)}
      onKeyDown={record(which)}
      onBlur={() => setRecording(null)}
      aria-label={`${what} shortcut`}
    >
      {recording === which ? "Press keys…" : label(shortcuts[which])}
    </button>
  );

  const isDefault = shortcuts.mic === DEFAULT_SHORTCUTS.mic && shortcuts.cam === DEFAULT_SHORTCUTS.cam;
  return (
    <section className="setting" aria-labelledby="shortcuts-head">
      <div className="setting-text">
        <h2 id="shortcuts-head">Call shortcuts</h2>
        <p>Turn your mic and camera on or off from any app, even with the movie fullscreen. They only work while you're in a room.</p>
        {error && (
          <p className="setting-error" role="alert">
            {error}
          </p>
        )}
      </div>
      <div className="setting-control shortcuts">
        <label>
          Mic {field("mic", "Mic")}
        </label>
        <label>
          Camera {field("cam", "Camera")}
        </label>
        {!isDefault && (
          <button className="link" onClick={() => void change(DEFAULT_SHORTCUTS)}>
            Reset
          </button>
        )}
      </div>
    </section>
  );
}
