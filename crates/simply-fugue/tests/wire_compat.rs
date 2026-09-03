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
    assert_eq!(
        serde_json::to_value(&forever).unwrap()["loop_mode"],
        serde_json::json!("loop")
    );
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
    assert_eq!(
        serde_json::to_value(&header).unwrap()["quantize"],
        serde_json::json!("none")
    );
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

/// **A defect this crate arrived with, pinned rather than fixed.**
///
/// `NoteItem` / `CcPointItem` / `ParamPointItem` each deserialize through an
/// `#[serde(untagged)]` helper whose *first* arm is `Array(serde_json::Value)`.
/// A `serde_json::Value` matches anything, objects included, so the object form
/// — which the doc comments on those types and `AI_SAMPLER_MCP.md` both
/// advertise — never reaches its own arm and fails inside the array branch.
///
/// The consequence is larger than the documented form being dead: `Serialize`
/// is derived, and it *emits* the object form, so **the crate does not re-parse
/// its own output** whenever `notes`, `ccs` or `params` is non-empty.
///
/// Fixing it is a behaviour change (JSON that errors today would start parsing)
/// and is not this lane's to make. This test states today's truth so the defect
/// cannot be forgotten; **delete it in the change that fixes it.**
#[test]
fn known_defect_object_form_and_self_round_trip_are_broken() {
    let with_notes = r#"{"notes":[[0,"C3",1.0,100]]}"#;
    let fugue: CompositeFugue = serde_json::from_str(with_notes).unwrap();
    let emitted = serde_json::to_string(&fugue).unwrap();
    assert!(emitted.contains(r#"{"beat":0.0,"note":60,"#), "emitted: {emitted}");

    let reparsed = serde_json::from_str::<CompositeFugue>(&emitted);
    assert!(
        reparsed.is_err(),
        "the object form now parses — good; delete this test and its issue row"
    );
}
