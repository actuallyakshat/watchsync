// The call's voice gate, on the audio thread. It lets your voice through and
// keeps typing, mouse clicks and room noise out:
//
// - It opens only for sound that is loud enough AND mostly in the voice band
//   (250–3500 Hz) AND lasts: clicks are short, broadband bursts.
// - "Loud enough" follows the room: the noise floor is the quietest moment
//   of the last ~2 s (speech always has gaps between syllables), so a fan
//   or hum doesn't hold it open and a quiet voice isn't shut out.
// - The output is delayed 30 ms, so the gate opens before your first syllable
//   reaches the output instead of clipping it.
// - It stays open briefly after you stop, and fades rather than cuts.
//
// Settings arrive on the port: { suppress, auto, thresholdDb }. It reports
// { level, open, thresholdDb } about 20 times a second.

const LOOKAHEAD_S = 0.03;
const WINDOW_S = 0.04; // how long sound must last to count as voice
const VOICED_SHARE = 0.7; // of that window
const HOLD_S = 0.35;
const CLOSED_GAIN = 0.015; // about -36 dB: quiet, not dead silence
const FLOOR_MIN_DB = -62;
const FLOOR_PIECE_S = 0.25; // the floor is the minimum over 8 of these
const FLOOR_PIECES = 8;
const ABOVE_FLOOR = 6; // ×6 energy ≈ +8 dB over the room's noise
const VOICE_SHARE = 0.35; // of the energy in the voice band

/** A biquad filter (RBJ cookbook). */
function biquad(type, freq, q) {
  const w = (2 * Math.PI * freq) / sampleRate;
  const a = Math.sin(w) / (2 * q);
  const c = Math.cos(w);
  let b0, b1, b2;
  if (type === "highpass") {
    b0 = (1 + c) / 2;
    b1 = -(1 + c);
    b2 = (1 + c) / 2;
  } else {
    b0 = (1 - c) / 2;
    b1 = 1 - c;
    b2 = (1 - c) / 2;
  }
  const a0 = 1 + a;
  const f = { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: (-2 * c) / a0, a2: (1 - a) / a0, x1: 0, x2: 0, y1: 0, y2: 0 };
  return (x) => {
    const y = f.b0 * x + f.b1 * f.x1 + f.b2 * f.x2 - f.a1 * f.y1 - f.a2 * f.y2;
    f.x2 = f.x1;
    f.x1 = x;
    f.y2 = f.y1;
    f.y1 = y;
    return y;
  };
}

const dbToEnergy = (db) => Math.pow(10, db / 10);
const energyToDb = (e) => 10 * Math.log10(Math.max(e, 1e-12));

class VoiceGate extends AudioWorkletProcessor {
  constructor() {
    super();
    this.suppress = true;
    this.auto = true;
    this.thresholdDb = -50;
    this.port.onmessage = (e) => Object.assign(this, e.data);

    this.lowCut = biquad("highpass", 250, 0.7);
    this.highCut = biquad("lowpass", 3500, 0.7);
    this.floor = dbToEnergy(-70);
    this.pieceMin = Infinity;
    this.pieceBlocks = 0;
    // Nothing below allocates while running: the audio thread must not
    // pause for garbage collection.
    this.mins = new Float64Array(FLOOR_PIECES).fill(Infinity);
    this.minAt = 0;
    this.recent = new Uint8Array(64);
    this.recentAt = 0;
    this.loudCount = 0;
    this.delay = new Float32Array(Math.max(1, Math.round(LOOKAHEAD_S * sampleRate)));
    this.pos = 0;
    this.gain = 0;
    this.holdLeft = 0;
    this.attack = 1 - Math.exp(-1 / (0.005 * sampleRate));
    this.release = 1 - Math.exp(-1 / (0.06 * sampleRate));
    this.sinceReport = 0;
  }

  process(inputs, outputs) {
    const input = inputs[0] && inputs[0][0];
    const output = outputs[0] && outputs[0][0];
    if (!output) return true;
    const n = output.length;
    if (!input) {
      output.fill(0);
      return true;
    }

    let full = 0;
    let voice = 0;
    for (let i = 0; i < n; i++) {
      const x = input[i];
      const v = this.highCut(this.lowCut(x));
      full += x * x;
      voice += v * v;
    }
    full /= n;
    voice /= n;

    // The room's noise floor (minimum statistics): follows a drop at once,
    // a rise smoothly.
    this.pieceMin = Math.min(this.pieceMin, voice);
    if (++this.pieceBlocks >= Math.max(1, Math.round((FLOOR_PIECE_S * sampleRate) / n))) {
      this.mins[this.minAt] = this.pieceMin;
      this.minAt = (this.minAt + 1) % FLOOR_PIECES;
      this.pieceMin = Infinity;
      this.pieceBlocks = 0;
    }
    let now = this.pieceMin;
    for (let i = 0; i < FLOOR_PIECES; i++) if (this.mins[i] < now) now = this.mins[i];
    if (now < this.floor) this.floor = now;
    else this.floor += (now - this.floor) * 0.01;
    this.floor = Math.max(this.floor, dbToEnergy(FLOOR_MIN_DB - 20));

    const threshold = this.auto ? Math.max(this.floor * ABOVE_FLOOR, dbToEnergy(FLOOR_MIN_DB)) : dbToEnergy(this.thresholdDb);
    const loud = voice > threshold && voice > full * VOICE_SHARE;
    // How many of the last `blocks` blocks were loud (a ring, kept counted).
    const blocks = Math.min(this.recent.length, Math.max(1, Math.round((WINDOW_S * sampleRate) / n)));
    const leaving = (this.recentAt - blocks + this.recent.length) % this.recent.length;
    this.loudCount -= this.recent[leaving];
    this.recent[leaving] = 0;
    this.recent[this.recentAt] = loud ? 1 : 0;
    this.loudCount += this.recent[this.recentAt];
    this.recentAt = (this.recentAt + 1) % this.recent.length;
    const voiced = this.loudCount >= VOICED_SHARE * blocks;
    if (voiced) this.holdLeft = HOLD_S * sampleRate;
    else this.holdLeft = Math.max(0, this.holdLeft - n);
    const open = !this.suppress || this.holdLeft > 0;
    const target = open ? 1 : CLOSED_GAIN;

    const d = this.delay;
    for (let i = 0; i < n; i++) {
      const delayed = d[this.pos];
      d[this.pos] = input[i];
      this.pos = (this.pos + 1) % d.length;
      this.gain += (target - this.gain) * (target > this.gain ? this.attack : this.release);
      output[i] = delayed * this.gain;
    }

    this.sinceReport += n;
    if (this.sinceReport >= sampleRate / 20) {
      this.sinceReport = 0;
      // The voice-band level, the same thing the threshold is compared to.
      this.port.postMessage({ level: Math.sqrt(voice), open, thresholdDb: energyToDb(threshold) });
    }
    return true;
  }
}

registerProcessor("voice-gate", VoiceGate);
