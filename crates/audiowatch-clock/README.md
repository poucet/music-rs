# audiowatch-clock

Recording in time with a MIDI clock: read its downbeats off a MIDI input and trim takes to the bars between them.

Built for `audiowatch rec --bars N`, where a MIDI clock (from a drum machine, groovebox or DAW) decides when the bars are. `audiowatch` uses this to land takes on the next downbeat and end them exactly N bars later, counted in clock ticks — a tempo change mid-take still ends on the bar.

* **Where the bar is**: MIDI clock carries ticks (24 per beat), not bars. The bar is counted from the clock's Start (or Song Position + Continue). A clock that was already running when the take armed has no bar, so the take waits for the next Start.
* **The downbeat fit**: a least-squares fit through the ticks half a beat either side of the downbeat, averaging MIDI's arrival jitter. It is mapped to a frame through each buffer's capture time. Both are on macOS's host clock. The file is trimmed to it.
* **The tempo**: mean BPM over the take from the ticks, with the slowest and fastest beat.

## What it does

* [`reading`] — pure: the clock messages, where the bar is, the tempo, and the least-squares downbeat fit. Tested against synthetic timelines with no device.
* [`port`] — which MIDI input is sending clock, and listening to it. Input only: nothing here opens a MIDI output or sends a byte.
* [`take`] — a take of N bars: a windowed [`audiowatch_record::Recording`] given its window by the clock.

Nothing here plays anything or opens an output stream.

## Consumers

Used by `audiowatch --mcp` (the MCP recorder) and `audiowatch rec --bars` (the command line). audiowatch is the only consumer.

## Depending on it

```toml
[dependencies]
audiowatch-clock = { git = "https://github.com/simply-audio/music-rs.git" }
```

## Licence

MIT — see [LICENSE](../../LICENSE).
