// In-app updates: GitHub releases publish a signed latest.json; the updater
// plugin checks it, verifies the signature, swaps the app and we relaunch.
import { useCallback, useEffect, useState } from "react";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

export type UpdateState =
  | { kind: "none" }
  | { kind: "available"; version: string }
  | { kind: "installing"; percent: number | null }
  | { kind: "failed"; message: string };

const SIX_HOURS = 6 * 60 * 60 * 1000;

export function useUpdater() {
  const [state, setState] = useState<UpdateState>({ kind: "none" });
  const [update, setUpdate] = useState<Update | null>(null);

  useEffect(() => {
    let alive = true;
    const look = async () => {
      try {
        const u = await check();
        if (!alive) return;
        setUpdate(u);
        setState(u ? { kind: "available", version: u.version } : { kind: "none" });
      } catch {
        // Offline or no release yet: try again later, quietly.
      }
    };
    void look();
    const timer = setInterval(look, SIX_HOURS);
    return () => {
      alive = false;
      clearInterval(timer);
    };
  }, []);

  const install = useCallback(async () => {
    if (!update) return;
    let total = 0;
    let got = 0;
    setState({ kind: "installing", percent: null });
    try {
      await update.downloadAndInstall((e) => {
        if (e.event === "Started") total = e.data.contentLength ?? 0;
        if (e.event === "Progress") {
          got += e.data.chunkLength;
          setState({ kind: "installing", percent: total ? Math.round((got / total) * 100) : null });
        }
      });
      await relaunch();
    } catch (e) {
      setState({ kind: "failed", message: String(e) });
    }
  }, [update]);

  return { state, install };
}
