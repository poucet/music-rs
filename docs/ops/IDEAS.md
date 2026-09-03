# Ideas

An inbox: raw, half-formed, uncommitted. An idea only ever moves down; ids are
stable and never reused.

> What belongs here, and what does not: @AGENTS.md §2.
>
> Status: 💡 open · 🔶 partly answered by something that shipped · 🎯 next up ·
> 📋 specced and queued · 🔄 in flight · 🔬 being researched · ✅ shipped ·
> 🗑️ answered by deciding not to.

## Inbox
| Status | Added | # | Idea | Notes |
|---|---|---|---|---|
| 💡 | 2026-09-03 | 1 | **`simply-signal` as this repo's second tenant** | It is signal math, it already has three consumers in `flux` and three in `sense`, and the two copies are byte-identical *today* only because the fork is hours old. Moving it is also what proves the §2.4 locating scheme, since it is the first crate two repos would depend on here at once — better tested deliberately than discovered under a lane that is also changing a wire format. Its own lane, not part of the fugue track. (FUGUE-API.md §6.5.) |
| 💡 | 2026-09-03 | 2 | **A home for the other four duplicated crates** | `simply-api`, `simply-api-macros`, `simply-history` and `simply-op-macros` are byte-identical forks across `flux` and `sense` — 3 523 lines with idea 1, zero divergence, short half-life. They are generic infrastructure, and `music` is *not* their home. The question is which repo is. File before the first divergence makes it a merge instead of a `diff`. |

## In-flight
| Status | Added | # | Idea | Notes |
|---|---|---|---|---|

## Done
| Status | Added | # | Idea | Notes |
|---|---|---|---|---|
