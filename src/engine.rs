//! The sync engine. Runs on a tokio runtime and owns all state: the room,
//! the mpv player and the shared playback state. The UI sends it `UiCmd`s
//! and renders the `Snapshot`s it publishes.

use crate::fingerprint::fingerprint;
use crate::mpv::{Mpv, MpvEvent};
use crate::net::{self, Intent, Msg, NetEvent, PeerInfo};
use crate::sync::{
    ClockSync, Correction, Crop, PLAY_LEAD_MS, PlaybackState, correction, crop_geometry,
    format_time, now_ms,
};
use serde::{Deserialize, Serialize};
use iroh::Endpoint;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
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
    Leave,
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
}

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
}

enum Role {
    None,
    Host { peers: HashMap<u64, Peer> },
    Guest { tx: mpsc::UnboundedSender<Msg> },
}

struct Peer {
    tx: mpsc::UnboundedSender<Msg>,
    name: String,
    fingerprint: Option<String>,
    drift_ms: Option<f64>,
    version: String,
}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// After we pause/unpause mpv ourselves, pause events within this window are ours.
const OWN_PAUSE_GRACE: Duration = Duration::from_millis(400);

struct Engine {
    name: String,
    ui: async_channel::Sender<Snapshot>,
    net_tx: mpsc::UnboundedSender<NetEvent>,
    mpv_tx: mpsc::UnboundedSender<(u64, MpvEvent)>,

    phase: Phase,
    role: Role,
    _endpoint: Option<Endpoint>,
    code: Option<String>,
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
}

pub async fn run(
    name: String,
    ui: async_channel::Sender<Snapshot>,
    cmds: async_channel::Receiver<UiCmd>,
) {
    let (net_tx, mut net_rx) = mpsc::unbounded_channel();
    let (mpv_tx, mut mpv_rx) = mpsc::unbounded_channel();
    let now = Instant::now();
    let mut e = Engine {
        name,
        ui,
        net_tx,
        mpv_tx,
        phase: Phase::Home,
        role: Role::None,
        _endpoint: None,
        code: None,
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
            _ = tick.tick() => e.on_tick().await,
        }
        e.publish();
    }
}

impl Engine {
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
                match timeout(CONNECT_TIMEOUT, net::host(self.net_tx.clone()))
                    .await
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("timed out. Are you online?")))
                {
                    Ok((endpoint, code)) => {
                        self._endpoint = Some(endpoint);
                        self.code = Some(code);
                        self.role = Role::Host {
                            peers: HashMap::new(),
                        };
                        self.phase = Phase::Room;
                        self.refresh_peers();
                    }
                    Err(e) => {
                        self.error = Some(format!("Couldn't start a room: {e}"));
                        self.phase = Phase::Home;
                    }
                }
            }
            UiCmd::Join { code } if self.phase == Phase::Home => {
                self.phase = Phase::Connecting;
                self.publish();
                let joining = net::join(&code, self.name.clone(), self.net_tx.clone());
                match timeout(CONNECT_TIMEOUT, joining)
                    .await
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("timed out. Is the host still online?")))
                {
                    Ok((endpoint, tx)) => {
                        // A few quick clock probes so the offset is good from the start.
                        for _ in 0..5 {
                            let _ = tx.send(Msg::Ping { t0: now_ms() });
                        }
                        self._endpoint = Some(endpoint);
                        self.code = Some(code.trim().to_string());
                        self.role = Role::Guest { tx };
                        self.phase = Phase::Room;
                    }
                    Err(e) => {
                        self.error = Some(format!("Couldn't join: {e}"));
                        self.phase = Phase::Home;
                    }
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
            UiCmd::Leave => self.leave(),
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

    /// Leave the room and close the player. (Hosting ends the room.)
    fn leave(&mut self) {
        self.role = Role::None;
        self._endpoint = None;
        self.code = None;
        self.phase = Phase::Home;
        self.room_peers.clear();
        self.state = PlaybackState::default();
        self.clock = ClockSync::default();
        self.crop = Crop::Original;
        self.crop_by = None;
        self.mpv = None;
        self.mpv_gen += 1;
        self.applied_crop = None;
        self.drift_ms = None;
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
                self.resync().await;
                self.apply_crop().await;
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
        } else if let Role::Guest { tx } = &self.role {
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
        self.state = s;
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
        if self.is_host() {
            self.refresh_peers();
        } else if let Role::Guest { tx } = &self.role {
            let _ = tx.send(Msg::Ping { t0: now_ms() });
            let _ = tx.send(Msg::Status {
                fingerprint: self.fingerprint.clone(),
                drift_ms: self.drift_ms,
            });
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
            MpvEvent::Exited => {
                self.mpv = None;
                self.drift_ms = None;
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
            NetEvent::FromHost(msg) => match msg {
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
                _ => {}
            },
            NetEvent::HostLost => {
                if matches!(self.role, Role::Guest { .. }) {
                    self.role = Role::None;
                    self.room_peers.clear();
                    self.error = Some("Lost connection to the host. The room has ended.".into());
                    self.osd("Lost connection to the host").await;
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
        match msg {
            Msg::Hello { name, version } => {
                peer.name = name.clone();
                peer.version = version;
                let _ = peer.tx.send(Msg::State(self.state.clone()));
                let _ = peer.tx.send(Msg::Crop {
                    crop: self.crop,
                    by: self.crop_by.clone().unwrap_or_default(),
                });
                self.osd(&format!("{name} joined")).await;
                self.refresh_peers();
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
        let mut list = vec![PeerInfo {
            name: self.name.clone(),
            is_host: true,
            fingerprint: self.fingerprint.clone(),
            drift_ms: self.drift_ms,
            version: net::APP_VERSION.to_string(),
        }];
        let mut others: Vec<_> = peers.values().collect();
        others.sort_by(|a, b| a.name.cmp(&b.name));
        list.extend(others.into_iter().map(|p| PeerInfo {
            name: p.name.clone(),
            is_host: false,
            fingerprint: p.fingerprint.clone(),
            drift_ms: p.drift_ms,
            version: p.version.clone(),
        }));
        self.room_peers = list.clone();
        self.broadcast(Msg::Peers(list));
    }

    async fn osd(&self, text: &str) {
        if let Some(mpv) = &self.mpv {
            mpv.show_text(text).await;
        }
    }

    // ------------------------------------------------------------ snapshot

    fn publish(&self) {
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
        };
        // Bounded(1) channel: replace any snapshot the UI hasn't drawn yet.
        let _ = self.ui.force_send(snapshot);
    }
}
