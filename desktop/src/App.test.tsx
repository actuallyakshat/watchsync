import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { vi } from "vitest";
import type { Snapshot } from "./api";
import { EMPTY } from "./api";

// --- Tauri, mocked ---------------------------------------------------------
const sent: unknown[] = [];
let shortcutError: string | null = null;
let emit: (s: Snapshot) => void = () => {};
let setup = { installed: true, has_brew: true, croc: true };
let pendingUpdate: { version: string; downloadAndInstall: () => Promise<void> } | null = null;

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (cmd: string, args?: { cmd: unknown }) => {
    if (cmd === "send") sent.push(args!.cmd);
    if (cmd === "player_setup") return setup;
    if (cmd === "set_call_shortcuts" && shortcutError) throw shortcutError;
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
let askAnswer: boolean | Error = true;
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(async () => "/Movies/film.mkv"),
  ask: vi.fn(async () => {
    if (askAnswer instanceof Error) throw askAnswer;
    return askAnswer;
  }),
}));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn(async () => {}) }));
let checkFails = false;
vi.mock("@tauri-apps/plugin-updater", () => ({
  check: vi.fn(async () => {
    if (checkFails) throw new Error("offline");
    return pendingUpdate;
  }),
}));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: vi.fn(async () => {}) }));
let launchUrls: string[] | null = null;
let openUrl: (urls: string[]) => void = () => {};
vi.mock("@tauri-apps/plugin-deep-link", () => ({
  getCurrent: vi.fn(async () => launchUrls),
  onOpenUrl: vi.fn(async (f: (urls: string[]) => void) => {
    openUrl = f;
    return () => {};
  }),
}));
const { writeText } = await import("@tauri-apps/plugin-clipboard-manager");

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
    { name: "Akshat", is_host: true, file: "match", drift_ms: 0, version: "0.2.0", outdated: false, id: "id-akshat", mic: false, cam: false },
    { name: "Riya", is_host: false, file: "match", drift_ms: 12, version: "0.2.0", outdated: false, id: "id-riya", mic: false, cam: false },
    { name: "Aman", is_host: false, file: "mismatch", drift_ms: 30, version: "0.2.0", outdated: false, id: "id-aman", mic: false, cam: false },
    { name: "Kabir", is_host: false, file: "no_file", drift_ms: null, version: "", outdated: true, id: "id-kabir", mic: false, cam: false },
  ],
  my_file: "match",
  invite_link: "https://actuallyakshat.github.io/watchsync/join/#abc123",
  tracks: [
    { id: 1, kind: "audio", title: "Surround", lang: "eng", selected: true, channels: 6, codec: "aac" },
    { id: 2, kind: "audio", title: null, lang: "hin", selected: false, channels: 2, codec: "aac" },
    { id: 1, kind: "subtitle", title: null, lang: "eng", selected: false, channels: null, codec: "subrip" },
    { id: 2, kind: "subtitle", title: "SDH", lang: "eng", selected: false, channels: null, codec: "subrip" },
  ],
};

beforeEach(() => {
  sent.length = 0;
  setup = { installed: true, has_brew: true, croc: true };
  pendingUpdate = null;
  checkFails = false;
  askAnswer = true;
  launchUrls = null;
  localStorage.clear();
});

test("home: host a room, or join with a pasted code", async () => {
  render(<App />);
  expect(await screen.findByText("Movie night, from separate couches.")).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Host a room" }));
  expect(sent).toContainEqual({ type: "host" });

  await userEvent.type(screen.getByLabelText("Invite link"), "  7b22696422  ");
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
  expect(screen.getAllByRole("radio")).toHaveLength(11);
  await userEvent.click(screen.getByRole("radio", { name: "IMAX 1.43:1" }));
  expect(sent).toContainEqual({ type: "set_crop", crop: "imax_143" });
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
  setup = { installed: false, has_brew: false, croc: false };
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

const openSettings = async () => {
  await userEvent.click(await screen.findByRole("button", { name: "Settings" }));
  expect(screen.getByRole("heading", { name: "Settings" })).toBeInTheDocument();
};

test("settings: check for updates, up to date, then a new release appears", async () => {
  render(<App />);
  await openSettings();
  await userEvent.click(screen.getByRole("button", { name: "Check for updates" }));
  expect(await screen.findByText("You're up to date")).toBeInTheDocument();

  pendingUpdate = { version: "0.3.0", downloadAndInstall: vi.fn(async () => {}) };
  await userEvent.click(screen.getByRole("button", { name: "Check for updates" }));
  // Offered in Settings, and announced in the sidebar too.
  expect(await screen.findAllByRole("button", { name: /Update to 0\.3\.0/ })).toHaveLength(2);
});

test("settings: says so when offline, and can retry", async () => {
  checkFails = true;
  render(<App />);
  await openSettings();
  await userEvent.click(screen.getByRole("button", { name: "Check for updates" }));
  expect(await screen.findByText(/Couldn't reach GitHub/)).toBeInTheDocument();
  checkFails = false;
  await userEvent.click(screen.getByRole("button", { name: "Try again" }));
  expect(await screen.findByText("You're up to date")).toBeInTheDocument();
});

test("settings: set a display name, remembered for next time", async () => {
  const { unmount } = render(<App />);
  await act(async () => emit({ ...EMPTY, name: "Akshat Dubey" }));
  await openSettings();
  const input = screen.getByRole("textbox", { name: "Display name" });
  expect(input).toHaveValue("Akshat Dubey");
  expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();

  await userEvent.clear(input);
  await userEvent.type(input, "  Akshat  ");
  await userEvent.click(screen.getByRole("button", { name: "Save" }));
  expect(sent).toContainEqual({ type: "set_name", name: "Akshat" });

  // Next launch: the saved name is sent to the engine straight away.
  unmount();
  sent.length = 0;
  render(<App />);
  await waitFor(() => expect(sent).toContainEqual({ type: "set_name", name: "Akshat" }));
});

test("settings: Back returns to the room", async () => {
  render(<App />);
  await act(async () => emit(room));
  await openSettings();
  await userEvent.click(screen.getByRole("button", { name: /Back to the room/ }));
  expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("The Odyssey");
});

test("End room (host alone) asks first, and still works if the dialog can't be shown", async () => {
  render(<App />);
  await act(async () => emit({ ...room, peers: room.peers.slice(0, 1) }));
  askAnswer = false;
  await userEvent.click(screen.getByRole("button", { name: /End room/ }));
  expect(sent).not.toContainEqual({ type: "leave" });

  askAnswer = true;
  await userEvent.click(screen.getByRole("button", { name: /End room/ }));
  await waitFor(() => expect(sent).toContainEqual({ type: "leave" }));

  sent.length = 0;
  askAnswer = new Error("dialog.message not allowed");
  await userEvent.click(screen.getByRole("button", { name: /End room/ }));
  await waitFor(() => expect(sent).toContainEqual({ type: "leave" }));
});

test("a host with friends leaves, handing the room over", async () => {
  const { ask } = await import("@tauri-apps/plugin-dialog");
  render(<App />);
  await act(async () => emit(room));
  await userEvent.click(screen.getByRole("button", { name: /Leave room/ }));
  await waitFor(() => expect(sent).toContainEqual({ type: "leave" }));
  expect(vi.mocked(ask).mock.calls.at(-1)![0]).toMatch(/Riya becomes the host/);
});

test("invite link: copies the link, and opening one joins", async () => {
  launchUrls = ["watchsync://join/7b2269"];
  render(<App />);
  await waitFor(() => expect(sent).toContainEqual({ type: "join", code: "watchsync://join/7b2269" }));

  await act(async () => emit(room));
  await userEvent.click(screen.getByRole("button", { name: "Copy invite link" }));
  expect(writeText).toHaveBeenCalledWith(room.invite_link);
});

test("invite link while in a room: asks, then leaves and joins the new one", async () => {
  render(<App />);
  await act(async () => emit({ ...room, is_host: false }));
  sent.length = 0;
  await act(async () => openUrl(["watchsync://join/feed01"]));
  await waitFor(() =>
    expect(sent).toEqual([{ type: "leave" }, { type: "join", code: "watchsync://join/feed01" }]),
  );

  sent.length = 0;
  askAnswer = false;
  await act(async () => openUrl(["watchsync://join/feed02"]));
  await new Promise((r) => setTimeout(r, 50));
  expect(sent).toEqual([]);
});

test("chat: send a message and a reaction; typing doesn't pause the movie", async () => {
  render(<App />);
  await act(async () =>
    emit({
      ...room,
      chat: [
        { from: "Riya", text: "popcorn ready?", reaction: null, mine: false, at_ms: 1 },
        { from: "Aman", text: null, reaction: "laugh", mine: false, at_ms: 2 },
      ],
    }),
  );
  const feed = within(screen.getByRole("region", { name: "Chat" })).getAllByRole("listitem");
  expect(feed[0]).toHaveTextContent("Riya popcorn ready?");
  expect(feed[1]).toHaveTextContent("😂");
  expect(feed[1]).toHaveTextContent("Aman");

  const box = screen.getByRole("textbox", { name: "Message" });
  await userEvent.type(box, "  yes! {Enter}");
  expect(sent).toEqual([{ type: "chat", text: "yes!" }]);
  expect(box).toHaveValue("");
  await userEvent.type(box, " ");
  expect(sent).not.toContainEqual({ type: "toggle_play" });

  await userEvent.click(screen.getByRole("button", { name: "Fire" }));
  expect(sent).toContainEqual({ type: "react", reaction: "fire" });
});

test("sound: pick tracks, and comfort settings are remembered", async () => {
  const { unmount } = render(<App />);
  await act(async () => emit(room));
  const audio = screen.getByRole("combobox", { name: "Audio" });
  expect(audio).toHaveDisplayValue("English, 5.1, Surround");
  await userEvent.selectOptions(audio, "Hindi, stereo");
  expect(sent).toContainEqual({ type: "set_track", kind: "audio", id: 2 });

  const subs = screen.getByRole("combobox", { name: "Subtitles" });
  expect(subs).toHaveDisplayValue("Off");
  await userEvent.selectOptions(subs, "English, SDH");
  expect(sent).toContainEqual({ type: "set_track", kind: "subtitle", id: 2 });

  await userEvent.click(screen.getByRole("switch", { name: /Boost dialogue/ }));
  expect(sent).toContainEqual({ type: "set_comfort", comfort: { level_volume: false, boost_dialogue: true } });

  unmount();
  sent.length = 0;
  render(<App />);
  await waitFor(() =>
    expect(sent).toContainEqual({ type: "set_comfort", comfort: { level_volume: false, boost_dialogue: true } }),
  );
});

test("shows that it's reconnecting, calmly", async () => {
  render(<App />);
  await act(async () => emit({ ...room, is_host: false, reconnecting: true }));
  expect(screen.getByRole("status")).toHaveTextContent("Reconnecting to the room. Your movie keeps playing.");
});

const offer = {
  offer_id: "o1",
  from_id: "riya-id",
  from: "Riya",
  name: "The Odyssey 2026 1080p BluRay HEVC x265 5.1 BONE.mkv",
  size: 4470387000,
  fingerprint: "abc",
};

test("movie file: get a friend's copy, watch its progress, cancel", async () => {
  render(<App />);
  await act(async () => emit({ ...room, is_host: false, file_name: null, player_open: false, my_file: "no_file", offers: [offer] }));
  expect(screen.getByText(/can send you their copy of The Odyssey/)).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Get Riya's copy (4.2 GB)" }));
  expect(sent).toContainEqual({ type: "get_file", offer_id: "o1" });

  await act(async () =>
    emit({
      ...room,
      is_host: false,
      file_name: null,
      my_file: "no_file",
      offers: [offer],
      transfers: [
        {
          id: 7,
          direction: "receive",
          peer: "Riya",
          file: offer.name,
          size: offer.size,
          progress: { percent: 45, amount: "1.9/4.2 GB", speed: "12 MB/s", eta: "3m10s" },
          state: "running",
          message: null,
          path: null,
        },
      ],
    }),
  );
  expect(screen.getByText("Getting The Odyssey from Riya")).toBeInTheDocument();
  expect(screen.getByText("45%, 12 MB/s, 3m10s left")).toBeInTheDocument();
  expect(screen.getByRole("progressbar")).toHaveAttribute("aria-valuenow", "45");
  expect(screen.getByRole("button", { name: /Get Riya's copy/ })).toBeDisabled();
  await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(sent).toContainEqual({ type: "cancel_transfer", id: 7 });
});

test("movie file: no offer shown when you already have the right copy", async () => {
  render(<App />);
  await act(async () => emit({ ...room, is_host: false, my_file: "match", offers: [offer] }));
  expect(screen.queryByRole("button", { name: /Get Riya's copy/ })).not.toBeInTheDocument();
});

test("movie file: share your copy; croc is offered only when needed", async () => {
  setup = { installed: true, has_brew: true, croc: false };
  render(<App />);
  await act(async () => emit(room));
  expect(screen.queryByText(/Sending movies uses croc/)).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Share my copy" }));
  expect(sent).not.toContainEqual({ type: "share_file" });
  expect(screen.getByText(/Sending movies uses croc/)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Install croc" })).toBeInTheDocument();
});

test("movie file: failed downloads say why and can be retried", async () => {
  render(<App />);
  await act(async () =>
    emit({
      ...room,
      is_host: false,
      my_file: "no_file",
      offers: [offer],
      transfers: [
        {
          id: 3,
          direction: "receive",
          peer: "Riya",
          file: offer.name,
          size: offer.size,
          progress: { percent: 12, amount: null, speed: null, eta: null },
          state: "failed",
          message: "Couldn't connect to Riya",
          path: null,
        },
      ],
    }),
  );
  expect(screen.getByText("Couldn't connect to Riya")).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Try again" }));
  expect(sent).toContainEqual({ type: "get_file", offer_id: "o1" });
});

test("one-time setup installs both helper apps", async () => {
  setup = { installed: false, has_brew: false, croc: false };
  render(<App />);
  expect(await screen.findByText(/mpv, the video player, and croc/)).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Copy command" }));
  expect(writeText).toHaveBeenLastCalledWith(expect.stringMatching(/brew install mpv croc$/));
});

test("settings: helper apps, with a one-click install for what's missing", async () => {
  setup = { installed: true, has_brew: true, croc: false };
  render(<App />);
  await openSettings();
  expect(screen.getByText(/croc sends movies to friends \(not installed\)/)).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Install croc" }));
  const { invoke } = await import("@tauri-apps/api/core");
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("install_player"));
});

test("suggests the host's movie when it's found on this Mac", async () => {
  render(<App />);
  const path = "/Users/riya/Downloads/Movies/The Odyssey 2026 1080p BluRay HEVC x265 5.1 BONE.mkv";
  await act(async () =>
    emit({ ...room, is_host: false, file_name: null, player_open: false, my_file: "no_file", found: { path, folder: "Downloads › Movies" } }),
  );
  expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Found The Odyssey on your Mac");
  expect(screen.getByText(/It's in Downloads › Movies/)).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Open it" }));
  expect(sent).toContainEqual({ type: "open_file", path });
  expect(screen.getByRole("button", { name: "Choose a different file…" })).toBeInTheDocument();
});

test("settings: call shortcuts are recorded and handed to the app", async () => {
  localStorage.removeItem("watchsync.shortcuts");
  render(<App />);
  const { invoke } = await import("@tauri-apps/api/core");
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("set_call_shortcuts", { mic: "Alt+Command+KeyM", cam: "Alt+Command+KeyV" }));
  await openSettings();
  const mic = screen.getByRole("button", { name: "Mic shortcut" });
  expect(mic).toHaveTextContent("⌥⌘M");
  await userEvent.click(mic);
  expect(mic).toHaveTextContent("Press keys…");
  await userEvent.keyboard("{Control>}{Shift>}k{/Shift}{/Control}");
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("set_call_shortcuts", { mic: "Control+Shift+KeyK", cam: "Alt+Command+KeyV" }));
  expect(mic).toHaveTextContent("⌃⇧K");
  expect(JSON.parse(localStorage.getItem("watchsync.shortcuts")!).mic).toBe("Control+Shift+KeyK");

  // One another app already uses: say so, keep the old one.
  shortcutError = "Alt+KeyX is taken by another app";
  await userEvent.click(mic);
  await userEvent.keyboard("{Alt>}x{/Alt}");
  expect(await screen.findByRole("alert")).toHaveTextContent("taken by another app");
  expect(mic).toHaveTextContent("⌃⇧K");
  shortcutError = null;
});
