// The real voice gate, run on a pretend audio thread with made-up sounds.
const RATE = 48000;
const BLOCK = 128;

type Gate = {
  process(i: Float32Array[][], o: Float32Array[][]): boolean;
  port: { postMessage: (m: unknown) => void; onmessage: ((e: { data: unknown }) => void) | null };
};
let Processor: new () => Gate;

beforeAll(async () => {
  class Base {
    port = { postMessage: () => {}, onmessage: null };
  }
  Object.assign(globalThis, {
    sampleRate: RATE,
    AudioWorkletProcessor: Base,
    registerProcessor: (_: string, p: new () => Gate) => (Processor = p),
  });
  await import("./voiceGate.worklet.js");
});

/** Deterministic noise. */
function noise(seed = 1) {
  let s = seed;
  return () => {
    s = (s * 1103515245 + 12345) % 2 ** 31;
    return (s / 2 ** 31) * 2 - 1;
  };
}

const after = (frames: number[], from: number) => frames.slice(from);
const loudest = (frames: number[]) => Math.max(...frames);

/** Run `sound(t)` through a fresh gate; how much got out, per 10 ms. */
function run(sound: (t: number) => number, seconds: number, settings?: object) {
  const g = new Processor();
  if (settings) g.port.onmessage?.({ data: settings });
  const out: number[] = [];
  let t = 0;
  let acc = 0;
  let n = 0;
  for (let b = 0; b < (seconds * RATE) / BLOCK; b++) {
    const input = new Float32Array(BLOCK);
    for (let i = 0; i < BLOCK; i++) input[i] = sound((t + i) / RATE);
    const output = new Float32Array(BLOCK);
    g.process([[input]], [[output]]);
    for (let i = 0; i < BLOCK; i++) {
      acc += output[i] ** 2;
      if (++n === RATE / 100) {
        out.push(Math.sqrt(acc / n));
        acc = n = 0;
      }
    }
    t += BLOCK;
  }
  return out;
}

/** A voice-ish sound: harmonics in the voice band, in syllables with gaps. */
const voice = (t: number) => {
  const syllables = Math.max(0, Math.sin(2 * Math.PI * 3 * t)) ** 0.5;
  return 0.08 * syllables * (Math.sin(2 * Math.PI * 220 * t) + 0.7 * Math.sin(2 * Math.PI * 660 * t) + 0.5 * Math.sin(2 * Math.PI * 1100 * t));
};

/** Share of the sound's energy the gate lets out over 10 ms frames [from, to). */
function share(sound: (t: number) => number, seconds: number, from: number, to?: number, settings: object = {}) {
  const gated = run(sound, seconds, settings).slice(from, to);
  const open = run(sound, seconds, { ...settings, suppress: false }).slice(from, to);
  const e = (f: number[]) => f.reduce((a, x) => a + x * x, 0);
  return e(gated) / e(open);
}

test("a voice gets through", () => {
  expect(share(voice, 2, 30)).toBeGreaterThan(0.95);
});

test("typing and clicks stay out", () => {
  const n = noise(7);
  // Sharp 6 ms broadband clicks, loud, every 120 ms (fast typing).
  const clicks = (t: number) => (t % 0.12 < 0.006 ? 0.5 * n() : 0.0005 * n());
  expect(share(clicks, 3, 50)).toBeLessThan(0.01);
});

test("a fan or hum doesn't hold it open, and a voice over it still gets through", () => {
  const fan = noise(3);
  expect(share(() => 0.01 * fan(), 4, 100)).toBeLessThan(0.01);
  const hum = (t: number) => 0.02 * Math.sin(2 * Math.PI * 120 * t);
  expect(share(hum, 4, 250)).toBeLessThan(0.01);

  const f2 = noise(3);
  const talkOverFan = (t: number) => 0.01 * f2() + (t > 4 ? voice(t) : 0);
  expect(share(talkOverFan, 6, 430)).toBeGreaterThan(0.9);
});

test("it stays open a moment after you stop, then closes", () => {
  const n = noise(9);
  const sound = (t: number) => (t < 1 ? voice(t) : 0) + 0.003 * n();
  const out = run(sound, 2);
  // (The room sound is 0.0017 RMS: sent in full, or at 1.5% once closed.)
  expect(out[110]).toBeGreaterThan(0.0015); // 100 ms after: still sent
  expect(loudest(after(out, 160))).toBeLessThan(2e-4); // 600 ms after: closed
});

test("with suppression off, everything passes", () => {
  const n = noise(5);
  const out = run((t) => (t % 0.12 < 0.006 ? 0.5 * n() : 0), 2, { suppress: false });
  expect(loudest(after(out, 20))).toBeGreaterThan(0.1);
});

test("a quiet voice gets through on automatic, and a high manual threshold ignores it", () => {
  const quiet = (t: number) => voice(t) * 0.1;
  expect(share(quiet, 3, 50)).toBeGreaterThan(0.9);
  expect(share(quiet, 3, 50, undefined, { auto: false, thresholdDb: -20 })).toBeLessThan(0.01);
});
