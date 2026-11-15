import { FakePC, FakeTrack, installFakeRtc } from "../test/fakeRtc";
import { MAX_VIDEO_BITRATE, Mesh, type SignalData } from "./mesh";

beforeEach(installFakeRtc);

/** Meshes for each id, with signalling delivered between them in order. */
function room(ids: string[]) {
  const meshes = new Map<string, Mesh>();
  const queue: [string, string, SignalData][] = [];
  for (const id of ids) meshes.set(id, new Mesh(id, (to, data) => queue.push([id, to, data]), () => {}));
  const deliver = async () => {
    for (let i = 0; i < 50 && (queue.length || i < 3); i++) {
      await new Promise((r) => setTimeout(r, 0));
      while (queue.length) {
        const [from, to, data] = queue.shift()!;
        await meshes.get(to)?.onSignal(from, data);
      }
    }
  };
  return { meshes, deliver, queue };
}

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

test("people who leave are hung up on, and a new room starts fresh", async () => {
  const { meshes } = room(["a", "b", "c"]);
  const a = meshes.get("a")!;
  a.sync(["b", "c"]);
  expect(a.remotes().map((r) => r.id).sort()).toEqual(["b", "c"]);
  a.sync(["b"]);
  expect(FakePC.all[1].closed).toBe(true);
  a.setMyId("a2");
  expect(a.remotes()).toEqual([]);
  expect(FakePC.all[0].closed).toBe(true);
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
