import type { Comfort, Track, TrackKind } from "../api";

const LANGS: Record<string, string> = {
  eng: "English", en: "English", hin: "Hindi", hi: "Hindi", fre: "French", fra: "French", fr: "French",
  spa: "Spanish", es: "Spanish", ger: "German", deu: "German", de: "German", ita: "Italian", it: "Italian",
  jpn: "Japanese", ja: "Japanese", kor: "Korean", ko: "Korean", tam: "Tamil", tel: "Telugu", mal: "Malayalam",
  por: "Portuguese", pt: "Portuguese", rus: "Russian", ru: "Russian", chi: "Chinese", zho: "Chinese", ara: "Arabic",
};

/** "English, 5.1, Surround" from what the file says about a track. */
export function trackLabel(t: Track): string {
  const lang = t.lang ? (LANGS[t.lang.toLowerCase()] ?? t.lang.toUpperCase()) : null;
  const ch = t.channels === 6 ? "5.1" : t.channels === 8 ? "7.1" : t.channels === 2 ? "stereo" : t.channels === 1 ? "mono" : null;
  const parts = [lang, t.kind === "audio" ? ch : null, t.title && t.title !== lang ? t.title : null].filter(Boolean);
  return parts.length ? parts.join(", ") : `Track ${t.id}`;
}

interface Props {
  tracks: Track[];
  comfort: Comfort;
  playerOpen: boolean;
  onTrack: (kind: TrackKind, id: number | null) => void;
  onComfort: (c: Comfort) => void;
}

/** Personal settings: only change this person's sound and subtitles. */
export function Sound({ tracks, comfort, playerOpen, onTrack, onComfort }: Props) {
  const audio = tracks.filter((t) => t.kind === "audio");
  const subs = tracks.filter((t) => t.kind === "subtitle");
  const selected = (list: Track[]) => list.find((t) => t.selected)?.id ?? null;

  return (
    <section className="section" aria-labelledby="sound-head">
      <div className="section-head">
        <h2 id="sound-head">Sound and subtitles</h2>
        <span>Just for you</span>
      </div>
      <div className="sound">
        <label className="field">
          <span>Audio</span>
          <select
            value={selected(audio) ?? ""}
            disabled={!playerOpen || audio.length < 2}
            onChange={(e) => onTrack("audio", Number(e.target.value))}
          >
            {audio.length === 0 && <option value="">{playerOpen ? "Loading…" : "Choose a movie first"}</option>}
            {audio.map((t) => (
              <option key={t.id} value={t.id}>
                {trackLabel(t)}
              </option>
            ))}
          </select>
        </label>
        <label className="field">
          <span>Subtitles</span>
          <select
            value={selected(subs) ?? "off"}
            disabled={!playerOpen || subs.length === 0}
            onChange={(e) => onTrack("subtitle", e.target.value === "off" ? null : Number(e.target.value))}
          >
            <option value="off">{subs.length === 0 && playerOpen ? "None in this file" : "Off"}</option>
            {subs.map((t) => (
              <option key={t.id} value={t.id}>
                {trackLabel(t)}
              </option>
            ))}
          </select>
        </label>
        <Toggle
          label="Even out volume"
          hint="Quieter explosions, louder whispers"
          on={comfort.level_volume}
          onChange={(v) => onComfort({ ...comfort, level_volume: v })}
        />
        <Toggle
          label="Boost dialogue"
          hint="Voices stand out from music and effects"
          on={comfort.boost_dialogue}
          onChange={(v) => onComfort({ ...comfort, boost_dialogue: v })}
        />
      </div>
    </section>
  );
}

function Toggle({ label, hint, on, onChange }: { label: string; hint: string; on: boolean; onChange: (v: boolean) => void }) {
  return (
    <button className={`toggle ${on ? "on" : ""}`} role="switch" aria-checked={on} onClick={() => onChange(!on)}>
      <span className="switch" aria-hidden="true">
        <i />
      </span>
      <span className="toggle-text">
        <span>{label}</span>
        <small>{hint}</small>
      </span>
    </button>
  );
}
