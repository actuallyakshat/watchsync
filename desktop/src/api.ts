// The bridge to the Rust engine: commands go out with `send`, the engine's
// state comes back as `snapshot` events.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Phase = "home" | "connecting" | "room";
export type FileStatus = "no_file" | "match" | "mismatch" | "unknown";
export type Crop =
  | "original"
  | "classic"
  | "imax_143"
  | "euro_166"
  | "wide"
  | "flat"
  | "imax_190"
  | "univisium_200"
  | "todd_ao_220"
  | "scope"
  | "ultra_276";

export interface Person {
  name: string;
  is_host: boolean;
  file: FileStatus;
  drift_ms: number | null;
  version: string;
  outdated: boolean;
}

export interface Snapshot {
  name: string;
  phase: Phase;
  is_host: boolean;
  connected: boolean;
  code: string | null;
  file_name: string | null;
  player_open: boolean;
  playing: boolean;
  position: number;
  duration: number;
  last_action: string | null;
  peers: Person[];
  error: string | null;
  my_drift_ms: number | null;
  my_file: FileStatus;
  crop: Crop;
  crop_by: string | null;
  crop_applied: boolean;
}

export type Command =
  | { type: "host" }
  | { type: "join"; code: string }
  | { type: "open_file"; path: string }
  | { type: "toggle_play" }
  | { type: "seek_by"; seconds: number }
  | { type: "seek_to"; position: number }
  | { type: "set_crop"; crop: Crop }
  | { type: "set_name"; name: string }
  | { type: "leave" };

export interface PlayerSetup {
  installed: boolean;
  has_brew: boolean;
}

export const send = (cmd: Command) => invoke<void>("send", { cmd });
export const playerSetup = () => invoke<PlayerSetup>("player_setup");
export const installPlayer = () => invoke<void>("install_player");
export const onSnapshot = (f: (s: Snapshot) => void): Promise<UnlistenFn> =>
  listen<Snapshot>("snapshot", (e) => f(e.payload));

export const EMPTY: Snapshot = {
  name: "",
  phase: "home",
  is_host: false,
  connected: false,
  code: null,
  file_name: null,
  player_open: false,
  playing: false,
  position: 0,
  duration: 0,
  last_action: null,
  peers: [],
  error: null,
  my_drift_ms: null,
  my_file: "no_file",
  crop: "original",
  crop_by: null,
  crop_applied: false,
};
