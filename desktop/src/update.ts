// In-app updates: GitHub releases publish a signed latest.json; the updater
// plugin checks it, verifies the signature, swaps the app and we relaunch.
import { useCallback, useEffect, useRef, useState } from "react";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

export type UpdateState =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "latest" }
  | { kind: "check_failed" }
  | { kind: "available"; version: string }
  | { kind: "installing"; percent: number | null }
  | { kind: "failed"; message: string };

const SIX_HOURS = 6 * 60 * 60 * 1000;
const TEN_MINUTES = 10 * 60 * 1000;
/** How long "You're up to date" stays before the button returns. */
const LATEST_NOTE_MS = 4000;

export function useUpdater() {
  const [state, setState] = useState<UpdateState>({ kind: "idle" });
  const update = useRef<Update | null>(null);
  const busy = useRef(false);

  /** Look for an update. `manual` checks report their result; automatic ones stay quiet. */
  const look = useCallback(async (manual: boolean) => {
    if (busy.current) return;
    busy.current = true;
    if (manual) setState({ kind: "checking" });
    try {
      const u = await check();
      update.current = u;
      if (u) setState({ kind: "available", version: u.version });
      else if (manual) {
        setState({ kind: "latest" });
        setTimeout(() => setState((s) => (s.kind === "latest" ? { kind: "idle" } : s)), LATEST_NOTE_MS);
      }
    } catch {
      // Offline, or GitHub unreachable. Automatic checks just try again later.
      if (manual) setState({ kind: "check_failed" });
    } finally {
      busy.current = false;
    }
  }, []);

  useEffect(() => {
    void look(false);
    const timer = setInterval(() => void look(false), SIX_HOURS);
    // Also look whenever the window comes back to the front (at most every
    // 10 minutes), so a new release shows up without restarting the app.
    let last = Date.now();
    const onFocus = () => {
      if (Date.now() - last < TEN_MINUTES) return;
      last = Date.now();
      void look(false);
    };
    window.addEventListener("focus", onFocus);
    return () => {
      clearInterval(timer);
      window.removeEventListener("focus", onFocus);
    };
  }, [look]);

  const install = useCallback(async () => {
    const u = update.current;
    if (!u) return;
    let total = 0;
    let got = 0;
    setState({ kind: "installing", percent: null });
    try {
      await u.downloadAndInstall((e) => {
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
  }, []);

  const checkNow = useCallback(() => look(true), [look]);

  return { state, install, checkNow };
}
