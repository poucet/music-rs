//! Note parsing and pitch conversion helpers using standard DAW convention (C3 = 60).

use serde::{Deserialize, Deserializer};

/// Parse a note name string into a MIDI pitch number (0..=127).
///
/// Follows standard DAW convention (Bitwig, Ableton, Logic, Reaper, Studio One):
/// - `C3` = MIDI 60 (Middle C)
/// - `A3` = MIDI 69 (440 Hz concert pitch)
/// - `C-2` = MIDI 0 (lowest possible MIDI note)
/// - `G8` = MIDI 127 (highest note)
///
/// Supports sharp (`#`) and flat (`b`) accidentals, e.g. `C#3`, `Db3`, `F#2`, `Gb2`.
pub fn parse_note_name(name: &str) -> Result<u8, String> {
    let s = name.trim();
    if s.is_empty() {
        return Err("empty note name".into());
    }

    let mut chars = s.chars().peekable();
    let letter = chars.next().unwrap().to_ascii_uppercase();

    let base_semitone = match letter {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return Err(format!("invalid note letter '{letter}' in '{s}'")),
    };

    let mut accidental: i32 = 0;
    while let Some(&c) = chars.peek() {
        match c {
            '#' => {
                accidental += 1;
                chars.next();
            }
            'b' | 'B' => {
                // A 'b' is a flat when the octave follows it, or when no
                // accidental has been read yet; otherwise it ends the run.
                let octave_follows = chars
                    .clone()
                    .nth(1)
                    .is_some_and(|next| next.is_ascii_digit() || next == '-');
                if octave_follows || accidental == 0 {
                    accidental -= 1;
                    chars.next();
                } else {
                    break;
                }
            }
            _ => break,
        }
    }

    let octave_str: String = chars.collect();
    if octave_str.is_empty() {
        return Err(format!("missing octave in note '{s}'"));
    }

    let octave: i32 = octave_str
        .parse()
        .map_err(|_| format!("invalid octave '{octave_str}' in '{s}'"))?;

    // In DAW convention: octave -2 -> base 0 (C-2 = 0).
    // MIDI = (octave + 2) * 12 + base_semitone + accidental.
    let midi = (octave + 2) * 12 + base_semitone + accidental;

    if !(0..=127).contains(&midi) {
        return Err(format!("note '{s}' out of MIDI range 0..127 (calculated {midi})"));
    }

    Ok(midi as u8)
}

/// Format a MIDI pitch number (0..=127) as a DAW note name (e.g. 60 -> "C3").
pub fn format_note_name(midi: u8) -> String {
    let names = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    let octave = (midi as i32 / 12) - 2;
    let semitone = (midi % 12) as usize;
    format!("{}{}", names[semitone], octave)
}

/// Helper deserializer for a note value that can be either a numeric MIDI pitch (0..=127)
/// or a note name string like `"C3"`, `"F#2"`, `"Bb0"`.
pub fn deserialize_note_pitch<'de, D>(deserializer: D) -> Result<u8, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum NoteValue {
        Num(u8),
        Name(String),
    }

    match NoteValue::deserialize(deserializer)? {
        NoteValue::Num(n) => {
            if n <= 127 {
                Ok(n)
            } else {
                Err(serde::de::Error::custom(format!("MIDI note {n} exceeds 127")))
            }
        }
        NoteValue::Name(s) => parse_note_name(&s).map_err(serde::de::Error::custom),
    }
}
