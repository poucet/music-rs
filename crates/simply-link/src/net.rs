//! The engine on real sockets.
//!
//! **A peer is on every interface at once.** Multicast group membership is
//! per interface: a socket joined on Wi-Fi hears nothing another app on the
//! same machine sends over loopback, and a Link app announces to its
//! neighbours on this machine over loopback. So the group is joined on
//! every up, non-point-to-point IPv4 interface ([`interfaces`]) — loopback
//! included — every announcement goes out of each of them carrying that
//! interface's own address as its `mep4`, and the list is re-read every
//! [`RESCAN`] so a network that comes up later is joined too.
//! [`Network::interface`] narrows all of it to one interface.
//!
//! Four sockets, each read by its own thread with a short timeout so the
//! whole peer shuts down within one:
//!
//! * **discovery** — bound to the Link port with address and port reuse, so
//!   every Link app on this machine hears the group, and joined to it on
//!   each interface;
//! * **sender** — an ephemeral port every multicast and unicast leaves
//!   from, which also hears the unicast `Response`s peers send back to it
//!   (answering to the shared discovery port would reach only one of the
//!   several sockets bound there); a multicast leaves it once per
//!   interface, the outgoing interface set per send under a lock;
//! * **responder** — the measurement endpoint announced as `mep4`, bound to
//!   every address so each interface's `mep4` reaches it: answers every
//!   ping with this peer's ghost time, once it has one;
//! * a **ticker**, driving [`Engine::tick`] every [`TICK`] and re-reading
//!   the interfaces every [`RESCAN`].
//!
//! A measurement runs on a thread of its own with its own socket, since a
//! burst takes a few hundred milliseconds and nothing should wait on it.

use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use socket2::{Domain, Protocol, SockRef, Socket, Type};

use crate::clock::HostClock;
use crate::measure;
use crate::session::{Action, Config, Engine, SessionState};
use crate::timeline::Micros;
use crate::wire::{Discovery, GROUP, MAX_MESSAGE, NodeId, PORT, SessionId};

/// How often the engine is ticked.
const TICK: Duration = Duration::from_millis(20);
/// How long a socket read blocks before checking whether to stop.
const READ_TIMEOUT: Duration = Duration::from_millis(100);
/// How often the machine's interfaces are re-read, so a network that comes
/// up after the peer started (Wi-Fi joining, a cable plugged in) is joined.
pub const RESCAN: Duration = Duration::from_secs(5);

/// Where the peer talks. [`Network::default`] is the real Link group on
/// every interface this machine has, loopback included.
#[derive(Copy, Clone, Debug)]
pub struct Network {
    pub group: Ipv4Addr,
    pub port: u16,
    /// Restrict the peer to the interface with this address; unspecified —
    /// every interface ([`interfaces`]).
    pub interface: Ipv4Addr,
    /// The multicast time-to-live. 1 (the operating system's default)
    /// reaches the local network and no further; 0 keeps every datagram on
    /// this machine, which is what a test wants.
    pub multicast_ttl: u32,
}

impl Default for Network {
    fn default() -> Self {
        Network { group: GROUP, port: PORT, interface: Ipv4Addr::UNSPECIFIED, multicast_ttl: 1 }
    }
}

/// One IPv4 interface the peer is on: its address, which is what the
/// `mep4` announced over it says, and its netmask, which says which peers
/// reach us through it.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Interface {
    pub addr: Ipv4Addr,
    pub netmask: Ipv4Addr,
}

impl Interface {
    fn contains(&self, ip: Ipv4Addr) -> bool {
        let mask = u32::from(self.netmask);
        u32::from(self.addr) & mask == u32::from(ip) & mask
    }
}

/// What enumeration says about one address — the input [`select`] decides
/// over, kept apart from the OS call so the decision can be tested.
#[derive(Copy, Clone, Debug)]
struct Candidate {
    addr: Ipv4Addr,
    netmask: Ipv4Addr,
    up: bool,
    point_to_point: bool,
}

/// The interfaces a Link peer belongs on: every IPv4 address that is up,
/// loopback **included** (Link apps on one machine find each other there),
/// point-to-point links left out (a VPN tunnel is not a local network, and
/// most drop multicast anyway), each address once, in address order.
fn select(candidates: impl IntoIterator<Item = Candidate>) -> Vec<Interface> {
    let mut out: Vec<Interface> = candidates
        .into_iter()
        .filter(|c| c.up && !c.point_to_point && !c.addr.is_unspecified())
        .map(|c| Interface { addr: c.addr, netmask: c.netmask })
        .collect();
    out.sort();
    out.dedup_by_key(|i| i.addr);
    out
}

/// The interfaces `net` puts the peer on: the one [`Network::interface`]
/// names, or every one [`select`] keeps. Should enumeration fail or find
/// nothing, the interface the default route leaves by, then loopback — a
/// peer that is on no interface at all would be silently deaf.
pub fn interfaces(net: &Network) -> Vec<Interface> {
    if !net.interface.is_unspecified() {
        return vec![Interface { addr: net.interface, netmask: Ipv4Addr::BROADCAST }];
    }
    let found = if_addrs::get_if_addrs()
        .map(|all| {
            select(all.into_iter().filter_map(|i| match i.addr {
                if_addrs::IfAddr::V4(ref v4) => Some(Candidate {
                    addr: v4.ip,
                    netmask: v4.netmask,
                    up: i.is_oper_up() || v4.ip.is_loopback(),
                    point_to_point: i.is_p2p(),
                }),
                if_addrs::IfAddr::V6(_) => None,
            }))
        })
        .unwrap_or_default();
    if !found.is_empty() {
        return found;
    }
    let default_route = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .and_then(|s| {
            s.connect((net.group, net.port))?;
            s.local_addr()
        })
        .ok()
        .and_then(|a| match a {
            SocketAddr::V4(v4) if !v4.ip().is_unspecified() => Some(*v4.ip()),
            _ => None,
        })
        .unwrap_or(Ipv4Addr::LOCALHOST);
    vec![Interface { addr: default_route, netmask: Ipv4Addr::BROADCAST }]
}

/// The address a peer at `peer` reaches us at — what the `mep4` in a
/// unicast answer to it must say. Our own address if the peer is using it
/// (it is on this machine); loopback for a loopback peer; the interface
/// whose network holds the peer; failing all of those, the first
/// non-loopback interface.
pub fn address_for(interfaces: &[Interface], peer: Ipv4Addr) -> Option<Ipv4Addr> {
    let pick = |f: &dyn Fn(&Interface) -> bool| interfaces.iter().find(|i| f(i)).map(|i| i.addr);
    pick(&|i| i.addr == peer)
        .or_else(|| pick(&|i| peer.is_loopback() && i.addr.is_loopback()))
        .or_else(|| pick(&|i| !i.addr.is_loopback() && i.contains(peer)))
        .or_else(|| pick(&|i| !i.addr.is_loopback()))
        .or_else(|| interfaces.first().map(|i| i.addr))
}

/// Join `group` on each of `interfaces` that will have it, and say which did.
///
/// Membership is per *interface*, not per address, so "already a member"
/// counts as joined: a second address on the same interface (an alias), or
/// the new address Wi-Fi comes back with after rejoining — the membership
/// taken through the old one outlives it, and the leave through the old
/// address fails because that address is gone. Taken as a failure, that
/// interface would be retried every [`RESCAN`] and never announced on again.
fn join(socket: &UdpSocket, group: Ipv4Addr, interfaces: &[Interface]) -> Vec<Interface> {
    interfaces
        .iter()
        .copied()
        .filter(|i| joined(socket.join_multicast_v4(&group, &i.addr)))
        .collect()
}

/// Whether a join's result leaves the socket a member: it worked, or the
/// interface had the group already.
fn joined(result: io::Result<()>) -> bool {
    match result {
        Ok(()) => true,
        Err(e) => e.kind() == io::ErrorKind::AddrInUse,
    }
}

struct Shared {
    engine: Mutex<Engine>,
    clock: HostClock,
    stop: AtomicBool,
    sender: UdpSocket,
    /// Held while the sender's outgoing multicast interface is set and a
    /// datagram sent through it, so two threads cannot interleave the two.
    sending: Mutex<()>,
    discovery: UdpSocket,
    net: Network,
    group: SocketAddr,
    /// The interfaces the group is joined on right now.
    joined: Mutex<Vec<Interface>>,
}

impl Shared {
    fn engine(&self) -> MutexGuard<'_, Engine> {
        // A panic under the lock leaves an engine that is still a valid
        // value; keep going with it rather than poisoning every thread.
        self.engine.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn interfaces(&self) -> Vec<Interface> {
        self.joined.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Send `bytes` to the group out of `interface`.
    fn multicast(&self, interface: Ipv4Addr, bytes: &[u8]) {
        let _held = self.sending.lock().unwrap_or_else(|e| e.into_inner());
        if SockRef::from(&self.sender).set_multicast_if_v4(&interface).is_ok() {
            let _ = self.sender.send_to(bytes, self.group);
        }
    }

    /// Re-read the machine's interfaces: join the group on the new ones,
    /// leave it on the ones that went away.
    fn rescan(&self) {
        let now = interfaces(&self.net);
        let mut joined = self.joined.lock().unwrap_or_else(|e| e.into_inner());
        for gone in joined.iter().filter(|i| !now.iter().any(|n| n.addr == i.addr)) {
            let _ = self.discovery.leave_multicast_v4(&self.net.group, &gone.addr);
        }
        joined.retain(|i| now.iter().any(|n| n.addr == i.addr));
        let new: Vec<Interface> =
            now.into_iter().filter(|n| !joined.iter().any(|i| i.addr == n.addr)).collect();
        joined.extend(join(&self.discovery, self.net.group, &new));
        joined.sort();
    }

    fn perform(self: &Arc<Self>, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Broadcast(msg) => {
                    for i in self.interfaces() {
                        self.multicast(i.addr, &msg.clone().with_endpoint_ip(i.addr).encode());
                    }
                }
                Action::Send(to, msg) => {
                    let msg = match to {
                        SocketAddr::V4(v4) => match address_for(&self.interfaces(), *v4.ip()) {
                            Some(ip) => msg.with_endpoint_ip(ip),
                            None => msg,
                        },
                        SocketAddr::V6(_) => msg,
                    };
                    let _ = self.sender.send_to(&msg.encode(), to);
                }
                Action::Measure { session, endpoint } => {
                    let shared = Arc::clone(self);
                    let _ = thread::Builder::new()
                        .name("simply-link-measure".into())
                        .spawn(move || shared.measure(session, endpoint));
                }
            }
        }
    }

    fn measure(self: &Arc<Self>, session: SessionId, endpoint: SocketAddrV4) {
        let intercept = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
            .ok()
            .and_then(|socket| burst(&socket, endpoint, &self.clock));
        let now = self.clock.now();
        let actions = self.engine().on_measured(session, intercept, now);
        self.perform(actions);
    }
}

/// One measurement burst against `endpoint` over `socket`: [`measure::BURST`]
/// pings, one at a time, each waiting up to [`measure::PONG_TIMEOUT_MS`] for
/// its answer, settled to the median. A pong carrying another ping's host
/// time (a late answer to an earlier one) is not counted against this one.
pub fn burst(socket: &UdpSocket, endpoint: SocketAddrV4, clock: &HostClock) -> Option<Micros> {
    socket.set_read_timeout(Some(Duration::from_millis(measure::PONG_TIMEOUT_MS))).ok()?;
    let mut samples = Vec::with_capacity(measure::BURST);
    let mut buf = [0u8; MAX_MESSAGE];
    for _ in 0..measure::BURST {
        let sent = clock.now();
        if socket.send_to(&measure::ping(sent), endpoint).is_err() {
            continue;
        }
        while let Ok((len, _)) = socket.recv_from(&mut buf) {
            let received = clock.now();
            let reply = &buf[..len];
            let echoed = crate::wire::Measurement::decode(reply).ok().and_then(|m| m.host_time());
            if echoed.is_some_and(|t| t != sent) {
                continue; // an earlier ping's answer, arriving late
            }
            samples.extend(measure::sample(reply, sent, received));
            break;
        }
    }
    measure::settle(&mut samples)
}

/// Answer pings on `socket` with `ghost(host_now)` until `stop` — the
/// measurement endpoint's loop, and the whole of it.
fn respond(
    socket: &UdpSocket,
    stop: &AtomicBool,
    clock: &HostClock,
    ghost: impl Fn(Micros) -> Option<Micros>,
) {
    let mut buf = [0u8; MAX_MESSAGE];
    while !stop.load(Ordering::Relaxed) {
        let Ok((len, from)) = socket.recv_from(&mut buf) else { continue };
        let Some(ghost_now) = ghost(clock.now()) else { continue };
        if let Some(reply) = measure::pong(&buf[..len], ghost_now) {
            let _ = socket.send_to(&reply, from);
        }
    }
}

/// A socket that hears the Link group on `net` and sends nothing: bound to
/// the Link port with address and port reuse, so it shares it with every
/// Link app on this machine, joined to the group on every interface of
/// [`interfaces`] that accepts it, reads timing out every 100 ms. The
/// engine's own discovery socket, and a passive listener's. Returns the
/// interfaces it joined on; an error if it joined on none.
pub fn discovery_listener(net: &Network) -> io::Result<(UdpSocket, Vec<Interface>)> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    socket.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, net.port)).into())?;
    let socket: UdpSocket = socket.into();
    let joined = join(&socket, net.group, &interfaces(net));
    if joined.is_empty() {
        return Err(io::Error::other(format!("no interface would join {}", net.group)));
    }
    socket.set_read_timeout(Some(READ_TIMEOUT))?;
    Ok((socket, joined))
}

fn sender_socket(net: &Network) -> io::Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)).into())?;
    // Other Link apps on this machine must hear us too.
    socket.set_multicast_loop_v4(true)?;
    socket.set_multicast_ttl_v4(net.multicast_ttl)?;
    let socket: UdpSocket = socket.into();
    socket.set_read_timeout(Some(READ_TIMEOUT))?;
    Ok(socket)
}

/// A Link peer, running. Dropping it says goodbye to the session and stops
/// every thread within [`READ_TIMEOUT`].
pub struct Link {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

impl Link {
    /// Join the Link group on the default network. With the default
    /// [`Config`] this peer only ever *follows*: it joins the first session
    /// it hears and never founds one of its own.
    pub fn join(config: Config) -> io::Result<Link> {
        Self::join_on(config, Network::default())
    }

    pub fn join_on(config: Config, net: Network) -> io::Result<Link> {
        let clock = HostClock::new();
        let (discovery, joined) = discovery_listener(&net)?;
        let sender = sender_socket(&net)?;
        let responder = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
        responder.set_read_timeout(Some(READ_TIMEOUT))?;
        // The address is a placeholder: every copy of every message moves
        // it to the address of the interface the copy leaves by.
        let endpoint = SocketAddrV4::new(joined[0].addr, responder.local_addr()?.port());
        let mut engine = Engine::new(NodeId::random(), config, clock.now());
        engine.set_endpoint(endpoint);
        let shared = Arc::new(Shared {
            engine: Mutex::new(engine),
            clock,
            stop: AtomicBool::new(false),
            sender: sender.try_clone()?,
            sending: Mutex::new(()),
            discovery: discovery.try_clone()?,
            net,
            group: SocketAddr::from((net.group, net.port)),
            joined: Mutex::new(joined),
        });

        let mut threads = Vec::new();
        for (name, socket) in
            [("simply-link-discovery", discovery), ("simply-link-unicast", sender)]
        {
            let shared = Arc::clone(&shared);
            threads.push(thread::Builder::new().name(name.into()).spawn(move || {
                let mut buf = [0u8; MAX_MESSAGE];
                while !shared.stop.load(Ordering::Relaxed) {
                    let Ok((len, from)) = socket.recv_from(&mut buf) else { continue };
                    let Ok(msg) = Discovery::decode(&buf[..len]) else { continue };
                    let now = shared.clock.now();
                    let actions = shared.engine().on_discovery(from, &msg, now);
                    shared.perform(actions);
                }
            })?);
        }
        {
            let shared = Arc::clone(&shared);
            threads.push(thread::Builder::new().name("simply-link-responder".into()).spawn(
                move || {
                    respond(&responder, &shared.stop, &shared.clock, |h| shared.engine().ghost(h))
                },
            )?);
        }
        {
            let shared = Arc::clone(&shared);
            threads.push(thread::Builder::new().name("simply-link-tick".into()).spawn(
                move || {
                    let mut scanned = std::time::Instant::now();
                    while !shared.stop.load(Ordering::Relaxed) {
                        if scanned.elapsed() >= RESCAN {
                            shared.rescan();
                            scanned = std::time::Instant::now();
                        }
                        let now = shared.clock.now();
                        let actions = shared.engine().tick(now);
                        shared.perform(actions);
                        thread::sleep(TICK);
                    }
                },
            )?);
        }
        Ok(Link { shared, threads })
    }

    /// The host clock every time this peer reports is on.
    pub fn clock(&self) -> HostClock {
        self.shared.clock
    }

    pub fn node(&self) -> NodeId {
        self.shared.engine().node()
    }

    /// A snapshot of the session, to answer "beat at host time t" and
    /// "host time of beat b" without holding any lock.
    pub fn state(&self) -> SessionState {
        self.shared.engine().state()
    }

    /// Propose `bpm` from host time `at` on — shared with every peer.
    pub fn propose_tempo(&self, bpm: f64, at: Micros) {
        let now = self.shared.clock.now();
        let actions = self.shared.engine().propose_tempo(bpm, at, now);
        self.shared.perform(actions);
    }

    /// Propose starting or stopping the shared transport at host time `at`.
    pub fn propose_start_stop(&self, playing: bool, at: Micros) {
        let now = self.shared.clock.now();
        let actions = self.shared.engine().propose_start_stop(playing, at, now);
        self.shared.perform(actions);
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(bye) = self.shared.engine().byebye() {
            for i in self.shared.interfaces() {
                self.shared.multicast(i.addr, &bye);
            }
        }
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> Ipv4Addr {
        s.parse().unwrap()
    }

    fn cand(addr: &str, netmask: &str, up: bool, point_to_point: bool) -> Candidate {
        Candidate { addr: ip(addr), netmask: ip(netmask), up, point_to_point }
    }

    /// The machine this defect was measured on, roughly: loopback, Wi-Fi,
    /// a down Ethernet, a VPN tunnel, and Wi-Fi listed twice (an alias).
    fn this_mac() -> Vec<Candidate> {
        vec![
            cand("192.168.55.119", "255.255.255.0", true, false),
            cand("127.0.0.1", "255.0.0.0", true, false),
            cand("10.1.2.3", "255.255.0.0", false, false),
            cand("100.64.0.7", "255.255.255.255", true, true),
            cand("192.168.55.119", "255.255.255.0", true, false),
        ]
    }

    #[test]
    fn a_peer_is_on_loopback_and_every_up_network_and_nothing_else() {
        let got = select(this_mac());
        let addrs: Vec<Ipv4Addr> = got.iter().map(|i| i.addr).collect();
        assert_eq!(addrs, [ip("127.0.0.1"), ip("192.168.55.119")], "loopback included");
    }

    /// Two addresses on one interface — here loopback, named twice — share
    /// one membership; the second join is refused as already-a-member, and
    /// must still count, or that address would never be announced on (the
    /// Wi-Fi-came-back-with-a-new-address case, which needs a real network
    /// change to see).
    #[test]
    fn an_interface_already_in_the_group_counts_as_joined() {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).unwrap();
        let group = ip("239.255.76.79");
        let lo = Interface { addr: Ipv4Addr::LOCALHOST, netmask: ip("255.0.0.0") };
        assert_eq!(join(&socket, group, &[lo]), [lo]);
        let again = socket.join_multicast_v4(&group, &lo.addr);
        assert_eq!(again.as_ref().map_err(io::Error::kind), Err(io::ErrorKind::AddrInUse));
        assert_eq!(join(&socket, group, &[lo]), [lo], "already a member is joined");
        assert!(!joined(Err(io::ErrorKind::AddrNotAvailable.into())), "a real failure is not");
    }

    #[test]
    fn an_override_narrows_the_peer_to_one_interface() {
        let net = Network { interface: Ipv4Addr::LOCALHOST, ..Network::default() };
        let only = interfaces(&net);
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].addr, Ipv4Addr::LOCALHOST);
    }

    #[test]
    fn the_real_machine_offers_loopback() {
        // Whatever else is up, the enumeration must not drop loopback —
        // that is the path to every other Link app on this machine.
        assert!(interfaces(&Network::default()).iter().any(|i| i.addr.is_loopback()));
    }

    #[test]
    fn a_peer_is_answered_with_the_address_it_reaches_us_at() {
        let ifs = select(this_mac());
        // On this machine, over loopback: our loopback address.
        assert_eq!(address_for(&ifs, ip("127.0.0.1")), Some(ip("127.0.0.1")));
        // On this machine, sending from our own Wi-Fi address: that one.
        assert_eq!(address_for(&ifs, ip("192.168.55.119")), Some(ip("192.168.55.119")));
        // Across the room: the interface whose network holds it.
        assert_eq!(address_for(&ifs, ip("192.168.55.40")), Some(ip("192.168.55.119")));
        // From somewhere no interface holds: the network, never loopback.
        assert_eq!(address_for(&ifs, ip("172.16.0.9")), Some(ip("192.168.55.119")));
        // With loopback alone, loopback it is.
        let lo = select([cand("127.0.0.1", "255.0.0.0", true, false)]);
        assert_eq!(address_for(&lo, ip("10.0.0.1")), Some(ip("127.0.0.1")));
        assert_eq!(address_for(&[], ip("10.0.0.1")), None);
    }

    #[test]
    fn a_broadcast_announces_each_interfaces_own_address() {
        use crate::wire::{DiscoveryKind, Entry};
        let msg = Discovery {
            kind: DiscoveryKind::Alive,
            ttl: 5,
            group: 0,
            node: NodeId(*b"abcdefgh"),
            entries: vec![Entry::Endpoint4(SocketAddrV4::new(ip("192.168.55.119"), 51000))],
        };
        for i in select(this_mac()) {
            let copy = Discovery::decode(&msg.clone().with_endpoint_ip(i.addr).encode()).unwrap();
            assert_eq!(copy.endpoint(), Some(SocketAddrV4::new(i.addr, 51000)), "port kept");
        }
    }

    /// A real burst over loopback UDP against a responder whose ghost clock
    /// runs 5 s ahead: the measured intercept lands within the loopback
    /// round trip of 5 s. Loopback only — nothing reaches the network.
    #[test]
    fn a_burst_over_loopback_measures_the_responders_clock() {
        let clock = HostClock::new();
        let responder = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        responder.set_read_timeout(Some(READ_TIMEOUT)).unwrap();
        let SocketAddr::V4(endpoint) = responder.local_addr().unwrap() else { unreachable!() };
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = Arc::clone(&stop);
            thread::spawn(move || respond(&responder, &stop, &clock, |h| Some(h + 5_000_000)))
        };
        let pinger = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let intercept = burst(&pinger, endpoint, &clock);
        stop.store(true, Ordering::Relaxed);
        thread.join().unwrap();
        let intercept = intercept.expect("the burst was answered");
        assert!((intercept - 5_000_000).abs() < 2_000, "{intercept}");
    }

    /// A responder with no ghost time yet stays silent, and the burst says
    /// so rather than inventing an offset.
    #[test]
    fn an_unjoined_responder_is_not_measured() {
        let clock = HostClock::new();
        let responder = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        responder.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
        let SocketAddr::V4(endpoint) = responder.local_addr().unwrap() else { unreachable!() };
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = Arc::clone(&stop);
            thread::spawn(move || respond(&responder, &stop, &clock, |_| None))
        };
        let pinger = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        assert_eq!(burst(&pinger, endpoint, &clock), None);
        stop.store(true, Ordering::Relaxed);
        thread.join().unwrap();
    }
}
