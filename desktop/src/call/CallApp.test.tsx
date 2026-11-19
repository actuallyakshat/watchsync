import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { vi } from "vitest";
import { EMPTY, type Snapshot } from "../api";
import { FakePC, FakeStream, FakeTrack, installFakeRtc } from "../test/fakeRtc";
import { CALL_KEY } from "./prefs";

// --- Tauri, mocked ---------------------------------------------------------
const sent: { type: string; [k: string]: unknown }[] = [];
/** What each share asked macOS's picker for, in order. */
const chosen: string[] = [];
let speakers: boolean | null = false;
const listeners = new Map<string, (e: { payload: unknown }) => void>();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (cmd: string, args?: { cmd: { type: string }; kind?: string }) => {
    if (cmd === "send") sent.push(args!.cmd);
    if (cmd === "choose_share") chosen.push(args!.kind!);
    if (cmd === "output_is_speakers") return speakers;
  }),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (name: string, f: (e: { payload: unknown }) => void) => {
    listeners.set(name, f);
    return () => listeners.delete(name);
  }),
}));
const sizes: [number, number][] = [];
let resized: (w: number, h: number) => void = () => {};
vi.mock("./win", () => ({
  setWindowSize: vi.fn(async (w: number, h: number) => void sizes.push([w, h])),
  startResize: vi.fn(),
  onResized: vi.fn(async (f: (w: number, h: number) => void) => {
    resized = f;
    return () => {};
  }),
  restorePosition: vi.fn(async () => {}),
  rememberPosition: vi.fn(async () => () => {}),
}));

const getUserMedia = vi.fn(async (c: MediaStreamConstraints) => new FakeStream([new FakeTrack(c.audio ? "audio" : "video")]));

const getDisplayMedia = vi.fn(async (_c: DisplayMediaStreamOptions) => new FakeStream([new FakeTrack("video")]));

const { CallApp } = await import("./CallApp");

const room: Snapshot = {
  ...EMPTY,
  name: "Akshat",
  phase: "room",
  my_id: "a",
  player_open: true,
  peers: [
    { name: "Akshat", is_host: true, file: "match", drift_ms: 0, version: "0.5.0", outdated: false, id: "a", mic: false, cam: false },
    { name: "Riya", is_host: false, file: "match", drift_ms: 0, version: "0.5.0", outdated: false, id: "b", mic: true, cam: false },
  ],
};

const emit = (s: Snapshot) => act(() => listeners.get("snapshot")!({ payload: s }));
/** The room with Riya's camera on or off, and the movie fullscreen or not. */
const riya = (cam: boolean, player_fullscreen = false): Snapshot => ({
  ...room,
  player_fullscreen,
  peers: room.peers.map((p) => (p.id === "b" ? { ...p, cam } : p)),
});

beforeEach(() => {
  installFakeRtc();
  sent.length = 0;
  chosen.length = 0;
  sizes.length = 0;
  getDisplayMedia.mockClear();
  localStorage.clear();
  getUserMedia.mockClear();
  speakers = false;
  Object.defineProperty(navigator, "mediaDevices", { value: { getUserMedia, getDisplayMedia }, configurable: true });
  HTMLMediaElement.prototype.play = vi.fn(async () => {});
});

afterEach(() => vi.unstubAllGlobals());

async function start(s: Snapshot = room) {
  render(<CallApp />);
  await waitFor(() => expect(listeners.has("snapshot")).toBe(true));
  emit(s);
}

test("joining a room connects to everyone, mic and camera off the first time", async () => {
  await start();
  await waitFor(() => expect(FakePC.all).toHaveLength(1));
  expect(getUserMedia).not.toHaveBeenCalled();
  expect(sent).toContainEqual({ type: "set_call", mic: false, cam: false, screen: false });
  expect(screen.getByTitle("Riya")).toBeInTheDocument(); // in the strip
});

test("turning the mic on asks for it with echo cancellation and tells the room", async () => {
  await start();
  await userEvent.click(screen.getByRole("button", { name: "Turn your mic on" }));
  expect(getUserMedia).toHaveBeenCalledWith({ audio: expect.objectContaining({ echoCancellation: true, noiseSuppression: true }) });
  await waitFor(() => expect(sent).toContainEqual({ type: "set_call", mic: true, cam: false, screen: false }));
  expect(sent).toContainEqual({ type: "notify", text: "Mic on" });
  expect(JSON.parse(localStorage.getItem(CALL_KEY)!).mic).toBe(true);
});

test("the next room starts with the mic the way you left it, and leaving lets go of it", async () => {
  localStorage.setItem(CALL_KEY, JSON.stringify({ mic: true, cam: false }));
  await start();
  await waitFor(() => expect(getUserMedia).toHaveBeenCalledTimes(1));
  const track = (await getUserMedia.mock.results[0].value).getTracks()[0] as FakeTrack;
  emit({ ...EMPTY });
  await waitFor(() => expect(track.readyState).toBe("ended"));
  expect(FakePC.all[0].closed).toBe(true);
});

test("the camera starts off in every room, even if it was on last time", async () => {
  localStorage.setItem(CALL_KEY, JSON.stringify({ mic: false, cam: true }));
  await start();
  await waitFor(() => expect(FakePC.all).toHaveLength(1));
  expect(getUserMedia).not.toHaveBeenCalled();
  expect(sent).toContainEqual({ type: "set_call", mic: false, cam: false, screen: false });
  expect(screen.getByRole("button", { name: "Turn your camera on" })).toBeInTheDocument();
});

test("a blocked camera says where to allow it", async () => {
  getUserMedia.mockRejectedValueOnce(new DOMException("no", "NotAllowedError"));
  await start();
  await userEvent.click(screen.getByRole("button", { name: "Turn your camera on" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Privacy & Security → Camera");
  expect(sent).not.toContainEqual(expect.objectContaining({ type: "set_call", cam: true }));
});

test("a friend's camera coming on brings the call up", async () => {
  await start();
  const pill = sizes.at(-1)!;
  expect(pill[1]).toBeLessThan(60);
  expect(screen.getByRole("button", { name: "Expand the call" })).toHaveAttribute("title", "Expand");
  emit(riya(true));
  await waitFor(() => expect(sizes.at(-1)).toEqual([320, 200]));
  await userEvent.click(screen.getByRole("button", { name: "Shrink the call" }));
  await waitFor(() => expect(sizes.at(-1)).toEqual(pill));
  // The strip says there's video to see.
  expect(screen.getByTitle("Riya (camera on)")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Expand the call" })).toHaveAttribute("title", "Expand to see video");
});

test("once you shrink the call, only going fullscreen brings it back up", async () => {
  await start(riya(true));
  await userEvent.click(await screen.findByRole("button", { name: "Shrink the call" }));
  emit(riya(false));
  emit(riya(true));
  expect(screen.queryByRole("button", { name: "Shrink the call" })).toBeNull();
  emit(riya(true, true));
  expect(await screen.findByRole("button", { name: "Shrink the call" })).toBeInTheDocument();
});

test("dragging the strip bigger shows the video at that size", async () => {
  await start();
  act(() => resized(480, 300));
  expect(await screen.findByRole("button", { name: "Shrink the call" })).toBeInTheDocument();
  await waitFor(() => expect(sizes.at(-1)).toEqual([480, 300]));
});

test("the movie's keys work from the call window", async () => {
  await start();
  await userEvent.keyboard(" ");
  await userEvent.keyboard("{ArrowRight}");
  await userEvent.keyboard("f");
  expect(sent).toContainEqual({ type: "toggle_play" });
  expect(sent).toContainEqual({ type: "seek_by", seconds: 10 });
  expect(sent).toContainEqual({ type: "toggle_fullscreen" });
});

test("signalling goes to the engine addressed to the right person", async () => {
  await start();
  await waitFor(() => expect(FakePC.all).toHaveLength(1));
  // We have the lower id, so we offer.
  await waitFor(() => expect(sent.some((m) => m.type === "signal" && m.to === "b")).toBe(true));
});

test("friends are asked for video only as big as it's shown, and none while shrunk", async () => {
  // Every observed tile is 320×200 CSS pixels on a 2× screen.
  class RO {
    constructor(private f: (e: { contentRect: { width: number; height: number } }[]) => void) {}
    observe() {
      this.f([{ contentRect: { width: 320, height: 200 } }]);
    }
    disconnect() {}
  }
  vi.stubGlobal("ResizeObserver", RO);
  vi.stubGlobal("devicePixelRatio", 2);
  await start();
  await waitFor(() => expect(FakePC.all).toHaveLength(1));
  const channel = FakePC.all[0].channels[0];
  channel.readyState = "open";
  channel.onopen?.();
  const wants = () => channel.sent.map((m) => JSON.parse(m).want);
  await waitFor(() => expect(wants().at(-1)).toEqual({ w: 0, h: 0 }));

  emit(riya(true)); // expands
  await waitFor(() => expect(wants().at(-1)).toEqual({ w: 640, h: 448 }));
  await userEvent.click(screen.getByRole("button", { name: "Shrink the call" }));
  await waitFor(() => expect(wants().at(-1)).toEqual({ w: 0, h: 0 }));
});

test("the mic shortcut works from any app and says so on the movie", async () => {
  await start();
  await waitFor(() => expect(listeners.has("call-shortcut")).toBe(true));
  act(() => listeners.get("call-shortcut")!({ payload: "mic" }));
  await waitFor(() => expect(sent).toContainEqual({ type: "set_call", mic: true, cam: false, screen: false }));
  expect(sent).toContainEqual({ type: "notify", text: "Mic on" });
  act(() => listeners.get("call-shortcut")!({ payload: "mic" }));
  await waitFor(() => expect(sent).toContainEqual({ type: "notify", text: "Mic muted" }));
});

test("on speakers with the mic on, it asks for headphones", async () => {
  speakers = true;
  await start();
  expect(screen.queryByRole("button", { name: "You're on speakers" })).toBeNull();
  await userEvent.click(screen.getByRole("button", { name: "Turn your mic on" }));
  await userEvent.click(await screen.findByRole("button", { name: "You're on speakers" }));
  expect(await screen.findByRole("status")).toHaveTextContent("Use headphones");
  await userEvent.click(screen.getByRole("button", { name: "Dismiss" }));
  expect(screen.queryByRole("status")).toBeNull();
});

test("a chosen camera is used, and a missing one falls back to the default", async () => {
  localStorage.setItem(CALL_KEY, JSON.stringify({ camId: "usb-cam" }));
  getUserMedia.mockRejectedValueOnce(new DOMException("gone", "OverconstrainedError"));
  await start();
  await userEvent.click(screen.getByRole("button", { name: "Turn your camera on" }));
  await waitFor(() => expect(getUserMedia).toHaveBeenCalledTimes(2));
  expect(getUserMedia.mock.calls[0][0]).toEqual({ video: expect.objectContaining({ deviceId: { exact: "usb-cam" } }) });
  expect(getUserMedia.mock.calls[1][0]).toEqual({ video: expect.not.objectContaining({ deviceId: expect.anything() }) });
  await waitFor(() => expect(sent).toContainEqual({ type: "set_call", mic: false, cam: true, screen: false }));
});

test("voices and each friend have their own volume, remembered", async () => {
  await start(riya(true));
  await userEvent.click(await screen.findByRole("button", { name: "Call settings" }));
  fireEvent.change(screen.getByRole("slider", { name: "Voices volume" }), { target: { value: "0.9" } });
  expect(JSON.parse(localStorage.getItem(CALL_KEY)!).voices).toBeCloseTo(0.9);
  fireEvent.change(screen.getByRole("slider", { name: "Riya's volume" }), { target: { value: "0" } });
  expect(JSON.parse(localStorage.getItem(CALL_KEY)!).volumes).toEqual({ Riya: 0 });
  fireEvent.change(screen.getByRole("slider", { name: "Riya's volume" }), { target: { value: "0.5" } });
  await waitFor(() => expect(document.querySelector("audio")!.volume).toBeCloseTo(0.45));
});

test("the call can relay through iroh, on its own or when forced, and says when it does", async () => {
  const turn = { urls: "turn:127.0.0.1:5000?transport=udp", username: "u", credential: "p" };
  await start({ ...riya(true), turn });
  await waitFor(() => expect(FakePC.all).toHaveLength(1));
  const servers = FakePC.all[0].config.iceServers!;
  expect(servers).toContainEqual(turn);
  expect(FakePC.all[0].config.iceTransportPolicy).toBe("all");

  await userEvent.click(await screen.findByRole("button", { name: "Call settings" }));
  await userEvent.click(screen.getByText("Troubleshooting"));
  await userEvent.click(screen.getByRole("checkbox", { name: "Always relay the call through iroh" }));
  await waitFor(() => expect(FakePC.all.at(-1)!.config.iceTransportPolicy).toBe("relay"));
  expect(FakePC.all[0].closed).toBe(true);
  expect(JSON.parse(localStorage.getItem(CALL_KEY)!).forceRelay).toBe(true);

  FakePC.candidateType = "relay";
  FakePC.all.at(-1)!.connectionState = "connected";
  expect(await screen.findByText("via iroh", {}, { timeout: 4000 })).toBeInTheDocument();
  expect(screen.getByText("Riya: relayed through iroh")).toBeInTheDocument();
});

test("the mic is cleaned up out of the box, and can be tuned", async () => {
  await start();
  await userEvent.click(screen.getByRole("button", { name: "Turn your mic on" }));
  // No settings needed: system noise suppression and levelling are on.
  expect(getUserMedia).toHaveBeenLastCalledWith({
    audio: expect.objectContaining({ echoCancellation: true, noiseSuppression: true, autoGainControl: true }),
  });
  const mic = (await getUserMedia.mock.results[0].value).getTracks()[0] as FakeTrack;

  await userEvent.click(await screen.findByRole("button", { name: "Expand the call" }));
  await userEvent.click(screen.getByRole("button", { name: "Call settings" }));
  expect(screen.getByRole("checkbox", { name: "Noise suppression" })).toBeChecked();
  expect(screen.getByRole("checkbox", { name: "Set sensitivity automatically" })).toBeChecked();
  expect(screen.queryByRole("slider", { name: "Mic sensitivity" })).toBeNull();

  // Changes apply to the live mic and are remembered.
  await userEvent.click(screen.getByRole("checkbox", { name: "Adjust mic volume automatically" }));
  expect(mic.constraints.at(-1)).toMatchObject({ autoGainControl: false, noiseSuppression: true });
  fireEvent.change(screen.getByRole("slider", { name: "Mic input volume" }), { target: { value: "1.5" } });
  await userEvent.click(screen.getByRole("checkbox", { name: "Set sensitivity automatically" }));
  fireEvent.change(screen.getByRole("slider", { name: "Mic sensitivity" }), { target: { value: "-30" } });
  expect(JSON.parse(localStorage.getItem(CALL_KEY)!).micInput).toEqual({
    suppress: true,
    auto: false,
    thresholdDb: -65,
    volume: 1.5,
    autoGain: false,
  });
  await userEvent.click(screen.getByRole("checkbox", { name: "Noise suppression" }));
  expect(mic.constraints.at(-1)).toMatchObject({ noiseSuppression: false });
  expect(screen.queryByRole("checkbox", { name: "Set sensitivity automatically" })).toBeNull();
});

/** A pretend Web Audio engine that can stop, like macOS stops it on an
 * output change (plugging in headphones). */
class FakeAudio {
  static last: FakeAudio;
  state: string = "running";
  resumesTo = "running";
  onstatechange: (() => void) | null = null;
  out = new FakeTrack("audio");
  closed = false;
  audioWorklet = { addModule: async () => {} };
  currentTime = 0;
  constructor() {
    FakeAudio.last = this;
  }
  async resume() {
    this.state = this.resumesTo;
  }
  async close() {
    this.closed = true;
  }
  private node = () => ({ connect: (n: unknown) => n, disconnect: () => {} });
  createMediaStreamSource = this.node;
  createBiquadFilter = () => ({ ...this.node(), type: "", frequency: { value: 0 } });
  createGain = () => ({ ...this.node(), gain: { setTargetAtTime: () => {} } });
  createMediaStreamDestination = () => ({ ...this.node(), stream: new FakeStream([this.out]) });
  /** The engine stops (or starts again). */
  set(state: string) {
    this.state = state;
    this.onstatechange?.();
  }
}
class FakeWorkletNode {
  port = { postMessage: () => {}, onmessage: null };
  connect = (n: unknown) => n;
}

test("the processed mic is sent, and the plain mic while processing is stopped", async () => {
  vi.stubGlobal("AudioContext", FakeAudio);
  vi.stubGlobal("AudioWorkletNode", FakeWorkletNode);
  await start();
  await waitFor(() => expect(FakePC.all).toHaveLength(1));
  const sender = () => FakePC.all[0].transceivers[0].sender.track;
  await userEvent.click(screen.getByRole("button", { name: "Turn your mic on" }));
  const audio = FakeAudio.last;
  const raw = (await getUserMedia.mock.results[0].value).getTracks()[0] as FakeTrack;
  await waitFor(() => expect(sender()).toBe(audio.out));

  // Headphones plugged in: the engine stops and won't restart at once.
  audio.resumesTo = "suspended";
  act(() => audio.set("suspended"));
  await waitFor(() => expect(sender()).toBe(raw), { timeout: 2000 });
  await userEvent.click(screen.getByRole("button", { name: "Expand the call" }));
  await userEvent.click(screen.getByRole("button", { name: "Call settings" }));
  expect(await screen.findByText(/sending your mic as it is/)).toBeInTheDocument();

  // It comes back: processing again.
  act(() => audio.set("running"));
  await waitFor(() => expect(sender()).toBe(audio.out));
  expect(await screen.findByText("Quiet: nothing sent")).toBeInTheDocument();

  // Mic off lets go of everything.
  await userEvent.click(screen.getByRole("button", { name: "Mute your mic" }));
  await waitFor(() => expect(raw.readyState).toBe("ended"));
  expect(audio.out.readyState).toBe("ended");
  expect(audio.closed).toBe(true);
  expect(sender()).toBeNull();
});

/** The room with Riya sharing her screen (or not). */
const riyaShares = (on: boolean): Snapshot => ({
  ...room,
  peers: room.peers.map((p) => (p.id === "b" ? { ...p, screen: on } : p)),
});

test("sharing a window: pick it from the menu, then macOS's window picker", async () => {
  await start();
  // From the strip, the call opens up to ask what to share.
  await userEvent.click(screen.getByRole("button", { name: "Share your screen" }));
  expect(await screen.findByRole("menu", { name: "Share" })).toBeInTheDocument();
  await userEvent.click(screen.getByRole("menuitem", { name: /A window/ }));

  await waitFor(() => expect(sent).toContainEqual({ type: "set_call", mic: false, cam: false, screen: true }));
  expect(chosen).toEqual(["window"]);
  expect(getDisplayMedia).toHaveBeenCalledWith({ video: expect.objectContaining({ frameRate: { max: 30 } }), audio: false });
  const track = (await getDisplayMedia.mock.results[0].value).getTracks()[0] as FakeTrack & { contentHint?: string };
  expect(track.contentHint).toBe("detail");
  // It goes out on the screen's own transceiver, next to the camera's.
  expect(FakePC.all[0].transceivers[2].sender.track).toBe(track);
  expect(sent).toContainEqual({ type: "notify", text: "Sharing a window" });
  expect(screen.getByRole("status")).toHaveTextContent("You're sharing your screen");

  await userEvent.click(screen.getByRole("button", { name: "Stop sharing" }));
  expect(track.readyState).toBe("ended");
  expect(FakePC.all[0].transceivers[2].sender.track).toBe(null);
  await waitFor(() => expect(sent.filter((c) => c.type === "set_call").at(-1)).toEqual({ type: "set_call", mic: false, cam: false, screen: false }));
});

test("the entire screen opens macOS's screen picker", async () => {
  await start();
  await userEvent.click(screen.getByRole("button", { name: "Share your screen" }));
  await userEvent.click(await screen.findByRole("menuitem", { name: /Your entire screen/ }));
  await waitFor(() => expect(sent).toContainEqual(expect.objectContaining({ type: "set_call", screen: true })));
  expect(chosen).toEqual(["screen"]);
  expect(sent).toContainEqual({ type: "notify", text: "Sharing your screen" });
});

test("closing macOS's picker isn't an error", async () => {
  getDisplayMedia.mockImplementationOnce(async () => {
    await new Promise((r) => setTimeout(r, 700)); // a person looked, then closed it
    throw new DOMException("cancelled", "NotAllowedError");
  });
  await start();
  await userEvent.click(screen.getByRole("button", { name: "Share your screen" }));
  await userEvent.click(await screen.findByRole("menuitem", { name: /A window/ }));
  await waitFor(() => expect(getDisplayMedia).toHaveBeenCalled());
  expect(screen.queryByRole("alert")).toBeNull();
  expect(screen.queryByRole("menu")).toBeNull();
  expect(sent).not.toContainEqual(expect.objectContaining({ type: "set_call", screen: true }));
});

test("stopping from macOS's menu bar, or leaving the room, ends the share", async () => {
  await start();
  await userEvent.click(screen.getByRole("button", { name: "Share your screen" }));
  await userEvent.click(await screen.findByRole("menuitem", { name: /A window/ }));
  await waitFor(() => expect(sent).toContainEqual(expect.objectContaining({ type: "set_call", screen: true })));
  const first = (await getDisplayMedia.mock.results[0].value).getTracks()[0] as FakeTrack;
  act(() => first.onended!());
  await waitFor(() => expect(sent.filter((c) => c.type === "set_call").at(-1)).toMatchObject({ screen: false }));
  expect(screen.getByRole("button", { name: "Share your screen" })).toBeInTheDocument();

  await userEvent.click(screen.getByRole("button", { name: "Share your screen" }));
  await userEvent.click(await screen.findByRole("menuitem", { name: /A window/ }));
  await waitFor(() => expect(getDisplayMedia).toHaveBeenCalledTimes(2));
  const second = (await getDisplayMedia.mock.results[1].value).getTracks()[0] as FakeTrack;
  await waitFor(() => expect(screen.getByRole("button", { name: "Stop sharing your screen" })).toBeInTheDocument());
  emit({ ...EMPTY });
  await waitFor(() => expect(second.readyState).toBe("ended"));
});

test("a friend sharing brings the call up, big enough to read, with their screen on the big spot", async () => {
  await start();
  const pill = sizes.at(-1)!;
  expect(pill[1]).toBeLessThan(60);
  emit(riyaShares(true));
  await waitFor(() => expect(sizes.at(-1)).toEqual([640, 400]));
  expect(await screen.findByText("Riya's screen")).toBeInTheDocument();
  // Their screen is shown whole.
  expect(document.querySelector(".tile.screen")).not.toBeNull();

  // In the grid it's a tile of its own; clicking it puts it back on the big spot.
  await userEvent.click(screen.getByRole("button", { name: "Grid view" }));
  expect(screen.getByText("Riya's screen")).toBeInTheDocument();
  expect(screen.getByText("Riya")).toBeInTheDocument();
  await userEvent.click(screen.getByText("Riya's screen").closest(".tile")!);
  expect(screen.getByRole("button", { name: "Unpin" })).toBeInTheDocument();

  emit(riyaShares(false));
  await waitFor(() => expect(screen.queryByText("Riya's screen")).toBeNull());
});

test("when macOS refuses outright, it says where to allow screen sharing", async () => {
  getDisplayMedia.mockRejectedValueOnce(new DOMException("denied", "NotAllowedError"));
  await start();
  await userEvent.click(screen.getByRole("button", { name: "Share your screen" }));
  await userEvent.click(await screen.findByRole("menuitem", { name: /Your entire screen/ }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Screen & System Audio Recording");
});
