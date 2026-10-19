//! The bytes on the network.
//!
//! Two message families, both UDP:
//!
//! * **Discovery** — multicast to [`GROUP`]:[`PORT`], every peer announcing
//!   itself and its view of the session. A 20-byte header — the magic
//!   `_asdp_v` and a version byte, the message kind, a time-to-live in
//!   seconds, a 16-bit group, and the sender's 8-byte node id — followed
//!   by a payload of entries. A `ByeBye` is the bare header.
//! * **Measurement** — unicast ping/pong between one peer and another's
//!   measurement endpoint (the `mep4` entry), from which a joining peer
//!   learns the offset between its host clock and the session's.
//!
//! A payload is a run of entries, each a 4-byte ASCII key read as a
//! big-endian `u32`, a big-endian `u32` size, and that many bytes of
//! value. An entry whose key is unknown is skipped by its size, so a newer
//! peer's extra entries never stop an older reader.
//!
//! Provenance (see `docs/research/engine/ableton-link.md`): the discovery
//! header, `tmln`, `sess` and `mep4` layouts are read off the public packet
//! dumps of the `westhom/AbletonLinkProtocol` write-up, and pinned below
//! against those exact bytes. The `stst` entry and the measurement
//! messages are **not** in any public dump found; their layout here is
//! this crate's and is the part a packet capture against a real peer must
//! confirm (`link-midi listen --json` prints announcements raw; the
//! measurement exchange needs a packet capture — see the guide). One
//! measurement rule is known from how Ableton peers behave, and was missed
//! once: a pong names the responder's session (`sess`), and a pong that
//! does not name the session being measured fails the measurement.

use std::net::{Ipv4Addr, SocketAddrV4};

use crate::timeline::{StartStop, Timeline};

/// The multicast group: 224.76.78.75 — the last three octets are "LNK".
pub const GROUP: Ipv4Addr = Ipv4Addr::new(224, 76, 78, 75);
/// The discovery port.
pub const PORT: u16 = 20808;

/// `_asdp_v` and protocol version 1.
pub const DISCOVERY_MAGIC: [u8; 8] = *b"_asdp_v\x01";
/// `_link_v` and protocol version 1 — the measurement messages.
pub const MEASUREMENT_MAGIC: [u8; 8] = *b"_link_v\x01";

/// Header length of a discovery message: magic, kind, ttl, group, node.
pub const DISCOVERY_HEADER: usize = 8 + 1 + 1 + 2 + 8;
/// Header length of a measurement message: magic, kind.
pub const MEASUREMENT_HEADER: usize = 8 + 1;

/// The largest datagram either family sends or accepts.
pub const MAX_MESSAGE: usize = 512;

/// A peer's identity: eight bytes, printable ASCII in every capture seen.
/// A session is named by the node id of the peer that founded it.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub [u8; 8]);

impl NodeId {
    /// A fresh random id in the printable range `!`..=`~`, as the captured
    /// peers use. Seeded from the standard library's per-process random
    /// hasher keys and the clock — no extra dependency, and collisions
    /// between two peers on one network are a 2⁻⁵² event.
    pub fn random() -> Self {
        use std::collections::hash_map::RandomState;
        use std::hash::{BuildHasher, Hasher};
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        hasher.write_u32(std::process::id());
        let mut bits = hasher.finish();
        let mut id = [0u8; 8];
        for byte in &mut id {
            *byte = b'!' + (bits % 94) as u8;
            bits /= 94;
        }
        NodeId(id)
    }
}

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for b in self.0 {
            if b.is_ascii_graphic() {
                write!(f, "{}", b as char)?;
            } else {
                write!(f, "\\x{b:02x}")?;
            }
        }
        Ok(())
    }
}

/// A session's identity: the founder's node id.
pub type SessionId = NodeId;

/// The kind byte of a discovery message.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DiscoveryKind {
    /// Periodic multicast announcement.
    Alive = 1,
    /// Unicast answer to a newcomer's first `Alive`.
    Response = 2,
    /// The peer is leaving; forget it now rather than at its ttl.
    ByeBye = 3,
}

/// The kind byte of a measurement message.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MeasurementKind {
    Ping = 1,
    Pong = 2,
}

const TIMELINE: u32 = u32::from_be_bytes(*b"tmln");
const SESSION: u32 = u32::from_be_bytes(*b"sess");
const ENDPOINT4: u32 = u32::from_be_bytes(*b"mep4");
const START_STOP: u32 = u32::from_be_bytes(*b"stst");
const HOST_TIME: u32 = u32::from_be_bytes(*b"__ht");
const GHOST_TIME: u32 = u32::from_be_bytes(*b"__gt");
const PREV_GHOST_TIME: u32 = u32::from_be_bytes(*b"_pgt");

/// One payload entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    /// `tmln`: µs per beat, beat origin in µbeats, time origin in ghost µs.
    Timeline(Timeline),
    /// `sess`: the session the sender belongs to.
    Session(SessionId),
    /// `mep4`: where to send measurement pings (IPv4 + port).
    Endpoint4(SocketAddrV4),
    /// `stst`: playing, the beat it changed at (µbeats), and when (ghost µs).
    StartStop(StartStop),
    /// `__ht`: a host time in µs (the pinger's clock).
    HostTime(i64),
    /// `__gt`: a ghost time in µs (the session's clock).
    GhostTime(i64),
    /// `_pgt`: the previous ghost time seen.
    PrevGhostTime(i64),
    /// Anything else, kept so a responder can echo it untouched.
    Unknown { key: u32, value: Vec<u8> },
}

/// A decode that did not work, and why — never a panic on hostile bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WireError {
    Short,
    Magic,
    Kind(u8),
    Truncated { key: u32 },
    TooLong,
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Short => write!(f, "shorter than a header"),
            Self::Magic => write!(f, "not a Link message"),
            Self::Kind(k) => write!(f, "unknown message kind {k}"),
            Self::Truncated { key } => {
                write!(f, "entry {:?} runs past the datagram", key.to_be_bytes().map(char::from))
            }
            Self::TooLong => write!(f, "message longer than {MAX_MESSAGE} bytes"),
        }
    }
}

impl std::error::Error for WireError {}

fn i64_at(b: &[u8], at: usize) -> i64 {
    i64::from_be_bytes(b[at..at + 8].try_into().expect("slice of 8"))
}

impl Entry {
    fn encode(&self, out: &mut Vec<u8>) {
        let mut put = |k: u32, value: &[u8]| {
            out.extend_from_slice(&k.to_be_bytes());
            out.extend_from_slice(&(value.len() as u32).to_be_bytes());
            out.extend_from_slice(value);
        };
        match self {
            Entry::Timeline(t) => {
                let mut v = [0u8; 24];
                v[0..8].copy_from_slice(&t.micros_per_beat.to_be_bytes());
                v[8..16].copy_from_slice(&t.beat_origin.to_be_bytes());
                v[16..24].copy_from_slice(&t.time_origin.to_be_bytes());
                put(TIMELINE, &v);
            }
            Entry::Session(s) => put(SESSION, &s.0),
            Entry::Endpoint4(a) => {
                let mut v = [0u8; 6];
                v[0..4].copy_from_slice(&a.ip().octets());
                v[4..6].copy_from_slice(&a.port().to_be_bytes());
                put(ENDPOINT4, &v);
            }
            Entry::StartStop(s) => {
                let mut v = [0u8; 17];
                v[0] = u8::from(s.playing);
                v[1..9].copy_from_slice(&s.beats.to_be_bytes());
                v[9..17].copy_from_slice(&s.timestamp.to_be_bytes());
                put(START_STOP, &v);
            }
            Entry::HostTime(t) => put(HOST_TIME, &t.to_be_bytes()),
            Entry::GhostTime(t) => put(GHOST_TIME, &t.to_be_bytes()),
            Entry::PrevGhostTime(t) => put(PREV_GHOST_TIME, &t.to_be_bytes()),
            Entry::Unknown { key, value } => put(*key, value),
        }
    }

    /// One entry from its key and value. A known key with the wrong size is
    /// kept as `Unknown` rather than misread.
    fn decode(k: u32, v: &[u8]) -> Entry {
        match (k, v.len()) {
            (TIMELINE, 24) => Entry::Timeline(Timeline {
                micros_per_beat: i64_at(v, 0),
                beat_origin: i64_at(v, 8),
                time_origin: i64_at(v, 16),
            }),
            (SESSION, 8) => Entry::Session(NodeId(v.try_into().expect("8"))),
            (ENDPOINT4, 6) => Entry::Endpoint4(SocketAddrV4::new(
                Ipv4Addr::new(v[0], v[1], v[2], v[3]),
                u16::from_be_bytes([v[4], v[5]]),
            )),
            (START_STOP, 17) => Entry::StartStop(StartStop {
                playing: v[0] != 0,
                beats: i64_at(v, 1),
                timestamp: i64_at(v, 9),
            }),
            (HOST_TIME, 8) => Entry::HostTime(i64_at(v, 0)),
            (GHOST_TIME, 8) => Entry::GhostTime(i64_at(v, 0)),
            (PREV_GHOST_TIME, 8) => Entry::PrevGhostTime(i64_at(v, 0)),
            _ => Entry::Unknown { key: k, value: v.to_vec() },
        }
    }
}

/// The `sess` entry of a payload — a discovery message's session, or the
/// session a pong answers for.
fn session_of(entries: &[Entry]) -> Option<SessionId> {
    entries.iter().find_map(|e| match e {
        Entry::Session(s) => Some(*s),
        _ => None,
    })
}

fn encode_payload(entries: &[Entry], out: &mut Vec<u8>) {
    for e in entries {
        e.encode(out);
    }
}

fn decode_payload(mut b: &[u8]) -> Result<Vec<Entry>, WireError> {
    let mut entries = Vec::new();
    while !b.is_empty() {
        if b.len() < 8 {
            return Err(WireError::Truncated { key: 0 });
        }
        let k = u32::from_be_bytes(b[0..4].try_into().expect("4"));
        let size = u32::from_be_bytes(b[4..8].try_into().expect("4")) as usize;
        let rest = &b[8..];
        if rest.len() < size {
            return Err(WireError::Truncated { key: k });
        }
        entries.push(Entry::decode(k, &rest[..size]));
        b = &rest[size..];
    }
    Ok(entries)
}

/// A discovery message, decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Discovery {
    pub kind: DiscoveryKind,
    /// Seconds a receiver may keep this peer without hearing from it again.
    pub ttl: u8,
    pub group: u16,
    pub node: NodeId,
    pub entries: Vec<Entry>,
}

impl Discovery {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(96);
        out.extend_from_slice(&DISCOVERY_MAGIC);
        out.push(self.kind as u8);
        out.push(self.ttl);
        out.extend_from_slice(&self.group.to_be_bytes());
        out.extend_from_slice(&self.node.0);
        encode_payload(&self.entries, &mut out);
        out
    }

    pub fn decode(b: &[u8]) -> Result<Self, WireError> {
        if b.len() > MAX_MESSAGE {
            return Err(WireError::TooLong);
        }
        if b.len() < DISCOVERY_HEADER {
            return Err(WireError::Short);
        }
        if b[0..8] != DISCOVERY_MAGIC {
            return Err(WireError::Magic);
        }
        let kind = match b[8] {
            1 => DiscoveryKind::Alive,
            2 => DiscoveryKind::Response,
            3 => DiscoveryKind::ByeBye,
            k => return Err(WireError::Kind(k)),
        };
        Ok(Discovery {
            kind,
            ttl: b[9],
            group: u16::from_be_bytes([b[10], b[11]]),
            node: NodeId(b[12..20].try_into().expect("8")),
            entries: decode_payload(&b[DISCOVERY_HEADER..])?,
        })
    }

    pub fn timeline(&self) -> Option<Timeline> {
        self.entries.iter().find_map(|e| match e {
            Entry::Timeline(t) => Some(*t),
            _ => None,
        })
    }

    pub fn session(&self) -> Option<SessionId> {
        session_of(&self.entries)
    }

    pub fn endpoint(&self) -> Option<SocketAddrV4> {
        self.entries.iter().find_map(|e| match e {
            Entry::Endpoint4(a) => Some(*a),
            _ => None,
        })
    }

    pub fn start_stop(&self) -> Option<StartStop> {
        self.entries.iter().find_map(|e| match e {
            Entry::StartStop(s) => Some(*s),
            _ => None,
        })
    }

    /// The same message with its measurement endpoint (`mep4`) moved to
    /// `ip`, port kept. A peer on several interfaces announces on each the
    /// address it is reached at *there* — 127.0.0.1 on loopback, its LAN
    /// address on Wi-Fi — so every listener measures through a path it has.
    pub fn with_endpoint_ip(mut self, ip: Ipv4Addr) -> Self {
        for e in &mut self.entries {
            if let Entry::Endpoint4(a) = e {
                a.set_ip(ip);
            }
        }
        self
    }
}

/// A measurement message, decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Measurement {
    pub kind: MeasurementKind,
    pub entries: Vec<Entry>,
}

impl Measurement {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64);
        out.extend_from_slice(&MEASUREMENT_MAGIC);
        out.push(self.kind as u8);
        encode_payload(&self.entries, &mut out);
        out
    }

    pub fn decode(b: &[u8]) -> Result<Self, WireError> {
        if b.len() > MAX_MESSAGE {
            return Err(WireError::TooLong);
        }
        if b.len() < MEASUREMENT_HEADER {
            return Err(WireError::Short);
        }
        if b[0..8] != MEASUREMENT_MAGIC {
            return Err(WireError::Magic);
        }
        let kind = match b[8] {
            1 => MeasurementKind::Ping,
            2 => MeasurementKind::Pong,
            k => return Err(WireError::Kind(k)),
        };
        Ok(Measurement { kind, entries: decode_payload(&b[MEASUREMENT_HEADER..])? })
    }

    /// The session a pong answers for.
    pub fn session(&self) -> Option<SessionId> {
        session_of(&self.entries)
    }

    pub fn ghost_time(&self) -> Option<i64> {
        self.entries.iter().find_map(|e| match e {
            Entry::GhostTime(t) => Some(*t),
            _ => None,
        })
    }

    pub fn host_time(&self) -> Option<i64> {
        self.entries.iter().find_map(|e| match e {
            Entry::HostTime(t) => Some(*t),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
    }

    /// The "Timeline packet from Live Beta" dump of the public
    /// `westhom/AbletonLinkProtocol` write-up, byte for byte.
    const LIVE_BETA: &str = "5f 61 73 64 70 5f 76 01 01 05 00 00 52 7b 46 27
        65 45 53 3a 74 6d 6c 6e 00 00 00 18 00 00 00 00
        00 0c 35 00 00 00 00 00 4f 14 99 30 00 00 00 00
        06 2c 36 f4 73 65 73 73 00 00 00 08 52 7b 46 27
        65 45 53 3a 6d 65 70 34 00 00 00 06 0a 00 01 21
        fa f4";

    /// …and its iPad dump.
    const IPAD: &str = "5f 61 73 64 70 5f 76 01 01 05 00 00 6a 5b 3e 5b
        69 5c 6a 66 74 6d 6c 6e 00 00 00 18 00 00 00 00
        00 2d c6 c0 00 00 00 00 2b 5b 71 6b 00 00 00 01
        9d ec ea 72 73 65 73 73 00 00 00 08 6a 5b 3e 5b
        69 5c 6a 66 6d 65 70 34 00 00 00 06 c0 a8 00 a2
        f8 75";

    /// …and its disconnect dump.
    const BYEBYE: &str = "5f 61 73 64 70 5f 76 01 03 00 00 00 23 66 75 58 5e 6b 28 2d";

    #[test]
    fn decodes_the_public_live_capture() {
        let bytes = hex(LIVE_BETA);
        let m = Discovery::decode(&bytes).unwrap();
        assert_eq!(m.kind, DiscoveryKind::Alive);
        assert_eq!(m.ttl, 5);
        assert_eq!(m.group, 0);
        assert_eq!(m.node.to_string(), "R{F'eES:");
        let t = m.timeline().unwrap();
        assert_eq!(t.micros_per_beat, 800_000, "75 BPM");
        assert!((t.bpm() - 75.0).abs() < 1e-9);
        assert_eq!(t.beat_origin, 0x4f14_9930);
        assert_eq!(t.time_origin, 0x062c_36f4);
        // The founder announces its own id as the session.
        assert_eq!(m.session(), Some(m.node));
        assert_eq!(m.endpoint(), Some("10.0.1.33:64244".parse().unwrap()));
        // And it re-encodes to exactly the bytes that were captured.
        assert_eq!(m.encode(), bytes);
    }

    #[test]
    fn decodes_the_public_ipad_capture() {
        let bytes = hex(IPAD);
        let m = Discovery::decode(&bytes).unwrap();
        assert_eq!(m.timeline().unwrap().micros_per_beat, 3_000_000, "20 BPM");
        assert_eq!(m.endpoint(), Some("192.168.0.162:63605".parse().unwrap()));
        assert_eq!(m.encode(), bytes);
    }

    #[test]
    fn a_byebye_is_the_bare_header() {
        let bytes = hex(BYEBYE);
        let m = Discovery::decode(&bytes).unwrap();
        assert_eq!(m.kind, DiscoveryKind::ByeBye);
        assert!(m.entries.is_empty());
        assert_eq!(m.encode(), bytes);
    }

    #[test]
    fn start_stop_and_measurement_round_trip() {
        let d = Discovery {
            kind: DiscoveryKind::Response,
            ttl: 5,
            group: 0,
            node: NodeId(*b"abcdefgh"),
            entries: vec![
                Entry::StartStop(StartStop { playing: true, beats: -3, timestamp: 42 }),
                Entry::Unknown { key: u32::from_be_bytes(*b"xtra"), value: vec![1, 2, 3] },
            ],
        };
        assert_eq!(Discovery::decode(&d.encode()).unwrap(), d);
        let m = Measurement {
            kind: MeasurementKind::Pong,
            entries: vec![Entry::HostTime(7), Entry::GhostTime(-9), Entry::PrevGhostTime(11)],
        };
        let decoded = Measurement::decode(&m.encode()).unwrap();
        assert_eq!(decoded, m);
        assert_eq!(decoded.host_time(), Some(7));
        assert_eq!(decoded.ghost_time(), Some(-9));
    }

    #[test]
    fn hostile_bytes_are_errors_never_panics() {
        assert_eq!(Discovery::decode(b"_asdp_v"), Err(WireError::Short));
        let mut live = hex(LIVE_BETA);
        live[0] = b'X';
        assert_eq!(Discovery::decode(&live), Err(WireError::Magic));
        let mut live = hex(LIVE_BETA);
        live[8] = 9;
        assert_eq!(Discovery::decode(&live), Err(WireError::Kind(9)));
        let live = hex(LIVE_BETA);
        // Every cut inside an entry is an error; a cut on an entry's end is
        // a shorter, valid message (bytes 52 and 68 are such ends).
        for cut in DISCOVERY_HEADER + 1..live.len() {
            if cut != 52 && cut != 68 {
                assert!(Discovery::decode(&live[..cut]).is_err(), "cut at {cut}");
            }
        }
        assert_eq!(Discovery::decode(&live[..52]).unwrap().entries.len(), 1);
        assert_eq!(Measurement::decode(&hex(BYEBYE)), Err(WireError::Magic));
        // A known key at the wrong size is kept, not misread.
        let mut odd = DISCOVERY_MAGIC.to_vec();
        odd.extend_from_slice(&[1, 5, 0, 0]);
        odd.extend_from_slice(b"abcdefgh");
        odd.extend_from_slice(b"tmln");
        odd.extend_from_slice(&3u32.to_be_bytes());
        odd.extend_from_slice(&[1, 2, 3]);
        let m = Discovery::decode(&odd).unwrap();
        assert_eq!(m.timeline(), None);
    }

    #[test]
    fn a_random_node_id_is_printable() {
        let a = NodeId::random();
        assert!(a.0.iter().all(u8::is_ascii_graphic), "{a}");
    }
}
