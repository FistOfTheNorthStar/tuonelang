# ADR-0013: The OS effect boundary — clock, argv, and files

- **Status:** accepted (2026-08-23 — all stages landed; see Resolution)
- **Date:** 2026-08-23
- **Context:** ADR-0006 landed the effect seam (`std::rt::{write, read_byte,
  exit}` over already-open file descriptors) and deliberately deferred "argv,
  clock, filesystem, and thread primitives (their stdlib entry points remain
  contract-tier, each naming the primitive it awaits)". ADR-0007 has since
  resolved the thread primitive (`std::rt::par_map`). What remains between
  tuonelang and a *usable* command-line language is exactly the deferred trio:

  - `std::time::now`/`elapsed` are `CONTRACT:` stubs — no program can measure
    its own duration (the performance lab has to time tuonelang binaries from
    the *outside*).
  - `std::process::arg_count` is a `CONTRACT:` stub — no program can read its
    command line, so every dogfooding CLI hard-codes its input.
  - All four `std::fs` disk operations (`read`, `write`, `exists`, `remove`)
    are `CONTRACT:` stubs — `std::rt::write(fd, …)`/`read_byte(fd)` can already
    move bytes through descriptors 0/1/2, but nothing can **open** one, so no
    file is reachable.

  A Go-parity review (2026-08-23) confirmed these as the highest-value gaps
  short of sockets. The project rule forbids bolting any of them on ad hoc:
  each is an ABI-touching effect primitive, exactly what an ADR is for.

- **Decision:** extend the ADR-0006 seam with **six further effect builtins in
  `std::rt`** — the minimal scalar set from which the stdlib layers everything
  else in tuonelang, exactly as `read_line` is layered over `read_byte`. All
  keep the seam's contract: fixed non-generic signatures, **never trap**,
  errors as negative return values.

  | Signature | Meaning |
  |-----------|---------|
  | `fn now_nanos() -> Int` | The monotonic clock, in nanoseconds since an arbitrary process-local epoch. Only differences are meaningful. Effectful because non-deterministic. |
  | `fn arg_count() -> Int` | The number of process arguments, **including** the program name (argv[0]), matching C/Go. |
  | `fn arg_byte(take i: Int, take j: Int) -> Int` | Byte `j` (`0..=255`) of argument `i`, or `-1` when `i` is out of range or `j` is past that argument's end. The stdlib builds owned `String` arguments from it, exactly as `read_line` builds one from `read_byte`. |
  | `fn open(in path: Str, take mode: Int) -> Int` | Opens `path`; returns a file descriptor (`>= 0`), `-2` when the path does not exist (read mode), or another negative value on host error. Modes: `0` read, `1` write (create + truncate), `2` append (create). An unknown mode is a host error (negative), never a trap. |
  | `fn close(take fd: Int) -> Int` | Closes `fd`; `0` on success, negative on host error. |
  | `fn remove_file(in path: Str) -> Int` | Removes the file at `path`; `0` on success, `-2` when it does not exist, another negative value on other host errors. |

  No new value types and no new MIR concepts are introduced: the builtins reuse
  `Statement::Effect` and the existing scalar/`Str` ABI. Files compose with the
  ADR-0006 primitives — `open` + the existing `read_byte`/`write`/`write_string`
  + `close` are the whole file story; there is deliberately **no** separate
  read-file/write-file primitive, keeping the trusted surface minimal.

  **Runtime obligations (ABI):** the runtime shim gains
  `tuo_rt_now_nanos` (`CLOCK_MONOTONIC`), `tuo_rt_arg_count`/`tuo_rt_arg_byte`
  (the C `main` already owned by the runtime stashes `argc`/`argv` before
  calling the program's entry), and `tuo_rt_open`/`tuo_rt_close`/
  `tuo_rt_remove_file` (POSIX `open`/`close`/`unlink`; the `Str` path is
  copied to a bounded NUL-terminated buffer — a path over the bound is a host
  error, never a trap). `specification/abi.md` documents each; `ABI_VERSION`
  bumps in the same commit.

  **Stdlib payoff (the acceptance oracle):**
  - `std::time::now` becomes `EFFECT:` (an `Instant` holding `now_nanos`);
    `elapsed` becomes **pure executable** (it is subtraction) with a spec.
  - `std::process::arg_count` becomes `EFFECT:`; a new `std::process::arg(i)`
    returns argument `i` as an owned `String` built from `arg_byte`.
  - All four `std::fs` operations become `EFFECT:` implementations over
    `open`/`read_byte`/`write_string`/`close`/`remove_file`, keeping their
    existing `Result[…, FsError]` signatures; `-2` maps to the not-found
    `FsError`, other negatives to the generic one.
  - The spec sandbox is untouched **by construction**: the new builtins join
    the effectful set, so `R0007` statically refuses any spec that could reach
    them, and the reference interpreter continues to execute no effect, ever.

- **Consequences:**
  - *Easier:* real CLI tools (read args, read files, time themselves, write
    results) become expressible end to end; the stdlib contract tier shrinks
    to exactly the socket-shaped remainder; self-timing makes an in-language
    benchmark harness possible.
  - *Harder:* programs using the new builtins are non-deterministic
    (clock/argv/filesystem), so they can never enter the differential suites
    or the fuzz corpus — those stay on the pure core, which the effect type
    discipline already enforces.
  - *Trade-off:* byte-at-a-time argv (`arg_byte`) and file reads
    (`read_byte`) are slow but keep the seam scalar and auditable; a buffered
    read primitive is a later, additive optimization if the file-io benchmark
    shows it matters.

- **Benchmark consideration:** ADR-0006's acceptance recorded that the
  **effect-crossing cost benchmark is deferred to the ADR that lands the first
  effectful lab workload (sockets or files)** — this is that ADR. Acceptance
  therefore requires the performance lab's `file-io` workload: a committed
  tuonelang program (open, write, read back, checksum, remove; the checksum is
  the exit byte) with its equivalent-semantics C peer, measured under the
  lab's rules like every other workload. `networking` remains
  `Support::Unsupported` — no socket effect exists and this ADR does not add
  one.

- **Deliberately out of scope:** socket effects (`networking` stays honest),
  a calendar/wall clock (only the monotonic clock lands; formatting a date
  needs no ADR once a wall-clock primitive exists), environment variables,
  directory listing, and buffered I/O primitives. Each is additive on this
  seam when its need is demonstrated by dogfooding.

- **Resolution (2026-08-23):** all stages landed in one increment and every
  acceptance condition is met by a committed, test-pinned artifact:
  - *Front end + MIR:* the six builtins resolve as real `std::rt` symbols
    (`Builtin::{RtNowNanos, RtArgCount, RtArgByte, RtOpen, RtClose,
    RtRemoveFile}`), join the effectful set (so `R0007` shields the spec
    sandbox with no new mechanism — pinned by the `Builtin::ALL` loop in
    `crates/tuo-types/tests/effects.rs`), and lower to the six new
    `EffectOp`s, verified per-op by `check_effect_types` (`M0011`). The
    reference interpreter is untouched: its blanket effect refusal covers
    the new ops by construction. Normative text: `static-semantics.md` §3.6,
    `mir.md` §4.2.
  - *Native lowering (ABI v7):* both backends lower the new ops to the six
    `tuo_rt_*` shims (`specification/abi.md` "OS-boundary effect symbols");
    the runtime's effect C source implements them (`CLOCK_MONOTONIC`; argv
    captured before `main` by platform initializer; POSIX
    `open`/`close`/`unlink` with the `-2`/`-1` not-found/other vocabulary),
    and `ABI_VERSION` bumped 6 → 7. Pinned end-to-end on **both** backends by
    `crates/tuo-cli/tests/effects_native.rs` (clock monotonicity; real argv
    through a built executable; a full open/write/append/read/EOF/close/
    remove/not-found roundtrip).
  - *Stdlib payoff:* `std::time::now` (EFFECT over `now_nanos`; `elapsed`
    became pure executable with specs, plus the new `instant_at`
    constructor), `std::process::arg_count`/`arg` (EFFECT over the argv
    primitives — `arg` builds an owned `String` exactly as `read_line`
    does), and all four `std::fs` disk operations (EFFECT over
    `open`/`read_byte`/`write`/`close`/`remove_file`) — each pinned by a
    native CLI test in `crates/tuo-cli/tests/stdlib.rs` on both backends,
    with the effect-tier list test updated to exactly the twelve wrappers.
    *Amendment recorded at acceptance:* `std::fs::read` returns
    `Result[String, FsError]`, not the contract's original
    `Result[Str, FsError]` — a borrowed `Str` of file contents would have no
    owner, so the original signature was latently unimplementable; the
    owned `String` is the honest type.
  - *Benchmark condition:* the performance lab's **`file-io`** workload
    landed `Support::Supported` — the committed
    `benchmarks/runtime/programs/tuo/file-io.tuo` (open/write/close, reopen,
    byte-at-a-time read-back, remove; exit byte 240) with its
    equivalent-semantics C and Go peers, measured live by
    `crates/tuo-cli/tests/lab_command.rs`. This discharges ADR-0006's
    recorded amendment that the effect-crossing benchmark lands with the
    first effectful workload. `networking` stays `Support::Unsupported`,
    its reason updated to name what ADR-0013 did and did not add.
