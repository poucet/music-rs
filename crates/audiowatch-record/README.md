# audiowatch-record

Record any channel, or adjacent pair of channels, of any device to 32-bit float WAV — what arrives at it, or (where the OS allows) what the computer sends to it.

Built for an agent first: everything here is non-interactive, a recording has a length or an explicit stop, and what comes back is a finished file by absolute path — never the audio itself, which an agent hands on to whatever measures it.

**For a length**: `record(takes, seconds)` returns when it is done. **For a window of host time**: `Recording::start(takes, None)` runs until `set_window(start_ns, end_ns)` arrives and the take is trimmed to it. For a MIDI clock's downbeats, say. The `audiowatch` binary serves it over MCP (`audiowatch --mcp`) and on the command line (`audiowatch rec`).

## What it covers

* **Take specs** ([`TakeSpec`]): a device by name, which direction (`in` or `out`), which channels (`3` or `3-4`), and an optional path.
* **Catalog** ([`list_devices`]): what the machine has, and which direction of each device can be recorded. An `out` tap is a CoreAudio process tap (macOS 14.2+), so only a device with no input side is tappable. The catalog says why not and what to do instead (route through BlackHole or Loopback and record that `in`).
* **Sessions** ([`Recording`]): open takes together, stop them together — after a length, or trimmed to a [`TakeWindow`] of host time that the caller supplies once it knows it.
* **Picking channels** ([`frames`]) and the WAV writer thread ([`wav`]).

Nothing here knows about MIDI. What decides a window — a MIDI clock, a Link session, a cue — is the caller's; this crate only keeps the audio that falls inside it, to the frame.

Nothing here ever opens an **output** stream. Recording what goes out to a speaker is a CoreAudio process tap read like an input; every stream, in or out, is a `simply_audio_capture` capture.

## Consumers

Used by `audiowatch --mcp` and `audiowatch rec` (the MCP and command-line surfaces of the recorder). audiowatch is the only consumer.

## Depending on it

```toml
[dependencies]
audiowatch-record = { git = "https://github.com/simply-audio/music-rs.git" }
```

## Licence

MIT — see [LICENSE](../../LICENSE).
