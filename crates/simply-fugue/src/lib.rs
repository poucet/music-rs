//! `simply-fugue`: Canonical Fugue musical definition, compact note parsing,
//! and performance scheduling across the Simply ecosystem (Flux, Nexus, Droplets).

pub mod composite;
pub mod note;
pub mod types;

pub use composite::{CcPointItem, CompiledFugue, CompositeFugue, NoteItem, ParamPointItem};
pub use note::{format_note_name, parse_note_name};
pub use types::{
    CancelMode, FugueDefinition, FugueEvent, FugueHeader, InterpolationMode, LoopMode,
    QuantizeMode, StartMode, TimedFugueEvent,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_note_names() {
        assert_eq!(parse_note_name("C-2").unwrap(), 0);
        assert_eq!(parse_note_name("C3").unwrap(), 60);
        assert_eq!(parse_note_name("c3").unwrap(), 60);
        assert_eq!(parse_note_name("A3").unwrap(), 69);
        assert_eq!(parse_note_name("C#3").unwrap(), 61);
        assert_eq!(parse_note_name("Db3").unwrap(), 61);
        assert_eq!(parse_note_name("F#2").unwrap(), 54);
        assert_eq!(parse_note_name("Gb2").unwrap(), 54);
        assert_eq!(parse_note_name("G8").unwrap(), 127);
        assert_eq!(format_note_name(60), "C3");
        assert_eq!(format_note_name(0), "C-2");
        assert_eq!(format_note_name(69), "A3");
    }

    #[test]
    fn deserialize_array_shorthand_notes() {
        let json_data = r#"[
            [0, "C3", 1.0, 100],
            [0.5, 64, 0.5, 90, 1],
            [1.0, "G3"]
        ]"#;

        let notes: Vec<NoteItem> = serde_json::from_str(json_data).unwrap();
        assert_eq!(notes.len(), 3);
        assert_eq!(notes[0].beat, 0.0);
        assert_eq!(notes[0].note, 60);
        assert_eq!(notes[0].duration, 1.0);
        assert_eq!(notes[0].velocity, 100);
        assert_eq!(notes[0].channel, 0);

        assert_eq!(notes[1].beat, 0.5);
        assert_eq!(notes[1].note, 64);
        assert_eq!(notes[1].channel, 1);

        assert_eq!(notes[2].beat, 1.0);
        assert_eq!(notes[2].note, 67);
        assert_eq!(notes[2].duration, 1.0); // default duration
    }

    #[test]
    fn deserialize_points_with_curves() {
        let json_data = r#"{
            "notes": [
                [0, "C3", 2.0, 100]
            ],
            "ccs": {
                "74": [
                    [0.0, 30, "linear"],
                    [1.0, 120, "exp"]
                ]
            }
        }"#;

        let fugue: CompositeFugue = serde_json::from_str(json_data).unwrap();
        assert_eq!(fugue.notes.len(), 1);
        assert_eq!(fugue.ccs.get(&74).unwrap().len(), 2);
        assert_eq!(fugue.ccs.get(&74).unwrap()[0].curve, InterpolationMode::Linear);
        assert_eq!(fugue.ccs.get(&74).unwrap()[1].curve, InterpolationMode::Exp);

        let compiled = fugue.compile();
        assert_eq!(compiled.events.len(), 3);
        assert_eq!(compiled.duration_beats, 2.0);
    }
}
