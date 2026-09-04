//! **`compile()` puts a note-off before a note-on at the same beat.**
//!
//! Sorting by `beat_offset` alone left ties to stable-sort insertion order —
//! `raw_events`, then notes, then ccs, then params — so a same-pitch retrigger
//! at a note boundary compiled as *on, then off*. A host that emits the
//! compiled order as MIDI bytes (nexus does) therefore started the second note
//! and immediately killed it with the first note's off: one audible note where
//! the fugue says two. Flux never saw it, because `NoteEdge::order()` re-sorts
//! whatever it is handed (@flux docs/design/SEQUENCER.md §5.4).
//!
//! Filed as `I-54` in flux's issues, against the crate rather than either host
//! because the crate is where one answer lives.

use simply_fugue::{CompositeFugue, FugueEvent, InterpolationMode, TimedFugueEvent};

/// `(is note-on, note)` for every note edge in compiled order.
fn note_edges(fugue: &CompositeFugue) -> Vec<(bool, u8)> {
    fugue
        .compile()
        .events
        .iter()
        .filter_map(|e| match e.event {
            FugueEvent::NoteOn { note, .. } | FugueEvent::TimedNote { note, .. } => {
                Some((true, note))
            }
            FugueEvent::NoteOff { note, .. } => Some((false, note)),
            _ => None,
        })
        .collect()
}

/// The retrigger, in the raw-event form that shows the ordering directly: an
/// off and an on for the same pitch at beat 2, written on
/// (which is how an agent would type them, and how the old sort left them).
#[test]
fn a_same_pitch_retrigger_compiles_off_before_on() {
    let fugue: CompositeFugue = serde_json::from_str(
        r#"{
            "raw_events": [
                {"beat_offset": 0.0, "event": {"type": "note_on",  "channel": 0, "note": 60, "velocity": 100}},
                {"beat_offset": 2.0, "event": {"type": "note_on",  "channel": 0, "note": 60, "velocity": 100}},
                {"beat_offset": 2.0, "event": {"type": "note_off", "channel": 0, "note": 60, "velocity": 0}}
            ]
        }"#,
    )
    .unwrap();

    assert_eq!(
        note_edges(&fugue),
        vec![(true, 60), (false, 60), (true, 60)],
        "the off at beat 2 must precede the on at beat 2"
    );
}

/// The same case in the form an agent actually writes: two abutting `notes`,
/// where the first note's off lands on the second note's beat. `TimedNote`
/// counts as an on, so it sorts after the off it collides with.
#[test]
fn abutting_notes_of_one_pitch_keep_the_off_first() {
    let fugue: CompositeFugue = serde_json::from_str(
        r#"{"notes": [[0, "C3", 1.0, 100], [1, "C3", 1.0, 100]],
            "raw_events": [
                {"beat_offset": 1.0, "event": {"type": "note_off", "channel": 0, "note": 60, "velocity": 0}}
            ]}"#,
    )
    .unwrap();

    // beat 0: on. beat 1: the explicit off, then the second note's on.
    assert_eq!(note_edges(&fugue), vec![(true, 60), (false, 60), (true, 60)]);
}

/// Everything that is not a note-on keeps its place *ahead* of one at the same
/// beat — a CC or a param that sets up a note lands before the note starts,
/// which is the order a player means.
#[test]
fn controls_at_a_beat_precede_the_note_on_at_that_beat() {
    let fugue: CompositeFugue = serde_json::from_str(
        r#"{"notes": [[0, "C3", 1.0, 100]], "ccs": {"74": [[0.0, 30]]}}"#,
    )
    .unwrap();

    let kinds: Vec<&'static str> = fugue
        .compile()
        .events
        .iter()
        .map(|e| match e.event {
            FugueEvent::Cc { .. } => "cc",
            FugueEvent::TimedNote { .. } => "note",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, vec!["cc", "note"]);
}

/// The predicate itself, since both hosts sort on it.
#[test]
fn is_note_on_is_true_for_exactly_the_two_note_starting_variants() {
    let on = FugueEvent::NoteOn { channel: 0, note: 60, velocity: 100 };
    let timed = FugueEvent::TimedNote { channel: 0, note: 60, velocity: 100, duration_beats: 1.0 };
    let off = FugueEvent::NoteOff { channel: 0, note: 60, velocity: 0 };
    let cc = FugueEvent::Cc {
        channel: 0,
        controller: 74,
        value: 30,
        interpolation: InterpolationMode::None,
    };

    assert!(on.is_note_on() && timed.is_note_on());
    assert!(!off.is_note_on() && !cc.is_note_on());
}

/// Order is otherwise untouched: distinct beats sort by beat, as they always
/// did, and the tie-break only ever reorders within one instant.
#[test]
fn distinct_beats_are_unaffected() {
    let fugue = CompositeFugue {
        raw_events: vec![
            TimedFugueEvent {
                beat_offset: 3.0,
                event: FugueEvent::NoteOff { channel: 0, note: 62, velocity: 0 },
            },
            TimedFugueEvent {
                beat_offset: 1.0,
                event: FugueEvent::NoteOn { channel: 0, note: 62, velocity: 90 },
            },
        ],
        ..Default::default()
    };

    let beats: Vec<f64> = fugue.compile().events.iter().map(|e| e.beat_offset).collect();
    assert_eq!(beats, vec![1.0, 3.0]);
}
