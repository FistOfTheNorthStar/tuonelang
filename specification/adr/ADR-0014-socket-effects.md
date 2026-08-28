# ADR-0014: Socket effects — the network joins the descriptor seam

- **Status:** accepted (2026-08-24 — all stages landed; see Resolution)
- **Date:** 2026-08-24
- **Context:** ADR-0006 landed the descriptor seam (`std::rt::{write,
  read_byte}` over already-open file descriptors) and its amendment 2 recorded
  the gap exactly: *no socket-open effect primitive exists (no
  socket/bind/listen/accept/connect), so no program can create a connection*.
  ADR-0013 closed the same gap for files by adding **open** — the descriptor
  seam already moved the bytes; only the descriptor *source* was missing. The
  network is the identical shape: a POSIX socket **is** a file descriptor, so
  `write`/`read_byte`/`close` already work on one; what is missing is the
  handful of effects that produce a connected descriptor. The consequences of
  the gap are all recorded in-tree: the performance lab's `networking`
  workload is the one `Support::Unsupported` entry, `examples/http-service`
  carries a `CONTRACT serve` naming this exact missing primitive, and the
  Go-parity review lists `net/http` as a gap. The project rule forbids bolting
  sockets on ad hoc: they are ABI-touching effect primitives, exactly what an
  ADR is for.

- **Decision:** extend the seam with **four further effect builtins in
  `std::rt`** — the minimal set from which a TCP server and client are both
  expressible, composing with the existing `write`/`read_byte`/`close` for all
  data movement. All keep the seam's contract: fixed non-generic signatures,
  **never trap**, errors as negative return values.

  | Signature | Meaning |
  |-----------|---------|
  | `fn listen(take port: Int) -> Int` | Creates an IPv4 TCP socket bound to `127.0.0.1:port` and listening (backlog 16, `SO_REUSEADDR`); returns the listening descriptor (`>= 0`) or a negative value on host error. `port` `0` asks the host for an ephemeral port — pair with `bound_port`. |
  | `fn bound_port(take fd: Int) -> Int` | The local port a listening descriptor is actually bound to (`getsockname`), or a negative value on host error. Exists so `listen(0)` is usable — tests and benchmarks never hard-code a port. |
  | `fn accept(take fd: Int) -> Int` | Accepts one pending connection on a listening descriptor; returns the connected descriptor (`>= 0`) or a negative value on host error. Blocks until a connection arrives. |
  | `fn connect(in host: Str, take port: Int) -> Int` | Opens a TCP connection to `host:port`; returns the connected descriptor (`>= 0`) or a negative value on host error. `host` is a numeric IPv4 address (e.g. `"127.0.0.1"`) — name resolution is deliberately absent (DNS is a much larger, blocking, configuration-dependent surface; a later additive primitive if dogfooding demands it). |

  No new value types and no new MIR concepts: the builtins reuse
  `Statement::Effect` and the existing scalar/`Str` ABI. Sockets compose with
  the ADR-0006/0013 primitives — `listen`/`accept`/`connect` produce
  descriptors; the existing `read_byte`/`write`/`write_string` move the bytes
  and the existing `close` releases them. There is deliberately **no**
  send/recv pair: on POSIX, `read`/`write` on a connected TCP socket are the
  same operation, and keeping one spelling per fundamental task is the
  stdlib's own rule. IPv4 loopback-and-numeric-address only, TCP only: the
  smallest surface that makes a real server/client pair expressible.

  **Runtime obligations (ABI):** the runtime shim gains `tuo_rt_listen`
  (`socket`/`SO_REUSEADDR`/`bind`/`listen`), `tuo_rt_bound_port`
  (`getsockname`), `tuo_rt_accept` (`accept`, `EINTR` retried), and
  `tuo_rt_connect` (`inet_pton` + `connect`, `EINTR` handled; the `Str` host
  is copied to a bounded NUL-terminated buffer — a host over the bound is a
  host error, never a trap). `specification/abi.md` documents each;
  `ABI_VERSION` bumps in the same commit.

  **Stdlib payoff (the acceptance oracle):** a new `std::net` module —
  effect-tier wrappers `listen`, `bound_port`, `accept`, `connect`, plus
  `close` re-exposed for symmetry (a socket is closed exactly like a file) —
  each marked `EFFECT:` and pinned by a native CLI test, per the three-tier
  rule. `examples/http-service` replaces its `CONTRACT serve` with a real
  `serve_once`: `main` now proves the pure parser against a **live loopback
  round-trip** (listen ephemeral → connect to itself → accept → send a real
  request line → parse → respond over the socket → the client reads the
  status back), exiting 200 only when the wire agrees with the model. The
  spec sandbox is untouched by construction: the new builtins join the
  effectful set, so `R0007` statically refuses any spec that could reach
  them, and the reference interpreter continues to execute no effect, ever.

- **Consequences:**
  - *Easier:* real network programs (a bounded TCP server, a client) become
    expressible end to end; the lab's last `Unsupported` workload becomes
    measurable; `http-service` stops advertising a contract it cannot run.
  - *Harder:* socket programs are non-deterministic and environment-dependent
    (ports, kernel buffers), so they can never enter the differential suites
    or the fuzz corpus — the effect type discipline already enforces this.
    CI must tolerate loopback sockets (standard on every runner).
  - *Trade-off:* a single-process test must exploit TCP's accept backlog
    (`connect` completes against a listening socket before `accept` runs) and
    keep in-flight data under the kernel's socket buffer; both are documented
    in the committed programs. Blocking `accept`/`connect` with no timeout
    keeps the seam minimal — a timeout variant is additive if dogfooding
    demands it.

- **Benchmark consideration (the gating workload):** the performance lab's
  `networking` entry — `Support::Unsupported` since the lab existed, its
  reason naming exactly this missing primitive — flips to
  `Support::Supported` with a committed program: per round, `listen(0)` →
  `connect` → `accept` → write a 16-byte payload → byte-at-a-time read-back →
  close all three descriptors, the exit byte a checksum of bytes received,
  with equivalent-semantics C and Go peers making the identical calls. This
  is the flip the lab's own comment promised: *the entry becomes measurable
  the moment the feature lands, with no other change.*

- **Deliberately out of scope** (*three of these were since taken up by
  [ADR-0017](ADR-0017-timeouts-ipv6-and-udp.md) — timeouts, IPv6, and UDP —
  exactly on the "when its need is demonstrated by dogfooding" condition this
  section states; TLS and DNS remain out*)**:** DNS/name resolution, IPv6, UDP,
  TLS, non-blocking I/O and timeouts, `net/http`-style request/response types
  (the `http-service` example remains the HTTP story — a library can be
  written in tuonelang later, in tuonelang), and listening on non-loopback
  addresses (binding `127.0.0.1` only keeps every committed test and
  benchmark from opening an externally reachable port). Each is additive on
  this seam when its need is demonstrated by dogfooding.

- **Resolution (2026-08-24):** all stages landed in one increment and every
  acceptance condition is met by a committed, test-pinned artifact:
  - *Front end + MIR:* the four builtins resolve as real `std::rt` symbols
    (`Builtin::{RtListen, RtBoundPort, RtAccept, RtConnect}`), join the
    effectful set (so `R0007` shields the spec sandbox with no new
    mechanism — pinned by the `Builtin::ALL` loop in
    `crates/tuo-types/tests/effects.rs`), and lower to the four new
    `EffectOp`s, verified per-op by `check_effect_types` (`M0011`). The
    reference interpreter is untouched: its blanket effect refusal covers
    the new ops by construction.
  - *Native lowering (ABI v8):* both backends lower the new ops to the four
    `tuo_rt_*` shims; the runtime's effect C source implements them
    (loopback-bound `socket`/`bind`/`listen`, `getsockname`, `EINTR`-retried
    `accept`, `inet_pton` + `EINTR`/`EISCONN`-aware `connect`), and
    `ABI_VERSION` bumped 7 → 8. Pinned end-to-end on **both** backends by
    `crates/tuo-cli/tests/effects_native.rs` (a full single-process
    listen/connect/accept/write/read/EOF/close roundtrip, plus
    refused-connection and non-numeric-host error paths).
  - *Stdlib payoff:* the new `std::net` module — the effect-tier wrappers
    `listen`/`bound_port`/`accept`/`connect`/`close` plus the pure, spec'd
    `is_descriptor` — pinned by a native CLI test in
    `crates/tuo-cli/tests/stdlib.rs` on both backends, with the effect-tier
    list test updated to exactly the seventeen wrappers.
    `examples/http-service` replaced `CONTRACT serve` with the real
    `serve_once`/`live_status`: `main` now exits 200 only when the live
    loopback round-trip's bytes agree with the pure parser — pinned by
    `crates/tuo-cli/tests/dogfood_examples.rs`, stdout still byte-exact.
  - *Benchmark condition:* the lab's `networking` entry — the catalog's
    last `Support::Unsupported` — flipped to `Supported` with the committed
    `benchmarks/runtime/programs/tuo/networking.tuo` (100 rounds of
    listen/connect/accept, 128 bytes over the wire read back
    byte-at-a-time; exit byte 128) and equivalent-semantics C and Go peers,
    measured live by `crates/tuo-cli/tests/lab_command.rs`. Every workload
    the lab names is now measurable.
