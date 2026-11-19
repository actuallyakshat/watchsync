// The real PCM player (your Mac's sound, into the call), on a pretend audio
// thread.
const RATE = 48000;
const BLOCK = 128;

type Player = {
  process(i: Float32Array[][], o: Float32Array[][]): boolean;
  port: { onmessage: ((e: { data: unknown }) => void) | null };
};
let Processor: new () => Player;

beforeAll(async () => {
  class Base {
    port = { postMessage: () => {}, onmessage: null };
  }
  Object.assign(globalThis, {
    sampleRate: RATE,
    AudioWorkletProcessor: Base,
    registerProcessor: (_: string, p: new () => Player) => (Processor = p),
  });
  await import("./pcmPlayer.worklet.js");
});

/** `ms` of stereo frames, left = `value`, right = -`value`. */
const chunk = (ms: number, value = 0.5) => {
  const n = (RATE * ms) / 1000;
  const s = new Float32Array(n * 2);
  for (let i = 0; i < n; i++) {
    s[2 * i] = value;
    s[2 * i + 1] = -value;
  }
  return s;
};
const push = (p: Player, s: Float32Array) => p.port.onmessage!({ data: s });
/** One block out: [left, right]. */
const block = (p: Player) => {
  const out = [new Float32Array(BLOCK), new Float32Array(BLOCK)];
  p.process([], [out]);
  return out;
};

test("waits for a little buffer, then plays both channels", () => {
  const p = new Processor();
  push(p, chunk(20));
  expect(block(p)[0][0]).toBe(0); // 20 ms isn't enough to start
  push(p, chunk(60));
  const [l, r] = block(p);
  expect(l[0]).toBe(0.5);
  expect(r[0]).toBe(-0.5);
});

test("running dry plays silence, and waits again before restarting", () => {
  const p = new Processor();
  push(p, chunk(70));
  for (let i = 0; i < Math.ceil((RATE * 0.07) / BLOCK); i++) block(p);
  expect(block(p)[0][0]).toBe(0);
  push(p, chunk(20));
  expect(block(p)[0][0]).toBe(0); // not primed yet
});

test("falling far behind skips ahead to stay close to live", () => {
  const p = new Processor();
  push(p, chunk(400, 0.25)); // old
  push(p, chunk(40, 0.75)); // newest
  // Of 400 ms queued, only about the last 60 ms still plays before it.
  const played: number[] = [];
  for (let i = 0; i < 40; i++) played.push(...block(p)[0]);
  const first = played.indexOf(0.75);
  expect(first).toBeGreaterThan(0);
  expect(first).toBeLessThanOrEqual(RATE * 0.065);
});

export {};
