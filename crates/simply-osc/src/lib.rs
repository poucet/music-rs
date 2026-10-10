//! Open Sound Control 1.0, in Rust — an independent implementation of the
//! protocol layer, written against the OSC 1.0 specification
//! (opensoundcontrol.stanford.edu/spec-1_0.html), not against any existing
//! library.
//!
//! OSC is how game engines, controller apps and headsets speak to audio
//! software: typed messages at hierarchical addresses, over UDP. This crate
//! is the protocol floor — bytes in, typed values out — and nothing above
//! it:
//!
//! * [`types`] — [`Message`], [`Bundle`], [`Packet`], [`Value`],
//!   [`TimeTag`]: what a packet *is*, independent of any socket;
//! * [`wire`] — bytes ↔ packet, with the 4-byte alignment rules exact and
//!   every malformed input an error value, never a panic (this parses
//!   untrusted UDP);
//! * [`pattern`] — OSC address pattern matching (`?`, `*`, `[]`, `{}`)
//!   against an [`AddressSpace`] of registered addresses;
//! * [`recv`] — a thin UDP [`Listener`]: bind, receive, parse, hand each
//!   typed packet over a channel. Everything below it tests without a
//!   socket.
//!
//! ```no_run
//! use simply_osc::{Listener, Packet, Value};
//!
//! let listener = Listener::bind("0.0.0.0:9000")?;
//! while let Some(inbound) = listener.recv() {
//!     if let Packet::Message(m) = inbound.packet {
//!         if let Some(Value::Float32(x)) = m.args.first() {
//!             println!("{} = {x}", m.address);
//!         }
//!     }
//! }
//! # Ok::<(), std::io::Error>(())
//! ```
//!
//! **What is implemented.** The four required argument types (`i` int32,
//! `f` float32, `s` string, `b` blob) and the extended types a performance
//! input meets in practice: `h` int64, `d` double, `t` timetag, `T`/`F`
//! booleans and `N` nil. Bundles nest, carry NTP-style timetags, and parse
//! recursively; the *scheduling* a timetag asks for is the consumer's job —
//! this crate parses and exposes the tag, it does not hold packets back.
//! A message whose type tag string is absent (pre-1.0 senders exist)
//! parses as its address with no arguments.
//!
//! **What is deliberately out.**
//! * A sending surface: Chris ruled OSC in as an *input*. [`Message::encode`]
//!   and [`Bundle::encode`] exist — the parser's round-trip tests need
//!   them, and a sender is one `UdpSocket::send_to` away when wanted — but
//!   no send socket, no client type.
//! * Delivery scheduling: a bundle's timetag says *when the consumer should
//!   act*, and the consumer owns its clock.
//! * The OSC 1.1 extensions: `//` (any-depth matching), `I` impulse, the
//!   `#reply` scheme. 1.0 is what controller apps emit; 1.1 can be added
//!   behind the same types if a real sender demands it.
//! * The rarer 1.0 optional types: `c` char, `r` RGBA, `m` MIDI, `[`/`]`
//!   arrays. Unknown tags are a [`wire::ParseError`], not a skip — a
//!   message this crate accepts is one it fully understood.

#![forbid(unsafe_code)]

pub mod pattern;
pub mod recv;
pub mod types;
pub mod wire;

pub use pattern::{AddressError, AddressSpace, matches};
pub use recv::{Inbound, Listener};
pub use types::{Bundle, Message, Packet, TimeTag, Value};
pub use wire::{EncodeError, ParseError};
