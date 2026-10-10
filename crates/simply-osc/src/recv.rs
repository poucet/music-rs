//! A thin UDP receiver: bind, receive, parse, hand over a channel.
//!
//! This is the only socket in the crate, and deliberately the least of
//! it: one `UdpSocket`, one thread reading with a short timeout so drop
//! stops it within [`READ_TIMEOUT`], one `mpsc` channel of parsed
//! [`Inbound`] packets. Everything below — parsing, types, matching —
//! tests without it.
//!
//! Receive-only, by ruling: OSC is an *input* here. A malformed datagram
//! is counted ([`Listener::dropped`]) and otherwise ignored — on an open
//! UDP port anything can arrive, and one stray packet must not wedge the
//! stream.

use std::io;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::types::Packet;
use crate::wire::{MAX_DATAGRAM, parse};

/// How long one socket read blocks before checking whether to stop; the
/// upper bound on how long drop waits.
pub const READ_TIMEOUT: Duration = Duration::from_millis(100);

/// How many parsed packets queue before the oldest unread cost arrivals
/// their place: a consumer that stalls drops *new* packets (and counts
/// them) rather than growing without bound.
const QUEUE: usize = 1024;

/// One received packet and where it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct Inbound {
    pub packet: Packet,
    pub from: SocketAddr,
}

/// A bound OSC input: a socket, a reader thread, a channel of parsed
/// packets. Dropping it stops the thread.
pub struct Listener {
    local: SocketAddr,
    rx: Receiver<Inbound>,
    stop: Arc<AtomicBool>,
    dropped: Arc<AtomicU64>,
    thread: Option<JoinHandle<()>>,
}

impl Listener {
    /// Bind `addr` (e.g. `"0.0.0.0:9000"`) and start receiving.
    pub fn bind(addr: impl ToSocketAddrs) -> io::Result<Listener> {
        let socket = UdpSocket::bind(addr)?;
        socket.set_read_timeout(Some(READ_TIMEOUT))?;
        let local = socket.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicU64::new(0));
        let (tx, rx) = std::sync::mpsc::sync_channel(QUEUE);
        let thread = std::thread::Builder::new().name("simply-osc listener".into()).spawn({
            let stop = Arc::clone(&stop);
            let dropped = Arc::clone(&dropped);
            move || read_loop(&socket, &tx, &stop, &dropped)
        })?;
        Ok(Listener { local, rx, stop, dropped, thread: Some(thread) })
    }

    /// The address actually bound — the way to learn the port after
    /// binding port 0.
    pub fn local_addr(&self) -> SocketAddr {
        self.local
    }

    /// The next packet, blocking until one arrives or the listener's
    /// thread is gone. `None` means the socket failed and the thread
    /// exited — the listener is finished, not momentarily idle.
    pub fn recv(&self) -> Option<Inbound> {
        self.rx.recv().ok()
    }

    /// The next packet if one is queued; never blocks. The call a GUI
    /// tick makes.
    pub fn try_recv(&self) -> Option<Inbound> {
        self.rx.try_recv().ok()
    }

    /// The next packet, waiting at most `timeout`. `None` on timeout and
    /// on a finished listener alike; [`Listener::recv`] distinguishes.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<Inbound> {
        self.rx.recv_timeout(timeout).ok()
    }

    /// How many datagrams were discarded: malformed bytes, and packets
    /// that arrived while the queue was full.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn read_loop(socket: &UdpSocket, tx: &SyncSender<Inbound>, stop: &AtomicBool, dropped: &AtomicU64) {
    let mut buf = vec![0u8; MAX_DATAGRAM];
    while !stop.load(Ordering::Relaxed) {
        let (len, from) = match socket.recv_from(&mut buf) {
            Ok(got) => got,
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {
                continue;
            }
            // A real socket error: the listener is finished. Dropping
            // the sender hangs up the channel, which is how the consumer
            // learns.
            Err(_) => return,
        };
        match parse(&buf[..len]) {
            Ok(packet) => match tx.try_send(Inbound { packet, from }) {
                Ok(()) => {}
                // Queue full: the consumer is stalled; drop and count.
                Err(TrySendError::Full(_)) => {
                    dropped.fetch_add(1, Ordering::Relaxed);
                }
                // Consumer gone: stop reading.
                Err(TrySendError::Disconnected(_)) => return,
            },
            Err(_) => {
                dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}
