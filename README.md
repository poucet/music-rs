# music-rs

Canonical musical definitions, performance sequencing models, and tool contracts shared across the Simply audio ecosystem (including [Nexus](https://github.com/poucet/nexus) and Flux).

## Overview

`music-rs` provides pure-data contracts, musical primitives, and protocol implementations for audio synthesis, plugin hosting, and AI-driven sound generation. Crates in this repository are designed to be lightweight, dependency-minimal, and deterministic.

## Crates

| Crate | Description |
| --- | --- |
| [`simply-fugue`](crates/simply-fugue) | Canonical Fugue musical performance schema, compact note notation (`[beat, "C3", duration, velocity, channel]`), pitch parsing (`C3` = 60), automation curves, and `render_audio` request/response types. |
| [`simply-osc`](crates/simply-osc) | Open Sound Control 1.0 — typed messages, bundles, the wire format, address pattern matching, a thin UDP receiver, and the `simply-osc` command-line sender. Zero dependencies. |
| [`simply-link`](crates/simply-link) | Ableton Link — an independent implementation of the protocol: peer discovery, the shared timeline, ghost-time measurement, quantum, and start/stop sync. |
| [`simply-midi`](crates/simply-midi) | MIDI 1.0 channel-voice decoding, and midir device input behind the `device` feature. |
| [`simply-midi-clock`](crates/simply-midi-clock) | MIDI beat clock out: the real-time messages, the follower's position, sleep-then-spin pacing, and output-port selection. |
| [`simply-link-midi`](crates/simply-link-midi) | The `link-midi` binary — an Ableton Link session out as MIDI beat clock, phase-aligned to the session's quantum. |

## Binaries

| Binary | What it does | Run |
| --- | --- | --- |
| `link-midi` | Joins the Ableton Link session on your network and sends MIDI beat clock to one output port, in time and in phase — a drum machine's bar one lands on the session's downbeat. Follows only; never changes the tempo or the transport. | `cargo run -p simply-link-midi -- run --port NAME` (or `cargo install --path crates/simply-link-midi`) |
| `simply-osc` | Sends one OSC message over UDP from the command line — types inferred, `i:`/`f:`/`s:` prefixes to force one. The by-hand half of `simply-osc`; the library receives. | `cargo run -p simply-osc -- /address 0.5` (or `cargo install --path crates/simply-osc`) |

Each crate's own README carries the details: [`simply-link-midi`](crates/simply-link-midi/README.md), [`simply-osc`](crates/simply-osc/README.md).

### Installing them

`./install` builds every binary in release and copies it into `~/.local/bin` (macOS and Linux alike):

```sh
./install                        # all of them, into ~/.local/bin
./install link-midi              # just the ones named
./install --prefix /opt/simply   # into /opt/simply/bin instead (or PREFIX=/opt/simply)
./install --list                 # what the workspace builds
./install --uninstall            # remove them again (names and --prefix work here too)
```

The binary list comes from `cargo metadata`, so a new `[[bin]]` is picked up without editing the script. If the destination is not on your `PATH`, the script prints the `export PATH=…` line to add; it never edits your shell's startup files.

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
