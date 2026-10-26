//! The 1.x surface consumers build by hand still builds: tussen and flux
//! write `SessionState { .. }` literals with exactly 1.0.1's fields, so the
//! struct must keep exactly those, and its answers must keep meaning the
//! session's beats whatever this peer's own numbering.

use simply_link::session::Status;
use simply_link::timeline::{GhostXForm, StartStop, Timeline};
use simply_link::{LocalBeats, SessionState};

/// Every 1.0.1 field, and no other — a new field would fail to compile here.
fn v1_0_1(bpm: f64) -> SessionState {
    SessionState {
        status: Status::Joined,
        session: None,
        peers: 0,
        founder: true,
        timeline: Timeline::at(bpm, 0.0, 0),
        xform: GhostXForm::default(),
        start_stop: Some(StartStop { playing: true, beats: 0, timestamp: 0 }),
    }
}

#[test]
fn a_1_0_1_session_state_literal_still_builds_and_means_the_same() {
    let s = v1_0_1(120.0);
    assert_eq!(s.beat_at(1_000_000), 2.0);
    assert_eq!(s.host_at_beat(2.0), 1_000_000);
    assert_eq!(s.phase_at(2_500_000, 4.0), 1.0);
    assert_eq!(s.next_boundary(2_500_000, 4.0), 8.0);
    // Our own numbering is a separate value beside it.
    let local = LocalBeats { state: s, beat_offset: -4.0 };
    assert_eq!(local.beat_at(2_500_000), 1.0);
    assert_eq!(local.phase_at(2_500_000, 4.0), 1.0, "phase is the session's");
    assert_eq!(local.host_at_beat(1.0), 2_500_000);
    assert_eq!(local.next_boundary(2_500_000, 4.0), 4.0);
    assert_eq!(s.beat_at(2_500_000), 5.0, "the session's beat is unmoved");
}
