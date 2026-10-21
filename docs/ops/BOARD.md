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
| 2026-10-21 | `LINK-STORM` | `simply-link`: a member's timeline is adopted only if newer (total order, Ableton's beat-origin rule), and adopted changes are re-announced at most every 50 ms. Ends the crossing-proposal multicast storm (~10k msgs/s per peer) that cost tussen's RTP ~45 % of its packets |
| 2026-10-19 | `MUSIC-INSTALL` | `./install` POSIX script: release-builds every workspace binary (found via `cargo metadata`) into `~/.local/bin`, `--prefix`/`PREFIX`, `--list`, `--uninstall`, named subsets; README section |
| 2026-10-19 | `LINK-JOIN` | `simply-link` pongs carry `sess` (Ableton peers can join our sessions); sessions merge by Link's rule (older wins); founder ghost starts at 0; Ableton-peer model tests |
| 2026-10-16 | `LINK-MIDI-NET` | cross-machine Link audit: discovery/advertisement already right; `join` counts EADDRINUSE as joined (Wi-Fi new-address rejoin); `founder` example; two-laptop procedure in link-midi README |
| 2026-10-10 | `MIDIMOVE` | five protocol-floor crates moved in from flux (`simply-osc`, `simply-link`, `simply-midi-clock`, `simply-link-midi`, `simply-midi`); two binaries (`simply-osc`, `link-midi`); rustfmt.toml + fmt sweep; 133/0/1, clippy 0 |
| 2026-09-06 | `FUGUENEXUS-3` | `CompositeFugue.ccs` keyed by `CcTarget` for multi-channel CC (`I-53`); per-note expression docs updated (`I-91`); 12-fixture gate green unchanged |
| 2026-09-04 | `FUGUENEXUS-2` | `output_format` closed into `OutputFormat::{File, WavBase64}`, defaulting to `File`; `compile()` sorts `(beat_offset, is_note_on)` so a note-off precedes a note-on (flux `I-54`); 12-fixture gate green unchanged |
| 2026-09-03 | `FUGUECRATE` | self round-trip fixed (untagged array arms typed `Vec<Value>`); per-note lane `interpolation` now the default for curve-less points, via a new `PerNotePoint`; 12-fixture gate green unchanged |
| 2026-09-03 | `FUGUECRATE` | `simply-fugue` extracted from nexus into this repo and widened into the shared superset; `RenderAudioRequest`/`Response` moved in; 12-fixture wire-compat gate |
