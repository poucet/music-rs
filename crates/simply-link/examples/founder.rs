//! A Link peer that founds a session on one interface and walks its tempo —
//! the "other machine" for proving `simply-link` follows a peer over a LAN
//! address rather than loopback, when only one machine is to hand.
//!
//!     cargo run -p simply-link --example founder -- 192.168.1.20 100 110 120
//!
//! Joins the real Link group on the interface with that address **only**
//! (so every announcement carries that address as its measurement endpoint
//! and nothing goes over loopback), with a multicast time-to-live of 0, so
//! the datagrams are delivered to this machine and never transmitted. It
//! founds a session at the first tempo, then moves to each next one every
//! 5 s, and leaves.

use std::net::Ipv4Addr;
use std::thread;
use std::time::Duration;

use simply_link::{Config, Link, Network};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((interface, tempi)) = args.split_first() else {
        eprintln!("usage: founder <interface address> <bpm>...");
        std::process::exit(2);
    };
    let interface: Ipv4Addr = interface.parse().expect("an IPv4 interface address");
    let tempi: Vec<f64> = tempi.iter().map(|t| t.parse().expect("a tempo")).collect();
    let first = *tempi.first().unwrap_or(&120.0);
    let net = Network { interface, multicast_ttl: 0, ..Network::default() };
    let link =
        Link::join_on(Config { found_after: Some(200_000), bpm: first, ..Config::default() }, net)
            .expect("join the Link group");
    for bpm in tempi {
        if link.state().joined() {
            link.propose_tempo(bpm, link.clock().now());
        }
        for _ in 0..5 {
            let s = link.state();
            println!(
                "founder on {interface}: {:?} {:.2} BPM, {} peer(s)",
                s.status,
                s.tempo(),
                s.peers
            );
            thread::sleep(Duration::from_secs(1));
        }
    }
}
