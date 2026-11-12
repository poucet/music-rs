# simply-fugue

The Fugue musical definition, compact note parsing, and the `render_audio` tool contract — **shared by Nexus and Flux**, which take the same fugue JSON under the same tool name.

Droplets keeps its own fugue model and is deliberately not a consumer. It shares the vocabulary and almost none of the types: every mode enum has a disjoint variant set, and its `FugueDefinition` is flat where this one nests. Converging it would flip three defaults under a live sequencer.

**Pure-data contract**: `simply-fugue` depends strictly on `serde`, `serde_json` and `schemars`. It contains no DSP code, audio drivers, or host-specific logic. A dependency here is a dependency in every consumer.

## What it provides

* **Note parsing**: `parse_note_name("C3")` → `60`, `format_note_name(60)` → `"C3"`. Standard DAW pitch notation where `C3` = MIDI 60 (`C-2` = MIDI 0).
* **Compact fugue**: notes as `[beat, pitch, duration, velocity, channel]` arrays; automation as per-CC or per-parameter point lists with curve interpolation.
* **`render_audio` request/response**: `RenderAudioRequest` (a fugue, sample rate, output format) and `RenderAudioResponse` (the WAV or raw f32 bytes, warnings).

Both Flux and Nexus render per-note expression (pitch bend and pressure), as well as channel pitch bend, channel pressure, and polyphonic pressure. Host capabilities and plugin instance support are reported in `warnings` on the `render_audio` response.

## Compact fugue sequence

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

## Consumers

Used by Nexus (pinned to `tag = "v1.0.1"`), Flux and droplets (both unpinned on `main`). Nexus and Flux both serve the `render_audio` MCP tool over the same JSON.

## Depending on it

```toml
[dependencies]
simply-fugue = { git = "https://github.com/simply-audio/music-rs.git" }
```

Or pin a tag: `simply-fugue = { git = "https://github.com/simply-audio/music-rs.git", tag = "v1.0.1" }`

## Licence

MIT — see [LICENSE](../../LICENSE).
