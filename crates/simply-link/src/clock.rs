//! Host time: this machine's monotonic clock, in µs from a process-local
//! epoch. Every time the engine is told is on this clock, and a caller that
//! schedules against real instants converts with [`HostClock::instant`].

use std::time::{Duration, Instant};

use crate::timeline::Micros;

/// A monotonic clock reading µs since it was made. `Copy`: every copy of
/// one clock agrees, so the network threads and a caller share it freely.
#[derive(Copy, Clone, Debug)]
pub struct HostClock {
    epoch: Instant,
}

impl Default for HostClock {
    fn default() -> Self {
        Self::new()
    }
}

impl HostClock {
    pub fn new() -> Self {
        HostClock { epoch: Instant::now() }
    }

    /// Host µs now.
    pub fn now(&self) -> Micros {
        self.micros(Instant::now())
    }

    /// Host µs at `instant` (negative before the epoch).
    pub fn micros(&self, instant: Instant) -> Micros {
        match instant.checked_duration_since(self.epoch) {
            Some(d) => d.as_micros() as Micros,
            None => -(self.epoch.duration_since(instant).as_micros() as Micros),
        }
    }

    /// The instant at host µs `at` — what a sender spins to.
    pub fn instant(&self, at: Micros) -> Instant {
        if at >= 0 {
            self.epoch + Duration::from_micros(at as u64)
        } else {
            self.epoch.checked_sub(Duration::from_micros(at.unsigned_abs())).unwrap_or(self.epoch)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn micros_and_instants_round_trip() {
        let c = HostClock::new();
        for at in [0, 1, 1_500_000, 3_600_000_000] {
            assert_eq!(c.micros(c.instant(at)), at);
        }
        assert!(c.now() >= 0);
    }
}
