import { useEffect, useRef, useState } from "react";
import type { ChatLine, Reaction } from "../api";

export const REACTIONS: { id: Reaction; emoji: string; label: string }[] = [
  { id: "laugh", emoji: "😂", label: "Laugh" },
  { id: "wow", emoji: "😱", label: "Shocked" },
  { id: "love", emoji: "❤️", label: "Love it" },
  { id: "fire", emoji: "🔥", label: "Fire" },
  { id: "clap", emoji: "👏", label: "Clap" },
  { id: "sad", emoji: "😢", label: "Sad" },
];
const EMOJI = Object.fromEntries(REACTIONS.map((r) => [r.id, r.emoji])) as Record<Reaction, string>;
const MAX_CHAT = 300; // matches engine::MAX_CHAT

interface Props {
  lines: ChatLine[];
  onSend: (text: string) => void;
  onReact: (r: Reaction) => void;
}

/** The room's chat: messages and reactions, also shown on the movie. */
export function Chat({ lines, onSend, onReact }: Props) {
  const [draft, setDraft] = useState("");
  const feed = useRef<HTMLOListElement>(null);

  // Keep the newest line in view.
  useEffect(() => {
    feed.current?.scrollTo?.({ top: feed.current.scrollHeight });
  }, [lines.length]);

  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    const text = draft.trim();
    if (!text) return;
    onSend(text);
    setDraft("");
  };

  return (
    <section className="chat" aria-label="Chat">
      <ol className="chat-feed" ref={feed} aria-live="polite">
        {lines.length === 0 && <li className="chat-empty">Messages and reactions show up here and on the movie.</li>}
        {lines.map((l, i) =>
          l.reaction ? (
            <li key={i} className="chat-line reaction">
              <span className="chat-emoji" aria-hidden="true">
                {EMOJI[l.reaction]}
              </span>
              <span>
                <b>{l.mine ? "You" : l.from}</b>{" "}
                <span className="sr-only">reacted {REACTIONS.find((r) => r.id === l.reaction)?.label}</span>
              </span>
            </li>
          ) : (
            <li key={i} className={`chat-line ${l.mine ? "mine" : ""}`}>
              <b>{l.mine ? "You" : l.from}</b>{" "}
              <span className="chat-text">{l.text}</span>
            </li>
          ),
        )}
      </ol>
      <div className="reactions" role="group" aria-label="React">
        {REACTIONS.map((r) => (
          <button key={r.id} className="react" onClick={() => onReact(r.id)} aria-label={r.label} title={r.label}>
            {r.emoji}
          </button>
        ))}
      </div>
      <form className="chat-form" onSubmit={submit}>
        <input
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          maxLength={MAX_CHAT}
          placeholder="Say something"
          aria-label="Message"
          autoCorrect="off"
        />
      </form>
    </section>
  );
}
