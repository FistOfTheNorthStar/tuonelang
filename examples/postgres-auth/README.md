# postgres-auth

The **PostgreSQL v3 wire protocol's authentication handshake**, computed end to
end in tuonelang and checked against RFC 7677's published test vector.

```bash
tuo test --manifest .                        # 84 specs, 0 failed
tuo run src/main.tuo src/std_bits.tuo src/std_ct.tuo src/std_crypto.tuo src/std_str.tuo
echo $?                                      # 48
```

## Why this example exists

A PostgreSQL connector was the first dogfooding target the language could not
express **at all**. The protocol is defined in big-endian length prefixes, and
its authentication is defined in rotations, shifts, and xors — and v0 had no
operator for either. That gap is what opened
[ADR-0019](../../specification/adr/ADR-0019-bitwise-operations-and-crypto.md),
whose Stage A added the bitwise operators and whose Stage B wrote
`std::bits`/`std::crypto` in tuonelang on top of them. This program is the
payoff: the handshake that motivated the ADR, written in the language the ADR
produced.

## What it covers

| Part | Built from |
|------|-----------|
| Message framing — tag, self-counting big-endian length, payload | `std::bits::be32` / `byte_of_be32` (ADR-0019 Stage A) |
| The startup packet — the one untagged frame, null-terminated pairs | the same framing primitives |
| The SASL challenge — comma-separated `k=v` attributes | `std::str::split` |
| SCRAM-SHA-256 client proof | `std::crypto::scram_client_proof` (PBKDF2 → HMAC → SHA-256) |
| Verifying the server's signature | `std::crypto::verify` → `std::ct::bytes_eq` (ADR-0020) |
| The legacy MD5 challenge | `std::crypto::md5_password` (RFC 1321, ADR-0019) |

The server's signature is checked in **constant time**, and that is not
decoration. A client that skips the check authenticates happily against an
impostor; a client that checks it with an early-returning comparison leaks how
many leading bytes of a forged signature were correct, one byte at a time.
`std::crypto::verify` is the only comparison the module offers for this, so the
safe spelling is the convenient one.

## The exit byte is the test

`main` returns **48**: four SCRAM handshake steps that each compare against RFC
7677's published values, 40 for a framing round-trip, and 4 for the legacy MD5
challenge checked against its own pinned vector. Every step is reachable only
if the one before it was right, so a wrong proof, a misparsed attribute, or an
off-by-one in the self-counting length field all lower it.
[`dogfood_examples.rs`](../../crates/tuo-cli/tests/dogfood_examples.rs) asserts
that byte on every `cargo test`.

This caught a real bug while the example was being written. The client-first
message's `n=` username field was hardcoded empty — invisible to every
structural check, since the message still parses and the proof is still 32
bytes, but the auth message both sides sign then differs and the server rejects
the proof with no useful diagnostic. The published vector caught it
immediately; a self-consistent spec never would have.

## What it deliberately does not do

**There is no socket traffic here.** Everything is pure and spec-checked, so
the program is hermetic. A real connector would put these frames on a socket
with `std::net` — `http-service/` already serves a live request that way, so
that part is not blocked on the language.

What is missing for a *usable* driver is not I/O but the rest of the protocol:
the extended query flow, the type mapping from PostgreSQL OIDs onto tuonelang
types, and **TLS** — which ADR-0019 explicitly leaves out, since it needs X.509,
a certificate store, and AEAD ciphers, none of which SHA-256 alone provides.

**Legacy `md5` authentication is supported** via `md5_response`, for servers
too old for SCRAM. It is not a fallback to prefer: `password_encryption` has
defaulted to `scram-sha-256` since PostgreSQL 14 and MD5 auth is disabled
outright on several managed providers, and MD5 itself is cryptographically
broken — `std::crypto` says so at length. A client should attempt SCRAM first
and reach the MD5 path only when the server's `AuthenticationRequest` leaves no
choice.

## The vendored standard library

`src/std_bits.tuo`, `src/std_ct.tuo`, `src/std_crypto.tuo`, and
`src/std_str.tuo` are **verbatim copies** of the catalog modules in
`crates/tuo-stdlib/src/std/`, vendored because v0 has no registry and the
example must build from its own directory. They are byte-identical to the
catalog; edit the catalog, not these.
