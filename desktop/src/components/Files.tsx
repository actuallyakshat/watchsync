import { useState } from "react";
import type { Offer, SendMethod, Snapshot, Transfer } from "../api";
import { formatSize, movieTitle } from "../format";

interface Props {
  snap: Snapshot;
  crocInstalled: boolean;
  /** Installed, but too old to connect Macs directly. */
  crocOutdated: boolean;
  hasBrew: boolean;
  onInstallCroc: () => Promise<void>;
  onShare: (method: SendMethod) => void;
  onStopSharing: () => void;
  onGet: (offerId: string) => void;
  onCancel: (id: number) => void;
}

/** Sending and getting the movie file itself: directly, or with croc. */
export function Files({ snap, crocInstalled, crocOutdated, hasBrew, onInstallCroc, onShare, onStopSharing, onGet, onCancel }: Props) {
  const [needCroc, setNeedCroc] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [installError, setInstallError] = useState<string | null>(null);

  /** Only croc transfers need croc installed. */
  const withCroc = (method: SendMethod, action: () => void) => () => {
    if (method === "direct" || crocInstalled) action();
    else setNeedCroc(true);
  };
  const install = async () => {
    setInstalling(true);
    setInstallError(null);
    try {
      await onInstallCroc();
      setNeedCroc(false);
    } catch (e) {
      setInstallError(String(e));
    } finally {
      setInstalling(false);
    }
  };

  // Offer copies only to people who don't already have the right one.
  const offers = snap.my_file === "match" ? [] : snap.offers;
  const canShare = snap.file_name !== null;
  const visible = snap.transfers.filter((t) => t.state !== "cancelled").slice(-4);
  if (offers.length === 0 && !canShare && visible.length === 0) return null;

  return (
    <section className="section" aria-labelledby="files-head">
      <div className="section-head">
        <h2 id="files-head">Movie file</h2>
        <span>Your Mac stays awake while a transfer runs</span>
      </div>

      {offers.map((o) => (
        <OfferRow
          key={o.offer_id}
          offer={o}
          onGet={withCroc(o.method ?? "croc", () => onGet(o.offer_id))}
          busy={isBusy(snap.transfers, o)}
        />
      ))}

      {canShare && (
        <div className="file-row">
          <p>
            {snap.sharing
              ? snap.sharing_method === "croc"
                ? "You're offering your copy, sent with croc. Friends who don't have it can get it from you."
                : "You're offering your copy, sent straight from your Mac. Friends who don't have it can get it from you."
              : "Friends without the movie can get your copy. Directly is fastest: straight from your Mac to theirs when your networks allow it. croc goes through croc's public relay: a backup if direct is slow, or for friends who haven't updated."}
          </p>
          {snap.sharing ? (
            <button className="ghost" onClick={onStopSharing}>
              Stop sharing
            </button>
          ) : (
            <div className="share-choice">
              <button className="ghost" onClick={() => onShare("direct")}>
                Share directly
              </button>
              <button className="ghost" onClick={withCroc("croc", () => onShare("croc"))}>
                Share with croc
              </button>
            </div>
          )}
        </div>
      )}

      {crocInstalled && crocOutdated && (canShare || offers.some((o) => o.method === "croc")) && (
        <div className="croc-setup" role="status">
          <p>
            Your croc is an old version, so croc transfers always go through croc's public relay, which is slow. Newer
            croc connects your Macs directly when it can (your friend needs it too).
            {!hasBrew && " Update it in Terminal with: brew upgrade croc"}
          </p>
          {hasBrew && (
            <button className="primary" onClick={install} disabled={installing}>
              {installing ? "Updating croc…" : "Update croc"}
            </button>
          )}
          {installError && <p className="error-text">{installError}</p>}
        </div>
      )}

      {needCroc && !crocInstalled && (
        <div className="croc-setup" role="alert">
          <p>
            This uses croc, a free file-transfer tool. It isn't on this Mac yet.
            {!hasBrew && " Install it in Terminal with: brew install croc"}
          </p>
          {hasBrew && (
            <button className="primary" onClick={install} disabled={installing}>
              {installing ? "Installing croc…" : "Install croc"}
            </button>
          )}
          {installError && <p className="error-text">{installError}</p>}
        </div>
      )}

      {visible.length > 0 && (
        <ul className="transfers">
          {visible.map((t) => (
            <TransferRow key={t.id} t={t} onCancel={() => onCancel(t.id)} onRetry={retryFor(t, snap.offers, onGet)} />
          ))}
        </ul>
      )}
    </section>
  );
}

function isBusy(transfers: Transfer[], o: Offer) {
  return transfers.some(
    (t) => t.direction === "receive" && t.file === o.name && ["waiting", "running", "checking"].includes(t.state),
  );
}

function retryFor(t: Transfer, offers: Offer[], onGet: (id: string) => void) {
  if (t.direction !== "receive" || t.state !== "failed") return null;
  const o = offers.find((o) => o.name === t.file && o.from === t.peer);
  return o ? () => onGet(o.offer_id) : null;
}

function OfferRow({ offer, onGet, busy }: { offer: Offer; onGet: () => void; busy: boolean }) {
  const { title } = movieTitle(offer.name);
  return (
    <div className="file-row offer">
      <p>
        <b>{offer.from}</b> can send you their copy of {title}
        {offer.method === "direct" ? ", straight from their Mac." : ", with croc."}
      </p>
      <button className="primary" onClick={onGet} disabled={busy}>
        Get {offer.from}'s copy ({formatSize(offer.size)})
      </button>
    </div>
  );
}

function TransferRow({ t, onCancel, onRetry }: { t: Transfer; onCancel: () => void; onRetry: (() => void) | null }) {
  const what = t.direction === "receive" ? `Getting ${movieTitle(t.file).title} from ${t.peer}` : `Sending your copy to ${t.peer}`;
  const p = t.progress;
  let detail: string;
  switch (t.state) {
    case "waiting":
      detail = t.direction === "receive" ? `Waiting for ${t.peer}…` : `Waiting for ${t.peer} to connect…`;
      break;
    case "running":
      detail = [
        `${p.percent}%`,
        p.speed,
        p.eta && p.eta !== "0s" ? `${p.eta} left` : null,
        p.via === "direct" ? "direct" : p.via === "relayed" ? "through a relay (slower)" : null,
      ]
        .filter(Boolean)
        .join(", ");
      break;
    case "checking":
      detail = "Checking the file…";
      break;
    case "done":
      detail = t.direction === "receive" ? "Done. Saved in Movies › WatchSync and opened." : "Sent.";
      break;
    default:
      detail = t.message ?? "Stopped.";
  }
  const active = t.state === "waiting" || t.state === "running";
  const percent = t.state === "done" ? 100 : p.percent;
  return (
    <li className={`transfer ${t.state}`}>
      <div className="transfer-head">
        <span className="transfer-what">{what}</span>
        {active && (
          <button className="link" onClick={onCancel}>
            Cancel
          </button>
        )}
        {onRetry && (
          <button className="link" onClick={onRetry}>
            Try again
          </button>
        )}
      </div>
      {(active || t.state === "checking" || t.state === "done") && (
        <div className="bar" role="progressbar" aria-valuenow={percent} aria-valuemin={0} aria-valuemax={100} aria-label={what}>
          <i style={{ width: `${percent}%` }} />
        </div>
      )}
      <p className={`transfer-detail ${t.state === "failed" ? "bad" : ""}`}>{detail}</p>
    </li>
  );
}
