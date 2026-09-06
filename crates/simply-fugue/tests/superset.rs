//! What the superset added, tested from the wire in.
//!
//! The compatibility direction — that nothing which parsed before stopped — is
//! `wire_compat.rs`. This file is the other direction: that the fields added
//! for Flux and for Droplets-shaped JSON actually work.

use std::collections::HashMap;

use simply_fugue::{
    CancelTarget, CcTarget, CompositeFugue, FugueEvent, InterpolationMode, LoopMode, PerNoteLane,
    PerNotePoint, QuantizeMode, TimedFugueEvent,
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

/// Curve resolution has exactly two levels: **a point's own curve wins; a point
/// that names none takes the lane's `interpolation`.** Droplets' semantics for
/// the shape this was taken from.
///
/// Both branches, in one lane, so the precedence is pinned and not just the
/// happy path. Note the third point says `"none"` explicitly — that is a
/// *choice of stepped*, not an absence, and it must survive the lane's default.
#[test]
fn a_lanes_interpolation_fills_in_only_for_points_without_a_curve() {
    let json = r#"{"pitch_bends":[
        {"note":60,"interpolation":"linear","points":[[0,0],[1,1,"exp"],[2,0,"none"]]}
    ]}"#;
    let fugue: CompositeFugue = serde_json::from_str(json).unwrap();
    assert_eq!(fugue.pitch_bends[0].interpolation, InterpolationMode::Linear);
    assert_eq!(
        fugue.pitch_bends[0].points.iter().map(|p| p.curve).collect::<Vec<_>>(),
        vec![None, Some(InterpolationMode::Exp), Some(InterpolationMode::None)],
        "an absent curve must parse as None, distinct from an explicit \"none\""
    );

    let curves: Vec<_> = fugue
        .compile()
        .events
        .iter()
        .filter_map(|e| match e.event {
            FugueEvent::PerNotePitchBend { interpolation, .. } => Some(interpolation),
            _ => None,
        })
        .collect();
    assert_eq!(
        curves,
        vec![
            InterpolationMode::Linear, // inherited from the lane
            InterpolationMode::Exp,    // the point's own
            InterpolationMode::None,   // explicitly stepped, not inherited
        ]
    );
}

/// With no lane `interpolation` given, the lane default is `None` — so a
/// curve-less per-note lane is stepped, exactly as a curve-less CC lane is.
#[test]
fn a_lane_without_an_interpolation_leaves_its_points_stepped() {
    let fugue: CompositeFugue =
        serde_json::from_str(r#"{"pressures":[{"note":60,"points":[[0,0],[1,1]]}]}"#).unwrap();
    assert_eq!(fugue.pressures[0].interpolation, InterpolationMode::None);
    let curves: Vec<_> = fugue
        .compile()
        .events
        .iter()
        .filter_map(|e| match e.event {
            FugueEvent::PerNotePressure { interpolation, .. } => Some(interpolation),
            _ => None,
        })
        .collect();
    assert_eq!(curves, vec![InterpolationMode::None, InterpolationMode::None]);
}

/// The lane-level default is **lane**-level. `ccs` and `params` have no
/// equivalent and are not getting one — a fugue-level curve default is a third
/// precedence level, which FUGUE-API.md §2.3 rejects. Pinned so that stays true.
#[test]
fn cc_and_param_lanes_have_no_lane_level_curve() {
    let fugue: CompositeFugue = serde_json::from_str(
        r#"{"ccs":{"74":[[0,0],[1,127,"exp"]]},"params":{"7":[[0,0.0],[1,1.0,"linear"]]}}"#,
    )
    .unwrap();
    assert_eq!(fugue.ccs[&CcTarget::from(74)][0].curve, InterpolationMode::None);
    assert_eq!(fugue.ccs[&CcTarget::from(74)][1].curve, InterpolationMode::Exp);
    assert_eq!(fugue.params[&7][0].curve, InterpolationMode::None);
    assert_eq!(fugue.params[&7][1].curve, InterpolationMode::Linear);
}

// --- the round trip ---------------------------------------------------------

/// **Serialize → deserialize, over every superset field** — `tag`, `cancel`,
/// `LoopMode::Times`, both per-note events, and both per-note lanes — alongside
/// the fields that were always there, so the whole type round trips.
#[test]
fn every_superset_field_round_trips() {
    let mut ccs = HashMap::new();
    ccs.insert(CcTarget::from(74), vec![serde_json::from_str("[0, 30, \"linear\"]").unwrap()]);
    let mut params = HashMap::new();
    params.insert(7u32, vec![serde_json::from_str("[1, 0.5]").unwrap()]);

    let fugue = CompositeFugue {
        tag: Some("melody".into()),
        cancel: Some(CancelTarget::Tag("bass".into())),
        loop_mode: LoopMode::Times(4),
        duration_beats: Some(8.0),
        notes: vec![serde_json::from_str("[0, \"C3\", 2.0, 100, 1]").unwrap()],
        ccs,
        params,
        pitch_bends: vec![PerNoteLane {
            note: 60,
            channel: 1,
            interpolation: InterpolationMode::Linear,
            points: vec![
                PerNotePoint { beat: 0.0, value: 0.0, curve: None },
                PerNotePoint { beat: 1.0, value: -2.0, curve: Some(InterpolationMode::Exp) },
            ],
        }],
        pressures: vec![PerNoteLane {
            note: 64,
            points: vec![PerNotePoint { beat: 0.5, value: 0.75, curve: None }],
            ..Default::default()
        }],
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
        // No `..Default::default()`: every field is named above, and clippy
        // enforces that it stays that way. A field added to `CompositeFugue`
        // therefore breaks this test until it is round-tripped too.
    };

    let json = serde_json::to_string(&fugue).unwrap();
    let back: CompositeFugue = serde_json::from_str(&json).unwrap();
    assert_eq!(back, fugue);

    for key in ["tag", "cancel", "pitch_bends", "pressures"] {
        assert!(json.contains(key), "{key} missing from {json}");
    }
}

/// A point that inherited its curve keeps *inheriting* it across a round trip —
/// the absence is preserved on the wire rather than being baked into the point.
/// Otherwise a fugue would quietly change meaning every time it was re-saved.
#[test]
fn an_inherited_curve_survives_a_round_trip_as_an_absence() {
    let fugue = CompositeFugue {
        pitch_bends: vec![PerNoteLane {
            note: 60,
            interpolation: InterpolationMode::Exp,
            points: vec![PerNotePoint { beat: 0.0, value: 1.0, curve: None }],
            ..Default::default()
        }],
        ..Default::default()
    };

    let json = serde_json::to_string(&fugue).unwrap();
    assert!(!json.contains("curve"), "an absent curve must not be written: {json}");

    let back: CompositeFugue = serde_json::from_str(&json).unwrap();
    assert_eq!(back, fugue);
    assert_eq!(back.pitch_bends[0].points[0].curve, None);

    // …and it still resolves to the lane's curve after the trip.
    let curves: Vec<_> = back
        .compile()
        .events
        .iter()
        .filter_map(|e| match e.event {
            FugueEvent::PerNotePitchBend { interpolation, .. } => Some(interpolation),
            _ => None,
        })
        .collect();
    assert_eq!(curves, vec![InterpolationMode::Exp]);
}

/// Both point forms are accepted for a per-note lane, and they mean the same
/// thing — including the object form, which is what `Serialize` emits.
#[test]
fn per_note_points_accept_both_the_array_and_object_forms() {
    let array: CompositeFugue =
        serde_json::from_str(r#"{"pressures":[{"note":60,"points":[[1.5,0.25,"exp"]]}]}"#).unwrap();
    let object: CompositeFugue = serde_json::from_str(
        r#"{"pressures":[{"note":60,"points":[{"beat":1.5,"value":0.25,"curve":"exp"}]}]}"#,
    )
    .unwrap();
    assert_eq!(array, object);
    assert_eq!(
        array.pressures[0].points[0],
        PerNotePoint { beat: 1.5, value: 0.25, curve: Some(InterpolationMode::Exp) }
    );
}
