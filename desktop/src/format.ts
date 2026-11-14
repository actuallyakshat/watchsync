// Small text helpers, kept pure so they can be tested.

/** 3725 → "1:02:05", 65 → "1:05". */
export function formatTime(secs: number): string {
  const s = Math.max(0, Math.floor(secs));
  const h = Math.floor(s / 3600);
  const m = Math.floor(s / 60) % 60;
  const r = s % 60;
  const mm = h > 0 ? String(m).padStart(2, "0") : String(m);
  return `${h > 0 ? `${h}:` : ""}${mm}:${String(r).padStart(2, "0")}`;
}

/** Player offset from the room, as people read it. */
export function formatDrift(ms: number | null): string {
  if (ms === null) return "";
  if (Math.abs(ms) < 40) return "in sync";
  const sign = ms > 0 ? "+" : "−";
  return Math.abs(ms) >= 1000 ? `${sign}${(Math.abs(ms) / 1000).toFixed(1)} s` : `${sign}${Math.round(Math.abs(ms))} ms`;
}

const TAGS =
  /\b(2160p|1080p|720p|576p|480p|4k|uhd|blu-?ray|brrip|bdrip|web-?dl|web-?rip|webrip|web|hdtv|dvdrip|dvd|x26[45]|h\s?26[45]|hevc|avc|hdr10?|remux|proper|repack|extended|unrated|imax|amzn|nf|10bit|ddp?\s?5\s?1|dts|aac|atmos|multi)\b/i;

/**
 * A readable title from a release file name:
 * "The Odyssey 2026 1080p BluRay HEVC x265 5.1 BONE.mkv" → { title: "The Odyssey", year: "2026" }.
 */
export function movieTitle(fileName: string): { title: string; year: string | null } {
  let name = fileName.replace(/\.[a-z0-9]{2,4}$/i, "");
  name = name.replace(/[._]+/g, " ").replace(/\s+/g, " ").trim();

  // The release year: the last 19xx/20xx that isn't the very start (titles
  // like "2001 A Space Odyssey 1968" keep their leading number).
  let year: string | null = null;
  let cut = name.length;
  for (const m of name.matchAll(/(?:^|[\s([])((?:19|20)\d{2})(?=$|[\s)\]])/g)) {
    const at = (m.index ?? 0) + m[0].indexOf(m[1]);
    if (at === 0) continue;
    year = m[1];
    cut = Math.min(cut, at);
  }
  const tag = name.search(TAGS);
  if (tag > 0) cut = Math.min(cut, tag);

  const title = name
    .slice(0, cut)
    .replace(/[\s\-–([]+$/, "")
    .trim();
  return { title: title || name, year };
}

/** Width/height of the little frame drawn on a crop chip. */
export function frameSize(aspect: number | null): { w: number; h: number } {
  const a = aspect ?? 16 / 9;
  return a >= 16 / 9 ? { w: 18, h: Math.round(18 / a) } : { w: Math.round(10 * a), h: 10 };
}
