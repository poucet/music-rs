//! Ableton Link, in Rust — an independent implementation of the protocol,
//! written from its public description, not from Ableton's SDK.
//!
//! Link shares a musical beat, tempo, phase and start/stop state between
//! peers on a local network, with no master (Goltz, *Ableton Link — A
//! technology to synchronize music software*, LAC 2018). This crate is one
//! such peer:
//!
//! * [`wire`] — the discovery and measurement messages, byte for byte;
//! * [`timeline`] — the session's beat line, ghost time, quantum and phase;
//! * [`measure`] — the ping-pong burst that learns the session's clock;
//! * [`session`] — the rules a peer follows, as a pure [`session::Engine`];
//! * [`Link`] — the engine on real sockets and threads.
//!
//! ```no_run
//! use simply_link::{Config, Link};
//!
//! let link = Link::join(Config::default())?;           // follow a session
//! let now = link.clock().now();                        // host µs
//! let state = link.state();
//! if state.joined() {
//!     let phase = state.phase_at(now, 4.0);            // where in the bar
//!     let next = state.next_boundary(now, 4.0);        // the next downbeat…
//!     let when = state.host_at_beat(next);             // …and when it lands
//!     println!("{:.2} BPM, phase {phase:.3}, next bar at {when} µs", state.tempo());
//! }
//! # Ok::<(), std::io::Error>(())
//! ```
//!
//! ## Moving the beat
//!
//! What peers share is the session's tempo and *phase*; the beat's number
//! is each peer's own. [`SessionState`] answers in the session's beats, as
//! it always has; [`Link::local`] gives a [`LocalBeats`] that answers in
//! this peer's ([`LocalBeats::beat_offset`]). Three verbs move it,
//! after Ableton's `SessionState` (rule 7 in [`session`]); `at` is host
//! µs, as for [`Link::propose_tempo`]:
//!
//! * [`Link::request_beat_at`]`(beat, at, quantum)` — Ableton's
//!   `requestBeatAtTime`, the polite one. Alone in the session, it moves
//!   the session so `beat` falls at `at`. With peers it moves nothing they
//!   see: it renumbers our beats by whole quanta, so `beat` falls at the
//!   first moment from `at` on when the session's phase is `beat`'s.
//! * [`Link::force_beat_at`]`(beat, at, quantum)` — Ableton's
//!   `forceBeatAtTime`, the rude one. It shifts the session's line by
//!   less than half a quantum so `beat`'s phase falls at `at` for **every**
//!   peer, and broadcasts it like a tempo change (same tempo, a new anchor
//!   every peer, Ableton's included, adopts); our own beat at `at` is
//!   `beat` exactly.
//! * [`Link::propose_timeline`]`(bpm, beat, at, quantum)` — a tempo and a
//!   force in one commit and one broadcast: no peer ever holds the new
//!   tempo without the new phase.
//!
//! **Forcing is anti-social.** It moves every other app's beat: a DJ in
//! djay or Live sees their phase jump, mid-mix. Ableton reserves it for
//! "certain special circumstances". In tussen that circumstance is one: a
//! hub that bridges its local Link session to the mesh timeline as that
//! timeline's *follower* — where the mesh, not the room, owns the beat. It
//! is never a default, and never what a hub that merely joins a room does;
//! there, `request_beat_at` (or nothing) is right.
//!
//! Provenance and what is and is not verified against real peers:
//! `docs/research/engine/ableton-link.md`.

#![forbid(unsafe_code)]

pub mod clock;
pub mod measure;
mod net;
#[cfg(test)]
mod reference;
pub mod session;
pub mod timeline;
pub mod wire;

pub use clock::HostClock;
pub use net::{
    Interface, Link, Network, RESCAN, address_for, burst, discovery_listener, interfaces,
};
pub use session::{Config, LocalBeats, PeerInfo, PeerPath, SessionState, Status};
pub use timeline::{Micros, StartStop, Timeline};
pub use wire::{NodeId, SessionId};
