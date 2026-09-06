# Changelog

What got added, per landing — dated with a clock and the jj change id. The entry
format is stated once in AGENTS.md §2.

## Unreleased

#### Multi-channel CC support and per-note expression doc update (`I-53`, `I-91`) · 2026-09-06 18:10 · xrlntslz

- **Multi-channel CC lanes (`I-53`)**: `CompositeFugue.ccs` is now keyed by `CcTarget`, which deserializes both plain controller numbers (`"74"`) and channel-qualified pairs (`"channel:controller"` like `"1:74"`). Serialization formats channel 0 as `"{controller}"` preserving byte-identical backwards compatibility. `CompositeFugue::compile()` passes `target.channel` to `FugueEvent::Cc` instead of hardcoding `channel: 0`.
- **Per-note expression doc update (`I-91`)**: Updated `lib.rs`, `types.rs` (`PerNotePitchBend`, `PerNotePressure`), and `composite.rs` comments to reflect that both Flux and Nexus render per-note expressions.

38 tests (was 35); `cargo clippy --workspace --all-targets` 0 warnings; 12-fixture wire-compat gate green unchanged.

#### `output_format` closes, and note-offs sort first (`FUGUENEXUS-2`) · 2026-09-04 19:08 · prrqtusm

The two shared-crate changes nexus needs to follow FUGUE-API.md §6's ruling.
Deliberately the smallest pair that unblocks it, because consumers take this
crate by git URL and nothing here is visible to them until it is pushed.

- **`output_format` is an enum** — `OutputFormat::{File, WavBase64}`,
  `#[serde(rename_all = "snake_case")]`, defaulting to `File` (§6.6). Both
  spellings that worked parse unchanged and round-trip to the same bytes;
  **anything else is now a deserialize error naming both**. It was a bare
  `Option<String>` that only ever tested for `"file"`, so `"wav"`, `"File"` or
  a typo fell silently through to base64 and returned tens of megabytes of
  inline audio. This is the one place the compatibility rule is knowingly
  broken: such JSON parsed, but it never meant what its author typed (§3.5).
  The field stays `Option` so a host can tell an explicit `file` from silence;
  `RenderAudioRequest::output_format()` resolves the absent case once, so no
  consumer spells the default again.
- **`compile()` sorts by `(beat_offset, is_note_on)`** (flux's `I-54`). Ties
  used to fall to stable-sort insertion order — `raw_events`, notes, ccs,
  params — so a same-pitch retrigger at a note boundary compiled *on then off*
  and a host emitting the compiled order as MIDI killed the note it had just
  started: one audible note where the fugue says two. Flux never saw it because
  `NoteEdge::order()` re-sorts whatever it receives; nexus did.
  `FugueEvent::is_note_on()` holds the rule, on the event rather than in one
  host's renderer, because both hosts sort on it. Everything that is neither
  edge — a CC, a param, an expression point — keeps its place ahead of the note
  it sets up.

**This changes what already-valid JSON sounds like** on any host that renders
the compiled order directly, and it changes where a request with no
`output_format` sends its audio. Both were ruled, not assumed.

35 tests (was 29); `cargo clippy --workspace --all-targets` 0 warnings; the
12-fixture wire-compat gate green and **unchanged** — the sort is not a parse.

#### Two corrections to the superset (`FUGUECRATE`) · 2026-09-03 22:29 · tvmrstly

Both were found while building the superset, filed rather than fixed, and
released by Chris on 2026-09-03. The 12-fixture wire-compat gate is green
**unchanged** across both — the proof that nothing already accepted moved.

- **The crate re-parses its own serialized output.** It did not before. The
  `#[serde(untagged)]` helpers for `NoteItem`, `CcPointItem` and
  `ParamPointItem` typed their array arm `serde_json::Value`, which matches a
  JSON *object* too — so the object form that `Serialize` emits was swallowed
  by the array arm and then failed inside it. `from_str(to_string(f))` failed
  for any fugue with notes, CCs or params, and the object form that those
  types' doc comments and nexus's `AI_SAMPLER_MCP.md:43` advertise had never
  worked. The arms are now `Vec<serde_json::Value>`, disjoint by shape.
  **Additive**: JSON that used to error now parses; nothing that parsed
  changed meaning, and `malformed_points_are_still_rejected` pins that the
  array form did not get looser.
- **A per-note lane's `interpolation` is the default for points that name no
  curve of their own** — Droplets' semantics for `CompactPitchBend`, which is
  where the shape came from. Distinguishing *absent* from an explicit `"none"`
  needs a curve that can be absent, so per-note lanes get their own point type,
  `PerNotePoint` (`{beat, value, curve: Option<InterpolationMode>}`), instead of
  reusing `ParamPointItem`. Resolution is two levels and no more: the point's
  curve wins, else the lane's. **`ccs` and `params` are untouched** — a
  fugue-level curve default is a third level and stays rejected (FUGUE-API.md
  §2.3).

Along the way the `[beat, value, curve?]` array form — which was written out
three times and would have become four — moved into one `parse_point_array`
helper, parameterised by how each kind reads its value. Error messages are
byte-identical.

29 tests (was 24); `cargo clippy --workspace --all-targets` 0 warnings. Both
`known_defect_*` tests are gone, replaced by the positive properties they stood
in for: `a_fugue_survives_its_own_serialization` over all 12 fixtures,
`the_documented_object_form_parses_and_agrees_with_the_array_form`,
`every_superset_field_round_trips` (now over *every* field, with clippy
enforcing that a new field breaks it until round-tripped), and
`an_inherited_curve_survives_a_round_trip_as_an_absence`.

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
