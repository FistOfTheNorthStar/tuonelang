# ADR-0036: TLS in tuonelang — admitting ciphers, key exchange, and certificates into the standard library

- **Status:** accepted (retroactive — records the decision taken in the
  2026-09-15 TLS round, commit `3a06c2b`, PR #61)
- **Date:** 2026-10-14

## Context

Four ADRs left TLS out. ADR-0014 called it additive "when its need is
demonstrated by dogfooding". ADR-0017 kept it out because "TLS implies
either a cryptographic implementation or an external dependency", in a
workspace whose SHA-256 is hand-rolled to avoid taking one. ADR-0019 and
ADR-0020 narrowed the reason once `std::crypto` landed: hashing no longer
needed a dependency, but "TLS additionally needs X.509, a certificate store,
and AEAD ciphers".

`std::crypto`'s header named the next step: no symmetric cipher, no
public-key operation, no certificate handling, "and adding them is a
decision for a future ADR rather than an incremental commit".
`examples/router` said the same of an HTTPS server.

On 2026-09-15 all of it landed anyway, in one commit, and no ADR was written.
`std::tls` is a TLS 1.3 server handshake and record layer (RFC 8446) built on
six new catalog modules. This ADR records that decision after the fact, as
the process required beforehand. The change touched no grammar, no ABI, and
no runtime shim, so it was not a *language* change in the dogfooding rule's
sense. It still reversed scope statements in four ADRs and a module header
that asked for an ADR by name, which is the case ADRs exist for. Writing it
late does not change what was decided. It does mean the review that an ADR
invites came after the code, and drafting this one turned up two conformance
gaps that review should have found (see *Limits*; both since fixed).

## Decision

**Cryptography beyond hashing is admitted into the standard library: an
AEAD cipher, key exchange, signatures, DER/X.509 decoding, and the TLS 1.3
protocol. It is written in tuonelang with no external dependency.** Nothing
is linked and no `std::rt` primitive is added. `std::tls`'s five `EFFECT:`
functions compose the existing `read_byte_timeout` and `write` from
ADR-0006/ADR-0017. OpenSSL appears only as an independent peer in tests.

| Module | Provides | Pinned against |
|---|---|---|
| `std::sha512` | SHA-512 | FIPS 180-4 published digests (specs) |
| `std::hkdf` | HKDF, TLS 1.3 `HKDF-Expand-Label`/`Derive-Secret` | RFC 5869 A.1 (specs) |
| `std::chacha` | ChaCha20-Poly1305 AEAD | RFC 8439 §2.1.1/2.3.2/2.5.2/2.8.2 |
| `std::x25519` | X25519 key exchange | RFC 7748 §5.2 |
| `std::ed25519` | Ed25519 verify, and a variable-time `sign` | RFC 8032 §7.1 (verify) |
| `std::der` | X.690 DER decoding, X.509 field extraction | a real OpenSSL-emitted certificate |
| `std::tls` | server handshake, record layer, connection | RFC 8448 key schedule; live OpenSSL |

The sub-decisions that carried weight:

1. **One suite per role, chosen for what the language can express safely.**
   It ships TLS_CHACHA20_POLY1305_SHA256, X25519, and Ed25519, and nothing
   else. ChaCha20 is defined over exactly ADR-0019's operations (32-bit add,
   xor, rotate) with no data-dependent memory access. AES in software is
   table-driven, and that is the textbook cache-timing vulnerability. X25519's
   Montgomery ladder is uniform and accepts any 32 bytes as a public key,
   while P-256 needs point decompression, a modular square root, and full
   point validation.
2. **Server side only.** No client handshake exists. `std::der` and
   `ed25519::verify` are the pieces a client would need, and they are pinned
   on their own, but no protocol code uses them.
3. **Signing stays outside `std::tls`.** `server_handshake` takes the
   CertificateVerify signature as an argument, and `signing_content` exposes
   the bytes to be signed. The caller supplies the signer.
4. **Every reader of network input is total.** A ClientHello and a
   certificate arrive before any authentication. tuonelang traps on overflow
   and on out-of-bounds indexing, and a trap aborts the whole process. So
   every parser returns a failure value instead of trapping. The same
   commit fixed a hostile `Content-Length` in `examples/router` that was a
   remote pre-authentication process kill, which shows the threat is real.

## Why the exclusion's reasoning no longer held

- *"A cryptographic implementation or an external dependency"* (ADR-0017)
  stopped being a dilemma at ADR-0019 Stage A. With the operators in place,
  the implementation can be written in the language. Stage B showed this by
  making a native tuonelang SHA-256 agree byte-for-byte with `tuo-package`'s
  Rust one.
- *"X.509, a certificate store, and AEAD ciphers"* (ADR-0019/0020) was an
  accurate list of what **deployable, full** TLS needs. It mixed together
  "expressible" and "deployable". An **AEAD** is expressible:
  ChaCha20-Poly1305 needs only ADR-0019's operators, plus `std::bignum` for
  Poly1305. **X.509**, as far as a server needs it (sending its certificate)
  and a client needs it (extracting a key), is a total DER decoder, not a
  dependency. A **certificate store** is needed only to validate a chain,
  which is a non-goal here, so that item is scoped out rather than satisfied.
- **What did hold** was the deployability concern. ADR-0022 now carries it
  (see Consequences).

## Limits and non-goals

**Not constant time, so not deployable.** This is the limitation that
matters most, and `std::tls`'s header puts it first.

`x25519::scalar_mult` on the server's ephemeral key and `ed25519::sign` on
its long-term key both run on `std::bignum`, which is variable-time by
design. Poly1305 also uses `std::bignum`, and HKDF inherits `std::crypto`'s
unmarked HMAC. No function in the seven modules carries `#[constant_time]`.
The shape is right in two places only: the ladder's conditional swap is
branchless through `std::ct::select_bytes` (commit `d786698`), and record
tags are compared through `std::crypto::verify`, which is `std::ct::bytes_eq`.
Shape is not the guarantee, because the arithmetic underneath still leaks.
Ed25519 verification is sound in this respect, since all of its inputs are
public.

**Absent rather than stubbed:** session resumption and PSK, 0-RTT, client
certificates, HelloRetryRequest, key update, RSA, P-256, AES-GCM, any
client-side handshake, a certificate store, and chain validation
(`std::der` decodes; it checks no signatures, dates, names, or extensions).

**Conformance gaps found while drafting this ADR — both since fixed.** As
first recorded:

- **The client's Finished was not verified.** `std::tls::fill` treated the
  first handshake-type record in the client's handshake epoch as its
  Finished and switched keys. It never compared the record's `verify_data`
  with `finished_data` over the transcript, which RFC 8446 §4.4.4 requires.
  The record is AEAD-authenticated under the client handshake key. That
  limits who can produce it, but it is not the transcript check the RFC
  specifies. Worse than recorded: `fill` also returned an application-data
  record from the handshake epoch as data, so a client could skip the
  Finished altogether.
- **There was no all-zero shared-secret check.** `server_handshake` used the
  X25519 output without checking for the all-zero value, which RFC 8446
  §7.4.2 says implementations MUST reject.

*Fixed.* `server_handshake` now returns the expected client `verify_data`
(HMAC under the client handshake secret's `finished` key over the
transcript through the server's Finished) in the `Handshake`, and `fill`
compares the client's Finished message against it through
`std::crypto::verify` (so `std::ct::bytes_eq`, constant time) before
switching epochs. A mismatch ends the connection with a fatal
`decrypt_error` alert. Any record other than a Finished or an alert in the
handshake epoch ends it with `unexpected_message`. Either refusal also
drops the refused record from the read buffer, which `read_byte` would
otherwise have handed out as data on the next call (the same applied to a
received alert's two bytes). `server_handshake`
refuses an all-zero X25519 output, tested through `std::ct::bytes_eq`, by
returning `failed_handshake()`. No new dependency edge was needed:
`std::tls` already declared `ct` and `crypto`. Each fix is pinned by a
native test that sends the forbidden input and by a spec on its predicate
(see the table below), and each test was confirmed to fail with its check
disabled.

No attack was analysed for either gap while it was open.

**A documentation contradiction.** The `std::ed25519` module header, its
`tuo-stdlib` `lib.rs` entry, and `server_handshake`'s doc comment all state
that `std::ed25519` "deliberately ships no `sign`". But `sign` exists (RFC 8032 §5.1.6, documented "Not constant time"), and
both OpenSSL tests use it to produce the CertificateVerify. Sub-decision 3
still stands: the caller supplies the signer. Its stated premise is false
and should be corrected.

**Totality is example-tested, not fuzzed.** The readers' specs feed chosen
hostile inputs: empty input, truncations, lengths past the end, and negative
or absurd values. No `tuo-fuzz` target drives `std::tls` or `std::der`.

## What pins each claim

| Claim | Pin |
|---|---|
| SHA-512, HKDF, and DER readers are correct and total | their module specs: 196 `spec` blocks across the seven modules at this revision (194 when this ADR was drafted; the two added since pin the Finished comparison and the zero-secret predicate) |
| ChaCha20-Poly1305 matches RFC 8439 and rejects forgery, on both backends | `chacha20_poly1305_matches_rfc_8439_natively` |
| X25519 matches RFC 7748, on both backends | `x25519_matches_rfc_7748` (it caught a `BB`-for-`AA` ladder bug that no self-consistent test could) |
| Ed25519 verifies RFC 8032 vectors and refuses forgeries | `ed25519_matches_rfc_8032` |
| DER and Ed25519 compose on OpenSSL artifacts | `certificate_chain_verifies_a_real_signature` (one certificate; despite the name, no chain) |
| The key schedule matches RFC 8448 | `tls_key_schedule_matches_rfc_8448` |
| ECDHE, key schedule, and AEAD compose across independent sides | `tls13_crypto_core_composes_end_to_end` |
| A real client completes the handshake, and a wrong signing key is rejected | `tls_server_completes_handshake_with_openssl` |
| The client's Finished is verified: an honest one is accepted, one flipped `verify_data` bit is refused with `decrypt_error`, and application data in its place with `unexpected_message` | `tls_server_verifies_the_client_finished` (both backends), and `spec client_finished_matches` for the comparison itself |
| A small-order X25519 key share (all eight blocklisted encodings) is refused, and an honest one still completes | `tls_rejects_small_order_key_shares` (both backends), and `spec shared_secret_is_zero` for the predicate |
| The stream layer carries bytes both ways, including the epoch switch, and a real client's Finished passes verification | `tls_serves_a_real_https_request` (three routes, one connection each) |
| The ladder swap does not branch on the scalar bit | `the_ladders_conditional_swap_does_not_branch_on_the_scalar_bit` (`tuo-cli/tests/constant_time.rs`; a behavioural test cannot tell the two forms apart) |
| The declared module graph is the real one and is acyclic | `the_dependency_graph_is_declared_and_acyclic` |

All of these tests are in `crates/tuo-cli/tests/stdlib.rs` unless the table
says otherwise. Two caveats apply:

- **`ed25519::sign` is not pinned to RFC 8032's published signatures.** It
  is pinned only indirectly, by OpenSSL accepting the CertificateVerify it
  produced.
- **Both OpenSSL tests return early, and pass, when no `openssl` binary is
  present.** A green run does not show that they ran.

The round also fixed a compiler bug it surfaced: a heap temporary in a
short-circuit operand was dropped on only one path, so `check` passed and
`build` failed. It is pinned by
`heap_temporaries_in_short_circuit_operands_lower_correctly`
(`tuo-cli/tests/heap_native.rs`) and
`crates/tuo-fuzz/regressions/mir/short_circuit_heap_temporary.tuo`.

## Catalog dependency edges added

These are declared in `DECLARED_DEPENDENCIES` (`tuo-cli/tests/stdlib.rs`):

| Module | Declared dependencies |
|---|---|
| `std::chacha` | `bits`, `ct`, `bignum`, `crypto` |
| `std::x25519` | `bignum`, `ct` |
| `std::hkdf` | `bits`, `ct`, `crypto` |
| `std::ed25519` | `bits`, `ct`, `bignum`, `x25519`, `sha512` |
| `std::tls` | `bits`, `ct`, `bignum`, `crypto`, `chacha`, `hkdf`, `x25519` |

`std::sha512` and `std::der` declare no dependencies.

`std::tls` does **not** depend on `ed25519`, `sha512`, or `der`. The
signature arrives as bytes (sub-decision 3), and the certificate is passed
through as an opaque DER blob.

The `bignum` edges into `x25519`, `ed25519`, and `tls` are the ones
ADR-0022's 2026-09-28 amendment names as the constant-time gap.

## Performance

No lab workload exists for any of these modules. The only measurements are
in commit messages, and no committed harness can reproduce them:

`3a06c2b` records X25519 at 1.63 s → 0.02 s (about 80x, from curve-specific
reduction and not rebuilding the prime on every reduction) and Ed25519 at
14 s → 0.25 s (about 56x, from extended coordinates). `d786698` records a
full `scalar_mult` at about 13.3 ms → 14.3 ms (20-round average), the price
of the branchless swap.

`std::x25519`'s header still says "roughly a second natively", which
predates the 80x and is stale. No benchmark plan was written, which departs
from the project rule.

## Consequences

- **ADR-0022 is now the path to deployability.**
  - Its 2026-09-28 amendment already records that the programs which need a
    constant-time bignum exist. The concrete work is fixed-width field
    arithmetic for p = 2²⁵⁵ − 19 that bypasses `std::bignum`, plus that
    ADR's requirement (5): limb arithmetic the checker can see cannot trap.
  - Until that work lands, the stack demonstrates that TLS 1.3 is
    expressible in tuonelang. It is not transport security, and nothing in
    the repository should present it as such.
- **ADR-0031 (wrapping arithmetic, drafted alongside this one) bears on
  both the speed and the soundness of these modules.**
  - `std::sha512::add64` adds through 32-bit halves, and `std::chacha` uses
    `std::bits::add32`'s masking. Each is several operations standing in for
    one machine add, and each relies on a range argument written in a
    comment.
  - Whatever ADR-0031 decides changes the emitted code of both modules and
    may discharge part of ADR-0022's requirement (5). It should measure
    ChaCha20 and SHA-512 before and after.
- **This ADR supersedes earlier statements without rewriting them.** The
  TLS exclusions in ADR-0014/0017/0019/0020 stay as the historical record.
  `std::crypto`'s "a decision for a future ADR" and `examples/router`'s "an
  HTTPS server is a future ADR" should point here.
- **Follow-up work this ADR names:** verify the client's Finished and reject
  an all-zero X25519 output (both done; see *Limits*); correct the "no
  `sign`" text in three places
  and the stale X25519 performance note; pin `ed25519::sign` against RFC
  8032's published signatures; add fuzz coverage for the hostile-input
  readers; and write a benchmark plan (for example a handshake-latency
  workload with an OpenSSL or Go `crypto/tls` peer) before any performance
  claim is made.
- **Each absent feature is additive and needs its own ADR.** Chain
  validation in particular also needs a trust-store decision, and that
  touches the OS effect boundary (ADR-0013).
- **Maintenance cost.** The seven modules are about 9,700 lines of
  tuonelang, and their public functions are now part of the generated
  cheat sheet (ADR-0018) that every primed model reads.
