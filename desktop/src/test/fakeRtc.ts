// A pretend WebRTC good enough to run negotiation between two meshes in
// tests: descriptions list the transceivers' kinds, tracks are plain objects.
let nextTrack = 1;

export class FakeTrack {
  id = `t${nextTrack++}`;
  readyState = "live";
  onended: (() => void) | null = null;
  enabled = true;
  constructor(public kind: string) {}
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

  constructor() {
    FakePC.all.push(this);
  }
  addTransceiver(kind: string, init: { direction: string }) {
    const t = new FakeTransceiver(kind, init.direction);
    this.transceivers.push(t);
    queueMicrotask(() => this.onnegotiationneeded?.());
    return t;
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
  Object.assign(globalThis, { RTCPeerConnection: FakePC, MediaStream: FakeStream });
}
