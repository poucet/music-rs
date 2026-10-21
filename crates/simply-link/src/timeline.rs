//! The session's musical time, as numbers.
//!
//! Three clocks meet here, and every function says which it takes:
//!
//! * **host time** — this machine's monotonic clock, in µs from a
//!   process-local epoch ([`crate::HostClock`]);
//! * **ghost time** — the session's shared clock, in µs. Every peer maps
//!   its host time onto it with a [`GhostXForm`] learned by measurement;
//!   the founder's runs at its host clock's rate from 0 at founding, so
//!   ghost time reads as the session's age;
//! * **beats** — musical time. A [`Timeline`] is a line through ghost time:
//!   a tempo, and one point `(time_origin, beat_origin)` it passes through
//!   (Goltz 2018, §4).
//!
//! On the wire every quantity is an integer — µs per beat, µbeats, µs — so
//! the stored forms are `i64`. The arithmetic between them is `f64`: 52 bits
//! hold a µs count for 142 years, and a beat to a nanobeat for longer than
//! any session.

/// µs — host or ghost, as named.
pub type Micros = i64;

/// A µbeat, the wire's unit of musical time.
pub const MICROBEATS: f64 = 1e6;

/// The shared beat line: `beat(t) = beat_origin + (t − time_origin) / tempo`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Timeline {
    /// Tempo as µs per beat — 500 000 is 120 BPM.
    pub micros_per_beat: i64,
    /// The beat the line passes through at `time_origin`, in µbeats.
    pub beat_origin: i64,
    /// The ghost time of that point, in µs.
    pub time_origin: i64,
}

/// The tempo range a timeline may carry. Outside it a proposal is clamped
/// and a received timeline is refused — a zero tempo would divide by zero.
pub const MIN_BPM: f64 = 20.0;
pub const MAX_BPM: f64 = 999.0;

impl Timeline {
    /// A timeline at `bpm` passing through `beat` at ghost time `ghost`.
    pub fn at(bpm: f64, beat: f64, ghost: Micros) -> Self {
        let bpm = bpm.clamp(MIN_BPM, MAX_BPM);
        Timeline {
            micros_per_beat: (60e6 / bpm).round() as i64,
            beat_origin: (beat * MICROBEATS).round() as i64,
            time_origin: ghost,
        }
    }

    /// Is this a timeline a peer could mean — a tempo inside the range?
    pub fn is_sane(&self) -> bool {
        let bpm = self.bpm();
        self.micros_per_beat > 0 && (MIN_BPM - 0.5..=MAX_BPM + 0.5).contains(&bpm)
    }

    pub fn bpm(&self) -> f64 {
        60e6 / self.micros_per_beat as f64
    }

    /// The beat at ghost time `ghost`.
    pub fn beat_at(&self, ghost: Micros) -> f64 {
        self.beat_origin as f64 / MICROBEATS
            + (ghost - self.time_origin) as f64 / self.micros_per_beat as f64
    }

    /// The ghost time at which the line reaches `beat` (rounded to a µs).
    pub fn ghost_at(&self, beat: f64) -> Micros {
        self.time_origin
            + ((beat - self.beat_origin as f64 / MICROBEATS) * self.micros_per_beat as f64).round()
                as i64
    }

    /// Whether this timeline supersedes `other` when members of one
    /// session announce different ones: the later beat origin wins — a
    /// change is made at a later beat than the line it changes, Ableton's
    /// own proxy for "newer" — and a tie goes to the later time origin,
    /// then the slower tempo. A total order, so every peer settles on the
    /// same line whatever order the announcements arrive in; two proposals
    /// that cross on the network can't swap places forever.
    pub fn newer_than(&self, other: &Timeline) -> bool {
        (self.beat_origin, self.time_origin, self.micros_per_beat)
            > (other.beat_origin, other.time_origin, other.micros_per_beat)
    }

    /// The same beat line at a new tempo, crossing the old one at ghost time
    /// `ghost` — what a tempo change *is* (Goltz 2018, §4, fig. 1): the beat
    /// at the moment of the change does not jump.
    pub fn retempo(&self, bpm: f64, ghost: Micros) -> Self {
        Timeline::at(bpm, self.beat_at(ghost), ghost)
    }
}

/// Host time → ghost time. A pure offset: both clocks run at a second a
/// second, and drift between them is taken up by re-measuring.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct GhostXForm {
    pub intercept: Micros,
}

impl GhostXForm {
    pub fn host_to_ghost(&self, host: Micros) -> Micros {
        host + self.intercept
    }
    pub fn ghost_to_host(&self, ghost: Micros) -> Micros {
        ghost - self.intercept
    }
}

/// The session's transport, shared when peers use start/stop sync.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct StartStop {
    pub playing: bool,
    /// The beat it changed at, in µbeats.
    pub beats: i64,
    /// When it changed, in ghost µs — the newer state wins.
    pub timestamp: Micros,
}

/// Where `beat` sits inside a `quantum` of beats: `[0, quantum)`. Peers with
/// the same quantum agree on it — that is the phase guarantee (Goltz 2018,
/// §7). A quantum ≤ 0 means no grid: the phase is always 0.
pub fn phase(beat: f64, quantum: f64) -> f64 {
    if quantum > 0.0 { beat.rem_euclid(quantum) } else { 0.0 }
}

/// The first quantum boundary at or after `beat`.
pub fn next_boundary(beat: f64, quantum: f64) -> f64 {
    if quantum > 0.0 { (beat / quantum).ceil() * quantum } else { beat }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timeline_maps_both_ways() {
        let t = Timeline::at(120.0, 4.0, 1_000_000);
        assert_eq!(t.micros_per_beat, 500_000);
        assert_eq!(t.beat_at(1_000_000), 4.0);
        assert_eq!(t.beat_at(1_500_000), 5.0);
        assert_eq!(t.beat_at(0), 2.0);
        assert_eq!(t.ghost_at(8.0), 3_000_000);
        for b in [-3.25, 0.0, 17.125, 1e5] {
            assert!((t.beat_at(t.ghost_at(b)) - b).abs() < 1e-6, "{b}");
        }
    }

    #[test]
    fn a_tempo_change_keeps_the_beat_where_it_was() {
        let t = Timeline::at(120.0, 0.0, 0);
        let at = 2_000_000; // beat 4
        let u = t.retempo(60.0, at);
        assert_eq!(u.beat_at(at), 4.0);
        assert_eq!(u.beat_at(at + 1_000_000), 5.0, "one beat a second now");
        assert!((u.bpm() - 60.0).abs() < 1e-9);
    }

    #[test]
    fn tempo_is_clamped_and_checked() {
        assert!((Timeline::at(5000.0, 0.0, 0).bpm() - MAX_BPM).abs() < 0.01);
        assert!(Timeline::at(120.0, 0.0, 0).is_sane());
        let zero = Timeline { micros_per_beat: 0, beat_origin: 0, time_origin: 0 };
        assert!(!zero.is_sane());
    }

    #[test]
    fn phase_and_boundaries_on_a_quantum() {
        assert_eq!(phase(9.5, 4.0), 1.5);
        assert_eq!(phase(-0.5, 4.0), 3.5);
        assert_eq!(phase(7.0, 0.0), 0.0);
        assert_eq!(next_boundary(9.5, 4.0), 12.0);
        assert_eq!(next_boundary(12.0, 4.0), 12.0, "a boundary is its own next");
        assert_eq!(next_boundary(-1.0, 3.0), 0.0);
        // Quanta of 3 and 4 share a downbeat every 12 (Goltz 2018, §7).
        let shared: Vec<f64> = (0..=24)
            .map(f64::from)
            .filter(|b| phase(*b, 3.0) == 0.0 && phase(*b, 4.0) == 0.0)
            .collect();
        assert_eq!(shared, vec![0.0, 12.0, 24.0]);
    }

    #[test]
    fn ghost_xform_is_an_offset() {
        let x = GhostXForm { intercept: -250 };
        assert_eq!(x.host_to_ghost(1000), 750);
        assert_eq!(x.ghost_to_host(750), 1000);
    }
}
