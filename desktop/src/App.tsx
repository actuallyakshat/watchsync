import { useCallback, useEffect, useRef, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { open, ask } from "@tauri-apps/plugin-dialog";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { getCurrent, onOpenUrl } from "@tauri-apps/plugin-deep-link";
import { EMPTY, installCroc, installPlayer, onSnapshot, playerSetup, send, setCallShortcuts, showCall, type Comfort, type PlayerSetup, type Snapshot } from "./api";
import { Sidebar } from "./components/Sidebar";
import { Home } from "./components/Home";
import { Room } from "./components/Room";
import { Setup } from "./components/Setup";
import { Settings } from "./components/Settings";
import { BackIcon, LeaveIcon } from "./components/Icons";
import { saveComfort, savedComfort, saveName, savedName } from "./name";
import { useUpdater } from "./update";
import { saveShortcuts, savedShortcuts, type Shortcuts } from "./shortcuts";

const VIDEO_EXTENSIONS = ["mkv", "mp4", "m4v", "mov", "avi", "webm", "ts", "wmv", "flv", "mpg", "mpeg"];

export function App() {
  const [snap, setSnap] = useState<Snapshot>(EMPTY);
  const [setup, setSetup] = useState<PlayerSetup | null>(null);
  const [version, setVersion] = useState("");
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [shortcuts, setShortcuts] = useState<Shortcuts>(savedShortcuts);
  const updater = useUpdater();

  useEffect(() => {
    const unlisten = onSnapshot(setSnap);
    const name = savedName();
    if (name) void send({ type: "set_name", name });
    const comfort = savedComfort();
    if (comfort) void send({ type: "set_comfort", comfort });
    void getVersion().then(setVersion);
    void playerSetup().then(setSetup);
    const keys = savedShortcuts();
    void setCallShortcuts(keys.mic, keys.cam).catch(() => {});
    return () => void unlisten.then((f) => f());
  }, []);

  const chooseMovie = useCallback(async () => {
    const path = await open({
      multiple: false,
      directory: false,
      title: "Choose your copy of the movie",
      filters: [{ name: "Movies", extensions: VIDEO_EXTENSIONS }],
    });
    if (typeof path === "string") void send({ type: "open_file", path });
  }, []);

  // Who takes over if the host leaves: the next person who joined.
  const successor = snap.peers.find((p) => !p.is_host && !p.outdated)?.name ?? null;
  const alone = snap.peers.length <= 1;

  const leave = useCallback(async () => {
    const message = !snap.is_host
      ? "You can rejoin with the same invite link."
      : alone
        ? "You're the only one here, so this ends the room."
        : `${successor ?? "The next person"} becomes the host and the movie keeps going for everyone.`;
    const ok = await confirmOr(message, {
      title: snap.is_host && alone ? "End the room?" : "Leave the room?",
      okLabel: snap.is_host && alone ? "End room" : "Leave",
      cancelLabel: "Stay",
      kind: "warning",
    });
    if (ok) void send({ type: "leave" });
  }, [snap.is_host, alone, successor]);

  // Invite links (watchsync://join/…) open the app and join that room.
  const phase = useRef(snap.phase);
  phase.current = snap.phase;
  useEffect(() => {
    const join = async (urls: string[] | null) => {
      const url = urls?.find((u) => u.startsWith("watchsync:"));
      if (!url) return;
      if (phase.current === "room") {
        const ok = await confirmOr("You'll leave the room you're in now.", {
          title: "Join the other room?",
          okLabel: "Join",
          cancelLabel: "Stay",
        });
        if (!ok) return;
        await send({ type: "leave" });
      }
      setSettingsOpen(false);
      void send({ type: "join", code: url });
    };
    void getCurrent()
      .then(join)
      .catch(() => {});
    const unlisten = onOpenUrl(join).catch(() => () => {});
    return () => void unlisten.then((f) => f());
  }, []);

  const runUpdate = useCallback(async () => {
    if (snap.phase === "room") {
      const ok = await confirmOr(
        snap.is_host
          ? `WatchSync restarts to update. ${alone ? "The room ends." : `${successor ?? "The next person"} becomes the host while you're away.`}`
          : "WatchSync restarts to update. Rejoin with the same invite link afterwards.",
        { title: "Update now?", okLabel: "Update", cancelLabel: "Later" },
      );
      if (!ok) return;
    }
    await updater.install();
  }, [snap.phase, snap.is_host, alone, successor, updater]);

  // Space plays/pauses for everyone; arrows skip 10 seconds.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (snap.phase !== "room" || !snap.player_open) return;
      const t = e.target;
      if (
        t instanceof HTMLInputElement ||
        t instanceof HTMLTextAreaElement ||
        t instanceof HTMLSelectElement ||
        (t instanceof HTMLElement && t.isContentEditable) ||
        e.metaKey ||
        e.ctrlKey
      )
        return;
      if (e.code === "Space") {
        e.preventDefault();
        void send({ type: "toggle_play" });
      } else if (e.key === "ArrowLeft") void send({ type: "seek_by", seconds: -10 });
      else if (e.key === "ArrowRight") void send({ type: "seek_by", seconds: 10 });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [snap.phase, snap.player_open]);

  const rename = useCallback((name: string) => {
    saveName(name);
    void send({ type: "set_name", name });
  }, []);

  const setComfort = useCallback((comfort: Comfort) => {
    saveComfort(comfort);
    void send({ type: "set_comfort", comfort });
  }, []);

  const needsPlayer = setup !== null && !setup.installed;

  return (
    <div className="app">
      <Sidebar
        snap={snap}
        version={version}
        update={updater.state}
        onCopyInvite={async () => {
          if (snap.invite_link) await writeText(snap.invite_link);
        }}
        onChat={(text) => void send({ type: "chat", text })}
        onReact={(reaction) => void send({ type: "react", reaction })}
        onShowCall={() => void showCall()}
        onUpdate={runUpdate}
        settingsOpen={settingsOpen}
        onSettings={() => setSettingsOpen((o) => !o)}
      />
      <main className="main">
        <div className="drag topbar" data-tauri-drag-region>
          {settingsOpen ? (
            <button className="ghost" onClick={() => setSettingsOpen(false)}>
              <span style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
                <BackIcon />
                {snap.phase === "room" ? "Back to the room" : "Back"}
              </span>
            </button>
          ) : snap.phase === "room" && (
            <button className="ghost" onClick={leave}>
              <span style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
                <LeaveIcon />
                {snap.is_host && alone ? "End room" : "Leave room"}
              </span>
            </button>
          )}
        </div>
        <div className="content">
          {snap.reconnecting && (
            <div className="banner calm" role="status">
              <span className="spinner" />
              Reconnecting to the room. Your movie keeps playing.
            </div>
          )}
          {snap.error && <div className="banner" role="alert">{snap.error}</div>}
          {settingsOpen ? (
            <Settings
              name={snap.name}
              version={version}
              update={updater.state}
              onRename={rename}
              shortcuts={shortcuts}
              onShortcuts={async (next) => {
                await setCallShortcuts(next.mic, next.cam);
                saveShortcuts(next);
                setShortcuts(next);
              }}
              onCheck={() => void updater.checkNow()}
              onUpdate={runUpdate}
              setup={setup}
              onInstallHelpers={async () => {
                await installPlayer();
                setSetup(await playerSetup());
              }}
            />
          ) : needsPlayer ? (
            <Setup setup={setup} onRecheck={() => void playerSetup().then(setSetup)} onCopy={(t) => writeText(t)} />
          ) : snap.phase === "room" ? (
            <Room
              snap={snap}
              onChoose={chooseMovie}
              onToggle={() => void send({ type: "toggle_play" })}
              onSeekBy={(seconds) => void send({ type: "seek_by", seconds })}
              onSeekTo={(position) => void send({ type: "seek_to", position })}
              onCrop={(crop) => void send({ type: "set_crop", crop })}
              onTrack={(kind, id) => void send({ type: "set_track", kind, id })}
              onComfort={setComfort}
              onSubScale={(scale) => void send({ type: "set_sub_scale", scale })}
              onOpenFound={(path) => void send({ type: "open_file", path })}
              files={{
                crocInstalled: setup?.croc ?? false,
                crocOutdated: setup?.croc_outdated ?? false,
                hasBrew: setup?.has_brew ?? false,
                onInstallCroc: async () => {
                  await installCroc();
                  setSetup(await playerSetup());
                },
                onShare: (method) => void send({ type: "share_file", method }),
                onStopSharing: () => void send({ type: "stop_sharing" }),
                onGet: (offer_id) => void send({ type: "get_file", offer_id }),
                onCancel: (id) => void send({ type: "cancel_transfer", id }),
              }}
            />
          ) : (
            <Home
              connecting={snap.phase === "connecting"}
              onHost={() => void send({ type: "host" })}
              onJoin={(code) => void send({ type: "join", code })}
            />
          )}
        </div>
      </main>
    </div>
  );
}

/**
 * Ask for confirmation. If the dialog can't be shown, go ahead rather than
 * leave a button that silently does nothing.
 */
async function confirmOr(message: string, options: Parameters<typeof ask>[1]): Promise<boolean> {
  try {
    return await ask(message, options);
  } catch (e) {
    console.error("confirmation dialog failed", e);
    return true;
  }
}
