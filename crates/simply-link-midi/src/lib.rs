//! An Ableton Link session out as MIDI beat clock — the logic of the
//! `link-midi` bridge, with no sockets and no MIDI port in it, so it can be
//! run against a fake session and a fake sink.
//!
//! What a follower is sent, and when:
//!
//! * nothing, until the bridge has joined a session and (in
//!   [`Transport::Sync`]) the session's shared transport is playing;
//! * then, on the **next quantum boundary** of the session's beat grid, `F2`
//!   Song Position 0 and `FA` Start — so the follower's bar one is the
//!   session's downbeat, in phase with every Link peer using that quantum;
//! * `F8` Timing Clock 24 times a beat, each at the host instant the
//!   session's timeline puts that tick at — recomputed from the latest
//!   timeline every tick, so a tempo change reaches the next tick;
//! * `FC` Stop when the transport stops (or the session is lost), and a
//!   re-armed Start at the following boundary when it plays again.
//!
//! Every instant is moved earlier by the configured **latency**, the time
//! the follower takes to act on a message: a device that sounds 5 ms after
//! a clock arrives is sent it 5 ms early, so its sound lands on the beat.
//!
//! A timeline *jump* — the session re-anchored, a peer rejoined elsewhere —
//! cannot be followed tick by tick without the follower's bar drifting,
//! and a pointer is ignored while running; so it is a Stop and a re-armed
//! Start on the next boundary, which keeps the bars honest.

#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use simply_link::timeline::next_boundary;
use simply_link::{HostClock, Micros, SessionState};
pub use simply_midi_clock::ClockEvent;
use simply_midi_clock::{IDLE_POLL, POLL, TICKS_PER_BEAT, pace, spin_until};

/// Whether the clock follows the session's shared transport.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Transport {
    /// Start and stop with the session's start/stop sync. A session where
    /// nobody uses start/stop sync never starts the follower.
    Sync,
    /// Run whenever joined, whatever the session's transport says.
    Free,
}

/// How the bridge clocks its follower.
#[derive(Copy, Clone, Debug)]
pub struct Settings {
    /// Beats per quantum — the bar the follower's Start is aligned to.
    pub quantum: f64,
    /// µs the follower takes to act on a message; everything is sent that
    /// much early.
    pub latency: Micros,
    pub transport: Transport,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { quantum: 4.0, latency: 0, transport: Transport::Sync }
    }
}

/// What the schedule asks of a session: the answers [`SessionState`] gives.
pub trait Grid {
    fn joined(&self) -> bool;
    fn playing(&self) -> Option<bool>;
    fn tempo(&self) -> f64;
    fn beat_at(&self, host: Micros) -> f64;
    fn host_at_beat(&self, beat: f64) -> Micros;
}

impl Grid for SessionState {
    fn joined(&self) -> bool {
        SessionState::joined(self)
    }
    fn playing(&self) -> Option<bool> {
        self.is_playing()
    }
    fn tempo(&self) -> f64 {
        SessionState::tempo(self)
    }
    fn beat_at(&self, host: Micros) -> f64 {
        SessionState::beat_at(self, host)
    }
    fn host_at_beat(&self, beat: f64) -> Micros {
        SessionState::host_at_beat(self, beat)
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum Phase {
    /// Nothing running, nothing armed.
    Idle,
    /// Waiting for the boundary at beat `start`; `told` once the pointer
    /// has gone out.
    Armed { start: f64, told: bool },
    /// Ticking: tick `next` of the run that began at beat `origin`.
    Running { origin: f64, next: i64 },
}

/// The bridge's scheduler. Ask [`Schedule::next`] what is due and when;
/// once it is sent, say so with [`Schedule::sent`]. Asking twice without
/// sending gives the same answer (recomputed from the latest session), so a
/// sender may ask as often as it likes.
#[derive(Debug)]
pub struct Schedule {
    settings: Settings,
    phase: Phase,
}

impl Schedule {
    pub fn new(settings: Settings) -> Self {
        Schedule { settings, phase: Phase::Idle }
    }

    /// Is the follower running (a Start or a tick has gone out)?
    pub fn running(&self) -> bool {
        matches!(self.phase, Phase::Running { .. })
    }

    /// The next message due, and the host instant it is due at, given the
    /// session as it is now. `None` — nothing to send yet.
    pub fn next(&mut self, grid: &impl Grid, now: Micros) -> Option<(ClockEvent, Micros)> {
        let wanted = grid.joined()
            && match self.settings.transport {
                Transport::Free => true,
                Transport::Sync => grid.playing() == Some(true),
            };
        let early = self.settings.latency;
        if self.phase == Phase::Idle && wanted {
            // The boundary the follower will hear first: the next one at or
            // after the beat that is *heard* now.
            let start = next_boundary(grid.beat_at(now + early), self.settings.quantum);
            self.phase = Phase::Armed { start, told: false };
        }
        match self.phase {
            Phase::Idle => None,
            Phase::Armed { .. } if !wanted => {
                self.phase = Phase::Idle;
                None
            }
            Phase::Armed { start, told } => {
                let due = grid.host_at_beat(start) - early;
                Some((if told { ClockEvent::Start } else { ClockEvent::Position(0) }, due))
            }
            Phase::Running { .. } if !wanted => Some((ClockEvent::Stop, now)),
            Phase::Running { origin, next } => {
                let due = grid.host_at_beat(origin + next as f64 / TICKS_PER_BEAT) - early;
                let period = 60e6 / grid.tempo() / TICKS_PER_BEAT;
                let slack = (2.0 * period) as Micros;
                if due < now - slack || due > now + slack {
                    // The timeline jumped: stop, and re-arm on the next
                    // boundary rather than drift the follower's bar.
                    Some((ClockEvent::Stop, now))
                } else {
                    Some((ClockEvent::Tick, due))
                }
            }
        }
    }

    /// `event` (what [`Schedule::next`] gave) has been sent.
    pub fn sent(&mut self, event: ClockEvent) {
        self.phase = match (self.phase, event) {
            (Phase::Armed { start, .. }, ClockEvent::Position(_)) => {
                Phase::Armed { start, told: true }
            }
            (Phase::Armed { start, .. }, ClockEvent::Start) => {
                Phase::Running { origin: start, next: 0 }
            }
            (Phase::Running { origin, next }, ClockEvent::Tick) => {
                Phase::Running { origin, next: next + 1 }
            }
            (_, ClockEvent::Stop) => Phase::Idle,
            (phase, _) => phase,
        };
    }
}

/// Where the bytes go: a MIDI port, or anything a test stands in for one.
pub trait Sink {
    fn send(&mut self, event: ClockEvent, at: Micros);
}

/// The sender loop: read the session, wait for what is due — sleeping
/// until [`simply_midi_clock::LEAD`] before it, then spinning — send it,
/// repeat, until `stop`. A running follower is sent a Stop on the way out.
pub fn run(
    session: impl Fn() -> SessionState,
    clock: HostClock,
    schedule: &mut Schedule,
    sink: &mut impl Sink,
    stop: &AtomicBool,
) {
    while !stop.load(Ordering::Relaxed) {
        let state = session();
        let now = clock.now();
        let Some((event, due)) = schedule.next(&state, now) else {
            thread::sleep(if state.joined() { POLL } else { IDLE_POLL });
            continue;
        };
        let remaining = std::time::Duration::from_micros((due - now).max(0) as u64);
        match pace(remaining) {
            Some(sleep) => thread::sleep(sleep),
            None => {
                spin_until(|| clock.now() >= due);
                sink.send(event, due);
                schedule.sent(event);
            }
        }
    }
    if schedule.running() {
        sink.send(ClockEvent::Stop, clock.now());
        schedule.sent(ClockEvent::Stop);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use simply_link::session::Status;
    use simply_link::timeline::{GhostXForm, StartStop, Timeline};

    /// A joined session at `bpm` with beat 0 at host time 0, playing.
    fn session(bpm: f64) -> SessionState {
        SessionState {
            status: Status::Joined,
            session: None,
            peers: 1,
            founder: false,
            timeline: Timeline::at(bpm, 0.0, 0),
            xform: GhostXForm::default(),
            start_stop: Some(StartStop { playing: true, beats: 0, timestamp: 0 }),
        }
    }

    /// A perfect sender: every event sent exactly at its due instant, from
    /// `from` until `until`, against the session `at(now)` gives.
    fn drive(
        schedule: &mut Schedule,
        mut at: impl FnMut(Micros) -> SessionState,
        from: Micros,
        until: Micros,
    ) -> Vec<(ClockEvent, Micros)> {
        let mut now = from;
        let mut out = Vec::new();
        while now < until {
            match schedule.next(&at(now), now) {
                Some((e, due)) if due <= until => {
                    now = now.max(due);
                    out.push((e, now));
                    schedule.sent(e);
                }
                _ => now += 1000,
            }
        }
        out
    }

    fn ticks(events: &[(ClockEvent, Micros)]) -> Vec<Micros> {
        events.iter().filter(|(e, _)| *e == ClockEvent::Tick).map(|(_, t)| *t).collect()
    }

    /// Joined mid-bar at 120 BPM: nothing until the next bar line (beat 4 =
    /// 2 s), then pointer 0, Start, and a tick every 1/48 s from there.
    #[test]
    fn starts_on_the_next_quantum_boundary_then_ticks_24_a_beat() {
        let mut s = Schedule::new(Settings::default());
        let events = drive(&mut s, |_| session(120.0), 1_300_000, 2_999_999);
        assert_eq!(events[0], (ClockEvent::Position(0), 2_000_000));
        assert_eq!(events[1], (ClockEvent::Start, 2_000_000));
        let ticks = ticks(&events);
        assert_eq!(ticks.len(), 48, "a second at 120 BPM is two beats");
        for (n, t) in ticks.iter().enumerate() {
            let want = 2_000_000 + (n as f64 * 1e6 / 48.0).round() as Micros;
            assert!((t - want).abs() <= 1, "tick {n}: {t} vs {want}");
        }
    }

    /// The Start lands on the same boundary for any quantum's multiples:
    /// quantum 3 from beat 1.2 starts at beat 3.
    #[test]
    fn the_quantum_sets_the_boundary() {
        let mut s = Schedule::new(Settings { quantum: 3.0, ..Settings::default() });
        let events = drive(&mut s, |_| session(60.0), 1_200_000, 3_500_000);
        assert_eq!(events[1], (ClockEvent::Start, 3_000_000));
    }

    /// Latency moves everything earlier by itself — Start included.
    #[test]
    fn latency_sends_everything_early() {
        let mut s = Schedule::new(Settings { latency: 5_000, ..Settings::default() });
        let events = drive(&mut s, |_| session(120.0), 1_300_000, 2_100_000);
        assert_eq!(events[1], (ClockEvent::Start, 1_995_000));
        assert_eq!(ticks(&events)[1], 1_995_000 + 20_833);
    }

    /// In sync mode a stopped session starts nothing; its stop mid-run
    /// sends Stop at once; playing again re-arms at the next boundary.
    #[test]
    fn follows_the_shared_transport() {
        let mut s = Schedule::new(Settings::default());
        let playing = |host: Micros| {
            let mut st = session(120.0);
            let on = !(2_500_000..4_300_000).contains(&host);
            st.start_stop =
                Some(StartStop { playing: host >= 1_000_000 && on, beats: 0, timestamp: 0 });
            st
        };
        assert!(drive(&mut s, playing, 0, 999_000).is_empty(), "stopped: silence");
        let events = drive(&mut s, playing, 1_000_000, 6_500_000);
        let stops: Vec<Micros> =
            events.iter().filter(|(e, _)| *e == ClockEvent::Stop).map(|(_, t)| *t).collect();
        assert_eq!(stops, vec![2_500_000]);
        let starts: Vec<Micros> =
            events.iter().filter(|(e, _)| *e == ClockEvent::Start).map(|(_, t)| *t).collect();
        // Beat 4 (2 s), then — playing again at beat 8.6 — beat 12 (6 s).
        assert_eq!(starts, vec![2_000_000, 6_000_000]);
    }

    /// Free mode runs as soon as joined, whatever the transport says; an
    /// unjoined session is silence.
    #[test]
    fn free_mode_ignores_the_transport_but_not_the_session() {
        let mut s = Schedule::new(Settings { transport: Transport::Free, ..Settings::default() });
        let mut st = session(120.0);
        st.start_stop = None;
        let mut unjoined = st;
        unjoined.status = Status::Searching;
        assert!(drive(&mut s, |_| unjoined, 0, 3_000_000).is_empty());
        let events = drive(&mut s, |_| st, 0, 100_000);
        assert_eq!(events[1], (ClockEvent::Start, 0), "beat 0 is a boundary");
    }

    /// A tempo change mid-run moves only the ticks after it: at 60 BPM from
    /// beat 1 on, ticks are 1/24 s apart.
    #[test]
    fn a_tempo_change_reaches_the_next_tick() {
        let mut s = Schedule::new(Settings { quantum: 1.0, ..Settings::default() });
        let slower = |host: Micros| {
            let mut st = session(120.0);
            if host >= 500_000 {
                st.timeline = st.timeline.retempo(60.0, 500_000);
            }
            st
        };
        let t = ticks(&drive(&mut s, slower, 0, 1_500_000));
        assert!((t[23] - t[22] - 20_833).abs() <= 1, "1/48 s at 120");
        assert!((t[25] - t[24] - 41_667).abs() <= 1, "1/24 s at 60");
        assert_eq!(t[24], 500_000, "beat 1 lands where it did");
    }

    /// A timeline that jumps forwards two beats is a Stop and a fresh Start
    /// on the next boundary — never a burst of catch-up ticks.
    #[test]
    fn a_jump_is_a_stop_and_a_restart() {
        let mut s = Schedule::new(Settings { quantum: 1.0, ..Settings::default() });
        let jumpy = |host: Micros| {
            let mut st = session(120.0);
            if host >= 1_000_000 {
                st.timeline = Timeline::at(120.0, 4.0, 1_000_000); // was beat 2
            }
            st
        };
        let events = drive(&mut s, jumpy, 0, 1_600_000);
        let kinds: Vec<ClockEvent> =
            events.iter().map(|(e, _)| *e).filter(|e| *e != ClockEvent::Tick).collect();
        assert_eq!(
            kinds,
            vec![
                ClockEvent::Position(0),
                ClockEvent::Start,
                ClockEvent::Stop,
                ClockEvent::Position(0),
                ClockEvent::Start
            ]
        );
        let restart = events.iter().rev().find(|(e, _)| *e == ClockEvent::Start).unwrap().1;
        assert_eq!(restart, 1_000_000, "beat 4 is itself a boundary");
        let late = ticks(&events).windows(2).map(|w| w[1] - w[0]).max().unwrap();
        assert!(late <= 20_834, "no burst: {late}");
    }

    /// Asking twice without sending gives the same answer.
    #[test]
    fn next_is_idempotent_until_sent() {
        let mut s = Schedule::new(Settings::default());
        let a = s.next(&session(120.0), 100);
        assert_eq!(a, s.next(&session(120.0), 200));
        assert_eq!(a, Some((ClockEvent::Position(0), 2_000_000)), "the next bar, at 2 s");
    }

    /// Records each event with the instant it was due and the instant it
    /// was actually handed over.
    struct Recorder(HostClock, Vec<(ClockEvent, Micros, Micros)>);
    impl Sink for Recorder {
        fn send(&mut self, e: ClockEvent, due: Micros) {
            self.1.push((e, due, self.0.now()));
        }
    }

    /// The real sender loop against a fake sink, on the real clock, for a
    /// quarter of a second at 300 BPM. Returns what the sink was handed, the
    /// beat-0 boundary, and when the stop was asked for. No MIDI anywhere.
    fn one_window() -> (Vec<(ClockEvent, Micros, Micros)>, Micros, Micros) {
        let clock = HostClock::new();
        let origin = clock.now() + 50_000;
        let state = move || {
            let mut st = session(300.0);
            st.timeline = Timeline::at(300.0, 0.0, origin);
            st
        };
        let stop = AtomicBool::new(false);
        let mut schedule = Schedule::new(Settings { quantum: 1.0, ..Settings::default() });
        let mut sink = Recorder(clock, Vec::new());
        // The window is measured, not assumed: on a loaded machine (a CI
        // runner) the 250 ms sleep can overshoot by tens of milliseconds,
        // and the loop rightly keeps ticking until it is told to stop.
        let stopped_at = std::thread::scope(|scope| {
            let stopper = scope.spawn(|| {
                std::thread::sleep(std::time::Duration::from_millis(250));
                let at = clock.now();
                stop.store(true, Ordering::Relaxed);
                at
            });
            run(state, clock, &mut schedule, &mut sink, &stop);
            stopper.join().unwrap()
        });
        (sink.1, origin, stopped_at)
    }

    /// How late each tick was handed over, sorted.
    fn lateness(events: &[(ClockEvent, Micros, Micros)]) -> Vec<Micros> {
        let mut late: Vec<Micros> = events
            .iter()
            .filter(|(e, ..)| *e == ClockEvent::Tick)
            .map(|(_, due, at)| at - due)
            .collect();
        late.sort_unstable();
        late
    }

    /// What the loop sends, in what order and how many: Start on the
    /// quantum, a tick every 1/120 s from it, never one early, Stop on the
    /// way out. True on any machine, however loaded.
    #[test]
    fn the_sender_loop_keeps_time_against_a_fake_sink() {
        let (events, origin, stopped_at) = one_window();
        assert_eq!(events[1].0, ClockEvent::Start);
        assert_eq!(events[1].1, origin, "beat 0 is the first boundary");
        assert_eq!(events.last().unwrap().0, ClockEvent::Stop);
        let late = lateness(&events);
        // 300 BPM is 120 ticks a second, from `origin` to the stop.
        let expected = (stopped_at - origin) as f64 * 120.0 / 1e6;
        assert!(
            (late.len() as f64 - expected).abs() <= 3.0,
            "300 BPM is 120 ticks a second: {} over {} µs",
            late.len(),
            stopped_at - origin
        );
        assert!(late.iter().all(|l| *l >= 0), "never early");
    }

    /// How close to due a tick is handed over: under a millisecond, median.
    /// A measurement of the machine as much as of the loop — a shared CI
    /// runner preempts the spinning thread and reads ~3 ms — so it is run
    /// deliberately, on a quiet machine, rather than deciding a build.
    #[test]
    #[ignore = "timing measurement"]
    fn the_sender_loop_hands_a_tick_over_within_a_millisecond() {
        let late = lateness(&one_window().0);
        assert!(late[late.len() / 2] < 1_000, "median lateness {} µs", late[late.len() / 2]);
    }
}
