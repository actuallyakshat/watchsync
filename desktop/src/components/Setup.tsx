import { useState } from "react";
import { installPlayer, type PlayerSetup } from "../api";

/** Installs Homebrew, mpv and croc, for pasting into Terminal. */
export const INSTALL_COMMAND =
  '/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)" && eval "$(/opt/homebrew/bin/brew shellenv 2>/dev/null || /usr/local/bin/brew shellenv)" && brew install mpv croc';

interface Props {
  setup: PlayerSetup;
  onRecheck: () => void;
  onCopy: (text: string) => Promise<void>;
}

/** Shown until mpv, the player WatchSync drives, is installed. */
export function Setup({ setup, onRecheck, onCopy }: Props) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  const install = async () => {
    setBusy(true);
    setError(null);
    try {
      await installPlayer();
      onRecheck();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="home">
      <h1>One more thing.</h1>
      <p className="lede">
        WatchSync needs two free helper apps: mpv, the video player, and croc, for sending movies to friends.
        You only do this once.
      </p>
      {setup.has_brew ? (
        <div className="row-actions">
          <button className="primary" onClick={install} disabled={busy}>
            {busy ? "Installing mpv and croc…" : "Install mpv and croc"}
          </button>
          {busy && <span className="connecting"><span className="spinner" />This takes a minute or two.</span>}
        </div>
      ) : (
        <>
          <p className="lede">
            Copy this command, paste it into the Terminal app and press Return. It installs Homebrew, then mpv and croc,
            and asks for your Mac password.
          </p>
          <div className="command">{INSTALL_COMMAND}</div>
          <div className="row-actions">
            <button
              className="primary"
              onClick={async () => {
                await onCopy(INSTALL_COMMAND);
                setCopied(true);
              }}
            >
              {copied ? "Copied" : "Copy command"}
            </button>
            <button className="ghost" onClick={onRecheck}>
              Check again
            </button>
          </div>
        </>
      )}
      {error && <p className="error-text">{error}</p>}
    </div>
  );
}
