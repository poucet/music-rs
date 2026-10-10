//! The one test with a real socket (the `simply-link` precedent: loopback
//! only, port 0, nothing leaves this machine): a listener hears what a
//! plain `UdpSocket` sends it, parsed; counts what it cannot parse; and
//! stops when dropped.

use std::net::{Ipv4Addr, UdpSocket};
use std::time::{Duration, Instant};

use simply_osc::{Listener, Message, Packet, Value};

#[test]
fn a_listener_receives_parses_counts_junk_and_stops_on_drop() {
    let listener = Listener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind loopback port 0");
    let to = listener.local_addr();
    let sender = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("sender socket");

    // A valid message arrives typed.
    let sent = Message::new("/pad/x").arg(Value::Float32(0.5)).arg(Value::Int32(7));
    sender.send_to(&sent.encode().unwrap(), to).unwrap();
    let got = listener.recv_timeout(Duration::from_secs(5)).expect("the message");
    assert_eq!(got.packet, Packet::Message(sent));
    assert_eq!(got.from.port(), sender.local_addr().unwrap().port());

    // Junk is counted, not delivered, and does not wedge the stream.
    sender.send_to(b"not osc at all", to).unwrap();
    let after = Message::new("/after");
    sender.send_to(&after.encode().unwrap(), to).unwrap();
    let got = listener.recv_timeout(Duration::from_secs(5)).expect("the message after the junk");
    assert_eq!(got.packet, Packet::Message(after));
    let until = Instant::now() + Duration::from_secs(5);
    while listener.dropped() == 0 {
        assert!(Instant::now() < until, "the junk datagram was never counted");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(listener.dropped(), 1);

    // Drop joins the reader thread — bounded by its read timeout.
    let t = Instant::now();
    drop(listener);
    assert!(t.elapsed() < Duration::from_secs(2));
}
