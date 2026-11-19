// Where each tile goes in the expanded call. Everything a person shows is a
// tile: their camera, and their screen while they share. In the spotlight
// view one tile is big (the stage) and the rest sit in a filmstrip beside
// it; in the grid every tile is the same size.

export type TileKind = "cam" | "screen";

export interface TileRef {
  /** The person's id, plus ":screen" for their screen. "" is you. */
  key: string;
  kind: TileKind;
  /** Your own camera or screen. */
  self: boolean;
}

export const tileKey = (id: string, kind: TileKind) => (kind === "screen" ? `${id}:screen` : id);

/** What the stage shows: what you pinned, else the newest friend's screen,
 * else whoever is talking, else the first friend. Null: just you. */
export function pickStage(tiles: TileRef[], pinned: string | null, speaker: string | null): string | null {
  if (pinned && tiles.some((t) => t.key === pinned)) return pinned;
  const theirs = tiles.filter((t) => !t.self);
  const screens = theirs.filter((t) => t.kind === "screen");
  if (screens.length) return screens[screens.length - 1].key;
  const cams = theirs.filter((t) => t.kind === "cam");
  if (speaker && cams.some((t) => t.key === speaker)) return speaker;
  return cams[0]?.key ?? null;
}

export interface Box {
  w: number;
  h: number;
}

/** Room left for a filmstrip next to the stage: beside it in a wide window,
 * under it in a tall one; none in a small one (a corner tile instead). */
export function filmstripSide(box: Box | null): "right" | "bottom" | null {
  if (!box || box.w < 420 || box.h < 240) return null;
  return box.w / box.h >= 1.2 ? "right" : "bottom";
}

export const GAP = 4;
const ASPECT = 16 / 9;

/** Columns and tile size that show `n` 16:9 tiles as big as they fit. */
export function gridFit(n: number, box: Box | null): { cols: number; w: number; h: number } | null {
  if (n <= 0) return null;
  if (!box || box.w <= 0 || box.h <= 0) return null;
  let best = { cols: 1, w: 0, h: 0 };
  for (let cols = 1; cols <= n; cols++) {
    const rows = Math.ceil(n / cols);
    const cellW = (box.w - GAP * (cols - 1)) / cols;
    const cellH = (box.h - GAP * (rows - 1)) / rows;
    const w = Math.min(cellW, cellH * ASPECT);
    if (w > best.w) best = { cols, w: Math.floor(w), h: Math.floor(w / ASPECT) };
  }
  return best;
}
