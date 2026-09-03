//! `RenderAudioRequest` / `RenderAudioResponse` moved here from
//! `nexus-tools/src/types.rs:579-612`. Both hosts now share the type rather
//! than keeping two copies in step by hand, so the thing worth testing is that
//! the move changed nothing an existing caller can see.

use simply_fugue::{CompositeFugue, RenderAudioRequest, RenderAudioResponse, default_instance};

/// `instance` is optional on the wire and defaults to `"default"` — and it is
/// omitted again on the way out when it still is `"default"`, which is what
/// keeps nexus's serialized request byte-identical.
#[test]
fn instance_defaults_in_and_disappears_out() {
    let req: RenderAudioRequest = serde_json::from_str(r#"{"fugue":{}}"#).unwrap();
    assert_eq!(req.instance, "default");
    assert_eq!(req.instance, default_instance());

    let value = serde_json::to_value(&req).unwrap();
    let keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
    assert_eq!(keys, ["fugue"], "a default instance must not reach the wire");

    let named: RenderAudioRequest =
        serde_json::from_str(r#"{"instance":"second","fugue":{}}"#).unwrap();
    assert_eq!(named.instance, "second");
    assert_eq!(serde_json::to_value(&named).unwrap()["instance"], "second");
}

/// Every optional request field is absent-able, and `output_format` is still a
/// bare string — turning it into an enum is a decision, not a refactor.
#[test]
fn a_full_request_parses_with_the_fields_nexus_documents() {
    let json = r#"{
        "instance": "default",
        "fugue": { "notes": [[0, "C3", 2.0, 100]] },
        "tempo_bpm": 90.0,
        "sample_rate": 44100,
        "block_size": 256,
        "output_format": "file",
        "output_path": "/tmp/out.wav"
    }"#;
    let req: RenderAudioRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.tempo_bpm, Some(90.0));
    assert_eq!(req.sample_rate, Some(44100));
    assert_eq!(req.block_size, Some(256));
    assert_eq!(req.output_format.as_deref(), Some("file"));
    assert_eq!(req.output_path.as_deref(), Some("/tmp/out.wav"));
    assert_eq!(req.fugue.notes.len(), 1);
    assert_eq!(req.fugue.notes[0].note, 60);

    let minimal: RenderAudioRequest = serde_json::from_str(r#"{"fugue":{}}"#).unwrap();
    assert_eq!(minimal.fugue, CompositeFugue::default());
    assert!(minimal.tempo_bpm.is_none() && minimal.output_format.is_none());
}

/// **The response nexus sends today, unchanged.** The three added fields are
/// skipped when unused, so a host that fills none of them emits exactly the
/// nine keys it always did.
#[test]
fn a_response_serializes_exactly_as_nexus_did() {
    let res = RenderAudioResponse {
        sample_rate: 48000,
        channels: 2,
        frames: 24000,
        duration_s: 0.5,
        peak_db: -6.0,
        rms_db: -12.0,
        is_silent: false,
        file_path: Some("/tmp/out.wav".into()),
        ..Default::default()
    };
    let value = serde_json::to_value(&res).unwrap();
    let keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
    assert_eq!(
        keys,
        [
            "channels",
            "duration_s",
            "file_path",
            "frames",
            "is_silent",
            "peak_db",
            "rms_db",
            "sample_rate"
        ],
        "warnings/rms/peak_amplitude must be invisible when unused"
    );

    let back: RenderAudioResponse = serde_json::from_str(&value.to_string()).unwrap();
    assert_eq!(back, res);
}

/// A host that *does* fill them — flux — gets them across, and a host that
/// reads an older response still parses it, because all three default.
#[test]
fn the_added_response_fields_travel_and_default() {
    let res = RenderAudioResponse {
        sample_rate: 48000,
        channels: 2,
        frames: 100,
        warnings: vec!["channels 1, 2, 10 collapsed onto one instrument".into()],
        rms: Some(0.25),
        peak_amplitude: Some(0.5),
        ..Default::default()
    };
    let json = serde_json::to_string(&res).unwrap();
    let back: RenderAudioResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(back, res);

    // The shape nexus emits today, read by a client that knows the new fields.
    let old = r#"{"sample_rate":48000,"channels":2,"frames":100,"duration_s":0.0,
        "peak_db":0.0,"rms_db":0.0,"is_silent":false}"#;
    let parsed: RenderAudioResponse = serde_json::from_str(old).unwrap();
    assert!(parsed.warnings.is_empty());
    assert_eq!((parsed.rms, parsed.peak_amplitude), (None, None));
}

/// `Default` is derived so the three response constructors in nexus can take
/// the new fields with `..Default::default()` and change nothing else.
#[test]
fn default_response_is_the_silent_zero_render() {
    let res = RenderAudioResponse::default();
    assert_eq!((res.sample_rate, res.channels, res.frames), (0, 0, 0));
    assert!(!res.is_silent && res.wav_base64.is_none() && res.warnings.is_empty());
}
