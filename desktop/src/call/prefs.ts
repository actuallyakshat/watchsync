import { DEFAULT_MIC, type MicSettings } from "./micChain";

// Call settings remembered on this Mac. The mic starts off the first time;
// after that a room starts with it the way you last left it. The camera
// always starts off (`cam` is kept, but not used to turn it on).
export interface CallPrefs {
  mic: boolean;
  cam: boolean;
  /** Your own camera in the corner of the speaker view. */
  selfView: boolean;
  layout: "speaker" | "grid";
  /** Size of the expanded call window (logical pixels). */
  size: { width: number; height: number };
  /** Everyone's voices (0–1), and per person, by name. */
  voices: number;
  volumes: Record<string, number>;
  /** Chosen devices ("" = the system default). */
  micId: string;
  camId: string;
  speakerId: string;
  /** Troubleshooting: always relay through iroh, even when direct works. */
  forceRelay: boolean;
  /** How your mic is cleaned up before it's sent. */
  micInput: MicSettings;
}

export const CALL_KEY = "watchsync.call";

export const DEFAULT_PREFS: CallPrefs = {
  mic: false,
  cam: false,
  selfView: false,
  layout: "speaker",
  size: { width: 320, height: 200 },
  voices: 1,
  volumes: {},
  micId: "",
  camId: "",
  speakerId: "",
  forceRelay: false,
  micInput: DEFAULT_MIC,
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
      voices: volume(p.voices),
      volumes: Object.fromEntries(
        Object.entries(p.volumes && typeof p.volumes === "object" ? p.volumes : {}).map(([k, v]) => [k, volume(v)]),
      ),
      micId: typeof p.micId === "string" ? p.micId : "",
      camId: typeof p.camId === "string" ? p.camId : "",
      speakerId: typeof p.speakerId === "string" ? p.speakerId : "",
      forceRelay: p.forceRelay === true,
      micInput: micSettings(p.micInput),
    };
  } catch {
    return DEFAULT_PREFS;
  }
}

export function savePrefs(p: CallPrefs) {
  localStorage.setItem(CALL_KEY, JSON.stringify(p));
}

function micSettings(m: unknown): MicSettings {
  if (!m || typeof m !== "object") return DEFAULT_MIC;
  const o = m as Record<string, unknown>;
  const bool = (k: keyof MicSettings) => (typeof o[k] === "boolean" ? (o[k] as boolean) : (DEFAULT_MIC[k] as boolean));
  const num = (k: keyof MicSettings, lo: number, hi: number) =>
    typeof o[k] === "number" && Number.isFinite(o[k]) ? clamp(o[k] as number, lo, hi) : (DEFAULT_MIC[k] as number);
  return {
    suppress: bool("suppress"),
    auto: bool("auto"),
    thresholdDb: num("thresholdDb", -80, -10),
    volume: num("volume", 0, 2),
    autoGain: bool("autoGain"),
  };
}

const volume = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? clamp(v, 0, 1) : 1);
const clamp = (n: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, n));
