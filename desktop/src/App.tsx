import { useCallback, useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { open, ask } from "@tauri-apps/plugin-dialog";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { EMPTY, onSnapshot, playerSetup, send, type PlayerSetup, type Snapshot } from "./api";
import { Sidebar } from "./components/Sidebar";
import { Home } from "./components/Home";
import { Room } from "./components/Room";
import { Setup } from "./components/Setup";
import { LeaveIcon } from "./components/Icons";
import { useUpdater } from "./update";

const VIDEO_EXTENSIONS = ["mkv", "mp4", "m4v", "mov", "avi", "webm", "ts", "wmv", "flv", "mpg", "mpeg"];

export function App() {
  const [snap, setSnap] = useState<Snapshot>(EMPTY);
  const [setup, setSetup] = useState<PlayerSetup | null>(null);
  const [version, setVersion] = useState("");
  const updater = useUpdater();

  useEffect(() => {
    const unlisten = onSnapshot(setSnap);
    void getVersion().then(setVersion);
    void playerSetup().then(setSetup);
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

  const leave = useCallback(async () => {
    const ok = await ask(
      snap.is_host ? "You're the host, so this ends the room for everyone." : "You can rejoin with the same invite code.",
      { title: "Leave the room?", okLabel: "Leave", cancelLabel: "Stay", kind: "warning" },
    );
    if (ok) void send({ type: "leave" });
  }, [snap.is_host]);

  const runUpdate = useCallback(async () => {
    if (snap.phase === "room") {
      const ok = await ask(
        snap.is_host
          ? "WatchSync restarts to update. You're the host, so the room ends for everyone."
          : "WatchSync restarts to update. Rejoin with the same invite code afterwards.",
        { title: "Update now?", okLabel: "Update", cancelLabel: "Later" },
      );
      if (!ok) return;
    }
    await updater.install();
  }, [snap.phase, snap.is_host, updater]);

  // Space plays/pauses for everyone; arrows skip 10 seconds.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (snap.phase !== "room" || !snap.player_open) return;
      if (e.target instanceof HTMLInputElement || e.metaKey || e.ctrlKey) return;
      if (e.code === "Space") {
        e.preventDefault();
        void send({ type: "toggle_play" });
      } else if (e.key === "ArrowLeft") void send({ type: "seek_by", seconds: -10 });
      else if (e.key === "ArrowRight") void send({ type: "seek_by", seconds: 10 });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [snap.phase, snap.player_open]);

  const needsPlayer = setup !== null && !setup.installed;

  return (
    <div className="app">
      <Sidebar
        snap={snap}
        version={version}
        update={updater.state}
        onCopyCode={async () => {
          if (snap.code) await writeText(snap.code);
        }}
        onUpdate={runUpdate}
        onCheck={() => void updater.checkNow()}
      />
      <main className="main">
        <div className="drag topbar" data-tauri-drag-region>
          {snap.phase === "room" && (
            <button className="ghost" onClick={leave}>
              <span style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
                <LeaveIcon />
                {snap.is_host ? "End room" : "Leave room"}
              </span>
            </button>
          )}
        </div>
        <div className="content">
          {snap.error && <div className="banner" role="alert">{snap.error}</div>}
          {needsPlayer ? (
            <Setup setup={setup} onRecheck={() => void playerSetup().then(setSetup)} onCopy={(t) => writeText(t)} />
          ) : snap.phase === "room" ? (
            <Room
              snap={snap}
              onChoose={chooseMovie}
              onToggle={() => void send({ type: "toggle_play" })}
              onSeekBy={(seconds) => void send({ type: "seek_by", seconds })}
              onSeekTo={(position) => void send({ type: "seek_to", position })}
              onCrop={(crop) => void send({ type: "set_crop", crop })}
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
