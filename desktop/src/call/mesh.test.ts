import { FakePC, FakeTrack, installFakeRtc } from "../test/fakeRtc";
import { vi } from "vitest";
import { GRACE_MS, MAX_VIDEO_BITRATE, Mesh, RETRY_MS, type SignalData } from "./mesh";

beforeEach(() => {
  installFakeRtc();
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
});
afterEach(() => vi.useRealTimers());

/** Meshes for each id, with signalling delivered between them in order
 * (unless `drop` says to lose it, like a room with no host for a moment). */
function room(ids: string[]) {
  const meshes = new Map<string, Mesh>();
  const queue: [string, string, SignalData][] = [];
  let drop: (data: SignalData) => boolean = () => false;
  const add = (id: string) => {
    const m = new Mesh(id, (to, data) => queue.push([id, to, data]), () => {});
    meshes.set(id, m);
    return m;
  };
  for (const id of ids) add(id);
  const deliver = async () => {
    for (let i = 0; i < 50; i++) {
      await new Promise<void>((r) => queueMicrotask(r));
      while (queue.length) {
        const [from, to, data] = queue.shift()!;
        if (!drop(data)) await meshes.get(to)?.onSignal(from, data);
      }
    }
  };
  return { meshes, deliver, queue, add, dropIf: (f: typeof drop) => (drop = f) };
}

const connect = async (r: ReturnType<typeof room>) => {
  for (const [id, m] of r.meshes) m.sync([...r.meshes.keys()].filter((x) => x !== id));
  await r.deliver();
};

test("two people end up sending each other audio and video", async () => {
  const { meshes, deliver } = room(["a", "b"]);
  const mic = new FakeTrack("audio");
  meshes.get("a")!.setTrack("audio", mic as unknown as MediaStreamTrack);
  for (const [id, m] of meshes) m.sync(["a", "b"].filter((x) => x !== id).concat(id));
  await deliver();

  const [pa, pb] = FakePC.all;
  // Only the lower id created transceivers; the other adopted them.
  expect(pa.transceivers.map((t) => t.kind)).toEqual(["audio", "video"]);
  expect(pb.transceivers.map((t) => t.direction)).toEqual(["sendrecv", "sendrecv"]);
  expect(pa.transceivers[0].sender.track).toBe(mic);
  expect(pa.signalingState).toBe("stable");

  const remoteOfA = meshes.get("b")!.remotes()[0];
  expect(remoteOfA.id).toBe("a");
  expect(remoteOfA.stream.getTracks().map((t) => t.kind).sort()).toEqual(["audio", "video"]);

  // B turns the camera on later: no renegotiation, just the track.
  const cam = new FakeTrack("video");
  meshes.get("b")!.setTrack("video", cam as unknown as MediaStreamTrack);
  await deliver();
  expect(pb.transceivers[1].sender.track).toBe(cam);
  expect(pb.transceivers[1].sender.params.encodings[0].maxBitrate).toBe(MAX_VIDEO_BITRATE);
});

test("people who leave are hung up on after a grace period, and a new room starts fresh", async () => {
  const { meshes } = room(["a", "b", "c"]);
  const a = meshes.get("a")!;
  a.sync(["b", "c"]);
  expect(a.remotes().map((r) => r.id).sort()).toEqual(["b", "c"]);
  a.sync(["b"]);
  expect(FakePC.all[1].closed).toBe(false);
  vi.advanceTimersByTime(GRACE_MS);
  expect(FakePC.all[1].closed).toBe(true);
  expect(a.remotes().map((r) => r.id)).toEqual(["b"]);
  a.setMyId("a2");
  expect(a.remotes()).toEqual([]);
  expect(FakePC.all[0].closed).toBe(true);
});

test("a friend whose connection to the host blinks stays in the call", async () => {
  const r = room(["a", "b", "c"]);
  await connect(r);
  const before = FakePC.all.length;
  const a = r.meshes.get("a")!;
  a.sync(["c"]); // b dropped off the host's list…
  vi.advanceTimersByTime(5_000);
  a.sync(["b", "c"]); // …and came back
  vi.advanceTimersByTime(GRACE_MS);
  expect(FakePC.all.length).toBe(before);
  expect(FakePC.all.every((pc) => !pc.closed)).toBe(true);
});

test("a new host's list starts with just them, without hanging up on anyone", async () => {
  const r = room(["a", "b", "c"]);
  await connect(r);
  const b = r.meshes.get("b")!;
  b.sync([]); // b takes over; nobody has reconnected yet
  vi.advanceTimersByTime(3_000);
  b.sync(["c"]);
  vi.advanceTimersByTime(3_000);
  b.sync(["a", "c"]);
  vi.advanceTimersByTime(GRACE_MS);
  expect(FakePC.all.every((pc) => !pc.closed)).toBe(true);
});

test("when the other side starts over, we start over with them", async () => {
  const r = room(["a", "b"]);
  await connect(r);
  const [oldA, oldB] = FakePC.all;
  // a's call window reloads: a fresh connection, offered under a new session.
  oldA.close();
  r.add("a").sync(["b"]);
  await r.deliver();
  expect(oldB.closed).toBe(true);
  const [newA, newB] = FakePC.all.slice(2);
  expect(newA.connectionState).toBe("connected");
  expect(newB.signalingState).toBe("stable");
  expect(r.meshes.get("b")!.remotes()[0].stream.getTracks()).toHaveLength(2);
});

test("an offer lost while the room had no host is retried", async () => {
  const r = room(["a", "b"]);
  r.dropIf((d) => d.description?.type === "offer");
  await connect(r);
  expect(FakePC.all[0].connectionState).toBe("new");
  r.dropIf(() => false);
  vi.advanceTimersByTime(RETRY_MS);
  await r.deliver();
  expect(FakePC.all[0].closed).toBe(true);
  const fresh = FakePC.all.find((pc) => !pc.closed && pc.transceivers.length && pc.localDescription?.type === "offer");
  expect(fresh?.connectionState).toBe("connected");
});

test("signals are handled in order, even when they arrive all at once", async () => {
  const r = room(["a", "b"]);
  r.meshes.get("a")!.sync(["b"]);
  await r.deliver();
  // Replay a's offer and a candidate to a fresh b without waiting in between.
  const fresh = new Mesh("b", () => {}, () => {});
  const offer = { description: FakePC.all[0].localDescription!.toJSON(), session: "s1" };
  const a = fresh.onSignal("a", offer);
  const c = fresh.onSignal("a", { candidate: { candidate: "x" }, session: "s1" });
  await Promise.all([a, c]);
  const pc = FakePC.all.at(-1)!;
  expect(pc.signalingState).toBe("stable");
  expect(pc.candidates).toEqual([{ candidate: "x" }]);
});

test("signals that arrive before the window knows who it is aren't lost", async () => {
  const r = room(["a"]);
  const b = r.add("b");
  b.setMyId(""); // still starting up
  r.meshes.get("a")!.sync(["b"]);
  await r.deliver();
  b.setMyId("b");
  await r.deliver();
  expect(r.meshes.get("a")!.remotes()[0].state).toBe("connected");
});

test("a broken connection looks for a new path", () => {
  const { meshes } = room(["a", "b"]);
  meshes.get("a")!.sync(["b"]);
  const pc = FakePC.all[0];
  pc.connectionState = "failed";
  pc.onconnectionstatechange?.();
  expect(pc.restarts).toBe(1);
});

test("signals for someone not yet listed still connect", async () => {
  const { meshes, deliver } = room(["a", "b"]);
  meshes.get("a")!.sync(["b"]); // b hasn't seen a in the room list yet
  await deliver();
  expect(meshes.get("b")!.remotes().map((r) => r.id)).toEqual(["a"]);
});
