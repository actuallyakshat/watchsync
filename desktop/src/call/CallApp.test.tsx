import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { vi } from "vitest";
import { EMPTY, type Snapshot } from "../api";
import { FakePC, FakeStream, FakeTrack, installFakeRtc } from "../test/fakeRtc";
import { CALL_KEY } from "./prefs";

// --- Tauri, mocked ---------------------------------------------------------
const sent: { type: string; [k: string]: unknown }[] = [];
const listeners = new Map<string, (e: { payload: unknown }) => void>();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (cmd: string, args?: { cmd: { type: string } }) => {
    if (cmd === "send") sent.push(args!.cmd);
  }),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (name: string, f: (e: { payload: unknown }) => void) => {
    listeners.set(name, f);
    return () => listeners.delete(name);
  }),
}));
const sizes: [number, number][] = [];
vi.mock("./win", () => ({
  setWindowSize: vi.fn(async (w: number, h: number) => void sizes.push([w, h])),
  startResize: vi.fn(),
  onResized: vi.fn(async () => () => {}),
}));

const getUserMedia = vi.fn(async (c: MediaStreamConstraints) => new FakeStream([new FakeTrack(c.audio ? "audio" : "video")]));

const { CallApp } = await import("./CallApp");

const room: Snapshot = {
  ...EMPTY,
  name: "Akshat",
  phase: "room",
  my_id: "a",
  player_open: true,
  peers: [
    { name: "Akshat", is_host: true, file: "match", drift_ms: 0, version: "0.5.0", outdated: false, id: "a", mic: false, cam: false },
    { name: "Riya", is_host: false, file: "match", drift_ms: 0, version: "0.5.0", outdated: false, id: "b", mic: true, cam: true },
  ],
};

const emit = (s: Snapshot) => act(() => listeners.get("snapshot")!({ payload: s }));

beforeEach(() => {
  installFakeRtc();
  sent.length = 0;
  sizes.length = 0;
  localStorage.clear();
  getUserMedia.mockClear();
  Object.defineProperty(navigator, "mediaDevices", { value: { getUserMedia }, configurable: true });
  HTMLMediaElement.prototype.play = vi.fn(async () => {});
});

async function start(s: Snapshot = room) {
  render(<CallApp />);
  await waitFor(() => expect(listeners.has("snapshot")).toBe(true));
  emit(s);
}

test("joining a room connects to everyone, mic and camera off the first time", async () => {
  await start();
  await waitFor(() => expect(FakePC.all).toHaveLength(1));
  expect(getUserMedia).not.toHaveBeenCalled();
  expect(sent).toContainEqual({ type: "set_call", mic: false, cam: false });
  expect(screen.getByTitle("Riya")).toBeInTheDocument(); // in the strip
});

test("turning the mic on asks for it with echo cancellation and tells the room", async () => {
  await start();
  await userEvent.click(screen.getByRole("button", { name: "Turn your mic on" }));
  expect(getUserMedia).toHaveBeenCalledWith({ audio: expect.objectContaining({ echoCancellation: true, noiseSuppression: true }) });
  await waitFor(() => expect(sent).toContainEqual({ type: "set_call", mic: true, cam: false }));
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

test("a blocked camera says where to allow it", async () => {
  getUserMedia.mockRejectedValueOnce(new DOMException("no", "NotAllowedError"));
  await start();
  await userEvent.click(screen.getByRole("button", { name: "Turn your camera on" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Privacy & Security → Camera");
  expect(sent).not.toContainEqual(expect.objectContaining({ type: "set_call", cam: true }));
});

test("going fullscreen with a friend's camera on brings the call up", async () => {
  await start();
  const pill = sizes.at(-1)!;
  emit({ ...room, player_fullscreen: true });
  await waitFor(() => expect(sizes.at(-1)).toEqual([320, 200]));
  expect(pill[1]).toBeLessThan(60);
  await userEvent.click(screen.getByRole("button", { name: "Shrink the call" }));
  await waitFor(() => expect(sizes.at(-1)).toEqual(pill));
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
