//! One peer's view of the session, and the rules it follows — with no
//! sockets and no clock of its own, so every rule can be run under test.
//!
//! The [`Engine`] is fed what arrives ([`Engine::on_discovery`],
//! [`Engine::on_measured`]) and the time ([`Engine::tick`]), and answers
//! with [`Action`]s for the network layer to carry out. It never sleeps and
//! never reads a clock: every call is told `now` in host µs.
//!
//! The rules, as this crate implements them:
//!
//! 1. **Announce.** A joined peer multicasts an `Alive` every
//!    [`Config::announce_every`] carrying its timeline, session, start/stop
//!    state and measurement endpoint, with a ttl of [`Config::ttl`]
//!    seconds. A peer not heard from within its ttl is forgotten; a
//!    `ByeBye` forgets it at once. A newcomer's first `Alive` is answered
//!    with a unicast `Response`, so it learns about us before our next
//!    announcement.
//! 2. **Join.** A peer that is not yet in a session measures a member of
//!    the first session it hears (see [`crate::measure`]) and, once it has
//!    the offset, adopts that session's id, timeline and start/stop state.
//! 3. **Follow.** In the session, a member's timeline is adopted when it
//!    is newer than ours ([`Timeline::newer_than`]: a change is made at a
//!    later beat than the line it changes). So an old announcement still
//!    in flight cannot undo a newer change of ours, and two changes that
//!    cross on the network settle on one everywhere instead of swapping
//!    places at network speed — a multicast storm. The start/stop state with the
//!    latest timestamp wins. What is adopted is passed on at most once per
//!    [`REANNOUNCE_MIN`]; a change in between goes out with the next.
//! 4. **Found** (optional — [`Config::found_after`], or on demand with
//!    [`Engine::found`]). A peer that has heard no session for that long
//!    founds one: its session id is its node id, and its ghost clock
//!    starts at 0 — a session's ghost time is its age. A peer that never
//!    founds by itself is silent until it joins: no session of its own
//!    exists for a later one to lose to, so the first session it hears
//!    keeps its tempo and it follows. [`Engine::leave`] puts a peer back
//!    there.
//! 5. **Merge.** Two sessions meeting become one by the rule every
//!    Ableton peer follows, so that all of them pick the same winner: a
//!    joined peer measures each foreign session it has not yet decided
//!    on, and the **older** session wins — the one whose ghost time is
//!    ahead of ours by more than [`SESSION_EPS`] — with a tie inside that
//!    margin going to the id that sorts first. A peer that loses moves,
//!    timeline and all; one that wins remembers the decision until that
//!    session's last peer is gone. A newcomer is the youngest session in
//!    the room (an Ableton peer resets its ghost clock to 0 when Link is
//!    switched on), so it joins the session it finds — whoever founded it.
//! 6. **Re-measure** every [`Config::remeasure_every`] against a member, so
//!    drift between the two machines' clocks is taken up.
//! 7. **Move the beat** (Ableton's `requestBeatAtTime` and
//!    `forceBeatAtTime`). What peers share is the session's *phase*; the
//!    beat's *number* is each peer's own. So a peer keeps a local
//!    [`LocalBeats::beat_offset`] (its beats minus the session's), and
//!    moving the beat has two parts: shifting the session's line by less
//!    than half a quantum, so that the beat's phase lands where asked —
//!    which every peer sees — and setting the offset, so that the beat's
//!    number does too — which nobody else sees.
//!    [`Engine::force_beat_at`] does both, always: it commits a new line
//!    (anchored past the old one, [`Timeline::committed_after`], so every
//!    peer adopts it) and announces it like a tempo change.
//!    [`Engine::request_beat_at`] does both only while alone in the
//!    session; with peers it moves the session nowhere and only sets the
//!    offset, to a whole number of quanta, so the beat lands at the first
//!    moment from then on with its phase. The offset is reset when the
//!    peer founds, joins (or merges into) a session, or leaves.

use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, SocketAddr, SocketAddrV4};

use crate::timeline::{
    GhostXForm, MICROBEATS, Micros, StartStop, Timeline, closest_phase_match, next_boundary,
    next_phase_match, phase,
};
use crate::wire::{Discovery, DiscoveryKind, Entry, NodeId, SessionId};

/// How a peer behaves; the defaults are the bridge's (follow, never found).
#[derive(Copy, Clone, Debug)]
pub struct Config {
    /// The tempo a founded session starts at.
    pub bpm: f64,
    /// Found a session after hearing none for this long (µs); `None` —
    /// only ever join one.
    pub found_after: Option<Micros>,
    /// Seconds a peer may go unheard before others forget it.
    pub ttl: u8,
    /// µs between two announcements.
    pub announce_every: Micros,
    /// µs between two re-measurements of the session's clock.
    pub remeasure_every: Micros,
    /// µs to wait after a failed measurement before trying again.
    pub retry_after: Micros,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            bpm: 120.0,
            found_after: None,
            ttl: 5,
            announce_every: 250_000,
            remeasure_every: 30_000_000,
            retry_after: 1_000_000,
        }
    }
}

/// Where a peer is in joining.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// No session yet; listening.
    Searching,
    /// Measuring a session's clock before joining it.
    Measuring,
    /// In a session, announcing.
    Joined,
}

/// What the network layer must do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Multicast this message to the discovery group — on every interface
    /// the network layer is on, each copy's `mep4` moved to that
    /// interface's own address ([`Discovery::with_endpoint_ip`]).
    Broadcast(Discovery),
    /// Send this message to one peer, its `mep4` moved to the address that
    /// peer reaches us at.
    Send(SocketAddr, Discovery),
    /// Measure `session`'s clock through `endpoint`, then call
    /// [`Engine::on_measured`].
    Measure { session: SessionId, endpoint: SocketAddrV4 },
}

#[derive(Clone, Debug)]
struct Peer {
    session: SessionId,
    timeline: Timeline,
    start_stop: Option<StartStop>,
    /// The measurement endpoint the peer announced, once per source address
    /// it was heard from. A peer on several interfaces announces a
    /// different `mep4` on each — its loopback address to this machine, its
    /// LAN address to the network — and each is reachable only by the path
    /// it came over, so they are kept apart rather than overwriting each
    /// other.
    endpoints: Vec<Heard>,
    /// Host µs the peer was last heard from.
    heard: Micros,
    /// Host µs after which the peer is forgotten.
    expires: Micros,
}

/// One `mep4`, and where and until when it holds.
#[derive(Copy, Clone, Debug)]
struct Heard {
    from: IpAddr,
    endpoint: SocketAddrV4,
    expires: Micros,
}

impl Peer {
    /// The endpoint to measure through: a loopback one if the peer offers
    /// it (it is on this machine, and loopback is the shortest round trip
    /// there is), else the most recently heard.
    fn endpoint(&self) -> Option<SocketAddrV4> {
        self.endpoints
            .iter()
            .max_by_key(|h| (h.endpoint.ip().is_loopback(), h.expires))
            .map(|h| h.endpoint)
    }
}

/// One peer this one hears, as a plain copy — for a display ("who is
/// out there"), not for any rule. Link's messages carry no application
/// name: what a peer is known by is its node id and where it is heard from.
#[derive(Clone, Debug, PartialEq)]
pub struct PeerInfo {
    pub node: NodeId,
    /// The session the peer announces.
    pub session: SessionId,
    /// Is it in ours? (Never while we are not in one.)
    pub in_session: bool,
    /// Its timeline as last announced — its tempo.
    pub timeline: Timeline,
    pub start_stop: Option<StartStop>,
    /// The paths it is heard over, most recently heard first.
    pub paths: Vec<PeerPath>,
    /// Host µs it was last heard from.
    pub heard: Micros,
}

/// One path a peer is heard over: the address its messages come from and
/// the measurement endpoint (`mep4`) it announces there.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PeerPath {
    pub from: IpAddr,
    pub endpoint: SocketAddrV4,
}

/// A copy of the session as one peer sees it — everything a sequencer or a
/// clock bridge asks, answered in **host** time.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SessionState {
    pub status: Status,
    pub session: Option<SessionId>,
    /// Other peers in our session.
    pub peers: usize,
    /// Did we found it?
    pub founder: bool,
    pub timeline: Timeline,
    pub xform: GhostXForm,
    /// The shared transport, if any member has ever set one.
    pub start_stop: Option<StartStop>,
}

impl SessionState {
    pub fn joined(&self) -> bool {
        self.status == Status::Joined
    }

    pub fn tempo(&self) -> f64 {
        self.timeline.bpm()
    }

    /// The session's beat at host time `host`.
    pub fn beat_at(&self, host: Micros) -> f64 {
        self.timeline.beat_at(self.xform.host_to_ghost(host))
    }

    /// Where the beat at `host` sits in a quantum — what every peer with the
    /// same quantum agrees on.
    pub fn phase_at(&self, host: Micros, quantum: f64) -> f64 {
        phase(self.beat_at(host), quantum)
    }

    /// The host time at which the session reaches `beat`.
    pub fn host_at_beat(&self, beat: f64) -> Micros {
        self.xform.ghost_to_host(self.timeline.ghost_at(beat))
    }

    /// The first quantum boundary at or after the beat at `host`.
    pub fn next_boundary(&self, host: Micros, quantum: f64) -> f64 {
        next_boundary(self.beat_at(host), quantum)
    }

    /// Is the shared transport playing? `None` if nobody in the session
    /// uses start/stop sync.
    pub fn is_playing(&self) -> Option<bool> {
        self.start_stop.map(|s| s.playing)
    }
}

/// The session as this peer counts it: a [`SessionState`] (the shared
/// line, in the session's beats) and this peer's own numbering of those
/// beats (module docs, rule 7). [`Engine::local`] and [`crate::Link::local`]
/// give one. Phase always comes from the session, so the offset never moves
/// it (Link's phase-encoded beats).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct LocalBeats {
    pub state: SessionState,
    /// This peer's beats minus the session's: 0 unless
    /// [`Engine::request_beat_at`] or [`Engine::force_beat_at`] renumbered
    /// them. Never shared.
    pub beat_offset: f64,
}

impl LocalBeats {
    /// This peer's beat at host time `host`: the session's, plus the offset.
    pub fn beat_at(&self, host: Micros) -> f64 {
        self.state.beat_at(host) + self.beat_offset
    }

    /// The session's phase at `host` ([`SessionState::phase_at`]).
    pub fn phase_at(&self, host: Micros, quantum: f64) -> f64 {
        self.state.phase_at(host, quantum)
    }

    /// The host time at which this peer reaches `beat`, in its own beats.
    pub fn host_at_beat(&self, beat: f64) -> Micros {
        self.state.host_at_beat(beat - self.beat_offset)
    }

    /// The first beat, in this peer's beats, at or after the beat at
    /// `host` where the session's phase is 0.
    pub fn next_boundary(&self, host: Micros, quantum: f64) -> f64 {
        self.state.next_boundary(host, quantum) + self.beat_offset
    }
}

/// One peer's session logic. See the module docs for the rules.
#[derive(Debug)]
pub struct Engine {
    node: NodeId,
    config: Config,
    status: Status,
    session: SessionId,
    founder: bool,
    timeline: Timeline,
    xform: GhostXForm,
    start_stop: Option<StartStop>,
    /// Our beats minus the session's (rule 7).
    beat_offset: f64,
    endpoint: Option<SocketAddrV4>,
    peers: BTreeMap<NodeId, Peer>,
    /// Foreign sessions measured and found not to win over ours (rule 5),
    /// kept until no peer is in them.
    outranked: BTreeSet<SessionId>,
    searching_since: Micros,
    last_announce: Option<Micros>,
    /// Something adopted from a member is waiting for [`REANNOUNCE_MIN`]
    /// to pass before it is announced.
    announce_due: bool,
    measuring: Option<(SessionId, Micros)>,
    measured_at: Micros,
    no_retry_before: Micros,
}

/// A measurement that has not answered in this long is given up on.
const MEASURE_TIMEOUT: Micros = 5_000_000;
/// Two sessions whose ghost times are closer than this are a tie, settled
/// by id (rule 5) — Ableton's own margin.
pub const SESSION_EPS: Micros = 500_000;
/// What a member announced is passed on at most this often (Ableton's
/// gateway keeps the same minimum between broadcasts): whatever happens,
/// a peer's announcements stay countable.
pub const REANNOUNCE_MIN: Micros = 50_000;

impl Engine {
    pub fn new(node: NodeId, config: Config, now: Micros) -> Self {
        Engine {
            node,
            config,
            status: Status::Searching,
            session: node,
            founder: false,
            timeline: Timeline::at(config.bpm, 0.0, now),
            xform: GhostXForm::default(),
            start_stop: None,
            beat_offset: 0.0,
            endpoint: None,
            peers: BTreeMap::new(),
            outranked: BTreeSet::new(),
            searching_since: now,
            last_announce: None,
            announce_due: false,
            measuring: None,
            measured_at: now,
            no_retry_before: now,
        }
    }

    pub fn node(&self) -> NodeId {
        self.node
    }

    /// Where our measurement responder listens — announced as `mep4`.
    pub fn set_endpoint(&mut self, endpoint: SocketAddrV4) {
        self.endpoint = Some(endpoint);
    }

    /// Ghost time at host time `host`, if we are in a session and so have
    /// one to give — what the measurement responder answers with.
    pub fn ghost(&self, host: Micros) -> Option<Micros> {
        (self.status == Status::Joined).then(|| self.xform.host_to_ghost(host))
    }

    /// The measurement responder's answer to `ping` at host time `host`:
    /// our session and its ghost time. Silent until we are in a session.
    pub fn pong(&self, ping: &[u8], host: Micros) -> Option<Vec<u8>> {
        crate::measure::pong(ping, self.session, self.ghost(host)?)
    }

    pub fn state(&self) -> SessionState {
        SessionState {
            status: self.status,
            session: (self.status == Status::Joined).then_some(self.session),
            peers: self.members(),
            founder: self.founder,
            timeline: self.timeline,
            xform: self.xform,
            start_stop: self.start_stop,
        }
    }

    /// Our beats minus the session's (rule 7).
    pub fn beat_offset(&self) -> f64 {
        self.beat_offset
    }

    /// [`Engine::state`] with our own beat numbering.
    pub fn local(&self) -> LocalBeats {
        LocalBeats { state: self.state(), beat_offset: self.beat_offset }
    }

    /// Every peer heard and not yet forgotten, in node-id order.
    pub fn peers(&self) -> Vec<PeerInfo> {
        let joined = self.status == Status::Joined;
        self.peers
            .iter()
            .map(|(node, p)| {
                let mut heard: Vec<&Heard> = p.endpoints.iter().collect();
                heard.sort_by_key(|h| std::cmp::Reverse(h.expires));
                PeerInfo {
                    node: *node,
                    session: p.session,
                    in_session: joined && p.session == self.session,
                    timeline: p.timeline,
                    start_stop: p.start_stop,
                    paths: heard
                        .into_iter()
                        .map(|h| PeerPath { from: h.from, endpoint: h.endpoint })
                        .collect(),
                    heard: p.heard,
                }
            })
            .collect()
    }

    fn members(&self) -> usize {
        self.peers.values().filter(|p| p.session == self.session).count()
    }

    fn message(&self, kind: DiscoveryKind) -> Discovery {
        let mut entries = Vec::with_capacity(4);
        if kind != DiscoveryKind::ByeBye {
            entries.push(Entry::Timeline(self.timeline));
            entries.push(Entry::Session(self.session));
            if let Some(s) = self.start_stop {
                entries.push(Entry::StartStop(s));
            }
            if let Some(e) = self.endpoint {
                entries.push(Entry::Endpoint4(e));
            }
        }
        Discovery { kind, ttl: self.config.ttl, group: 0, node: self.node, entries }
    }

    /// What to multicast on the way out, if we were announcing.
    pub fn byebye(&self) -> Option<Vec<u8>> {
        (self.status == Status::Joined).then(|| self.message(DiscoveryKind::ByeBye).encode())
    }

    fn announce(&mut self, now: Micros) -> Action {
        self.last_announce = Some(now);
        self.announce_due = false;
        Action::Broadcast(self.message(DiscoveryKind::Alive))
    }

    fn measure(&mut self, session: SessionId, now: Micros) -> Option<Action> {
        if self.measuring.is_some() || now < self.no_retry_before {
            return None;
        }
        // Prefer the founder (its ghost time is its own clock), then anyone.
        let endpoint = self
            .peers
            .get(&session)
            .filter(|p| p.session == session)
            .and_then(Peer::endpoint)
            .or_else(|| {
                self.peers.values().filter(|p| p.session == session).find_map(Peer::endpoint)
            })?;
        self.measuring = Some((session, now));
        if self.status == Status::Searching {
            self.status = Status::Measuring;
        }
        Some(Action::Measure { session, endpoint })
    }

    /// A discovery message arrived from `from`.
    pub fn on_discovery(&mut self, from: SocketAddr, msg: &Discovery, now: Micros) -> Vec<Action> {
        let mut out = Vec::new();
        if msg.node == self.node {
            return out; // our own multicast, looped back
        }
        if msg.kind == DiscoveryKind::ByeBye {
            self.peers.remove(&msg.node);
            return out;
        }
        let (Some(session), Some(timeline)) = (msg.session(), msg.timeline()) else {
            return out;
        };
        if !timeline.is_sane() {
            return out;
        }
        let expires = now + Micros::from(msg.ttl.max(1)) * 1_000_000;
        let mut endpoints = self.peers.get(&msg.node).map_or(Vec::new(), |p| p.endpoints.clone());
        if let Some(endpoint) = msg.endpoint() {
            let heard = Heard { from: from.ip(), endpoint, expires };
            match endpoints.iter_mut().find(|h| h.from == heard.from) {
                Some(h) => *h = heard,
                None => endpoints.push(heard),
            }
        }
        let peer = Peer {
            session,
            timeline,
            start_stop: msg.start_stop(),
            endpoints,
            heard: now,
            expires,
        };
        let previous = self.peers.insert(msg.node, peer);
        let joined = self.status == Status::Joined;
        if joined && previous.is_none() && msg.kind == DiscoveryKind::Alive {
            out.push(Action::Send(from, self.message(DiscoveryKind::Response)));
        }
        if joined && session == self.session {
            let mut announce = false;
            if timeline.newer_than(&self.timeline) {
                self.timeline = timeline;
                announce = true;
            }
            if let Some(s) = msg.start_stop()
                && self.start_stop.is_none_or(|ours| s.timestamp > ours.timestamp)
            {
                self.start_stop = Some(s);
                announce = true;
            }
            if announce {
                if self.last_announce.is_some_and(|t| now - t < REANNOUNCE_MIN) {
                    self.announce_due = true;
                } else {
                    out.push(self.announce(now));
                }
            }
        } else if !self.outranked.contains(&session) {
            // Not yet in a session: the first one heard is joined. In one:
            // a foreign session is measured to see which of the two wins.
            out.extend(self.measure(session, now));
        }
        out
    }

    /// A measurement of `session` finished: `Some(intercept)` on success.
    pub fn on_measured(
        &mut self,
        session: SessionId,
        intercept: Option<Micros>,
        now: Micros,
    ) -> Vec<Action> {
        if self.measuring.is_some_and(|(s, _)| s == session) {
            self.measuring = None;
        }
        if session == self.session {
            // A comparison with a foreign session does not put off the
            // next re-measurement of our own.
            self.measured_at = now;
        }
        let Some(intercept) = intercept else {
            self.no_retry_before = now + self.config.retry_after;
            if self.status == Status::Measuring {
                self.status = Status::Searching;
            }
            return Vec::new();
        };
        if self.status == Status::Joined {
            if session == self.session {
                self.xform = GhostXForm { intercept };
                return Vec::new();
            }
            let ahead = (now + intercept) - self.xform.host_to_ghost(now);
            let theirs_wins =
                ahead > SESSION_EPS || (ahead.abs() < SESSION_EPS && session < self.session);
            if !theirs_wins {
                self.outranked.insert(session);
                return Vec::new();
            }
        }
        // Join: the newest start/stop and the founder's timeline (or any
        // member's) become ours.
        let members: Vec<&Peer> = self.peers.values().filter(|p| p.session == session).collect();
        let Some(first) = members.first() else {
            // The session left while we measured it.
            if self.status == Status::Measuring {
                self.status = Status::Searching;
            }
            return Vec::new();
        };
        let timeline = self.peers.get(&session).map_or(first.timeline, |p| p.timeline);
        let start_stop = members.iter().filter_map(|p| p.start_stop).max_by_key(|s| s.timestamp);
        if self.status == Status::Joined {
            // Merged: the session we leave is the one that lost.
            self.outranked.insert(self.session);
        }
        self.outranked.remove(&session);
        self.session = session;
        self.timeline = timeline;
        self.start_stop = start_stop;
        self.beat_offset = 0.0;
        self.xform = GhostXForm { intercept };
        self.measured_at = now;
        self.founder = false;
        self.status = Status::Joined;
        vec![self.announce(now)]
    }

    /// Time passes: forget the silent, announce, found, re-measure.
    pub fn tick(&mut self, now: Micros) -> Vec<Action> {
        let mut out = Vec::new();
        self.peers.retain(|_, p| p.expires > now);
        for p in self.peers.values_mut() {
            p.endpoints.retain(|h| h.expires > now);
        }
        let peers = &self.peers;
        self.outranked.retain(|s| peers.values().any(|p| p.session == *s));
        if let Some((session, since)) = self.measuring
            && now - since > MEASURE_TIMEOUT
        {
            out.extend(self.on_measured(session, None, now));
        }
        match self.status {
            Status::Joined => {
                let since = self.last_announce.map(|t| now - t);
                if since.is_none_or(|s| {
                    s >= self.config.announce_every || (self.announce_due && s >= REANNOUNCE_MIN)
                }) {
                    out.push(self.announce(now));
                }
                // A foreign session still undecided — heard while another
                // measurement ran, or one that failed and is due a retry.
                let undecided = self
                    .peers
                    .values()
                    .map(|p| p.session)
                    .find(|s| *s != self.session && !self.outranked.contains(s));
                if let Some(session) = undecided {
                    out.extend(self.measure(session, now));
                }
                // The founder's ghost clock is its own: nothing to measure.
                if !self.founder
                    && self.members() > 0
                    && now - self.measured_at >= self.config.remeasure_every
                {
                    let session = self.session;
                    out.extend(self.measure(session, now));
                }
            }
            Status::Searching => {
                let foreign =
                    self.peers.values().find(|p| p.endpoint().is_some()).map(|p| p.session);
                if let Some(session) = foreign {
                    out.extend(self.measure(session, now));
                } else if self
                    .config
                    .found_after
                    .is_some_and(|after| now - self.searching_since >= after)
                {
                    out.extend(self.found(self.config.bpm, 0.0, now));
                }
            }
            Status::Measuring => {}
        }
        out
    }

    /// Found a session now (rule 4, on demand): at `bpm`, with the beat at
    /// `now` being `beat` — so a peer that kept a beat of its own while it
    /// waited carries it into the session without a jump. Ignored while in
    /// a session. A measurement still running goes on; if its session
    /// turns out the older, rule 5 moves us into it.
    pub fn found(&mut self, bpm: f64, beat: f64, now: Micros) -> Vec<Action> {
        if self.status == Status::Joined {
            return Vec::new();
        }
        self.session = self.node;
        self.founder = true;
        // Ghost time starts at 0 as the session does, as an Ableton peer's
        // does, so it reads as the session's age — what rule 5 compares.
        self.xform = GhostXForm { intercept: -now };
        self.timeline = Timeline::at(bpm, beat, 0);
        self.beat_offset = 0.0;
        self.status = Status::Joined;
        vec![self.announce(now)]
    }

    /// Leave the session and listen again, as a peer that has joined
    /// none: no more announcements, no pongs, and the next session heard
    /// is joined (rule 2) — as a newcomer, so it is that session's tempo
    /// that holds. Returns the `ByeBye` to multicast; `None` (and nothing
    /// changes) if we were in no session.
    pub fn leave(&mut self, now: Micros) -> Option<Vec<u8>> {
        let bye = self.byebye()?;
        self.status = Status::Searching;
        self.session = self.node;
        self.founder = false;
        self.xform = GhostXForm::default();
        self.start_stop = None;
        self.beat_offset = 0.0;
        self.outranked.clear();
        self.last_announce = None;
        self.searching_since = now;
        Some(bye)
    }

    /// Propose a new tempo from host time `at` on. Ignored until joined.
    pub fn propose_tempo(&mut self, bpm: f64, at: Micros, now: Micros) -> Vec<Action> {
        if self.status != Status::Joined {
            return Vec::new();
        }
        self.timeline = self.timeline.retempo(bpm, self.xform.host_to_ghost(at));
        vec![self.announce(now)]
    }

    /// Ask for `beat` at host time `at` — Ableton's `requestBeatAtTime`,
    /// the polite one (rule 7). Alone in the session it is
    /// [`Engine::force_beat_at`]. With peers it moves the session nowhere
    /// and broadcasts nothing: it renumbers *our* beats by a whole number
    /// of `quantum`s, so `beat` falls at the first moment at or after `at`
    /// when the session's phase is `beat`'s. Ignored until joined.
    pub fn request_beat_at(
        &mut self,
        beat: f64,
        at: Micros,
        quantum: f64,
        now: Micros,
    ) -> Vec<Action> {
        if self.status != Status::Joined {
            return Vec::new();
        }
        if self.members() == 0 {
            return self.force_beat_at(beat, at, quantum, now);
        }
        let session = self.timeline.beat_at(self.xform.host_to_ghost(at));
        self.beat_offset = beat - next_phase_match(session, beat, quantum);
        Vec::new()
    }

    /// Put `beat` at host time `at` for the whole session — Ableton's
    /// `forceBeatAtTime`, the rude one (rule 7). The session's line moves
    /// by less than half a `quantum`, so that every peer's phase at `at` is
    /// `beat`'s, and is broadcast like a tempo change (same tempo, a new
    /// anchor every peer adopts); our own beat at `at` is `beat` exactly.
    /// It moves every other app's beat: a DJ sees the phase jump. A
    /// `quantum` ≤ 0 has no phase to move, so only our numbering changes
    /// and nothing is sent. Ignored until joined.
    pub fn force_beat_at(
        &mut self,
        beat: f64,
        at: Micros,
        quantum: f64,
        now: Micros,
    ) -> Vec<Action> {
        if self.status != Status::Joined {
            return Vec::new();
        }
        self.place(self.timeline, beat, at, quantum, now)
    }

    /// A new tempo and [`Engine::force_beat_at`] in one commit: the line
    /// re-tempoed at `at`, then moved so `beat` lands there. One
    /// broadcast, so no peer ever sees the tempo without the phase.
    /// Ignored until joined.
    pub fn propose_timeline(
        &mut self,
        bpm: f64,
        beat: f64,
        at: Micros,
        quantum: f64,
        now: Micros,
    ) -> Vec<Action> {
        if self.status != Status::Joined {
            return Vec::new();
        }
        let line = self.timeline.retempo(bpm, self.xform.host_to_ghost(at));
        self.place(line, beat, at, quantum, now)
    }

    /// Make `line`, moved so that `beat`'s phase is the session's at `at`,
    /// the session's line, and number our beats so `beat` is ours there.
    fn place(
        &mut self,
        line: Timeline,
        beat: f64,
        at: Micros,
        quantum: f64,
        now: Micros,
    ) -> Vec<Action> {
        let ghost = self.xform.host_to_ghost(at);
        let session = line.beat_at(ghost);
        let lands = closest_phase_match(session, beat, quantum);
        self.beat_offset = beat - lands;
        let line = line.shifted(lands - session);
        if line == self.timeline {
            return Vec::new(); // nothing the session would see
        }
        self.timeline = line.committed_after(ghost, &self.timeline);
        vec![self.announce(now)]
    }

    /// Propose starting or stopping the shared transport at host time `at`.
    pub fn propose_start_stop(&mut self, playing: bool, at: Micros, now: Micros) -> Vec<Action> {
        if self.status != Status::Joined {
            return Vec::new();
        }
        let ghost = self.xform.host_to_ghost(at);
        self.start_stop = Some(StartStop {
            playing,
            beats: (self.timeline.beat_at(ghost) * MICROBEATS).round() as i64,
            timestamp: ghost,
        });
        vec![self.announce(now)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measure;
    use crate::wire::DiscoveryKind;

    const FOUNDER: NodeId = NodeId(*b"founder!");
    const JOINER: NodeId = NodeId(*b"joiner!!");

    fn addr(port: u16) -> SocketAddr {
        SocketAddr::from(([10, 0, 0, 1], port))
    }

    fn founding() -> Config {
        Config { found_after: Some(1_000_000), ..Config::default() }
    }

    /// Deliver every broadcast in `actions` to `to`, returning its answers.
    fn deliver(actions: &[Action], to: &mut Engine, from: u16, now: Micros) -> Vec<Action> {
        let mut out = Vec::new();
        for a in actions {
            if let Action::Broadcast(m) | Action::Send(_, m) = a {
                // Through the bytes, so the wire format is exercised too.
                let m = Discovery::decode(&m.encode()).unwrap();
                out.extend(to.on_discovery(addr(from), &m, now));
            }
        }
        out
    }

    /// Run a whole measurement burst of `pinger` against `responder`, whose
    /// host clock reads `skew` more than the pinger's, over a network that
    /// takes 400 µs each way.
    fn measure_burst(
        responder: &Engine,
        session: SessionId,
        skew: Micros,
        start: Micros,
    ) -> Option<Micros> {
        let mut samples = Vec::new();
        for i in 0..measure::BURST as Micros {
            let sent = start + i * 10_000;
            let reply = responder.pong(&measure::ping(sent), sent + 400 + skew)?;
            samples.extend(measure::sample(&reply, session, sent, sent + 800));
        }
        measure::settle(&mut samples)
    }

    fn found(now: Micros) -> (Engine, Vec<Action>) {
        let mut e = Engine::new(FOUNDER, founding(), now);
        e.set_endpoint("10.0.0.1:4000".parse().unwrap());
        assert!(e.tick(now + 500_000).is_empty(), "not yet");
        let actions = e.tick(now + 1_000_000);
        assert_eq!(e.state().status, Status::Joined);
        assert!(e.state().founder);
        (e, actions)
    }

    #[test]
    fn a_follower_never_founds() {
        let mut e = Engine::new(JOINER, Config::default(), 0);
        assert!(e.tick(60_000_000).is_empty());
        assert_eq!(e.state().status, Status::Searching);
        assert!(e.byebye().is_none(), "nothing to say goodbye from");
    }

    /// The whole join, on two clocks 7 s apart: the joiner hears the
    /// founder, measures it, adopts its session — and then the two agree on
    /// the beat at the same real instant.
    #[test]
    fn a_joiner_measures_and_then_agrees_on_the_beat() {
        let skew = 7_000_000; // founder's host clock reads 7 s more
        let (founder, announced) = found(10_000_000);
        let mut joiner = Engine::new(JOINER, Config::default(), 3_000_000);
        let actions = deliver(&announced, &mut joiner, 1, 4_000_000);
        let [Action::Measure { session, endpoint }] = actions.as_slice() else {
            panic!("{actions:?}")
        };
        assert_eq!(*session, FOUNDER);
        assert_eq!(endpoint.port(), 4000);
        assert_eq!(joiner.state().status, Status::Measuring);
        let intercept = measure_burst(&founder, FOUNDER, skew, 4_000_000);
        // The founder's ghost clock started at 0 when it founded, at 11 s.
        let expect = skew - 11_000_000;
        assert_eq!(intercept, Some(expect), "a symmetric network measures the offset exactly");
        let joined = joiner.on_measured(FOUNDER, intercept, 4_300_000);
        assert!(matches!(joined.as_slice(), [Action::Broadcast(_)]));
        let s = joiner.state();
        assert_eq!(s.session, Some(FOUNDER));
        assert!(!s.founder);
        for real in [4_300_000, 9_000_000, 60_000_000] {
            let b_joiner = s.beat_at(real);
            let b_founder = founder.state().beat_at(real + skew);
            assert!((b_joiner - b_founder).abs() < 1e-9, "{b_joiner} vs {b_founder}");
            // And back: the same beat lands on the same real instant.
            assert!((s.host_at_beat(b_joiner) - real).abs() <= 1);
        }
    }

    fn joined_pair() -> (Engine, Engine) {
        let (mut founder, announced) = found(0);
        let mut joiner = Engine::new(JOINER, Config::default(), 0);
        deliver(&announced, &mut joiner, 1, 1_100_000);
        // The joiner announces as it joins; the founder hears it and answers.
        let answer =
            joiner.on_measured(FOUNDER, measure_burst(&founder, FOUNDER, 0, 1_100_000), 1_500_000);
        let back = deliver(&answer, &mut founder, 2, 1_500_000);
        assert!(matches!(back.as_slice(), [Action::Send(..)]), "{back:?}");
        assert_eq!(founder.state().peers, 1);
        assert_eq!(joiner.state().peers, 1);
        (founder, joiner)
    }

    #[test]
    fn a_tempo_change_reaches_the_other_peer_without_moving_the_beat() {
        let (mut founder, mut joiner) = joined_pair();
        let at = 3_000_000;
        let before = joiner.state().beat_at(at);
        let change = founder.propose_tempo(90.0, at, at);
        deliver(&change, &mut joiner, 1, at + 1000);
        let s = joiner.state();
        assert!((s.tempo() - 90.0).abs() < 1e-3);
        assert!((s.beat_at(at) - before).abs() < 1e-6, "the beat does not jump");
        assert!((founder.state().tempo() - 90.0).abs() < 1e-3);
    }

    /// Our own change is not undone by a member's announcement that was
    /// already in flight: it is not a change relative to what that member
    /// said before, so it is not adopted.
    #[test]
    fn an_announcement_in_flight_does_not_undo_our_change() {
        let (mut founder, mut joiner) = joined_pair();
        let at = 3_000_000;
        let in_flight = founder.tick(at); // still 120 BPM
        let change = joiner.propose_tempo(90.0, at, at);
        deliver(&in_flight, &mut joiner, 1, at + 100);
        assert!((joiner.state().tempo() - 90.0).abs() < 1e-3, "kept");
        deliver(&change, &mut founder, 2, at + 200);
        assert!((founder.state().tempo() - 90.0).abs() < 1e-3, "and followed");
    }

    fn broadcasts(actions: Vec<Action>) -> Vec<Action> {
        actions.into_iter().filter(|a| matches!(a, Action::Broadcast(_))).collect()
    }

    /// Two joined peers talking over a network that takes 100 µs each way,
    /// for one simulated second from `at`, both ticked as the network
    /// layer does: when (in µs after `at`) they first agree on the
    /// timeline, and how many multicasts they sent between them.
    fn converse(
        a: &mut Engine,
        b: &mut Engine,
        mut to_b: Vec<Action>,
        mut to_a: Vec<Action>,
        at: Micros,
    ) -> (Option<Micros>, usize) {
        let (mut agreed, mut sent) = (None, to_a.len() + to_b.len());
        for step in 1..=10_000 {
            let now = at + step * 100;
            let mut from_b = deliver(&to_b, b, 1, now);
            let mut from_a = deliver(&to_a, a, 2, now);
            if step % 200 == 0 {
                from_a.extend(a.tick(now));
                from_b.extend(b.tick(now));
            }
            (to_a, to_b) = (broadcasts(from_b), broadcasts(from_a));
            sent += to_a.len() + to_b.len();
            if agreed.is_none() && a.state().timeline == b.state().timeline {
                agreed = Some(now - at);
            }
        }
        (agreed, sent)
    }

    /// Two members proposing different tempos at the same moment: each
    /// hears the other's change before its own arrives. They must settle
    /// on one — not swap timelines back and forth at network speed, which
    /// was a multicast storm of thousands of announcements a second that
    /// drowned everything else on the network (the tussen hub's RTP audio).
    #[test]
    fn two_tempo_changes_that_cross_settle_on_one() {
        let (mut founder, mut joiner) = joined_pair();
        let at = 3_000_000;
        let to_joiner = founder.propose_tempo(90.0, at, at);
        let to_founder = joiner.propose_tempo(100.0, at, at);
        let (agreed, sent) = converse(&mut founder, &mut joiner, to_joiner, to_founder, at);
        assert_eq!(agreed, Some(100), "agreed on arrival ({sent} multicasts)");
        assert!((founder.state().tempo() - joiner.state().tempo()).abs() < 1e-9);
        // The periodic announcements (4 a second each) plus a few — not
        // one per network round trip.
        assert!(sent <= 12, "{sent} multicasts in one second");
    }

    #[test]
    fn the_same_change_from_both_is_not_echoed() {
        let (mut founder, mut joiner) = joined_pair();
        let at = 3_000_000;
        let to_joiner = founder.propose_tempo(90.0, at, at);
        let to_founder = joiner.propose_tempo(90.0, at, at);
        assert!(broadcasts(deliver(&to_joiner, &mut joiner, 1, at + 100)).is_empty());
        assert!(broadcasts(deliver(&to_founder, &mut founder, 2, at + 100)).is_empty());
        assert_eq!(founder.state().timeline, joiner.state().timeline);
    }

    /// A change proposed for a moment before the session's current line
    /// (an older line) loses everywhere, the proposer included, rather
    /// than leaving it on a timeline of its own.
    #[test]
    fn a_change_older_than_the_sessions_line_gives_way() {
        let (mut founder, mut joiner) = joined_pair();
        founder.propose_tempo(90.0, 3_000_000, 3_000_000);
        let stale = joiner.propose_tempo(100.0, 2_000_000, 3_000_000);
        assert!(deliver(&stale, &mut founder, 2, 3_000_100).is_empty(), "refused");
        let periodic = founder.tick(3_300_000);
        deliver(&periodic, &mut joiner, 1, 3_300_100);
        assert_eq!(joiner.state().timeline, founder.state().timeline);
        assert!((joiner.state().tempo() - 90.0).abs() < 1e-3);
    }

    /// What a member changed is passed on at most once per
    /// [`REANNOUNCE_MIN`]; the latest change goes out with the next.
    #[test]
    fn adopted_changes_are_passed_on_at_most_once_per_reannounce_min() {
        let (mut founder, mut joiner) = joined_pair();
        let at = 3_000_000;
        let mut passed_on = 0;
        for i in 0..10 {
            let t = at + i * 1_000;
            let change = founder.propose_tempo(90.0 + i as f64, t, t);
            passed_on += broadcasts(deliver(&change, &mut joiner, 1, t + 100)).len();
        }
        assert_eq!(passed_on, 1, "the first at once, the rest held");
        assert!(broadcasts(joiner.tick(at + 40_000)).is_empty());
        let next = broadcasts(joiner.tick(at + 1_000 + REANNOUNCE_MIN));
        let [Action::Broadcast(m)] = next.as_slice() else { panic!("{next:?}") };
        assert_eq!(m.timeline(), Some(founder.state().timeline), "the latest");
    }

    #[test]
    fn the_newest_start_stop_wins_in_both_directions() {
        let (mut founder, mut joiner) = joined_pair();
        assert_eq!(joiner.state().is_playing(), None);
        let start = founder.propose_start_stop(true, 2_000_000, 2_000_000);
        deliver(&start, &mut joiner, 1, 2_000_100);
        assert_eq!(joiner.state().is_playing(), Some(true));
        let stop = joiner.propose_start_stop(false, 3_000_000, 3_000_000);
        deliver(&stop, &mut founder, 2, 3_000_100);
        assert_eq!(founder.state().is_playing(), Some(false));
        // An older state arriving late is ignored.
        deliver(&start, &mut founder, 1, 3_000_200);
        assert_eq!(founder.state().is_playing(), Some(false));
    }

    #[test]
    fn a_silent_peer_is_forgotten_and_a_byebye_forgets_at_once() {
        let (mut founder, joiner) = joined_pair();
        founder.tick(1_500_000 + 5_000_001);
        assert_eq!(founder.state().peers, 0, "ttl passed");
        let bye = joiner.byebye().unwrap();
        let msg = Discovery::decode(&bye).unwrap();
        assert_eq!(msg.kind, DiscoveryKind::ByeBye);
        let mut founder2 = found(0).0;
        founder2.on_discovery(
            addr(2),
            &Discovery::decode(&joiner.message(DiscoveryKind::Alive).encode()).unwrap(),
            1,
        );
        assert_eq!(founder2.state().peers, 1);
        founder2.on_discovery(addr(2), &msg, 2);
        assert_eq!(founder2.state().peers, 0);
    }

    #[test]
    fn a_failed_measurement_backs_off_then_retries() {
        let (_, announced) = found(0);
        let mut joiner = Engine::new(JOINER, Config::default(), 0);
        let first = deliver(&announced, &mut joiner, 1, 1_100_000);
        assert!(matches!(first.as_slice(), [Action::Measure { .. }]));
        joiner.on_measured(FOUNDER, None, 1_200_000);
        assert_eq!(joiner.state().status, Status::Searching);
        assert!(joiner.tick(1_300_000).is_empty(), "backing off");
        assert!(matches!(joiner.tick(2_300_000).as_slice(), [Action::Measure { .. }]));
    }

    /// Two founders meet; each measures the other's session and both pick
    /// the same winner — the older session, or within the tie margin the
    /// id that sorts first — so neither waits on the other.
    #[test]
    fn two_lone_founders_converge_on_one_session() {
        // `a` founds at 1 s. (When b founds, its id, who wins): a tie
        // settled by id either way, then a b older and a b younger by more
        // than the margin, whatever the ids say.
        let (aaa, zzz) = (NodeId(*b"aaaaaaaa"), NodeId(*b"zzzzzzzz"));
        for (b_founds, b_id, winner) in [
            (1_200_000, aaa, aaa),
            (1_200_000, zzz, FOUNDER),
            (0, zzz, zzz),
            (1_800_000, aaa, FOUNDER),
        ] {
            let b_id = b_id.0;
            let (mut a, a_says) = found(0);
            let mut b = Engine::new(NodeId(b_id), founding(), b_founds - 1_000_000);
            b.set_endpoint("10.0.0.2:4001".parse().unwrap());
            let b_says = b.tick(b_founds);
            let now = 2_000_000;
            // Both hear each other and both measure — the deadlock was
            // each waiting for the other to move.
            let a_does = deliver(&b_says, &mut a, 2, now);
            let b_does = deliver(&a_says, &mut b, 1, now);
            for does in [&a_does, &b_does] {
                assert!(does.iter().any(|x| matches!(x, Action::Measure { .. })), "{does:?}");
            }
            let a_back = a.on_measured(b.node, measure_burst(&b, b.node, 0, now), now + 1);
            let b_back = b.on_measured(FOUNDER, measure_burst(&a, FOUNDER, 0, now), now + 1);
            deliver(&a_back, &mut b, 1, now + 2);
            deliver(&b_back, &mut a, 2, now + 2);
            let (sa, sb) = (a.state(), b.state());
            assert_eq!(sa.session, sb.session, "one session");
            assert_eq!(sa.session, Some(winner), "{b_founds} {b_id:?}");
            assert_eq!((sa.peers, sb.peers), (1, 1));
        }
    }

    /// A peer on this machine announces itself twice — over loopback with
    /// 127.0.0.1 and over the network with its LAN address. Both are kept,
    /// the loopback one is measured through, and a path that goes quiet is
    /// dropped while the other lives on.
    #[test]
    fn a_peer_heard_on_two_interfaces_is_measured_over_loopback() {
        let (founder, _) = found(0);
        let alive = |ip: [u8; 4]| {
            let m = founder.message(DiscoveryKind::Alive);
            m.with_endpoint_ip(std::net::Ipv4Addr::from(ip))
        };
        let mut joiner = Engine::new(JOINER, Config::default(), 0);
        let lan = SocketAddr::from(([192, 168, 55, 119], 20808));
        let lo = SocketAddr::from(([127, 0, 0, 1], 20808));
        let first = joiner.on_discovery(lan, &alive([192, 168, 55, 119]), 1_000_000);
        let [Action::Measure { endpoint, .. }] = first.as_slice() else { panic!("{first:?}") };
        assert_eq!(*endpoint.ip(), std::net::Ipv4Addr::new(192, 168, 55, 119));
        joiner.on_measured(FOUNDER, None, 1_000_100); // fails; try again later
        joiner.on_discovery(lo, &alive([127, 0, 0, 1]), 1_100_000);
        let peer = &joiner.peers[&FOUNDER];
        assert_eq!(peer.endpoints.len(), 2, "one per path, neither overwriting");
        assert!(peer.endpoint().unwrap().ip().is_loopback(), "loopback preferred");
        // Keep hearing it over the LAN only: the loopback path expires.
        joiner.on_discovery(lan, &alive([192, 168, 55, 119]), 5_500_000);
        joiner.tick(6_200_000);
        let peer = &joiner.peers[&FOUNDER];
        assert_eq!(peer.endpoints.len(), 1);
        assert!(!peer.endpoint().unwrap().ip().is_loopback());
    }

    /// The peer list is a plain copy of who is heard: node, session and
    /// whether it is ours, tempo, the paths it came over (newest first),
    /// and when it was last heard — and it forgets with the engine.
    #[test]
    fn the_peer_list_is_a_snapshot_of_who_is_heard() {
        let (mut founder, mut joiner) = joined_pair();
        let lone = Engine::new(NodeId(*b"stranger"), founding(), 0);
        assert!(lone.peers().is_empty());
        let peers = founder.peers();
        let [p] = peers.as_slice() else { panic!("{peers:?}") };
        assert_eq!((p.node, p.session, p.in_session), (JOINER, FOUNDER, true));
        assert!((p.timeline.bpm() - 120.0).abs() < 1e-3);
        assert_eq!(p.heard, 1_500_000);
        // A peer is listed by the paths it announces an endpoint over: the
        // joiner, given one, is heard over the LAN, then over loopback (it
        // is on this machine) with a tempo change.
        assert!(p.paths.is_empty(), "no endpoint announced yet");
        joiner.set_endpoint("10.0.0.2:4001".parse().unwrap());
        deliver(&joiner.tick(1_900_000), &mut founder, 2, 1_900_000);
        let change = joiner.propose_tempo(90.0, 2_000_000, 2_000_000);
        for m in change.iter().filter_map(|a| match a {
            Action::Broadcast(m) => Some(m.clone().with_endpoint_ip([127, 0, 0, 1].into())),
            _ => None,
        }) {
            founder.on_discovery(SocketAddr::from(([127, 0, 0, 1], 9)), &m, 2_000_000);
        }
        let p = &founder.peers()[0];
        assert!((p.timeline.bpm() - 90.0).abs() < 1e-3);
        assert_eq!(p.heard, 2_000_000);
        let from: Vec<IpAddr> = p.paths.iter().map(|h| h.from).collect();
        assert_eq!(from, [IpAddr::from([127, 0, 0, 1]), addr(2).ip()], "newest first");
        assert!(p.paths[0].endpoint.ip().is_loopback());
        // A peer of another session is listed, and not as ours.
        let (other, says) = {
            let mut o = Engine::new(NodeId(*b"otherone"), founding(), 0);
            o.set_endpoint("10.0.0.3:4002".parse().unwrap());
            let says = o.tick(1_000_000);
            (o.node(), says)
        };
        deliver(&says, &mut founder, 3, 2_100_000);
        let theirs = founder.peers().into_iter().find(|p| p.node == other).unwrap();
        assert!(!theirs.in_session);
        assert_eq!(theirs.session, other);
        founder.tick(2_000_000 + 5_000_001);
        assert!(founder.peers().iter().all(|p| p.node != JOINER), "forgotten");
    }

    /// Founding on demand: a follower that never founds by itself can be
    /// told to, at a tempo and a beat of its choosing, and is then a
    /// founder like any other — joinable, with its ghost clock at 0.
    #[test]
    fn a_follower_founds_on_demand_keeping_its_beat() {
        let mut e = Engine::new(FOUNDER, Config::default(), 0);
        e.set_endpoint("10.0.0.1:4000".parse().unwrap());
        assert!(e.tick(5_000_000).is_empty(), "a follower does not found by itself");
        let said = e.found(97.0, 12.5, 6_000_000);
        assert!(matches!(said.as_slice(), [Action::Broadcast(_)]));
        let s = e.state();
        assert!(s.joined() && s.founder && s.session == Some(FOUNDER));
        assert!((s.tempo() - 97.0).abs() < 1e-3);
        assert!((s.beat_at(6_000_000) - 12.5).abs() < 1e-6, "the beat carries on");
        assert_eq!(e.ghost(6_000_000), Some(0), "a new session: its age is 0");
        assert!(e.found(140.0, 0.0, 7_000_000).is_empty(), "already in one");
        // And it is joined like a session founded by the clock.
        let mut joiner = Engine::new(JOINER, Config::default(), 0);
        let m = deliver(&said, &mut joiner, 1, 6_100_000);
        assert!(matches!(m.as_slice(), [Action::Measure { .. }]));
    }

    /// Leaving: a ByeBye goes out, the peer is silent and searching again
    /// — and the next session it hears it joins as a newcomer, at that
    /// session's tempo, without that session measuring anything.
    #[test]
    fn a_peer_that_leaves_waits_and_then_joins_the_next_session() {
        let (mut founder, mut joiner) = joined_pair();
        let bye = joiner.leave(2_000_000).expect("was in a session");
        assert_eq!(Discovery::decode(&bye).unwrap().kind, DiscoveryKind::ByeBye);
        founder.on_discovery(addr(2), &Discovery::decode(&bye).unwrap(), 2_000_000);
        assert_eq!(founder.state().peers, 0, "the session saw it go");
        assert_eq!(joiner.state().status, Status::Searching);
        assert_eq!(joiner.ghost(2_000_000), None, "no pongs while waiting");
        assert!(joiner.leave(2_000_001).is_none(), "nothing to leave");
        assert!(joiner.byebye().is_none());
        // Waiting: it says nothing, however long — and does not found.
        joiner.peers.clear();
        for t in (2_000_000..12_000_000).step_by(250_000) {
            assert!(joiner.tick(t).iter().all(|a| !matches!(a, Action::Broadcast(_))));
        }
        // A session at 126 BPM appears: it is joined, at 126.
        let mut other = Engine::new(NodeId(*b"session2"), founding(), 11_000_000);
        other.set_endpoint("10.0.0.3:4002".parse().unwrap());
        let says = other.tick(12_000_000);
        other.propose_tempo(126.0, 12_000_000, 12_000_000);
        let says2 = other.tick(12_300_000);
        let m = deliver(&says, &mut joiner, 3, 12_000_000);
        assert!(matches!(m.as_slice(), [Action::Measure { .. }]), "{m:?}");
        deliver(&says2, &mut joiner, 3, 12_300_000);
        let back = joiner.on_measured(
            other.node(),
            measure_burst(&other, other.node(), 0, 12_300_000),
            12_400_000,
        );
        let s = joiner.state();
        assert_eq!(s.session, Some(other.node()));
        assert!((s.tempo() - 126.0).abs() < 1e-3, "{}", s.tempo());
        // The session hears a member, not a rival: nothing to measure.
        let theirs = deliver(&back, &mut other, 2, 12_400_000);
        assert!(theirs.iter().all(|a| !matches!(a, Action::Measure { .. })), "{theirs:?}");
        assert!((other.state().tempo() - 126.0).abs() < 1e-3);
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-5
    }

    /// Is `a` the same beat as `b` in `quantum` — equal phase?
    fn in_phase(a: f64, b: f64, quantum: f64) -> bool {
        let d = (a - b).rem_euclid(quantum);
        d < 1e-5 || quantum - d < 1e-5
    }

    /// Alone, forcing a beat remaps the session: the beat lands at `at`,
    /// the tempo stays, and the new line is announced, superseding the old.
    #[test]
    fn forcing_a_beat_while_alone_moves_the_session() {
        let (mut e, _) = found(0);
        let at = 5_150_000; // session beat 8.3 at 120 BPM (founded at 1 s)
        let old = e.local().state.timeline;
        assert!(close(e.local().beat_at(at), 8.3));
        let said = e.force_beat_at(0.0, at, 4.0, at);
        let [Action::Broadcast(m)] = said.as_slice() else { panic!("{said:?}") };
        let s = e.local();
        assert_eq!(m.timeline(), Some(s.state.timeline), "the new line is what goes out");
        assert!(s.state.timeline.newer_than(&old));
        assert!((s.state.tempo() - 120.0).abs() < 1e-9, "same tempo");
        assert!(close(s.beat_at(at), 0.0), "{}", s.beat_at(at));
        assert!(close(s.phase_at(at, 4.0), 0.0));
        // The session's line moved by -0.3 beat (nearest bar line, 8),
        // and our numbering by the two bars between 8 and 0.
        assert!(close(s.state.beat_at(at), 8.0));
        assert!(close(s.beat_offset, -8.0));
        assert!((s.host_at_beat(0.0) - at).abs() <= 1);
        assert!(close(s.next_boundary(at + 1, 4.0), 4.0));
        // Alone, the polite request is the same thing.
        let (mut r, _) = found(0);
        assert_eq!(r.request_beat_at(0.0, at, 4.0, at).len(), 1);
        assert_eq!(r.local().state.timeline, s.state.timeline);
        assert_eq!(r.local().beat_offset, s.beat_offset);
    }

    /// With a peer, forcing moves the peer's phase too: after one
    /// announcement its beat at `at` is the forced beat, modulo the
    /// quantum, and they agree on the phase from then on. Either peer may
    /// force, and a force that moves the beat back still wins.
    #[test]
    fn forcing_a_beat_with_a_peer_moves_its_phase() {
        for (joiner_forces, q) in [(false, 4.0), (true, 4.0), (false, 3.0), (true, 1.0)] {
            let (mut founder, mut joiner) = joined_pair();
            let at = 3_123_000;
            // A tempo change (to the same tempo) anchored at `at`: every
            // force below lands less than a beat past that anchor (some
            // behind it), so it must re-anchor one beat on to be adopted.
            let anchor = founder.propose_tempo(120.0, at, at - 60_000);
            deliver(&anchor, &mut joiner, 1, at - 59_000);
            let beat = if joiner_forces { 1.0 } else { 6.75 };
            let (forcer, other) = if joiner_forces {
                (&mut joiner, &mut founder)
            } else {
                (&mut founder, &mut joiner)
            };
            let said = forcer.force_beat_at(beat, at, q, at - 50_000);
            assert_eq!(broadcasts(said.clone()).len(), 1);
            deliver(&said, other, 1, at - 49_000);
            let (f, o) = (forcer.local(), other.local());
            assert_eq!(f.state.timeline, o.state.timeline, "adopted");
            assert!(close(f.beat_at(at), beat), "{} {}", f.beat_at(at), beat);
            assert!(in_phase(o.beat_at(at), beat, q), "{} vs {beat} in {q}", o.beat_at(at));
            assert_eq!(o.beat_offset, 0.0, "the other peer's numbering is its own");
            for t in [at, at + 777_000, at + 60_000_000] {
                assert!(close(f.phase_at(t, q), o.phase_at(t, q)), "{t}");
            }
        }
    }

    /// With a peer, the polite request moves nothing shared: it renumbers
    /// our beats by whole quanta, so the beat lands at the next moment the
    /// session's phase is its phase.
    #[test]
    fn requesting_a_beat_with_a_peer_only_renumbers_ours() {
        let (founder, mut joiner) = joined_pair();
        let at = 3_150_000; // session beat 4.3
        let before = joiner.local().state.timeline;
        assert!(joiner.request_beat_at(0.0, at, 4.0, at).is_empty(), "nothing to say");
        let s = joiner.local();
        assert_eq!(s.state.timeline, before, "the session did not move");
        assert!(close(s.beat_offset, -8.0), "two bars: {}", s.beat_offset);
        // Beat 0 lands at the next downbeat, session beat 8, at 5 s.
        assert!((s.host_at_beat(0.0) - 5_000_000).abs() <= 1);
        assert!(close(s.beat_at(5_000_000), 0.0));
        for t in [at, 5_000_000, 9_999_999] {
            assert!(close(s.phase_at(t, 4.0), founder.local().phase_at(t, 4.0)));
            assert!(in_phase(s.beat_at(t), founder.local().beat_at(t), 4.0));
        }
        assert_eq!(founder.local().beat_offset, 0.0);
        // A later announcement leaves the renumbering alone; leaving drops it.
        let alive = [Action::Broadcast(founder.message(DiscoveryKind::Alive))];
        deliver(&alive, &mut joiner, 1, 4_000_000);
        assert!(close(joiner.local().beat_offset, -8.0));
        joiner.leave(5_000_000);
        assert_eq!(joiner.local().beat_offset, 0.0);
    }

    /// Force and tempo changes compose, in either order: a tempo change
    /// after a force keeps the forced beat where it is (the beat at the
    /// change does not jump), and a force after a tempo change lands at
    /// the new tempo. The peer follows both.
    #[test]
    fn forcing_a_beat_across_a_tempo_change() {
        let (mut founder, mut joiner) = joined_pair();
        let at = 3_000_000;
        let forced = founder.force_beat_at(2.5, at, 4.0, at);
        deliver(&forced, &mut joiner, 1, at + 100);
        assert!(close(founder.local().beat_at(at), 2.5), "as forced");
        assert!(close(joiner.local().phase_at(at, 4.0), 2.5));
        let tempo = founder.propose_tempo(90.0, at + 1_000_000, at + 1_000_000);
        deliver(&tempo, &mut joiner, 1, at + 1_000_100);
        let (f, j) = (founder.local(), joiner.local());
        assert!((j.state.tempo() - 90.0).abs() < 1e-3);
        assert!(close(f.beat_at(at + 1_000_000), 4.5), "two beats at 120, then the change");
        assert!(close(f.beat_at(at + 3_000_000), 7.5), "then three at 90");
        assert!(close(j.phase_at(at + 3_000_000, 4.0), 3.5));
        // Now force again, at the new tempo.
        let t2 = at + 5_000_000;
        let forced = joiner.force_beat_at(0.0, t2, 4.0, t2);
        deliver(&forced, &mut founder, 2, t2 + 100);
        let (f, j) = (founder.local(), joiner.local());
        assert!((f.state.tempo() - 90.0).abs() < 1e-3, "the tempo kept");
        assert!(close(j.beat_at(t2), 0.0));
        assert!(close(f.phase_at(t2, 4.0), 0.0));
        assert!(close(f.beat_at(t2 + 2_000_000) - f.beat_at(t2), 3.0), "at 90");
    }

    /// A new tempo and a new phase in one commit: one broadcast, and the
    /// peer, hearing it, goes straight to both — it never holds a line
    /// with one and not the other.
    #[test]
    fn proposing_a_timeline_is_one_broadcast() {
        let (mut founder, mut joiner) = joined_pair();
        let at = 3_400_000;
        let said = founder.propose_timeline(132.0, 1.0, at, 4.0, at - 20_000);
        let [Action::Broadcast(m)] = said.as_slice() else { panic!("{said:?}") };
        assert!((m.timeline().unwrap().bpm() - 132.0).abs() < 1e-3);
        let m = Discovery::decode(&m.encode()).unwrap();
        joiner.on_discovery(addr(1), &m, at - 19_000);
        assert_eq!(
            joiner.local().state.timeline,
            founder.local().state.timeline,
            "one step, to the final line"
        );
        let (f, j) = (founder.local(), joiner.local());
        assert!((j.state.tempo() - 132.0).abs() < 1e-3);
        assert!(close(f.beat_at(at), 1.0));
        assert!(close(j.phase_at(at, 4.0), 1.0));
        assert!(close(j.beat_at(at + 1_000_000) - j.beat_at(at), 2.2), "132 BPM from at");
    }

    /// The forced line crosses the wire byte for byte, and what decodes
    /// is what the forcer holds.
    #[test]
    fn a_forced_timeline_round_trips_the_wire() {
        let (mut e, _) = found(0);
        let said = e.propose_timeline(87.5, -3.25, 2_345_678, 3.0, 2_000_000);
        let [Action::Broadcast(m)] = said.as_slice() else { panic!("{said:?}") };
        let bytes = m.encode();
        let back = Discovery::decode(&bytes).unwrap();
        assert_eq!(&back, m);
        assert_eq!(back.encode(), bytes);
        assert_eq!(back.timeline(), Some(e.local().state.timeline));
    }

    /// Forcing with no grid (quantum 0) has no phase to move: only our
    /// numbering changes, and nothing is sent.
    #[test]
    fn forcing_with_no_quantum_only_renumbers() {
        let (mut e, _) = found(0);
        let old = e.local().state.timeline;
        assert!(e.force_beat_at(100.0, 3_000_000, 0.0, 3_000_000).is_empty());
        assert_eq!(e.local().state.timeline, old);
        assert!(close(e.local().beat_at(3_000_000), 100.0));
    }

    #[test]
    fn nothing_is_proposed_before_joining() {
        let mut e = Engine::new(JOINER, Config::default(), 0);
        assert!(e.propose_tempo(100.0, 0, 0).is_empty());
        assert!(e.propose_start_stop(true, 0, 0).is_empty());
        assert!(e.request_beat_at(1.0, 0, 4.0, 0).is_empty());
        assert!(e.force_beat_at(1.0, 0, 4.0, 0).is_empty());
        assert!(e.propose_timeline(100.0, 1.0, 0, 4.0, 0).is_empty());
        assert_eq!(e.beat_offset(), 0.0);
        assert_eq!(e.ghost(0), None, "no ghost time to answer a ping with");
    }
}
