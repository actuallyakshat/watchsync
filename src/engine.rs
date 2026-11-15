//! The sync engine. Runs on a tokio runtime and owns all state: the room,
//! the mpv player and the shared playback state. The UI sends it `UiCmd`s
//! and renders the `Snapshot`s it publishes.

use crate::fingerprint::fingerprint;
use crate::mpv::{Comfort, Mpv, MpvEvent, Track, TrackKind};
use crate::net::{self, HostLink, Intent, Msg, NetEvent, Offer, PeerInfo, Reaction};
use crate::relay::{Relay, TurnInfo};
use crate::share::{self, Croc, CrocEvent, Progress};
use crate::sync::{
    ClockSync, Correction, Crop, PLAY_LEAD_MS, PlaybackState, correction, crop_geometry,
    format_time, now_ms,
};
use serde::{Deserialize, Serialize};
use iroh::Endpoint;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::time::{MissedTickBehavior, timeout};

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UiCmd {
    Host,
    Join { code: String },
    OpenFile { path: PathBuf },
    TogglePlay,
    SeekBy { seconds: f64 },
    SeekTo { position: f64 },
    SetCrop { crop: Crop },
    SetName { name: String },
    Chat { text: String },
    /// Pick this person's audio or subtitle track (`id: None` = off).
    SetTrack { kind: TrackKind, id: Option<i64> },
    /// This person's volume levelling / dialogue boost.
    SetComfort { comfort: Comfort },
    /// Offer the open movie to the room (sent with croc on request).
    ShareFile,
    StopSharing,
    /// Download someone's offered copy.
    GetFile { offer_id: String },
    CancelTransfer { id: u64 },
    React { reaction: Reaction },
    /// This person's mic / camera went on or off (the call window owns them).
    SetCall { mic: bool, cam: bool },
    /// A WebRTC signalling message for the peer with endpoint id `to`.
    Signal { to: String, data: serde_json::Value },
    /// Show a short message on the movie ("Mic muted").
    Notify { text: String },
    /// Enter or leave fullscreen in the movie window.
    ToggleFullscreen,
    Leave,
    /// Test only: drop the connection to the host as if the network blinked.
    #[doc(hidden)]
    TestDropHostLink,
    /// Test only: vanish without telling anyone, as if the Mac went to sleep.
    #[doc(hidden)]
    TestCrash,
}

/// Where downloads go and how long to wait for transfers to start.
#[derive(Clone, Debug)]
pub struct Options {
    pub timing: Timing,
    pub downloads: PathBuf,
    /// A send nobody connects to is stopped after this.
    pub send_start_timeout: Duration,
    /// Give up on a download if the sender's app doesn't answer.
    pub request_timeout: Duration,
    /// Where WebRTC signalling from others goes (the call window). Not part
    /// of the snapshot, which may drop all but the latest.
    pub signals: Option<async_channel::Sender<SignalIn>>,
}

/// A signalling message from the peer with endpoint id `from`.
#[derive(Clone, Debug, Serialize)]
pub struct SignalIn {
    pub from: String,
    pub data: serde_json::Value,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            timing: Timing::default(),
            downloads: share::default_downloads(),
            send_start_timeout: Duration::from_secs(120),
            request_timeout: Duration::from_secs(30),
            signals: None,
        }
    }
}

/// At most this many sends at once, so requests can't pile up.
const MAX_SENDS: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Send,
    Receive,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferState {
    /// Waiting for the other side (code, or croc connecting).
    Waiting,
    Running,
    /// Received file is being checked.
    Checking,
    Done,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Serialize)]
pub struct TransferView {
    pub id: u64,
    pub direction: Direction,
    /// Who it's from / to.
    pub peer: String,
    pub file: String,
    pub size: u64,
    pub progress: Progress,
    pub state: TransferState,
    pub message: Option<String>,
    /// Where a received file was saved.
    pub path: Option<PathBuf>,
}

struct Transfer {
    view: TransferView,
    offer_id: String,
    /// Receiver: the offer, to verify what arrives. Sender: requester's id.
    offer: Option<Offer>,
    requester_id: String,
    croc: Option<Croc>,
    created: Instant,
}

/// How quickly connection problems are noticed and handled.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// No word from the host for this long means the connection is gone.
    pub silence: Duration,
    /// Keep trying the old host this long before moving to a successor.
    pub old_host_grace: Duration,
    /// How long to try each successor before trying the next.
    pub successor_window: Duration,
    /// Give up on the room after this long without a host.
    pub give_up: Duration,
    /// Host drops a guest it hasn't heard from for this long.
    pub peer_timeout: Duration,
    /// One connection attempt.
    pub attempt: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            silence: Duration::from_secs(6),
            old_host_grace: Duration::from_secs(8),
            successor_window: Duration::from_secs(12),
            give_up: Duration::from_secs(60),
            peer_timeout: Duration::from_secs(10),
            attempt: Duration::from_secs(6),
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    #[default]
    Home,
    Connecting,
    Room,
}

#[derive(Clone, Copy, PartialEq, Debug, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    #[default]
    NoFile,
    Match,
    Mismatch,
    Unknown,
}

#[derive(Clone, Debug, Serialize)]
pub struct PeerRow {
    pub name: String,
    pub is_host: bool,
    pub file: FileStatus,
    pub drift_ms: Option<f64>,
    pub version: String,
    /// Runs a different WatchSync version than the host.
    pub outdated: bool,
    /// Endpoint id ("" for apps too old to say): the call's address for them.
    pub id: String,
    pub mic: bool,
    pub cam: bool,
}

/// One line in the room's chat: a message or a reaction.
#[derive(Clone, Debug, Serialize)]
pub struct ChatLine {
    pub from: String,
    pub text: Option<String>,
    pub reaction: Option<Reaction>,
    pub mine: bool,
    pub at_ms: f64,
}

/// Longest chat message, in characters.
pub const MAX_CHAT: usize = 300;
/// Chat lines kept.
const CHAT_KEEP: usize = 100;

#[derive(Clone, Default, Debug, Serialize)]
pub struct Snapshot {
    pub name: String,
    pub phase: Phase,
    pub is_host: bool,
    pub connected: bool,
    pub code: Option<String>,
    pub file_name: Option<String>,
    pub player_open: bool,
    pub playing: bool,
    pub position: f64,
    pub duration: f64,
    pub last_action: Option<String>,
    pub peers: Vec<PeerRow>,
    pub error: Option<String>,
    /// This player's measured offset from where the room is (ms).
    pub my_drift_ms: Option<f64>,
    /// mpv's control socket (for tests).
    pub player_socket: Option<PathBuf>,
    /// Whether this player's file matches the host's.
    pub my_file: FileStatus,
    /// The room's shared picture crop.
    pub crop: Crop,
    pub crop_by: Option<String>,
    /// Whether the crop is actually applied to this player.
    pub crop_applied: bool,
    /// Lost the host; reconnecting (or waiting for a new host) in the background.
    pub reconnecting: bool,
    /// How many times this session has reconnected after losing the host.
    pub reconnects: u32,
    /// Recent chat and reactions, oldest first.
    pub chat: Vec<ChatLine>,
    /// Audio and subtitle tracks in this person's file.
    pub tracks: Vec<Track>,
    pub comfort: Comfort,
    /// The link to share (only the host has a working one).
    pub invite_link: Option<String>,
    /// Copies others in the room offer to send.
    pub offers: Vec<Offer>,
    /// This person offers their open movie.
    pub sharing: bool,
    pub transfers: Vec<TransferView>,
    /// The host's movie, found on this Mac by its exact name (not opened yet).
    pub found: Option<FoundFile>,
    /// This Mac's endpoint id in the room (the call's address for us).
    pub my_id: String,
    pub mic: bool,
    pub cam: bool,
    /// The movie window is fullscreen.
    pub player_fullscreen: bool,
    /// This Mac's fallback relay for the call (a TURN server on 127.0.0.1).
    pub turn: Option<TurnInfo>,
}

#[derive(Clone, Debug, Serialize)]
pub struct FoundFile {
    pub path: PathBuf,
    /// "Downloads › Movies"
    pub folder: String,
}

enum Role {
    None,
    Host {
        peers: HashMap<u64, Peer>,
    },
    Guest {
        /// None while reconnecting.
        link: Option<HostLink>,
        lost: Option<Lost>,
    },
}

/// A guest that has lost its host.
struct Lost {
    since: Instant,
    /// The old host's code, for retrying it first.
    old_code: String,
    old_id: String,
    /// Set when the host said who takes over.
    successor: Option<String>,
    /// Background connection attempt in flight (its number).
    attempting: Option<u64>,
    last_attempt: Instant,
}

struct Peer {
    tx: mpsc::UnboundedSender<Msg>,
    name: String,
    fingerprint: Option<String>,
    drift_ms: Option<f64>,
    version: String,
    /// Endpoint id and address code ("" for 0.2.x apps).
    id: String,
    code: String,
    last_seen: Instant,
    offer: Option<Offer>,
    mic: bool,
    cam: bool,
}

/// Longest display name, in characters.
pub const MAX_NAME: usize = 32;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// After we pause/unpause mpv ourselves, pause events within this window are ours.
const OWN_PAUSE_GRACE: Duration = Duration::from_millis(400);

struct Engine {
    timing: Timing,
    options: Options,
    share_tx: mpsc::UnboundedSender<(u64, CrocEvent)>,
    /// This person's offer and the file behind it.
    my_offer: Option<(Offer, PathBuf)>,
    /// Every offer in the room (from the host).
    offers: Vec<Offer>,
    transfers: Vec<Transfer>,
    next_transfer: u64,
    /// The open movie's path.
    file_path: Option<PathBuf>,
    /// The host's file name we last looked for, and what we found.
    searched_for: Option<String>,
    found: Option<FoundFile>,
    name: String,
    ui: async_channel::Sender<Snapshot>,
    net_tx: mpsc::UnboundedSender<NetEvent>,
    mpv_tx: mpsc::UnboundedSender<(u64, MpvEvent)>,

    phase: Phase,
    role: Role,
    endpoint: Option<Endpoint>,
    /// Accepting guests on `endpoint` (this Mac is the host).
    serving: Arc<AtomicBool>,
    /// The call's fallback relay, while in a room.
    relay: Option<Relay>,
    /// The host's code (shared as the invite).
    code: Option<String>,
    /// Numbers host connections and attempts so stale ones are ignored.
    next_conn: u64,
    last_host_msg: Instant,
    reconnects: u32,
    chat: Vec<ChatLine>,
    tracks: Vec<Track>,
    comfort: Comfort,
    /// (player generation, comfort) last applied to mpv.
    applied_comfort: Option<(u64, Comfort)>,
    error: Option<String>,

    state: PlaybackState,
    clock: ClockSync,
    room_peers: Vec<PeerInfo>,

    mpv: Option<Arc<Mpv>>,
    mpv_gen: u64,
    mpv_paused: bool,
    speed: f64,
    file_name: Option<String>,
    fingerprint: Option<String>,
    duration: f64,
    drift_ms: Option<f64>,

    last_pause_cmd: Instant,
    /// No drift correction / user-seek detection until then (after our own seeks).
    quiet_until: Instant,
    /// Don't fight mpv's pause state until then (we asked the host for a change).
    hold_until: Instant,
    ticks: u64,

    /// The room's picture crop (host decides, everyone applies).
    crop: Crop,
    crop_by: Option<String>,
    /// (player generation, crop) last applied to mpv.
    applied_crop: Option<(u64, Crop)>,

    /// This person's mic / camera in the call.
    mic: bool,
    cam: bool,
    player_fullscreen: bool,
}

pub async fn run(
    name: String,
    ui: async_channel::Sender<Snapshot>,
    cmds: async_channel::Receiver<UiCmd>,
) {
    run_with(name, ui, cmds, Options::default()).await
}

pub async fn run_with(
    name: String,
    ui: async_channel::Sender<Snapshot>,
    cmds: async_channel::Receiver<UiCmd>,
    options: Options,
) {
    let timing = options.timing;
    let (share_tx, mut share_rx) = mpsc::unbounded_channel();
    let (net_tx, mut net_rx) = mpsc::unbounded_channel();
    let (mpv_tx, mut mpv_rx) = mpsc::unbounded_channel();
    let now = Instant::now();
    let mut e = Engine {
        timing,
        options,
        share_tx,
        my_offer: None,
        offers: Vec::new(),
        transfers: Vec::new(),
        next_transfer: 1,
        file_path: None,
        searched_for: None,
        found: None,
        name,
        ui,
        net_tx,
        mpv_tx,
        phase: Phase::Home,
        role: Role::None,
        endpoint: None,
        serving: Arc::new(AtomicBool::new(false)),
        relay: None,
        code: None,
        next_conn: 1,
        last_host_msg: now,
        reconnects: 0,
        chat: Vec::new(),
        tracks: Vec::new(),
        comfort: Comfort::default(),
        applied_comfort: None,
        error: None,
        state: PlaybackState::default(),
        clock: ClockSync::default(),
        room_peers: Vec::new(),
        mpv: None,
        mpv_gen: 0,
        mpv_paused: true,
        speed: 1.0,
        file_name: None,
        fingerprint: None,
        duration: 0.0,
        drift_ms: None,
        last_pause_cmd: now,
        quiet_until: now,
        hold_until: now,
        ticks: 0,
        crop: Crop::Original,
        crop_by: None,
        applied_crop: None,
        mic: false,
        cam: false,
        player_fullscreen: false,
    };
    e.publish();

    let mut tick = tokio::time::interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        // Wake up exactly at a scheduled start, not just on the next tick.
        let until_start = e.time_until_start();
        tokio::select! {
            _ = tokio::time::sleep(until_start.unwrap_or(Duration::from_secs(3600))),
                if until_start.is_some() => e.enforce().await,
            cmd = cmds.recv() => match cmd {
                Ok(cmd) => e.on_ui(cmd).await,
                Err(_) => break, // window closed
            },
            Some(ev) = net_rx.recv() => e.on_net(ev).await,
            Some((generation, ev)) = mpv_rx.recv() => e.on_mpv(generation, ev).await,
            Some((id, ev)) = share_rx.recv() => e.on_croc(id, ev).await,
            _ = tick.tick() => e.on_tick().await,
        }
        e.publish();
    }
}

impl Engine {
    /// Sender to the host, if connected as a guest.
    fn host_tx(&self) -> Option<&mpsc::UnboundedSender<Msg>> {
        match &self.role {
            Role::Guest { link: Some(l), .. } => Some(&l.tx),
            _ => None,
        }
    }

    fn host_now(&self) -> f64 {
        match self.role {
            Role::Guest { .. } => now_ms() + self.clock.offset(),
            _ => now_ms(),
        }
    }

    /// Time until a scheduled "play" starts, if one is pending.
    fn time_until_start(&self) -> Option<Duration> {
        let host_now = self.host_now();
        (self.mpv.is_some() && self.state.playing && host_now < self.state.anchor_ms)
            .then(|| Duration::from_secs_f64((self.state.anchor_ms - host_now) / 1000.0))
    }

    fn is_host(&self) -> bool {
        matches!(self.role, Role::Host { .. })
    }

    fn quiet_for(&mut self, secs: f64) {
        let until = Instant::now() + Duration::from_secs_f64(secs);
        self.quiet_until = self.quiet_until.max(until);
    }

    // ---------------------------------------------------------------- UI

    async fn on_ui(&mut self, cmd: UiCmd) {
        self.error = None;
        match cmd {
            UiCmd::Host if self.phase == Phase::Home => {
                self.phase = Phase::Connecting;
                self.publish();
                match timeout(CONNECT_TIMEOUT, net::bind()).await {
                    Ok(Ok(endpoint)) => {
                        self.start_listening(&endpoint, true).await;
                        self.code = Some(net::code_of(&endpoint));
                        self.endpoint = Some(endpoint);
                        self.role = Role::Host {
                            peers: HashMap::new(),
                        };
                        self.phase = Phase::Room;
                        self.refresh_peers();
                    }
                    Ok(Err(e)) => self.fail_to_home(format!("Couldn't start a room: {e}")),
                    Err(_) => self.fail_to_home("Couldn't start a room: timed out. Are you online?".into()),
                }
            }
            UiCmd::Join { code } if self.phase == Phase::Home => {
                self.phase = Phase::Connecting;
                self.publish();
                let code = net::extract_code(&code);
                let attempt = async {
                    let endpoint = net::bind().await?;
                    let hello = net::hello(&self.name, &endpoint);
                    let link = net::connect(&endpoint, &code, self.next_conn, hello, self.net_tx.clone()).await?;
                    anyhow::Ok((endpoint, link))
                };
                match timeout(CONNECT_TIMEOUT, attempt).await {
                    Ok(Ok((endpoint, link))) => {
                        self.start_listening(&endpoint, false).await;
                        self.next_conn += 1;
                        self.endpoint = Some(endpoint);
                        self.code = Some(link.code.clone());
                        self.connected_to(link, true);
                        self.phase = Phase::Room;
                    }
                    Ok(Err(e)) => self.fail_to_home(format!("Couldn't join: {e}")),
                    Err(_) => self.fail_to_home("Couldn't join: timed out. Is the host still online?".into()),
                }
            }
            UiCmd::OpenFile { path } => self.open_file(path).await,
            UiCmd::TogglePlay => {
                if self.state.playing {
                    let position = self.current_position().await;
                    self.submit(Intent::Pause { position }).await;
                } else {
                    self.submit(Intent::Play).await;
                }
            }
            UiCmd::SeekBy { seconds } => {
                let position = self.state.expected_position(self.host_now()) + seconds;
                self.seek_to(position).await;
            }
            UiCmd::SeekTo { position } => self.seek_to(position).await,
            UiCmd::SetCrop { crop } => {
                if crop != self.crop {
                    self.submit(Intent::SetCrop { crop }).await;
                }
            }
            UiCmd::SetName { name } => self.rename(name).await,
            UiCmd::Chat { text } => {
                if let Some(text) = clean_chat(&text) {
                    self.add_chat(self.name.clone(), Some(text.clone()), None, true);
                    self.relay(Msg::Chat { from: self.name.clone(), text }, None);
                }
            }
            UiCmd::SetTrack { kind, id } => {
                if let Some(mpv) = self.mpv.clone()
                    && mpv.set_track(kind, id).await.is_ok()
                {
                    self.tracks = mpv.tracks().await.unwrap_or_default();
                }
            }
            UiCmd::ShareFile => self.share_file(),
            UiCmd::StopSharing => self.stop_sharing(),
            UiCmd::GetFile { offer_id } => self.get_file(offer_id),
            UiCmd::CancelTransfer { id } => {
                if let Some(t) = self.transfers.iter_mut().find(|t| t.view.id == id)
                    && matches!(t.view.state, TransferState::Waiting | TransferState::Running)
                {
                    t.croc = None; // stops croc (and its caffeinate)
                    t.view.state = TransferState::Cancelled;
                }
            }
            UiCmd::SetComfort { comfort } => {
                self.comfort = comfort;
                self.apply_comfort().await;
            }
            UiCmd::React { reaction } => {
                self.add_chat(self.name.clone(), None, Some(reaction), true);
                self.relay(Msg::React { from: self.name.clone(), reaction }, None);
            }
            UiCmd::SetCall { mic, cam } => {
                self.mic = mic;
                self.cam = cam;
                if self.is_host() {
                    self.refresh_peers();
                } else if let Some(tx) = self.host_tx() {
                    let _ = tx.send(Msg::Call { mic, cam });
                }
            }
            UiCmd::Signal { to, data } => {
                let from_id = self.my_id();
                self.route(Msg::Signal { to, from_id, data });
            }
            UiCmd::Notify { text } => {
                let text: String = text.chars().take(MAX_CHAT).collect();
                self.osd(&text).await;
            }
            UiCmd::ToggleFullscreen => {
                if let Some(mpv) = self.mpv.clone() {
                    let _ = mpv.toggle_fullscreen().await;
                }
            }
            UiCmd::Leave => self.leave().await,
            UiCmd::TestDropHostLink => {
                if let Role::Guest { link: Some(l), .. } = &self.role {
                    l.connection.close(0u32.into(), b"test");
                }
            }
            UiCmd::TestCrash => {
                // Close without HostLeaving: guests must notice on their own.
                self.role = Role::None;
                if let Some(endpoint) = self.endpoint.take() {
                    endpoint.close().await;
                }
                self.leave().await;
            }
            _ => {}
        }
    }

    async fn seek_to(&mut self, position: f64) {
        let mut position = position.max(0.0);
        if self.duration > 0.0 {
            position = position.min(self.duration - 1.0);
        }
        self.submit(Intent::Seek { position }).await;
    }

    /// Change this person's display name, live if they're in a room.
    async fn rename(&mut self, name: String) {
        let name: String = name.trim().chars().take(MAX_NAME).collect();
        if name.is_empty() || name == self.name {
            return;
        }
        self.name = name.clone();
        if self.is_host() {
            self.refresh_peers();
        } else if let Some(tx) = self.host_tx() {
            let _ = tx.send(Msg::Rename { name });
        }
    }

    fn add_chat(&mut self, from: String, text: Option<String>, reaction: Option<Reaction>, mine: bool) {
        self.chat.push(ChatLine {
            from,
            text,
            reaction,
            mine,
            at_ms: now_ms(),
        });
        if self.chat.len() > CHAT_KEEP {
            self.chat.remove(0);
        }
    }

    /// Send chat/reactions on: a guest to the host, the host to everyone
    /// except `skip` (the peer it came from).
    fn relay(&self, msg: Msg, skip: Option<u64>) {
        match &self.role {
            Role::Host { peers } => {
                for (id, p) in peers {
                    if Some(*id) != skip {
                        let _ = p.tx.send(msg.clone());
                    }
                }
            }
            _ => {
                if let Some(tx) = self.host_tx() {
                    let _ = tx.send(msg);
                }
            }
        }
    }

    /// Someone else's chat line or reaction arrived: log it and show it on the movie.
    async fn heard(&mut self, from: String, text: Option<String>, reaction: Option<Reaction>) {
        let osd = match (&text, reaction) {
            (Some(t), _) => format!("{from}: {t}"),
            (None, Some(r)) => format!("{from} {}", r.phrase()),
            _ => return,
        };
        self.add_chat(from, text, reaction, false);
        self.osd(&osd).await;
    }

    fn fail_to_home(&mut self, error: String) {
        self.error = Some(error);
        self.phase = Phase::Home;
        self.role = Role::None;
        if let Some(endpoint) = self.endpoint.take() {
            tokio::spawn(async move { endpoint.close().await });
        }
        self.serving.store(false, Ordering::SeqCst);
        self.relay = None;
    }

    /// Who takes over if the host leaves: the earliest-joined guest that
    /// can host (0.2.x apps can't), in the host's member list order.
    fn successors(&self, excluding: &str) -> Vec<PeerInfo> {
        self.room_peers
            .iter()
            .filter(|p| !p.is_host && !p.id.is_empty() && !p.code.is_empty() && p.id != excluding)
            .cloned()
            .collect()
    }

    /// Leave the room and close the player. A host hands the room over to
    /// the next person; leaving alone ends it.
    async fn leave(&mut self) {
        if let Role::Host { peers } = &self.role
            && !peers.is_empty()
        {
            let me = self.endpoint.as_ref().map(net::id_of).unwrap_or_default();
            if let Some(next) = self.successors(&me).first() {
                self.broadcast(Msg::HostLeaving {
                    successor: next.id.clone(),
                });
            }
        }
        self.role = Role::None;
        // Close properly so the others see us leave right away instead of
        // after a network timeout (after a moment, so HostLeaving goes out).
        if let Some(endpoint) = self.endpoint.take() {
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                endpoint.close().await
            });
        }
        self.serving.store(false, Ordering::SeqCst);
        self.relay = None;
        self.code = None;
        self.phase = Phase::Home;
        self.room_peers.clear();
        self.state = PlaybackState::default();
        self.clock = ClockSync::default();
        self.crop = Crop::Original;
        self.crop_by = None;
        self.chat.clear();
        self.my_offer = None;
        self.offers.clear();
        self.searched_for = None;
        self.found = None;
        // The next room starts without a movie.
        self.file_path = None;
        self.file_name = None;
        self.fingerprint = None;
        self.duration = 0.0;
        for t in &mut self.transfers {
            if t.croc.take().is_some() {
                t.view.state = TransferState::Cancelled;
            }
        }
        self.mpv = None;
        self.mpv_gen += 1;
        self.applied_crop = None;
        self.drift_ms = None;
        self.player_fullscreen = false;
        // The call window sets these again when it joins the next call.
        self.mic = false;
        self.cam = false;
    }

    /// Apply the room's crop to the player if it isn't already. Needs the
    /// video size, which exists only after the first frame is decoded, so
    /// this is retried from the heartbeat until it sticks.
    async fn apply_crop(&mut self) {
        let Some(mpv) = self.mpv.clone() else { return };
        let wanted = (self.mpv_gen, self.crop);
        if self.applied_crop == Some(wanted) {
            return;
        }
        let Ok((w, h, dw, dh)) = mpv.video_size().await else {
            return;
        };
        let geometry = crop_geometry(w, h, dw, dh, self.crop);
        if mpv.set_crop(&geometry).await.is_ok() {
            self.applied_crop = Some(wanted);
        }
    }

    /// Apply this person's audio comfort settings to the player.
    async fn apply_comfort(&mut self) {
        let Some(mpv) = self.mpv.clone() else { return };
        let wanted = (self.mpv_gen, self.comfort);
        if self.applied_comfort != Some(wanted) && mpv.set_comfort(self.comfort).await.is_ok() {
            self.applied_comfort = Some(wanted);
        }
    }

    async fn current_position(&self) -> f64 {
        if let Some(mpv) = &self.mpv
            && let Ok(pos) = mpv.time_pos().await
        {
            return pos;
        }
        self.state.expected_position(self.host_now())
    }

    async fn open_file(&mut self, path: PathBuf) {
        let p = path.clone();
        self.fingerprint = tokio::task::spawn_blocking(move || fingerprint(&p))
            .await
            .ok()
            .and_then(Result::ok);
        self.file_name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        if self.my_offer.is_some() {
            self.stop_sharing(); // the offer was for the previous file
        }
        self.file_path = Some(path.clone());
        self.found = None;

        // Replace any previous player. Events are tagged with a generation so
        // the old player's "exited" event doesn't close the new one.
        self.mpv = None;
        self.mpv_gen += 1;
        let generation = self.mpv_gen;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let forward = self.mpv_tx.clone();
        tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                if forward.send((generation, ev)).is_err() {
                    break;
                }
            }
        });

        let start = self.state.expected_position(self.host_now());
        self.quiet_for(3.0);
        match Mpv::launch(&path, start, tx).await {
            Ok(mpv) => {
                let mpv = Arc::new(mpv);
                self.duration = mpv.duration().await.unwrap_or(0.0);
                self.mpv = Some(mpv);
                self.mpv_paused = true;
                self.speed = 1.0;
                self.drift_ms = None;
                self.applied_crop = None;
                self.applied_comfort = None;
                self.resync().await;
                self.apply_crop().await;
                self.apply_comfort().await;
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        if self.is_host() {
            self.refresh_peers();
        }
    }

    // ---------------------------------------------------------- intents

    /// Ask for a change. The host applies it; guests forward it to the host.
    async fn submit(&mut self, intent: Intent) {
        if self.is_host() {
            let name = self.name.clone();
            self.apply_intent(intent, name).await;
        } else if let Some(tx) = self.host_tx() {
            let _ = tx.send(Msg::Intent(intent));
            // Let mpv keep whatever the user just did until the host answers.
            self.hold_until = Instant::now() + Duration::from_millis(1500);
        }
    }

    /// Host only: turn an intent into a new authoritative state and broadcast it.
    async fn apply_intent(&mut self, intent: Intent, by: String) {
        let now = now_ms();
        let mut s = self.state.clone();
        match intent {
            Intent::Play => {
                if s.playing {
                    // Someone's player is out of step: resend the current state.
                    self.broadcast(Msg::State(self.state.clone()));
                    return;
                }
                s.playing = true;
                s.anchor_ms = now + PLAY_LEAD_MS;
                s.action = "pressed play".into();
            }
            Intent::Pause { position } => {
                s.playing = false;
                s.position = position.max(0.0);
                s.anchor_ms = now;
                s.action = format!("paused at {}", format_time(position));
            }
            Intent::SetCrop { crop } => {
                self.crop = crop;
                self.crop_by = Some(by.clone());
                self.broadcast(Msg::Crop { crop, by: by.clone() });
                self.osd(&format!("{by} set the picture to {}", crop.label())).await;
                self.apply_crop().await;
                return;
            }
            Intent::Seek { position } => {
                s.position = position.max(0.0);
                s.anchor_ms = if s.playing { now + PLAY_LEAD_MS } else { now };
                s.action = format!("jumped to {}", format_time(position));
            }
        }
        s.version += 1;
        s.by = by;
        self.broadcast(Msg::State(s.clone()));
        self.apply_state(s).await;
    }

    async fn apply_state(&mut self, s: PlaybackState) {
        if s.version < self.state.version {
            return;
        }
        let changed = s.version != self.state.version;
        // A reconnect or a new host resends a state that says "keep going":
        // don't seek for that, let speed nudging close any small gap.
        let host_now = self.host_now();
        let gentle = s.playing == self.state.playing
            && (s.expected_position(host_now) - self.state.expected_position(host_now)).abs() < 0.25;
        self.state = s;
        if gentle {
            self.hold_until = Instant::now();
            self.enforce().await;
            return;
        }
        self.hold_until = Instant::now();
        if changed
            && let (Some(mpv), Some(label)) = (self.mpv.clone(), self.state.label())
        {
            mpv.show_text(&label).await;
        }
        self.resync().await;
    }

    /// Snap the player to the shared state (after a state change or file open).
    async fn resync(&mut self) {
        let Some(mpv) = self.mpv.clone() else { return };
        let host_now = self.host_now();
        if self.state.should_be_paused(host_now)
            && !self.mpv_paused
            && mpv.set_pause(true).await.is_ok()
        {
            self.mpv_paused = true;
            self.last_pause_cmd = Instant::now();
        }
        let target = self.state.expected_position(host_now);
        if let Ok(pos) = mpv.time_pos().await
            && (pos - target).abs() > 0.02
            && mpv.seek(target).await.is_ok()
        {
            self.quiet_for(1.5);
        }
        if self.speed != 1.0 && mpv.set_speed(1.0).await.is_ok() {
            self.speed = 1.0;
        }
        // Unpausing happens in `enforce`, exactly at the scheduled time.
        self.enforce().await;
    }

    // ------------------------------------------------------------- tick

    async fn on_tick(&mut self) {
        self.ticks += 1;
        self.enforce().await;
        if self.ticks.is_multiple_of(3) {
            self.reconnect_step().await;
        }
        if self.ticks.is_multiple_of(5) {
            self.correct_drift().await;
        }
        if self.ticks.is_multiple_of(10) {
            self.heartbeat().await;
        }
    }

    /// Make mpv's paused/playing match the shared state (start on schedule).
    async fn enforce(&mut self) {
        let Some(mpv) = self.mpv.clone() else { return };
        if Instant::now() < self.hold_until {
            return;
        }
        let want_paused = self.state.should_be_paused(self.host_now());
        if want_paused != self.mpv_paused && mpv.set_pause(want_paused).await.is_ok() {
            self.mpv_paused = want_paused;
            self.last_pause_cmd = Instant::now();
            if !want_paused {
                // time-pos is jumpy right after resuming
                self.quiet_for(1.0);
            }
        }
    }

    async fn correct_drift(&mut self) {
        let Some(mpv) = self.mpv.clone() else { return };
        let now = Instant::now();
        if now < self.quiet_until || now < self.hold_until {
            return;
        }
        let Ok(actual) = mpv.time_pos().await else { return };
        let host_now = self.host_now();
        let expected = self.state.expected_position(host_now);
        self.drift_ms = Some((actual - expected) * 1000.0);

        if self.state.should_be_paused(host_now) {
            // Paused: just make sure we're on the same frame.
            if (actual - expected).abs() > 0.25 && mpv.seek(expected).await.is_ok() {
                self.quiet_for(1.5);
            }
            return;
        }
        match correction(actual, expected, self.speed) {
            Correction::None => {}
            Correction::Speed(speed) => {
                if mpv.set_speed(speed).await.is_ok() {
                    self.speed = speed;
                }
            }
            Correction::Seek(target) => {
                // Aim slightly ahead to cover the time the seek itself takes.
                if mpv.seek(target + 0.1).await.is_ok() {
                    self.quiet_for(1.5);
                }
                if mpv.set_speed(1.0).await.is_ok() {
                    self.speed = 1.0;
                }
            }
        }
    }

    async fn heartbeat(&mut self) {
        if let Some(mpv) = self.mpv.clone()
            && self.duration <= 0.0
        {
            self.duration = mpv.duration().await.unwrap_or(0.0);
        }
        self.apply_crop().await;
        self.apply_comfort().await;
        self.expire_transfers();
        self.look_for_host_movie().await;
        // The person may switch tracks in the movie window too (j, #).
        if let Some(mpv) = self.mpv.clone() {
            self.tracks = mpv.tracks().await.unwrap_or_default();
        } else {
            self.tracks.clear();
        }
        if self.is_host() {
            self.refresh_peers();
            self.prune_silent_peers().await;
        } else if let Some(tx) = self.host_tx() {
            let _ = tx.send(Msg::Ping { t0: now_ms() });
            let _ = tx.send(Msg::Status {
                fingerprint: self.fingerprint.clone(),
                drift_ms: self.drift_ms,
            });
            if self.last_host_msg.elapsed() > self.timing.silence {
                // Nothing from the host for a while: treat it as gone.
                self.host_lost(None);
            }
        }
    }

    // ---------------------------------------------------------------- mpv

    async fn on_mpv(&mut self, generation: u64, ev: MpvEvent) {
        if generation != self.mpv_gen {
            return;
        }
        match ev {
            MpvEvent::Pause(paused) => {
                self.mpv_paused = paused;
                if self.last_pause_cmd.elapsed() < OWN_PAUSE_GRACE {
                    return; // our own command echoing back
                }
                if paused == self.state.should_be_paused(self.host_now()) {
                    return;
                }
                // The user pressed space / clicked in the mpv window.
                if paused {
                    let position = self.current_position().await;
                    self.submit(Intent::Pause { position }).await;
                } else {
                    self.submit(Intent::Play).await;
                }
            }
            MpvEvent::PlaybackRestart => {
                self.apply_crop().await;
                if Instant::now() < self.quiet_until {
                    return; // a seek we made
                }
                let Some(mpv) = self.mpv.clone() else { return };
                let Ok(pos) = mpv.time_pos().await else { return };
                let expected = self.state.expected_position(self.host_now());
                if (pos - expected).abs() > 1.0 {
                    // The user seeked in the mpv window: share it.
                    self.quiet_for(1.5);
                    self.submit(Intent::Seek { position: pos }).await;
                }
            }
            MpvEvent::Fullscreen(on) => self.player_fullscreen = on,
            MpvEvent::Exited => {
                self.mpv = None;
                self.drift_ms = None;
                self.player_fullscreen = false;
            }
        }
    }

    // ------------------------------------------------------------ network

    async fn on_net(&mut self, ev: NetEvent) {
        match ev {
            NetEvent::PeerJoined { id, tx } => {
                if let Role::Host { peers } = &mut self.role {
                    peers.insert(
                        id,
                        Peer {
                            tx,
                            name: "Someone".into(),
                            fingerprint: None,
                            drift_ms: None,
                            version: String::new(),
                            id: String::new(),
                            code: String::new(),
                            last_seen: Instant::now(),
                            offer: None,
                            mic: false,
                            cam: false,
                        },
                    );
                }
            }
            NetEvent::FromPeer { id, msg } => self.on_peer_msg(id, msg).await,
            NetEvent::PeerLeft { id } => {
                let left = match &mut self.role {
                    Role::Host { peers } => peers.remove(&id).map(|p| p.name),
                    _ => None,
                };
                if let Some(name) = left {
                    self.osd(&format!("{name} left")).await;
                    self.refresh_peers();
                }
            }
            NetEvent::FromHost { conn, msg } => {
                if !matches!(&self.role, Role::Guest { link: Some(l), .. } if l.conn == conn) {
                    return; // an old connection
                }
                self.last_host_msg = Instant::now();
                match msg {
                    Msg::Pong { t0, host_ms } => self.clock.add_sample(t0, host_ms, now_ms()),
                    Msg::State(s) => self.apply_state(s).await,
                    Msg::Peers(list) => self.room_peers = list,
                    Msg::Crop { crop, by } => {
                        if crop != self.crop {
                            self.osd(&format!("{by} set the picture to {}", crop.label()))
                                .await;
                        }
                        self.crop = crop;
                        self.crop_by = Some(by);
                        self.apply_crop().await;
                    }
                    Msg::HostLeaving { successor } => self.host_lost(Some(successor)),
                    Msg::Chat { from, text } => {
                        if let Some(text) = clean_chat(&text) {
                            self.heard(from, Some(text), None).await;
                        }
                    }
                    Msg::React { from, reaction } => self.heard(from, None, Some(reaction)).await,
                    Msg::Offers(list) => self.offers = list,
                    Msg::FileRequest { offer_id, from_id, from, .. } => {
                        self.serve_request(offer_id, from_id, from)
                    }
                    Msg::FileCode { offer_id, code, .. } => self.start_download(offer_id, code),
                    Msg::FileRefused { offer_id, reason, .. } => self.refused(&offer_id, reason),
                    msg @ Msg::Signal { .. } => self.route(msg),
                    _ => {}
                }
            }
            NetEvent::HostLost { conn } => {
                if matches!(&self.role, Role::Guest { link: Some(l), .. } if l.conn == conn) {
                    self.host_lost(None);
                }
            }
            NetEvent::Connected { attempt, result } => {
                let Role::Guest { lost: Some(lost), .. } = &mut self.role else {
                    return;
                };
                if lost.attempting != Some(attempt) {
                    return; // stale, or we left meanwhile
                }
                lost.attempting = None;
                match result {
                    Ok(link) => {
                        let new_host = link.code != lost.old_code;
                        self.connected_to(link, new_host);
                        self.reconnects += 1;
                        self.error = None;
                        self.osd("Reconnected").await;
                    }
                    Err(_) => lost.last_attempt = Instant::now(),
                }
            }
        }
    }

    /// Now talking to the host over `link`.
    fn connected_to(&mut self, link: HostLink, new_host: bool) {
        if new_host {
            // A different Mac's clock: measure afresh, and don't correct
            // drift until the first measurements are in.
            self.clock = ClockSync::default();
            self.quiet_for(2.5);
        }
        for _ in 0..5 {
            let _ = link.tx.send(Msg::Ping { t0: now_ms() });
        }
        let _ = link.tx.send(Msg::Call {
            mic: self.mic,
            cam: self.cam,
        });
        self.last_host_msg = Instant::now();
        self.role = Role::Guest {
            link: Some(link),
            lost: None,
        };
    }

    /// The host is gone (or said who takes over). Keep playing; reconnect in
    /// the background.
    fn host_lost(&mut self, successor: Option<String>) {
        let Role::Guest { link, lost } = &mut self.role else {
            return;
        };
        if let Some(l) = link.take() {
            l.connection.close(0u32.into(), b"lost");
            let old_id = self
                .room_peers
                .iter()
                .find(|p| p.is_host)
                .map(|p| p.id.clone())
                .unwrap_or_default();
            *lost = Some(Lost {
                since: Instant::now(),
                old_code: l.code,
                old_id,
                successor,
                attempting: None,
                last_attempt: Instant::now() - Duration::from_secs(60),
            });
        } else if let (Some(l), Some(s)) = (lost.as_mut(), successor) {
            l.successor = Some(s);
        }
    }

    /// While lost: retry the old host, then the successor (or become it).
    async fn reconnect_step(&mut self) {
        let Role::Guest { lost: Some(lost), .. } = &self.role else {
            return;
        };
        if lost.attempting.is_some() || lost.last_attempt.elapsed() < Duration::from_secs(1) {
            return;
        }
        let elapsed = lost.since.elapsed();
        if elapsed > self.timing.give_up {
            self.leave().await;
            self.error = Some("Lost the room: nobody could take over as host.".into());
            return;
        }
        let my_id = self.endpoint.as_ref().map(net::id_of).unwrap_or_default();
        let candidates = self.successors(&lost.old_id);
        let target = if let Some(next) = &lost.successor {
            // The host told us who takes over.
            candidates.iter().find(|p| &p.id == next).cloned()
        } else if elapsed < self.timing.old_host_grace {
            None // keep trying the old host for now
        } else {
            let i = ((elapsed - self.timing.old_host_grace).as_secs_f64()
                / self.timing.successor_window.as_secs_f64()) as usize;
            candidates.get(i).cloned()
        };
        let code = match &target {
            Some(p) if p.id == my_id => {
                self.become_host().await;
                return;
            }
            Some(p) => p.code.clone(),
            None if lost.successor.is_none() => lost.old_code.clone(),
            None => return, // named successor isn't in our list yet
        };
        let (Some(endpoint), Role::Guest { lost: Some(lost), .. }) = (self.endpoint.clone(), &mut self.role)
        else {
            return;
        };
        let attempt = self.next_conn;
        self.next_conn += 1;
        lost.attempting = Some(attempt);
        lost.last_attempt = Instant::now();
        let hello = net::hello(&self.name, &endpoint);
        let events = self.net_tx.clone();
        let limit = self.timing.attempt;
        tokio::spawn(async move {
            let result = match timeout(limit, net::connect(&endpoint, &code, attempt, hello, events.clone())).await {
                Ok(Ok(link)) => Ok(link),
                Ok(Err(e)) => Err(e.to_string()),
                Err(_) => Err("timed out".into()),
            };
            let _ = events.send(NetEvent::Connected { attempt, result });
        });
    }

    /// Take over as host, carrying on from where the room is.
    async fn become_host(&mut self) {
        let Some(endpoint) = self.endpoint.clone() else { return };
        // Re-anchor the shared state on this Mac's clock.
        let host_now = self.host_now();
        let mut s = self.state.clone();
        if s.playing {
            let wait = (s.anchor_ms - host_now).max(0.0);
            s.position = s.expected_position(host_now);
            s.anchor_ms = now_ms() + wait;
        } else {
            s.anchor_ms = now_ms();
        }
        s.version += 1;
        s.by = self.name.clone();
        s.action = "is now the host".into();
        self.clock = ClockSync::default();
        self.state = s;
        // Guests dialling us before now were turned away; they retry.
        self.serving.store(true, Ordering::SeqCst);
        self.code = Some(net::code_of(&endpoint));
        self.role = Role::Host {
            peers: HashMap::new(),
        };
        self.error = None;
        self.refresh_peers();
        self.osd("You're now the host").await;
    }

    /// Host: drop guests we haven't heard from (their connection died quietly).
    async fn prune_silent_peers(&mut self) {
        let timeout = self.timing.peer_timeout;
        let gone: Vec<String> = match &mut self.role {
            Role::Host { peers } => {
                let ids: Vec<u64> = peers
                    .iter()
                    .filter(|(_, p)| p.last_seen.elapsed() > timeout)
                    .map(|(id, _)| *id)
                    .collect();
                ids.iter().filter_map(|id| peers.remove(id)).map(|p| p.name).collect()
            }
            _ => return,
        };
        for name in gone {
            self.osd(&format!("{name} left")).await;
        }
    }

    /// Handle incoming connections on a new endpoint (guests if `host`, and
    /// friends' call relays), and start the call's relay.
    async fn start_listening(&mut self, endpoint: &Endpoint, host: bool) {
        self.serving = Arc::new(AtomicBool::new(host));
        let (relay_tx, relay_rx) = mpsc::unbounded_channel();
        net::listen(endpoint.clone(), self.net_tx.clone(), self.serving.clone(), relay_tx);
        self.relay = match Relay::start(endpoint.clone(), relay_rx).await {
            Ok(r) => Some(r),
            Err(e) => {
                eprintln!("call relay unavailable: {e}");
                None
            }
        };
    }

    /// Guest without a movie: look for the host's file, by exact name, once
    /// per name. Suggests it; opening is the person's choice.
    async fn look_for_host_movie(&mut self) {
        if self.is_host() || self.file_path.is_some() {
            return;
        }
        let Some(name) = self.room_peers.iter().find(|p| p.is_host).and_then(|p| p.file_name.clone())
        else {
            return;
        };
        if self.searched_for.as_deref() == Some(name.as_str()) {
            return;
        }
        self.searched_for = Some(name.clone());
        self.found = tokio::task::spawn_blocking(move || crate::find::find_by_name(&name))
            .await
            .ok()
            .flatten()
            .map(|path| FoundFile {
                folder: crate::find::describe_folder(&path),
                path,
            });
    }

    // ---------------------------------------------------------------- files

    fn my_id(&self) -> String {
        self.endpoint.as_ref().map(net::id_of).unwrap_or_default()
    }

    /// Send a message to whoever it's addressed to (`to` = endpoint id):
    /// the host delivers it; guests hand everything to the host.
    fn route(&mut self, msg: Msg) {
        let to = match &msg {
            Msg::FileRequest { to, .. }
            | Msg::FileCode { to, .. }
            | Msg::FileRefused { to, .. }
            | Msg::Signal { to, .. } => to.clone(),
            _ => return,
        };
        if to == self.my_id() {
            match msg {
                Msg::Signal { from_id, data, .. } => {
                    if let Some(signals) = &self.options.signals {
                        let _ = signals.try_send(SignalIn { from: from_id, data });
                    }
                }
                Msg::FileRequest { offer_id, from_id, from, .. } => self.serve_request(offer_id, from_id, from),
                Msg::FileCode { offer_id, code, .. } => self.start_download(offer_id, code),
                Msg::FileRefused { offer_id, reason, .. } => self.refused(&offer_id, reason),
                _ => {}
            }
            return;
        }
        match &self.role {
            Role::Host { peers } => {
                if let Some(p) = peers.values().find(|p| p.id == to) {
                    let _ = p.tx.send(msg);
                }
            }
            _ => {
                if let Some(tx) = self.host_tx() {
                    let _ = tx.send(msg);
                }
            }
        }
    }

    /// Offer the open movie to the room.
    fn share_file(&mut self) {
        let (Some(path), Some(fingerprint)) = (self.file_path.clone(), self.fingerprint.clone()) else {
            self.error = Some("Choose your movie first, then share it.".into());
            return;
        };
        let Ok(offer_id) = share::random_hex(8) else { return };
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let offer = Offer {
            offer_id,
            from_id: self.my_id(),
            from: self.name.clone(),
            name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            size,
            fingerprint,
        };
        self.my_offer = Some((offer.clone(), path));
        if self.is_host() {
            self.refresh_peers();
        } else if let Some(tx) = self.host_tx() {
            let _ = tx.send(Msg::Offer(offer));
        }
    }

    fn stop_sharing(&mut self) {
        let Some((offer, _)) = self.my_offer.take() else { return };
        // Sends nobody has connected to yet stop; ones in progress finish.
        for t in &mut self.transfers {
            if t.view.direction == Direction::Send && t.view.state == TransferState::Waiting {
                t.croc = None;
                t.view.state = TransferState::Cancelled;
            }
        }
        if self.is_host() {
            self.refresh_peers();
        } else if let Some(tx) = self.host_tx() {
            let _ = tx.send(Msg::OfferEnd { offer_id: offer.offer_id });
        }
    }

    fn add_transfer(&mut self, direction: Direction, peer: String, offer: &Offer) -> u64 {
        let id = self.next_transfer;
        self.next_transfer += 1;
        self.transfers.push(Transfer {
            view: TransferView {
                id,
                direction,
                peer,
                file: offer.name.clone(),
                size: offer.size,
                progress: Progress::default(),
                state: TransferState::Waiting,
                message: None,
                path: None,
            },
            offer_id: offer.offer_id.clone(),
            offer: Some(offer.clone()),
            requester_id: String::new(),
            croc: None,
            created: Instant::now(),
        });
        id
    }

    fn fail(&mut self, id: u64, message: impl Into<String>) {
        if let Some(t) = self.transfers.iter_mut().find(|t| t.view.id == id) {
            t.croc = None;
            t.view.state = TransferState::Failed;
            t.view.message = Some(message.into());
        }
    }

    /// Ask the offerer to send their copy.
    fn get_file(&mut self, offer_id: String) {
        let Some(offer) = self.offers.iter().find(|o| o.offer_id == offer_id).cloned() else {
            self.error = Some("That copy isn't on offer anymore.".into());
            return;
        };
        if !share::is_installed() {
            self.error = Some("Receiving movies needs croc. Install it with: brew install croc".into());
            return;
        }
        let busy = self.transfers.iter().any(|t| {
            t.offer_id == offer_id
                && t.view.direction == Direction::Receive
                && matches!(t.view.state, TransferState::Waiting | TransferState::Running | TransferState::Checking)
        });
        if busy {
            return;
        }
        let dir = self.options.downloads.clone();
        let _ = std::fs::create_dir_all(&dir);
        if let Some(free) = share::free_space(&dir)
            && free < offer.size + 200 * 1024 * 1024
        {
            self.error = Some(format!(
                "Not enough space in {} for {}.",
                dir.display(),
                offer.name
            ));
            return;
        }
        self.add_transfer(Direction::Receive, offer.from.clone(), &offer);
        self.route(Msg::FileRequest {
            offer_id,
            to: offer.from_id.clone(),
            from_id: self.my_id(),
            from: self.name.clone(),
        });
    }

    /// Someone asked for our offered copy: start a croc session just for them.
    fn serve_request(&mut self, offer_id: String, requester_id: String, requester: String) {
        let refuse = |e: &mut Self, reason: &str| {
            e.route(Msg::FileRefused {
                offer_id: offer_id.clone(),
                to: requester_id.clone(),
                reason: reason.into(),
            })
        };
        // Only ever the file we offered: requests name an offer, never a path.
        let Some((offer, path)) = self.my_offer.clone().filter(|(o, _)| o.offer_id == offer_id) else {
            return refuse(self, "isn't sharing that copy anymore");
        };
        if !share::is_installed() {
            return refuse(self, "needs croc installed to send (brew install croc)");
        }
        let active = |t: &Transfer| matches!(t.view.state, TransferState::Waiting | TransferState::Running);
        if self
            .transfers
            .iter()
            .any(|t| t.view.direction == Direction::Send && t.requester_id == requester_id && active(t))
        {
            return; // already sending to them
        }
        if self.transfers.iter().filter(|t| t.view.direction == Direction::Send && active(t)).count() >= MAX_SENDS {
            return refuse(self, "is already sending to a few people. Try again in a bit");
        }
        let Ok(code) = share::new_code() else { return };
        let id = self.add_transfer(Direction::Send, requester.clone(), &offer);
        match share::start(id, share::Job::Send { file: &path }, &code, self.share_tx.clone()) {
            Ok(croc) => {
                if let Some(t) = self.transfers.iter_mut().find(|t| t.view.id == id) {
                    t.croc = Some(croc);
                    t.requester_id = requester_id.clone();
                }
                self.route(Msg::FileCode {
                    offer_id,
                    to: requester_id,
                    code,
                });
            }
            Err(e) => {
                self.fail(id, e.to_string());
                refuse(self, "couldn't start sending");
            }
        }
    }

    /// The sender gave us a code: start receiving.
    fn start_download(&mut self, offer_id: String, code: String) {
        let Some(t) = self.transfers.iter_mut().find(|t| {
            t.offer_id == offer_id
                && t.view.direction == Direction::Receive
                && t.view.state == TransferState::Waiting
                && t.croc.is_none()
        }) else {
            return;
        };
        let id = t.view.id;
        let into = self.options.downloads.clone();
        match share::start(id, share::Job::Receive { into: &into }, &code, self.share_tx.clone()) {
            Ok(croc) => {
                if let Some(t) = self.transfers.iter_mut().find(|t| t.view.id == id) {
                    t.croc = Some(croc);
                }
            }
            Err(e) => self.fail(id, e.to_string()),
        }
    }

    fn refused(&mut self, offer_id: &str, reason: String) {
        let ids: Vec<(u64, String)> = self
            .transfers
            .iter()
            .filter(|t| t.offer_id == offer_id && t.view.direction == Direction::Receive && t.view.state == TransferState::Waiting)
            .map(|t| (t.view.id, t.view.peer.clone()))
            .collect();
        for (id, who) in ids {
            self.fail(id, format!("{who} {reason}"));
        }
    }

    /// Stop sends nobody connected to, and requests nobody answered.
    fn expire_transfers(&mut self) {
        let (send_limit, ask_limit) = (self.options.send_start_timeout, self.options.request_timeout);
        for t in &mut self.transfers {
            if t.view.state != TransferState::Waiting {
                continue;
            }
            let age = t.created.elapsed();
            let expired = match t.view.direction {
                Direction::Send => age > send_limit,
                Direction::Receive => age > if t.croc.is_some() { send_limit } else { ask_limit },
            };
            if expired {
                t.view.state = TransferState::Failed;
                t.view.message = Some(match t.view.direction {
                    Direction::Send => format!("{} didn't connect", t.view.peer),
                    Direction::Receive if t.croc.is_some() => format!("Couldn't connect to {}", t.view.peer),
                    Direction::Receive => format!("{}'s app didn't answer", t.view.peer),
                });
                t.croc = None;
            }
        }
    }

    async fn on_croc(&mut self, id: u64, ev: CrocEvent) {
        let Some(t) = self.transfers.iter_mut().find(|t| t.view.id == id) else { return };
        if matches!(t.view.state, TransferState::Cancelled | TransferState::Failed | TransferState::Done) {
            return;
        }
        match ev {
            CrocEvent::Started => t.view.state = TransferState::Running,
            CrocEvent::Progress(p) => {
                t.view.state = TransferState::Running;
                t.view.progress = p;
            }
            CrocEvent::Finished(Err(e)) => {
                t.croc = None;
                t.view.state = TransferState::Failed;
                t.view.message = Some(e);
            }
            CrocEvent::Finished(Ok(())) => {
                t.croc = None;
                if t.view.direction == Direction::Send {
                    t.view.state = TransferState::Done;
                    return;
                }
                // Received: check it's exactly the offered copy before using it.
                let offer = t.offer.clone();
                t.view.state = TransferState::Checking;
                let Some(offer) = offer else { return };
                let Some(name) = share::safe_file_name(&offer.name) else {
                    return self.fail(id, "the file name wasn't usable");
                };
                let path = self.options.downloads.join(name);
                let p = path.clone();
                let fp = tokio::task::spawn_blocking(move || fingerprint(&p)).await.ok().and_then(Result::ok);
                if fp.as_deref() != Some(offer.fingerprint.as_str()) {
                    return self.fail(id, "the file that arrived doesn't match the offered copy");
                }
                if let Some(t) = self.transfers.iter_mut().find(|t| t.view.id == id) {
                    t.view.state = TransferState::Done;
                    t.view.progress.percent = 100;
                    t.view.path = Some(path.clone());
                }
                // Use it now, unless already watching a matching copy.
                if self.fingerprint.as_deref() != Some(offer.fingerprint.as_str()) {
                    self.open_file(path).await;
                }
            }
        }
    }

    async fn on_peer_msg(&mut self, id: u64, msg: Msg) {
        let Role::Host { peers } = &mut self.role else {
            return;
        };
        let Some(peer) = peers.get_mut(&id) else {
            return;
        };
        peer.last_seen = Instant::now();
        match msg {
            Msg::Hello {
                name,
                version,
                id: peer_id,
                code,
            } => {
                peer.name = name.clone();
                peer.version = version;
                peer.id = peer_id.clone();
                peer.code = code;
                // Reconnecting after a blip: drop the stale entry for this Mac.
                if !peer_id.is_empty() {
                    peers.retain(|other, p| *other == id || p.id != peer_id);
                }
                let Some(peer) = peers.get(&id) else { return };
                let _ = peer.tx.send(Msg::State(self.state.clone()));
                let _ = peer.tx.send(Msg::Crop {
                    crop: self.crop,
                    by: self.crop_by.clone().unwrap_or_default(),
                });
                self.osd(&format!("{name} joined")).await;
                self.refresh_peers();
            }
            Msg::Rename { name } => {
                let name: String = name.trim().chars().take(MAX_NAME).collect();
                if !name.is_empty() {
                    peer.name = name;
                    self.refresh_peers();
                }
            }
            Msg::Ping { t0 } => {
                let _ = peer.tx.send(Msg::Pong {
                    t0,
                    host_ms: now_ms(),
                });
            }
            Msg::Status {
                fingerprint,
                drift_ms,
            } => {
                peer.fingerprint = fingerprint;
                peer.drift_ms = drift_ms;
            }
            Msg::Intent(intent) => {
                let name = peer.name.clone();
                self.apply_intent(intent, name).await;
            }
            // Chat: the host vouches for who sent it.
            Msg::Chat { text, .. } => {
                let from = peer.name.clone();
                if let Some(text) = clean_chat(&text) {
                    self.relay(Msg::Chat { from: from.clone(), text: text.clone() }, Some(id));
                    self.heard(from, Some(text), None).await;
                }
            }
            Msg::Offer(mut offer) => {
                // The host vouches for who offers it.
                offer.from_id = peer.id.clone();
                offer.from = peer.name.clone();
                if !offer.from_id.is_empty() {
                    peer.offer = Some(offer);
                    self.refresh_peers();
                }
            }
            Msg::OfferEnd { .. } => {
                peer.offer = None;
                self.refresh_peers();
            }
            Msg::FileRequest { offer_id, to, .. } => {
                let (from_id, from) = (peer.id.clone(), peer.name.clone());
                if from_id.is_empty() {
                    return;
                }
                self.route(Msg::FileRequest { offer_id, to, from_id, from });
            }
            msg @ (Msg::FileCode { .. } | Msg::FileRefused { .. }) => self.route(msg),
            Msg::Call { mic, cam } => {
                if (peer.mic, peer.cam) != (mic, cam) {
                    peer.mic = mic;
                    peer.cam = cam;
                    self.refresh_peers();
                }
            }
            Msg::Signal { to, data, .. } => {
                // The host vouches for who sent it.
                let from_id = peer.id.clone();
                if !from_id.is_empty() {
                    self.route(Msg::Signal { to, from_id, data });
                }
            }
            Msg::React { reaction, .. } => {
                let from = peer.name.clone();
                self.relay(Msg::React { from: from.clone(), reaction }, Some(id));
                self.heard(from, None, Some(reaction)).await;
            }
            _ => {}
        }
    }

    fn broadcast(&self, msg: Msg) {
        if let Role::Host { peers } = &self.role {
            for p in peers.values() {
                let _ = p.tx.send(msg.clone());
            }
        }
    }

    /// Host only: rebuild the member list and send it to everyone.
    fn refresh_peers(&mut self) {
        let Role::Host { peers } = &self.role else {
            return;
        };
        let (my_id, my_code) = self
            .endpoint
            .as_ref()
            .map(|e| (net::id_of(e), net::code_of(e)))
            .unwrap_or_default();
        let mut list = vec![PeerInfo {
            name: self.name.clone(),
            is_host: true,
            fingerprint: self.fingerprint.clone(),
            drift_ms: self.drift_ms,
            version: net::APP_VERSION.to_string(),
            id: my_id,
            code: my_code,
            file_name: self.file_name.clone(),
            mic: self.mic,
            cam: self.cam,
        }];
        // Join order: this is also the order of succession.
        let mut others: Vec<_> = peers.iter().collect();
        others.sort_by_key(|(id, _)| **id);
        list.extend(others.into_iter().map(|(_, p)| PeerInfo {
            name: p.name.clone(),
            is_host: false,
            fingerprint: p.fingerprint.clone(),
            drift_ms: p.drift_ms,
            version: p.version.clone(),
            id: p.id.clone(),
            code: p.code.clone(),
            file_name: None,
            mic: p.mic,
            cam: p.cam,
        }));
        self.room_peers = list.clone();
        self.broadcast(Msg::Peers(list));
        let mut offers: Vec<Offer> = self.my_offer.iter().map(|(o, _)| o.clone()).collect();
        if let Role::Host { peers } = &self.role {
            offers.extend(peers.values().filter_map(|p| p.offer.clone()));
        }
        self.offers = offers.clone();
        self.broadcast(Msg::Offers(offers));
    }

    async fn osd(&self, text: &str) {
        if let Some(mpv) = &self.mpv {
            mpv.show_text(text).await;
        }
    }

    // ------------------------------------------------------------ snapshot

    fn publish(&self) {
        if let Some(relay) = &self.relay {
            relay.set_peers(self.room_peers.iter().map(|p| (p.id.as_str(), p.code.as_str())));
        }
        let host = self.room_peers.iter().find(|p| p.is_host);
        let host_fp = host.and_then(|p| p.fingerprint.clone());
        let host_version = host.map(|p| p.version.clone()).unwrap_or_default();
        let status = |fp: &Option<String>| match (fp, &host_fp) {
            (None, _) => FileStatus::NoFile,
            (Some(a), Some(b)) if a == b => FileStatus::Match,
            (Some(_), Some(_)) => FileStatus::Mismatch,
            (Some(_), None) => FileStatus::Unknown,
        };
        let peers = self
            .room_peers
            .iter()
            .map(|p| PeerRow {
                name: p.name.clone(),
                is_host: p.is_host,
                file: status(&p.fingerprint),
                drift_ms: p.drift_ms,
                version: p.version.clone(),
                outdated: !p.is_host && p.version != host_version,
                id: p.id.clone(),
                mic: p.mic,
                cam: p.cam,
            })
            .collect();
        let snapshot = Snapshot {
            name: self.name.clone(),
            phase: self.phase,
            is_host: self.is_host(),
            connected: !matches!(self.role, Role::None),
            code: self.code.clone(),
            file_name: self.file_name.clone(),
            player_open: self.mpv.is_some(),
            playing: self.state.playing,
            position: self.state.expected_position(self.host_now()),
            duration: self.duration,
            last_action: self.state.label(),
            peers,
            error: self.error.clone(),
            my_drift_ms: self.drift_ms,
            player_socket: self.mpv.as_ref().map(|m| m.socket().to_path_buf()),
            my_file: status(&self.fingerprint),
            crop: self.crop,
            crop_by: self.crop_by.clone(),
            crop_applied: self.applied_crop == Some((self.mpv_gen, self.crop)),
            reconnecting: matches!(self.role, Role::Guest { link: None, .. }),
            reconnects: self.reconnects,
            chat: self.chat.clone(),
            tracks: self.tracks.clone(),
            comfort: self.comfort,
            invite_link: self.code.as_deref().filter(|_| self.is_host()).map(net::invite_link),
            offers: {
                let me = self.my_id();
                self.offers.iter().filter(|o| o.from_id != me).cloned().collect()
            },
            sharing: self.my_offer.is_some(),
            transfers: self.transfers.iter().map(|t| t.view.clone()).collect(),
            found: self.found.clone(),
            my_id: self.my_id(),
            mic: self.mic,
            cam: self.cam,
            player_fullscreen: self.player_fullscreen,
            turn: self.relay.as_ref().map(|r| r.info.clone()),
        };
        // Bounded(1) channel: replace any snapshot the UI hasn't drawn yet.
        let _ = self.ui.force_send(snapshot);
    }
}

/// One line, trimmed, at most MAX_CHAT characters; None if empty.
fn clean_chat(text: &str) -> Option<String> {
    let one_line: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_CHAT)
        .collect();
    let t = one_line.trim();
    (!t.is_empty()).then(|| t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_is_one_trimmed_line() {
        assert_eq!(clean_chat("  hi\nthere\t "), Some("hi there".into()));
        assert_eq!(clean_chat("   "), None);
        assert_eq!(clean_chat(&"x".repeat(500)).unwrap().len(), MAX_CHAT);
    }
}
