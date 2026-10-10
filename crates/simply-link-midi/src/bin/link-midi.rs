//! `link-midi` — an Ableton Link session out as MIDI beat clock, for a
//! drum machine, a modular's MIDI→clock interface, anything that follows
//! `F8`. Every word of timing logic is in the library; this file parses
//! arguments, opens the port and prints.

use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::json;
use simply_link::wire::{Discovery, GROUP, PORT};
use simply_link::{Config, Link, Network, SessionState, Status, discovery_listener};
use simply_link_midi::{ClockEvent, Schedule, Settings, Sink, Transport, run};
use simply_midi_clock::{MidiOutputConnection, open_output, output_port_names};

/// The name this program registers with the MIDI system.
const CLIENT: &str = "link-midi";

const USAGE: &str = "\
link-midi — an Ableton Link session out as MIDI beat clock

USAGE
  link-midi ports   [--json]
  link-midi session [--seconds N] [--json]
  link-midi listen  [--seconds N] [--json]
  link-midi run     --port NAME [--quantum BEATS] [--latency-ms MS]
                    [--transport sync|free] [--dry-run]

COMMANDS
  ports    list this machine's MIDI output ports
  session  join the Link session on this network as a peer and report it —
           tempo, peers, phase, whether it is playing. Proposes nothing.
  listen   hear the Link multicast group without joining it, and print every
           announcement decoded (with --json, its raw bytes too)
  run      join the session and send MIDI clock to PORT until Ctrl-C:
           24 clocks a beat, and — on the next quantum boundary — Song
           Position 0 and Start, so the device's bar one is the session's
           downbeat. Stop when the session stops (or on Ctrl-C).

OPTIONS
  --port NAME       the output port, by exact name or an unambiguous part
  --quantum BEATS   beats per bar the Start is aligned to (default 4)
  --latency-ms MS   how long the device takes to act on a clock; everything
                    is sent that much early (default 0; may be fractional)
  --transport sync  start/stop with the session's Start Stop Sync (default)
  --transport free  run whenever joined, ignoring the session's transport
  --dry-run         print what would be sent, with times, and open no port
  --seconds N       how long `session` / `listen` wait (default 5)

This peer never founds a session and never changes the tempo or the
transport: it follows. With no Link app on the network it waits.";

#[derive(Debug)]
enum Command {
    Ports { json: bool },
    Session { seconds: f64, json: bool },
    Listen { seconds: f64, json: bool },
    Run { port: Option<String>, settings: Settings, dry_run: bool },
}

fn number(it: &mut std::slice::Iter<String>, flag: &str) -> Result<f64, String> {
    let v = it.next().ok_or(format!("{flag} needs a value"))?;
    let n: f64 = v.parse().map_err(|_| format!("{flag} {v:?} is not a number"))?;
    if !n.is_finite() {
        return Err(format!("{flag} must be finite"));
    }
    Ok(n)
}

fn parse(args: &[String]) -> Result<Command, String> {
    let mut it = args.iter();
    let command = it.next().map(String::as_str);
    let (mut json, mut seconds, mut port, mut dry_run) = (false, 5.0, None, false);
    let mut settings = Settings::default();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--dry-run" => dry_run = true,
            "--seconds" => {
                seconds = number(&mut it, arg)?;
                if seconds <= 0.0 {
                    return Err("--seconds must be positive".into());
                }
            }
            "--port" => port = Some(it.next().ok_or("--port needs a value")?.clone()),
            "--quantum" => {
                settings.quantum = number(&mut it, arg)?;
                if settings.quantum <= 0.0 {
                    return Err("--quantum must be positive".into());
                }
            }
            "--latency-ms" => settings.latency = (number(&mut it, arg)? * 1000.0).round() as i64,
            "--transport" => {
                settings.transport = match it.next().map(String::as_str) {
                    Some("sync") => Transport::Sync,
                    Some("free") => Transport::Free,
                    other => return Err(format!("--transport is sync or free, not {other:?}")),
                }
            }
            "--help" | "-h" => return Err(USAGE.into()),
            other => return Err(format!("unknown argument {other}\n\n{USAGE}")),
        }
    }
    match command {
        Some("ports") => Ok(Command::Ports { json }),
        Some("session") => Ok(Command::Session { seconds, json }),
        Some("listen") => Ok(Command::Listen { seconds, json }),
        Some("run") => {
            if port.is_none() && !dry_run {
                return Err("run needs --port NAME (see `link-midi ports`), or --dry-run".into());
            }
            Ok(Command::Run { port, settings, dry_run })
        }
        _ => Err(USAGE.into()),
    }
}

fn describe(state: &SessionState, now: i64) -> serde_json::Value {
    json!({
        "status": match state.status {
            Status::Searching => "searching",
            Status::Measuring => "measuring",
            Status::Joined => "joined",
        },
        "session": state.session.map(|s| s.to_string()),
        "peers": state.peers,
        "tempo": state.joined().then(|| state.tempo()),
        "beat": state.joined().then(|| state.beat_at(now)),
        "phase_4": state.joined().then(|| state.phase_at(now, 4.0)),
        "playing": state.is_playing(),
        "ghost_offset_us": state.joined().then_some(state.xform.intercept),
    })
}

fn line(state: &SessionState, now: i64) -> String {
    match state.status {
        Status::Searching => "searching — no Link session heard yet".into(),
        Status::Measuring => "measuring the session's clock…".into(),
        Status::Joined => format!(
            "{:.2} BPM · beat {:.2} · phase {:.2}/4 · {} peer(s) · {}",
            state.tempo(),
            state.beat_at(now),
            state.phase_at(now, 4.0),
            state.peers,
            match state.is_playing() {
                Some(true) => "playing",
                Some(false) => "stopped",
                None => "no start/stop sync",
            }
        ),
    }
}

fn session(seconds: f64, json: bool) -> Result<(), String> {
    let link = Link::join(Config::default()).map_err(|e| format!("could not join Link: {e}"))?;
    let until = Instant::now() + Duration::from_secs_f64(seconds);
    if !json {
        println!("joined the Link group as {} — listening {seconds} s", link.node());
    }
    while Instant::now() < until {
        thread::sleep(Duration::from_millis(500));
        if !json {
            println!("{}", line(&link.state(), link.clock().now()));
        }
    }
    if json {
        println!("{}", describe(&link.state(), link.clock().now()));
    }
    Ok(())
}

/// Hear the group without joining — a bare socket, nothing ever sent. It is
/// joined on every interface, loopback included, because a Link app on this
/// machine announces to its neighbours over loopback. With `--json`, one
/// object at the end — `{"interfaces", "seconds", "announcements"}` — so
/// hearing nothing is an empty list, not an empty output.
fn listen(seconds: f64, json: bool) -> Result<(), String> {
    let (socket, joined) = discovery_listener(&Network::default())
        .map_err(|e| format!("could not join the Link group {GROUP}:{PORT}: {e}"))?;
    let interfaces: Vec<String> = joined.iter().map(|i| i.addr.to_string()).collect();
    if !json {
        println!("listening on {} for {seconds} s", interfaces.join(", "));
    }
    let until = Instant::now() + Duration::from_secs_f64(seconds);
    let mut buf = [0u8; 2048];
    let (mut count, mut heard) = (0usize, Vec::new());
    while Instant::now() < until {
        let Ok((len, from)) = socket.recv_from(&mut buf) else { continue };
        count += 1;
        let bytes = &buf[..len];
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ");
        match Discovery::decode(bytes) {
            Ok(m) if json => heard.push(json!({
                "from": from.to_string(), "kind": format!("{:?}", m.kind), "node": m.node.to_string(),
                "session": m.session().map(|s| s.to_string()), "tempo": m.timeline().map(|t| t.bpm()),
                "playing": m.start_stop().map(|s| s.playing), "endpoint": m.endpoint().map(|e| e.to_string()),
                "entries": m.entries.len(), "bytes": hex,
            })),
            Ok(m) => println!(
                "{from} {:?} node {} session {} {} {} mep4 {}",
                m.kind,
                m.node,
                m.session().map_or("-".into(), |s| s.to_string()),
                m.timeline().map_or(String::new(), |t| format!("{:.2} BPM", t.bpm())),
                m.start_stop().map_or(String::new(), |s| format!("playing={}", s.playing)),
                m.endpoint().map_or("-".into(), |e| e.to_string()),
            ),
            Err(e) if json => heard.push(json!({
                "from": from.to_string(), "error": e.to_string(), "bytes": hex,
            })),
            Err(e) => println!("{from} undecodable ({e}): {hex}"),
        }
    }
    if json {
        println!(
            "{}",
            json!({ "interfaces": interfaces, "seconds": seconds, "announcements": heard })
        );
    } else {
        println!("{count} announcement(s) in {seconds} s");
    }
    Ok(())
}

struct Port(MidiOutputConnection);
impl Sink for Port {
    fn send(&mut self, event: ClockEvent, _at: i64) {
        let (bytes, len) = event.bytes();
        let _ = self.0.send(&bytes[..len]);
    }
}

/// `--dry-run`: say what would go out, and when, relative to the start —
/// every message but the clocks, and one line a beat for those.
struct Print {
    start: i64,
    ticks: u64,
}
impl Sink for Print {
    fn send(&mut self, event: ClockEvent, at: i64) {
        let ms = (at - self.start) as f64 / 1000.0;
        if event == ClockEvent::Tick {
            if self.ticks.is_multiple_of(24) {
                println!("{ms:>10.3} ms  Tick (beat {})", self.ticks / 24);
            }
            self.ticks += 1;
        } else {
            if event == ClockEvent::Start {
                self.ticks = 0;
            }
            println!("{ms:>10.3} ms  {event:?}");
        }
    }
}

fn run_bridge(port: Option<String>, settings: Settings, dry_run: bool) -> Result<(), String> {
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = Arc::clone(&stop);
        ctrlc::set_handler(move || stop.store(true, Ordering::Relaxed))
            .map_err(|e| format!("could not catch Ctrl-C: {e}"))?;
    }
    let link = Link::join(Config::default()).map_err(|e| format!("could not join Link: {e}"))?;
    let clock = link.clock();
    let mut schedule = Schedule::new(settings);
    eprintln!(
        "link-midi: quantum {} · latency {} ms · transport {:?} — waiting for a Link session (Ctrl-C to stop)",
        settings.quantum,
        settings.latency as f64 / 1000.0,
        settings.transport
    );
    let session = || link.state();
    if dry_run {
        let mut sink = Print { start: clock.now(), ticks: 0 };
        run(session, clock, &mut schedule, &mut sink, &stop);
    } else {
        let port = port.expect("checked in parse");
        let connection = open_output(CLIENT, &port)?;
        eprintln!("link-midi: clock → {port}");
        let mut sink = Port(connection);
        run(session, clock, &mut schedule, &mut sink, &stop);
    }
    eprintln!("link-midi: stopped");
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = match parse(&args) {
        Ok(c) => c,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    let result = match command {
        Command::Ports { json } => {
            let names = output_port_names(CLIENT);
            if json {
                println!("{}", json!({ "ports": names }));
            } else if names.is_empty() {
                println!("no MIDI output ports");
            } else {
                names.iter().for_each(|n| println!("{n}"));
            }
            Ok(())
        }
        Command::Session { seconds, json } => session(seconds, json),
        Command::Listen { seconds, json } => listen(seconds, json),
        Command::Run { port, settings, dry_run } => run_bridge(port, settings, dry_run),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("link-midi: {message}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn parses_a_run() {
        let Command::Run { port, settings, dry_run } =
            parse(&args("run --port Clock --quantum 3 --latency-ms 2.5 --transport free")).unwrap()
        else {
            panic!()
        };
        assert_eq!(port.as_deref(), Some("Clock"));
        assert_eq!(settings.quantum, 3.0);
        assert_eq!(settings.latency, 2500);
        assert_eq!(settings.transport, Transport::Free);
        assert!(!dry_run);
    }

    #[test]
    fn refuses_what_it_cannot_do() {
        assert!(parse(&args("run")).unwrap_err().contains("--port"));
        assert!(parse(&args("run --dry-run")).is_ok());
        assert!(parse(&args("run --port x --quantum 0")).is_err());
        assert!(parse(&args("run --port x --transport maybe")).is_err());
        assert!(parse(&args("session --seconds -1")).is_err());
        assert!(parse(&args("nonsense")).is_err());
    }
}
