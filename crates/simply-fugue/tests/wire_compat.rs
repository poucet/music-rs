//! **The compatibility gate: every fugue JSON nexus accepts today still parses,
//! to the same `CompositeFugue`.**
//!
//! Widening this crate into a superset is only safe if nothing that already
//! works stops working. That is a claim about *parsing*, so it is tested by
//! parsing — not by reading the diff and believing it.
//!
//! Each `fixtures/<name>.json` is an input harvested from nexus at the moment
//! the crate moved out. Each `fixtures/<name>.expected.json` is what the **old**
//! crate parsed that input to, recorded by serializing its `CompositeFugue`
//! before a single superset change was made. A fixture passes when the current
//! crate parses the input to a value that serializes to the same JSON.
//!
//! Where the fixtures came from — 12 in total:
//!
//! | fixture | source |
//! |---|---|
//! | `points_with_curves` | the crate's own `deserialize_points_with_curves` test |
//! | `array_shorthand_notes` | the crate's own `deserialize_array_shorthand_notes` test |
//! | `instructions_chord` | `nexus-mcp/src/instructions.md`, "Fugue Note Shorthand" |
//! | `sampler_doc_cc_points` | `docs/design/AI_SAMPLER_MCP.md`, the CC/param point form and all four curve spellings |
//! | `raw_events_all_variants` | one of each of the eight `FugueEvent` variants nexus parses |
//! | `empty` | `{}` — `CompositeFugue::default()`, which `nexus-host/src/render.rs:50` builds |
//! | `service_smoke` | `nexus-tools/src/service.rs:765`, `play_and_render_audio_smoke` |
//! | `sample_vital` | `nexus-mcp/examples/sample_vital.rs:68` |
//! | `sample_vital_clap` | `nexus-mcp/examples/sample_vital_clap.rs:22` |
//! | `render_sine` | `nexus-host/src/render.rs:478`, `render_offline_sine_wave` |
//! | `render_param_curve` | `nexus-host/src/render.rs:616`, `render_offline_with_param_curve_automation` |
//! | `cc_lane` | the `CcPointItem` Rust shape, so its point form is pinned too |
//!
//! The last six are fugues nexus builds in **Rust**, not JSON. They were
//! harvested by writing each as the array-form JSON an agent would type and
//! checking, against the old crate, that it parsed back to the identical Rust
//! value — so the fixture stands for nexus's code and not merely for itself.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use simply_fugue::CompositeFugue;

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Every `<name>` that has both an input and an expected file.
fn fixture_names() -> Vec<String> {
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(fixtures_dir()).expect("fixtures directory") {
        let path = entry.unwrap().path();
        let file = path.file_name().unwrap().to_str().unwrap();
        if let Some(name) = file.strip_suffix(".expected.json") {
            names.insert(name.to_string());
        }
    }
    names.into_iter().collect()
}

fn read_json(path: &Path) -> serde_json::Value {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn every_nexus_fugue_json_still_parses_to_the_same_value() {
    let names = fixture_names();
    assert_eq!(
        names.len(),
        12,
        "fixture count changed; update the inventory in this file's docs. Found: {names:?}"
    );

    for name in &names {
        let dir = fixtures_dir();
        let input = fs::read_to_string(dir.join(format!("{name}.json")))
            .unwrap_or_else(|e| panic!("{name}: no input fixture: {e}"));

        let parsed: CompositeFugue = serde_json::from_str(&input)
            .unwrap_or_else(|e| panic!("{name}: the superset REJECTS JSON nexus accepts: {e}"));

        let got = serde_json::to_value(&parsed).unwrap();
        let want = read_json(&dir.join(format!("{name}.expected.json")));
        assert_eq!(got, want, "{name}: parses, but not to the value the old crate produced");
    }
}

/// The four superset fields are absent from the serialized form of a fugue that
/// does not use them, so an old fugue's bytes are unchanged on the way out too.
#[test]
fn superset_fields_are_invisible_when_unused() {
    let fugue: CompositeFugue = serde_json::from_str("{}").unwrap();
    let value = serde_json::to_value(&fugue).unwrap();
    let keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
    assert_eq!(
        keys,
        ["ccs", "duration_beats", "loop_mode", "notes", "params", "raw_events"],
        "an unused superset field leaked into the wire form"
    );
}

// --- the aliases: spellings from Droplets' vocabulary that now also parse -----

#[test]
fn cc_is_an_alias_for_controller() {
    let json = r#"{"raw_events":[{"beat_offset":0.0,
        "event":{"type":"cc","channel":0,"cc":74,"value":30}}]}"#;
    let fugue: CompositeFugue = serde_json::from_str(json).unwrap();
    let simply_fugue::FugueEvent::Cc { controller, value, .. } = &fugue.raw_events[0].event else {
        panic!("expected a Cc event, got {:?}", fugue.raw_events[0].event);
    };
    assert_eq!((*controller, *value), (74, 30));

    // and the canonical spelling is unaffected
    let canonical = json.replace("\"cc\":74", "\"controller\":74");
    let same: CompositeFugue = serde_json::from_str(&canonical).unwrap();
    assert_eq!(same, fugue);
}

#[test]
fn forever_is_an_alias_for_loop() {
    let forever: CompositeFugue = serde_json::from_str(r#"{"loop_mode":"forever"}"#).unwrap();
    assert_eq!(forever.loop_mode, simply_fugue::LoopMode::Loop);
    let canonical: CompositeFugue = serde_json::from_str(r#"{"loop_mode":"loop"}"#).unwrap();
    assert_eq!(canonical.loop_mode, simply_fugue::LoopMode::Loop);
    // An alias is read-only: the canonical spelling is what goes back out.
    assert_eq!(serde_json::to_value(&forever).unwrap()["loop_mode"], serde_json::json!("loop"));
}

#[test]
fn immediate_is_an_alias_for_none_quantize() {
    use simply_fugue::{FugueHeader, QuantizeMode};
    let header: FugueHeader =
        serde_json::from_str(r#"{"duration_beats":4.0,"quantize":"immediate"}"#).unwrap();
    assert_eq!(header.quantize, QuantizeMode::None);
    let canonical: FugueHeader =
        serde_json::from_str(r#"{"duration_beats":4.0,"quantize":"none"}"#).unwrap();
    assert_eq!(canonical.quantize, QuantizeMode::None);
    assert_eq!(serde_json::to_value(&header).unwrap()["quantize"], serde_json::json!("none"));
}

/// Every mode default is load-bearing: changing one silently changes what an
/// existing fugue does. Pinned here so a future edit has to argue with a test.
#[test]
fn defaults_are_unchanged() {
    use simply_fugue::{CancelMode, InterpolationMode, LoopMode, QuantizeMode, StartMode};
    assert_eq!(LoopMode::default(), LoopMode::Once);
    assert_eq!(QuantizeMode::default(), QuantizeMode::None);
    assert_eq!(InterpolationMode::default(), InterpolationMode::None);
    assert_eq!(CancelMode::default(), CancelMode::Immediate);
    assert_eq!(StartMode::default(), StartMode::Immediate);
}

/// **The crate re-parses its own serialized output.**
///
/// It did not until 2026-09-03. `NoteItem` / `CcPointItem` / `ParamPointItem`
/// each deserialize through an `#[serde(untagged)]` helper whose array arm was
/// typed `serde_json::Value` — which matches a JSON *object* too, so the object
/// form that `Serialize` emits was swallowed by the array arm and then failed
/// inside it. Typing those arms `Vec<serde_json::Value>` makes the two arms
/// disjoint by shape. Every fixture above still parses to the same value, which
/// is the proof that nothing already accepted changed meaning.
///
/// This is the property every host relies on the moment it saves a fugue it
/// built in Rust and reads it back.
#[test]
fn a_fugue_survives_its_own_serialization() {
    for name in fixture_names() {
        let input = fs::read_to_string(fixtures_dir().join(format!("{name}.json"))).unwrap();
        let fugue: CompositeFugue = serde_json::from_str(&input).unwrap();
        let emitted = serde_json::to_string(&fugue).unwrap();
        let back: CompositeFugue = serde_json::from_str(&emitted)
            .unwrap_or_else(|e| panic!("{name}: cannot re-parse its own output: {e}\n{emitted}"));
        assert_eq!(back, fugue, "{name}: round trip changed the value");
    }
}

/// The object form the type doc comments and nexus's `AI_SAMPLER_MCP.md:43`
/// advertise — dead until 2026-09-03 — parses, and means the same as the array
/// form it sits beside.
#[test]
fn the_documented_object_form_parses_and_agrees_with_the_array_form() {
    let array: CompositeFugue = serde_json::from_str(
        r#"{"notes":[[0,"C3",1.0,100,0]],"ccs":{"74":[[0,30,"linear"]]},
            "params":{"7":[[0,0.5,"exp"]]}}"#,
    )
    .unwrap();
    let object: CompositeFugue = serde_json::from_str(
        r#"{"notes":[{"beat":0.0,"note":"C3","duration":1.0,"velocity":100,"channel":0}],
            "ccs":{"74":[{"beat":0.0,"value":30,"curve":"linear"}]},
            "params":{"7":[{"beat":0.0,"value":0.5,"curve":"exp"}]}}"#,
    )
    .unwrap();
    assert_eq!(array, object);

    // The object form's own defaults still apply: duration 1.0, velocity 100.
    let sparse: CompositeFugue =
        serde_json::from_str(r#"{"notes":[{"beat":2.0,"note":64}]}"#).unwrap();
    assert_eq!(sparse.notes[0].duration, 1.0);
    assert_eq!(sparse.notes[0].velocity, 100);
    assert_eq!(sparse.notes[0].channel, 0);
}

/// Fixing the arm order must not have widened what an array is allowed to be.
/// These were rejected before and are rejected now.
#[test]
fn malformed_points_are_still_rejected() {
    for bad in [
        r#"{"notes":[[0]]}"#,                              // too short
        r#"{"notes":[["nope","C3"]]}"#,                    // beat not a number
        r#"{"notes":[[0,{"a":1}]]}"#,                      // note neither number nor name
        r#"{"notes":[[0,"H9"]]}"#,                         // not a note name
        r#"{"notes":[[0,200]]}"#,                          // out of MIDI range
        r#"{"ccs":{"74":[[0]]}}"#,                         // CC point too short
        r#"{"ccs":{"74":[["x",1]]}}"#,                     // CC beat not a number
        r#"{"params":{"7":[[0,"x"]]}}"#,                   // param value not a number
        r#"{"pitch_bends":[{"note":60,"points":[[0]]}]}"#, // per-note point too short
    ] {
        assert!(
            serde_json::from_str::<CompositeFugue>(bad).is_err(),
            "should have been rejected: {bad}"
        );
    }
}
