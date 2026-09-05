# postgres-client

A **real PostgreSQL client**: connects over TCP, authenticates with
SCRAM-SHA-256, runs a query, and reads the rows back off the wire.

```bash
tuo test --manifest .                        # 98 specs, 0 failed
tuo run src/main.tuo src/std_bits.tuo src/std_ct.tuo src/std_crypto.tuo src/std_str.tuo
echo $?                                      # 42 (the answer SELECT 42 returned)
```

## What this is, and how it differs from `postgres-auth`

[`postgres-auth/`](../postgres-auth/) computes the authentication
*mathematics* — hermetically, with no socket, checked against RFC 7677's
published vector. This is the other half: the same protocol driven over a live
connection to an actual server. Together they are the connector that
[ADR-0019](../../specification/adr/ADR-0019-bitwise-operations-and-crypto.md)
was opened for, which at the time the language could not express **at all**.

It is a *client*, not a driver. What a production driver adds — the extended
query protocol, the full PostgreSQL type map, connection pooling, TLS — is
breadth, not anything the language is missing. Every piece here is ordinary v0.

## The exchange it performs

1. **Startup packet** — the one untagged frame: self-counting length, protocol
   version 3.0, then null-terminated `user` and `database` parameters.
2. **`AuthenticationSASL`** — the server offers SCRAM-SHA-256.
3. **`SASLInitialResponse`** — the client sends `n,,n=<user>,r=<nonce>`.
4. **`SASLContinue`** — the server returns its nonce, salt, and iteration
   count, which the client parses off the wire (never assumes).
5. **`SASLResponse`** — the client sends `c=biws,r=<nonce>,p=<proof>`, the
   proof derived through PBKDF2 → HMAC → SHA-256.
6. **`SASLFinal`** — the server proves *it* knew the stored credentials, and
   the client **verifies that signature in constant time**.
7. **`Query` / `DataRow` / `ReadyForQuery`** — `SELECT 42`, decoded from the
   frame.

Steps 6 and 7 are the ones worth dwelling on. A client that skips the server
signature authenticates happily against an impostor; a client that checks it
with an early-returning comparison leaks how many leading bytes of a forged
signature were correct. `std::crypto::verify` is the constant-time comparison
(ADR-0020), and it is the only one `std::crypto` offers, so the safe spelling
is the convenient one.

## The two tiers

Kept strictly apart, exactly as the standard library keeps them:

| Tier | What | Specs? |
|------|------|--------|
| **Executable** | building frames, parsing SCRAM attributes, decoding rows | yes — every function |
| **Effect** | `connect` / `write_string` / `read_byte_timeout` / `close` | no — `R0007` keeps the sandbox pure; pinned by the dogfood test |

So **what the client puts on the wire is decided by spec-checked code**, and
only the I/O itself depends on a live server.

Every read uses `read_byte_timeout` (ADR-0017), never the unbounded
`read_byte`. A client that blocks forever on a server that stopped talking is a
hung process.

## Bytes are `String`

v0 has no `[u8]`, and `Str`/`String` are byte containers that hold zero bytes
and high bytes perfectly well — the wire is full of both. So a frame is built
as a `String` and written with `std::rt::write_string`. That is not a
workaround; it is what those types are.

## The exit byte names the failing step

`main` returns **42** — the value `SELECT 42` returned, decoded from a
`DataRow`. Anything else is diagnostic rather than a bare mismatch:

| Exit | Meaning |
|-----:|---------|
| 42 | success: authenticated and the query returned the right answer |
| 3 | could not connect (no server) — the dogfood test treats this as a skip |
| 5 | authenticated, but the query returned the wrong value |
| 20 | the client's proof was rejected — wrong password |
| 21 | the **server's** signature failed verification |

Those last two are verified to be load-bearing: substituting a wrong password
really produces 20, and tampering with the expected server signature really
produces 21.

## Running the live path

The test skips when no server is reachable, so a developer without a local
PostgreSQL still gets a green suite. To run it for real:

```bash
mkdir -p /tmp/tuopg
initdb -D /tmp/tuopg/data -U tuo_admin --auth-host=scram-sha-256 \
  --pwfile=<(echo adminpw)
pg_ctl -D /tmp/tuopg/data -o "-p 55432 -k /tmp/tuopg" -l /tmp/tuopg/log start
PGPASSWORD=adminpw psql -h 127.0.0.1 -p 55432 -U tuo_admin -d postgres \
  -c "CREATE ROLE tuo_test LOGIN PASSWORD 'tuo_secret';" \
  -c "CREATE DATABASE tuo_testdb OWNER tuo_test;"
```

`--auth-host=scram-sha-256` is the part that matters: with the default `trust`
the server never issues a challenge, and the client would "succeed" without
authenticating at all.

## What it does not do

**No TLS.** ADR-0019 leaves it out deliberately: SHA-256 and HMAC are written
in tuonelang and need no dependency, but TLS additionally needs X.509, a
certificate store, and AEAD ciphers. Everything here is readable by anyone on
the wire, which is fine over loopback and not fine over a network.

**No extended query protocol**, so no parameter binding — and therefore no
prepared statements. A real driver needs `Parse`/`Bind`/`Execute` for both
performance and to keep query text and data separate.

**Only the first column of the first row**, as text. The simple query protocol
returns everything in text format; mapping PostgreSQL OIDs onto tuonelang types
is the type-map work a driver adds.

**A fixed client nonce**, so the dogfood test is reproducible. A real client
uses `std::crypto::nonce` (the platform CSPRNG). The server's nonce is random
regardless, so each exchange is still unique.

## The vendored standard library

`src/std_bits.tuo`, `src/std_ct.tuo`, `src/std_crypto.tuo`, and
`src/std_str.tuo` are **verbatim copies** of the catalog modules in
`crates/tuo-stdlib/src/std/`, vendored because v0 has no registry. They are
byte-identical to the catalog and pinned by the dogfood test; edit the catalog,
not these.
