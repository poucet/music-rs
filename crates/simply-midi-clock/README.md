# simply-midi-clock

MIDI beat clock out, the parts every sender of it shares.

Two programs in this repository send MIDI clock to a follower — the standalone app off its own transport (`flux-gui`'s `midi_clock`) and the Link bridge off an Ableton Link session (`simply-link-midi`) — and they differ only in *where the instants come from*. What they share lives here, with no flux dependency.

## What it provides

* **[`ClockEvent`]** — the MIDI 1.0 real-time messages a clock sends, and their bytes: `F8` Timing Clock (24 per quarter note), `FA` Start, `FB` Continue, `FC` Stop, `F2` Song Position Pointer (in sixteenths).
* **[`Follower`]** — a mirror of what the follower has been told, so a port opened mid-play can be told where it is.
* **[`pace`] and [`spin_until`]** — how a sender waits for an instant. A follower hears every microsecond of jitter as tempo wobble; `thread::sleep` is good to about a millisecond, a spin to about a microsecond, so a sender sleeps until [`LEAD`] before the instant and spins the rest.
* **[`pick`] / [`name_matches`]** — which output port a setting names, and (feature `device`) [`open_output`] to open it through midir.

## Consumers

Used by `simply-link-midi` (the Link → MIDI clock binary) and flux's `midi_clock` module. Both send MIDI clock; `simply-midi` re-exports the port matcher so its input-opening uses the same rule.

## Depending on it

```toml
[dependencies]
simply-midi-clock = { git = "https://github.com/simply-audio/music-rs.git" }
```

Add `features = ["device"]` to open ports through midir.

## Licence

MIT — see [LICENSE](../../LICENSE).
