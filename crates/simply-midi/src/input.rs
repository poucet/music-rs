//! Live MIDI input via midir: enumerate ports, open them, and hand decoded
//! [`MidiMessage`]s to a per-device handler.
//!
//! Ports are opened once, when the caller asks; there is no hotplug watcher
//! (a shell that wants one re-opens on its own schedule). Every entry point
//! takes the midir **client name** the ports are registered under — the one
//! thing about a port that belongs to the host application, not to MIDI.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use midir::{Ignore, MidiInput, MidiInputConnection};

use simply_midi_clock::name_matches;

use crate::message::{MidiMessage, source_name};

/// Everything that can go wrong reaching the machine's MIDI ports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiError(pub String);

impl std::fmt::Display for MidiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MidiError {}

/// One MIDI input port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiPort {
    /// midir's stable port id (survives re-enumeration).
    pub id: String,
    /// The port's display name — what a mapping's `device` filter matches.
    pub name: String,
}

fn client(client_name: &str) -> Result<MidiInput, MidiError> {
    let mut input =
        MidiInput::new(client_name).map_err(|e| MidiError(format!("no MIDI client: {e}")))?;
    // Timing/active-sensing/sysex traffic never maps to anything we do.
    input.ignore(Ignore::All);
    Ok(input)
}

/// The MIDI input ports this machine currently offers, enumerated under
/// `client_name`.
pub fn input_ports(client_name: &str) -> Result<Vec<MidiPort>, MidiError> {
    let input = client(client_name)?;
    Ok(input
        .ports()
        .iter()
        .map(|port| MidiPort {
            id: port.id(),
            name: input.port_name(port).unwrap_or_else(|_| "<unnamed>".into()),
        })
        .collect())
}

/// Open input ports; dropping this closes them.
#[derive(Default)]
pub struct MidiListener {
    /// Held only to keep the connections alive.
    _connections: Vec<MidiInputConnection<()>>,
    /// Names of the ports actually opened.
    pub devices: Vec<String>,
    /// Ports that matched the selection but could not be opened.
    pub failures: Vec<String>,
}

/// Open every input port whose name contains `select` (case-insensitive;
/// `None` opens all of them), building one handler per port from its name.
///
/// The handler runs on midir's own thread — not the audio thread — so it may
/// lock and allocate; it must push its work onto a realtime ring for the
/// audio callback to drain.
pub fn open_inputs<H>(
    client_name: &str,
    select: Option<&str>,
    mut make_handler: impl FnMut(&str) -> H,
) -> Result<MidiListener, MidiError>
where
    H: FnMut(MidiMessage) + Send + 'static,
{
    let ports = input_ports(client_name)?;
    let mut connections = Vec::new();
    let mut devices = Vec::new();
    let mut failures = Vec::new();
    for port in ports {
        if select.is_some_and(|w| !name_matches(&port.name, w)) {
            continue;
        }
        let input = client(client_name)?;
        let Some(handle) = input.find_port_by_id(&port.id) else {
            failures.push(format!("{}: port disappeared", port.name));
            continue;
        };
        let mut handler = make_handler(&port.name);
        match input.connect(
            &handle,
            &port.name,
            move |_timestamp, bytes, ()| {
                if let Some(message) = MidiMessage::decode(bytes) {
                    handler(message);
                }
            },
            (),
        ) {
            Ok(connection) => {
                connections.push(connection);
                devices.push(port.name);
            }
            Err(e) => failures.push(format!("{}: {e}", port.name)),
        }
    }
    Ok(MidiListener { _connections: connections, devices, failures })
}

/// The controller that moved most during an [`identify`] window.
#[derive(Debug, Clone, PartialEq)]
pub struct CcObservation {
    pub device: String,
    /// Source code — a CC number, or the pitch-bend / aftertouch sentinel.
    pub source: u16,
    /// How it is spelled on the wire: `"cc74"`, `"pitch_bend"`, `"aftertouch"`.
    pub name: String,
    /// Widest travel seen, `0..=1` — how the winner is chosen.
    pub span: f32,
    /// Where it ended up, `0..=1`.
    pub value: f32,
    /// How many messages it sent.
    pub events: usize,
}

#[derive(Default, Clone, Copy)]
struct Observation {
    min: f32,
    max: f32,
    last: f32,
    events: usize,
}

impl Observation {
    fn record(&mut self, travel: f32) {
        if self.events == 0 {
            self.min = travel;
            self.max = travel;
        } else {
            self.min = self.min.min(travel);
            self.max = self.max.max(travel);
        }
        self.last = travel;
        self.events += 1;
    }
}

/// Wiggle-to-identify: listen for `seconds`, then report the controller that
/// travelled furthest — so "map the knob I'm turning" needs no hardware
/// MIDI-learn mode. `None` when nothing moved.
pub fn identify(client_name: &str, seconds: f32) -> Result<Option<CcObservation>, MidiError> {
    let seen: Arc<Mutex<HashMap<(String, u16), Observation>>> = Arc::default();
    let listener = open_inputs(client_name, None, |device| {
        let seen = Arc::clone(&seen);
        let device = device.to_owned();
        move |message: MidiMessage| {
            let Some((source, travel)) = message.source() else { return };
            if let Ok(mut seen) = seen.lock() {
                seen.entry((device.clone(), source)).or_default().record(travel);
            }
        }
    })?;
    if listener.devices.is_empty() {
        return Err(MidiError(format!(
            "no MIDI input device could be opened{}",
            match listener.failures.len() {
                0 => String::new(),
                _ => format!(" ({})", listener.failures.join("; ")),
            }
        )));
    }
    std::thread::sleep(Duration::from_secs_f32(seconds.clamp(0.1, 60.0)));
    drop(listener);

    let seen = seen.lock().map_err(|_| MidiError("observer thread panicked".into()))?;
    Ok(seen
        .iter()
        .max_by(|a, b| {
            (a.1.max - a.1.min).total_cmp(&(b.1.max - b.1.min)).then(a.1.events.cmp(&b.1.events))
        })
        .map(|((device, source), obs)| CcObservation {
            device: device.clone(),
            source: *source,
            name: source_name(*source),
            span: obs.max - obs.min,
            value: obs.last,
            events: obs.events,
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Enumeration must never panic or hang on a machine with no MIDI at all
    /// (CI, a headless build box) — it returns a list or a readable error.
    #[test]
    fn enumerating_ports_is_safe_without_hardware() {
        match input_ports("simply-midi-test") {
            Ok(ports) => {
                for port in ports {
                    assert!(!port.id.is_empty());
                }
            }
            Err(e) => assert!(!e.0.is_empty(), "errors must say something"),
        }
    }

    #[test]
    fn observations_track_span_and_last_value() {
        let mut obs = Observation::default();
        obs.record(0.5);
        obs.record(0.1);
        obs.record(0.9);
        assert_eq!((obs.min, obs.max, obs.last, obs.events), (0.1, 0.9, 0.9, 3));
    }
}
