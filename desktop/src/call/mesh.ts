// The call's media: one RTCPeerConnection to every other person in the room
// (a mesh; fine for the 2–4 people WatchSync calls are for). Signalling
// (offers, answers, ICE candidates) travels through the engine and the
// room's host; the media goes straight between Macs.
//
// Negotiation follows the "perfect negotiation" pattern, so either side may
// renegotiate at any time without glare. Whoever has the lower endpoint id
// creates the audio and video transceivers; the other side adopts the ones
// that arrive with the first offer. Turning the mic or camera on or off only
// swaps the track on an existing transceiver, so it never renegotiates.

export interface SignalData {
  description?: RTCSessionDescriptionInit;
  candidate?: RTCIceCandidateInit | null;
}

export type Kind = "audio" | "video";

export interface Remote {
  id: string;
  /** Their audio and video, as received (tracks may be silent or black). */
  stream: MediaStream;
  state: RTCPeerConnectionState;
  /** Bumped whenever `stream`'s tracks change, so views can reattach. */
  version: number;
  /** The receiver of their audio, for speaking levels. */
  audio: RTCRtpReceiver | null;
}

export const ICE_SERVERS: RTCIceServer[] = [{ urls: "stun:stun.l.google.com:19302" }];

/** Sent at most this fast; adaptive bitrate works below it. */
export const MAX_VIDEO_BITRATE = 2_500_000;

type Send = (to: string, data: SignalData) => void;

export class Mesh {
  private links = new Map<string, Link>();
  private tracks: Record<Kind, MediaStreamTrack | null> = { audio: null, video: null };

  constructor(
    private myId: string,
    private signal: Send,
    private onChange: () => void,
    private config: RTCConfiguration = { iceServers: ICE_SERVERS },
  ) {}

  /** A new room (or none): drop every connection made as the old self. */
  setMyId(id: string) {
    if (id === this.myId) return;
    this.closeLinks();
    this.myId = id;
  }

  /** Keep a connection to exactly these people (endpoint ids). */
  sync(ids: string[]) {
    if (!this.myId) return;
    const wanted = new Set(ids.filter((id) => id && id !== this.myId));
    for (const [id, link] of this.links) {
      if (!wanted.has(id)) {
        link.close();
        this.links.delete(id);
      }
    }
    for (const id of wanted) this.link(id);
    this.onChange();
  }

  async onSignal(from: string, data: SignalData) {
    if (!this.myId || !from || from === this.myId) return;
    await this.link(from).onSignal(data);
  }

  /** Send this track to everyone (null = stop sending). */
  setTrack(kind: Kind, track: MediaStreamTrack | null) {
    this.tracks[kind] = track;
    for (const link of this.links.values()) void link.send(kind, track);
  }

  track(kind: Kind): MediaStreamTrack | null {
    return this.tracks[kind];
  }

  remotes(): Remote[] {
    return [...this.links.values()].map((l) => l.view());
  }

  close() {
    this.closeLinks();
    this.myId = "";
  }

  private closeLinks() {
    for (const link of this.links.values()) link.close();
    this.links.clear();
    this.onChange();
  }

  private link(id: string): Link {
    let link = this.links.get(id);
    if (!link) {
      link = new Link(id, this.myId < id, this.config, this.signal, this.onChange, (k) => this.tracks[k]);
      this.links.set(id, link);
    }
    return link;
  }
}

class Link {
  private pc: RTCPeerConnection;
  private makingOffer = false;
  private ignoreOffer = false;
  private stream = new MediaStream();
  private version = 0;
  private tx: Record<Kind, RTCRtpTransceiver | null> = { audio: null, video: null };
  private closed = false;
  /** Impolite peers win offer collisions; the polite one rolls back. */
  private polite: boolean;

  constructor(
    private id: string,
    initiator: boolean,
    config: RTCConfiguration,
    private signal: Send,
    private onChange: () => void,
    private local: (k: Kind) => MediaStreamTrack | null,
  ) {
    this.polite = !initiator;
    this.pc = new RTCPeerConnection(config);
    this.pc.onicecandidate = ({ candidate }) => this.signal(id, { candidate: candidate?.toJSON() ?? null });
    this.pc.onnegotiationneeded = () => void this.offer();
    this.pc.ontrack = ({ track }) => {
      for (const t of this.stream.getTracks()) if (t.kind === track.kind && t !== track) this.stream.removeTrack(t);
      if (!this.stream.getTracks().includes(track)) this.stream.addTrack(track);
      this.version++;
      this.onChange();
    };
    this.pc.onconnectionstatechange = () => {
      // A network change (Wi-Fi to Ethernet, a VPN) can break the path:
      // look for a new one rather than give up.
      if (this.pc.connectionState === "failed" && !this.closed) this.pc.restartIce();
      this.onChange();
    };
    if (initiator) {
      for (const kind of ["audio", "video"] as Kind[]) {
        const t = this.pc.addTransceiver(kind, { direction: "sendrecv" });
        this.tx[kind] = t;
        void this.send(kind, this.local(kind));
      }
      preferHardwareVideo(this.tx.video);
    }
  }

  view(): Remote {
    return {
      id: this.id,
      stream: this.stream,
      state: this.pc.connectionState,
      version: this.version,
      audio: this.tx.audio?.receiver ?? null,
    };
  }

  async send(kind: Kind, track: MediaStreamTrack | null) {
    const t = this.tx[kind];
    if (!t || this.closed) return; // adopted later, with the current track
    try {
      await t.sender.replaceTrack(track);
      if (kind === "video" && track) await capBitrate(t.sender);
    } catch (e) {
      console.warn("couldn't switch the call's", kind, e);
    }
  }

  async onSignal({ description, candidate }: SignalData) {
    if (this.closed) return;
    try {
      if (description) {
        const collision =
          description.type === "offer" && (this.makingOffer || this.pc.signalingState !== "stable");
        this.ignoreOffer = !this.polite && collision;
        if (this.ignoreOffer) return;
        await this.pc.setRemoteDescription(description);
        if (description.type === "offer") {
          this.adopt();
          await this.pc.setLocalDescription();
          this.signal(this.id, { description: this.pc.localDescription!.toJSON() });
        }
      } else if (candidate !== undefined) {
        try {
          await this.pc.addIceCandidate(candidate ?? undefined);
        } catch (e) {
          if (!this.ignoreOffer) throw e;
        }
      }
    } catch (e) {
      console.warn("call signalling with", this.id, "failed", e);
    }
  }

  close() {
    this.closed = true;
    this.pc.onicecandidate = this.pc.onnegotiationneeded = this.pc.ontrack = this.pc.onconnectionstatechange = null;
    this.pc.close();
  }

  private async offer() {
    if (this.closed) return;
    try {
      this.makingOffer = true;
      await this.pc.setLocalDescription();
      this.signal(this.id, { description: this.pc.localDescription!.toJSON() });
    } catch (e) {
      console.warn("call offer to", this.id, "failed", e);
    } finally {
      this.makingOffer = false;
    }
  }

  /** The answering side: use the transceivers the offer created. */
  private adopt() {
    for (const t of this.pc.getTransceivers()) {
      const kind = t.receiver.track.kind as Kind;
      if ((kind !== "audio" && kind !== "video") || this.tx[kind]) continue;
      this.tx[kind] = t;
      t.direction = "sendrecv";
      if (kind === "video") preferHardwareVideo(t);
      void this.send(kind, this.local(kind));
    }
  }
}

/** H.264 first: Macs encode and decode it in hardware. */
function preferHardwareVideo(t: RTCRtpTransceiver | null) {
  const caps = typeof RTCRtpReceiver !== "undefined" ? RTCRtpReceiver.getCapabilities?.("video") : null;
  if (!t || !caps || !t.setCodecPreferences) return;
  const h264 = caps.codecs.filter((c) => c.mimeType.toLowerCase() === "video/h264");
  const rest = caps.codecs.filter((c) => c.mimeType.toLowerCase() !== "video/h264");
  try {
    t.setCodecPreferences([...h264, ...rest]);
  } catch {
    // Keep the browser's order.
  }
}

async function capBitrate(sender: RTCRtpSender) {
  const p = sender.getParameters();
  if (!p.encodings?.length) p.encodings = [{}];
  p.encodings[0].maxBitrate = MAX_VIDEO_BITRATE;
  p.encodings[0].maxFramerate = 30;
  await sender.setParameters(p);
}
