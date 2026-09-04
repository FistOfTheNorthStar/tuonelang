# ADR-0020: Constant-time code — the branchless subset and what tuonelang can honestly promise

- **Status:** accepted (2026-09-04 — **all three stages landed**; see the
  Stage A/B and Stage C resolutions for what the implementations changed about
  the decision)
- **Date:** 2026-09-04
- **Context:** ADR-0019 landed `std::crypto` (SHA-256, HMAC, PBKDF2) and the
  follow-on `std::bignum`, and both carry the same prominent caveat: **not
  constant time**, therefore safe for public values and unsafe for secrets.
  That caveat is honest, but it is also a standing defect in a *crypto*
  library, because the values a crypto library exists to process are exactly
  the ones that must not leak. PBKDF2 stretches a password. HMAC keys are
  secret. A SCRAM client proof is computed from a salted password. The library
  the previous ADR shipped is, by its own admission, the wrong tool for its
  own headline use case whenever an attacker can measure it.

  This ADR asks the narrow question that caveat provokes: **can tuonelang
  express constant-time code at all, and if so, what can it promise?** The
  question was investigated empirically before any decision was reached, and
  the findings below are what the compiler actually does today, not what it
  was assumed to do.

  **Finding 1 — the branchless primitives are already expressible.** Every
  operator a constant-time toolkit needs landed in ADR-0019 Stage A. Written
  against them, the standard idioms compile and run:

  ```tuo
  fn mask_of(take bit: Int) -> Int { 0 - bit }              // 0 -> 0, 1 -> all-ones
  fn select(take c: Int, take a: Int, take b: Int) -> Int {
      let m = mask_of(c);
      (a & m) | (b & ~m)
  }
  ```

  Their specs pass and the native binary returns the predicted value. Nothing
  in the type system, the ownership checker, or the MIR lowering objects. So
  the *expressiveness* half of the problem is already solved, which was not
  obvious in advance and is the reason this ADR is scoped as narrowly as it is.

  **Finding 2 — the naive mask idiom traps, and the trap is tuonelang's own
  doing.** `0 - bit` is arithmetic negation, and tuonelang's integers **trap**
  on overflow by deliberate design (Constitution §24). Negating `i64::MIN`
  aborts with `tuo: trap: integer overflow` — verified, not assumed. The
  emitted ARM64 for every function built on that idiom carries an overflow
  branch:

  ```
  negs x8, x0
  b.vs <trap>          ; branch-on-overflow
  ```

  For a genuine 0/1 input that branch is never taken, so it is not an
  exploitable leak today. But "safe because the input happens to stay in
  range" is not the standard constant-time code is held to, and the language's
  memory-safety guarantee and its constant-time story are in real tension at
  precisely this point. This is a *language-design* collision, not a library
  bug, which is what makes it ADR material.

  **Finding 3 — a trapless derivation exists.** The mask can be built from
  shifts alone, with no arithmetic operator anywhere, so no trap is reachable:

  ```tuo
  fn mask_of(take bit: Int) -> Int { (bit << 63) >> 63 }
  ```

  `>>` is arithmetic on a signed type (ADR-0019 Stage A), so the sign bit
  smears across the word. A `nonzero` test likewise folds bits with `|` and
  `>>` rather than `0 - x`. Specs confirm both, *including on `i64::MIN`* —
  the exact value that traps the naive version. The resulting release build is
  entirely trap-free: no `negs`, no `b.vs`, no call to `tuo_rt_trap`.

  **Finding 4 — and this is the finding that decides the ADR — the two
  backends disagree about the security property.** Compiling the *same*
  trapless `select`, Cranelift (debug) preserves the branchless form:

  ```
  and  x7, x1, x0          ; a & mask
  bic  x8, x2, x0          ; b & ~mask
  orr  x0, x7, x8          ; combine
  ```

  while LLVM at `O2` (release) **recognizes the masking idiom and rewrites
  it**:

  ```
  tst  x0, #0x1
  csel x0, x2, x1, eq      ; conditional select
  ```

  The rewrite is semantically correct and, on current ARM64 cores, `csel` is
  itself constant time — so this specific output is probably fine in practice.
  That "probably" is the whole problem. The source said *branchless* and the
  optimizer decided *conditional*; it happened to pick a constant-time
  conditional on this target, and nothing asked it to. The property was
  preserved by luck, and the debug and release binaries do not agree about
  what the program is. Whether the same idiom survives on x86-64 — as `cmov`,
  or as a real branch — is **not known**, because the workspace targets the
  host only (`TargetSpec::host`) and it could not be tested here.

  The load-bearing conclusion: **an optimizing compiler with no notion of the
  property will not preserve it on purpose.** Constant-time-ness is not a
  semantic property of the program — it is a property of the *emitted
  instructions* — and every layer between source and machine code is licensed
  to destroy it. This is not a tuonelang failing; it is why C libraries resort
  to inline assembly, volatile barriers, and per-compiler workarounds. What is
  specific to tuonelang is that the project's own rules forbid papering over
  it: the standard the codebase already holds itself to is that **a promise is
  pinned by an artifact or it is not made** (`RELEASE-0.1-GATE.md`, the
  stdlib's three-tier rule, the lab's no-superlative rule). A constant-time
  claim with no test that fails when it breaks would be exactly the kind of
  unbacked assertion those rules exist to prevent.

  Two further limits are inherent rather than incidental, and neither is fixed
  by any amount of library work:

  - **Array indexing is bounds-checked**, so a table lookup branches on the
    index. Constant-time table access therefore requires scanning the whole
    table and selecting, which is a different algorithm rather than a
    different spelling. Every S-box, every windowed exponentiation table.
  - **tuonelang cannot *verify* the property.** There is no effect marking, no
    type-level taint, and no measurement harness. Without one, any claim is an
    assertion about generated code that nothing checks.

- **Decision:**

  Adopt constant-time support as a **capability with a stated boundary**, in
  three stages, and — the part that shapes everything else — **make no
  constant-time claim that an artifact does not pin.** Stages A and B are
  additive and carry no language change; Stage C is the language change and is
  deliberately deferred until the first two have been used.

  ### Stage A — `std::ct`, the trapless branchless subset (a library)

  A new catalog module, `std::ct`, containing the primitives every
  constant-time algorithm is built from, each written **without any arithmetic
  operator** so no trap is reachable and each carrying an executable spec:

  - `mask(take bit: Int) -> Int` — `(bit << 63) >> 63`; `0 → 0`, `1 → -1`.
  - `select(take c: Int, take a: Int, take b: Int) -> Int` — branchless choice.
  - `nonzero(take x: Int) -> Int` / `is_zero(take x: Int) -> Int` — bit-folding
    tests, correct on `i64::MIN`.
  - `eq(take a: Int, take b: Int) -> Int` — `is_zero(a ^ b)`.
  - `lt(take a: Int, take b: Int) -> Int` — unsigned-style comparison built
    from shifts and masks.
  - `swap`-style conditional exchange, and `select_array` for scanning a whole
    `Array[Int]` rather than indexing it.
  - `bytes_eq(in a: Array[Int], in b: Array[Int]) -> Int` — the fixed-time tag
    comparison, the single most common real use, and the one whose naive
    early-return form is a textbook vulnerability.

  These depend on `std::bits` and nothing else, extending the declared,
  acyclic dependency graph ADR-0019 introduced.

  ### Stage B — the honest verification artifact

  Stage A is worthless as a *security* claim without something that fails when
  the property breaks. Two artifacts, in increasing strength:

  1. **A codegen-level branchlessness test.** Build the `std::ct` primitives
     on **both** backends, disassemble the emitted functions, and assert the
     instruction stream contains no conditional branch, no call to
     `tuo_rt_trap`, and no unexpected control flow. This is the test that
     would have caught Finding 4 the moment it appeared. It must run against
     both Cranelift and LLVM, precisely because they were observed to
     disagree.

  2. **A statistical timing check** over the primitives, in the spirit of
     `dudect`: run each with fixed versus random secret inputs and assert the
     timing distributions are not distinguishable. This is a weaker, noisier
     signal than the instruction-level test and must be reported as a
     *measurement*, never a proof — matching the performance lab's existing
     discipline of recording what was measured and refusing to editorialize.

  The disassembly test is the load-bearing one and Stage B is not complete
  without it.

  ### Stage C — deferred: a verified constant-time marking

  A function attribute (spelling to be decided — `#[constant_time]` or an
  effect-style marker) that makes the property **checkable rather than
  asserted**: the compiler rejects a marked function containing a data-
  dependent branch, a bounds-checked index, or a trapping arithmetic operator,
  and both backends commit to preserving branchlessness through optimization.

  This is deferred, and the deferral is the point. It touches the type system,
  both backends, and the optimizer's contract; it should be designed against
  real `std::ct` usage rather than in advance of it. **Until Stage C lands,
  tuonelang does not promise constant-time execution — it provides primitives
  that are branchless as emitted today, pinned by a test that will fail when
  that stops being true.** `std::ct`'s documentation must say exactly that.

  ### What is explicitly *not* decided here

  - **`std::bignum` is not being made constant time.** Modular exponentiation
    over secret exponents is the hard case and needs Montgomery arithmetic
    plus Stage C's guarantees. Its "not constant time" caveat stands
    unchanged, and this ADR does not weaken it.
  - **`std::crypto` is not being rewritten.** SHA-256 is already naturally
    branchless in its compression function; HMAC and PBKDF2 are not audited
    here. Whether they can adopt `std::ct` is Stage-A-usage work, not a
    decision to take in advance.
  - **TLS remains out of scope**, for the reasons ADR-0019 gave: X.509, a
    certificate store, and AEAD ciphers, none of which this addresses.

- **Consequences:**

  **Easier.** The primitives a constant-time algorithm needs become available
  as one obvious API rather than each caller re-deriving shift tricks and
  getting the `i64::MIN` case wrong. `bytes_eq` in particular removes the most
  common real vulnerability — the early-returning tag comparison — from every
  future protocol implementation. The disassembly test gives the project its
  first artifact that inspects *emitted instructions*, which is reusable
  beyond this ADR.

  **Harder, and honestly so.** The gap between "branchless as written" and
  "branchless as executed" is now a documented, tested concern rather than an
  unexamined assumption, and it constrains the optimizer: a future MIR pass or
  LLVM upgrade that rewrites a masking idiom will now break a test, which is
  the intended behavior and will occasionally be inconvenient. Constant-time
  table access requires whole-table scans, so any algorithm needing one is
  asymptotically worse and must be written differently rather than
  transliterated.

  **The trap tension is recorded but not resolved.** Trapping arithmetic is a
  deliberate safety property and constant-time code must avoid it; Stage A
  sidesteps this by using only shifts. That works for the primitives and does
  not generalize — a constant-time algorithm needing genuine addition (the
  modular arithmetic of Stage C's eventual scope) collides with the trap
  again. `std::bits`'s modular `add32`/`mul32` are the existing precedent for
  a wrapping-arithmetic escape hatch, and a future ADR may need to widen it.

  **The risk of doing this at all** is the reason for the staging: a library
  named `std::ct` invites the belief that using it makes code constant time,
  which is stronger than what Stage A delivers. The mitigation is documentary
  and must not be softened — the module states the boundary in its header, the
  way `std::bignum` states its own, and Stage C is what converts the statement
  into a guarantee.

  **Benchmark plan** (required by the project rule that every language change
  ships with one): constant-time code is *expected to be slower*, and the
  point of measuring is to quantify the price rather than to celebrate it. Add
  a `constant-time` lab workload comparing branchless `bytes_eq` against the
  early-returning form, and branchless `select` against the `if` form, both
  with C peers written the same two ways. The honest expectation is that the
  constant-time versions lose on wall clock; the lab's existing rule — record
  the measurement, draw no superlative — applies unchanged. The workload
  cannot be written before Stage A, matching the gating pattern ADR-0017 and
  ADR-0019 established.

  **Follow-up, named rather than quietly omitted.** Cross-target behavior is
  unverified: the LLVM `csel` rewrite was observed on ARM64 only, and whether
  the same idiom yields `cmov` or a branch on x86-64 is unknown because the
  workspace targets the host. That question should be answered before any
  constant-time claim is made for a non-ARM target, and Stage B's disassembly
  test is the natural place to answer it once cross-compilation exists.

---

## Stage A/B resolution (2026-09-04)

Both stages landed. Implementing them changed three things this ADR had
asserted, and each correction is recorded rather than quietly folded in.

**`std::ct` shipped as the catalog's sixteenth module**, self-contained and
entirely executable: `mask`, `select`, `nonzero`, `is_zero`, `eq`, `ne`,
`is_negative`, `lt`, `gt`, plus the scans `select_array` and `bytes_eq` and the
local array builders `of2`/`of3`. Fourteen specs, all passing. It declares no
dependency on another module — the ADR had expected it to need `std::bits`, and
it does not, because every primitive is built from operators rather than
library calls.

**Correction 1 — the "no arithmetic" rule was overstated, and is now precise.**
The Decision above says the primitives are written "without any arithmetic
operator so no trap is reachable". Two of them do use arithmetic: `lt` computes
`(a >> 1) - (b >> 1)`, and the array scans increment a loop counter. Both are
safe, and for stated reasons — halving first bounds the operands to
`[MIN/2, MAX/2]`, so the difference is at most `MAX` in magnitude and the
subtraction **provably cannot overflow for any input**; the loop counter is
bounded by a public array length. But "this module contains no `-`" would have
been a false claim a reader could check in ten seconds, so the module documents
the invariant it actually keeps: *no arithmetic whose overflow check could
depend on secret data*.

**Correction 2 — the array scans are weaker than Stage A implied, because
tuonelang emits bounds checks that survive optimization.** `bytes_eq`'s first
implementation clamped a possibly-out-of-range index into `b`; the clamp was
correct and provably never trapped, but it did not convince LLVM, which kept a
bounds check and its trap edge inside the loop. Rewriting the loop to run over
`min(len_a, len_b)` — so every index is trivially in range for both arrays —
did not remove it either. Testing a minimal `while i < len(xs)` summing loop
showed the check survives there too, so **this is a property of the compiler
today, not of how `std::ct` is written**, and no rewriting inside the module
can fix it.

The consequence is a weaker but honest claim for the scans. They are *not*
branch-free; what they promise is that their control flow depends only on array
*lengths*, which are public, and never on array *contents*. That is the
property a tag comparison actually needs, and it is what the tests pin.
`the_bounds_check_limitation_is_the_compilers_not_this_modules` pins the scope,
so if the compiler ever gains bounds-check elimination the test fails and says
the documentation is now out of date — good news arriving as a visible failure
rather than as silent staleness.

**Correction 3 — the two backends require different assertions, and the reason
is structural.** Stage B expected one branchlessness rule applied to both.
Cranelift is deliberately non-optimizing, so it keeps every trap check
tuonelang's semantics imply *even on compile-time constants*: `mask`'s
`(bit << 63) >> 63` arrives with four ADR-0019 shift-amount comparisons against
the literal 63, and `lt`'s provably-unreachable overflow check survives.
Demanding their absence would be demanding optimization from a backend that
states it does not optimize. So the release build (LLVM) is held to the strict
rule — no conditional branch and no trap edge at all — and the debug build is
held to the rule that matters on both: no branch that is not a trap guard. The
relaxation is checked rather than waved through.

**The observation that motivated the ADR reproduced exactly.** LLVM rewrites a
branchless `select` into `csel`, and it also converts an `if`-based `select`
into the *same* `csel` — so on the release backend the branchless and branchful
spellings are indistinguishable in the emitted code. That is precisely why the
Cranelift assertion is not redundant: it is the backend that still shows the
difference.

### The test that could not fail

The disassembly test initially passed against a deliberately branch-ful
`select`. Its instruction parser took the wrong tab-separated field of
`objdump`'s output and returned an *operand* instead of a mnemonic, so
`is_conditional_branch` was asked whether `0x100000630` was a branch, always
answered no, and every assertion in the file passed unconditionally.

This is worth recording because of what it implies about this ADR's whole
approach. Stage B's argument is that a constant-time claim is worthless without
an artifact that fails when the property breaks — and the artifact was, for its
first hour, exactly the unbacked assertion it existed to prevent, while
reporting success. It was caught only by deliberately sabotaging `std::ct` and
noticing the tests stayed green.

Two mitigations followed. `mnemonic_parsing_finds_real_branches` pins the
parser against real `objdump` lines, so the failure mode cannot recur silently.
And both backend assertions were verified by sabotage: replacing `select`'s
mask with an `if` makes the Cranelift test fail on a `b.eq`, and giving
`nonzero` a data-dependent loop — which LLVM cannot flatten — makes the release
test fail on a `b.lt`. **A test that has never been observed to fail is a
hypothesis, not evidence**, and that should be the standing rule for any future
artifact of this kind.

### Two more defects the artifacts caught

Recorded because both were invisible to every test that existed at the time,
and both are the kind that decay silently rather than break loudly.

**Three doc examples did not compile.** `select_array`, `bytes_eq`, and
`element_or_zero` illustrated themselves with the module's own array builders
written unqualified — `of3(7, 8, 9)` rather than `std::ct::of3(7, 8, 9)` —
which resolves inside the module and *not* from a caller's scope, which is
where a copied example runs. A doc example that does not compile is worse than
no example, so `every_doc_example_in_std_ct_is_accurate` now compiles all
fourteen as a caller would write them and checks each against its documented
value. Verified by sabotage: changing one stated value makes the test fail and
name the example.

**`bytes_eq`'s prose outlived its implementation.** It said "the loop runs over
the first array's length" — true of the version that clamped, false of the
version that iterates the shorter of the two. The sentence was load-bearing,
since it is the claim about what the timing reveals, and nothing would have
flagged it.

### Still outstanding, named rather than omitted

- **Stage C is not done**, and the promise stays limited accordingly: `std::ct`
  is branchless *as emitted today, pinned by a test*, which is not a
  constant-time guarantee.
- **Cross-target behavior is unverified.** Everything above was measured on
  ARM64. Whether the same idioms yield `cmov` or a real branch on x86-64 is
  unknown, because the workspace targets the host only.
- **The benchmark workload is not written.** The Consequences section commits
  to a `constant-time` lab workload quantifying the price of branchless code;
  Stage A makes it writable, and it has not been added.
- **`std::crypto` and `std::bignum` do not use `std::ct`.** Adopting it is
  Stage-A-usage work, and both keep their existing "not constant time" caveats
  unchanged until they do.

---

## Stage C resolution (2026-09-04)

Stage C landed, earlier than this ADR planned. The Decision above deferred it
"until the first two have been used", on the reasoning that it touches the type
system, both backends, and the optimizer's contract. That estimate was wrong in
a specific and instructive way: **the marking is a front-end discipline, and
needs no backend involvement at all.** Stage B already owns the
backend-and-optimizer half of the problem by disassembling real binaries, so
Stage C only had to constrain the *source*. Scoped that way it is a checker
pass, not a cross-cutting change.

### What shipped

The attribute `#[constant_time]`, and with it tuonelang's first attribute
syntax: the `#` token, an `attribute` production, an `Attribute` CST node, an
`attributes()` accessor on every declaration view, and the field on HIR
`Function`. `item` is not `[FROZEN]` and `#` was previously a lexical error, so
the change is additive by the same argument ADR-0019 Stage A used;
GRAMMAR-VERSION moves 0.2 → 0.3 and EDITION stays 2024.

The checker (`T0017`–`T0021`) refuses, inside a marked function, data-dependent
control flow, array indexing, trapping arithmetic, and calls to unmarked
functions — and reports an unrecognized attribute as an error rather than
ignoring it. `std::ct`'s ten scalar primitives now carry the attribute and are
verified by the compiler on every build.

### Correction — the marking is checkable, but `std::ct::lt` had to change

This ADR predicted a conflict, and it appeared exactly where expected: `lt`
computed `(a >> 1) - (b >> 1)`, which the Stage A/B resolution had already
recorded as provably non-overflowing. The checker refused it, because the
checker cannot follow that proof.

The planned response was a documented exemption. That turned out to be
unnecessary: `lt` can be written with **no arithmetic at all**. Where two
integers differ, the highest differing bit decides the unsigned comparison —
`a < b` exactly when that bit is set in `b` — and differing sign bits invert
the answer. Thirteen cases pass, including `MIN` against `MAX` in both
directions.

The outcome is worth naming, because it is the argument for having a checker at
all rather than a convention: **the gate rejected working code, and the code was
genuinely improvable.** The old `lt` was correct; the new one is *checkable*,
which is a different and better property. A convention would have produced a
comment saying "this subtraction is safe, trust me"; the checker produced a
function that needs no such comment.

### The exemption that is real, and visible

`select_array` and `bytes_eq` cannot be marked. A scan needs a loop (`T0017`)
and indexing (`T0018`) by its nature, and no rewriting removes that — Stage B
already established that tuonelang's bounds check survives optimization even
for the canonical `while i < len(xs)` loop.

The exemption is therefore an **absence, not an override**. There is
deliberately no `#[allow]` and no escape hatch: a function that cannot satisfy
the checker simply goes unmarked, and a reader can tell which functions the
compiler verified by looking for the attribute. Adding an override would have
recreated exactly the problem this ADR was opened about — a function that looks
guaranteed while carrying no checked guarantee.

### The three links, and why none is the guarantee alone

Stage C completes a chain that should be stated plainly, since each link is
easy to mistake for the whole:

1. **The source** is constrained by the checker (Stage C) — no data-dependent
   construct is *written*.
2. **The emitted code** is checked by disassembly on both backends (Stage B) —
   no data-dependent branch is *generated*, which the source rule alone cannot
   promise, since an optimizer may rewrite a branchless idiom into a
   conditional.
3. **The execution** is a *hardware* property, beyond any compiler's reach and
   still unverified here.

So tuonelang can now say something considerably stronger than before Stage C,
and still short of a guarantee: the source is verified branchless, the emitted
code is tested branchless, and the hardware is trusted rather than checked.
`std::ct`'s documentation says exactly that.

### Still outstanding

- **Cross-target behavior is unverified.** Everything was measured on ARM64;
  the workspace targets the host only.
- **The `constant-time` benchmark workload is not written.** Stage A made it
  writable and it remains undone.
- **`std::crypto` and `std::bignum` do not use `std::ct`**, and keep their
  "not constant time" caveats until they do. Marking their hot paths is now
  *possible*, which it was not before this stage.
- **The attribute applies only to functions.** Nothing yet marks a type as
  secret-carrying, so the checker verifies how a function computes, never what
  data flows into it. A taint discipline is a much larger design and is not
  proposed here.
