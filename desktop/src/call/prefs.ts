// Call settings remembered on this Mac. Mic and camera start off the first
// time; after that a room starts the way you last left them.
export interface CallPrefs {
  mic: boolean;
  cam: boolean;
  /** Your own camera in the corner of the speaker view. */
  selfView: boolean;
  layout: "speaker" | "grid";
  /** Size of the expanded call window (logical pixels). */
  size: { width: number; height: number };
}

export const CALL_KEY = "watchsync.call";

export const DEFAULT_PREFS: CallPrefs = {
  mic: false,
  cam: false,
  selfView: false,
  layout: "speaker",
  size: { width: 320, height: 200 },
};

export function loadPrefs(): CallPrefs {
  try {
    const p = JSON.parse(localStorage.getItem(CALL_KEY) ?? "null");
    if (!p || typeof p !== "object") return DEFAULT_PREFS;
    const size =
      p.size && Number.isFinite(p.size.width) && Number.isFinite(p.size.height)
        ? { width: clamp(p.size.width, 160, 4000), height: clamp(p.size.height, 100, 3000) }
        : DEFAULT_PREFS.size;
    return {
      mic: p.mic === true,
      cam: p.cam === true,
      selfView: p.selfView === true,
      layout: p.layout === "grid" ? "grid" : "speaker",
      size,
    };
  } catch {
    return DEFAULT_PREFS;
  }
}

export function savePrefs(p: CallPrefs) {
  localStorage.setItem(CALL_KEY, JSON.stringify(p));
}

const clamp = (n: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, n));
