# Changelog

What got added, per landing — dated with a clock and the jj change id. The entry
format is stated once in AGENTS.md §2.

## Unreleased

#### `simply-fugue`, widened into the shared superset (`FUGUECRATE`) · 2026-09-03 22:11 · zskpmwqu

The fugue wire format both Nexus and Flux will speak. Additive only: every new
field is `#[serde(default)]` and skipped when unused, every new spelling is a
`#[serde(alias)]`, and nothing that existed changed spelling, default, or
meaning.

- **Modes**: `LoopMode::Times(n)` (`{"times": 4}`) and `QuantizeMode::Bars(n)`
  (`{"bars": 4}`). `"forever"` now parses as `Loop`, `"immediate"` as `None`.
  `StartMode`, `CancelMode` and `InterpolationMode` are untouched — including
  `InterpolationMode`'s `None` (stepped) default.
- **Per-note expression**: `FugueEvent::PerNotePitchBend` (semitones) and
  `PerNotePressure`, both with a defaulted `interpolation`; and
  `CompositeFugue.pitch_bends` / `.pressures`, lists of `PerNoteLane`
  (`{note, channel, points, interpolation}`, `note` accepting `"C3"`).
  `compile()` emits one sparse event per point and extends `duration_beats` to
  cover them. **Nothing renders them yet on either host** — they are carried so
  a fugue crosses hosts, and the tool description must not promise they sound.
- **`CompositeFugue.tag`** and **`.cancel`** (`CancelTarget`: `"none"` ·
  `{"tag": "melody"}` · `"all"`) — the what-to-cancel axis, which is not the
  when-to-stop axis `CancelMode` already covers.
- **`FugueEvent::Cc.controller` also accepts `cc`**, Droplets' spelling.
- **`RenderAudioRequest` / `RenderAudioResponse` moved in** from
  `nexus-tools/src/types.rs`, so parity on the return leg is type identity
  rather than two structs kept in step by hand. The response gains `warnings`,
  `rms` and `peak_amplitude` (linear) — all skipped when unused, so nexus's
  serialized response is unchanged — and derives `Default`. `output_format`
  stays `Option<String>`.

**The gate is a fixture test, not a reading of the diff.** `tests/wire_compat.rs`
holds all 12 fugue JSONs that existed in nexus at the moment of the move — from
the crate's own tests, `instructions.md`, `AI_SAMPLER_MCP.md`, and the six
fugues nexus builds in Rust — each with what the **old** crate parsed it to,
recorded before a single superset change. 24 tests; `cargo clippy
--workspace --all-targets` 0 warnings.

**Found and not fixed**: the crate does not re-parse its own serialized output.
See @docs/ops/ISSUES.md.

#### Extracted from nexus into this repo (`FUGUECRATE`) · 2026-09-03 22:03 · mqksrwnl

`crates/simply-fugue` moved out of `~/projects/simply/nexus`, where it had been
vendored since 2026-09-01 and never pushed. Two repos now need the same fugue
JSON, so it lives in one neither owns. Moved verbatim — the only source change
was dropping `repository.workspace`, which this root does not define.
