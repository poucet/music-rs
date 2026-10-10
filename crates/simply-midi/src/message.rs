//! MIDI 1.0 channel-voice decoding — the one decoder every shell shares.
//!
//! A standalone shell feeds it raw bytes from midir; a CLAP plugin feeds
//! it the 3 bytes of a `clap_event_midi`. Sysex, MIDI 2.0 and running status
//! are deliberately out of scope. Per-note expression *is* decoded here (poly
//! aftertouch); interpreting it — MPE zones, channel-per-note demux — is the
//! consumer's, because what a note's pressure *reaches* is host business.

/// A decoded channel-voice message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MidiMessage {
    NoteOn {
        channel: u8,
        key: u8,
        velocity: u8,
    },
    NoteOff {
        channel: u8,
        key: u8,
    },
    ControlChange {
        channel: u8,
        cc: u8,
        value: u8,
    },
    /// Program change: select patch `program` on the channel. It names a
    /// sound, not a knob position, so [`MidiMessage::source`] is `None`.
    ProgramChange {
        channel: u8,
        program: u8,
    },
    /// 14-bit, 0..=16383 with 8192 at rest.
    PitchBend {
        channel: u8,
        value: u16,
    },
    /// Channel aftertouch — every note on the channel. Under MPE each note
    /// owns a channel, so this *is* the note's pressure.
    ChannelPressure {
        channel: u8,
        value: u8,
    },
    /// Polyphonic aftertouch: pressure for one key, whatever the mode. Not a
    /// knob source — it addresses voices, so [`MidiMessage::source`] is
    /// `None`; an MPE layer routes it.
    PolyPressure {
        channel: u8,
        key: u8,
        value: u8,
    },
}

/// Mapping source code for pitch bend — one past the 0..=127 CC range, so a
/// single table addresses every continuous controller.
pub const SOURCE_PITCH_BEND: u16 = 128;
/// Mapping source code for channel aftertouch.
pub const SOURCE_AFTERTOUCH: u16 = 129;
/// One past the last valid source code.
pub const SOURCE_COUNT: u16 = 130;

/// Human name of a source code, as the API and the mapping file spell it.
pub fn source_name(source: u16) -> String {
    match source {
        SOURCE_PITCH_BEND => "pitch_bend".into(),
        SOURCE_AFTERTOUCH => "aftertouch".into(),
        cc => format!("cc{cc}"),
    }
}

impl MidiMessage {
    /// Decode one complete channel-voice message. `None` for anything else
    /// (system messages, sysex, a truncated or running-status packet).
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let status = *bytes.first()?;
        if !(0x80..0xf0).contains(&status) {
            return None; // running status / system common / realtime
        }
        let channel = status & 0x0f;
        let d1 = *bytes.get(1)? & 0x7f;
        match status & 0xf0 {
            0x80 => Some(Self::NoteOff { channel, key: d1 }),
            0xa0 => Some(Self::PolyPressure { channel, key: d1, value: *bytes.get(2)? & 0x7f }),
            0x90 => match *bytes.get(2)? & 0x7f {
                // Velocity 0 is the running-status idiom for note-off.
                0 => Some(Self::NoteOff { channel, key: d1 }),
                velocity => Some(Self::NoteOn { channel, key: d1, velocity }),
            },
            0xb0 => Some(Self::ControlChange { channel, cc: d1, value: *bytes.get(2)? & 0x7f }),
            0xc0 => Some(Self::ProgramChange { channel, program: d1 }),
            0xd0 => Some(Self::ChannelPressure { channel, value: d1 }),
            0xe0 => {
                let msb = u16::from(*bytes.get(2)? & 0x7f);
                Some(Self::PitchBend { channel, value: (msb << 7) | u16::from(d1) })
            }
            _ => None,
        }
    }

    /// The channel this message arrived on (0-15).
    pub fn channel(&self) -> u8 {
        match *self {
            Self::NoteOn { channel, .. }
            | Self::NoteOff { channel, .. }
            | Self::ControlChange { channel, .. }
            | Self::ProgramChange { channel, .. }
            | Self::PitchBend { channel, .. }
            | Self::ChannelPressure { channel, .. }
            | Self::PolyPressure { channel, .. } => channel,
        }
    }

    /// The mapping source this message writes and its normalized travel in
    /// `0..=1`, or `None` for note messages (which drive voices, not knobs).
    pub fn source(&self) -> Option<(u16, f32)> {
        match *self {
            Self::ControlChange { cc, value, .. } => {
                Some((u16::from(cc), f32::from(value) / 127.0))
            }
            Self::PitchBend { value, .. } => Some((SOURCE_PITCH_BEND, f32::from(value) / 16383.0)),
            Self::ChannelPressure { value, .. } => {
                Some((SOURCE_AFTERTOUCH, f32::from(value) / 127.0))
            }
            // These address voices, not knobs.
            Self::NoteOn { .. } | Self::NoteOff { .. } | Self::PolyPressure { .. } => None,
            // A program change names a sound, not a knob position.
            Self::ProgramChange { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_channel_voice_messages_we_map() {
        assert_eq!(
            MidiMessage::decode(&[0x90, 60, 100]),
            Some(MidiMessage::NoteOn { channel: 0, key: 60, velocity: 100 })
        );
        assert_eq!(
            MidiMessage::decode(&[0x93, 60, 0]),
            Some(MidiMessage::NoteOff { channel: 3, key: 60 }),
            "note-on with velocity 0 is a note-off"
        );
        assert_eq!(
            MidiMessage::decode(&[0x81, 62, 40]),
            Some(MidiMessage::NoteOff { channel: 1, key: 62 })
        );
        assert_eq!(
            MidiMessage::decode(&[0xb0, 74, 64]),
            Some(MidiMessage::ControlChange { channel: 0, cc: 74, value: 64 })
        );
        assert_eq!(
            MidiMessage::decode(&[0xe0, 0x00, 0x40]),
            Some(MidiMessage::PitchBend { channel: 0, value: 8192 })
        );
        assert_eq!(
            MidiMessage::decode(&[0xd2, 90]),
            Some(MidiMessage::ChannelPressure { channel: 2, value: 90 })
        );
        assert_eq!(
            MidiMessage::decode(&[0xc5, 0]),
            Some(MidiMessage::ProgramChange { channel: 5, program: 0 }),
            "program 0 is the first patch, not an absence"
        );
        assert_eq!(
            MidiMessage::decode(&[0xc0, 127]),
            Some(MidiMessage::ProgramChange { channel: 0, program: 127 })
        );
        // Poly aftertouch: per-note pressure, routed by the consumer's MPE layer.
        assert_eq!(
            MidiMessage::decode(&[0xa0, 60, 100]),
            Some(MidiMessage::PolyPressure { channel: 0, key: 60, value: 100 })
        );
    }

    #[test]
    fn rejects_what_it_does_not_model() {
        assert_eq!(MidiMessage::decode(&[]), None);
        assert_eq!(MidiMessage::decode(&[0x90]), None, "truncated");
        assert_eq!(MidiMessage::decode(&[0x40, 60, 100]), None, "running status");
        assert_eq!(MidiMessage::decode(&[0xf0, 1, 2]), None, "sysex");
        assert_eq!(MidiMessage::decode(&[0xc0]), None, "truncated program change");
    }

    #[test]
    fn sources_normalize_to_unit_travel() {
        let cc = MidiMessage::ControlChange { channel: 0, cc: 74, value: 127 };
        assert_eq!(cc.source(), Some((74, 1.0)));
        let bend = MidiMessage::PitchBend { channel: 0, value: 16383 };
        assert_eq!(bend.source(), Some((SOURCE_PITCH_BEND, 1.0)));
        let bend = MidiMessage::PitchBend { channel: 0, value: 0 };
        assert_eq!(bend.source(), Some((SOURCE_PITCH_BEND, 0.0)));
        let touch = MidiMessage::ChannelPressure { channel: 0, value: 0 };
        assert_eq!(touch.source(), Some((SOURCE_AFTERTOUCH, 0.0)));
        assert_eq!(MidiMessage::NoteOn { channel: 0, key: 60, velocity: 1 }.source(), None);
        // A program change names a sound, not a knob position.
        let program = MidiMessage::ProgramChange { channel: 4, program: 12 };
        assert_eq!(program.source(), None);
        assert_eq!(program.channel(), 4);
        // Poly aftertouch addresses voices, not knobs.
        let touch = MidiMessage::PolyPressure { channel: 0, key: 60, value: 100 };
        assert_eq!(touch.source(), None);
        assert_eq!(touch.channel(), 0);
    }

    #[test]
    fn source_names_round_trip_the_wire_spelling() {
        assert_eq!(source_name(74), "cc74");
        assert_eq!(source_name(SOURCE_PITCH_BEND), "pitch_bend");
        assert_eq!(source_name(SOURCE_AFTERTOUCH), "aftertouch");
    }
}
