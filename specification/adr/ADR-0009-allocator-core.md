# ADR-0009: The allocator core — owned `String` and growable `Array`

- **Status:** accepted (2026-08-13 — all three stages landed; see Resolution)
- **Date:** 2026-08-11
- **Context:** ADR-0006's first amendment (2026-08-10) deliberately split the
  string surface in two: the **borrowed `Str`** (a `{ptr, len}` view of an
  existing buffer, no heap required) landed with the effect boundary, while
  "building an owned, growable `String` value (and with it `concat`,
  formatting, and `String`→`Str` borrowing) is real allocator-dependent work
  and gets its own ADR." This is that ADR. The same wall stands in front of
  every collection: `tdg-stdlib`'s `std::collections` is a **contract tier**
  whose `Array[Int]`-monomorphic entry points (`push`, `pop`, `get`, …) are
  documented but unrunnable, "awaiting the allocator" — and the performance
  lab's **`allocation`** workload is `Support::Unsupported` for exactly the
  same reason. The groundwork already exists on both sides of the gap:

  - [`specification/abi.md`](../abi.md) **already specifies the layouts**:
    `String` and `Array[T]` are three-word `{ptr, len, cap}` headers, and the
    allocation boundary is two C-ABI symbols (`tuo_rt_alloc`/`tuo_rt_dealloc`,
    provided as C source by `tuo-runtime`'s `alloc` module) — specified and
    tested, but **not yet linked** into any built binary, because nothing
    generated ever allocates.
  - The type system has `Ty::String` and `Ty::Array` (both correctly
    non-`Copy`), MIR has `Rvalue::Len` and `Index` projections over growable
    arrays, and the reference interpreter's `Value::Str` is already a byte
    buffer that `Str` and `String` share, with `Value::Array` beside it. What
    is missing is any way for a *program* to construct or grow one.

  Per the project rule, the operations move here rather than riding along with
  ADR-0006, and per the ADR-0004/0006 precedent the work is staged
  spec-before-code: **Stage A** = this normative text, the front end, MIR +
  verifier, and the reference interpreter; **Stage B** = native lowering, drop
  glue, and linking the C allocator; **Stage C** = stdlib executable tier,
  examples, and the performance-lab `allocation` workload, at which point this
  ADR can be accepted.

- **Decision (proposed):** land the **v0 allocator core** — the minimal owned,
  growable heap values and their operation surface — as language-provided
  builtins, exactly as ADR-0006 landed `std::rt`/`std::str`:

  1. **Owned `String`** — a UTF-8-**by-convention** byte buffer (the
    `abi.md` three-word header, unchanged). Like `Str`, its operations are
    byte-level: a `slice` may split a multi-byte code point, so a `String`'s
    bytes need not remain valid UTF-8 (this corrects `abi.md`'s earlier
    "always valid UTF-8" phrasing to match the ADR-0006 byte-level contract).
    The builtin module **`std::string`** provides:

     | Signature | Meaning |
     |-----------|---------|
     | `fn empty() -> String` | The empty string. Never traps. |
     | `fn from_str(in s: Str) -> String` | Copy `s`'s bytes into a new owned buffer. |
     | `fn push_byte(mut s: String, take b: Int)` | Append one byte. **Traps `InvalidByte`** when `b < 0` or `b > 255`. |
     | `fn append(mut s: String, in t: Str)` | Append `t`'s bytes. |
     | `fn concat(in a: Str, in b: Str) -> String` | A new owned buffer holding `a`'s bytes then `b`'s (the operation ADR-0006 deferred here). |
     | `fn len(in s: String) -> Int` | The byte length. Never traps. |
     | `fn byte_at(in s: String, take i: Int) -> Int` | The byte at `i`. **Traps `IndexOutOfBounds`** when `i < 0` or `i >= len(s)` — the `std::str` rule. |
     | `fn slice(in s: String, take a: Int, take b: Int) -> String` | The byte range `[a, b)` **copied out as a new owned `String`** — no aliasing view (Q-0012 stays deferred). **Traps `IndexOutOfBounds`** unless `0 <= a <= b <= len(s)`. |

  2. **Growable `Array[Int]`** — a *monomorphic operation surface* over the
     existing generic `Ty::Array` type, exactly as the stdlib's contract tier
     is already `Array[Int]`-monomorphic. The type `Array[T]` continues to
     exist for any `T`; the v0 builtins operate on `Array[Int]` only, and a
     call with any other element type is an ordinary type error (`T0001`).
     The builtin module **`std::array`** provides:

     | Signature | Meaning |
     |-----------|---------|
     | `fn empty() -> Array[Int]` | The empty array. Never traps. |
     | `fn push(mut xs: Array[Int], take v: Int)` | Append `v`. |
     | `fn pop(mut xs: Array[Int]) -> Option[Int]` | Remove and return the last element; `None` when empty. Never traps. |
     | `fn len(in xs: Array[Int]) -> Int` | The element count. Never traps. |
     | `fn get(in xs: Array[Int], take i: Int) -> Int` | The element at `i`. **Traps `IndexOutOfBounds`** when `i < 0` or `i >= len(xs)`. |

     Native `xs[i]` indexing and `for` over growable arrays already
     type-check and lower to MIR (`Rvalue::Len` + a bounds-checked `Index`
     projection); that existing surface is kept as is — neither widened nor
     narrowed — and its *native* lowering is Stage B's concern along with the
     rest of the heap ops.

  3. **A fourth effect builtin** — `std::rt::write_string(take fd: Int,
     in s: String) -> Int` — so an owned `String` can be printed without a
     `String`→`Str` view (which stays deferred, Q-0012). An `in` borrow of
     the header is safe for the call's duration. Same contract as
     `std::rt::write`: returns bytes written or a negative host-error value,
     never traps, **effectful**.

  The load-bearing rules:

  - **Heap operations are pure.** Allocation is deterministic computation, not
    I/O: every `std::string`/`std::array` builtin is **pure**, so specs may
    build strings and arrays freely (bounded by the interpreter sandbox's
    existing `MemoryBudget`, which counts byte-buffer and element growth).
    Only `std::rt::*` is effectful — `write_string` joins the effectful set
    and a spec reaching it is refused with the existing `R0007`. The
    transitive purity computation needs no new machinery, only the correct
    registration of the new builtins.
  - **`String` and `Array[T]` remain non-`Copy`** (`ownership.md` §2 already
    says so); passing one to a `take` parameter moves it, `mut` requires a
    mutable place (`O0004` otherwise), and drops remain meaningful — a
    dropped `String`/`Array` is exactly where Stage B's drop glue will free
    memory, so Stage A pins the lowering's drop placement with tests.
  - **`String == String` / `!=` land here**, as byte-wise content equality —
    the same contract as `Str == Str`, over the same interpreter
    representation (`Value::Str` is the shared byte buffer). Ordering
    (`<` etc.) stays a type error for both string types. In MIR the operands
    of a `String` equality are **borrow-reads** (the comparison consumes
    nothing); `specification/mir.md` §5.3 documents this.
  - **One new trap code, appended:** `InvalidByte` ("byte value out of
    range") for `push_byte`'s out-of-range argument. Reusing
    `IndexOutOfBounds` would be dishonest (nothing is indexed) and masking
    the value silently is forbidden; the trap taxonomy is append-only, so
    `TrapCode::InvalidByte` is appended to `tuo-runtime` (Rust reference and
    C shim together), to the interpreter's `TrapKind`, and to the normative
    docs in the same change.
  - **MIR extends the ADR-0006 shapes rather than inventing new ones.** The
    value-producing pure ops become a new rvalue, `Rvalue::HeapOp` (with the
    borrowed subject carried as a *place*, the discipline `Len`/`Discriminant`
    already use); the in-place mutators (`push_byte`/`append`/`push`/`pop` —
    `pop` mutates, so it lives with the mutators even though it also produces
    a value) become a new statement, `Statement::HeapMutate`, mutating
    through a `mut`-borrowed place exactly as a `BorrowMut` call argument
    does; and `write_string` joins `EffectOp`, with `Statement::Effect`'s
    arguments generalized from operands to call-style `Arg`s so the `String`
    is passed as an explicit borrow. The verifier gains `M0012`/`M0013`; the
    optimization passes treat `HeapOp` as non-foldable (it allocates and may
    trap) and `HeapMutate`/`Effect` as never-eliminable. See
    `specification/mir.md` §4.2–§4.3, §5.7.

  **Deliberately out of scope** (each stays declared-and-refused or a plain
  type error, never a silent half-feature):

  - surface `Box`/`Shared`/`Weak` values — a later ADR; the types stay
    declared, construction stays refused;
  - `Array[T]` *operations* for non-`Int` element types — the type exists,
    the op surface is monomorphic v0; a mismatched element type is an
    ordinary type error;
  - `String` ordering, formatting/interpolation, and code-point-aware
    iteration;
  - `String`→`Str` borrowing (Q-0012) — `slice` copies, `write_string`
    borrows the header, and nothing aliases.

- **Consequences:**
  - *Easier:* programs can finally *build* text and collections at runtime —
    request lines can be assembled, not just parsed; the stdlib's
    `std::collections` contract tier gains a path to an executable tier
    (Stage C); specs can exercise string/array-producing code directly,
    because heap ops are pure.
  - *Harder:* Stage B must now produce real drop glue — every construction
    site the front end accepts becomes a native allocation that must be freed
    exactly once, which is why Stage A's lowering-level drop-placement
    guarantees (drops at scope end, reassignment, and early exits; the
    verifier's use-after-drop and double-drop data-flow rules) are pinned by
    tests before any native code exists. The interpreter's `MemoryBudget` now
    counts growth, so runaway-allocation specs abort deterministically rather
    than exhausting the host.
  - *Trade-off:* a monomorphic `Array[Int]` surface is deliberately narrow —
    generic builtins would demand method dispatch or instantiation machinery
    v0 does not have. The narrow surface is honest (a wrong element type is a
    type error today, not a promise broken tomorrow) and the generic type
    underneath means widening later is additive.

- **Benchmark consideration:** the workload this ADR must flip is the
  performance lab's **`allocation`** entry (`crates/tuo-bench/src/lab/runtime.rs`),
  today `Support::Unsupported` with a reason naming the missing allocator.
  When Stage B lands the native lowering and links `tuo_rt_alloc`, Stage C
  commits the workload program (an allocate/grow/drop loop over
  `String`/`Array[Int]`) and its equivalent-semantics C peer
  (`malloc`/`realloc`/`free`), measured under the lab's rule that a `Verdict`
  is `Measured` only when both sides really compiled and ran — with no other
  change to the lab, exactly how ADR-0004 flipped `collections` and ADR-0006
  flipped `string-processing`. The **effect-crossing benchmark** ADR-0006
  deferred at acceptance stays deferred: `write_string` writes to an
  already-open descriptor just as `write` does and enables no new I/O claim,
  and the lab still publishes no I/O number — so the entry keeps waiting for
  the ADR that lands the first effectful lab workload (sockets or files).
  This ADR is not "accepted" until the `allocation` workload is committed and
  measured.

- **Resolution (2026-08-13):** all three stages landed and every acceptance
  condition is met by a committed, test-pinned artifact:
  - *Stage A (spec text + front end + MIR + interpreter, commit `1b7e648`):*
    the builtins `std::string::{empty, from_str, push_byte, append, concat,
    len, byte_at, slice}`, `std::array::{empty, push, pop, len, get}`, and
    `std::rt::write_string` resolve always, as real module symbols
    (`specification/static-semantics.md`, pinned by
    `crates/tuo-resolve/tests/resolve.rs` and the type fixtures
    `tests/types/fixtures/{ok,err}/allocator.tuo`). Heap ops are **pure** —
    `String == String` is byte equality, ordering stays a type error, and a
    spec may build strings/arrays; only `write_string` is effectful and an
    effectful spec is refused with `R0007`. MIR gained `Rvalue::HeapOp` and
    `Statement::HeapMutate` and generalized `Statement::Effect`'s args
    (`specification/mir.md`, verifier `M0012`/`M0013`), executed by the
    reference interpreter (`crates/tuo-mir-interp/tests/conformance.rs`); the
    new append-only **`TrapCode::InvalidByte`** fires for a `push_byte` argument
    outside `0..=255`. The drop-placement audit confirmed the existing generic
    Drop machinery frees each buffer exactly once across scope-end,
    reassignment, move-out, early-exit, and conditional-move.
  - *Stage B (native lowering, drop glue, linked allocator, ABI v4, commit
    `40c6b76`):* both backends lower `String`/`Array[Int]` as the
    `specification/abi.md` three-word `{ptr, len, cap}` header, every `HeapOp`
    and `HeapMutate` matching the interpreter instruction for instruction
    (alloc+memcpy, in-place doubling growth, bounds-checked load, `Option[Int]`
    pop), and `Statement::Drop` frees a non-empty buffer through
    `tuo_rt_dealloc`. Pinned by the six heap fixtures agreeing interpreter ==
    Cranelift == LLVM in `crates/tuo-cli/tests/codegen_differential.rs` and
    `crates/tuo-cli/tests/codegen_three_way.rs` (the `str_*`/`arr_*` fixtures,
    the `InvalidByte` trap included), and by `crates/tuo-cli/tests/heap_native.rs`
    (a `write_string` stdout pin and a 200k-round allocate/free **leak proxy**
    that completes in bounded memory on both backends — the evidence every
    buffer is freed exactly once).
  - *Stage C (stdlib, oracle, lab; this change):* `tdg-stdlib`'s
    `std::collections` array tier became **real pure-executable code** —
    `sum`/`max_of`/`contains`/`index_of`/`reversed` (plus `singleton`/`of2`/`of3`
    constructors) over `std::array::{empty, push, len, get}`, each with a
    worked example and an executable `spec` (the ops are pure, so the specs build
    the arrays they fold), pinned by `crates/tuo-cli/tests/stdlib.rs`. The
    redundant caller-supplied `count` parameter was dropped, since
    `std::array::len` makes it obsolete — the single obvious API. `std::io::read_line`
    moved from the `CONTRACT` tier to the **`EFFECT`** tier: it builds an owned
    `String` from the bytes `std::rt::read_byte` yields, stopping at newline or
    EOF, and is pinned by a native CLI test that pipes stdin and asserts the
    round-tripped bytes, byte count, and the EOF/`Err` path on both backends
    (`crates/tuo-cli/tests/stdlib.rs::stdlib_read_line_really_reads_natively`).
  - *Oracle:* `examples/data-pipeline` — the ADR's named oracle — now answers its
    query through a **growable-collection** path: `collect_amounts` `push`es the
    filtered subset (a data-dependent size a fixed `[Int; N]` cannot express)
    into a heap-backed `Array[Int]`, and `sum_collected` folds it; specs pin that
    path **equal** to the streaming fold for every category, and `main` returns
    the same exit byte (400 → 144), asserted by
    `crates/tuo-cli/tests/dogfood_examples.rs`.
  - *Benchmark condition:* the performance lab's **`allocation`** workload is
    `Support::Supported` with the committed
    `benchmarks/runtime/programs/tuo/allocation.tuo` (a bounded allocate/grow/free
    loop over a growable `Array[Int]` and an owned `String`, expected exit 136)
    and its equivalent-semantics C peer
    `benchmarks/runtime/programs/c/allocation.c` (`malloc`/`realloc`/`free`, same
    doubling growth, same exit), measured live by `crates/tuo-bench/tests/lab.rs`
    and `crates/tuo-cli/tests/lab_command.rs`. The supported count is now
    **seven of eight**; only **`networking`** stays unsupported (no socket-open
    effect — ADR-0007 or a successor), publishing a reason, never a number.
  - *Deliberately out of scope, unchanged (each stays declared-and-refused or a
    plain type error, never a silent half-feature):* surface `Box`/`Shared`/`Weak`
    **values** — the wrapper types stay declared, construction stays refused, a
    later ADR; `Array[T]` **operations** for non-`Int` element types (the type
    exists, the op surface is monomorphic v0; a mismatched element type is an
    ordinary `T0001`); `String` **ordering**, formatting/interpolation, and
    code-point-aware iteration; and `String`→`Str` **borrowing** (Q-0012 —
    `slice` copies, `write_string` borrows the header, nothing aliases).
