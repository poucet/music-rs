//! What the superset added, tested from the wire in.
//!
//! The compatibility direction — that nothing which parsed before stopped — is
//! `wire_compat.rs`. This file is the other direction: that the fields added
//! for Flux and for Droplets-shaped JSON actually work.

use simply_fugue::{
    CancelTarget, CompositeFugue, FugueEvent, InterpolationMode, LoopMode, PerNoteLane,
    QuantizeMode, TimedFugueEvent,
};

// --- the two new mode variants, and how they are spelled on the wire ---------

#[test]
fn times_and_bars_use_the_serde_derive_wire_form() {
    let fugue: CompositeFugue = serde_json::from_str(r#"{"loop_mode":{"times":4}}"#).unwrap();
    assert_eq!(fugue.loop_mode, LoopMode::Times(4));
    assert_eq!(
        serde_json::to_value(&fugue).unwrap()["loop_mode"],
        serde_json::json!({ "times": 4 })
    );

    let header: simply_fugue::FugueHeader =
        serde_json::from_str(r#"{"duration_beats":8.0,"quantize":{"bars":4}}"#).unwrap();
    assert_eq!(header.quantize, QuantizeMode::Bars(4));
    assert_eq!(
        serde_json::to_value(&header).unwrap()["quantize"],
        serde_json::json!({ "bars": 4 })
    );
}

/// Droplets writes `Times` as the bare string `"4"`. That is a hand-written
/// deserializer, not a derive, and it is deliberately **not** built here — so
/// state what happens instead of leaving it to be discovered.
#[test]
fn droplets_bare_string_times_is_not_accepted() {
    assert!(serde_json::from_str::<CompositeFugue>(r#"{"loop_mode":"4"}"#).is_err());
}

// --- cancel target ----------------------------------------------------------

#[test]
fn cancel_target_has_three_spellings() {
    let cases = [
        (r#""none""#, CancelTarget::None),
        (r#"{"tag":"melody"}"#, CancelTarget::Tag("melody".into())),
        (r#""all""#, CancelTarget::All),
    ];
    for (json, want) in cases {
        let got: CancelTarget = serde_json::from_str(json).unwrap();
        assert_eq!(got, want, "parsing {json}");
        assert_eq!(serde_json::to_string(&got).unwrap(), json, "re-emitting {want:?}");
    }
}

// --- per-note expression ----------------------------------------------------

/// A Droplets-shaped lane: a note *name*, and terse `[beat, value, curve?]`
/// points. It compiles to one sparse event per point, in beat order, each
/// carrying its own curve — a renderer interpolates between them, exactly as it
/// does for a param lane.
#[test]
fn a_droplets_shaped_lane_compiles_to_sparse_events_in_beat_order() {
    let json = r#"{
        "pitch_bends": [
            { "note": "C3", "points": [[0, 0], [0.5, 1, "exp"], [1, -1]] }
        ],
        "pressures": [
            { "note": 64, "channel": 2, "points": [[0.25, 0.5, "linear"]] }
        ]
    }"#;
    let fugue: CompositeFugue = serde_json::from_str(json).unwrap();

    assert_eq!(fugue.pitch_bends[0].note, 60, "\"C3\" is MIDI 60");
    assert_eq!(fugue.pitch_bends[0].channel, 0, "channel defaults to 0");

    let compiled = fugue.compile();
    let bends: Vec<_> = compiled
        .events
        .iter()
        .filter_map(|e| match e.event {
            FugueEvent::PerNotePitchBend { note, semitones, interpolation, .. } => {
                Some((e.beat_offset, note, semitones, interpolation))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        bends,
        vec![
            (0.0, 60, 0.0, InterpolationMode::None),
            (0.5, 60, 1.0, InterpolationMode::Exp),
            (1.0, 60, -1.0, InterpolationMode::None),
        ],
        "one sparse event per point, in beat order, each with its own curve"
    );

    let pressures: Vec<_> = compiled
        .events
        .iter()
        .filter_map(|e| match e.event {
            FugueEvent::PerNotePressure { channel, note, pressure, interpolation } => {
                Some((e.beat_offset, channel, note, pressure, interpolation))
            }
            _ => None,
        })
        .collect();
    assert_eq!(pressures, vec![(0.25, 2, 64, 0.5, InterpolationMode::Linear)]);

    // The lanes extend the compiled duration the same way notes and CCs do.
    assert_eq!(compiled.duration_beats, 1.0);
}

/// The lanes are the only content, so they alone must set the length — and an
/// explicit `duration_beats` still wins where it is longer.
#[test]
fn lanes_extend_duration_without_shortening_it() {
    let long = CompositeFugue {
        duration_beats: Some(16.0),
        pressures: vec![PerNoteLane {
            note: 60,
            points: vec![serde_json::from_str("[2.0, 0.75]").unwrap()],
            ..Default::default()
        }],
        ..Default::default()
    };
    assert_eq!(long.compile().duration_beats, 16.0);
}

/// Droplets' per-note events have no `interpolation` field. Ours defaults, so
/// JSON written against that shape still parses.
#[test]
fn per_note_events_parse_without_an_interpolation_field() {
    let json = r#"{"raw_events":[
        {"beat_offset":0.0,"event":{"type":"per_note_pitch_bend","channel":0,"note":60,"semitones":2.0}},
        {"beat_offset":1.0,"event":{"type":"per_note_pressure","channel":0,"note":60,"pressure":0.5}}
    ]}"#;
    let fugue: CompositeFugue = serde_json::from_str(json).unwrap();
    assert_eq!(
        fugue.raw_events[0].event,
        FugueEvent::PerNotePitchBend {
            channel: 0,
            note: 60,
            semitones: 2.0,
            interpolation: InterpolationMode::None,
        }
    );
    assert_eq!(
        fugue.raw_events[1].event,
        FugueEvent::PerNotePressure {
            channel: 0,
            note: 60,
            pressure: 0.5,
            interpolation: InterpolationMode::None,
        }
    );
}

/// The lane's own `interpolation` is carried but is deliberately not folded
/// into its points — see [`PerNoteLane::interpolation`]. Pinned, because doing
/// it silently would be a second curve-precedence level nobody has ruled on.
#[test]
fn a_lanes_own_interpolation_does_not_override_its_points() {
    let json = r#"{"pitch_bends":[
        {"note":60,"interpolation":"linear","points":[[0,0],[1,1,"exp"]]}
    ]}"#;
    let fugue: CompositeFugue = serde_json::from_str(json).unwrap();
    assert_eq!(fugue.pitch_bends[0].interpolation, InterpolationMode::Linear);

    let curves: Vec<_> = fugue
        .compile()
        .events
        .iter()
        .filter_map(|e| match e.event {
            FugueEvent::PerNotePitchBend { interpolation, .. } => Some(interpolation),
            _ => None,
        })
        .collect();
    assert_eq!(curves, vec![InterpolationMode::None, InterpolationMode::Exp]);
}

// --- the round trip ---------------------------------------------------------

/// **Serialize → deserialize, over every superset field**: `tag`, `cancel`,
/// `LoopMode::Times`, and both per-note events.
///
/// `pitch_bends` / `pressures` are absent here for a reason, and it is not
/// laziness — see the test below.
#[test]
fn every_superset_field_round_trips() {
    let fugue = CompositeFugue {
        tag: Some("melody".into()),
        cancel: Some(CancelTarget::Tag("bass".into())),
        loop_mode: LoopMode::Times(4),
        duration_beats: Some(8.0),
        raw_events: vec![
            TimedFugueEvent {
                beat_offset: 0.0,
                event: FugueEvent::PerNotePitchBend {
                    channel: 1,
                    note: 60,
                    semitones: -2.5,
                    interpolation: InterpolationMode::Exp,
                },
            },
            TimedFugueEvent {
                beat_offset: 2.0,
                event: FugueEvent::PerNotePressure {
                    channel: 1,
                    note: 60,
                    pressure: 0.875,
                    interpolation: InterpolationMode::Linear,
                },
            },
            TimedFugueEvent {
                beat_offset: 4.0,
                event: FugueEvent::Cc {
                    channel: 0,
                    controller: 74,
                    value: 30,
                    interpolation: InterpolationMode::Log,
                },
            },
        ],
        ..Default::default()
    };

    let json = serde_json::to_string(&fugue).unwrap();
    let back: CompositeFugue = serde_json::from_str(&json).unwrap();
    assert_eq!(back, fugue);

    for key in ["tag", "cancel"] {
        assert!(json.contains(key), "{key} missing from {json}");
    }
}

/// **The per-note lanes cannot round trip, and the cause is inherited.**
///
/// `PerNoteLane.points` is `Vec<ParamPointItem>` — reuse, because a per-note
/// point is a param point in every respect but what the number means. It
/// therefore inherits `ParamPointItem`'s deserializer, whose `#[serde(untagged)]`
/// helper puts `Array(serde_json::Value)` first and so swallows the object form
/// that `Serialize` emits. `wire_compat.rs` documents the defect in full.
///
/// Parsing — the direction the wire contract is about — is unaffected: every
/// test above feeds the array form and passes. **Delete this test with the fix.**
#[test]
fn known_defect_per_note_lanes_do_not_round_trip() {
    let fugue = CompositeFugue {
        pitch_bends: vec![PerNoteLane {
            note: 60,
            points: vec![serde_json::from_str("[0.0, 1.0]").unwrap()],
            ..Default::default()
        }],
        ..Default::default()
    };
    let json = serde_json::to_string(&fugue).unwrap();
    assert!(
        serde_json::from_str::<CompositeFugue>(&json).is_err(),
        "lanes round trip now — good; delete this test and its issue row"
    );
}
