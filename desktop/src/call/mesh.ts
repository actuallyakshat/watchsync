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
//
// A third transceiver (the second video one) carries a shared screen. It
// stays receive-only until you share: apps older than screen sharing treat
// any second video they're sent as the camera, so it's sent only while
// someone is actually sharing. Starting or stopping a share renegotiates.
//
// The call must outlive the room's own trouble: a guest's connection blinking
// or the host handing over briefly shortens the member list, but the media
// path is direct and fine. So a person missing from the list is hung up on
// only after a grace period. Each connection has a session id, so an offer
// from a replaced connection (the other side reloaded, or gave up and
// started over) replaces ours too, and stale answers and candidates are
// dropped. The side that offers retries if a connection never comes up
// (an offer lost while the room had no host).

export interface SignalData {
  description?: RTCSessionDescriptionInit;
  candidate?: RTCIceCandidateInit | null;
  /** The sender's connection, and the receiver's as the sender knows it. */
  session?: string;
  to_session?: string | null;
  /** "I started over (new settings): start over too." */
  restart?: boolean;
}

/** How a friend's media reaches us: straight, or relayed through iroh. */
export type Path = "direct" | "relay";

/** Someone missing from the room list this long is hung up on. */
export const GRACE_MS = 45_000;
/** The offering side starts over if not connected this long after offering. */
export const RETRY_MS = 12_000;
/** Longest wait between retries for someone who never answers (an older app). */
export const MAX_RETRY_MS = 120_000;
/** Signals kept while we don't know our own id yet (the window is starting). */
const EARLY_SIGNALS = 100;

export type Kind = "audio" | "video" | "screen";

export interface Remote {
  id: string;
  /** Their audio and camera, as received (tracks may be silent or black). */
  stream: MediaStream;
  /** Their shared screen (black or absent unless they're sharing). */
  screen: MediaStream;
  state: RTCPeerConnectionState;
  /** Bumped whenever `stream`'s tracks change, so views can reattach. */
  version: number;
  /** The receiver of their audio, for speaking levels. */
  audio: RTCRtpReceiver | null;
}

export const ICE_SERVERS: RTCIceServer[] = [{ urls: "stun:stun.l.google.com:19302" }];

/** Sent at most this fast; adaptive bitrate works below it. */
export const MAX_VIDEO_BITRATE = 2_500_000;
/** Bitrate floor for a small but visible picture. */
export const MIN_VIDEO_BITRATE = 150_000;
const FULL_HD = 1920 * 1080;
/** A shared screen: sharp text matters more than smooth motion. */
export const SCREEN_BITRATE = 4_000_000;
export const SCREEN_FRAMERATE = 30;

export interface Size {
  w: number;
  h: number;
}

/** How to encode our camera for someone showing it at `want` device pixels
 * (null: they haven't said, so send everything; 0×0: they aren't looking).
 * Tiles crop to fill, so the picture must cover the tile in both directions. */
export function encodingFor(src: Size | null, want: Size | null): { active: boolean; scale: number; maxBitrate: number } {
  if (want && (want.w <= 0 || want.h <= 0)) return { active: false, scale: 1, maxBitrate: MIN_VIDEO_BITRATE };
  if (!want || !src || src.w <= 0 || src.h <= 0) return { active: true, scale: 1, maxBitrate: MAX_VIDEO_BITRATE };
  const scale = Math.max(1, Math.min(src.w / want.w, src.h / want.h));
  const pixels = (src.w / scale) * (src.h / scale);
  // Bitrate grows a little slower than the pixel count.
  const maxBitrate = Math.round(Math.min(MAX_VIDEO_BITRATE, Math.max(MIN_VIDEO_BITRATE, MAX_VIDEO_BITRATE * (pixels / FULL_HD) ** 0.75)));
  return { active: true, scale: Math.round(scale * 100) / 100, maxBitrate };
}

/** Voice bitrate. Opus defaults to ~32 kbps, which sounds like a phone;
 * Discord sends about 64. Even four people stay well under 1 Mbps. */
export const VOICE_BITRATE = 96_000;
const OPUS_PARAMS: Record<string, string> = {
  maxaveragebitrate: String(VOICE_BITRATE),
  maxplaybackrate: "48000",
  "sprop-maxcapturerate": "48000",
  useinbandfec: "1",
};

/** Asks the other side to send full-band, high-bitrate Opus. Applied to
 * the description we *receive*: WebRTC sets up our encoder from their
 * Opus parameters, so this is what changes the sound we send. */
export function richOpus(sdp: string): string {
  const eol = sdp.includes("\r\n") ? "\r\n" : "\n";
  const lines = sdp.split(eol);
  const pts = new Set<string>();
  for (const l of lines) {
    const m = /^a=rtpmap:(\d+) opus\/48000/i.exec(l);
    if (m) pts.add(m[1]);
  }
  if (!pts.size) return sdp;
  const fmtp = (pt: string, existing: string) => {
    const params = new Map<string, string>();
    for (const kv of existing.split(";")) {
      const [k, v = ""] = kv.trim().split("=");
      if (k) params.set(k, v);
    }
    for (const [k, v] of Object.entries(OPUS_PARAMS)) params.set(k, v);
    return `a=fmtp:${pt} ${[...params].map(([k, v]) => `${k}=${v}`).join(";")}`;
  };
  const withFmtp = new Set<string>();
  const out = lines.map((l) => {
    const m = /^a=fmtp:(\d+) (.*)$/.exec(l);
    if (!m || !pts.has(m[1])) return l;
    withFmtp.add(m[1]);
    return fmtp(m[1], m[2]);
  });
  return out
    .flatMap((l) => {
      const m = /^a=rtpmap:(\d+) /.exec(l);
      return m && pts.has(m[1]) && !withFmtp.has(m[1]) ? [l, fmtp(m[1], "")] : [l];
    })
    .join(eol);
}

/** Round a tile's size so tiny resizes don't renegotiate quality. */
export function roundSize({ w, h }: Size): Size {
  const step = (n: number) => (n <= 0 ? 0 : Math.ceil(n / 64) * 64);
  return { w: step(w), h: step(h) };
}

type Send = (to: string, data: SignalData) => void;

export class Mesh {
  private links = new Map<string, Link>();
  private tracks: Record<Kind, MediaStreamTrack | null> = { audio: null, video: null, screen: null };
  private leaving = new Map<string, ReturnType<typeof setTimeout>>();
  private early: [string, SignalData][] = [];
  /** Signals are handled one at a time, in order: a candidate must not
   * overtake the offer it belongs to. */
  private chain: Promise<void> = Promise.resolve();

  constructor(
    private myId: string,
    private signal: Send,
    private onChange: () => void,
    private config: RTCConfiguration = { iceServers: ICE_SERVERS },
  ) {}

  /** ICE servers and policy for connections. Changing them starts every
   * connection over (the other side is told to, too). */
  setConfig(config: RTCConfiguration) {
    if (JSON.stringify(config) === JSON.stringify(this.config)) return;
    this.config = config;
    for (const id of [...this.links.keys()]) {
      this.links.get(id)!.close();
      this.links.delete(id);
      const fresh = this.link(id);
      // The offering side's fresh offer tells the other side; the answering
      // side has to ask.
      if (!(this.myId < id)) this.signal(id, { restart: true, session: fresh.session });
    }
    this.onChange();
  }

  /** How each friend's media gets here, as far as WebRTC says. */
  async paths(): Promise<Map<string, Path | null>> {
    const out = new Map<string, Path | null>();
    await Promise.all([...this.links].map(async ([id, l]) => out.set(id, await l.path())));
    return out;
  }

  /** A new room (or none): drop every connection made as the old self. */
  setMyId(id: string) {
    if (id === this.myId) return;
    this.closeLinks();
    this.myId = id;
    const early = this.early;
    this.early = [];
    for (const [from, data] of early) void this.onSignal(from, data);
  }

  /** Keep a connection to these people (endpoint ids); hang up on the rest
   * once they've been gone for the grace period. */
  sync(ids: string[]) {
    if (!this.myId) return;
    const wanted = new Set(ids.filter((id) => id && id !== this.myId));
    for (const id of this.links.keys()) {
      if (wanted.has(id) || this.leaving.has(id)) continue;
      this.leaving.set(
        id,
        setTimeout(() => {
          this.leaving.delete(id);
          this.links.get(id)?.close();
          this.links.delete(id);
          this.onChange();
        }, GRACE_MS),
      );
    }
    for (const id of wanted) {
      const timer = this.leaving.get(id);
      if (timer !== undefined) {
        clearTimeout(timer);
        this.leaving.delete(id);
      }
      this.link(id);
    }
    this.onChange();
  }

  onSignal(from: string, data: SignalData): Promise<void> {
    this.chain = this.chain.then(() => this.handle(from, data)).catch((e) => console.warn("call signal failed", e));
    return this.chain;
  }

  private async handle(from: string, data: SignalData) {
    if (!from || from === this.myId) return;
    if (!this.myId) {
      if (this.early.length < EARLY_SIGNALS) this.early.push([from, data]);
      return;
    }
    let link = this.link(from);
    if (data.restart) {
      link.close();
      this.links.delete(from);
      this.link(from);
      this.onChange();
      return;
    }
    // Their side started a new connection: start ours over to match.
    if (data.description?.type === "offer" && data.session && link.remoteSession && data.session !== link.remoteSession) {
      link.close();
      this.links.delete(from);
      link = this.link(from);
    }
    await link.onSignal(data);
  }

  /** How big we're showing this person's video, in device pixels (0×0:
   * not at all). They scale what they send to match. */
  setWanted(id: string, size: Size) {
    this.links.get(id)?.want(roundSize(size));
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
    for (const timer of this.leaving.values()) clearTimeout(timer);
    this.leaving.clear();
    for (const link of this.links.values()) link.close();
    this.links.clear();
    this.onChange();
  }

  private link(id: string, retryAfter = RETRY_MS): Link {
    let link = this.links.get(id);
    if (!link) {
      const restart = (wait: number) => {
        if (this.links.get(id) !== link) return;
        link!.close();
        this.links.delete(id);
        this.link(id, Math.min(wait * 2, MAX_RETRY_MS));
        this.onChange();
      };
      link = new Link(id, this.myId < id, this.config, this.signal, this.onChange, (k) => this.tracks[k], retryAfter, restart);
      this.links.set(id, link);
    }
    return link;
  }
}

class Link {
  readonly session = randomId();
  /** The other side's connection, once heard from. */
  remoteSession: string | null = null;
  private watchdog: ReturnType<typeof setTimeout> | null = null;
  private pc: RTCPeerConnection;
  private makingOffer = false;
  private ignoreOffer = false;
  private stream = new MediaStream();
  private screen = new MediaStream();
  private version = 0;
  private tx: Record<Kind, RTCRtpTransceiver | null> = { audio: null, video: null, screen: null };
  /** The other side's first description has been applied (the answering
   * side has adopted the offer's transceivers). */
  private adopted = false;
  private closed = false;
  /** Size messages travel here (both sides create it as channel 0). */
  private channel: RTCDataChannel | null = null;
  /** How big we show them / they show us. */
  private wanted: Size | null = null;
  private theirWant: Size | null = null;
  /** setParameters calls must not overlap. */
  private encoding: Promise<void> = Promise.resolve();
  /** Impolite peers win offer collisions; the polite one rolls back. */
  private polite: boolean;

  constructor(
    private id: string,
    initiator: boolean,
    config: RTCConfiguration,
    private signal: Send,
    private onChange: () => void,
    private local: (k: Kind) => MediaStreamTrack | null,
    private retryAfter: number,
    private restart: (wait: number) => void,
  ) {
    this.polite = !initiator;
    this.pc = new RTCPeerConnection(config);
    this.pc.onicecandidate = ({ candidate }) => this.send_({ candidate: candidate?.toJSON() ?? null });
    this.pc.onnegotiationneeded = () => void this.offer();
    this.pc.ontrack = ({ track, transceiver }) => {
      // The first video is the camera, any later one a shared screen (both
      // sides see the transceivers in the same, m-line order).
      const videos = this.pc.getTransceivers().filter((t) => t.receiver.track.kind === "video");
      const into = track.kind === "video" && transceiver && videos.indexOf(transceiver) > 0 ? this.screen : this.stream;
      for (const t of into.getTracks()) if (t.kind === track.kind && t !== track) into.removeTrack(t);
      if (!into.getTracks().includes(track)) into.addTrack(track);
      this.version++;
      this.onChange();
    };
    this.pc.onconnectionstatechange = () => {
      // A network change (Wi-Fi to Ethernet, a VPN) can break the path:
      // look for a new one rather than give up.
      if (this.pc.connectionState === "failed" && !this.closed) this.pc.restartIce();
      if (this.pc.connectionState === "connected") this.retryAfter = RETRY_MS;
      this.onChange();
    };
    if (initiator) {
      for (const kind of ["audio", "video"] as Kind[]) {
        const t = this.pc.addTransceiver(kind, { direction: "sendrecv" });
        this.tx[kind] = t;
        void this.send(kind, this.local(kind));
      }
      this.tx.screen = this.pc.addTransceiver("video", { direction: this.local("screen") ? "sendrecv" : "recvonly" });
      void this.send("screen", this.local("screen"));
      preferHardwareVideo(this.tx.video);
      preferHardwareVideo(this.tx.screen);
      this.openChannel();
      this.adopted = true;
    }
  }

  want(size: Size) {
    if (this.wanted && this.wanted.w === size.w && this.wanted.h === size.h) return;
    this.wanted = size;
    this.tell();
  }

  private openChannel() {
    if (this.channel || typeof this.pc.createDataChannel !== "function") return;
    try {
      const c = this.pc.createDataChannel("watchsync", { negotiated: true, id: 0 });
      c.onopen = () => this.tell();
      c.onmessage = (e) => {
        try {
          const m = JSON.parse(String(e.data));
          if (m && Number.isFinite(m.want?.w) && Number.isFinite(m.want?.h)) {
            this.theirWant = { w: m.want.w, h: m.want.h };
            this.encode();
          }
        } catch {
          // Not ours.
        }
      };
      this.channel = c;
    } catch (e) {
      console.warn("no size channel to", this.id, e);
    }
  }

  private tell() {
    if (this.wanted && this.channel?.readyState === "open") this.channel.send(JSON.stringify({ want: this.wanted }));
  }

  /** Match what we send to how big they show it. */
  private encode() {
    const sender = this.tx.video?.sender;
    if (!sender || this.closed) return;
    this.encoding = this.encoding
      .then(async () => {
        const settings = sender.track?.getSettings?.();
        const src = settings?.width && settings?.height ? { w: settings.width, h: settings.height } : null;
        const e = encodingFor(src, this.theirWant);
        const p = sender.getParameters();
        if (!p.encodings?.length) p.encodings = [{}];
        const enc = p.encodings[0];
        enc.active = e.active;
        enc.scaleResolutionDownBy = e.scale;
        enc.maxBitrate = e.maxBitrate;
        enc.maxFramerate = 30;
        try {
          await sender.setParameters(p);
        } catch {
          // Some WebKit versions reject `active`: keep the size and bitrate.
          const q = sender.getParameters();
          if (!q.encodings?.length) q.encodings = [{}];
          Object.assign(q.encodings[0], { scaleResolutionDownBy: e.scale, maxBitrate: e.active ? e.maxBitrate : MIN_VIDEO_BITRATE, maxFramerate: 30 });
          await sender.setParameters(q);
        }
      })
      .catch((err) => console.warn("couldn't set video quality for", this.id, err));
  }

  async path(): Promise<Path | null> {
    if (this.closed || typeof this.pc.getStats !== "function") return null;
    try {
      const stats = await this.pc.getStats();
      let pairId: string | undefined;
      stats.forEach((s) => {
        if (s.type === "transport" && s.selectedCandidatePairId) pairId = s.selectedCandidatePairId;
      });
      let pair: { localCandidateId?: string } | undefined;
      stats.forEach((s) => {
        if (s.type !== "candidate-pair") return;
        if (pairId ? s.id === pairId : s.nominated && s.state === "succeeded") pair = s;
      });
      if (!pair?.localCandidateId) return null;
      return stats.get(pair.localCandidateId)?.candidateType === "relay" ? "relay" : "direct";
    } catch {
      return null;
    }
  }

  view(): Remote {
    return {
      id: this.id,
      stream: this.stream,
      screen: this.screen,
      state: this.pc.connectionState,
      version: this.version,
      audio: this.tx.audio?.receiver ?? null,
    };
  }

  async send(kind: Kind, track: MediaStreamTrack | null) {
    if (this.closed) return;
    if (kind === "screen") this.screenDirection(track !== null);
    const t = this.tx[kind];
    if (!t) return; // adopted later, with the current track
    try {
      await t.sender.replaceTrack(track);
      if (kind === "video" && track) this.encode();
      if (kind === "screen" && track) this.encodeScreen();
    } catch (e) {
      console.warn("couldn't switch the call's", kind, e);
    }
  }

  /** Send the screen only while sharing (see the top of the file). */
  private screenDirection(sharing: boolean) {
    let t = this.tx.screen;
    if (!t) {
      // Connected to an app that offered no screen transceiver (older, or
      // the offer hasn't arrived yet): add one when there's something to send.
      if (!sharing || !this.adopted) return;
      t = this.tx.screen = this.pc.addTransceiver("video", { direction: "sendrecv" });
      preferHardwareVideo(t);
      return;
    }
    const want: RTCRtpTransceiverDirection = sharing ? "sendrecv" : "recvonly";
    if (t.direction !== want) t.direction = want; // renegotiates
  }

  /** Text on a shared screen stays sharp; motion may get choppy. */
  private encodeScreen() {
    const sender = this.tx.screen?.sender;
    if (!sender || this.closed) return;
    this.encoding = this.encoding
      .then(async () => {
        const p = sender.getParameters() as RTCRtpSendParameters & { degradationPreference?: string };
        if (!p.encodings?.length) p.encodings = [{}];
        Object.assign(p.encodings[0], { active: true, scaleResolutionDownBy: 1, maxBitrate: SCREEN_BITRATE, maxFramerate: SCREEN_FRAMERATE });
        p.degradationPreference = "maintain-resolution";
        try {
          await sender.setParameters(p);
        } catch {
          // WebKit may not take every setting: the bitrate matters most.
          const q = sender.getParameters();
          if (!q.encodings?.length) q.encodings = [{}];
          Object.assign(q.encodings[0], { maxBitrate: SCREEN_BITRATE });
          await sender.setParameters(q);
        }
      })
      .catch((err) => console.warn("couldn't set screen quality for", this.id, err));
  }

  async onSignal({ description, candidate, session, to_session }: SignalData) {
    if (this.closed) return;
    // Meant for a connection of ours that's been replaced, or from one of
    // theirs that has been.
    if (to_session && to_session !== this.session) return;
    if (session && this.remoteSession && session !== this.remoteSession) return;
    if (session) this.remoteSession = session;
    try {
      if (description) {
        const collision =
          description.type === "offer" && (this.makingOffer || this.pc.signalingState !== "stable");
        this.ignoreOffer = !this.polite && collision;
        if (this.ignoreOffer) return;
        await this.pc.setRemoteDescription(description.sdp ? { ...description, sdp: richOpus(description.sdp) } : description);
        if (description.type === "offer") {
          this.adopt();
          this.adopted = true;
          await this.pc.setLocalDescription();
          this.send_({ description: this.pc.localDescription!.toJSON() });
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
    if (this.watchdog) clearTimeout(this.watchdog);
    if (this.channel) this.channel.onopen = this.channel.onmessage = null;
    this.channel?.close();
    this.pc.onicecandidate = this.pc.onnegotiationneeded = this.pc.ontrack = this.pc.onconnectionstatechange = null;
    this.pc.close();
  }

  private async offer() {
    if (this.closed) return;
    try {
      this.makingOffer = true;
      await this.pc.setLocalDescription();
      this.send_({ description: this.pc.localDescription!.toJSON() });
      // The offering side owns retries, so the two never start over at once.
      if (!this.polite) this.armWatchdog();
    } catch (e) {
      console.warn("call offer to", this.id, "failed", e);
    } finally {
      this.makingOffer = false;
    }
  }

  private send_(data: SignalData) {
    this.signal(this.id, { ...data, session: this.session, to_session: this.remoteSession });
  }

  private armWatchdog() {
    if (this.watchdog) clearTimeout(this.watchdog);
    this.watchdog = setTimeout(() => {
      this.watchdog = null;
      if (!this.closed && this.pc.connectionState !== "connected") this.restart(this.retryAfter);
    }, this.retryAfter);
  }

  /** The answering side: use the transceivers the offer created (the first
   * video one for the camera, the second for a shared screen). */
  private adopt() {
    for (const t of this.pc.getTransceivers()) {
      const media = t.receiver.track.kind;
      if (media !== "audio" && media !== "video") continue;
      const kind: Kind = media === "audio" ? "audio" : !this.tx.video || this.tx.video === t ? "video" : "screen";
      if (this.tx[kind] || Object.values(this.tx).includes(t)) continue;
      this.tx[kind] = t;
      t.direction = kind === "screen" ? (this.local("screen") ? "sendrecv" : "recvonly") : "sendrecv";
      if (kind !== "audio") preferHardwareVideo(t);
      void this.send(kind, this.local(kind));
    }
    // Sharing with an app whose offer had no screen transceiver: add ours.
    if (!this.tx.screen && this.local("screen")) {
      this.adopted = true;
      void this.send("screen", this.local("screen"));
    }
    // The offer carries the size channel's m-line by now.
    this.openChannel();
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

function randomId(): string {
  const b = new Uint8Array(8);
  crypto.getRandomValues(b);
  return Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
}

