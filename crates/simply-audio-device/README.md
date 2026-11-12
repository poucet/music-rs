# simply-audio-device

Thin, app-agnostic helpers over cpal's device layer: enumerate device names, resolve a device by its persisted name (or fall back to the system default), and fetch an f32 stream config. Everything here is written once and takes a [`Direction`], because input and output differ only in which cpal call answers the question.

The point of the crate is to keep the cpal 0.18 quirks in one place: human-readable names come from `Device::description().name()` (the 0.15-era `Device::name()` is gone), and enumeration errors are folded into "no devices" because a missing backend and an empty backend look the same to a settings dialog. Stream building stays with the caller — what runs inside the callback is the app's business.

## Input is not output backwards

An output device sets the rate: the app renders at whatever the device opened at. An **input** device has to be asked for a rate, because it is the second stream on a machine that is already playing at one, and two clocks running at different nominal rates is a pitch shift, not a latency. So [`f32_input_config`] takes the rate it must match and fails when the device cannot be held to it, rather than silently opening at another one.

## Consumers

Used by `audiowatch-record` (the `audiowatch` recorder's device side), by flux's GUI for its audio input and output menus, and by tussen (pinned by rev).

## Depending on it

```toml
[dependencies]
simply-audio-device = { git = "https://github.com/simply-audio/music-rs.git" }
```

## Licence

MIT — see [LICENSE](../../LICENSE).
