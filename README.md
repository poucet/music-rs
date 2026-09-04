# music-rs

Canonical musical definitions, performance sequencing models, and tool contracts shared across the Simply audio ecosystem (including [Nexus](https://github.com/poucet/nexus) and Flux).

## Overview

`music-rs` provides pure-data contracts and musical primitives for audio synthesis, plugin hosting, and AI-driven sound generation. Crates in this repository are designed to be lightweight, dependency-minimal, and deterministic wire formats.

## Crates

| Crate | Description |
| --- | --- |
| [`simply-fugue`](crates/simply-fugue) | Canonical Fugue musical performance schema, compact note notation (`[beat, "C3", duration, velocity, channel]`), pitch parsing (`C3` = 60), automation curves, and `render_audio` request/response types. |

### Design Principles

- **Pure-Data Contract**: `simply-fugue` depends strictly on `serde`, `serde_json`, and `schemars`. It contains no DSP code, audio drivers, or host-specific logic.
- **Wire Compatibility**: Serialized performance definitions can be exchanged seamlessly between hosts, agents, and modular environments over JSON and the Model Context Protocol (MCP).
- **Standard Pitch Mapping**: Standard DAW pitch notation where `C3` = MIDI 60 (`C-2` = MIDI 0).

## Usage

Add `simply-fugue` from GitHub to your `Cargo.toml`:

```toml
[dependencies]
simply-fugue = { git = "https://github.com/poucet/music-rs.git" }
```

### Parsing Note Names

```rust
use simply_fugue::{parse_note_name, format_note_name};

assert_eq!(parse_note_name("C3").unwrap(), 60);
assert_eq!(parse_note_name("A4").unwrap(), 81);
assert_eq!(format_note_name(60), "C3");
```

### Compact Fugue Sequence

```json
{
  "tempo": 120.0,
  "notes": [
    [0.0, "C3", 1.0, 100],
    [1.0, "E3", 1.0, 90],
    [2.0, "G3", 1.0, 95],
    [3.0, "B3", 1.0, 110]
  ]
}
```

## License

Licensed under the [MIT License](LICENSE).
