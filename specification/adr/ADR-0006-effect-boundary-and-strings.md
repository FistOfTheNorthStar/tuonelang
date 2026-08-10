# ADR-0006: The effect boundary and runtime strings

- **Status:** proposed
- **Date:** 2026-08-04
- **Context:** Dogfooding v0 (see [`DOGFOODING.md`](../../DOGFOODING.md), finding
  **D-3**) hit the same wall from every direction: v0 has **no effect boundary**
  (no FFI, no syscalls) and **no runtime `String` value**, so no program can do
  I/O of any kind. Concretely:

  - The [`examples/http-service`](../../examples/http-service/) routing core runs
    and is spec-checked, but `serve`, `parse_request_line`, and `write_response`
    are `CONTRACT:` comments — the compiler cannot bind a socket, cannot read the
    request bytes, and has no `String` to hold `"GET /health HTTP/1.1"`.
  - Every command-line example returns its result as the **process exit byte**
    because that is the *only* output channel the scalar core has. There is no
    `println`, no `read_line`, no argv.
  - `tdg-stdlib`'s `std::io`/`std::fs`/`std::time`/`std::process` are honest about
    this: their effectful entry points are contract-tier, documented but
    unrunnable, precisely because this boundary does not exist.

  This is the single biggest blocker to any dogfooding program becoming a
  *deployable* application, and it is upstream of networking (ADR-0007's `serve`
  needs both a socket effect and a byte buffer).

  The project rule forbids bolting on `println` to make one example print. An
  effect boundary is a hard-to-reverse, ABI-touching design decision — exactly
  what an ADR is for — because it determines how a pure, memory-safe language
  talks to an impure OS without discarding its guarantees.

- **Decision (proposed):** design the effect boundary as **one narrow, explicit
  seam**, specified before implementation, with two coupled parts:

  1. **A runtime `String`/byte-buffer value.** A heap-backed, owned sequence of
     bytes with a length, lowered through the `Box`/allocator seam the ABI
     already specifies (`tuo_rt_alloc`/`tuo_rt_dealloc`). String *literals* exist
     in the grammar today; this gives them a runnable representation and the
     minimal operations (length, slice, byte-at, concat) needed to parse a
     request line and format a response. Depends on the allocator, so it is
     sequenced after ADR-0004's aggregate work touches the same layout code.

  2. **A single FFI/syscall effect primitive.** One narrow, `unsafe`-gated seam
     (matching the Constitution's narrow-`unsafe` posture) through which the
     runtime exposes a small set of host effects — write(fd), read(fd), and
     process exit to begin — with everything else (a `println`, a file read, a
     socket) built in tuonelang on top of that one primitive. The effect must be
     *visible in the type system* (effectful functions are distinguished from
     pure ones) so the spec runner can keep executing only the pure core in its
     deterministic sandbox, and so the honesty split the stdlib already documents
     becomes a *checked* property rather than a convention.

  As with ADR-0003 and ADR-0004, the normative spec (an extension of
  `specification/abi.md` covering the allocation boundary for strings and the
  effect ABI) plus fixtures land before code. The acceptance oracle is concrete:
  `http-service`'s `parse_request_line`/`write_response` contracts become
  runnable functions, and a CLI example gains a real `println` whose output is
  observed by a test.

- **Consequences:**
  - *Easier:* real CLIs, file processing, and (with ADR-0007) network services
    become expressible; the stdlib's contract tier shrinks as entry points gain
    runnable bodies; LLM generation success on I/O tasks rises because `println`
    stops being a hallucination.
  - *Harder:* purity and determinism must be *preserved* across the new boundary
    — the spec sandbox and the fuzz harness both assume total, effect-free stage
    functions, and an effect type discipline is required to keep that true. The
    differential suites must never run effectful programs through the pure
    interpreter path expecting agreement.
  - *Trade-off:* one narrow primitive with everything layered on top is slower to
    build out (no rich std::io on day one) but keeps the trusted surface tiny and
    auditable, which the memory-safety story requires.

- **Benchmark consideration:** this unblocks two performance-lab workloads at once
  — **`string-processing`** and **`networking`** — both currently
  `Support::Unsupported` in `crates/tuo-bench/src/lab/runtime.rs`, and it is a
  prerequisite for the whole of ADR-0007. When strings land, `string-processing`
  gains a committed program and a C peer and is measured under the lab's
  equivalent-semantics rule. Critically, the effect boundary also needs its **own**
  benchmark: syscall/FFI crossing cost is a real, measurable overhead, and the lab
  must record it (versus a C program making the same `write`) so any claim about
  I/O performance is backed by a number the compiler produced. No "fast I/O" claim
  is admissible until that entry exists. This ADR is not "accepted" until the
  string workload and the effect-crossing benchmark are committed.

## Amendments (2026-08-10)

Two corrections to the proposal above, made while staging the implementation
(Stage A = spec text, front end, MIR + verifier, reference interpreter; Stage B =
native lowering; Stage C = stdlib/examples/lab, at which point this ADR can be
accepted):

1. **`concat` and the owned-`String` operations move to the forthcoming
   allocator ADR.** The proposal's part 1 coupled the string surface to the
   heap (`tuo_rt_alloc`) because it included growable/owned operations such as
   `concat`. That coupling is unnecessary for the effect boundary itself and
   would sequence this ADR behind allocator work it does not actually need.
   ADR-0006's string surface is the **borrowed `Str`**: string literals (whose
   bytes are static data), `Str == Str` byte equality (already defined), and
   the three pure byte-level operations `std::str::len`, `std::str::byte_at`,
   and `std::str::slice`. A `Str` is a `{ptr, len}` view of an existing
   buffer — slicing narrows the view and byte-at reads through it — so **no
   heap is required**, and the ADR stands on its own. Building an owned,
   growable `String` value (and with it `concat`, formatting, and
   `String`→`Str` borrowing) is real allocator-dependent work and gets its own
   ADR rather than riding along here. These operations are deliberately
   **byte-level on the UTF-8 buffer**: `byte_at` returns one byte, and `slice`
   may split a multi-byte code point — that is the plainly documented v0
   contract, not an oversight; code-point-aware iteration is `String`-ADR
   material.

2. **The `networking` lab workload is *not* unblocked by this ADR.** The
   proposal claimed both `string-processing` and `networking`. That was wrong:
   the effect set specified here — `std::rt::write(fd)`, `std::rt::read_byte(fd)`,
   `std::rt::exit` — can write to and read from **already-open file
   descriptors** and terminate the process, but it cannot **open a socket**
   (no `socket`/`bind`/`listen`/`accept`/`connect` effect exists). A
   networking benchmark that cannot create a connection is not a networking
   benchmark. `networking` therefore stays `Support::Unsupported`, with its
   reason updated to name the missing socket effects, until ADR-0007 (or a
   successor effect ADR) specifies them. **`string-processing` is the workload
   this ADR flips**: once Stage B lands the native lowering, a committed
   program using `Str` literals and `std::str::{len, byte_at, slice}` and its
   C peer are measurable under the lab's equivalent-semantics rule.

The acceptance oracle is adjusted accordingly: the `http-service` contracts
this ADR makes runnable are the ones expressible over open descriptors and the
pure `Str` core; `serve` (socket setup) remains contract-tier pending the
socket effects.
