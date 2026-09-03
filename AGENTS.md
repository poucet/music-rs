# AGENTS — Simply Music

**Read @SYSTEM.md first.** It is the working system — the roles, the doc web,
the landing rules, and every scar behind them — and it is shared with every
project run this way. This file adds only what is true of *music*.

`SYSTEM.md` and `.claude/agents` are symlinks into `~/projects/simply/dev/`, and
both are gitignored because their targets are machine-specific absolute paths.
They therefore do **not** exist in a fresh clone, on a new machine, or in a new
`jj` workspace — recreate them there:

    mkdir -p .claude
    ln -sfn ~/projects/simply/dev/agents .claude/agents
    ln -sfn ~/projects/simply/dev/AGENTS.md SYSTEM.md

**Bootstrap**: this file plus @SYSTEM.md is the entry point for an empty context
window. Live state is in @docs/ops/BOARD.md.

## What this repo is

**Musical vocabulary shared by more than one repo.** A crate earns a place here
when two projects would otherwise each keep a copy of it and drift. It is not a
home for generic infrastructure — an MCP-tool derive or an undo tree is shared
too, and belongs somewhere that is not `music`.

## Gates

Both, **unfiltered**, reported verbatim with counts, and re-run *after* the final
rebase:

    cargo test --workspace
    cargo clippy --workspace --all-targets     # 0 warnings

## The crates

| crate | what it is |
|---|---|
| `simply-fugue` | the fugue wire format: `CompositeFugue`, events, modes, note-name parsing (`C3` = 60), and the `render_audio` request/response pair |

**`simply-fugue` depends on `serde`, `serde_json` and `schemars`, and on nothing
else, ever.** It is a pure-data contract two hosts deserialize the same JSON
with; a dependency here is a dependency in every consumer. `cargo tree -p
simply-fugue` is the check.

## The consumers

| repo | uses it for |
|---|---|
| `~/projects/simply/nexus` | `render_audio` / `play` over a hosted VST3/CLAP plugin |
| `~/projects/simply/flux` | `render_audio` over a flux patch — the same JSON, the same tool name |

**`droplets` is deliberately not a consumer.** It keeps its own fugue model,
which differs on every mode enum and on the whole shape of `FugueDefinition`;
converging it is an L-sized lane nobody has asked for (Chris, 2026-09-03: *"pick
the better one, I think that's nexus? Don't update droplets (unless it's
trivial). Go with the superset but update only nexus and flux."*). See
@docs/ops/DECISIONS.md.

## How a consumer finds this repo

Both obvious answers fail; the working shape is the two steps below, and it
works because the dependency path stays lexically **outside** every workspace
root.

1. The consumer declares the dependency **once, at its workspace root**, and its
   member crates say `simply-fugue.workspace = true`:

       simply-fugue = { path = "../music/crates/simply-fugue" }

2. Each consumer gets **one symlink inside its already-gitignored lane
   directory**, so `../music` also resolves from a lane workspace:

       ln -sfn ~/projects/simply/music <consumer>/.workspaces/music

**Why not a symlink at the consumer's root.** A path dependency whose lexical
path lies *under* the consumer's workspace root is treated by cargo as an
automatic workspace member, so `simply-fugue` tries to inherit the consumer's
`[workspace.package]` and fails with *"error inheriting license from workspace
root manifest"*. A bare `../music/...` alone is not enough either: it is true
from the consumer's default workspace and resolves to
`<consumer>/.workspaces/music/...` from a lane. Hence both, and hence the
symlink.

Three things that follow, so nobody rediscovers them:

- **The symlink is per-workspace ritual**, exactly like `SYSTEM.md` and
  `.claude/agents` — one `ln -sfn` line in each consumer's own "recreate these"
  block. No new `.gitignore` entry is needed; `/.workspaces/` is already ignored
  in both consumers.
- **A stale-workspace sweep may list `.workspaces/music` as deletable.**
  Harmless-looking and not harmless: it is a symlink, not a build tree, and
  deleting it breaks every lane build with a path that does not exist.
- **No repo builds without both checked out**, and there is no remote to fall
  back on. The trigger for switching to a git dependency with tags is the first
  time one consumer needs a version another does not — recorded in
  @docs/ops/DECISIONS.md so it is not re-derived.

## The doc web

@SYSTEM.md §2 says what belongs in each kind of document. These are this
project's files.

- @docs/ops/BOARD.md — the live board.
- @docs/ops/CHANGELOG.md — a landed lane's entry under `## Unreleased`, dated
  with a clock **and** its jj change id.
- @docs/ops/DECISIONS.md — ADR-lite log of small binding decisions.
- @docs/ops/IDEAS.md — the inbox.
- @docs/ops/ISSUES.md — the parking lot.
- @docs/ops/ROADMAP.md — horizons.
- `docs/design/` — human-review gated.
- `docs/research/` — investigations and measurements.

Docs live under `docs/`, **never inside `crates/`**.

## Workspaces and version control

Colocated git+jj; use **jj, never git** (a `git commit` after `jj new` lands on a
detached head). Trunk is the `main` bookmark. Lane workspaces are
`.workspaces/<lane>/`, gitignored, inside the repo.

**Changing the wire format is a cross-repo change.** `simply-fugue` is a
contract, so a change to it is not done when this repo is green — it is done
when every consumer in the table above still builds and its own gates pass.
Additive-and-defaulted (`#[serde(default)]`, `#[serde(alias)]`) is the way to
change it without a flag day; anything that alters what existing JSON *means*
is a decision, not a refactor, and belongs in @docs/ops/DECISIONS.md first.

## Recovery

@SYSTEM.md §7. Read this file and @docs/ops/BOARD.md first, then `jj log`,
`jj workspace list`, and a per-workspace `jj status`.
