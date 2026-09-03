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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum LoopMode {
    #[default]
    Once,
    Loop,
    PingPong,
}

/// Quantization mode for starting a Fugue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum QuantizeMode {
    #[default]
    None,
    Bar,
    Beat,
    Eighth,
    Sixteenth,
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
