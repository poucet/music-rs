//! High-level Composite Fugue structure and compilation engine.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

use crate::note::{deserialize_note_pitch, parse_note_name};
use crate::types::*;

/// A compact note specification that can be written either as an array:
/// `[beat, note, duration_beats, velocity?, channel?]`
/// or as an object:
/// `{ beat: 0.0, note: "C3", duration: 1.0, velocity: 100, channel: 0 }`
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct NoteItem {
    pub beat: f64,
    pub note: u8,
    pub duration: f64,
    #[serde(default = "default_velocity")]
    pub velocity: u8,
    #[serde(default)]
    pub channel: u8,
}

fn default_velocity() -> u8 {
    100
}

impl<'de> Deserialize<'de> for NoteItem {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum NoteItemHelper {
            // `Vec<Value>` and not `Value`: a bare `Value` matches a JSON object
            // too, so this arm used to swallow the object form below and then
            // fail inside it. Typed this way the two arms are disjoint by shape.
            Array(Vec<serde_json::Value>),
            Object {
                beat: f64,
                #[serde(deserialize_with = "deserialize_note_pitch")]
                note: u8,
                #[serde(default = "default_duration")]
                duration: f64,
                #[serde(default = "default_velocity")]
                velocity: u8,
                #[serde(default)]
                channel: u8,
            },
        }

        fn default_duration() -> f64 {
            1.0
        }

        match NoteItemHelper::deserialize(deserializer)? {
            NoteItemHelper::Object {
                beat,
                note,
                duration,
                velocity,
                channel,
            } => Ok(NoteItem {
                beat,
                note,
                duration,
                velocity,
                channel,
            }),
            NoteItemHelper::Array(arr) => {
                if arr.len() < 2 {
                    return Err(serde::de::Error::custom(
                        "note array must have at least 2 elements: [beat, note]",
                    ));
                }

                let beat = arr[0]
                    .as_f64()
                    .ok_or_else(|| serde::de::Error::custom("beat must be a number"))?;

                let note = match &arr[1] {
                    serde_json::Value::Number(n) => n
                        .as_u64()
                        .and_then(|num| if num <= 127 { Some(num as u8) } else { None })
                        .ok_or_else(|| serde::de::Error::custom("MIDI note must be 0..127"))?,
                    serde_json::Value::String(s) => {
                        parse_note_name(s).map_err(serde::de::Error::custom)?
                    }
                    _ => {
                        return Err(serde::de::Error::custom(
                            "note must be a MIDI integer or note string like 'C3'",
                        ));
                    }
                };

                let duration = if arr.len() >= 3 {
                    arr[2].as_f64().unwrap_or(1.0)
                } else {
                    1.0
                };

                let velocity = if arr.len() >= 4 {
                    arr[3].as_u64().unwrap_or(100) as u8
                } else {
                    100
                };

                let channel = if arr.len() >= 5 {
                    arr[4].as_u64().unwrap_or(0) as u8
                } else {
                    0
                };

                Ok(NoteItem {
                    beat,
                    note,
                    duration,
                    velocity,
                    channel,
                })
            }
        }
    }
}

/// The `[beat, value, curve?]` array form, which every automation point shares.
///
/// Only how `value` is read differs between the point kinds, so that is the
/// parameter; `beat` and the optional trailing curve are parsed identically
/// everywhere. An unrecognised curve falls back to the default rather than
/// failing the point — long-standing behaviour, kept.
///
/// The returned curve is `None` when the third element is **absent**, which is
/// what lets a per-note lane supply its own default for exactly those points.
fn parse_point_array<V, E>(
    arr: &[serde_json::Value],
    what: &str,
    value_expected: &str,
    read_value: impl FnOnce(&serde_json::Value) -> Option<V>,
) -> Result<(f64, V, Option<InterpolationMode>), E>
where
    E: serde::de::Error,
{
    if arr.len() < 2 {
        return Err(E::custom(format!("{what} array must have at least [beat, value]")));
    }
    let beat = arr[0]
        .as_f64()
        .ok_or_else(|| E::custom("beat must be a number"))?;
    let value = read_value(&arr[1])
        .ok_or_else(|| E::custom(format!("value must be {value_expected}")))?;
    let curve = arr
        .get(2)
        .map(|c| serde_json::from_value(c.clone()).unwrap_or_default());
    Ok((beat, value, curve))
}

/// A Control Change automation point: `[beat, value, curve?]` or object `{ beat, value, curve }`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct CcPointItem {
    pub beat: f64,
    pub value: u8,
    #[serde(default)]
    pub curve: InterpolationMode,
}

impl<'de> Deserialize<'de> for CcPointItem {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum CcPointHelper {
            // `Vec<Value>` and not `Value`: a bare `Value` matches a JSON object
            // too, so this arm used to swallow the object form below and then
            // fail inside it. Typed this way the two arms are disjoint by shape.
            Array(Vec<serde_json::Value>),
            Object {
                beat: f64,
                value: u8,
                #[serde(default)]
                curve: InterpolationMode,
            },
        }

        match CcPointHelper::deserialize(deserializer)? {
            CcPointHelper::Object { beat, value, curve } => Ok(CcPointItem { beat, value, curve }),
            CcPointHelper::Array(arr) => {
                let (beat, value, curve) = parse_point_array(&arr, "CC point", "0..127", |v| {
                    v.as_u64().map(|n| n as u8)
                })?;
                Ok(CcPointItem { beat, value, curve: curve.unwrap_or_default() })
            }
        }
    }
}

/// A Parameter automation point: `[beat, value, curve?]` or object `{ beat, value, curve }`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ParamPointItem {
    pub beat: f64,
    pub value: f32,
    #[serde(default)]
    pub curve: InterpolationMode,
}

impl<'de> Deserialize<'de> for ParamPointItem {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum ParamPointHelper {
            Array(Vec<serde_json::Value>),
            Object {
                beat: f64,
                value: f32,
                #[serde(default)]
                curve: InterpolationMode,
            },
        }

        match ParamPointHelper::deserialize(deserializer)? {
            ParamPointHelper::Object { beat, value, curve } => {
                Ok(ParamPointItem { beat, value, curve })
            }
            ParamPointHelper::Array(arr) => {
                let (beat, value, curve) =
                    parse_point_array(&arr, "param point", "a number", |v| {
                        v.as_f64().map(|n| n as f32)
                    })?;
                Ok(ParamPointItem { beat, value, curve: curve.unwrap_or_default() })
            }
        }
    }
}

/// A per-note expression point: `[beat, value, curve?]` or object
/// `{ beat, value, curve }`.
///
/// The one thing that separates it from [`ParamPointItem`] is that its curve is
/// **optional rather than defaulted**, so a point that says nothing about its
/// curve is distinguishable from one that says `"none"`. That distinction is
/// the whole point: an absent curve inherits [`PerNoteLane::interpolation`],
/// which is Droplets' semantics for the shape this was taken from.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema, Default)]
pub struct PerNotePoint {
    pub beat: f64,
    pub value: f32,
    /// Absent means "use the lane's `interpolation`".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curve: Option<InterpolationMode>,
}

impl<'de> Deserialize<'de> for PerNotePoint {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum PerNotePointHelper {
            Array(Vec<serde_json::Value>),
            Object {
                beat: f64,
                value: f32,
                #[serde(default)]
                curve: Option<InterpolationMode>,
            },
        }

        match PerNotePointHelper::deserialize(deserializer)? {
            PerNotePointHelper::Object { beat, value, curve } => {
                Ok(PerNotePoint { beat, value, curve })
            }
            PerNotePointHelper::Array(arr) => {
                let (beat, value, curve) =
                    parse_point_array(&arr, "per-note point", "a number", |v| {
                        v.as_f64().map(|n| n as f32)
                    })?;
                Ok(PerNotePoint { beat, value, curve })
            }
        }
    }
}

/// One note's worth of per-note expression: a lane of `[beat, value, curve?]`
/// points aimed at a single `(channel, note)`.
///
/// The same shape carries pitch bend (points are semitones) and pressure
/// (points are 0.0..=1.0) — the two differ only in what the value means, so
/// they share one type. `note` accepts a MIDI number or a name (`"C3"`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
pub struct PerNoteLane {
    /// The note this lane bends or presses. `60` or `"C3"`.
    #[serde(deserialize_with = "deserialize_note_pitch")]
    pub note: u8,
    #[serde(default)]
    pub channel: u8,
    /// `[beat, value, curve?]` points. A point that omits its curve takes the
    /// lane's `interpolation` below.
    #[serde(default)]
    pub points: Vec<PerNotePoint>,
    /// The lane's curve, used for every point that does not name one of its own.
    ///
    /// This is lane-level only, and deliberately so: a *fugue*-level curve
    /// default is a third precedence level and is rejected (FUGUE-API.md §2.3).
    /// `ccs` and `params` have no equivalent and are not getting one.
    #[serde(default)]
    pub interpolation: InterpolationMode,
}

/// High-level, developer-friendly Fugue definition.
///
/// Can contain notes, CC automation tracks, parameter automation tracks,
/// per-note expression lanes, or raw Fugue event lists.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
pub struct CompositeFugue {
    /// List of notes (either compact `[beat, "C3", dur, vel]` or full objects).
    #[serde(default)]
    pub notes: Vec<NoteItem>,

    /// CC automation points keyed by CC number (0..=127).
    #[serde(default)]
    pub ccs: HashMap<u8, Vec<CcPointItem>>,

    /// Parameter automation points keyed by parameter ID.
    #[serde(default)]
    pub params: HashMap<u32, Vec<ParamPointItem>>,

    /// Raw timed events (if already compiled or explicitly scheduled).
    #[serde(default)]
    pub raw_events: Vec<TimedFugueEvent>,

    /// Overall performance duration in beats (auto-calculated from notes if omitted).
    #[serde(default)]
    pub duration_beats: Option<f64>,

    #[serde(default)]
    pub loop_mode: LoopMode,

    // --- added by the superset ----------------------------------------------
    // Every one is defaulted and skipped when unused, so a fugue that predates
    // them parses to the same value and serializes to the same bytes.
    /// Grouping / cancellation label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,

    /// What this fugue cancels when it starts — the axis [`CancelMode`] is not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel: Option<CancelTarget>,

    /// Per-note pitch-bend lanes, one per target note. Values are semitones.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pitch_bends: Vec<PerNoteLane>,

    /// Per-note pressure lanes, one per target note. Values are 0.0..=1.0.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pressures: Vec<PerNoteLane>,
}

/// Compiled performance sequence ready for offline rendering or transport playback.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CompiledFugue {
    pub duration_beats: f64,
    pub events: Vec<TimedFugueEvent>,
    pub loop_mode: LoopMode,
}

/// Emit one timed event per point across a set of per-note lanes, growing
/// `max_beat` to cover them.
///
/// Pitch bend and pressure differ only in which [`FugueEvent`] a point becomes,
/// so they share this loop; `make` is the only thing that varies.
fn push_per_note_lane(
    events: &mut Vec<TimedFugueEvent>,
    max_beat: &mut f64,
    lanes: &[PerNoteLane],
    make: fn(u8, u8, f32, InterpolationMode) -> FugueEvent,
) {
    for lane in lanes {
        for pt in &lane.points {
            // The point's own curve wins; a point that names none takes the
            // lane's. Two levels, no more.
            let curve = pt.curve.unwrap_or(lane.interpolation);
            events.push(TimedFugueEvent {
                beat_offset: pt.beat,
                event: make(lane.channel, lane.note, pt.value, curve),
            });
            if pt.beat > *max_beat {
                *max_beat = pt.beat;
            }
        }
    }
}

impl CompositeFugue {
    /// Compile the high-level representation into a linear, sorted list of timed events.
    pub fn compile(&self) -> CompiledFugue {
        let mut events = self.raw_events.clone();
        let mut max_beat = self.duration_beats.unwrap_or(0.0);

        for n in &self.notes {
            events.push(TimedFugueEvent {
                beat_offset: n.beat,
                event: FugueEvent::TimedNote {
                    channel: n.channel,
                    note: n.note,
                    velocity: n.velocity,
                    duration_beats: n.duration,
                },
            });
            let end_beat = n.beat + n.duration;
            if end_beat > max_beat {
                max_beat = end_beat;
            }
        }

        for (&controller, points) in &self.ccs {
            for pt in points {
                events.push(TimedFugueEvent {
                    beat_offset: pt.beat,
                    event: FugueEvent::Cc {
                        channel: 0,
                        controller,
                        value: pt.value,
                        interpolation: pt.curve,
                    },
                });
                if pt.beat > max_beat {
                    max_beat = pt.beat;
                }
            }
        }

        for (&id, points) in &self.params {
            for pt in points {
                events.push(TimedFugueEvent {
                    beat_offset: pt.beat,
                    event: FugueEvent::Param {
                        id,
                        value: pt.value,
                        interpolation: pt.curve,
                    },
                });
                if pt.beat > max_beat {
                    max_beat = pt.beat;
                }
            }
        }

        // Per-note expression: one event per point, sparse. Nothing renders
        // these yet on either host; a renderer that does interpolates between
        // consecutive points itself, exactly as it does for a param lane.
        push_per_note_lane(&mut events, &mut max_beat, &self.pitch_bends, |channel, note, value, interpolation| {
            FugueEvent::PerNotePitchBend { channel, note, semitones: value, interpolation }
        });
        push_per_note_lane(&mut events, &mut max_beat, &self.pressures, |channel, note, value, interpolation| {
            FugueEvent::PerNotePressure { channel, note, pressure: value, interpolation }
        });

        // Sort events chronologically
        events.sort_by(|a, b| {
            a.beat_offset
                .partial_cmp(&b.beat_offset)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        CompiledFugue {
            duration_beats: max_beat.max(1.0),
            events,
            loop_mode: self.loop_mode,
        }
    }
}
