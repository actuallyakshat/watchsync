// Who the speaker view shows: whoever is talking, without flicking between
// people on every cough. Someone new takes over only after talking for a
// moment while the current speaker is quiet.

/** Talking this long takes over the view. */
export const TAKE_OVER_MS = 800;
/** A pause shorter than this still counts as talking. */
export const HANGOVER_MS = 600;
/** Speaking level (0–1, as WebRTC reports audioLevel) that counts as talking. */
export const SPEAKING_LEVEL = 0.04;

export interface Talk {
  /** When this run of talking started. */
  since: number;
  /** Last time they were above the speaking level. */
  last: number;
}

/** Update who is talking from the latest levels. */
export function track(talk: Map<string, Talk>, levels: Map<string, number>, now: number): Map<string, Talk> {
  const next = new Map<string, Talk>();
  for (const [id, level] of levels) {
    const prev = talk.get(id);
    const loud = level >= SPEAKING_LEVEL;
    if (loud) next.set(id, { since: prev && now - prev.last <= HANGOVER_MS ? prev.since : now, last: now });
    else if (prev && now - prev.last <= HANGOVER_MS) next.set(id, prev);
  }
  return next;
}

export function isTalking(talk: Map<string, Talk>, id: string, now: number): boolean {
  const t = talk.get(id);
  return !!t && now - t.last <= HANGOVER_MS;
}

/** Who to show, out of `people` (in room order). */
export function pickSpeaker(current: string | null, people: string[], talk: Map<string, Talk>, now: number): string | null {
  if (people.length === 0) return null;
  const valid = current !== null && people.includes(current);
  if (valid && isTalking(talk, current, now)) return current;
  let best: string | null = null;
  let longest = 0;
  for (const id of people) {
    const t = talk.get(id);
    if (!t || !isTalking(talk, id, now)) continue;
    const length = now - t.since;
    if (length >= TAKE_OVER_MS && length > longest) {
      best = id;
      longest = length;
    }
  }
  return best ?? (valid ? current : people[0]);
}
