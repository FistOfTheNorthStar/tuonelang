# ADR-0021: Secret taint tracking — marking data, not just functions

- **Status:** proposed (2026-09-05)
- **Date:** 2026-09-05
- **Context:** ADR-0020 shipped `#[constant_time]`, which the compiler verifies
  by refusing every data-dependent construct in a marked function's body:
  branches, bounds-checked indexing, trapping arithmetic, and calls to unmarked
  functions (`T0017`–`T0021`). The primitives it guards are pinned twice over —
  by that checker, and by disassembling the emitted code on both backends and,
  since the cross-target work, on x86-64 as well as ARM64.

  That is a real guarantee and it has a specific hole, recorded in ADR-0020's
  own outstanding list: **the attribute marks functions, not data.** The
  compiler verifies *how* a marked function computes; it says nothing about
  *what flows into it*. So this compiles clean today:

  ```tuo
  let key = load_secret();
  if key_len(key) > 32 { … }        // an ordinary branch on a secret
  let tag = compute_tag(key);        // an unmarked function, unchecked
  if tag == expected { … }           // == on a MAC tag: the exact leak
  ```

  Nothing here is marked, so nothing is checked. `std::crypto::verify` exists
  precisely so the last line has a safe spelling, but the language cannot
  *require* its use, because it does not know `tag` is a secret. The library
  can offer the safe primitive; only the type system can insist on it.

  This is the gap between "a constant-time subset exists" and "secrets are
  handled in constant time", and it is the difference between a library a
  careful author can use correctly and a language that catches the careless
  case. The audience this project is built for — coding agents and newcomers —
  is exactly the audience that will not know which comparison is which.

- **The question this ADR must answer:** what is the smallest addition that
  makes the compiler refuse the code above, without making ordinary programs
  harder to write?

- **Decision:** *Not yet taken.* This ADR is opened to record the design space
  and the constraints, per the project's rule that a gap discovered by
  dogfooding becomes an ADR rather than an ad-hoc feature. It should not be
  implemented until the questions in "What must be settled first" are answered.

## The design space

Four shapes, in increasing order of cost and of what they buy.

### 1. A `Secret[T]` wrapper type

A library-level newtype whose contents can only be reached through
constant-time operations. `Secret[Int]` has no `==`, no `<`, and cannot be
branched on; `std::ct` gains `Secret`-taking variants.

*Buys:* the common mistakes, with no new language machinery — this is
expressible in v0 today if generics over a nominal type land.
*Costs:* every unwrap is a hole, and there must be unwraps (a MAC tag
eventually gets written to a socket). Discipline lives in whoever writes the
unwrap, which is where it already lives. It documents intent more than it
enforces it.

### 2. Taint inference with no annotations

The checker infers taint from a small set of sources (`std::crypto::random_byte`,
anything derived from a function parameter marked secret) and propagates it
through assignment, arithmetic, and calls, refusing a data-dependent construct
on a tainted value.

*Buys:* catches the careless case with no burden on ordinary code.
*Costs:* whole-program analysis with no annotation boundary. Every call has to
be analysed in context or conservatively assumed tainted, which either loses
precision at module boundaries or requires an interprocedural summary the
compiler does not have today. Error messages get worse the further the source
is from the violation.

### 3. Annotated taint with inferred propagation

Sources are marked (`#[secret] fn load_key() -> Str`), propagation is inferred
within a function, and function *signatures* carry the taint of their
parameters and result — so a summary is written down rather than inferred
across the whole program.

*Buys:* most of what (2) buys, with a checkable boundary at each function and
error messages that can name the source.
*Costs:* a real addition to the type system — taint becomes part of a
signature, so it is part of the module interface, which touches resolution,
typing, the incremental query graph's `module interface` seam, and the agent
protocol's rendered signatures. Not a checker pass.

### 4. Full information-flow types

Taint as a lattice with declassification, the literature's answer (Jif, FlowCaml).

*Buys:* soundness, including implicit flows.
*Costs:* out of proportion to v0. It would be the largest single feature in the
language, and the fraction of tuonelang programs handling secrets does not
justify it.

**Provisional lean: (3), with (1) as a stepping stone that can ship first and
is useful regardless.** But this should not be decided from the armchair — see
below.

## What must be settled first

1. **Is there a real program that gets this wrong?** ADR-0020's own adoption
   note records that `std::crypto` shipped with *no* comparison at all, so
   callers wrote their own — the exact failure this would catch. That is
   evidence, but it is one instance and it has since been fixed by adding
   `verify`. Before adding a type-system feature, the dogfooding corpus should
   be checked for whether the mistake recurs when the safe primitive exists.
   *Answered* in "Findings: question 1 (2026-11-28)" below: it does not recur.

2. **Does the attribute already cover the cases that matter?** Every function
   in `std::crypto` that touches a secret could carry `#[constant_time]`
   instead. That fails today — the checker refuses loops and indexing, which
   HMAC and PBKDF2 need — so answering this means knowing whether the checker
   can be extended to accept *public-length* loops without accepting
   data-dependent ones. That is a smaller and more concrete question than taint,
   and it may dissolve most of the need.

3. **What is the interaction with `#[constant_time]`?** Two marking systems
   that do not compose would be worse than either alone. A tainted value passed
   to a marked function should presumably be fine — that is what marked
   functions are *for* — which suggests the two are complementary rather than
   overlapping, but it needs stating precisely.

4. **What does it cost an ordinary program?** The rule from ADR-0020 holds: a
   gate that rejects working code is acceptable only when the code was
   genuinely improvable. A taint system that makes non-cryptographic programs
   harder to write has failed, however sound it is.

## Findings: question 1 (2026-11-28)

Question 1 asks whether the wrong comparison still turns up once
`std::crypto::verify` exists. This section records a survey of every place the
repository's tuonelang code compares secret-derived bytes, run against commit
`a4a5128`.

### Method

Six areas were searched, `.tuo` files only (the C and Go benchmark peers are
not tuonelang and were not counted):

- `examples` (24 `.tuo` files, eleven programs),
- `corpus` (9 files),
- `training/seeds` (207 files: 139 reference solutions plus 68 mutants),
- `benchmarks` (191 files),
- the callers inside `crates/tuo-stdlib/src/std` (24 files),
- `training/live`, the model-written attempts: `training/live/gemma4-latest-primed`
  (108 files), `training/live/qwen3-coder-30b` (134) and
  `training/live/qwen3-coder-30b-primed` (82).

Five searches were run, then every hit was read by hand:

```bash
A="examples corpus training/seeds training/live benchmarks crates/tuo-stdlib/src/std"
# 1. Calls to the safe primitives.
grep -rnE 'crypto::verify|bytes_eq|\bverify\(' --include=*.tuo $A
# 2. == / != with a tag-, mac-, digest-, proof-, signature- or key-like name.
grep -rniE '(tag|mac|digest|proof|signature|sig|key|hash|verify_data|secret|expected|finished)[a-z_]*\s*(==|!=)|(==|!=)\s*[a-z_:]*(tag|mac|digest|proof|signature|key|hash|verify_data|secret|expected)' --include=*.tuo $A
grep -rniE '(password|token|nonce|auth|credential|hmac|sha256|signature|proof|secret|digest|fingerprint).*(==|!=)|(==|!=).*(password|token|nonce|auth|credential|hmac|sha256|signature|proof|secret|digest|fingerprint)' --include=*.tuo $A
# 3. Index loops comparing two arrays element by element.
grep -rnE '\[[a-z_]+\]\s*(==|!=)\s*[a-z_]+\[[a-z_]+\]|get\([a-z_]+, *[a-z_]+\)\s*(!=|==)\s*(std::array::)?get\(' --include=*.tuo $A
grep -rnE '\|\s*\(std::array::get\([a-z_]+, *[a-z_]+\)\s*\^\s*std::array::get' --include=*.tuo $A
# 4. Any function taking two byte arrays and answering Bool or Int.
grep -rnE 'fn [a-z_0-9]+\(\s*(in |take )?[a-z_]+: Array\[Int\],\s*(in |take )?[a-z_]+: Array\[Int\][^)]*\)\s*->\s*(Bool|Int)' --include=*.tuo $A
# 5. Made-up constant-time helpers a model might reach for.
grep -rnE 'constant_time_compare|ct_eq|secure_compare|timing_safe|constant_time_eq' --include=*.tuo $A
```

Counting rules:

- A row is a comparison written in a function body, including helper
  functions that only specs call. `then` assertions inside `spec` blocks are
  not rows: they compare a function's output with a published test vector in
  the spec sandbox, where nothing is timed by an attacker. Doc comments
  (`///`) are not rows either.
- The definitions of the primitives themselves (`std::crypto::verify`,
  `std::ct::bytes_eq`) are not rows. `examples/postgres-auth/src` and
  `examples/postgres-client/src` vendor byte-identical copies of them
  (`std_crypto.tuo`, `std_ct.tuo`); those are not rows for the same reason.
- `std::ed25519::verify` is an algorithm, not a byte comparison. The
  comparison inside it is a row.
- Only `training/live` is model-written. The repository records no other
  provenance, so everything else is counted as human-written.
- A hit was dropped when neither side is secret-derived: DER tag checks in
  `crates/tuo-stdlib/src/std/der.tuo`, hash-map and settings keys, HTTP
  method tokens, the file contents compared by `same_bytes` in
  `examples/file-report/src/main.tuo`, and two model attempts comparing echoed
  network payloads (`training/live/qwen3-coder-30b/udp/echo_reply.tuo:21`, an
  early-returning loop, and `training/live/gemma4-latest-primed/ipv6/echo6.tuo:119`,
  a call to a `std::array::bytes_eq` that does not exist).
- An instance is *unsafe* when the comparison's time depends on the contents
  and one operand can be supplied by an attacker while the other is
  secret-derived. A length check that returns early is not unsafe: lengths
  are public.

### Inventory

Human-written (14 rows):

| # | Where | What is compared | How |
|---|-------|------------------|-----|
| H1 | `training/seeds/crypto-encoding/is_authentic.tuo:7` | HMAC-SHA-256 tag | `std::crypto::verify` |
| H2 | `examples/postgres-auth/src/main.tuo:347` (`server_signature_ok`) | SCRAM server signature | `std::crypto::verify` |
| H3 | `examples/postgres-client/src/main.tuo:786` | SCRAM server signature | `std::crypto::verify` |
| H4 | `examples/postgres-auth/src/main.tuo:397` (`handshake_steps_passed`) | SCRAM client proof, computed from a hard-coded password, against RFC 7677's published value | `==` on `Str` |
| H5 | `crates/tuo-stdlib/src/std/tls.tuo:1245` (`client_finished_matches`) | client Finished message, `verify_data` included | `std::crypto::verify` |
| H6 | `crates/tuo-stdlib/src/std/tls.tuo:1229` (`shared_secret_is_zero`) | X25519 shared secret against all zeros | `std::ct::bytes_eq` |
| H7 | `crates/tuo-stdlib/src/std/chacha.tuo:1203` (`aead_open`) | Poly1305 tag | `std::crypto::verify` |
| H8 | `crates/tuo-stdlib/src/std/ed25519.tuo:604` (`verify`, through `points_equal` and `std::bignum::compare` at `crates/tuo-stdlib/src/std/bignum.tuo:219`) | the verification equation's two points, `[S]B` and `R + [h]A` | hand-written loop, returns early (`std::bignum::compare`) |
| H9 | `crates/tuo-stdlib/src/std/hkdf.tuo:670` (`differs`, spec helper) | derived traffic key against traffic IV | hand-written loop, does not return early |
| H10 | `crates/tuo-stdlib/src/std/tls.tuo:3152` (`secrets_differ`, spec helper) | client and server handshake secrets | hand-written loop, does not return early |
| H11 | `crates/tuo-stdlib/src/std/tls.tuo:3167` (`application_differs_from_handshake`, spec helper) | application and handshake secrets | hand-written loop, does not return early |
| H12 | `crates/tuo-stdlib/src/std/ed25519.tuo:1945` (`same_bytes`, spec helper) | encoded outputs of the constant-time point and scalar arithmetic, on fixed test inputs | hand-written loop, returns early |
| H13 | `benchmarks/runtime/programs/tuo/constant-time.tuo:86` (`ct_bytes_eq`) | 32-byte tag, the benchmark's safe side | hand-written loop, does not return early |
| H14 | `benchmarks/runtime/programs/tuo/constant-time.tuo:97` (`naive_bytes_eq`) | 32-byte tag, the benchmark's naive side | hand-written loop, returns early |

`corpus` and the other `training/seeds` concepts contain no comparison of
secret-derived bytes. The `crypto-encoding` seeds `basic_auth`, `fingerprint`
and `payload_of` encode, hash or split, and compare only in `spec` blocks.

Model-written (3 rows), one per run, all on the `is_authentic` task:

| # | Where | What is compared | How |
|---|-------|------------------|-----|
| M1 | `training/live/gemma4-latest-primed/crypto-encoding/is_authentic.tuo:30` | HMAC-SHA-256 tag | `std::crypto::verify` (after a length check that returns early) |
| M2 | `training/live/qwen3-coder-30b/crypto-encoding/is_authentic.tuo:17` | HMAC-SHA-256 tag | a made-up `constant_time_compare`, imported as `crypto.constant_time_compare`: an intent to compare safely, in code that does not parse |
| M3 | `training/live/qwen3-coder-30b-primed/crypto-encoding/is_authentic.tuo:16` | HMAC-SHA-256 tag | `std::crypto::verify` |

No model attempt in any run compares secret-derived bytes anywhere else.

### Counts

| Method | Human-written | Model-written |
|--------|---------------|---------------|
| `std::crypto::verify` | 5 (H1, H2, H3, H5, H7) | 2 (M1, M3) |
| `std::ct::bytes_eq` | 1 (H6) | 0 |
| `==` / `!=` | 1 (H4) | 0 |
| hand-written loop, does not return early | 4 (H9, H10, H11, H13) | 0 |
| hand-written loop, returns early | 3 (H8, H12, H14) | 0 |
| made-up constant-time helper | 0 | 1 (M2) |
| **Total** | **14** | **3** |

### Unsafe instances

**None.** No row has an attacker-supplied operand compared in time that
depends on a secret.

Intentional, not mistakes:

- H14, `naive_bytes_eq` in `benchmarks/runtime/programs/tuo/constant-time.tuo`:
  the early-returning form, kept on purpose as the benchmark's baseline.
- `std::ct` describes the early-returning form as the vulnerability in prose
  only (`crates/tuo-stdlib/src/std/ct.tuo:11` and `:346`). There is no code
  counter-example in it, so it has no row.

Content-dependent comparisons that are not unsafe, and why:

- H8, `std::ed25519::verify`: every input (public key, message, signature) is
  public, which is why `crates/tuo-stdlib/src/std/tls.tuo` records Ed25519
  verification as sound on `std::bignum`.
- H12: a spec helper, run only on fixed test inputs in the spec sandbox.
- H4: both operands are fixed (a proof from the hard-coded password `pencil`
  and RFC 7677's published value), so nothing an attacker sends is compared.

Follow-ups: none is required. H4 is the one `==` on secret-derived bytes in a
shipped example. It is safe where it stands, but it is the spelling a reader
might copy, so changing it to `std::crypto::verify` in a separate change is
optional.

### Limits

- **A text search cannot prove a value is a secret.** Rows were chosen by
  reading the code around each hit, and the line between "secret-derived" and
  "public" is a judgement (H8 and H12 are on that line and are listed rather
  than dropped).
- **Indirect spellings can be missed.** A comparison through a helper with an
  unrelated name, `==` on a struct or enum that holds bytes, or a loop written
  in a shape none of the patterns match would not show up. Search 4 looks for
  any two-array helper to narrow this, but does not close it.
- **The model attempts are biased.** `training/live` keeps only the attempts
  the compiler objected to: each failed to check or failed its own specs, and
  M2 does not even parse. For `is_authentic` that covers every run's single
  attempt (each run asked each of the 139 seed tasks once, and all three kept
  this one), so nothing on that task is hidden. Across the whole set, though,
  broken code is over-represented, and passing attempts on other tasks were
  not available to search.
- **The prompt names the answer.** The `is_authentic` task says "in constant
  time", and its notes name `std::crypto::verify`; the two primed runs also
  had the language brief, which lists `verify`. So M1 to M3 show that models
  follow an explicit instruction, not what they write unprompted.
- **The samples are small.** Three model attempts, from two models, on one
  task. Fourteen human rows, in a repository whose `CLAUDE.md` describes
  `std::crypto::verify` as the comparison to use. The repository records no
  authorship, so nothing here says who wrote the human rows.

### Conclusion

The mistake does not recur where the safe primitive exists. Every comparison
that faces an attacker (H2, H3, H5, H6, H7) uses `std::crypto::verify` or
`std::ct::bytes_eq`, and so do the reference solution (H1) and both model
attempts that name a real function (M1, M3); the one unprimed attempt (M2)
reached for a constant-time helper rather than `==`, though one kept attempt
from one run is all the evidence for that. The evidence is weak: the one task
that asks for a secret comparison says "in constant time" and names `verify`,
so the survey cannot show what an author does unprompted. It finds no program
that gets this wrong today, which supports this ADR's position that shipping
nothing is currently right.

## Consequences (if implemented)

- **A benchmark plan is required**, per the project rule. Taint is a
  compile-time property with no runtime cost, so the benchmark is a *compiler*
  one: the added checking must not measurably slow `tuo check`, and the
  incremental scenarios must not lose their early cutoff — a taint summary in
  the module interface would invalidate callers on a change that currently
  does not.
- **The corpus needs a category.** ADR-0020's checker got dedicated tests
  (`T0017`–`T0021`); taint would need the same, plus corpus entries in the
  `type-error` category proving each rejection is real.
- **The honesty rule carries over.** Whatever ships must state precisely what
  it does not cover. Implicit flows, timing through the cache, and the hardware
  question ADR-0020 already disclaims all remain out of reach, and a taint
  system is likely to *feel* more complete than it is.

## What this ADR does not propose

Deferring is a decision, and this is one: **shipping nothing is currently the
right answer.** `#[constant_time]` plus `std::crypto::verify` covers the
primitives that exist, both are verified twice over, and question (2) above may
show the remaining gap is better closed by extending the checker than by adding
a second marking system. This ADR exists so that conclusion is reached
deliberately rather than by the gap being forgotten.
