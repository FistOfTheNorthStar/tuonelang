# ADR-0019: Bitwise operations and the crypto primitives

- **Status:** accepted (2026-09-04 — **Stages A and B landed**, including the
  entropy primitive and both gating benchmark workloads; only `md5` remains,
  see the Stage B resolution)
- **Date:** 2026-09-01
- **Context** (*describes the state **before** this ADR; Stage A has since
  changed it — see the Stage A resolution*)**:** A PostgreSQL client — the connector itself deliberately *not*
  written here — is the first dogfooding target that the language cannot
  express at all, rather than merely express awkwardly. Two distinct gaps
  block it, and they are separable: one is a missing **operator surface**, the
  other a missing **library**. Only the first is a language change.

  **1. There are no bitwise operators.** This is not an oversight to be
  patched but an explicit, documented v0 commitment. `grammar.ebnf`'s
  punctuation production lists `|` with the comment *"pattern alternative
  only; NOT a bitwise operator"*, and `crates/tuo-lexer/src/token.rs:155`
  says the same on `TokenKind::Pipe`: *"Not a bitwise operator: v0 has no
  bitwise expression operators."* There is no `&`, `^`, `~`, `<<`, or `>>`
  token anywhere in the lexer. The grammar even notes that `<`/`>` are
  *"comparison/relational, shift-free"*. So the absence is deliberate and
  stated in three places — which is precisely why re-adding it is an ADR and
  not a patch.

  The PostgreSQL wire protocol is defined in terms the language therefore
  cannot spell. Every message is length-prefixed with a big-endian `Int32`;
  every parameter in a `Bind` message, every column in a `DataRow`, and the
  `ReadyForQuery` transaction status are framed the same way. Assembling or
  decoding a four-byte big-endian integer is the single most common operation
  in the protocol, and its natural form is `(b0 << 24) | (b1 << 16) |
  (b2 << 8) | b3`. Today that must be written as
  `b0 * 16777216 + b1 * 65536 + b2 * 256 + b3`.

  To be precise about the cost, since it would be easy to overstate: on
  well-formed byte input the two forms are genuinely equivalent, and an
  out-of-range byte corrupts the adjacent field in *both* (verified — a spec
  asserting `arith_form(0, 0, 256, 0) == arith_form(0, 1, 0, 0)` passes, and
  so does the shift-form equivalent). The multiply-add form is therefore not
  *silently wrong*; it is unreadable, unmaintainable at the scale a protocol
  demands, and — the part that actually blocks — **it does not generalize**.
  Masking (`x & 0xFF`), field extraction (`x >> 24`), flag testing, and the
  parity/rotation work underneath any checksum have no arithmetic spelling at
  all. The framing case is merely the most visible instance of a surface
  that is missing outright, which is why gap 2 below is the sharper argument.

  **2. There is no crypto, and PostgreSQL authentication requires it.**
  A Postgres server that accepts a connection replies with an
  `AuthenticationRequest`. The realistic responses are `AuthenticationMD5Password`
  (`md5(md5(password + username) + salt)`) and, on any modern server default,
  SASL/**SCRAM-SHA-256** — which needs SHA-256, HMAC-SHA-256, and PBKDF2
  layered on top. None of these are expressible: they are *defined* in terms
  of rotations, xors, and shifts. This is the sharpest possible demonstration
  of gap 1, because it shows the missing operators are not a convenience but a
  hard expressiveness boundary. SHA-256 is not slow to write without bitwise
  ops — it is impossible.

  The irony is exact and worth recording: **this workspace already contains a
  SHA-256 implementation.** `crates/tuo-package/src/sha256.rs` is hand-rolled
  precisely so the workspace need not take a crypto dependency, and it is
  written in Rust with `rotate_right`, `^`, `>>`, and `&`. The runtime likewise
  implements splitmix64 and FNV-1a in C (`crates/tuo-runtime/src/map.rs`) to
  back `Map`'s hidden index. So bit manipulation happens *inside* the
  toolchain today, in two languages, and is unreachable from the language the
  toolchain compiles. tuonelang cannot currently express its own package
  manager's checksum function.

  **The integer types are already there.** `IntKind` (`crates/tuo-types/src/ty.rs:24-44`)
  has all ten widths — `I8`…`Isize`, `U8`…`Usize` — and both backends already
  know each one's width and signedness (`crates/tuo-codegen-cranelift/src/abi.rs:64-77`).
  This is what makes the change genuinely additive: it needs no new type, no
  new layout, and no ABI bump. The unsigned types exist and are currently
  nearly unusable, because the operations that give them their purpose are
  missing.

- **Decision:** add bitwise operations as **two independent increments**, the
  second depending on the first. Split this way because they are separable
  decisions with different reversibility: Stage A changes the *frozen surface
  grammar*, Stage B adds library code written in tuonelang that the ADR-0018
  cheatsheet and the stdlib's three-tier rule already govern.

### Stage A — the operator surface (a language change)

Six operators, on integers only:

| Operator | Meaning | Notes |
|----------|---------|-------|
| `a & b` | bitwise and | both operands the same integer type |
| `a \| b` | bitwise or | **see the ambiguity note below** |
| `a ^ b` | bitwise xor | |
| `~a` | bitwise not | unary, integers only |
| `a << n` | left shift | `n` an integer shift amount |
| `a >> n` | right shift | **arithmetic** on signed, **logical** on unsigned |

Three sub-decisions carry the real weight:

**The `|` ambiguity is the reason this is an ADR.** `|` is currently the
pattern-alternative separator (grammar section (H)), and the grammar
explicitly reserves it as such. Making it also a binary operator creates a
genuine grammatical conflict in exactly one place: a `match` arm's pattern
list, where `A | B` must stay two alternatives and never an or-expression.
The resolution is that patterns and expressions are **already disjoint
grammatical contexts** — the parser knows which it is parsing — so `|` is
lexed identically and disambiguated by context, with no new token and no
lookahead change. This is the single production this ADR touches that is
`[FROZEN]`-adjacent, and the claim that it is safe must be *proven by the
parser's own recovery tests*, not asserted here.

**`>>` is signedness-directed, not a second operator.** Rather than
introducing distinct arithmetic and logical shift spellings, `>>` sign-extends
on a signed type and zero-fills on an unsigned one. This is what makes the
existing `IntKind` signedness carry its weight, and it means `U32 >> n` is the
correct primitive for SHA-256 without a special operator. A protocol author
who wants zero-fill writes the value as unsigned — which is what it *is*.

**Shift amounts out of range trap.** `x << 64` on a 64-bit value is a bug, not
a value. It joins the existing trap taxonomy as an appended
`TrapCode::InvalidShift`, following the append-only rule that
`crates/tuo-runtime/src/lib.rs:82` states and that `InvalidByte = 4` already
set the precedent for. Constant folding must **never** fold a trapping shift,
exactly as the MIR optimizer already refuses to fold `1/0` — a folded trap
would erase an observable abort.

Precedence sits between comparison and the arithmetic operators, in the
conventional order `~` > `<<`/`>>` > `&` > `^` > `|`. Mixed-type operands are
a type error, never an implicit widening.

### Stage B — `std::crypto`, written in tuonelang

A thirteenth stdlib module, in the **executable tier** — pure computation, so
every function carries a real `spec`, and nothing in it is an effect or a
contract stub:

| Function | Purpose |
|----------|---------|
| `sha256(in data: Str) -> String` | the hex digest |
| `sha256_bytes(in data: Str) -> Array[Int]` | the raw 32 bytes |
| `hmac_sha256(in key: Str, in msg: Str) -> Array[Int]` | RFC 2104 |
| `pbkdf2_sha256(in pw: Str, in salt: Array[Int], take iters: Int) -> Array[Int]` | SCRAM's key derivation |
| `md5(in data: Str) -> String` | **legacy**, for `AuthenticationMD5Password` only |
| `base64_encode` / `base64_decode` | SCRAM message framing |

**SCRAM additionally needs a random nonce**, which is neither bitwise nor
crypto but an *OS entropy* primitive — RFC 5802's client-first message carries
a fresh client nonce, and a predictable one breaks the exchange's security
property. `std::rt` has no such primitive today, so Stage B also needs one
small additive effect builtin on the ADR-0013 OS seam (a `random_bytes`-shaped
`fn`, reading the platform CSPRNG), exposed through `std::crypto`. It is
called out here so Stage B does not discover it late: without it the crypto
tier can verify a server's proof but cannot correctly *initiate* an exchange.

Plus a small `std::bits` for the byte-order work the wire protocol needs
(`be_u32`, `be_u16`, and their decoders) — the operations Stage A makes
expressible, given one obvious spelling rather than re-derived per caller.

`md5` ships **documented as broken for security** and present solely because
the Postgres protocol still offers it; the doc comment must say so, since a
stdlib that ships MD5 without that sentence teaches the wrong thing to exactly
the LLM audience this language is built for.

**Every one of these has published test vectors** (FIPS-180-4, RFC 2104,
RFC 6070, RFC 4648), which is what makes them the ideal Stage B payload: the
specs are not invented, and a `spec` that reproduces a published vector is
proof rather than self-agreement. The strongest single acceptance test is that
`std::crypto::sha256` must agree with `tuo-package`'s Rust `sha256` on the same
input — the language reproducing its own toolchain's checksum function, which
is currently impossible.

**Why SCRAM and not just MD5.** `password_encryption` has defaulted to
`scram-sha-256` since PostgreSQL 14, and MD5 auth is disabled outright on
several managed providers, so a connector that implements only
`AuthenticationMD5Password` cannot authenticate against a current server at
all. SCRAM is therefore the *primary* target and MD5 the compatibility shim,
which is why the function table above is sized for RFC 7677 (SCRAM-SHA-256,
over RFC 5802's SCRAM framework) rather than for
the simpler MD5 challenge. Note also that SCRAM **authenticates but does not
encrypt**: it establishes identity over whatever transport carries it, so
until TLS exists (out of scope below) the honest use is a local socket or an
already-trusted network, and the docs must say so rather than imply a
connection is secure.

**A Stage-B design note Stage A already surfaces.** tuonelang's integer
arithmetic **traps** on overflow (Constitution §24), but every hash function is
defined over *modular* arithmetic — SHA-256's compression function adds mod
2³² by definition. So `a + b` on a `U32` is the **wrong** primitive: it aborts
exactly where the algorithm requires wraparound. The correct spelling is a
masked add on `Int` — `(a + b) & 0xFFFF_FFFF` — which Stage A makes
expressible and which is *verified working today*: a spec asserting
`add32(4294967295, 1) == 0` and `rotr32(1, 1) == 2147483648` passes through
the interpreter. Stage B must therefore expose the masked operations
deliberately (in `std::bits`) rather than letting each call site rediscover
that the obvious `+` traps; that is an API decision, not an oversight, and it
is recorded here so it is made once.

- **Deliberately out of scope:** TLS (ADR-0014 and ADR-0017 both excluded it,
  and this ADR does **not** reverse that — SHA-256 and HMAC are hash
  primitives, not a TLS stack; TLS additionally needs X.509, a certificate
  store, and AEAD ciphers, and remains out); the PostgreSQL connector itself
  (the user's own framing, and correctly so — it is a library written *in*
  tuonelang on these primitives, not compiler work); DNS (still properly
  written in tuonelang on ADR-0017's UDP); rotate operators as builtins
  (`rotl`/`rotr` are two shifts and an `or`, so they belong in `std::bits`,
  not the grammar); and bitwise ops on `Bool` (`&&`/`||` already exist and
  are the correct spelling).

- **Consequences:**
  - **`GRAMMAR-VERSION` moves 0.1 → 0.2, and `ABI_VERSION` 10 → 11.** Unlike ADR-0004's, ADR-0008's, and
    ADR-0017's additive productions, this adds **new tokens** (`&`, `^`, `~`,
    `<<`, `>>`) and overloads a reserved one (`|`). By `grammar.ebnf`'s own
    versioning rule the version marker and the file move together, and
    release-gate criterion G1 reads that marker — so the gate is implicated
    and must be re-checked, not assumed.
  - **The cheatsheet regenerates or it lies silently.** ADR-0018's brief is
    generated from the compiler and pinned byte-for-byte; a new operator
    surface that does not reach it produces confidently wrong generations,
    which is the exact failure mode the cheatsheet exists to prevent.
  - **ABI v10 → v11.** No new type, layout, or runtime shim — the appended
    `InvalidShift` trap code is the only runtime-visible addition, and the
    taxonomy is explicitly append-only, so nothing existing is reinterpreted.
    The version still moves, because the set of integers `tuo_rt_trap`
    accepts grew and `ABI_VERSION` is the marker that makes such a change
    visible rather than silent.
  - **The benchmark plan the project rule demands:** a `sha256-hash` runtime
    workload (a fixed-size digest loop, with C and Go peers — Go's
    `crypto/sha256` being the apt comparison, as `encoding/json` was for
    ADR-0016), plus a `wire-decode` workload measuring the big-endian framing
    that motivated Stage A. Both are `Unsupported` with the exact reason until
    Stage A lands, flipping the moment it does — the mechanism ADR-0014's
    `networking` entry demonstrated.
  - **`std::bits` and `std::crypto` are the first stdlib modules whose
    correctness is externally defined**, by published vectors rather than by
    the project's own reasoning. That is a strengthening: a spec that
    reproduces FIPS-180-4 cannot be self-consistently wrong.
  - **Trade-off accepted:** `|` becoming context-dependent is a real cost to
    the grammar's "a token is always one thing" simplicity, which the lexer
    doc comment currently enjoys. The alternative — a distinct spelling for
    bitwise or, such as `bor` — was rejected as worse: it would make every
    protocol and crypto routine read unlike every other language an LLM has
    been trained on, which cuts against this language's stated purpose.

- **Resolution — Stage A (2026-09-01):** the operator surface landed, and every
  acceptance condition is met by a committed, test-pinned artifact.

  - *Lexer:* five new tokens (`&`, `^`, `~`, `<<`, `>>`), with `<<`/`>>` matched
    before `<`/`>` by maximal munch. `TokenKind::Pipe`'s doc comment — which
    previously asserted *"v0 has no bitwise expression operators"* — now records
    the two-context rule instead.
  - *Parser:* four levels (`bit_or_expr` → `bit_xor_expr` → `bit_and_expr` →
    `shift_expr`) inserted between `compare_expr` and `range_expr`, plus `~` in
    `unary_expr`. **Both** parsers moved together — the handwritten one and the
    Chumsky `grammar.rs` oracle the differential compares against — since a
    divergence between them is itself a test failure. The depth pre-scan's
    `BINARY` array gained the five chaining operators, so a long `a & b & …`
    chain cannot rebuild the stack overflow the fuzz sweep found on `+` chains.
  - *The `|` question is settled empirically, not by assertion.*
    `precedence.rs::pipe_is_pattern_alternation_in_patterns_and_bitwise_or_in_expressions`
    parses a single program containing `1 | 2 => n | 8` and asserts the arm's
    pattern is one `OrPattern` while its body is one `BinaryExpr`. The
    disjoint-production argument holds structurally: `pattern()` never descends
    into the expression chain.
  - *Types:* `&`/`|`/`^` unify their operands; **shifts deliberately do not**
    (the amount is a bit count, so `x: U32 << 3` needs no cast), and the result
    takes the left operand's type. Floats and `Bool` are rejected (`T0006`), so
    `~` and `!` stay distinct operators. Pinned in `tuo-types/tests/typeck.rs`.
  - *MIR:* `BitAnd`/`BitOr`/`BitXor`/`Shl`/`Shr` + `UnOp::BitNot`. The verifier's
    same-type rule gained its one exception, with `check_shift_types` enforcing
    integers on both sides in its place.
  - *The shift bound is the load-bearing correctness decision.* x86's shift
    instructions mask the amount to 6 bits, so an unguarded `1 << 64` would be
    `1` natively while the interpreter trapped — a silent three-way divergence,
    the exact class of bug the differential suites exist to catch. Both backends
    emit an explicit guard, and `TrapCode::InvalidShift` (5) joins the
    append-only taxonomy. Constant folding refuses an out-of-range shift for the
    same reason it refuses `1/0`: folding it would erase an observable abort.
  - *Backends:* both lower all six operators, and
    `codegen_three_way.rs::bitwise_operators_agree_across_all_three_engines` +
    `…::an_out_of_range_shift_traps_across_all_three_engines` pin
    interpreter == Cranelift == LLVM on both the values and the trap.
  - *A latent bug this work exposed and fixed:* `trap_runtime_c_source()` built
    its C `switch` from a hand-maintained array, and the test that was supposed
    to mirror it hardcoded **the same list** — so both were updated in lockstep
    by hand and neither caught an omission. `InvalidShift` was therefore emitted
    as `tuo: trap: unknown` natively while the interpreter named it correctly,
    and the test still passed. Both now iterate a single `TrapCode::ALL`, and
    `all_lists_every_trap_code_in_discriminant_order` walks the discriminants via
    `from_i32` to prove that list is complete — so the next appended trap cannot
    repeat this.
  - *A second bug the shift exemption itself introduced, found by testing the
    narrow-width path rather than only the `Int` one:* the interpreter's
    `eval_binary` opened with `debug_assert_eq!(ka, kb, "verifier guarantees
    matching operand kinds")` — an assumption that was true until shifts
    stopped unifying their operands. `U32 >> Int` therefore panicked the
    interpreter. Two properties made this worth pinning permanently rather
    than just fixing: it was invisible in release builds (a `debug_assert`),
    and invisible for `Int` operands (the overwhelmingly common case), so the
    entire existing bitwise test surface passed while it was live. The
    assertion now exempts `Shl`/`Shr` exactly as the verifier does, and
    `bitwise_narrow.tuo` pins the narrow-width case three-way — including
    that `>>` is logical on `U32` and arithmetic on `I32`, and that the
    width bound is per-type (an amount of `32` is valid for `Int` and traps
    for `U32`).
  - *The same stale assumption appeared a third time, in the constant folder*
    (`fold_binary`'s `debug_assert_eq!` on operand kinds) and was fixed with
    the interpreter's. Blessing the golden fixtures also exposed that
    `fold_unary` had no `BitNot` arm, so `~` on a constant silently went
    unfolded — visible only because the `.opt.mir` golden showed
    `bitnot(const 70)` surviving. Both are pinned by unit tests and by the
    `const_fold_bitwise` / `const_fold_shift_traps` fixture pair, whose diff
    *is* the rule: the non-trapping expression folds to a single constant,
    and the trapping `1 << 64` survives into the optimized MIR untouched.
  - *Documentation moved with the code, because a stale brief fails silently:*
    `grammar.ebnf` (productions, punctuation table, and a version-log entry
    explaining why GRAMMAR-VERSION stays 0.1 — no [FROZEN] production's surface
    changed, and the new tokens were previously lexical errors, so no existing
    program changes meaning), `lexical-grammar.md`, `mir.md` §5.2/§5.3 + the trap
    table, `static-semantics.md` §3.9, `REFERENCE.md`, and the ADR-0018
    cheatsheet — which had actively claimed *"There are NO bitwise operators"*,
    the precise silent-failure mode this ADR's Consequences predicted.

  Both version markers moved with the change: **`GRAMMAR-VERSION` 0.1 → 0.2**
  (the token table grew) and **`ABI_VERSION` 10 → 11** (the trap taxonomy
  grew). Neither is an *edition* bump — `EDITION` stays `2024`, since no
  [FROZEN] production's surface changed and no existing program's meaning
  moved, which is the condition Constitution §30 attaches a breaking change
  to. Release-gate criterion G1 stays **MET** with its pin moved to `0.2`
  (`release_gate.rs` green), the generated cheatsheet reports `grammar 0.2`
  because it reads the marker rather than restating it, and the full
  workspace suite passes.

- **Resolution — Stage B (2026-09-01):** `std::bits` and `std::crypto` landed
  as the catalog's thirteenth and fourteenth modules, both in the **executable
  tier** — every public function is pure, spec'd, and runs.

  - *`std::bits`* owns the width-and-byte-order layer: masking (`low32`/
    `low8`), the **modular** arithmetic (`add32`/`mul32`), rotation and
    logical shifts (`rotr32`/`rotl32`/`shr32`/`shl32`), big-endian assembly
    and splitting (`be32`/`be16`/`byte_of_be32`/`byte_of_be16`), and
    inspection (`test_bit`/`count_ones32`). The **overflow rule** this ADR
    predicted is enforced here rather than left to callers: tuonelang's `+`
    traps, but every hash is defined over modular arithmetic, so `add32` is
    the named, documented way to ask for the wraparound and no call site
    rediscovers that the obvious `+` aborts.
  - *`std::crypto`* owns SHA-256 (`sha256`/`sha256_bytes`), HMAC-SHA-256,
    PBKDF2-HMAC-SHA-256, Base64, hex rendering, and the byte/text bridge.
  - *Correctness is externally defined*, which is what makes these specs proof
    rather than self-agreement: they assert **published vectors** — FIPS 180-4
    (SHA-256, including the multi-block case), RFC 4231 (HMAC cases 1, 2, and
    6, the last exercising the over-long-key path), the published
    PBKDF2-HMAC-SHA-256 counterparts to RFC 6070, and RFC 4648 §10 (all seven
    Base64 vectors). All 52 specs pass through the reference interpreter, and
    the modules run **natively on both backends**.
  - *The headline claim is discharged by a real test.*
    `tuo-cli/tests/crypto_cross_check.rs` builds a **native tuonelang binary**
    with the shipped catalog sources and compares its digests against
    `tuo-package`'s **Rust** `sha256` across nine inputs chosen to straddle
    every padding boundary (0, 55, 56, 64, and multi-block). The two agree
    byte for byte: the language now reproduces its own package manager's
    checksum function, which was impossible before Stage A.
  - *One catalog invariant changed deliberately.* `std::crypto` uses
    `std::bits`, making it the first module that is not standalone. Rather
    than duplicate `rotr32`/`add32`/`be32` into a second copy free to drift,
    `tuo-cli/tests/stdlib.rs` now carries a `DECLARED_DEPENDENCIES` table:
    each module is checked with exactly its declared dependencies and nothing
    else (so an undeclared use still fails to resolve), and
    `the_dependency_graph_is_declared_and_acyclic` proves the listed edges are
    the only ones and form no cycle. The invariant weakened from "every module
    is standalone" to "the dependency graph is declared and acyclic", which is
    the honest statement of what is now true.
  - *The entropy primitive landed too, closing the SCRAM story.* The Context
    section flagged that SCRAM needs a random nonce and `std::rt` had no
    entropy source. `std::rt::random_byte` is now a real effect builtin
    (**ABI v12**), reading the platform CSPRNG via `getentropy` and returning
    `-1` on failure rather than substituting a fallback — a nonce drawn from a
    predictable stream silently voids the property it exists to provide, so
    failure must be visible. It is deliberately byte-at-a-time, the same shape
    as `read_byte`, so it needed no new ABI concept. `std::crypto::nonce(n)`
    wraps it, returning an **empty** array on failure rather than a short one
    (a short nonce is a weak nonce). Both are the **effect tier**: drawing
    randomness cannot be pure, so `R0007` refuses them inside a spec with no
    new mechanism, and `stdlib_crypto_entropy_really_draws_randomness_natively`
    pins them on both backends. The emitted C reads `getentropy` where it is
    declared (macOS, the BSDs, glibc >= 2.25) and falls back to reading
    `/dev/urandom` — the same kernel CSPRNG, needing no header and no version
    test — where it is not; the fallback branch was compiled and run in
    isolation rather than assumed, since CI's glibc is the path this host
    never takes — checking the properties a *broken*
    implementation would violate (in-range bytes, exact length, and two
    independent 32-byte nonces differing, which catches a stubbed constant, a
    zeroed buffer, or a repeated PRNG stream).
  - *The motivating case is discharged end to end.*
    `a_native_scram_sha256_client_proof_matches_the_rfc_7677_vector` has a
    native tuonelang binary compute a full **SCRAM-SHA-256 client proof** and
    compares it against RFC 7677 §3's **published** vector. That one
    assertion exercises essentially all of Stage B at once — Base64 both
    ways, PBKDF2-HMAC-SHA-256 over 4096 iterations, two HMACs, a bare
    SHA-256, and a byte-wise XOR — over Stage A's operators. Since
    PostgreSQL has defaulted `password_encryption` to `scram-sha-256` since
    version 14, this is the exchange a connector actually needs, not the
    legacy MD5 challenge.
  - *The gating benchmark workloads landed*, discharging the project rule that
    every language change gets a benchmark plan. **`sha256-hash`** measures the
    full FIPS 180-4 compression function over a fixed 64-byte message against a
    `uint32_t` C peer and Go's standard `crypto/sha256`; **`wire-decode`**
    measures the length-prefixed framing walk — the PostgreSQL message header
    this ADR was opened for — against a C peer making the identical shifts and
    a Go peer using `encoding/binary`. Both are workloads that **could not be
    written before Stage A**: SHA-256 is *defined* in rotations and shifts, and
    masking/field-extraction have no arithmetic spelling. All three
    implementations of each agree on the observable exit byte (150 and 120,
    each verified against an independent computation rather than against the
    tuonelang program's own output), and `tuo-cli/tests/lab_command.rs` proves
    they really compile, run, and match their **live** C and Go peers. The
    catalog is now seventeen workloads, none `Unsupported`.
  - *The doc examples are checked by evaluation, not by eye.* The stdlib suite
    proves each public function has an example and that its signature is real,
    but nothing checked an example's stated *value* — a comment reading
    `// 1024` beside a call returning something else ships silently, and is
    exactly the confidently-wrong text the ADR-0018 cheatsheet exists to
    prevent. These two modules are where that does the most damage, since a
    caller who mis-implements a hash gets a plausible digest rather than an
    error. `every_doc_example_in_the_adr_0019_modules_is_accurate` really
    evaluates all thirty-odd of them, and was itself verified by breaking one
    expectation and confirming the test failed with that check's number. (A
    general extractor over all fourteen catalog modules is the right eventual
    shape and is deliberately not attempted here.)
  - *A fourth bug, this one in the tests rather than the compiler, and worth
    recording because of how it presented:* both cross-check tests initially
    shared one scratch directory, which each cleared with `remove_dir_all` on
    entry. Cargo runs a file's tests in parallel threads, so one thread
    deleted the directory while the other was still writing sources into it.
    It passed every time it was run alone and failed inside the full workspace
    suite — an intermittent failure, which is worse than a consistent one
    since a re-run "fixes" it and only CI sees the flake. Each test now takes
    its own subdirectory (the convention `stdlib.rs` already documented), and
    the fix was confirmed over twelve consecutive runs rather than one.
  - *A follow-on module, `std::bignum`, landed on Stage A's operators.* It is
    not part of this ADR's decision — arbitrary-precision arithmetic was never
    in scope here — but it is recorded because Stage A is what made it
    writable and because it is the **prerequisite** for the public-key work
    any future TLS or certificate ADR would need (X25519, RSA verification,
    and DH are all modular arithmetic on numbers `Int` cannot hold). It is
    28-bit limbs, and the size is a *correctness* property rather than a
    tuning knob: schoolbook multiplication accumulates
    `limb + limb*limb + carry`, which is 56 bits at 28-bit limbs and would
    overflow at 32 — and in tuonelang an overflow is a **trap**, not a
    wraparound, so a larger limb would abort rather than compute. It is
    deliberately **not constant time**, states so, and is therefore safe for
    public values and unsafe for secrets; tuonelang cannot express or verify
    constant-time code today, so claiming otherwise would be dishonest.
    Its arithmetic is cross-checked against an independent
    arbitrary-precision implementation over 30-digit operands
    (`bignum_arithmetic_agrees_with_an_independent_implementation`), which
    caught three wrong expectations that had been worked out by hand.
  - *Still outstanding, and named rather than quietly omitted:* `md5` is
    unimplemented, so the legacy `AuthenticationMD5Password` challenge is
    unsupported. That is acceptable only because SCRAM is the default on every
    current server, and it should be revisited if a real connector meets an
    older one.
