//! Tests for multi-channel CC lane targets (`I-53`).

use simply_fugue::{CcPointItem, CcTarget, CompositeFugue, FugueEvent, InterpolationMode};

#[test]
fn cc_target_from_str_and_display() {
    let plain: CcTarget = "74".parse().unwrap();
    assert_eq!(plain, CcTarget { channel: 0, controller: 74 });
    assert_eq!(plain.to_string(), "74");

    let qualified: CcTarget = "1:74".parse().unwrap();
    assert_eq!(qualified, CcTarget { channel: 1, controller: 74 });
    assert_eq!(qualified.to_string(), "1:74");

    let max: CcTarget = "15:127".parse().unwrap();
    assert_eq!(max, CcTarget { channel: 15, controller: 127 });
    assert_eq!(max.to_string(), "15:127");

    // Invalid channels and controllers
    assert!("16:74".parse::<CcTarget>().is_err());
    assert!("1:128".parse::<CcTarget>().is_err());
    assert!("bad".parse::<CcTarget>().is_err());
}

#[test]
fn multi_channel_cc_compiles_with_assigned_channel() {
    let json = r#"{
        "ccs": {
            "74": [
                [0.0, 10, "linear"],
                [1.0, 20, "linear"]
            ],
            "2:74": [
                [0.0, 50, "exp"],
                [1.0, 100, "exp"]
            ]
        }
    }"#;

    let fugue: CompositeFugue = serde_json::from_str(json).unwrap();
    assert_eq!(fugue.ccs.len(), 2);

    let ch0_target = CcTarget { channel: 0, controller: 74 };
    let ch2_target = CcTarget { channel: 2, controller: 74 };

    assert!(fugue.ccs.contains_key(&ch0_target));
    assert!(fugue.ccs.contains_key(&ch2_target));

    let compiled = fugue.compile();
    let cc_events: Vec<_> = compiled
        .events
        .iter()
        .filter_map(|e| match &e.event {
            FugueEvent::Cc { channel, controller, value, interpolation } => {
                Some((*channel, *controller, *value, *interpolation, e.beat_offset))
            }
            _ => None,
        })
        .collect();

    assert_eq!(cc_events.len(), 4);
    assert!(cc_events.contains(&(0, 74, 10, InterpolationMode::Linear, 0.0)));
    assert!(cc_events.contains(&(0, 74, 20, InterpolationMode::Linear, 1.0)));
    assert!(cc_events.contains(&(2, 74, 50, InterpolationMode::Exp, 0.0)));
    assert!(cc_events.contains(&(2, 74, 100, InterpolationMode::Exp, 1.0)));
}

#[test]
fn serialization_round_trip_preserves_plain_and_qualified_keys() {
    let mut fugue = CompositeFugue::default();
    fugue.ccs.insert(
        CcTarget::controller(74),
        vec![CcPointItem { beat: 0.0, value: 64, curve: InterpolationMode::Linear }],
    );
    fugue.ccs.insert(
        CcTarget::new(3, 11),
        vec![CcPointItem { beat: 0.5, value: 127, curve: InterpolationMode::None }],
    );

    let val = serde_json::to_value(&fugue).unwrap();
    let ccs_obj = val.get("ccs").unwrap().as_object().unwrap();

    assert!(ccs_obj.contains_key("74"), "channel 0 should serialize as plain controller '74'");
    assert!(ccs_obj.contains_key("3:11"), "channel 3 should serialize as '3:11'");

    let round_tripped: CompositeFugue = serde_json::from_value(val).unwrap();
    assert_eq!(round_tripped, fugue);
}
