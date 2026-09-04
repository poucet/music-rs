//! Core Fugue musical definitions, events, headers, and modes.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Interpolation curve for automation points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum InterpolationMode {
    #[default]
    None,
    Linear,
    Exp,
    Log,
}

/// Loop mode for a Fugue stream.
///
/// `"forever"` is accepted as a spelling of [`LoopMode::Loop`], so a fugue
/// written against Droplets' vocabulary parses here too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum LoopMode {
    #[default]
    Once,
    #[serde(alias = "forever")]
    Loop,
    PingPong,
    /// Repeat a fixed number of times: `{"times": 4}`.
    Times(u32),
}

/// Quantization mode for starting a Fugue.
///
/// `"immediate"` is accepted as a spelling of [`QuantizeMode::None`] — both
/// mean "do not wait for a grid line".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum QuantizeMode {
    #[default]
    #[serde(alias = "immediate")]
    None,
    Bar,
    Beat,
    Eighth,
    Sixteenth,
    /// Align to a multiple of N bars: `{"bars": 4}`.
    Bars(u32),
}

/// Cancel mode when stopping a Fugue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum CancelMode {
    #[default]
    Immediate,
    End,
    Quantized,
}

/// Start mode for a queued Fugue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum StartMode {
    #[default]
    Immediate,
    NextBeat,
    NextBar,
}

/// *What* a starting Fugue cancels — a different axis from [`CancelMode`],
/// which says *when* a Fugue stops.
///
/// `"none"` · `{"tag": "melody"}` · `"all"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum CancelTarget {
    /// Cancel nothing; play alongside whatever is running.
    #[default]
    None,
    /// Cancel every running Fugue carrying this tag.
    Tag(String),
    /// Cancel everything currently running.
    All,
}

/// One event in a Fugue timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FugueEvent {
    /// Note On event.
    NoteOn {
        channel: u8,
        note: u8,
        velocity: u8,
    },
    /// Note Off event.
    NoteOff {
        channel: u8,
        note: u8,
        velocity: u8,
    },
    /// Timed Note (duration in beats).
    TimedNote {
        channel: u8,
        note: u8,
        velocity: u8,
        duration_beats: f64,
    },
    /// MIDI Control Change event.
    Cc {
        channel: u8,
        /// The CC number. Also accepted spelled `cc`, which is how Droplets
        /// writes it.
        #[serde(alias = "cc")]
        controller: u8,
        value: u8,
        #[serde(default)]
        interpolation: InterpolationMode,
    },
    /// Plugin / Slot parameter change event.
    Param {
        id: u32,
        value: f32,
        #[serde(default)]
        interpolation: InterpolationMode,
    },
    /// Pitch Bend event (-8192..=8191).
    PitchBend {
        channel: u8,
        value: i16,
    },
    /// Channel Pressure / Aftertouch (0..=127).
    Pressure {
        channel: u8,
        value: u8,
    },
    /// Polyphonic Per-Note Aftertouch (0..=127).
    PolyPressure {
        channel: u8,
        note: u8,
        value: u8,
    },
    /// Per-note pitch bend, in semitones, addressed by `(channel, note)`.
    ///
    /// The MIDI-2 / MPE shape, as distinct from channel-wide [`FugueEvent::PitchBend`].
    /// No host renders this yet — see the crate docs.
    PerNotePitchBend {
        channel: u8,
        note: u8,
        semitones: f32,
        #[serde(default)]
        interpolation: InterpolationMode,
    },
    /// Per-note pressure (0.0..=1.0), addressed by `(channel, note)`.
    ///
    /// The continuous counterpart of [`FugueEvent::PolyPressure`]'s 0..=127 byte.
    /// No host renders this yet — see the crate docs.
    PerNotePressure {
        channel: u8,
        note: u8,
        pressure: f32,
        #[serde(default)]
        interpolation: InterpolationMode,
    },
}

impl FugueEvent {
    /// Whether this event *starts* a note — a [`FugueEvent::NoteOn`], or the on
    /// edge of a [`FugueEvent::TimedNote`].
    ///
    /// This is the tie-break every consumer sorts on, which is why it lives on
    /// the event and not in one host's renderer. At a single instant a note-off
    /// must come **before** a note-on, or a same-pitch retrigger at a note
    /// boundary is an on followed by the previous note's off and the key goes
    /// silent. `false < true`, so `(beat, is_note_on)` is the whole rule and
    /// everything that is neither — a CC, a param, an expression point — keeps
    /// its place ahead of the note it belongs to.
    pub fn is_note_on(&self) -> bool {
        matches!(self, FugueEvent::NoteOn { .. } | FugueEvent::TimedNote { .. })
    }
}

/// An event placed at a specific beat offset within a Fugue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TimedFugueEvent {
    /// Offset in beats from the start of the Fugue.
    pub beat_offset: f64,
    /// The event to trigger.
    pub event: FugueEvent,
}

/// Header specifying playback properties of a Fugue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
pub struct FugueHeader {
    /// Duration of the fugue pattern in beats.
    pub duration_beats: f64,
    #[serde(default)]
    pub loop_mode: LoopMode,
    #[serde(default)]
    pub quantize: QuantizeMode,
    #[serde(default)]
    pub cancel_mode: CancelMode,
    #[serde(default)]
    pub start_mode: StartMode,
    /// Optional human-readable tag / name for this pattern.
    #[serde(default)]
    pub tag: Option<String>,
}

/// Full specification of a single Fugue timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
pub struct FugueDefinition {
    pub header: FugueHeader,
    pub events: Vec<TimedFugueEvent>,
}
