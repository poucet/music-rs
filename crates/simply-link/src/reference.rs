//! A model of how Ableton's own Link peers behave — djay, Live, a
//! TouchDesigner Link CHOP — just far enough to put this crate's
//! [`Engine`] in a room with one, with no sockets.
//!
//! What the model does — Ableton Link's behaviour as its public source
//! (`Sessions.hpp`, `Measurement.hpp`, `PingResponder.hpp`,
//! `Controller.hpp`) describes it; the model is written from that
//! description, nothing copied:
//!
//! * **Enabling Link starts a fresh session**: the peer's own node id, its
//!   ghost clock reset to 0 ("to avoid hijacking tempo in existing
//!   sessions" — the newcomer is the youngest session in the room).
//! * **Every session it has not seen before is measured**, through the
//!   founder if it is reachable, else any member — whatever session this
//!   peer is in, founder or not, alone or not.
//! * **A pong only counts if it names the session being measured** (its
//!   `sess` entry); anything else fails the measurement.
//! * **The older session wins**: after measuring, switch if the other
//!   session's ghost time is ahead of ours by more than 0.5 s, or within
//!   0.5 s and its id sorts first.
//! * **A failed measurement forgets that session**, so it is measured again
//!   the next time it is heard.
//! * **Within a session the timeline with the greater beat origin wins**,
//!   and a tempo change always moves the beat origin forward.

use std::collections::BTreeMap;
use std::net::{SocketAddr, SocketAddrV4};

use crate::measure;
use crate::session::{Action, Engine};
use crate::timeline::{MICROBEATS, Micros, Timeline};
use crate::wire::{
    Discovery, DiscoveryKind, Entry, Measurement, MeasurementKind, NodeId, SessionId,
};

/// Session ghost times closer than this are a tie, settled by id.
const SESSION_EPS: Micros = 500_000;

/// What a Link responder at `responder` answers a ping with — the path the
/// network layer's responder thread takes.
pub fn answer(responder: &Engine, ping: &[u8], host: Micros) -> Option<Vec<u8>> {
    responder.pong(ping, host)
}

/// An Ableton Link peer, as far as the session rules go.
pub struct RealLink {
    pub node: NodeId,
    pub session: SessionId,
    /// ghost = host + intercept.
    intercept: Micros,
    /// This peer's host clock reads `skew` more than the engine's.
    skew: Micros,
    pub timeline: Timeline,
    /// Sessions measured and lost to (or not yet decided): their timelines.
    others: BTreeMap<SessionId, Timeline>,
    pub endpoint: SocketAddrV4,
    /// Measurements attempted, and how many failed — for the assertions.
    pub measured: usize,
    pub failed: usize,
}

impl RealLink {
    /// Link switched on at engine host time `now`, at `bpm`.
    pub fn enable(node: NodeId, bpm: f64, now: Micros, skew: Micros) -> Self {
        RealLink {
            node,
            session: node,
            intercept: -(now + skew),
            skew,
            timeline: Timeline::at(bpm, 0.0, 0),
            others: BTreeMap::new(),
            endpoint: "10.0.0.9:5000".parse().unwrap(),
            measured: 0,
            failed: 0,
        }
    }

    fn ghost(&self, now: Micros) -> Micros {
        now + self.skew + self.intercept
    }

    pub fn alive(&self) -> Discovery {
        Discovery {
            kind: DiscoveryKind::Alive,
            ttl: 5,
            group: 0,
            node: self.node,
            entries: vec![
                Entry::Timeline(self.timeline),
                Entry::Session(self.session),
                Entry::Endpoint4(self.endpoint),
            ],
        }
    }

    /// The pong a Link peer sends: its session, its ghost time now, then
    /// the ping's own payload.
    pub fn pong(&self, ping: &[u8], now: Micros) -> Option<Vec<u8>> {
        let ping = Measurement::decode(ping).ok()?;
        (ping.kind == MeasurementKind::Ping).then_some(())?;
        let mut entries = vec![Entry::Session(self.session), Entry::GhostTime(self.ghost(now))];
        entries.extend(ping.entries);
        Some(Measurement { kind: MeasurementKind::Pong, entries }.encode())
    }

    /// The tempo slider moved at engine host time `now`.
    pub fn set_tempo(&mut self, bpm: f64, now: Micros) {
        let ghost = self.ghost(now);
        let beat = self.timeline.beat_at(ghost);
        let origin = ((beat * MICROBEATS).round() as i64).max(self.timeline.beat_origin + 1);
        let mut t = Timeline::at(bpm, origin as f64 / MICROBEATS, 0);
        t.time_origin = self.timeline.ghost_at(origin as f64 / MICROBEATS);
        self.timeline = t;
    }

    /// Hear one discovery message from the engine `peer`, measuring it
    /// through its responder if this is a session not seen before.
    pub fn hear(&mut self, msg: &Discovery, peer: &Engine, now: Micros) {
        if msg.kind == DiscoveryKind::ByeBye || msg.node == self.node {
            return;
        }
        let (Some(session), Some(timeline)) = (msg.session(), msg.timeline()) else { return };
        if session == self.session {
            if timeline.beat_origin > self.timeline.beat_origin {
                self.timeline = timeline;
            }
            return;
        }
        if let Some(known) = self.others.get_mut(&session) {
            if timeline.beat_origin > known.beat_origin {
                *known = timeline;
            }
            return;
        }
        self.others.insert(session, timeline);
        self.measured += 1;
        match self.measure(session, peer, now) {
            None => {
                self.failed += 1;
                self.others.remove(&session);
            }
            Some(intercept) => {
                let diff = (now + self.skew + intercept) - self.ghost(now);
                if diff > SESSION_EPS || (diff.abs() < SESSION_EPS && session < self.session) {
                    let theirs = self.others.remove(&session).expect("inserted above");
                    self.others.insert(self.session, self.timeline);
                    self.session = session;
                    self.intercept = intercept;
                    self.timeline = theirs;
                }
            }
        }
    }

    /// Ping `peer`'s responder until enough pongs naming `session` came
    /// back; any pong naming another session (or none) fails it.
    fn measure(&self, session: SessionId, peer: &Engine, now: Micros) -> Option<Micros> {
        let mut samples = Vec::new();
        for i in 0..measure::BURST as Micros {
            let host = now + self.skew + i * 1_000;
            let reply = answer(peer, &measure::ping(host), now + i * 1_000 + 200)?;
            let m = Measurement::decode(&reply).ok()?;
            let named = m.entries.iter().find_map(|e| match e {
                Entry::Session(s) => Some(*s),
                _ => None,
            });
            if named != Some(session) {
                return None;
            }
            samples.push(m.ghost_time()? - (host + host + 400) / 2);
        }
        measure::settle(&mut samples)
    }
}

/// A room with one of our engines (`hub`) and one Ableton peer, run in
/// steps of `step` µs of engine host time.
pub struct Room {
    pub hub: Engine,
    pub dj: Option<RealLink>,
    pub now: Micros,
    /// Every message the hub sent, with the dj's tempo just after hearing
    /// it — for asserting on what the hub ever said.
    pub said: Vec<(Discovery, Option<f64>)>,
}

impl Room {
    pub fn new(hub: Engine, now: Micros) -> Self {
        Room { hub, dj: None, now, said: Vec::new() }
    }

    /// Run for `for_us`: the hub ticks every 20 ms, both sides hear each
    /// other's announcements, and whatever the hub asks to measure is
    /// measured against the dj's responder.
    pub fn run(&mut self, for_us: Micros) {
        let until = self.now + for_us;
        let mut next_alive = self.now;
        while self.now < until {
            let actions = self.hub.tick(self.now);
            self.perform(actions);
            if self.now >= next_alive {
                next_alive += 250_000;
                if let Some(dj) = &self.dj {
                    let msg = Discovery::decode(&dj.alive().encode()).unwrap();
                    let from = SocketAddr::from((*dj.endpoint.ip(), dj.endpoint.port()));
                    let actions = self.hub.on_discovery(from, &msg, self.now);
                    self.perform(actions);
                }
            }
            self.now += 20_000;
        }
    }

    fn perform(&mut self, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::Broadcast(m) | Action::Send(_, m) => {
                    let m = Discovery::decode(&m.encode()).unwrap();
                    if let Some(dj) = self.dj.as_mut() {
                        dj.hear(&m, &self.hub, self.now);
                    }
                    let dj_bpm = self.dj.as_ref().map(|dj| dj.timeline.bpm());
                    self.said.push((m, dj_bpm));
                }
                Action::Measure { session, .. } => {
                    let intercept = self.dj.as_ref().and_then(|dj| {
                        let mut samples = Vec::new();
                        for i in 0..measure::BURST as Micros {
                            let sent = self.now + i * 1_000;
                            let reply = dj.pong(&measure::ping(sent), sent + 200)?;
                            samples.extend(measure::sample(&reply, session, sent, sent + 400));
                        }
                        measure::settle(&mut samples)
                    });
                    let actions = self.hub.on_measured(session, intercept, self.now);
                    self.perform(actions);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{Config, Status};

    /// Sorts before every other id here, so an id-ordered rule would have
    /// the hub never yield.
    const HUB: NodeId = NodeId(*b"AAAAhub!");
    const DJ: NodeId = NodeId(*b"zzzzdjay");

    fn hub(node: NodeId, now: Micros) -> Engine {
        let mut e =
            Engine::new(node, Config { found_after: Some(1_000_000), ..Config::default() }, now);
        e.set_endpoint("10.0.0.1:4000".parse().unwrap());
        e
    }

    fn in_one_session(room: &Room) -> bool {
        let dj = room.dj.as_ref().unwrap();
        room.hub.state().session == Some(dj.session) && room.hub.state().peers == 1
    }

    /// Bug 1, as Chris met it: the hub is up and has founded; a minute
    /// later the DJ switches Link on. Link's own rule makes the hub's
    /// (older) session the one both end up in, at the hub's tempo — and
    /// from then on the DJ's tempo changes reach the hub.
    #[test]
    fn a_dj_switching_link_on_after_the_hub_founded_ends_up_in_one_session() {
        for (hub_id, dj_id) in [(HUB, DJ), (DJ, HUB)] {
            let mut room = Room::new(hub(hub_id, 0), 0);
            room.run(2_000_000);
            assert!(room.hub.state().founder, "alone for a second: founded");
            room.run(58_000_000);
            room.dj = Some(RealLink::enable(dj_id, 98.0, room.now, 3_000_000));
            room.run(3_000_000);
            assert!(in_one_session(&room), "{hub_id} / {dj_id}: never met");
            let dj = room.dj.as_ref().unwrap();
            assert_eq!(dj.session, hub_id, "the older session — the hub's — wins");
            assert!((dj.timeline.bpm() - 120.0).abs() < 1e-3, "the newcomer adopts its tempo");

            room.dj.as_mut().unwrap().set_tempo(127.0, room.now);
            room.run(500_000);
            assert!((room.hub.state().tempo() - 127.0).abs() < 1e-3, "and the hub follows");
        }
    }

    /// Bug 2's setup: the DJ has been in Link a while; the hub starts.
    /// The hub joins the DJ's session at the DJ's tempo and follows every
    /// tempo change after.
    #[test]
    fn a_hub_started_after_the_dj_follows_its_tempo() {
        for (hub_id, dj_id) in [(HUB, DJ), (DJ, HUB)] {
            let mut room = Room::new(hub(hub_id, 10_000_000), 10_000_000);
            room.dj = Some(RealLink::enable(dj_id, 98.0, 0, -4_000_000));
            room.run(3_000_000);
            assert!(in_one_session(&room), "{hub_id} / {dj_id}: never met");
            assert_eq!(room.hub.state().session, Some(dj_id));
            assert!(!room.hub.state().founder);
            assert!((room.hub.state().tempo() - 98.0).abs() < 1e-3);
            for bpm in [101.5, 96.0, 140.0] {
                room.dj.as_mut().unwrap().set_tempo(bpm, room.now);
                room.run(500_000);
                assert!((room.hub.state().tempo() - bpm).abs() < 1e-3, "{bpm}");
            }
            assert_eq!(room.hub.state().status, Status::Joined);
        }
    }

    /// The fix for bug 1, from the hub's side: a hub that only ever
    /// follows (no `found_after`) is silent while it waits, however long,
    /// so a DJ switching Link on after it is alone in the room, keeps its
    /// own session and tempo, and the hub joins it as the newcomer — the
    /// DJ measures nothing and changes nothing. Told to found (the
    /// console's Start session), the hub's session is the room's, as any
    /// running session is to a newcomer.
    #[test]
    fn a_waiting_hub_joins_a_dj_who_switches_link_on_later_at_the_djs_tempo() {
        for (hub_id, dj_id) in [(HUB, DJ), (DJ, HUB)] {
            let mut e = Engine::new(hub_id, Config::default(), 0);
            e.set_endpoint("10.0.0.1:4000".parse().unwrap());
            let mut room = Room::new(e, 0);
            room.run(60_000_000);
            assert_eq!(room.hub.state().status, Status::Searching, "waiting, not founding");
            room.dj = Some(RealLink::enable(dj_id, 126.0, room.now, 3_000_000));
            room.run(3_000_000);
            assert!(in_one_session(&room), "{hub_id} / {dj_id}: never met");
            let dj = room.dj.as_ref().unwrap();
            assert_eq!(dj.session, dj_id, "the DJ's session holds");
            assert!((dj.timeline.bpm() - 126.0).abs() < 1e-3, "the DJ's tempo never changes");
            assert_eq!(dj.measured, 0, "the DJ never even had a rival to measure");
            assert!((room.hub.state().tempo() - 126.0).abs() < 1e-3, "the hub runs at 126");
            assert!(!room.hub.state().founder);
        }
        // Started on demand: the hub's session, and its tempo, are the room's.
        let mut e = Engine::new(HUB, Config::default(), 0);
        e.set_endpoint("10.0.0.1:4000".parse().unwrap());
        let mut room = Room::new(e, 0);
        room.run(2_000_000);
        let started = room.hub.found(110.0, 0.0, room.now);
        assert_eq!(started.len(), 1);
        room.run(10_000_000);
        room.dj = Some(RealLink::enable(DJ, 126.0, room.now, 0));
        room.run(3_000_000);
        assert!(in_one_session(&room));
        assert_eq!(room.dj.as_ref().unwrap().session, HUB);
        assert!((room.dj.as_ref().unwrap().timeline.bpm() - 110.0).abs() < 1e-3);
    }

    /// LINK-ADOPT, word for word what Chris did: the hub waits a minute
    /// with a private 120; djay, mid-track at 126, switches Link on — a
    /// fresh session whose beat origin carries on the track's beat, as
    /// Link's reset on enable does. Everything the hub ever says carries
    /// the DJ's session and the DJ's timeline exactly (so Link's
    /// newer-beat-origin rule has nothing to adopt), and the DJ is at 126
    /// after every message it hears.
    #[test]
    fn a_joining_hub_only_ever_announces_the_timeline_it_adopted() {
        for (hub_id, dj_id) in [(HUB, DJ), (DJ, HUB)] {
            let mut e = Engine::new(hub_id, Config::default(), 0);
            e.set_endpoint("10.0.0.1:4000".parse().unwrap());
            let mut room = Room::new(e, 0);
            room.run(60_000_000);
            assert!(room.said.is_empty(), "waiting: silent");
            let mut dj = RealLink::enable(dj_id, 126.0, room.now, 3_000_000);
            dj.timeline = Timeline::at(126.0, 1234.567, 0);
            let theirs = dj.timeline;
            room.dj = Some(dj);
            room.run(3_000_000);
            assert!(in_one_session(&room), "{hub_id} / {dj_id}: never met");
            assert!(!room.said.is_empty());
            for (m, dj_bpm) in &room.said {
                assert_eq!(m.session(), Some(dj_id), "{m:?}");
                assert_eq!(m.timeline(), Some(theirs), "only the adopted timeline");
                assert_eq!(*dj_bpm, Some(theirs.bpm()));
            }
            assert_eq!(room.dj.as_ref().unwrap().timeline, theirs, "the DJ never changes");
            assert_eq!(room.hub.state().timeline, theirs);
        }
    }

    /// The race behind the deadlock: the hub and the DJ each measure the
    /// other's session at the same moment. Whatever the order, both must
    /// end in the same one.
    #[test]
    fn two_sessions_measuring_each_other_at_once_still_converge() {
        for skew in [-2_000_000, 0, 2_000_000] {
            // A DJ whose session is within the 0.5 s tie of the hub's, so
            // the id decides — both orders.
            for (hub_id, dj_id, ago) in [(HUB, DJ, 1_300_000), (DJ, HUB, 1_700_000)] {
                let mut room = Room::new(hub(hub_id, 0), 0);
                room.run(1_500_000);
                room.dj = Some(RealLink::enable(dj_id, 98.0, room.now - ago, skew));
                room.run(5_000_000);
                assert!(in_one_session(&room), "skew {skew}, {hub_id} / {dj_id}");
            }
        }
    }
}
