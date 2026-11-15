// What happens to your mic before friends hear it: a high-pass filter
// (desk thumps, rumble), the voice gate (typing, clicks, room noise; see
// voiceGate.worklet.js), then your input volume. If any of it isn't
// available, the mic is sent as it is rather than not at all.

export interface MicSettings {
  /** Keep typing, clicks and background noise out. */
  suppress: boolean;
  /** Sensitivity follows the room (else `thresholdDb`). */
  auto: boolean;
  thresholdDb: number;
  /** Input volume, 0–2 (1 = as the mic gives it). */
  volume: number;
  /** Let the system level your voice (WebKit's auto gain). */
  autoGain: boolean;
}

export const DEFAULT_MIC: MicSettings = { suppress: true, auto: true, thresholdDb: -50, volume: 1, autoGain: true };

/** The system's own processing for the mic, from our settings. */
export function micConstraints(s: MicSettings): MediaTrackConstraints {
  return { echoCancellation: true, noiseSuppression: s.suppress, autoGainControl: s.autoGain };
}

export interface MicChain {
  /** The processed track (what's sent). */
  track: MediaStreamTrack;
  set(s: MicSettings): void;
  /** Input level, 0–1 (before the gate). */
  level(): number;
  /** Your voice is being let through right now. */
  open(): boolean;
  /** Where the gate opens, in dB (to draw on the meter). */
  thresholdDb(): number;
  close(): void;
}

const START_TIMEOUT_MS = 800;

export async function processMic(raw: MediaStreamTrack, settings: MicSettings): Promise<MicChain | null> {
  if (typeof AudioContext === "undefined" || typeof AudioWorkletNode === "undefined") return null;
  let ctx: AudioContext | null = null;
  try {
    ctx = new AudioContext({ latencyHint: "interactive" });
    await ctx.audioWorklet.addModule(new URL("./voiceGate.worklet.js", import.meta.url));
    // A page that's capturing may start audio by itself; if WebKit holds it
    // back anyway, send the plain mic rather than silence.
    await Promise.race([ctx.resume(), new Promise((r) => setTimeout(r, START_TIMEOUT_MS))]);
    if (ctx.state !== "running") throw new Error("audio didn't start");

    const source = ctx.createMediaStreamSource(new MediaStream([raw]));
    const lowCut = ctx.createBiquadFilter();
    lowCut.type = "highpass";
    lowCut.frequency.value = 85;
    const gate = new AudioWorkletNode(ctx, "voice-gate", {
      numberOfInputs: 1,
      numberOfOutputs: 1,
      outputChannelCount: [1],
      channelCount: 1,
      channelCountMode: "explicit",
    });
    const volume = ctx.createGain();
    const out = ctx.createMediaStreamDestination();
    source.connect(lowCut).connect(gate).connect(volume).connect(out);

    let report = { level: 0, open: false, thresholdDb: -60 };
    gate.port.onmessage = (e) => (report = e.data);
    const apply = (s: MicSettings) => {
      gate.port.postMessage({ suppress: s.suppress, auto: s.auto, thresholdDb: s.thresholdDb });
      volume.gain.setTargetAtTime(Math.min(2, Math.max(0, s.volume)), ctx!.currentTime, 0.02);
    };
    apply(settings);

    const track = out.stream.getAudioTracks()[0];
    const context = ctx;
    return {
      track,
      set: apply,
      level: () => report.level,
      open: () => report.open,
      thresholdDb: () => report.thresholdDb,
      close: () => {
        track.stop();
        source.disconnect();
        void context.close().catch(() => {});
      },
    };
  } catch (e) {
    console.warn("sending the mic without processing:", e);
    void ctx?.close().catch(() => {});
    return null;
  }
}
