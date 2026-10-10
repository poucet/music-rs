//! Fuzz-shaped properties over the parser, with a seeded xorshift
//! generator — deterministic, dependency-free, and hermetic (no sockets).
//!
//! Three properties:
//! * the parser never panics, whatever the bytes;
//! * neither do mutations of valid packets (the inputs most likely to
//!   reach deep, half-trusting code paths);
//! * `parse(encode(p))` is `p`, and re-encoding is byte-identical, for
//!   randomly generated packets over the whole type set.

use simply_osc::types::{Bundle, Message, Packet, TimeTag, Value};
use simply_osc::wire::parse;

/// xorshift64*: enough randomness for a fuzzer, no dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| self.next() as u8).collect()
    }
}

#[test]
fn arbitrary_bytes_never_panic_the_parser() {
    let mut rng = Rng(0x5EED_0001);
    for _ in 0..20_000 {
        let len = rng.below(200);
        let mut data = rng.bytes(len);
        // Half the time, steer toward the interesting entrances.
        if len > 8 && rng.below(2) == 0 {
            match rng.below(3) {
                0 => data[0] = b'/',
                1 => data[..8].copy_from_slice(b"#bundle\0"),
                _ => {
                    data[0] = b'/';
                    // A null to end the address somewhere plausible.
                    let at = 1 + rng.below(len - 1);
                    data[at] = 0;
                }
            }
        }
        let _ = parse(&data); // any Result is fine; a panic is the failure
    }
}

#[test]
fn mutations_of_valid_packets_never_panic_the_parser() {
    let mut rng = Rng(0x5EED_0002);
    for round in 0..4_000 {
        let packet = random_packet(&mut rng, 0);
        let mut bytes = packet.encode().expect("generated packets encode");
        if bytes.is_empty() {
            continue;
        }
        for _ in 0..1 + rng.below(4) {
            match rng.below(3) {
                0 => {
                    let i = rng.below(bytes.len());
                    bytes[i] = rng.next() as u8;
                }
                1 => {
                    bytes.truncate(rng.below(bytes.len() + 1));
                }
                _ => {
                    let n = rng.below(8);
                    bytes.extend(rng.bytes(n));
                }
            }
            if bytes.is_empty() {
                break;
            }
        }
        let _ = parse(&bytes); // round {round}: any Result, never a panic
        let _ = round;
    }
}

#[test]
fn generated_packets_round_trip_exactly() {
    let mut rng = Rng(0x5EED_0003);
    for _ in 0..4_000 {
        let packet = random_packet(&mut rng, 0);
        let bytes = packet.encode().expect("generated packets encode");
        let parsed = parse(&bytes).expect("own encoding parses");
        // Value equality would reject a NaN float that survived
        // perfectly; byte equality of the re-encoding is the real claim.
        assert_eq!(parsed.encode().unwrap(), bytes);
        if !has_nan(&packet) {
            assert_eq!(parsed, packet);
        }
    }
}

fn has_nan(p: &Packet) -> bool {
    match p {
        Packet::Message(m) => m.args.iter().any(|a| match a {
            Value::Float32(f) => f.is_nan(),
            Value::Double(d) => d.is_nan(),
            _ => false,
        }),
        Packet::Bundle(b) => b.elements.iter().any(has_nan),
    }
}

fn random_packet(rng: &mut Rng, depth: usize) -> Packet {
    if depth < 3 && rng.below(4) == 0 {
        let n = rng.below(4);
        Packet::Bundle(Bundle {
            time: TimeTag(rng.next()),
            elements: (0..n).map(|_| random_packet(rng, depth + 1)).collect(),
        })
    } else {
        Packet::Message(random_message(rng))
    }
}

fn random_message(rng: &mut Rng) -> Message {
    let parts = 1 + rng.below(3);
    let mut address = String::new();
    for _ in 0..parts {
        address.push('/');
        for _ in 0..1 + rng.below(8) {
            // Printable, not reserved, not '/',
            address.push(match rng.below(3) {
                0 => (b'a' + rng.below(26) as u8) as char,
                1 => (b'A' + rng.below(26) as u8) as char,
                _ => (b'0' + rng.below(10) as u8) as char,
            });
        }
    }
    let mut m = Message::new(address);
    for _ in 0..rng.below(6) {
        m.args.push(random_value(rng));
    }
    m
}

fn random_value(rng: &mut Rng) -> Value {
    match rng.below(9) {
        0 => Value::Int32(rng.next() as i32),
        1 => Value::Float32(f32::from_bits(rng.next() as u32)),
        2 => {
            let len = rng.below(12);
            Value::Str(
                (0..len).map(|_| (b' ' + 1 + rng.below(94) as u8) as char).collect::<String>(),
            )
        }
        3 => {
            let n = rng.below(16);
            Value::Blob(rng.bytes(n))
        }
        4 => Value::Int64(rng.next() as i64),
        5 => Value::Double(f64::from_bits(rng.next())),
        6 => Value::Time(TimeTag(rng.next())),
        7 => Value::Bool(rng.below(2) == 0),
        _ => Value::Nil,
    }
}
