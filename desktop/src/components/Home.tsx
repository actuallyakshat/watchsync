import { useState } from "react";

interface Props {
  connecting: boolean;
  onHost: () => void;
  onJoin: (code: string) => void;
}

export function Home({ connecting, onHost, onJoin }: Props) {
  const [code, setCode] = useState("");

  if (connecting)
    return (
      <div className="home">
        <p className="connecting">
          <span className="spinner" />
          Connecting. This can take a few seconds.
        </p>
      </div>
    );

  const join = (e: React.FormEvent) => {
    e.preventDefault();
    if (code.trim()) onJoin(code.trim());
  };

  return (
    <div className="home">
      <h1>Movie night, from separate couches.</h1>
      <p className="lede">
        Everyone plays their own copy of the movie. Press pause on one Mac and it pauses on all of them.
      </p>
      <div className="home-actions">
        <button className="primary" onClick={onHost}>
          Host a room
        </button>
        <div className="or">or join a friend's</div>
        <form className="join" onSubmit={join}>
          <input
            value={code}
            onChange={(e) => setCode(e.target.value)}
            placeholder="Paste the invite link or code"
            aria-label="Invite link"
            spellCheck={false}
            autoCorrect="off"
          />
          <button className="ghost" type="submit" disabled={!code.trim()}>
            Join
          </button>
        </form>
      </div>
    </div>
  );
}
