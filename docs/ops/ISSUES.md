# Issues

> What belongs here, and what does not: @AGENTS.md §2.
> 🐞 open · 🔬 triaged (cause known, fix specced) · 🔄 being fixed ·
> ✅ fixed · 🗑️ won't fix / not a bug. Sizes: S ≈ a session · M ≈ a lane.

| Status | Found | What | Verdict |
|---|---|---|---|
| 🐞 | 2026-09-03 | **The crate does not re-parse its own serialized output.** `NoteItem`, `CcPointItem` and `ParamPointItem` each deserialize through an `#[serde(untagged)]` helper whose first arm is `Array(serde_json::Value)`. A `Value` matches anything, so an object never reaches its own arm and fails inside the array branch. `Serialize` is derived and *emits* the object form, so `from_str(to_string(f))` fails whenever `notes`, `ccs`, `params`, `pitch_bends` or `pressures` is non-empty — and the object form both the type doc comments and nexus's `AI_SAMPLER_MCP.md:43` advertise has never worked. | Open. S. Fix: order the untagged arms `Object` first, or type the array arm `Vec<serde_json::Value>` so it cannot match a map. Strictly additive on the wire — JSON that errors today would start parsing, nothing that parses today changes — but it *is* a behaviour change, so it is Chris's call, not `FUGUECRATE`'s. Pinned by `known_defect_*` in `tests/wire_compat.rs` and `tests/superset.rs`; **delete those two tests with the fix.** |
| 🔬 | 2026-09-03 | **`PerNoteLane.interpolation` is carried but never applied.** `compile()` takes each point's own `curve` and ignores the lane's field. | Open, deliberate. Folding it in would add a second curve-precedence level, which FUGUE-API.md §2.3 rejects for `cc_interpolation` for exactly the ambiguity reason. Needs a ruling before a renderer honours it. Pinned by `a_lanes_own_interpolation_does_not_override_its_points`. |
| 🔬 | 2026-09-03 | **The metadata half of this model has never been exercised anywhere.** `FugueDefinition`, `FugueHeader`, `QuantizeMode`, `StartMode`, `CancelMode` and now `tag` / `cancel` are constructed by no code path in any consumer — both tools take `CompositeFugue`. | Open, named so it is not a surprise. The first real use of any of them will be its first use ever, so it gets tests rather than trust. (FUGUE-API.md §1.2.) |
