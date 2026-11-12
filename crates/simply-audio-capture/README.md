# simply-audio-capture

Capture any audio device by name, inputs and outputs alike, through one call.

The rule ([`resolve`]):

* a device with **input channels** is captured as an input, through cpal;
* an **output-only** device (speakers, headphones, a display, a Multi-Output Device) is captured by tapping what the Mac plays on it: a CoreAudio process tap of every process, mixed to stereo and **not muted**, inside a private aggregate device this process reads like an input (macOS 14.2+);
* `""` is the current default output, tapped, and **followed**: when the Mac's default output changes mid-tap the tap moves to the new device (a clean restart, the same sink) and the stream carries on.

Either way the device runs at its own rate (nothing on the machine is retuned) and the frames are resampled to the rate asked for, if it differs, before they reach the sink.

```rust,no_run
let devices = simply_audio_capture::list_devices();
let source = simply_audio_capture::resolve(&devices, "")?; // the default output
let capture = simply_audio_capture::start(&source, 48_000, 2, |frames: &[f32], _ns: u64| {
    // interleaved stereo at 48 kHz; runs on the audio thread
})?;
std::thread::sleep(std::time::Duration::from_secs(2));
drop(capture); // stops; the tap and its aggregate device are destroyed
# Ok::<(), simply_audio_capture::Error>(())
```

## Tapping applications

[`start_app`] taps **applications** rather than a device, so an app's audio reaches this process without Loopback or BlackHole (macOS 14.2+):

* [`Apps::Only`]: these apps (by bundle id, executable name or pid; one or several), mixed to stereo, wherever they play;
* [`Apps::AllExcept`]: every process except these (leave yourself out, [`App::This`], so your own output doesn't feed back).

[`App::PlayingInto`] names processes by the output device they play into (a device UID prefix), and is re-read the moment one moves: a program with its own virtual devices leaves out what already reaches it through them, so the tap doesn't carry it twice.

The capture **follows** the apps. While none of them runs it reports [`AppStatus::Waiting`]; it attaches when one starts, and makes a new tap when one restarts. A tapped app that exits, a clock device that disappears, or a tap that stops calling back is reported in [`AppCapture::status`] and [`AppCapture::error`], never as silence.

[`start_app_channels`] takes the same [`AppTap`] but keeps the apps apart: each process gets a tap of its own, from when it starts playing until it exits or has been quiet for [`CHANNEL_HOLD`]. An [`AppSinks`] hears each channel start (and hands back its sink) and stop; [`AppChannels::status`] lists them.

```rust,no_run
use simply_audio_capture::{App, AppTap, Mute};
let tap = AppTap::only([App::parse("com.algoriddim.djay-iphone-free")]).with_mute(Mute::Unmuted);
let capture = simply_audio_capture::start_app(&tap, 48_000, 2, |frames: &[f32], _ns: u64| {
    // the same sink as start's: interleaved stereo at 48 kHz, host-clock ns
})?;
println!("{}", capture.status()); // "waiting for com.algoriddim.djay-iphone-free", or "tapping djay Pro (pid 412) at 48000 Hz"
# Ok::<(), simply_audio_capture::Error>(())
```

## Platform

The taps are **macOS 14.2+ only**. Off macOS the code compiles and devices are captured like any other cpal project, but `Tap` is never resolved and app taps fail at once. `cargo check --workspace --all-targets --target x86_64-pc-windows-msvc` is the check.

## Consumers

Used by `audiowatch-record` (which the `audiowatch` recorder uses) and tussen (pinned by rev). Both capture devices; tussen also taps applications (every app except tussen and its virtual devices).

## Depending on it

```toml
[dependencies]
simply-audio-capture = { git = "https://github.com/simply-audio/music-rs.git" }
```

## Licence

MIT — see [LICENSE](../../LICENSE).
