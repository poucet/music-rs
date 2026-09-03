# Issues

> What belongs here, and what does not: @AGENTS.md §2.
> 🐞 open · 🔬 triaged (cause known, fix specced) · 🔄 being fixed ·
> ✅ fixed · 🗑️ won't fix / not a bug. Sizes: S ≈ a session · M ≈ a lane.

| Status | Found | What | Verdict |
|---|---|---|---|
| ✅ | 2026-09-03 | **The crate does not re-parse its own serialized output.** The `#[serde(untagged)]` helpers for `NoteItem`, `CcPointItem` and `ParamPointItem` typed their array arm `serde_json::Value`, which matches a JSON object too, so the object form was swallowed by the array arm and failed inside it. | Fixed 2026-09-03 · `tvmrstly` — array arms typed `Vec<serde_json::Value>`, making the arms disjoint by shape. Account in @docs/ops/CHANGELOG.md. |
| ✅ | 2026-09-03 | **`PerNoteLane.interpolation` is carried but never applied.** `compile()` took each point's own `curve` and ignored the lane's field. | Fixed 2026-09-03 · `tvmrstly` — ruled: the lane's curve is the default for points that name none (Droplets' semantics). Lane-level only; `ccs`/`params` unchanged. Account in @docs/ops/CHANGELOG.md. |
| 🔬 | 2026-09-03 | **The metadata half of this model has never been exercised anywhere.** `FugueDefinition`, `FugueHeader`, `QuantizeMode`, `StartMode`, `CancelMode` and now `tag` / `cancel` are constructed by no code path in any consumer — both tools take `CompositeFugue`. | Open, named so it is not a surprise. The first real use of any of them will be its first use ever, so it gets tests rather than trust. (FUGUE-API.md §1.2.) |
