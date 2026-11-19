// What happens to your mic before friends hear it: a high-pass filter
// (desk thumps, rumble), the voice gate (typing, clicks, room noise; see
// voiceGate.worklet.js), then your input volume. If any of it isn't
// available, or the audio engine stops mid-call (macOS stops it on an output
// change, like plugging in headphones), the mic is sent as it is rather
// than not at all.

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
  /** Your level in the voice band, 0–1 (what the gate listens to). */
  level(): number;
  /** Your voice is being let through right now. */
  open(): boolean;
  /** Where the gate opens, in dB (to draw on the meter). */
  thresholdDb(): number;
  /** Processing is running (else send the raw mic). */
  running(): boolean;
  /** Called when processing stops (after a try to restart it) or resumes. */
  onRunning(f: (running: boolean) => void): void;
  close(): void;
}

const START_TIMEOUT_MS = 800;
/** A stopped audio engine gets this long to restart before falling back. */
export const FALLBACK_MS = 500;
/** Meter readings older than this mean the gate has stopped reporting. */
const STALE_MS = 300;

export async function processMic(raw: MediaStreamTrack, settings: MicSettings): Promise<MicChain | null> {
  if (typeof AudioContext === "undefined" || typeof AudioWorkletNode === "undefined") return null;
  let ctx: AudioContext | null = null;
  try {
    // Pinned to 48 kHz: by default it follows the output device, and a
    // Bluetooth headset in call mode runs at 16–24 kHz, which would cut the
    // mic down to phone quality before it's ever sent.
    ctx = new AudioContext({ latencyHint: "interactive", sampleRate: 48000 });
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
    let reportAt = 0;
    gate.port.onmessage = (e) => {
      report = e.data;
      reportAt = performance.now();
    };
    const fresh = () => performance.now() - reportAt < STALE_MS;
    const apply = (s: MicSettings) => {
      gate.port.postMessage({ suppress: s.suppress, auto: s.auto, thresholdDb: s.thresholdDb });
      volume.gain.setTargetAtTime(Math.min(2, Math.max(0, s.volume)), ctx!.currentTime, 0.02);
    };
    apply(settings);

    const track = out.stream.getAudioTracks()[0];
    const context = ctx;
    let listener: (running: boolean) => void = () => {};
    let wasRunning = true;
    let fallback: ReturnType<typeof setTimeout> | undefined;
    let closed = false;
    const report_ = (running: boolean) => {
      if (running !== wasRunning && !closed) {
        wasRunning = running;
        listener(running);
      }
    };
    context.onstatechange = () => {
      clearTimeout(fallback);
      if (context.state === "running") return report_(true);
      if (context.state === "closed") return;
      // "suspended" or WebKit's "interrupted": try to start again, and send
      // the plain mic meanwhile if that doesn't work quickly.
      void context.resume().catch(() => {});
      fallback = setTimeout(() => report_((context.state as string) === "running"), FALLBACK_MS);
    };
    return {
      track,
      set: apply,
      level: () => (fresh() ? report.level : 0),
      open: () => fresh() && report.open,
      thresholdDb: () => report.thresholdDb,
      running: () => wasRunning,
      onRunning: (f) => (listener = f),
      close: () => {
        closed = true;
        clearTimeout(fallback);
        context.onstatechange = null;
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
