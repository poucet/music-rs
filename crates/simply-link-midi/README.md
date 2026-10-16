# simply-link-midi

`link-midi` — an Ableton Link session out as MIDI beat clock, phase-aligned to the session's quantum: a drum machine or a modular's MIDI→clock interface plays in time *and in phase* with every Link app on the network, its bar one on the session's downbeat.

It only follows. It never starts a session of its own, never changes the tempo, and never starts or stops the transport; with no Link peer on the network it waits.

## Use

```sh
link-midi ports                       # which MIDI outputs this machine has
link-midi session                     # join, and print tempo / beat / phase / peers
link-midi listen                      # decode Link traffic without joining [--seconds N]
link-midi run --port "L6max MIDI"     # clock that port until Ctrl-C
```

`run` options:

| option | meaning |
|---|---|
| `--port NAME` | exact name, or an unambiguous part of it |
| `--quantum 4` | beats per bar the Start is aligned to — use your Link app's quantum |
| `--latency-ms 3.5` | how late the device acts on a clock; everything is sent that much early |
| `--transport sync` | start and stop with the session's **Start Stop Sync** (default) |
| `--transport free` | run whenever joined, whatever the session's transport says |
| `--dry-run` | open no port; print Start/Stop and one line a beat instead |

What the device receives: nothing until the session plays; then, on the next
quantum boundary, Song Position 0 and Start; 24 clocks a beat, each placed on
the session's own timeline (a tempo change reaches the next clock); Stop when
the session stops, and a new Start at the following boundary when it plays
again. A jump in the session's timeline is a Stop and a fresh Start rather
than a burst of catch-up clocks. Ctrl-C sends Stop and leaves the session.

**In `sync` mode, turn on Start Stop Sync in the Link app** — otherwise the
session carries no transport and the device is never started. `--transport
free` is the way round that.

**`--json`** is available on `ports`, `session` and `listen` for an agent.
`listen --json` prints one object when it is done —
`{"interfaces": [...], "seconds": 5, "announcements": [...]}` — so hearing
nothing is an empty list rather than no output.

**Every interface, loopback included.** A Link app talks to other Link apps
on the same machine over loopback, so `link-midi` joins the Link group on
every network interface that is up, loopback among them, and `listen` says
which. A network that comes up while `run` is going is joined within 5 s.
IPv6-only Link peers are not heard.

## Proving it against a real Link peer

The Link peer (`simply-link`) is tested against itself and against the
public packet dumps; interop with a real Link app is still to be witnessed.
Each step below only listens or joins — none changes the tempo or the
transport — and step 5 sends to no port.

1. Start a Link app on this machine or the same network, Link on, **Start
   Stop Sync on**, tempo set to something recognisable (say 97 BPM).
2. `link-midi listen --seconds 5` — expect the app's announcements, decoded,
   at 97 BPM: from `127.0.0.1` for an app on this machine. `--json` adds the
   raw bytes: press play in the app and the `stst` entry's bytes are the
   evidence for the one discovery entry no public dump shows.
3. `link-midi session --seconds 10` — expect `joined`, 97 BPM, one peer, and
   the app's Link indicator counting one more peer. **If it stays at
   `measuring`/`searching`, the measurement exchange does not match**: run
   step 4 and keep the capture.
4. The measurement bytes (only needed if step 3 failed, or to pin them in a
   test): in one terminal `sudo tcpdump -i any -n -X 'udp and not port
   20808 and not port 53'`, in another `link-midi session --seconds 5`. The
   pings are the datagrams starting `_link_v` (`5f 6c 69 6e 6b 5f 76`).
5. `link-midi run --dry-run` with the app playing — the `Start` line should
   print as the app's bar line comes round (quantum 4), and the beat lines
   once a beat. If Start lands mid-bar, the phase convention is off.
6. Only then a real port: `link-midi run --port <device>` with the device
   stopped, and press play in the app.

## Proving it across machines

On one machine this is proven only as far as a LAN *address*: a second
peer pinned to en0's IP (`cargo run -p simply-link --example founder --
<en0 IP> 100 130`), followed by `link-midi session`/`run`, measured through
that address and not loopback. The kernel still delivers those datagrams
internally; a real wire, a real Wi-Fi access point and its multicast
filtering need two laptops on the same network:

1. Laptop A: start a Link app (Ableton, Flux, Rekordbox, TouchDesigner), Link on, **Start Stop Sync on**, 97 BPM, playing.
2. Laptop B: `link-midi listen --seconds 5` shows A's announcements **from A's LAN address** (not `127.0.0.1`), 97 BPM, with a `mep4` on A's network. Hearing nothing means the network filters multicast (guest Wi-Fi, client isolation) or a firewall drops UDP 20808. Use a phone hotspot or a cable instead.
3. Laptop B: `link-midi session --seconds 10` reaches `joined`, 97 BPM, 1 peer; A's Link indicator counts one more peer.
4. On A, change the tempo to 120: within a second, B's `session` line reads 120.
5. Laptop B: `link-midi run --port <device>` starts the device on A's next bar line, follows tempo changes, and stops when A stops.

Built on [`simply-link`](../simply-link) (the Link peer — discovery, shared
timeline, ghost-time measurement) and
[`simply-midi-clock`](../simply-midi-clock) (the clock messages and the
sleep-then-spin pacing), with midir for the output port.
