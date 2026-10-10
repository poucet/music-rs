# simply-osc

Open Sound Control 1.0 — an independent implementation of the protocol layer: typed messages, bundles, the wire format, address pattern matching, and a thin UDP receiver. No dependencies; the whole protocol is `std`.

The crate also ships the `simply-osc` binary, a command-line sender: one message per invocation, straight at anything speaking OSC 1.0 over UDP.

## The sender

```sh
cargo run -p simply-osc -- /flux/cutoff 0.5     # one float → 127.0.0.1:8000
simply-osc /flux/play                           # a bang (bare address)
simply-osc /flux/drums true                     # T — a rising edge
simply-osc --port 9000 /flux/m1 0.25
simply-osc --to 192.168.1.7:8000 /flux/tempo 128
simply-osc /flux/name s:42                      # force a string argument
simply-osc -v /flux/m1 0.25                     # echo what went out
```

Types are inferred: integers become `i`, numbers with a point become `f`, `true`/`false` become `T`/`F`, everything else is a string. Prefixes `i:`, `f:`, `d:`, `h:`, `s:` force a type.

## The library

`Message`, `Bundle`, encode/decode of the 1.0 wire format (big-endian, 4-byte padding), OSC address pattern matching (`?`, `*`, `[]`, `{}`), and a UDP receiver thread. See the crate docs (`cargo doc -p simply-osc --open`).
