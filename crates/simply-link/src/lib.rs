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
//! Provenance and what is and is not verified against real peers:
//! `docs/research/engine/ableton-link.md`.

#![forbid(unsafe_code)]

pub mod clock;
pub mod measure;
mod net;
pub mod session;
pub mod timeline;
pub mod wire;

pub use clock::HostClock;
pub use net::{
    Interface, Link, Network, RESCAN, address_for, burst, discovery_listener, interfaces,
};
pub use session::{Config, SessionState, Status};
pub use timeline::{Micros, StartStop, Timeline};
pub use wire::{NodeId, SessionId};
