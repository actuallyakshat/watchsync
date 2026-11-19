// Plays your Mac's sound (stereo float chunks from the app, see
// shareSound.ts) into the call, with a small buffer against uneven arrival.
// Runs on the audio thread.

/** Start (or restart after running dry) once this much is buffered. */
const PRIME = 0.06;
/** More than this behind: skip ahead, so the sound doesn't drift late. */
const MAX_BEHIND = 0.25;

class PcmPlayer extends AudioWorkletProcessor {
  constructor() {
    super();
    // Interleaved stereo, one second.
    this.buf = new Float32Array(sampleRate * 2);
    this.read = 0;
    this.write = 0;
    this.size = 0; // frames buffered
    this.playing = false;
    this.port.onmessage = (e) => this.push(e.data);
  }

  push(samples) {
    const cap = this.buf.length / 2;
    const frames = samples.length / 2;
    for (let i = 0; i < frames; i++) {
      this.buf[this.write * 2] = samples[i * 2];
      this.buf[this.write * 2 + 1] = samples[i * 2 + 1];
      this.write = (this.write + 1) % cap;
    }
    this.size = Math.min(cap, this.size + frames);
    if (this.size === cap) this.read = this.write;
    const behind = Math.floor(MAX_BEHIND * sampleRate);
    if (this.size > behind) {
      const skip = this.size - Math.floor(PRIME * sampleRate);
      this.read = (this.read + skip) % cap;
      this.size -= skip;
    }
  }

  process(_inputs, outputs) {
    const [left, right] = outputs[0];
    if (!left) return true;
    if (!this.playing && this.size >= PRIME * sampleRate) this.playing = true;
    const cap = this.buf.length / 2;
    for (let i = 0; i < left.length; i++) {
      if (!this.playing || this.size === 0) {
        this.playing = false;
        left[i] = 0;
        if (right) right[i] = 0;
        continue;
      }
      left[i] = this.buf[this.read * 2];
      if (right) right[i] = this.buf[this.read * 2 + 1];
      this.read = (this.read + 1) % cap;
      this.size--;
    }
    return true;
  }
}

registerProcessor("pcm-player", PcmPlayer);
