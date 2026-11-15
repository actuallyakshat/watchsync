// The call's system-wide shortcuts, remembered on this Mac. Stored in the
// format the Rust side parses ("Alt+Command+KeyM").
export interface Shortcuts {
  mic: string;
  cam: string;
}

export const SHORTCUTS_KEY = "watchsync.shortcuts";
export const DEFAULT_SHORTCUTS: Shortcuts = { mic: "Alt+Command+KeyM", cam: "Alt+Command+KeyV" };

export function savedShortcuts(): Shortcuts {
  try {
    const s = JSON.parse(localStorage.getItem(SHORTCUTS_KEY) ?? "null");
    return s && typeof s.mic === "string" && typeof s.cam === "string" ? s : DEFAULT_SHORTCUTS;
  } catch {
    return DEFAULT_SHORTCUTS;
  }
}

export function saveShortcuts(s: Shortcuts) {
  localStorage.setItem(SHORTCUTS_KEY, JSON.stringify(s));
}

/** A key press as a shortcut, or null if it's only modifiers or has none. */
export function fromEvent(e: Pick<KeyboardEvent, "code" | "ctrlKey" | "altKey" | "shiftKey" | "metaKey">): string | null {
  if (/^(Control|Alt|Shift|Meta|OS)(Left|Right)?$/.test(e.code) || !e.code) return null;
  const mods = [e.ctrlKey && "Control", e.altKey && "Alt", e.shiftKey && "Shift", e.metaKey && "Command"].filter(Boolean);
  if (mods.length === 0) return null;
  return [...mods, e.code].join("+");
}

const SYMBOLS: Record<string, string> = { Control: "⌃", Alt: "⌥", Shift: "⇧", Command: "⌘" };

/** "Alt+Command+KeyM" → "⌥⌘M". */
export function label(shortcut: string): string {
  return shortcut
    .split("+")
    .map((p) => SYMBOLS[p] ?? p.replace(/^Key/, "").replace(/^Digit/, "").replace(/^Arrow/, ""))
    .join("");
}
