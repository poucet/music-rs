//! Learning the session's clock: ping-pong measurement.
//!
//! A joining peer does not know where ghost time is. It asks a peer that
//! does — any member of the session, at the `mep4` endpoint that peer
//! announces — in a burst of pings (Goltz 2018, §6, fig. 2). Each ping says
//! when it left, in the pinger's host time; each pong answers with the
//! responder's ghost time at the moment it answered. If the network is
//! symmetric the answer was given halfway through the round trip, so one
//! exchange gives one estimate of the offset:
//!
//! ```text
//! intercept ≈ ghost_at_responder − (host_sent + host_received) / 2
//! ```
//!
//! A single estimate is as bad as the round trip that carried it — a Wi-Fi
//! retry can make one exchange milliseconds lopsided — so the burst is
//! reduced to its **median**, which a few outliers cannot move.
//!
//! Everything here is pure; the sockets that carry it are in [`crate::net`].

use crate::timeline::Micros;
use crate::wire::{Entry, Measurement, MeasurementKind};

/// Pings in one burst.
pub const BURST: usize = 20;
/// The fewest answered pings a measurement is accepted on.
pub const MIN_SAMPLES: usize = 5;
/// How long a ping waits for its pong before it is counted lost.
pub const PONG_TIMEOUT_MS: u64 = 50;
/// A round trip longer than this is discarded outright: its midpoint says
/// nothing better than a few milliseconds.
pub const MAX_ROUND_TRIP: Micros = 50_000;

/// The ping a pinger sends at host time `host_now`.
pub fn ping(host_now: Micros) -> Vec<u8> {
    Measurement { kind: MeasurementKind::Ping, entries: vec![Entry::HostTime(host_now)] }.encode()
}

/// A responder's answer to `request`, if it is a ping: every entry of the
/// ping echoed untouched, plus the responder's ghost time now. Echoing
/// rather than rebuilding means a pinger's own bookkeeping comes back to it
/// whatever it chose to put there.
pub fn pong(request: &[u8], ghost_now: Micros) -> Option<Vec<u8>> {
    let ping = Measurement::decode(request).ok()?;
    if ping.kind != MeasurementKind::Ping {
        return None;
    }
    let mut entries = vec![Entry::GhostTime(ghost_now)];
    entries.extend(ping.entries.into_iter().filter(|e| !matches!(e, Entry::GhostTime(_))));
    Some(Measurement { kind: MeasurementKind::Pong, entries }.encode())
}

/// One estimate of the intercept from a pong received at `host_received`
/// for a ping sent at `host_sent`, or `None` if it is not a pong, has no
/// ghost time, or took too long to say anything precise.
pub fn sample(response: &[u8], host_sent: Micros, host_received: Micros) -> Option<Micros> {
    let pong = Measurement::decode(response).ok()?;
    if pong.kind != MeasurementKind::Pong {
        return None;
    }
    let ghost = pong.ghost_time()?;
    let round_trip = host_received - host_sent;
    if !(0..=MAX_ROUND_TRIP).contains(&round_trip) {
        return None;
    }
    Some(ghost - (host_sent + host_received) / 2)
}

/// The intercept a burst settles on: the median of its samples, or `None`
/// below [`MIN_SAMPLES`].
pub fn settle(samples: &mut [Micros]) -> Option<Micros> {
    if samples.len() < MIN_SAMPLES {
        return None;
    }
    samples.sort_unstable();
    let n = samples.len();
    Some(if n % 2 == 1 { samples[n / 2] } else { (samples[n / 2 - 1] + samples[n / 2]) / 2 })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A responder whose ghost clock is 1 000 000 µs ahead of the pinger's
    /// host clock, over a network that takes `up` there and `down` back.
    fn exchange(sent: Micros, up: Micros, down: Micros) -> Option<Micros> {
        let offset = 1_000_000;
        let reply = pong(&ping(sent), sent + up + offset)?;
        sample(&reply, sent, sent + up + down)
    }

    #[test]
    fn a_symmetric_exchange_measures_the_offset_exactly() {
        assert_eq!(exchange(5_000, 300, 300), Some(1_000_000));
    }

    #[test]
    fn an_asymmetric_one_is_off_by_half_the_asymmetry() {
        assert_eq!(exchange(5_000, 900, 100), Some(1_000_400));
    }

    #[test]
    fn the_median_ignores_a_few_lopsided_round_trips() {
        let mut samples: Vec<Micros> = (0..BURST as i64)
            .map(|i| match i {
                3 | 11 => exchange(i * 10_000, 9_000, 100).unwrap(),
                _ => exchange(i * 10_000, 250, 250).unwrap(),
            })
            .collect();
        assert_eq!(settle(&mut samples), Some(1_000_000));
        assert_eq!(settle(&mut samples[..MIN_SAMPLES - 1].to_vec()), None);
    }

    #[test]
    fn a_pong_echoes_the_ping_and_only_answers_pings() {
        let reply = pong(&ping(42), 99).unwrap();
        let m = Measurement::decode(&reply).unwrap();
        assert_eq!(m.kind, MeasurementKind::Pong);
        assert_eq!(m.host_time(), Some(42));
        assert_eq!(m.ghost_time(), Some(99));
        assert_eq!(pong(&reply, 1), None, "a pong is not answered");
        assert_eq!(pong(b"garbage", 1), None);
    }

    #[test]
    fn a_slow_or_backwards_round_trip_is_discarded() {
        assert_eq!(exchange(0, MAX_ROUND_TRIP, 1), None);
        let reply = pong(&ping(10), 5).unwrap();
        assert_eq!(sample(&reply, 10, 9), None, "received before it was sent");
        assert_eq!(sample(&ping(10), 10, 11), None, "a ping is not a sample");
    }
}
