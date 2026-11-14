// The display name, remembered on this Mac. The engine starts with the
// Mac's account name; a saved name replaces it as soon as the window opens.
export const NAME_KEY = "watchsync.name";
export const MAX_NAME = 32; // matches engine::MAX_NAME

export function savedName(): string | null {
  const n = localStorage.getItem(NAME_KEY)?.trim();
  return n ? n : null;
}

/** Trimmed and length-limited; null if empty. */
export function cleanName(raw: string): string | null {
  const n = [...raw.trim()].slice(0, MAX_NAME).join("");
  return n ? n : null;
}

export function saveName(name: string) {
  localStorage.setItem(NAME_KEY, name);
}
