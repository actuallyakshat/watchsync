import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { vi } from "vitest";
import type { Snapshot } from "./api";
import { EMPTY } from "./api";

// --- Tauri, mocked ---------------------------------------------------------
const sent: unknown[] = [];
let emit: (s: Snapshot) => void = () => {};
let setup = { installed: true, has_brew: true };
let pendingUpdate: { version: string; downloadAndInstall: () => Promise<void> } | null = null;

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (cmd: string, args?: { cmd: unknown }) => {
    if (cmd === "send") sent.push(args!.cmd);
    if (cmd === "player_setup") return setup;
    return undefined;
  }),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (_: string, f: (e: { payload: Snapshot }) => void) => {
    emit = (s) => f({ payload: s });
    return () => {};
  }),
}));
vi.mock("@tauri-apps/api/app", () => ({ getVersion: async () => "0.2.0" }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(async () => "/Movies/film.mkv"), ask: vi.fn(async () => true) }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn(async () => {}) }));
vi.mock("@tauri-apps/plugin-updater", () => ({ check: vi.fn(async () => pendingUpdate) }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: vi.fn(async () => {}) }));

const { App } = await import("./App");

const room: Snapshot = {
  ...EMPTY,
  name: "Akshat",
  phase: "room",
  is_host: true,
  connected: true,
  code: "abc123",
  file_name: "The Odyssey 2026 1080p BluRay HEVC x265 5.1 BONE.mkv",
  player_open: true,
  position: 3733,
  duration: 10355,
  last_action: "Riya paused at 1:02:13",
  peers: [
    { name: "Akshat", is_host: true, file: "match", drift_ms: 0, version: "0.2.0", outdated: false },
    { name: "Riya", is_host: false, file: "match", drift_ms: 12, version: "0.2.0", outdated: false },
    { name: "Aman", is_host: false, file: "mismatch", drift_ms: 30, version: "0.2.0", outdated: false },
    { name: "Kabir", is_host: false, file: "no_file", drift_ms: null, version: "", outdated: true },
  ],
  my_file: "match",
};

beforeEach(() => {
  sent.length = 0;
  setup = { installed: true, has_brew: true };
  pendingUpdate = null;
});

test("home: host a room, or join with a pasted code", async () => {
  render(<App />);
  expect(await screen.findByText("Movie night, from separate couches.")).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Host a room" }));
  expect(sent).toContainEqual({ type: "host" });

  await userEvent.type(screen.getByLabelText("Invite code"), "  7b22696422  ");
  await userEvent.click(screen.getByRole("button", { name: "Join" }));
  expect(sent).toContainEqual({ type: "join", code: "7b22696422" });
});

test("room: readable title, people with file status, shared crop", async () => {
  render(<App />);
  await act(async () => emit(room));

  expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("The Odyssey 2026");
  expect(screen.getByText("Riya paused at 1:02:13")).toBeInTheDocument();
  expect(screen.getByText("1:02:13")).toBeInTheDocument();
  expect(screen.getByText("2:52:35")).toBeInTheDocument();

  // People: host, in sync, wrong file, outdated app
  expect(screen.getByText("host")).toBeInTheDocument();
  expect(screen.getByText("in sync")).toBeInTheDocument();
  expect(screen.getByText("different file")).toBeInTheDocument();
  expect(screen.getByText("needs update")).toBeInTheDocument();

  await userEvent.click(screen.getByRole("radio", { name: /2\.39:1/ }));
  expect(sent).toContainEqual({ type: "set_crop", crop: "scope" });
  await userEvent.click(screen.getByRole("button", { name: "Play for everyone" }));
  expect(sent).toContainEqual({ type: "toggle_play" });
});

test("keyboard: space and arrows control playback for everyone", async () => {
  render(<App />);
  await act(async () => emit(room));
  await userEvent.keyboard(" ");
  await userEvent.keyboard("{ArrowRight}");
  expect(sent).toEqual([{ type: "toggle_play" }, { type: "seek_by", seconds: 10 }]);
});

test("guest sees whether their copy matches the host's", async () => {
  render(<App />);
  await act(async () => emit({ ...room, is_host: false, name: "Riya", my_file: "mismatch" }));
  expect(screen.getByText("Different file from the host")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: /Leave room/ })).toBeInTheDocument();
});

test("shows player setup until mpv is installed", async () => {
  setup = { installed: false, has_brew: false };
  render(<App />);
  expect(await screen.findByText("One more thing.")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Copy command" })).toBeInTheDocument();
});

test("offers an available update", async () => {
  const install = vi.fn(async () => {});
  pendingUpdate = { version: "0.3.0", downloadAndInstall: install };
  render(<App />);
  const button = await screen.findByRole("button", { name: /Update to 0\.3\.0/ });
  await userEvent.click(button);
  await waitFor(() => expect(install).toHaveBeenCalled());
});
