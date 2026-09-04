# Board

Line-items only; what belongs here is stated once in AGENTS.md §2. Scrathpad state
(what is running right now) lives in `docs/scratch/` — gitignored, invisible to
lanes; a lane gets its context from its brief, not from here.

Legend: 🚧 in flight · ✅ landed · ⏸ deferred.

## In flight

## Milestones

## Queued

## Deferred by decision

## Landed index

| Landed | Lane | What |
|---|---|---|
| 2026-09-04 | `FUGUENEXUS-2` | `output_format` closed into `OutputFormat::{File, WavBase64}`, defaulting to `File`; `compile()` sorts `(beat_offset, is_note_on)` so a note-off precedes a note-on (flux `I-54`); 12-fixture gate green unchanged |
| 2026-09-03 | `FUGUECRATE` | self round-trip fixed (untagged array arms typed `Vec<Value>`); per-note lane `interpolation` now the default for curve-less points, via a new `PerNotePoint`; 12-fixture gate green unchanged |
| 2026-09-03 | `FUGUECRATE` | `simply-fugue` extracted from nexus into this repo and widened into the shared superset; `RenderAudioRequest`/`Response` moved in; 12-fixture wire-compat gate |
