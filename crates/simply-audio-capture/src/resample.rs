//! A capture's frames at another rate than its device's: band-limited
//! interpolation, a Kaiser-windowed sinc, fed one frame at a time on the
//! device's real-time thread.
//!
//! **Why not linear.** Linear interpolation from 44.1 kHz to 48 kHz leaves
//! the images of everything above a few kHz in the band, folded to
//! frequencies that have nothing to do with the music: a 10 kHz tone comes
//! out with its aliases 20 dB under it (a 5 kHz one, 34 dB), heard as a
//! grainy fizz on everything. This filter keeps the band 75 dB or more clean.
//!
//! **The filter.** [`HALF`] taps a side at the input rate, cut off just
//! under the lower of the two Nyquists (wider, by the ratio, when
//! converting down, so what does not fit the output is filtered out, not
//! folded in). Its phases are tabulated in [`Resampler::new`], [`PHASES`]
//! of them, linearly interpolated between; each sums to 1, so a constant
//! comes out exactly. Before the first frame the input is taken to have
//! held that frame, so a capture starts without a step.
//!
//! **Latency.** An output frame is made once the input frames after it
//! that its taps reach have arrived: it runs `half` input frames behind
//! the input (16 at most rates, a third of a millisecond at 44.1 kHz).
//! Each output frame is handed over with its position in the input, so
//! its capture time is exact all the same.

/// Taps a side, at the input rate, when converting up.
const HALF: usize = 16;
/// Phases tabulated per input frame.
const PHASES: usize = 256;
/// The Kaiser window's β: ~80 dB stopband.
const BETA: f64 = 8.0;
/// The cutoff, as a fraction of the lower Nyquist: room for the
/// transition band below it, so 44.1 kHz's content up to 20 kHz passes and
/// its images (from 22.05 kHz up) are stopped.
const CUTOFF: f64 = 0.92;

/// Band-limited resampler, fed one frame at a time. Allocates only in
/// [`Resampler::new`].
pub(crate) struct Resampler {
    channels: usize,
    /// Input frames per output frame.
    step: f64,
    /// Taps a side, and both sides.
    half: usize,
    taps: usize,
    /// `PHASES + 1` rows of `taps` weights: row `k` for a read `k / PHASES`
    /// of a frame past a frame.
    table: Vec<f32>,
    /// The last `taps` input frames, interleaved, each written twice
    /// (at `head` and `head + taps`), so the window is always one slice.
    hist: Vec<f32>,
    head: usize,
    /// Input frames fed so far.
    fed: u64,
    /// The input position of the next output frame (frames from the first).
    pos: f64,
    cur: Vec<f32>,
}

impl Resampler {
    pub(crate) fn new(from_hz: f64, to_hz: f64, channels: usize) -> Resampler {
        let channels = channels.max(1);
        let step = from_hz / to_hz;
        // The cutoff in cycles per input frame × 2 (1: the input's Nyquist).
        let c = CUTOFF * (1.0 / step).min(1.0);
        let half = (HALF as f64 / (c / CUTOFF)).ceil() as usize;
        let taps = 2 * half;
        let i0b = bessel_i0(BETA);
        let mut table = vec![0.0f32; (PHASES + 1) * taps];
        for (p, row) in table.chunks_exact_mut(taps).enumerate() {
            let d = p as f64 / PHASES as f64;
            let mut w = vec![0.0f64; taps];
            for (j, w) in w.iter_mut().enumerate() {
                // Tap j is frame floor(pos) − (half − 1) + j: t frames from pos.
                let t = j as f64 - (half as f64 - 1.0) - d;
                let x = t / half as f64;
                let window =
                    if x.abs() >= 1.0 { 0.0 } else { bessel_i0(BETA * (1.0 - x * x).sqrt()) / i0b };
                *w = c * sinc(c * t) * window;
            }
            let sum: f64 = w.iter().sum();
            for (r, w) in row.iter_mut().zip(&w) {
                *r = (w / sum) as f32;
            }
        }
        Resampler {
            channels,
            step,
            half,
            taps,
            table,
            hist: vec![0.0; 2 * taps * channels],
            head: 0,
            fed: 0,
            pos: 0.0,
            cur: vec![0.0; channels],
        }
    }

    /// Input frames fed so far: the index the next one will have.
    pub(crate) fn fed(&self) -> u64 {
        self.fed
    }

    /// Take the next input frame; `emit(frame, pos)` every output frame
    /// that can now be made, `pos` its position in the input (frames from
    /// the first one fed, fractional; it lags the input by `half`).
    pub(crate) fn feed(&mut self, frame: &[f32], mut emit: impl FnMut(&[f32], f64)) {
        let (ch, taps) = (self.channels, self.taps);
        if self.fed == 0 {
            // Before the first frame: that frame, held.
            for slot in self.hist.chunks_exact_mut(ch) {
                slot.copy_from_slice(&frame[..ch]);
            }
        }
        let at = self.head * ch;
        self.hist[at..at + ch].copy_from_slice(&frame[..ch]);
        self.hist[at + taps * ch..at + (taps + 1) * ch].copy_from_slice(&frame[..ch]);
        self.head = (self.head + 1) % taps;
        self.fed += 1;
        // The window: input frames fed − taps .. fed − 1, oldest first.
        let newest = self.fed as f64 - 1.0;
        while self.pos.floor() + self.half as f64 <= newest {
            let window = &self.hist[self.head * ch..(self.head + taps) * ch];
            let frac = self.pos - self.pos.floor();
            let p = frac * PHASES as f64;
            let k = (p as usize).min(PHASES - 1);
            let a = (p - k as f64) as f32;
            let (lo, hi) = (
                &self.table[k * taps..(k + 1) * taps],
                &self.table[(k + 1) * taps..(k + 2) * taps],
            );
            // The nearest frame, and the taps' differences to it: a
            // constant comes out exactly.
            let near = (self.half - 1 + (frac >= 0.5) as usize) * ch;
            for (c, y) in self.cur.iter_mut().enumerate() {
                let x0 = window[near + c];
                let mut acc = 0.0f32;
                for t in 0..taps {
                    acc += (lo[t] + a * (hi[t] - lo[t])) * (window[t * ch + c] - x0);
                }
                *y = x0 + acc;
            }
            emit(&self.cur, self.pos);
            self.pos += self.step;
        }
    }
}

fn sinc(t: f64) -> f64 {
    if t == 0.0 {
        1.0
    } else {
        let x = std::f64::consts::PI * t;
        x.sin() / x
    }
}

/// The modified Bessel function of the first kind, order 0 (its series).
fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term, mut k) = (1.0, 1.0, 1.0);
    while term > sum * 1e-17 {
        term *= (x / (2.0 * k)) * (x / (2.0 * k));
        sum += term;
        k += 1.0;
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    /// A 0.5 tone at `hz` from `from` Hz to `to`: every output frame
    /// against the tone at its position in the input (after the start),
    /// the worst single frame's error in dB under the tone (stricter
    /// than THD+N, an RMS).
    fn error_db(from: f64, to: f64, hz: f64) -> f64 {
        let mut r = Resampler::new(from, to, 1);
        let w = 2.0 * PI * hz / from;
        let mut worst = 0.0f64;
        for i in 0..(from as usize / 2) {
            r.feed(&[(0.5 * (w * i as f64).sin()) as f32], |o, pos| {
                if pos > 200.0 {
                    worst = worst.max((o[0] as f64 - 0.5 * (w * pos).sin()).abs());
                }
            });
        }
        20.0 * (worst / 0.5).log10()
    }

    /// Linear interpolation measured −20 dB at 10 kHz and −34 at 5 kHz
    /// from 44.1 to 48 kHz (THD+N); this keeps the audible band 75 dB
    /// clean (measured: −78 dB at 10 kHz, the worst frame).
    #[test]
    fn a_tone_comes_through_clean_up_and_down() {
        for (from, to) in
            [(44_100.0, 48_000.0), (48_000.0, 44_100.0), (48_000.0, 96_000.0), (96_000.0, 48_000.0)]
        {
            for (hz, floor) in [
                (100.0, -95.0),
                (1_000.0, -75.0),
                (5_000.0, -75.0),
                (10_000.0, -75.0),
                (16_000.0, -72.0),
            ] {
                let db = error_db(from, to, hz);
                assert!(db < floor, "{hz} Hz from {from} to {to}: {db:.1} dB");
            }
        }
    }

    /// Above the output's Nyquist, converting down: filtered out, not
    /// folded back into the band.
    #[test]
    fn what_does_not_fit_the_output_is_filtered_out() {
        let mut r = Resampler::new(96_000.0, 48_000.0, 1);
        let w = 2.0 * PI * 30_000.0 / 96_000.0;
        let mut peak = 0.0f32;
        for i in 0..48_000 {
            r.feed(&[(0.5 * (w * i as f64).sin()) as f32], |o, pos| {
                if pos > 200.0 {
                    peak = peak.max(o[0].abs());
                }
            });
        }
        let db = 20.0 * (peak as f64 / 0.5).log10();
        assert!(db < -70.0, "30 kHz at 48 kHz: {db:.1} dB");
    }

    #[test]
    fn a_constant_comes_out_exact_from_the_first_frame() {
        let mut r = Resampler::new(44_100.0, 48_000.0, 2);
        let mut n = 0;
        for _ in 0..1000 {
            r.feed(&[0.25, -0.5], |o, _| {
                assert_eq!(o, [0.25, -0.5]);
                n += 1;
            });
        }
        assert!(n > 1000, "{n}");
    }
}
