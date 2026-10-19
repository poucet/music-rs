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
//! 3. **Follow.** In the session, a timeline *change* announced by any
//!    member is adopted — a change relative to what that member announced
//!    before, so an old announcement still in flight cannot undo a newer
//!    change of ours. The start/stop state with the latest timestamp wins.
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

use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, SocketAddr, SocketAddrV4};

use crate::timeline::{GhostXForm, MICROBEATS, Micros, StartStop, Timeline, next_boundary, phase};
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
    endpoint: Option<SocketAddrV4>,
    peers: BTreeMap<NodeId, Peer>,
    /// Foreign sessions measured and found not to win over ours (rule 5),
    /// kept until no peer is in them.
    outranked: BTreeSet<SessionId>,
    searching_since: Micros,
    last_announce: Option<Micros>,
    measuring: Option<(SessionId, Micros)>,
    measured_at: Micros,
    no_retry_before: Micros,
}

/// A measurement that has not answered in this long is given up on.
const MEASURE_TIMEOUT: Micros = 5_000_000;
/// Two sessions whose ghost times are closer than this are a tie, settled
/// by id (rule 5) — Ableton's own margin.
pub const SESSION_EPS: Micros = 500_000;

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
            endpoint: None,
            peers: BTreeMap::new(),
            outranked: BTreeSet::new(),
            searching_since: now,
            last_announce: None,
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
            let changed = previous.is_none_or(|p| p.session != session || p.timeline != timeline);
            let mut announce = false;
            if changed && timeline != self.timeline {
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
                out.push(self.announce(now));
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
                if self.last_announce.is_none_or(|t| now - t >= self.config.announce_every) {
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

    #[test]
    fn nothing_is_proposed_before_joining() {
        let mut e = Engine::new(JOINER, Config::default(), 0);
        assert!(e.propose_tempo(100.0, 0, 0).is_empty());
        assert!(e.propose_start_stop(true, 0, 0).is_empty());
        assert_eq!(e.ghost(0), None, "no ghost time to answer a ping with");
    }
}
