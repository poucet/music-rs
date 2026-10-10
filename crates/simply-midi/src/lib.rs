//! MIDI without a host's flavour: one decoder, one way onto the machine's
//! input ports.
//!
//! * [`MidiMessage`] — MIDI 1.0 channel-voice decoding, bytes in and a typed
//!   message out. A standalone shell feeds it raw bytes from midir; a CLAP
//!   plugin feeds it the 3 bytes of a `clap_event_midi`.
//! * [`input`] (feature `device`) — midir port enumeration, opening, and
//!   [`input::identify`], the wiggle-to-identify listener.
//!
//! Everything here takes the midir **client name** as an argument: which
//! name a port is registered under is the host application's business, and
//! this crate has no host. What a decoded message *means* — mappings,
//! channels, zones — is likewise the consumer's; this crate stops at the
//! wire.
//!
//! Out of scope: sysex, running status, MIDI 2.0, and the
//! clock messages (a host schedules those against its own transport —
//! `simply-midi-clock` owns their bytes and pacing).

#![forbid(unsafe_code)]

mod message;

#[cfg(feature = "device")]
pub mod input;

pub use message::{MidiMessage, SOURCE_AFTERTOUCH, SOURCE_COUNT, SOURCE_PITCH_BEND, source_name};

// The one port matcher, re-exported so a consumer that filters port names by
// the same rule as [`input::open_inputs`] does not grow its own copy (or its
// own dependency on the clock crate).
pub use simply_midi_clock::name_matches;

#[cfg(test)]
mod tests {
    #[test]
    fn the_port_matcher_is_re_exported() {
        assert!(crate::name_matches("MPK mini mk3 Port 1", "mpk"));
        assert!(!crate::name_matches("IAC Driver Bus 1", "mpk"));
    }
}
