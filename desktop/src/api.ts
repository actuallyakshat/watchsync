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

export type Reaction = "laugh" | "wow" | "love" | "fire" | "clap" | "sad";
export type TrackKind = "audio" | "subtitle";

export interface ChatLine {
  from: string;
  text: string | null;
  reaction: Reaction | null;
  mine: boolean;
  at_ms: number;
}

export interface Track {
  id: number;
  kind: TrackKind;
  title: string | null;
  lang: string | null;
  selected: boolean;
  channels: number | null;
  codec: string | null;
}

export interface Comfort {
  level_volume: boolean;
  boost_dialogue: boolean;
}

export interface Offer {
  offer_id: string;
  from_id: string;
  from: string;
  name: string;
  size: number;
  fingerprint: string;
}

export interface Transfer {
  id: number;
  direction: "send" | "receive";
  peer: string;
  file: string;
  size: number;
  progress: { percent: number; amount: string | null; speed: string | null; eta: string | null };
  state: "waiting" | "running" | "checking" | "done" | "failed" | "cancelled";
  message: string | null;
  path: string | null;
}

export interface Person {
  name: string;
  is_host: boolean;
  file: FileStatus;
  drift_ms: number | null;
  version: string;
  outdated: boolean;
  /** Endpoint id: their address in the call ("" for apps too old to call). */
  id: string;
  mic: boolean;
  cam: boolean;
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
  reconnecting: boolean;
  reconnects: number;
  chat: ChatLine[];
  tracks: Track[];
  comfort: Comfort;
  invite_link: string | null;
  offers: Offer[];
  sharing: boolean;
  transfers: Transfer[];
  /** The host's movie, found on this Mac by its exact name. */
  found: { path: string; folder: string } | null;
  /** This Mac's endpoint id in the room. */
  my_id: string;
  mic: boolean;
  cam: boolean;
  player_fullscreen: boolean;
}

/** WebRTC signalling from the person with endpoint id `from`. */
export interface SignalIn {
  from: string;
  data: unknown;
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
  | { type: "chat"; text: string }
  | { type: "react"; reaction: Reaction }
  | { type: "set_track"; kind: TrackKind; id: number | null }
  | { type: "set_comfort"; comfort: Comfort }
  | { type: "share_file" }
  | { type: "stop_sharing" }
  | { type: "get_file"; offer_id: string }
  | { type: "cancel_transfer"; id: number }
  | { type: "set_call"; mic: boolean; cam: boolean }
  | { type: "signal"; to: string; data: unknown }
  | { type: "notify"; text: string }
  | { type: "toggle_fullscreen" }
  | { type: "leave" };

export interface PlayerSetup {
  installed: boolean;
  has_brew: boolean;
  /** croc, for sending movies. */
  croc: boolean;
}

export const send = (cmd: Command) => invoke<void>("send", { cmd });
export const playerSetup = () => invoke<PlayerSetup>("player_setup");
export const installPlayer = () => invoke<void>("install_player");
export const installCroc = () => invoke<void>("install_croc");
export const showCall = () => invoke<void>("show_call");
export const onSnapshot = (f: (s: Snapshot) => void): Promise<UnlistenFn> =>
  listen<Snapshot>("snapshot", (e) => f(e.payload));
export const onSignal = (f: (s: SignalIn) => void): Promise<UnlistenFn> =>
  listen<SignalIn>("signal", (e) => f(e.payload));
/** "Show call" was pressed in the main window. */
export const onCallExpand = (f: () => void): Promise<UnlistenFn> => listen("call-expand", () => f());

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
  reconnecting: false,
  reconnects: 0,
  chat: [],
  tracks: [],
  comfort: { level_volume: false, boost_dialogue: false },
  invite_link: null,
  offers: [],
  sharing: false,
  transfers: [],
  found: null,
  my_id: "",
  mic: false,
  cam: false,
  player_fullscreen: false,
};
