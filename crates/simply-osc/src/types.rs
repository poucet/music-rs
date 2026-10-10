//! What a packet is, independent of any socket or byte layout.
//!
//! An OSC *packet* is either a [`Message`] — an address pattern plus typed
//! arguments — or a [`Bundle`] — a timetag plus a list of packets, which
//! may themselves be bundles. The byte layout lives in [`crate::wire`];
//! these types carry only the meaning.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// One typed argument of a [`Message`].
///
/// The tag letters are the OSC type tag characters: the four the spec
/// requires (`i`, `f`, `s`, `b`) and the extended set this crate accepts
/// (`h`, `d`, `t`, `T`, `F`, `N`). `T`, `F` and `N` carry no bytes on the
/// wire — the tag is the value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// `i` — 32-bit big-endian two's complement integer.
    Int32(i32),
    /// `f` — 32-bit big-endian IEEE 754 float.
    Float32(f32),
    /// `s` — null-terminated string, padded to a multiple of four bytes.
    Str(String),
    /// `b` — length-prefixed bytes, padded to a multiple of four.
    Blob(Vec<u8>),
    /// `h` — 64-bit big-endian two's complement integer.
    Int64(i64),
    /// `d` — 64-bit big-endian IEEE 754 double.
    Double(f64),
    /// `t` — an NTP-style timetag.
    Time(TimeTag),
    /// `T` or `F` — true or false, no argument bytes.
    Bool(bool),
    /// `N` — nil, no argument bytes.
    Nil,
}

impl Value {
    /// The OSC type tag character for this value.
    pub fn tag(&self) -> u8 {
        match self {
            Value::Int32(_) => b'i',
            Value::Float32(_) => b'f',
            Value::Str(_) => b's',
            Value::Blob(_) => b'b',
            Value::Int64(_) => b'h',
            Value::Double(_) => b'd',
            Value::Time(_) => b't',
            Value::Bool(true) => b'T',
            Value::Bool(false) => b'F',
            Value::Nil => b'N',
        }
    }

    /// This value as an `f32` control signal, the shape a knob wants:
    /// numbers cast, booleans as 0/1, everything else `None`.
    pub fn as_float(&self) -> Option<f32> {
        match *self {
            Value::Int32(v) => Some(v as f32),
            Value::Float32(v) => Some(v),
            Value::Int64(v) => Some(v as f32),
            Value::Double(v) => Some(v as f32),
            Value::Bool(v) => Some(if v { 1.0 } else { 0.0 }),
            _ => None,
        }
    }
}

/// An NTP-style timetag: 32 bits of seconds since 1900-01-01T00:00:00 UTC,
/// 32 bits of fractional second.
///
/// The value `1` (0 seconds, smallest fraction) means *immediately*, by
/// spec. A tag says when the consumer should act on the bundle it heads;
/// this crate only carries it.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TimeTag(pub u64);

/// Seconds between the NTP epoch (1900) and the Unix epoch (1970):
/// 70 years, 17 of them leap.
const NTP_UNIX_OFFSET: u64 = 2_208_988_800;

impl TimeTag {
    /// The special value meaning *now*.
    pub const IMMEDIATE: TimeTag = TimeTag(1);

    /// Build from whole seconds since 1900 and a 32-bit fraction.
    pub fn from_parts(seconds: u32, fraction: u32) -> Self {
        TimeTag(((seconds as u64) << 32) | fraction as u64)
    }

    /// Whole seconds since 1900.
    pub fn seconds(self) -> u32 {
        (self.0 >> 32) as u32
    }

    /// The fractional second, in units of 2⁻³² s (~233 ps).
    pub fn fraction(self) -> u32 {
        self.0 as u32
    }

    /// Whether this is the *immediately* tag.
    pub fn is_immediate(self) -> bool {
        self.0 == 1
    }

    /// The wall-clock instant this tag names, if it is on or after the
    /// Unix epoch. `None` for *immediately* and for tags before 1970 —
    /// the consumer decides what those mean to it.
    pub fn to_system_time(self) -> Option<SystemTime> {
        if self.is_immediate() {
            return None;
        }
        let secs = (self.seconds() as u64).checked_sub(NTP_UNIX_OFFSET)?;
        // Round to nearest rather than truncate: paired with the rounding
        // in `from_system_time`, a nanosecond round-trips exactly instead
        // of sitting one floor away from the tolerance.
        let nanos = ((self.fraction() as u64 * 1_000_000_000) + (1u64 << 31)) >> 32;
        Some(UNIX_EPOCH + Duration::new(secs, nanos as u32))
    }

    /// The tag naming a wall-clock instant. Instants before 1970 saturate
    /// to the Unix epoch; the format itself runs out in 2036.
    pub fn from_system_time(t: SystemTime) -> Self {
        let since = t.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
        let secs = (since.as_secs() + NTP_UNIX_OFFSET) as u32;
        let frac = (((since.subsec_nanos() as u64) << 32) + 500_000_000) / 1_000_000_000;
        TimeTag::from_parts(secs, frac as u32)
    }
}

/// An OSC message: an address pattern and its arguments.
#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    /// The address pattern, beginning with `/`. On a received message this
    /// may contain the pattern characters `?`, `*`, `[]`, `{}` — match it
    /// against an [`crate::AddressSpace`], do not compare it for equality.
    pub address: String,
    /// The arguments, in wire order.
    pub args: Vec<Value>,
}

impl Message {
    /// A message with no arguments.
    pub fn new(address: impl Into<String>) -> Self {
        Message { address: address.into(), args: Vec::new() }
    }

    /// Builder: append one argument.
    pub fn arg(mut self, v: Value) -> Self {
        self.args.push(v);
        self
    }
}

/// An OSC bundle: a timetag and the packets it carries, in order.
#[derive(Clone, Debug, PartialEq)]
pub struct Bundle {
    /// When the consumer should act on the contents. This crate parses and
    /// exposes the tag; it never holds packets back.
    pub time: TimeTag,
    /// The elements, each a message or a nested bundle.
    pub elements: Vec<Packet>,
}

/// One OSC packet — what a UDP datagram carries.
#[derive(Clone, Debug, PartialEq)]
pub enum Packet {
    Message(Message),
    Bundle(Bundle),
}

impl Packet {
    /// Every message in this packet, depth first, with the timetag of the
    /// innermost enclosing bundle (`None` for a bare message). The flat
    /// view a consumer that does its own scheduling wants.
    pub fn messages(&self) -> Vec<(Option<TimeTag>, &Message)> {
        let mut out = Vec::new();
        fn walk<'a>(
            p: &'a Packet,
            time: Option<TimeTag>,
            out: &mut Vec<(Option<TimeTag>, &'a Message)>,
        ) {
            match p {
                Packet::Message(m) => out.push((time, m)),
                Packet::Bundle(b) => {
                    for e in &b.elements {
                        walk(e, Some(b.time), out);
                    }
                }
            }
        }
        walk(self, None, &mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn immediate_is_one_and_converts_to_no_instant() {
        assert!(TimeTag::IMMEDIATE.is_immediate());
        assert_eq!(TimeTag::IMMEDIATE.0, 1);
        assert_eq!(TimeTag::IMMEDIATE.to_system_time(), None);
    }

    #[test]
    fn timetag_round_trips_through_system_time_to_the_nanosecond() {
        let t = UNIX_EPOCH + Duration::new(1_700_000_000, 123_456_789);
        let tag = TimeTag::from_system_time(t);
        let back = tag.to_system_time().unwrap();
        let diff = back.duration_since(t).unwrap_or_else(|e| e.duration());
        // One unit of 2⁻³² s is ~233 ps; allow a nanosecond of rounding.
        assert!(diff <= Duration::from_nanos(1), "off by {diff:?}");
    }

    #[test]
    fn timetag_parts_round_trip() {
        let tag = TimeTag::from_parts(0x8000_0001, 0xDEAD_BEEF);
        assert_eq!(tag.seconds(), 0x8000_0001);
        assert_eq!(tag.fraction(), 0xDEAD_BEEF);
    }

    #[test]
    fn pre_1970_tags_convert_to_none() {
        assert_eq!(TimeTag::from_parts(100, 0).to_system_time(), None);
    }

    #[test]
    fn messages_flattens_nested_bundles_with_innermost_tags() {
        let inner_time = TimeTag::from_parts(10, 0);
        let outer_time = TimeTag::from_parts(5, 0);
        let p = Packet::Bundle(Bundle {
            time: outer_time,
            elements: vec![
                Packet::Message(Message::new("/a")),
                Packet::Bundle(Bundle {
                    time: inner_time,
                    elements: vec![Packet::Message(Message::new("/b"))],
                }),
            ],
        });
        let msgs = p.messages();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0], (Some(outer_time), &Message::new("/a")));
        assert_eq!(msgs[1], (Some(inner_time), &Message::new("/b")));
    }

    #[test]
    fn as_float_casts_numbers_and_booleans_only() {
        assert_eq!(Value::Int32(3).as_float(), Some(3.0));
        assert_eq!(Value::Float32(0.5).as_float(), Some(0.5));
        assert_eq!(Value::Int64(-2).as_float(), Some(-2.0));
        assert_eq!(Value::Double(0.25).as_float(), Some(0.25));
        assert_eq!(Value::Bool(true).as_float(), Some(1.0));
        assert_eq!(Value::Bool(false).as_float(), Some(0.0));
        assert_eq!(Value::Str("x".into()).as_float(), None);
        assert_eq!(Value::Nil.as_float(), None);
    }
}
