//! `simply-osc` — send an OSC message from the command line.
//!
//! The by-hand half of the crate: the library receives, this pushes. One
//! message per invocation, straight at Flux's listener (or anything else
//! speaking OSC 1.0 over UDP).
//!
//! ```text
//! simply-osc /flux/cutoff 0.5                  # one float, to 127.0.0.1:8000
//! simply-osc /flux/play                        # a bang (no arguments)
//! simply-osc /flux/drums true                  # T — rising edge for a launch row
//! simply-osc --port 9000 /flux/m1 0.25
//! simply-osc --to 192.168.1.7:8000 /flux/tempo 128
//! simply-osc /flux/name s:42                   # force a string argument
//! ```
//!
//! Types are inferred: integers become `i`, numbers with a point become
//! `f`, `true`/`false` become `T`/`F`, everything else is a string.
//! Prefixes force a type: `i:`, `f:`, `d:`, `h:`, `s:`.

use simply_osc::{Message, Value};
use std::net::UdpSocket;

fn usage() -> ! {
    eprintln!(
        "simply-osc — send one OSC message over UDP\n\
         \n\
         usage: simply-osc [options] /address [args…]\n\
         \n\
         options:\n\
           --to HOST:PORT   where to send (default 127.0.0.1:8000 — Flux's OSC input)\n\
           --port PORT      shorthand for --to 127.0.0.1:PORT\n\
           -v, --verbose    echo what went out (bytes, address, type tags)\n\
           -h, --help       this\n\
         \n\
         arguments — types are inferred:\n\
           3            → i  (int32)\n\
           0.5          → f  (float32)\n\
           true, false  → T, F\n\
           nil          → N\n\
           anything     → s  (string)\n\
         force a type with a prefix: i:3  f:3  d:0.5  h:3  s:0.5\n\
         no arguments at all is a bang — how a launch/randomize binding fires.\n\
         \n\
         examples:\n\
           simply-osc /flux/cutoff 0.5          a knob binding, full resolution\n\
           simply-osc /flux/play                a bang\n\
           simply-osc /flux/drums true          rising edge for a launch row\n\
           simply-osc /flux/tempo 128           transport = \"tempo\": the float IS the BPM\n\
           simply-osc --port 9000 /flux/m1 0.25 macro 1 on a non-default port\n\
         \n\
         enable OSC in Flux: Settings → OSC (or --osc). An address you send\n\
         that nothing is bound to shows there as 'last unmatched' — send\n\
         first, then bind what you see. Guide: docs/guides/osc.md"
    );
    std::process::exit(2);
}

fn parse_value(raw: &str) -> Result<Value, String> {
    if let Some(rest) = raw.strip_prefix("i:") {
        return rest.parse().map(Value::Int32).map_err(|e| format!("i:{rest}: {e}"));
    }
    if let Some(rest) = raw.strip_prefix("f:") {
        return rest.parse().map(Value::Float32).map_err(|e| format!("f:{rest}: {e}"));
    }
    if let Some(rest) = raw.strip_prefix("d:") {
        return rest.parse().map(Value::Double).map_err(|e| format!("d:{rest}: {e}"));
    }
    if let Some(rest) = raw.strip_prefix("h:") {
        return rest.parse().map(Value::Int64).map_err(|e| format!("h:{rest}: {e}"));
    }
    if let Some(rest) = raw.strip_prefix("s:") {
        return Ok(Value::Str(rest.to_owned()));
    }
    match raw {
        "true" => return Ok(Value::Bool(true)),
        "false" => return Ok(Value::Bool(false)),
        "nil" => return Ok(Value::Nil),
        _ => {}
    }
    if let Ok(i) = raw.parse::<i32>() {
        return Ok(Value::Int32(i));
    }
    if let Ok(f) = raw.parse::<f32>() {
        return Ok(Value::Float32(f));
    }
    Ok(Value::Str(raw.to_owned()))
}

fn main() {
    let mut to = String::from("127.0.0.1:8000");
    let mut verbose = false;
    let mut address: Option<String> = None;
    let mut values = Vec::new();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--to" => match args.next() {
                Some(v) => to = v,
                None => usage(),
            },
            "--port" => match args.next().and_then(|v| v.parse::<u16>().ok()) {
                Some(p) => to = format!("127.0.0.1:{p}"),
                None => usage(),
            },
            "--verbose" | "-v" => verbose = true,
            "--help" | "-h" => usage(),
            _ if address.is_none() => {
                if !arg.starts_with('/') {
                    eprintln!("an OSC address starts with '/': {arg}");
                    usage();
                }
                address = Some(arg);
            }
            _ => match parse_value(&arg) {
                Ok(v) => values.push(v),
                Err(e) => {
                    eprintln!("bad argument: {e}");
                    std::process::exit(2);
                }
            },
        }
    }
    let Some(address) = address else { usage() };

    let mut message = Message::new(address);
    for v in values {
        message = message.arg(v);
    }
    let bytes = match message.encode() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("cannot encode: {e:?}");
            std::process::exit(1);
        }
    };

    let socket = match UdpSocket::bind("0.0.0.0:0") {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot open a socket: {e}");
            std::process::exit(1);
        }
    };
    match socket.send_to(&bytes, &to) {
        Ok(n) => {
            if verbose {
                let tags: String = message.args.iter().map(|a| a.tag() as char).collect();
                eprintln!("sent {n} bytes to {to}: {} ,{tags}", message.address);
            }
        }
        Err(e) => {
            eprintln!("cannot send to {to}: {e}");
            std::process::exit(1);
        }
    }
}
