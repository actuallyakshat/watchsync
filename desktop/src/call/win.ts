// The call window's own frame: size, position, drag-to-resize. Kept apart so
// tests can replace it.
import { availableMonitors, currentMonitor, getCurrentWindow, LogicalPosition, LogicalSize } from "@tauri-apps/api/window";

export async function setWindowSize(width: number, height: number) {
  try {
    await getCurrentWindow().setSize(new LogicalSize(Math.round(width), Math.round(height)));
  } catch (e) {
    console.warn("couldn't resize the call window", e);
  }
}

export function startResize() {
  void getCurrentWindow()
    .startResizeDragging("SouthEast")
    .catch(() => {});
}

/** Logical size after the person resizes the window. */
export function onResized(f: (width: number, height: number) => void): Promise<() => void> {
  const w = getCurrentWindow();
  return w.onResized(async ({ payload }) => {
    const scale = await w.scaleFactor();
    f(payload.width / scale, payload.height / scale);
  });
}

// Where the window was last put, per display (by name), in logical pixels.
const POSITION_KEY = "watchsync.call.position";
interface Positions {
  last?: string;
  at: Record<string, { x: number; y: number }>;
}

function load(): Positions {
  try {
    const p = JSON.parse(localStorage.getItem(POSITION_KEY) ?? "null");
    return p && typeof p.at === "object" ? p : { at: {} };
  } catch {
    return { at: {} };
  }
}

/** Put the window back where it was last left: on the display it was on if
 * that's still connected, else on any display it has been on, else in the
 * top-right corner of this one. */
export async function restorePosition(width: number) {
  try {
    const w = getCurrentWindow();
    const saved = load();
    const monitors = await availableMonitors();
    const names = monitors.map((m) => m.name ?? "");
    const name = (saved.last && names.includes(saved.last) ? saved.last : names.find((n) => saved.at[n])) ?? null;
    if (name && saved.at[name]) {
      const { x, y } = saved.at[name];
      await w.setPosition(new LogicalPosition(x, y));
      return;
    }
    const m = (await currentMonitor()) ?? monitors[0];
    if (!m) return;
    const s = m.scaleFactor;
    const area = m.workArea ?? { position: m.position, size: m.size };
    await w.setPosition(
      new LogicalPosition(area.position.x / s + area.size.width / s - width - 20, area.position.y / s + 20),
    );
  } catch (e) {
    console.warn("couldn't place the call window", e);
  }
}

/** Remember where the person drags the window. */
export function rememberPosition(): Promise<() => void> {
  const w = getCurrentWindow();
  let timer: ReturnType<typeof setTimeout> | undefined;
  return w.onMoved(({ payload }) => {
    clearTimeout(timer);
    timer = setTimeout(async () => {
      const m = await currentMonitor();
      if (!m) return;
      const name = m.name ?? "";
      const saved = load();
      saved.at[name] = { x: payload.x / m.scaleFactor, y: payload.y / m.scaleFactor };
      saved.last = name;
      localStorage.setItem(POSITION_KEY, JSON.stringify(saved));
    }, 400);
  });
}
