// Your Mac's sound, for a screen share. WebKit can't capture it, so the app
// does (natively, leaving out WatchSync's own sound: the call and the movie)
// and streams it here as raw stereo samples, which play into a track that's
// sent like any other.
import { Channel, invoke } from "@tauri-apps/api/core";

export interface SharedSound {
  track: MediaStreamTrack;
  stop(): void;
}

/** This Mac can share its sound (macOS 14.2 and later). */
export const shareSoundAvailable = () => invoke<boolean>("share_sound_available").catch(() => false);

/** Start sharing the Mac's sound; null if it can't be. */
export async function startShareSound(): Promise<SharedSound | null> {
  if (typeof AudioContext === "undefined" || typeof AudioWorkletNode === "undefined") return null;
  const channel = new Channel<ArrayBuffer>();
  let node: AudioWorkletNode | null = null;
  const early: Float32Array[] = [];
  channel.onmessage = (buf) => {
    const samples = new Float32Array(buf);
    if (node) node.port.postMessage(samples, [samples.buffer]);
    else if (early.length < 10) early.push(samples);
  };
  let ctx: AudioContext | null = null;
  try {
    const rate = await invoke<number>("start_share_sound", { onAudio: channel });
    ctx = new AudioContext({ sampleRate: rate || 48000 });
    await ctx.audioWorklet.addModule(new URL("./pcmPlayer.worklet.js", import.meta.url));
    node = new AudioWorkletNode(ctx, "pcm-player", { numberOfInputs: 0, outputChannelCount: [2] });
    for (const s of early.splice(0)) node.port.postMessage(s, [s.buffer]);
    const out = ctx.createMediaStreamDestination();
    out.channelCount = 2;
    node.connect(out);
    if (ctx.state === "suspended") await ctx.resume().catch(() => {});
    // Not running means silence: say it failed rather than send nothing.
    if (ctx.state !== "running") throw new Error(`audio ${ctx.state}`);
    const track = out.stream.getAudioTracks()[0];
    if (!track) throw new Error("no track");
    const context = ctx;
    const player = node;
    return {
      track,
      stop: () => {
        channel.onmessage = () => {};
        void invoke("stop_share_sound").catch(() => {});
        player.disconnect();
        track.stop();
        void context.close().catch(() => {});
      },
    };
  } catch (e) {
    console.warn("couldn't share the Mac's sound", e);
    void invoke("stop_share_sound").catch(() => {});
    void ctx?.close().catch(() => {});
    return null;
  }
}
