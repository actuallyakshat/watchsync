// Sizes of the call window's collapsed strip (logical pixels).
export const PILL_HEIGHT = 48;

/** Avatars, then mic, camera and expand buttons. */
export function pillWidth(people: number): number {
  return Math.max(200, 12 + Math.min(people, 4) * 30 + 3 * 34 + 16);
}
