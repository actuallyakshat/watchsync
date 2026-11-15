// The call window's own frame: size and drag-to-resize. Kept apart so tests
// can replace it.
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";

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
