//! Two of this crate's peers, on real sockets, finding each other: one
//! founds a session, the other joins it through a real measurement burst,
//! and a tempo change on one reaches the other.
//!
//! **Off the real Link network by construction**: an administratively
//! scoped group (239.255.76.78, not Link's 224.76.78.75) on a port of its
//! own, with a multicast time-to-live of 0, so a datagram sent even on a
//! real interface is delivered on this machine and never transmitted. No
//! Link app can hear these sessions and nothing here reaches another
//! machine.

use std::net::{Ipv4Addr, UdpSocket};
use std::thread;
use std::time::{Duration, Instant};

use simply_link::{Config, Link, Network};

fn private_network() -> Network {
    // A port nothing else is using: ask the OS for one, then let it go.
    let port = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap().local_addr().unwrap().port();
    Network {
        group: Ipv4Addr::new(239, 255, 76, 78),
        port,
        interface: Ipv4Addr::LOCALHOST,
        multicast_ttl: 0,
    }
}

fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(10);
    while !ok() {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_follower_joins_a_founded_session_and_follows_its_tempo() {
    let net = private_network();
    let founder =
        Link::join_on(Config { found_after: Some(200_000), bpm: 128.0, ..Config::default() }, net)
            .expect("founder");
    wait_for("the founder to found", || founder.state().joined());
    let follower = Link::join_on(Config::default(), net).expect("follower");
    wait_for("the follower to join", || follower.state().joined());
    let f = follower.state();
    assert_eq!(f.session, founder.state().session);
    assert!((f.tempo() - 128.0).abs() < 1e-3);
    wait_for("each to see the other", || founder.state().peers == 1 && follower.state().peers == 1);

    // Both agree on the beat at the same instant, to well within a
    // millisecond's worth of beat (loopback round trips are microseconds).
    let at = Instant::now();
    let a = founder.state().beat_at(founder.clock().micros(at));
    let b = follower.state().beat_at(follower.clock().micros(at));
    assert!((a - b).abs() < 128.0 / 60.0 * 0.001, "{a} vs {b}");

    founder.propose_tempo(97.0, founder.clock().now());
    wait_for("the tempo change to arrive", || (follower.state().tempo() - 97.0).abs() < 1e-3);

    // Leaving says goodbye: the founder forgets the follower at once.
    drop(follower);
    wait_for("the goodbye", || founder.state().peers == 0);
}

/// The defect this crate shipped with: a Link app on this machine announces
/// over **loopback**, and a peer joined on the default route's interface
/// alone never hears it (measured against a DJ app: 16 announcements in 4 s
/// on loopback, none on Wi-Fi). Here the other app is one of ours, bound to
/// loopback and nothing else; the peer under test is on every interface,
/// as `Link::join` puts it. It must hear the loopback founder, measure it
/// through the loopback endpoint that founder announced, join — and be
/// heard back, which needs its own loopback copy carrying 127.0.0.1.
#[test]
fn a_peer_on_every_interface_joins_an_app_that_is_only_on_loopback() {
    let loopback_only = private_network();
    let everywhere = Network { interface: Ipv4Addr::UNSPECIFIED, ..loopback_only };
    let app = Link::join_on(
        Config { found_after: Some(200_000), bpm: 123.0, ..Config::default() },
        loopback_only,
    )
    .expect("the loopback-only app");
    wait_for("the app to found", || app.state().joined());
    let ours = Link::join_on(Config::default(), everywhere).expect("our peer");
    wait_for("our peer to join the app's session", || ours.state().joined());
    assert_eq!(ours.state().session, app.state().session);
    assert!((ours.state().tempo() - 123.0).abs() < 1e-3);
    wait_for("the app to hear us back", || app.state().peers == 1);
    let at = Instant::now();
    let a = app.state().beat_at(app.clock().micros(at));
    let b = ours.state().beat_at(ours.clock().micros(at));
    assert!((a - b).abs() < 123.0 / 60.0 * 0.001, "{a} vs {b}");
}

/// What an Ableton peer does to measure a session through one of ours:
/// hear its announcement, ping the `mep4` it gave, and count only pongs
/// that name the session it announced. Ours left the session out of its
/// pongs, so no Ableton peer (djay, Live, a TouchDesigner Link CHOP) could
/// ever join a session through it — the defect behind a DJ switching Link
/// on after the hub and the two never meeting.
#[test]
fn a_founded_session_can_be_measured_the_way_an_ableton_peer_measures_it() {
    let net = private_network();
    let founder =
        Link::join_on(Config { found_after: Some(200_000), ..Config::default() }, net).unwrap();
    wait_for("the founder to found", || founder.state().joined());
    let session = founder.state().session.unwrap();
    let (listener, _) = simply_link::discovery_listener(&net).unwrap();
    let mut buf = [0u8; 512];
    let until = Instant::now() + Duration::from_secs(5);
    let endpoint = loop {
        assert!(Instant::now() < until, "no announcement heard");
        let Ok((len, _)) = listener.recv_from(&mut buf) else { continue };
        let msg = simply_link::wire::Discovery::decode(&buf[..len]).unwrap();
        if msg.node == founder.node() {
            assert_eq!(msg.session(), Some(session));
            break msg.endpoint().expect("a measurement endpoint");
        }
    };
    let clock = simply_link::HostClock::new();
    let pinger = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    assert!(simply_link::burst(&pinger, endpoint, session, &clock).is_some(), "measured");
    let elsewhere = simply_link::NodeId(*b"nobody!!");
    assert_eq!(
        simply_link::burst(&pinger, endpoint, elsewhere, &clock),
        None,
        "for its session only"
    );
}

/// The hub's case: a peer that would found on its own (`found_after`)
/// starts after a session exists, joins it at that session's tempo — and
/// keeps following every change after.
#[test]
fn a_late_founding_peer_joins_and_follows_every_tempo_change() {
    let net = private_network();
    let first =
        Link::join_on(Config { found_after: Some(200_000), bpm: 98.0, ..Config::default() }, net)
            .unwrap();
    wait_for("the first to found", || first.state().joined());
    thread::sleep(Duration::from_millis(600));
    let hub = Link::join_on(
        Config { found_after: Some(1_000_000), bpm: 120.0, ..Config::default() },
        net,
    )
    .unwrap();
    wait_for("the hub to join", || hub.state().joined() && hub.state().peers == 1);
    assert_eq!(hub.state().session, first.state().session);
    assert!(!hub.state().founder, "joined, not founded");
    assert!((hub.state().tempo() - 98.0).abs() < 1e-3);
    for bpm in [101.5, 87.0, 140.0] {
        first.propose_tempo(bpm, first.clock().now());
        wait_for("the change to reach the hub", || (hub.state().tempo() - bpm).abs() < 1e-3);
    }
}

/// A Link app alone at 126 BPM (an Ableton peer founds the moment Link is
/// on — here, after 200 ms of hearing nobody), and a peer that only ever
/// follows: started after the app, or waiting before it. Either way the
/// follower ends in the app's session at 126, and the app's tempo never
/// moves. Then the follower leaves and waits again — and, the app still
/// being there, joins it again, still at the app's tempo.
#[test]
fn a_follower_joins_a_lone_app_at_its_tempo_whether_it_came_first_or_not() {
    for follower_first in [false, true] {
        let net = private_network();
        let follow = Config { bpm: 120.0, ..Config::default() };
        let app_config = Config { found_after: Some(200_000), bpm: 126.0, ..Config::default() };
        let (app, hub);
        if follower_first {
            hub = Link::join_on(follow, net).expect("hub");
            thread::sleep(Duration::from_millis(600));
            assert!(!hub.state().joined(), "waiting: no session of its own");
            app = Link::join_on(app_config, net).expect("app");
        } else {
            app = Link::join_on(app_config, net).expect("app");
            wait_for("the app to found", || app.state().joined());
            hub = Link::join_on(follow, net).expect("hub");
        }
        wait_for("the hub to join", || hub.state().joined());
        let app_session = app.state().session;
        assert_eq!(hub.state().session, app_session, "follower_first {follower_first}");
        assert!((hub.state().tempo() - 126.0).abs() < 1e-3);
        wait_for("each to see the other", || app.state().peers == 1 && hub.state().peers == 1);
        assert_eq!(app.state().session, app_session, "the app never moved");
        assert!((app.state().tempo() - 126.0).abs() < 1e-3, "nor did its tempo");
        let peers = hub.peers();
        assert_eq!(peers.len(), 1);
        assert!(peers[0].in_session && (peers[0].timeline.bpm() - 126.0).abs() < 1e-3);

        // Leaving, the hub waits — and as the app is still announcing, it
        // joins it again at once (the goodbye itself is a unit test: here
        // the rejoin outruns any look at the app's peer count).
        hub.leave();
        wait_for("the hub to rejoin", || hub.state().joined());
        assert_eq!(hub.state().session, app_session);
        assert!((app.state().tempo() - 126.0).abs() < 1e-3);
    }
}
