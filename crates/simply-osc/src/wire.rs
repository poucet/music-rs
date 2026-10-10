//! Bytes ↔ packet — the OSC 1.0 wire format, alignment rules exact.
//!
//! Everything in OSC is 4-byte aligned: a string is its bytes, a null, and
//! up to three more nulls to reach a multiple of four; a blob is a 4-byte
//! big-endian size, its bytes, and null padding to a multiple of four;
//! int32/float32 are 4 bytes big-endian, int64/double/timetag 8. A message
//! is address string, type tag string (`,` then one tag per argument),
//! then the arguments in tag order. A bundle is the string `#bundle`, an
//! 8-byte timetag, then elements, each a 4-byte size followed by that many
//! bytes holding a complete packet.
//!
//! The parser reads **untrusted UDP**: every malformed input is a
//! [`ParseError`], never a panic, and nothing is allocated from a length
//! field before the bytes it promises are known to be present. It is also
//! strict — padding must be null, trailing bytes are an error, an unknown
//! type tag is an error — so a packet this crate accepts is one it fully
//! understood, and `parse(encode(p)) == p` exactly.
//!
//! Two deliberate loosenesses, both from the spec's own text: a message
//! with no type tag string at all parses as zero arguments (the spec
//! requires receivers to survive pre-1.0 senders that omit it), and
//! strings are accepted as UTF-8 rather than ASCII (what real senders
//! emit).

use crate::types::{Bundle, Message, Packet, TimeTag, Value};

/// The largest UDP payload a datagram can carry; a convenient receive
/// buffer size.
pub const MAX_DATAGRAM: usize = 65_507;

/// How deep bundles may nest before the parser refuses. Real traffic
/// nests once or twice; a crafted datagram could otherwise nest thousands
/// deep and exhaust the stack.
pub const MAX_BUNDLE_DEPTH: usize = 32;

/// Why a byte sequence is not an OSC packet. The offset is where in the
/// input the problem was found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// The input ended before the structure it promised.
    Truncated { offset: usize },
    /// The packet is empty.
    Empty,
    /// The first byte is neither `/` (message) nor `#` (bundle), or the
    /// `#` is not `#bundle`.
    NotOsc,
    /// A string's null padding contained a non-null byte, or an object's
    /// length is not a multiple of four.
    BadPadding { offset: usize },
    /// A string is not valid UTF-8.
    BadString { offset: usize },
    /// The type tag string does not start with `,`.
    BadTypeTags { offset: usize },
    /// A type tag this crate does not implement (see the crate docs for
    /// what is in and out).
    UnknownTypeTag { tag: u8, offset: usize },
    /// A blob or bundle-element size is negative or runs past the input.
    BadLength { offset: usize },
    /// Bytes remained after a complete packet.
    Trailing { offset: usize },
    /// Bundles nested deeper than [`MAX_BUNDLE_DEPTH`].
    TooDeep,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Truncated { offset } => write!(f, "input ends early at byte {offset}"),
            ParseError::Empty => write!(f, "empty packet"),
            ParseError::NotOsc => write!(f, "neither a message (/) nor a bundle (#bundle)"),
            ParseError::BadPadding { offset } => write!(f, "bad padding at byte {offset}"),
            ParseError::BadString { offset } => {
                write!(f, "invalid UTF-8 in string at byte {offset}")
            }
            ParseError::BadTypeTags { offset } => {
                write!(f, "type tag string does not start with ',' at byte {offset}")
            }
            ParseError::UnknownTypeTag { tag, offset } => {
                write!(f, "unknown type tag {:?} at byte {offset}", *tag as char)
            }
            ParseError::BadLength { offset } => write!(f, "bad length at byte {offset}"),
            ParseError::Trailing { offset } => write!(f, "trailing bytes at byte {offset}"),
            ParseError::TooDeep => write!(f, "bundles nested deeper than {MAX_BUNDLE_DEPTH}"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Why a packet cannot be put on the wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EncodeError {
    /// A message address must begin with `/`.
    BadAddress,
    /// A string (or address) contains an interior NUL, which the wire
    /// format cannot carry.
    NulInString,
    /// A blob longer than an int32 size field can state.
    BlobTooLong,
}

impl std::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EncodeError::BadAddress => write!(f, "address must begin with '/'"),
            EncodeError::NulInString => write!(f, "string contains an interior NUL"),
            EncodeError::BlobTooLong => write!(f, "blob longer than an int32 can state"),
        }
    }
}

impl std::error::Error for EncodeError {}

// ---------------------------------------------------------------- parsing

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], ParseError> {
        if self.data.len() - self.pos < n {
            return Err(ParseError::Truncated { offset: self.pos });
        }
        let out = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    fn u32(&mut self) -> Result<u32, ParseError> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64, ParseError> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }

    /// An OSC string: bytes to the first null, then null padding to a
    /// multiple of four. Returns the string.
    fn string(&mut self) -> Result<&'a str, ParseError> {
        let start = self.pos;
        let rest = &self.data[start..];
        let nul = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or(ParseError::Truncated { offset: self.data.len() })?;
        let s = std::str::from_utf8(&rest[..nul])
            .map_err(|e| ParseError::BadString { offset: start + e.valid_up_to() })?;
        // The null and the padding: total length a multiple of four.
        let padded = (nul / 4 + 1) * 4;
        if rest.len() < padded {
            return Err(ParseError::Truncated { offset: self.data.len() });
        }
        if rest[nul..padded].iter().any(|&b| b != 0) {
            return Err(ParseError::BadPadding { offset: start + nul });
        }
        self.pos += padded;
        Ok(s)
    }

    fn blob(&mut self) -> Result<Vec<u8>, ParseError> {
        let at = self.pos;
        let len = self.u32()?;
        if len > i32::MAX as u32 {
            return Err(ParseError::BadLength { offset: at });
        }
        let len = len as usize;
        let bytes = self.take(len)?.to_vec();
        let pad = (4 - len % 4) % 4;
        let at = self.pos;
        if self.take(pad)?.iter().any(|&b| b != 0) {
            return Err(ParseError::BadPadding { offset: at });
        }
        Ok(bytes)
    }

    fn done(&self) -> bool {
        self.pos == self.data.len()
    }
}

/// Parse one datagram into a packet.
pub fn parse(data: &[u8]) -> Result<Packet, ParseError> {
    parse_at_depth(data, 0)
}

fn parse_at_depth(data: &[u8], depth: usize) -> Result<Packet, ParseError> {
    if depth > MAX_BUNDLE_DEPTH {
        return Err(ParseError::TooDeep);
    }
    match data.first() {
        None => Err(ParseError::Empty),
        Some(b'/') => parse_message(data).map(Packet::Message),
        Some(b'#') => parse_bundle(data, depth).map(Packet::Bundle),
        Some(_) => Err(ParseError::NotOsc),
    }
}

fn parse_message(data: &[u8]) -> Result<Message, ParseError> {
    let mut c = Cursor { data, pos: 0 };
    let address = c.string()?.to_owned();
    if c.done() {
        // Pre-1.0 senders omit the type tag string; the spec tells
        // receivers to take such a message as having no arguments.
        return Ok(Message { address, args: Vec::new() });
    }
    let tags_at = c.pos;
    let tags = c.string()?;
    let Some(tags) = tags.strip_prefix(',') else {
        return Err(ParseError::BadTypeTags { offset: tags_at });
    };
    let tags = tags.as_bytes().to_vec();
    let mut args = Vec::with_capacity(tags.len());
    for (i, tag) in tags.iter().enumerate() {
        args.push(match tag {
            b'i' => Value::Int32(c.u32()? as i32),
            b'f' => Value::Float32(f32::from_bits(c.u32()?)),
            b's' => Value::Str(c.string()?.to_owned()),
            b'b' => Value::Blob(c.blob()?),
            b'h' => Value::Int64(c.u64()? as i64),
            b'd' => Value::Double(f64::from_bits(c.u64()?)),
            b't' => Value::Time(TimeTag(c.u64()?)),
            b'T' => Value::Bool(true),
            b'F' => Value::Bool(false),
            b'N' => Value::Nil,
            &tag => {
                return Err(ParseError::UnknownTypeTag { tag, offset: tags_at + 1 + i });
            }
        });
    }
    if !c.done() {
        return Err(ParseError::Trailing { offset: c.pos });
    }
    Ok(Message { address, args })
}

fn parse_bundle(data: &[u8], depth: usize) -> Result<Bundle, ParseError> {
    let mut c = Cursor { data, pos: 0 };
    if c.string()? != "#bundle" {
        return Err(ParseError::NotOsc);
    }
    let time = TimeTag(c.u64()?);
    let mut elements = Vec::new();
    while !c.done() {
        let at = c.pos;
        let len = c.u32()?;
        if len > i32::MAX as u32 || len % 4 != 0 {
            return Err(ParseError::BadLength { offset: at });
        }
        let bytes = c.take(len as usize)?;
        elements.push(parse_at_depth(bytes, depth + 1)?);
    }
    Ok(Bundle { time, elements })
}

// --------------------------------------------------------------- encoding

fn put_string(out: &mut Vec<u8>, s: &str) -> Result<(), EncodeError> {
    if s.bytes().any(|b| b == 0) {
        return Err(EncodeError::NulInString);
    }
    out.extend_from_slice(s.as_bytes());
    let pad = 4 - s.len() % 4;
    out.extend(std::iter::repeat_n(0u8, pad));
    Ok(())
}

impl Message {
    /// This message as wire bytes. Fails only on content the format
    /// cannot carry: an address not starting `/`, an interior NUL, a blob
    /// longer than an int32 size.
    pub fn encode(&self) -> Result<Vec<u8>, EncodeError> {
        if !self.address.starts_with('/') {
            return Err(EncodeError::BadAddress);
        }
        let mut out = Vec::new();
        put_string(&mut out, &self.address)?;
        let mut tags = String::with_capacity(self.args.len() + 1);
        tags.push(',');
        for a in &self.args {
            tags.push(a.tag() as char);
        }
        put_string(&mut out, &tags)?;
        for a in &self.args {
            match a {
                Value::Int32(v) => out.extend_from_slice(&v.to_be_bytes()),
                Value::Float32(v) => out.extend_from_slice(&v.to_bits().to_be_bytes()),
                Value::Str(s) => put_string(&mut out, s)?,
                Value::Blob(b) => {
                    let len = i32::try_from(b.len()).map_err(|_| EncodeError::BlobTooLong)?;
                    out.extend_from_slice(&len.to_be_bytes());
                    out.extend_from_slice(b);
                    out.extend(std::iter::repeat_n(0u8, (4 - b.len() % 4) % 4));
                }
                Value::Int64(v) => out.extend_from_slice(&v.to_be_bytes()),
                Value::Double(v) => out.extend_from_slice(&v.to_bits().to_be_bytes()),
                Value::Time(t) => out.extend_from_slice(&t.0.to_be_bytes()),
                Value::Bool(_) | Value::Nil => {}
            }
        }
        Ok(out)
    }
}

impl Bundle {
    /// This bundle as wire bytes.
    pub fn encode(&self) -> Result<Vec<u8>, EncodeError> {
        let mut out = Vec::new();
        put_string(&mut out, "#bundle").expect("constant");
        out.extend_from_slice(&self.time.0.to_be_bytes());
        for e in &self.elements {
            let bytes = e.encode()?;
            out.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
            out.extend_from_slice(&bytes);
        }
        Ok(out)
    }
}

impl Packet {
    /// This packet as wire bytes — one UDP datagram's payload.
    pub fn encode(&self) -> Result<Vec<u8>, EncodeError> {
        match self {
            Packet::Message(m) => m.encode(),
            Packet::Bundle(b) => b.encode(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Bundle, Message, Packet, TimeTag, Value};

    /// The spec's own first example: `/oscillator/4/frequency` with one
    /// float 440.0 (spec-1_0 "OSC Message Examples").
    #[test]
    fn the_specs_frequency_example_byte_for_byte() {
        #[rustfmt::skip]
        let bytes: &[u8] = &[
            b'/', b'o', b's', b'c', b'i', b'l', b'l', b'a',
            b't', b'o', b'r', b'/', b'4', b'/', b'f', b'r',
            b'e', b'q', b'u', b'e', b'n', b'c', b'y', 0,
            b',', b'f', 0, 0,
            0x43, 0xDC, 0x00, 0x00,
        ];
        let parsed = parse(bytes).unwrap();
        let want = Message::new("/oscillator/4/frequency").arg(Value::Float32(440.0));
        assert_eq!(parsed, Packet::Message(want.clone()));
        assert_eq!(want.encode().unwrap(), bytes);
    }

    /// The spec's second example: `/foo` with `,iisff` — 1000, -1,
    /// "hello", 1.234, 5.678 (spec-1_0 "OSC Message Examples").
    #[test]
    fn the_specs_foo_example_byte_for_byte() {
        #[rustfmt::skip]
        let bytes: &[u8] = &[
            b'/', b'f', b'o', b'o', 0, 0, 0, 0,
            b',', b'i', b'i', b's', b'f', b'f', 0, 0,
            0x00, 0x00, 0x03, 0xE8,
            0xFF, 0xFF, 0xFF, 0xFF,
            b'h', b'e', b'l', b'l', b'o', 0, 0, 0,
            0x3F, 0x9D, 0xF3, 0xB6,
            0x40, 0xB5, 0xB2, 0x2D,
        ];
        let want = Message::new("/foo")
            .arg(Value::Int32(1000))
            .arg(Value::Int32(-1))
            .arg(Value::Str("hello".into()))
            .arg(Value::Float32(f32::from_bits(0x3F9D_F3B6)))
            .arg(Value::Float32(f32::from_bits(0x40B5_B22D)));
        assert_eq!(parse(bytes).unwrap(), Packet::Message(want.clone()));
        assert_eq!(want.encode().unwrap(), bytes);
    }

    #[test]
    fn a_string_arg_is_padded_to_four_with_at_least_one_null() {
        // A 4-byte string still takes a full extra word of nulls.
        let m = Message::new("/s").arg(Value::Str("abcd".into()));
        let bytes = m.encode().unwrap();
        // "/s\0\0" + ",s\0\0" + "abcd\0\0\0\0"
        assert_eq!(bytes.len(), 4 + 4 + 8);
        assert_eq!(&bytes[8..], b"abcd\0\0\0\0");
        assert_eq!(parse(&bytes).unwrap(), Packet::Message(m));
    }

    #[test]
    fn blobs_pad_to_four_and_round_trip() {
        for len in 0..9 {
            let m = Message::new("/b").arg(Value::Blob((0..len as u8).collect()));
            let bytes = m.encode().unwrap();
            assert_eq!(bytes.len() % 4, 0, "len {len}");
            assert_eq!(parse(&bytes).unwrap(), Packet::Message(m), "len {len}");
        }
    }

    #[test]
    fn every_extended_type_round_trips() {
        let m = Message::new("/all")
            .arg(Value::Int64(-(1 << 40)))
            .arg(Value::Double(1.0 / 3.0))
            .arg(Value::Time(TimeTag::from_parts(3_900_000_000, 7)))
            .arg(Value::Bool(true))
            .arg(Value::Bool(false))
            .arg(Value::Nil);
        let bytes = m.encode().unwrap();
        assert_eq!(parse(&bytes).unwrap(), Packet::Message(m));
    }

    #[test]
    fn true_false_nil_take_no_argument_bytes() {
        let m = Message::new("/tfn").arg(Value::Bool(true)).arg(Value::Bool(false)).arg(Value::Nil);
        // "/tfn\0\0\0\0" + ",TFN\0\0\0\0" and nothing else.
        assert_eq!(m.encode().unwrap().len(), 8 + 8);
    }

    #[test]
    fn a_message_without_a_type_tag_string_parses_as_no_arguments() {
        assert_eq!(parse(b"/old\0\0\0\0").unwrap(), Packet::Message(Message::new("/old")),);
    }

    #[test]
    fn a_bundle_with_nested_bundle_round_trips() {
        let p = Packet::Bundle(Bundle {
            time: TimeTag::IMMEDIATE,
            elements: vec![
                Packet::Message(Message::new("/a").arg(Value::Int32(1))),
                Packet::Bundle(Bundle {
                    time: TimeTag::from_parts(100, 200),
                    elements: vec![Packet::Message(Message::new("/b"))],
                }),
            ],
        });
        let bytes = p.encode().unwrap();
        assert_eq!(parse(&bytes).unwrap(), p);
    }

    #[test]
    fn an_empty_bundle_is_legal() {
        let p = Packet::Bundle(Bundle { time: TimeTag::IMMEDIATE, elements: vec![] });
        let bytes = p.encode().unwrap();
        assert_eq!(bytes.len(), 16);
        assert_eq!(parse(&bytes).unwrap(), p);
    }

    #[test]
    fn malformed_inputs_are_errors_not_panics() {
        // Each case: (bytes, the error the parser must name).
        let cases: &[(&[u8], ParseError)] = &[
            (b"", ParseError::Empty),
            (b"x\0\0\0", ParseError::NotOsc),
            (b"#bundlx\0AAAAAAAA", ParseError::NotOsc),
            (b"/a", ParseError::Truncated { offset: 2 }), // no null, no padding
            (b"/a\0\0,i\0\0", ParseError::Truncated { offset: 8 }), // promised int32 missing
            (b"/a\0X,i\0\0\0\0\0\x05", ParseError::BadPadding { offset: 2 }), // pad byte not null
            (b"/a\0\0i\0\0\0", ParseError::BadTypeTags { offset: 4 }), // tags missing the comma
            (b"/a\0\0,q\0\0", ParseError::UnknownTypeTag { tag: b'q', offset: 5 }),
            (b"/a\0\0,\0\0\0XXXX", ParseError::Trailing { offset: 8 }),
            (b"/a\0\0,b\0\0\xFF\xFF\xFF\xFF", ParseError::BadLength { offset: 8 }), // negative blob size
            (b"/a\0\0,b\0\0\x00\x00\xFF\xFF", ParseError::Truncated { offset: 12 }), // blob size past end
            (b"#bundle\0AAAAAAAA\x00\x00\x00\x03XXX", ParseError::BadLength { offset: 16 }), // element size not /4
            (b"#bundle\0AAAAAAAA\x00\x00\x00\x08", ParseError::Truncated { offset: 20 }),
            (b"/a\0\0,s\0\0\xC3\x28\0\0", ParseError::BadString { offset: 8 }), // invalid UTF-8
        ];
        for (bytes, want) in cases {
            assert_eq!(parse(bytes).as_ref(), Err(want), "input {bytes:?}");
        }
    }

    #[test]
    fn a_huge_blob_length_does_not_allocate_before_checking() {
        // Size field says 2 GB; the input is 16 bytes. Must be an error
        // immediately, not an allocation.
        let mut bytes = b"/a\0\0,b\0\0".to_vec();
        bytes.extend_from_slice(&0x7FFF_FFF0u32.to_be_bytes());
        bytes.extend_from_slice(&[0; 4]);
        assert_eq!(parse(&bytes), Err(ParseError::Truncated { offset: 12 }));
    }

    #[test]
    fn bundles_nested_past_the_depth_limit_are_refused() {
        // Build a bundle nested MAX_BUNDLE_DEPTH + 2 deep by wrapping.
        let mut inner = Packet::Message(Message::new("/x"));
        for _ in 0..MAX_BUNDLE_DEPTH + 2 {
            inner = Packet::Bundle(Bundle { time: TimeTag::IMMEDIATE, elements: vec![inner] });
        }
        let bytes = inner.encode().unwrap();
        assert_eq!(parse(&bytes), Err(ParseError::TooDeep));
    }

    #[test]
    fn encode_refuses_what_the_wire_cannot_carry() {
        assert_eq!(Message::new("nope").encode(), Err(EncodeError::BadAddress));
        assert_eq!(
            Message::new("/s").arg(Value::Str("a\0b".into())).encode(),
            Err(EncodeError::NulInString),
        );
        assert_eq!(Message::new("/a\0b").encode(), Err(EncodeError::NulInString));
    }

    #[test]
    fn nan_floats_round_trip_bit_for_bit() {
        let bits = 0x7FC0_1234u32;
        let m = Message::new("/n").arg(Value::Float32(f32::from_bits(bits)));
        let bytes = m.encode().unwrap();
        match parse(&bytes).unwrap() {
            Packet::Message(got) => match got.args[0] {
                Value::Float32(f) => assert_eq!(f.to_bits(), bits),
                ref other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }
    }
}
