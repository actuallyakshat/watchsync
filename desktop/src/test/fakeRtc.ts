// A pretend WebRTC good enough to run negotiation between two meshes in
// tests: descriptions list the transceivers' kinds, tracks are plain objects.
let nextTrack = 1;

export class FakeTrack {
  id = `t${nextTrack++}`;
  readyState = "live";
  onended: (() => void) | null = null;
  enabled = true;
  constructor(public kind: string) {}
  getSettings() {
    return this.kind === "video" ? { width: 1920, height: 1080 } : {};
  }
  stop() {
    this.readyState = "ended";
  }
}

export class FakeStream {
  private tracks: FakeTrack[];
  constructor(tracks: FakeTrack[] = []) {
    this.tracks = [...tracks];
  }
  getTracks() {
    return [...this.tracks];
  }
  getAudioTracks() {
    return this.tracks.filter((t) => t.kind === "audio");
  }
  getVideoTracks() {
    return this.tracks.filter((t) => t.kind === "video");
  }
  addTrack(t: FakeTrack) {
    this.tracks.push(t);
  }
  removeTrack(t: FakeTrack) {
    this.tracks = this.tracks.filter((x) => x !== t);
  }
}

export class FakeChannel {
  readyState = "connecting";
  sent: string[] = [];
  other: FakeChannel | null = null;
  onopen: (() => void) | null = null;
  onmessage: ((e: { data: string }) => void) | null = null;
  constructor(public id: number) {}
  send(data: string) {
    this.sent.push(data);
    const o = this.other;
    if (o) queueMicrotask(() => o.onmessage?.({ data }));
  }
  close() {
    this.readyState = "closed";
  }
}

class FakeSender {
  track: FakeTrack | null = null;
  params: { encodings: Record<string, unknown>[] } = { encodings: [{}] };
  async replaceTrack(t: FakeTrack | null) {
    this.track = t;
  }
  getParameters() {
    return this.params;
  }
  async setParameters(p: { encodings: Record<string, unknown>[] }) {
    this.params = p;
  }
}

class FakeTransceiver {
  sender = new FakeSender();
  receiver: { track: FakeTrack; getSynchronizationSources: () => { audioLevel: number }[] };
  constructor(
    public kind: string,
    public direction: string,
  ) {
    this.receiver = { track: new FakeTrack(kind), getSynchronizationSources: () => [] };
  }
}

type Desc = { type: "offer" | "answer"; sdp: string };

export class FakePC {
  static all: FakePC[] = [];
  signalingState = "stable";
  connectionState = "new";
  localDescription: (Desc & { toJSON: () => Desc }) | null = null;
  transceivers: FakeTransceiver[] = [];
  candidates: unknown[] = [];
  closed = false;
  onicecandidate: ((e: { candidate: null }) => void) | null = null;
  onnegotiationneeded: (() => void) | null = null;
  ontrack: ((e: { track: FakeTrack }) => void) | null = null;
  onconnectionstatechange: (() => void) | null = null;
  restarts = 0;
  channels: FakeChannel[] = [];
  private needPending = false;
  private remoteSet = false;

  /** What getStats reports as the chosen path ("host", "relay"…). */
  static candidateType: string | null = null;
  constructor(public config: RTCConfiguration = {}) {
    FakePC.all.push(this);
  }
  async getStats() {
    const t = FakePC.candidateType;
    const m = new Map<string, Record<string, unknown>>();
    if (t && this.connectionState === "connected") {
      m.set("T", { id: "T", type: "transport", selectedCandidatePairId: "P" });
      m.set("P", { id: "P", type: "candidate-pair", localCandidateId: "L", nominated: true, state: "succeeded" });
      m.set("L", { id: "L", type: "local-candidate", candidateType: t });
    }
    return m;
  }
  addTransceiver(kind: string, init: { direction: string }) {
    const t = new FakeTransceiver(kind, init.direction);
    this.transceivers.push(t);
    // Like browsers: several changes in a row need one negotiation.
    if (!this.needPending) {
      this.needPending = true;
      queueMicrotask(() => {
        this.needPending = false;
        this.onnegotiationneeded?.();
      });
    }
    return t;
  }
  createDataChannel(_label: string, init: { id: number }) {
    const c = new FakeChannel(init.id);
    this.channels.push(c);
    return c;
  }
  /** Connect two peers' negotiated channels, as if the connection came up. */
  static openChannels(a: FakePC, b: FakePC) {
    for (const ca of a.channels) {
      const cb = b.channels.find((c) => c.id === ca.id);
      if (!cb) continue;
      ca.other = cb;
      cb.other = ca;
      ca.readyState = cb.readyState = "open";
      ca.onopen?.();
      cb.onopen?.();
    }
  }
  getTransceivers() {
    return this.transceivers;
  }
  async setLocalDescription() {
    const type = this.signalingState === "have-remote-offer" ? "answer" : "offer";
    const desc: Desc = { type, sdp: this.transceivers.map((t) => t.kind).join(",") };
    this.localDescription = { ...desc, toJSON: () => desc };
    this.signalingState = type === "offer" ? "have-local-offer" : "stable";
  }
  async setRemoteDescription(d: Desc) {
    await Promise.resolve(); // takes a moment, like the real thing
    this.remoteSet = true;
    const kinds = d.sdp.split(",").filter(Boolean);
    if (d.type === "offer") {
      kinds.forEach((kind, i) => {
        if (!this.transceivers[i]) this.transceivers.push(new FakeTransceiver(kind, "recvonly"));
      });
      this.signalingState = "have-remote-offer";
    } else {
      this.signalingState = "stable";
      this.connectionState = "connected";
    }
    for (const t of this.transceivers) this.ontrack?.({ track: t.receiver.track });
  }
  async addIceCandidate(c: unknown) {
    if (!this.remoteSet) throw new DOMException("no remote description", "InvalidStateError");
    this.candidates.push(c);
  }
  restartIce() {
    this.restarts++;
  }
  close() {
    this.closed = true;
  }
}

export function installFakeRtc() {
  FakePC.all = [];
  FakePC.candidateType = null;
  Object.assign(globalThis, { RTCPeerConnection: FakePC, MediaStream: FakeStream });
}
