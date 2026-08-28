# ADR-0017: Timeouts, IPv6, and UDP — the socket seam's additive increment

- **Status:** accepted (2026-08-25 — all three stages landed; see the stage resolutions)
- **Date:** 2026-08-25
- **Context:** ADR-0014 landed the socket effects (`listen`/`bound_port`/
  `accept`/`connect`) on the ADR-0006/0013 descriptor seam and, in its
  **Deliberately out of scope** section, named exactly what it left behind:
  *DNS/name resolution, IPv6, UDP, TLS, non-blocking I/O and timeouts* — each
  "additive on this seam when its need is demonstrated by dogfooding". Its
  **Consequences** were more specific still, recording the timeout gap as a
  trade-off rather than a closed decision: *"Blocking `accept`/`connect` with
  no timeout keeps the seam minimal — a timeout variant is additive if
  dogfooding demands it."*

  Three of those four have since accumulated demonstrated need; one has not.

  - **Timeouts are a robustness hole in committed code.** `examples/http-service`
    serves itself over a live loopback socket and the performance lab's
    `networking` workload runs a full `listen`/`connect`/`accept` round per
    iteration. Both block indefinitely by construction: a lost connection, a
    peer that never writes, or a kernel that never completes the handshake
    hangs the process with no recourse. The ADR-0014 seam offers no way to
    write a program that recovers, so every committed network program is a
    potential CI hang. This is the strongest of the three cases — it is not a
    missing capability but an unbounded wait in code that already ships.
  - **IPv6 is a correctness gap on modern hosts.** `connect` parses its host
    with `inet_pton(AF_INET, …)` only, so `"::1"` is a host error. On a host
    where loopback resolves to IPv6 first, a program that looks correct fails
    for a reason the language surface cannot express.
  - **UDP is a whole transport the seam cannot reach.** TCP-only means no
    datagram protocol — DNS clients, discovery, metrics/telemetry emission,
    NTP-style exchanges — is expressible at all.
  - **TLS and DNS stay out.** TLS implies either a cryptographic implementation
    or an external dependency, in a workspace whose SHA-256 is hand-rolled
    (`tuo-package`'s `sha256`) precisely to avoid adding one; DNS is, in
    ADR-0014's own words, "a much larger, blocking, configuration-dependent
    surface". Neither is in this increment. Note that UDP is a *prerequisite*
    for a future in-tuonelang DNS client, which is the honest way for name
    resolution to eventually arrive: written in tuonelang on this seam, not
    bolted into the runtime shim.

  The project rule forbids bolting these on ad hoc: they are ABI-touching
  effect primitives, exactly what an ADR is for.

- **Decision:** extend the socket seam with **ten further effect builtins in
  `std::rt`**, in three independent groups. All keep the seam's established
  contract: fixed non-generic signatures, **never trap**, errors as negative
  return values, and composition with the existing `write`/`read_byte`/`close`
  for all byte movement. No new value types and no new MIR concepts — every
  new builtin reuses `Statement::Effect` and the existing scalar/`Str` ABI.

### Group 1 — Timeouts (3 builtins)

Bounded-wait counterparts to the three operations that can block forever. The
existing blocking primitives are **unchanged**, so no committed program
changes behavior and a timeout is opt-in.

| Signature | Meaning |
|-----------|---------|
| `fn accept_timeout(take fd: Int, take ms: Int) -> Int` | As `accept`, but waits at most `ms` milliseconds. Returns the connected descriptor (`>= 0`), `NET_TIMEOUT` (`-3`) if the deadline passed with no pending connection, or `NET_ERROR` on host error. |
| `fn connect_timeout(in host: Str, take port: Int, take ms: Int) -> Int` | As `connect`, but abandons the handshake after `ms` milliseconds. Returns the connected descriptor (`>= 0`), `NET_TIMEOUT`, or `NET_ERROR`. |
| `fn read_byte_timeout(take fd: Int, take ms: Int) -> Int` | As `std::rt::read_byte`, but waits at most `ms` milliseconds for readability. Returns the byte (`0..=255`), `-1` at end of input (matching `read_byte` exactly), `NET_TIMEOUT`, or `NET_ERROR`. |

**A timeout is not an error, and the ABI must say so.** A timeout that
collapsed into an existing sentinel would be indistinguishable from a genuine
failure, and a program could not tell "the peer is slow" from "the peer is
gone". The seam already spends two negative values: `-1` is both `NET_ERROR`
and `read_byte`'s end-of-input (`READ_EOF`), and `-2` is `read_byte`'s host
error (`READ_ERROR`). `read_byte_timeout` is the call on which all four
outcomes — a byte, end of input, a host error, and a timeout — are
simultaneously possible, so it fixes the choice: this ADR adds the **distinct
sentinel** `NET_TIMEOUT = -3`, the first value that stays unambiguous
everywhere, plus `std::net::is_timeout` in the executable tier so the branch
is spelled once. A negative `ms` is a host error
(`NET_ERROR`), never an unbounded wait — a timeout primitive must not silently
become a blocking one. `ms` of `0` is a valid poll (return immediately).

`read_byte_timeout` is deliberately included even though it is a *descriptor*
operation rather than a socket one: it works on any descriptor the seam
produces (ADR-0013 files included), and it is the operation an actual hung
network read blocks in. Bounding `accept`/`connect` while leaving the read
unbounded would close two thirds of a hole.

### Group 2 — IPv6 (2 builtins, plus a widened `connect`)

Per the "one obvious API per fundamental task" rule, the *client* side gains no
new spelling: `connect` and `connect_timeout` **infer the address family** from
the numeric host string, trying `AF_INET` then `AF_INET6`. `connect("::1", p)`
simply works. This is a strict widening — every string that parsed before
parses identically, so no committed program changes meaning.

The *server* side cannot infer (there is no address to inspect, only a port),
so it gains explicit primitives:

| Signature | Meaning |
|-----------|---------|
| `fn listen6(take port: Int) -> Int` | As `listen`, but creates an IPv6 TCP socket bound to `[::1]:port` (loopback only, `IPV6_V6ONLY` set, backlog 16, `SO_REUSEADDR`). Returns the listening descriptor (`>= 0`) or `NET_ERROR`. |
| `fn peer_family(take fd: Int) -> Int` | The address family of a connected descriptor (`4` or `6`), or `NET_ERROR` on host error. Lets a program report and test which family it actually got. |

`bound_port` covers an IPv6 listener too, so `listen6(0)` needs no second
accessor. *(Implementation note, recorded during Stage B: the original
`bound_port` read `getsockname` into a `sockaddr_in`. On the POSIX layouts
this project targets, `sin_port` and `sin6_port` share offset 2, so it read
the right bytes for a v6 socket by coincidence of layout rather than by
guarantee — while also passing a too-small `sizeof(sockaddr_in)` as the
address length. Stage B rewrote it to read through `sockaddr_storage` and
switch on `ss_family`, which is correct by construction. The ADR originally
claimed it "works unchanged"; that was true in observable effect on these
hosts, but not for a reason worth relying on.)*
Binding stays **loopback-only** for the same reason ADR-0014 gave: no committed
test or benchmark may open an externally reachable port.

### Group 3 — UDP (5 builtins)

The minimal set from which a datagram client *and* a server that replies to
arbitrary senders are both expressible.

| Signature | Meaning |
|-----------|---------|
| `fn udp_bind(take port: Int) -> Int` | Creates an IPv4 UDP socket bound to `127.0.0.1:port`; returns the descriptor (`>= 0`) or `NET_ERROR`. Port `0` asks for an ephemeral port — pair with `bound_port`, which works unchanged on a UDP descriptor. |
| `fn udp_send(take fd: Int, in host: Str, take port: Int, in bytes: Str) -> Int` | Sends one datagram of `bytes` to `host:port` (numeric address, family inferred as in `connect`). Returns the number of bytes sent (`>= 0`) or `NET_ERROR`. |
| `fn udp_recv(take fd: Int, take ms: Int) -> Int` | Receives one datagram, waiting at most `ms` milliseconds, and stages it on the descriptor. Returns the datagram's length (`>= 0`), `NET_TIMEOUT`, or `NET_ERROR`. The bytes are then read with `udp_byte_at`, and the sender is available via `udp_peer_port`. |
| `fn udp_byte_at(take fd: Int, take i: Int) -> Int` | Byte `i` of the datagram most recently staged on `fd` (`0..=255`), or `NET_ERROR` if `i` is out of range or nothing is staged. |
| `fn udp_peer_port(take fd: Int) -> Int` | The source port of the most recent `udp_recv` on `fd`, or `NET_ERROR` if there was none. Lets a server reply. |

A datagram is a **message**, not a stream, so a byte-at-a-time read cannot by
itself express "one datagram" — a receive must report the boundary. `udp_recv`
therefore returns the *length* and stages the payload in a runtime-owned
per-descriptor buffer, which `udp_byte_at` indexes.

*(Amended during Stage C, before implementation.)* This ADR first specified
that the **existing `read_byte`** would drain the staged datagram, keeping one
spelling for "move bytes". That does not work: `read_byte` calls `read(2)`
directly and cannot see a runtime-owned buffer, so honoring the design would
mean giving *every* file and TCP read a staging-buffer lookup it can never
need — a cost on the hot path, and a `read_byte` whose behavior silently
depends on which kind of descriptor it is handed. A dedicated `udp_byte_at`
keeps the stream path untouched, makes the datagram's random access natural
(a parser wants byte `i`, not a cursor), and keeps the message boundary
explicit. The seam's "one obvious API per task" rule is preserved in
substance: reading a *stream* is still `read_byte`, and reading a *datagram*
is `udp_byte_at` — two different tasks, not two spellings of one.

`udp_recv` takes `ms` directly rather than offering a blocking variant: an
unbounded datagram receive is the classic unrecoverable hang, and this ADR
declines to add a new one while closing the existing ones. Pass a large `ms`
for effectively-blocking behavior.

The source *address* is deliberately not exposed as a `Str` in this increment:
returning host-allocated text would need a new `String`-producing effect shape,
which is a larger change than the seam warrants. Loopback-only binding means
the address is known; the port is the part that varies. A future additive
primitive can widen this when dogfooding demands it.

  **Runtime obligations (ABI):** the runtime shim gains `tuo_rt_accept_timeout`,
  `tuo_rt_connect_timeout`, and `tuo_rt_read_byte_timeout` (each `poll(2)` with
  the deadline, `EINTR` retried against a monotonic deadline so a signal storm
  cannot extend the wait — the retry recomputes the remaining time rather than
  restarting it), `tuo_rt_listen6` (`socket(AF_INET6)`/`IPV6_V6ONLY`/
  `SO_REUSEADDR`/`bind`/`listen`), `tuo_rt_peer_family` (`getsockname`),
  `tuo_rt_udp_bind`, `tuo_rt_udp_send` (`sendto`), `tuo_rt_udp_recv`
  (`poll` + `recvfrom` into the per-descriptor staging buffer),
  `tuo_rt_udp_byte_at`, and `tuo_rt_udp_peer_port`. The existing `tuo_rt_connect` is widened to try both
  families. `specification/abi.md` documents each; `ABI_VERSION` bumps from
  `9` to `10` in the same commit that moves the pinning tests.

  **Stdlib payoff (the acceptance oracle):** `std::net` gains effect-tier
  wrappers for all seven new primitives plus the executable-tier
  `is_timeout(outcome) -> Bool` classifier (and its spec), each `EFFECT:`
  entry pinned by a native CLI test per the three-tier rule. The spec sandbox
  is untouched by construction: the new builtins join the effectful set, so
  `R0007` statically refuses any spec that could reach them, and the reference
  interpreter continues to execute no effect, ever.

- **Consequences:**
  - *Easier:* network programs become **recoverable** — a hung peer is a value
    a program can branch on rather than a wedged process; IPv6-first hosts
    work; datagram protocols become expressible for the first time, which is
    the honest prerequisite for a future in-tuonelang DNS client.
  - *Harder:* the timeout primitives are the seam's first operations whose
    result depends on *wall-clock timing*, so a test asserting "this timed out"
    is inherently environment-sensitive. The committed tests therefore assert
    only the two robust directions — a connect to a port nothing listens on
    with a generous `ms` must not hang, and a `udp_recv` with a small `ms` on a
    silent socket must return `NET_TIMEOUT` — never a precise duration. As with
    ADR-0014, none of this may enter the differential suites or the fuzz corpus;
    the effect type discipline already enforces that.
  - *Trade-off:* `udp_recv`'s staging buffer is per-descriptor runtime state,
    the first the socket seam has held. It is bounded (one datagram, 2048
    bytes; a larger datagram is truncated and its true length still reported,
    matching `recvfrom` semantics) and process-lived like the ADR-0015 handle
    registries, so it introduces no new lifetime concept.
  - *Trade-off:* seven builtins is the largest single seam increment since
    ADR-0013's six. They are grouped precisely so they can land and be reviewed
    as three independent stages, and so a reader can see that no group is
    load-bearing for another.

- **Benchmark consideration (the gating workloads):** the performance lab gains
  **two** entries, since this ADR adds a transport and a failure mode rather
  than widening an existing one. Per the lab's honesty rule each ships with a
  committed program and equivalent-semantics C **and** Go peers making the
  identical calls:
  - **`udp-echo`** — per round, `udp_bind(0)` on two sockets, `udp_send` a
    16-byte payload from one to the other, `udp_recv` + byte-at-a-time
    read-back, reply to `udp_peer_port`, close both. The exit byte is a
    checksum of bytes received. The C peer uses `sendto`/`recvfrom`; the Go
    peer uses `net.ListenPacket`/`WriteTo`/`ReadFrom`. This measures the
    datagram round-trip against the `networking` entry's TCP one.
  - **`connect-timeout`** — the cost of a *bounded failure*: per round,
    `connect_timeout` to a closed loopback port with a fixed `ms`, asserting
    the `NET_TIMEOUT`/`NET_ERROR` result. The C peer uses non-blocking
    `connect` + `poll`; the Go peer uses `net.DialTimeout`. This is the
    workload that would have been impossible to write before this ADR, which
    is the point.

  No claim about timeout or datagram performance is admissible until both are
  committed; **the ADR is not "accepted" until they are**, exactly as ADR-0008
  and ADR-0014 required of their gating workloads.

- **Deliberately out of scope:** **TLS** and **DNS/name resolution** (see
  Context — TLS needs a crypto dependency this workspace will not take on
  implicitly; DNS should be written in tuonelang on the UDP primitives this
  ADR adds, not embedded in the runtime shim), non-blocking/`O_NONBLOCK` I/O
  as a *mode* (the timeout variants cover the demonstrated need without
  introducing a second I/O discipline), multicast and broadcast, Unix-domain
  sockets, `net/http`-style request/response types (`examples/http-service`
  remains the HTTP story, written in tuonelang), listening on non-loopback
  addresses, and exposing a peer's source *address* as a `Str`. Each is
  additive on this seam when its need is demonstrated by dogfooding.

- **Staging:** three independent stages, each landing complete — front end +
  MIR + interpreter refusal, both backends, stdlib wrappers, and tests —
  before the next begins. **Stage A: timeouts** (the robustness hole in
  committed code, so it goes first, and it introduces `NET_TIMEOUT` which the
  later stages reuse). **Stage B: IPv6** (smallest; the widened `connect` must
  land with `peer_family` so the widening is observable and testable).
  **Stage C: UDP** plus both gating lab workloads, and — once those are
  committed — acceptance of this ADR.

## Stage A resolution (2026-08-25 — timeouts landed)

Stage A is complete and pinned by committed artifacts. The ADR stays
**`proposed`** until Stages B and C land with their two gating lab workloads,
per the acceptance rule above.

- *Front end + MIR:* `accept_timeout`, `connect_timeout`, and
  `read_byte_timeout` resolve as real `std::rt` symbols
  (`Builtin::{RtAcceptTimeout, RtConnectTimeout, RtReadByteTimeout}`), join the
  effectful set — so `R0007` shields the spec sandbox with **no new
  mechanism** — and lower to three new `EffectOp`s, verified per-op by
  `check_effect_types`. The reference interpreter is untouched: its blanket
  effect refusal covers the new ops by construction.
- *Native lowering (ABI v9 → v10):* both backends lower the new ops to the
  three `tuo_rt_*` shims. The runtime's effect C source implements them over a
  shared `tuo_rt_poll_until` helper that computes a `CLOCK_MONOTONIC` deadline
  once and re-derives the remaining time on every `EINTR` retry, so a signal
  storm cannot extend the wait; `connect_timeout` uses the standard bounded
  handshake (`O_NONBLOCK` + `POLLOUT` + `SO_ERROR`, restoring the flags on
  success). `specification/abi.md` gains a **Bounded-wait symbols** section and
  its stale version history (which had drifted at `6`) is corrected through
  `10`.
- *The sentinel, corrected during implementation:* the design first proposed
  `NET_TIMEOUT = -2`, which the native test immediately falsified — `-2` is
  already `READ_ERROR`, so on `read_byte_timeout` a timeout would have been
  indistinguishable from a host read error, defeating the ADR's own
  distinctness rule. The sentinel is **`-3`**, the first value unambiguous on
  the one call where all four outcomes (byte, EOF, host error, timeout) are
  simultaneously possible. This is exactly the kind of divergence the
  "write the design first, then let the compiler falsify it" rule exists to
  surface.
- *Stdlib payoff:* `std::net` gains the three `EFFECT:`-marked wrappers and the
  executable-tier `is_timeout` classifier with its spec. The three-tier rule
  and the exact effect-tier roster are re-pinned in
  `crates/tuo-cli/tests/stdlib.rs`.
- *Proof:* `bounded_waits_time_out_without_blocking_forever` in
  `crates/tuo-cli/tests/effects_native.rs` runs on **both** backends and
  asserts only the environment-robust directions the ADR promised — a timeout
  is reported (never a hang, never an error), a timeout is distinguishable from
  both EOF and a host error, a negative `ms` is a host error rather than an
  unbounded wait, and a live roundtrip still completes. The test's own
  completion is the proof the waits are bounded: an unbounded one would hang
  the suite.

## Stage B resolution (2026-08-25 — IPv6 landed)

Stage B is complete and pinned. The ADR stays **`proposed`** until Stage C
lands with the two gating lab workloads.

- *Front end + MIR:* `listen6` and `peer_family` resolve as real `std::rt`
  symbols (`Builtin::{RtListen6, RtPeerFamily}`), join the effectful set, and
  lower to two new `EffectOp`s verified by `check_effect_types`. Both backends
  lower them through the existing single-scalar socket arm — no new shape.
- *Runtime:* `tuo_rt_listen6` binds `[::1]` with `IPV6_V6ONLY` (so
  `peer_family` on an accepted connection is unambiguous); `tuo_rt_peer_family`
  reports the portable `4`/`6` rather than the host's `AF_*`; and a shared
  `tuo_rt_addr_parse` helper gives `connect` **and** `connect_timeout` their
  family inference from one implementation rather than two copies.
- *Correction recorded:* the ADR's claim that `bound_port` "works unchanged"
  was imprecise — see the Group 2 note. It now reads through
  `sockaddr_storage`/`ss_family`, correct by construction rather than by an
  offset coincidence.
- *Stdlib payoff:* `std::net` gains the `listen6`/`peer_family` `EFFECT:`
  wrappers and the executable-tier `is_ipv6` classifier with its spec.
- *Proof:* `ipv6_listen_connect_and_family_reporting` in
  `crates/tuo-cli/tests/effects_native.rs` runs on **both** backends: the v4
  path still works, `listen6(0)` + `bound_port` + `connect("::1", port)` +
  `peer_family` all agree, a byte crosses the v6 connection, and a bad
  address is a host error rather than a trap. A host with IPv6 loopback
  disabled is tolerated at listener creation only; every later step is
  asserted. Verified to really execute the v6 path on the development host
  (a standalone program exits `99` only if the full v6 roundtrip ran).

## Stage C resolution (2026-08-25 — UDP landed; ADR accepted)

Stage C is complete, and with both gating workloads committed the acceptance
condition this ADR set for itself is met. **Status: accepted.**

- *Front end + MIR:* the five UDP builtins (`udp_bind`, `udp_send`,
  `udp_recv`, `udp_byte_at`, `udp_peer_port`) resolve as real `std::rt`
  symbols, join the effectful set, and lower to five new `EffectOp`s verified
  by `check_effect_types`. `udp_send` is the seam's first **four-operand**
  effect (six machine arguments, since two operands are `Str`), which both
  backends lower.
- *Design amended before implementation:* the ADR originally had the existing
  `read_byte` drain the staged datagram. It cannot — `read_byte` calls
  `read(2)` directly and cannot see a runtime buffer — so honoring it would
  have put a staging lookup on every file and TCP read. `udp_byte_at` was
  added instead (ten builtins, not nine); the amendment is recorded in
  Group 3 above, written before the code, per the project rule.
- *Runtime:* a 16-entry per-descriptor staging table, process-lived like the
  ADR-0015 registries. A datagram over `UDP_DATAGRAM_CAP` (2048) is truncated
  while its true length is still reported, matching `recvfrom`.
- *Stdlib payoff:* `std::net` gains the five `EFFECT:` wrappers, taking the
  module to the full TCP + UDP surface over one descriptor seam.
- *Proof:* `udp_datagram_roundtrip_with_reply_to_sender` in
  `crates/tuo-cli/tests/effects_native.rs` runs on **both** backends: a real
  datagram crosses loopback, the receiver reads it by index, replies to the
  port `udp_peer_port` names, and the client receives the reply — plus the
  timeout and out-of-range paths, which report rather than hang or trap.

### The gating workloads (the acceptance condition)

Both are committed with tuonelang, C, and Go programs, and both **measure**:

- **`udp-echo`** — per round two ephemeral UDP sockets exchange 8 datagrams
  with an echo back to `udp_peer_port`; exit byte 128. Peers: `sendto`/
  `recvfrom` in C, `net.ListenPacket`/`WriteTo`/`ReadFrom` in Go.
- **`connect-timeout`** — the cost of a *bounded failure*: 200 rounds of
  `connect_timeout` to a port nothing listens on, each required to come back
  rather than hang; exit byte 200. Peers: non-blocking `connect` + `poll` in
  C, `net.DialTimeout` in Go. **This workload could not have been written
  before this ADR** — with only the blocking `connect` there is no bounded
  outcome to measure — which is the clearest statement of what the increment
  bought.

The lab catalog now holds **fifteen** supported runtime workloads, none
unsupported. `crates/tuo-cli/tests/lab_command.rs` drives both host seams
live: all fifteen tuonelang programs compile-link-run to their expected exit
byte, and the C and Go peers agree where the toolchains exist.
