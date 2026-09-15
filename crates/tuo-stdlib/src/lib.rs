//! The tuonelang standard library — its source, as a catalog.
//!
//! The standard library is *written in tuonelang*: each module is a `.tuo`
//! source file under `src/std/`, embedded here at compile time. This crate is a
//! pure catalog — it holds no compiler machinery and depends on no stage crate
//! (it sits at the stdlib layer, below the compiler facade). A host that wants
//! to compile, check, or query the standard library loads these sources into
//! its own [`tuo_source::SourceMap`] and runs its own pipeline over them; the
//! catalog only says *what the modules are and where their text lives*.
//!
//! # What the library promises
//!
//! Every public function in the library carries an exact type signature, a doc
//! comment, at least one worked example, and — where it is executable in v0 — an
//! executable `spec`. There is deliberately **one** obvious API for each
//! fundamental task; the library never ships two competing spellings of the same
//! operation. These promises are enforced by the integration tests in
//! `tuo-cli` (which really compile every module and run every spec) rather than
//! asserted here.
//!
//! # Three honest tiers
//!
//! ADR-0006 landed a narrow effect boundary — the `std::rt` builtins
//! `write`/`read_byte`/`exit`, over already-open descriptors — and the spec
//! sandbox stays pure: a spec whose executed closure reaches an effect is a
//! front-end error (`R0007`). So the library is split, per function, into:
//!
//! * an **executable tier** — pure computation (ordering, `Option`/`Result`
//!   combinators, `Duration` arithmetic, error classification, the pure state
//!   models of a lock or latch), which runs and whose specs run;
//! * an **effect tier** — real tuonelang implementations over the `std::rt`
//!   effect primitives (`std::io::print`/`println`/`read_line`,
//!   `std::process::exit`/`arg_count`/`arg`, `std::time::now`, the whole
//!   `std::fs` disk tier `read`/`write`/`exists`/`remove` — real since
//!   ADR-0013 landed the clock, argv, and file open/close/remove primitives
//!   — the whole `std::net` socket tier
//!   `listen`/`bound_port`/`accept`/`connect`/`close`, real since ADR-0014
//!   landed the socket primitives, and the whole `std::sync` concurrency
//!   tier: `par_map` (ADR-0007), the channels
//!   `channel`/`send`/`recv`/`close`, and the mutexes
//!   `mutex`/`lock`/`unlock`, the last two groups real since ADR-0015).
//!   These compile and run natively, but `R0007` makes an effectful spec
//!   impossible, so each is marked `EFFECT:` in its doc and pinned by a named
//!   native CLI test (in `tuo-cli`'s `stdlib.rs`) instead of a spec
//!   (`read_line` builds an owned `String` from the bytes
//!   `std::rt::read_byte` yields — real since ADR-0009 landed the allocator);
//!   and
//! * a **contract tier** — the effectful entry points whose primitive does
//!   not exist yet, given as exact signatures with documented contracts and
//!   **no** executable spec, each marked `CONTRACT:` in its doc so no reader
//!   — human or machine — is misled into thinking it runs today. **Since
//!   ADR-0015 discharged `std::sync::lock`/`unlock`, this tier is empty** —
//!   the mechanism stays for any future entry, and a CLI test pins its
//!   emptiness so nothing re-enters silently.
//!
//! This mirrors the toolchain's standing rule that the compiler never advertises
//! behavior it cannot perform.

/// One standard-library module: its dotted path and its tuonelang source.
#[derive(Debug, Clone, Copy)]
pub struct Module {
    /// The module's canonical dotted path, e.g. `"std::core"`.
    pub path: &'static str,
    /// A stable, file-like name for the module, e.g. `"std/core.tuo"`. Hosts use
    /// this when interning the source into a [`tuo_source::SourceMap`] so
    /// diagnostics carry a meaningful location.
    pub name: &'static str,
    /// The module's full tuonelang source text.
    pub source: &'static str,
}

/// `std::core` — ordering, arithmetic helpers, and `Option`/`Result`
/// combinators. Entirely executable.
pub const CORE: Module = Module {
    path: "std::core",
    name: "std/core.tuo",
    source: include_str!("std/core.tuo"),
};

/// `std::collections` — `Pair`, counted ranges, pure `Array[Int]` algorithms
/// (`sum`/`min_of`/`max_of`/`contains`/`index_of`/`reversed`/`concat`), the
/// sorting and ordered-array queries (`sorted_ascending`/`sorted_descending`/
/// `insert_ascending`/`is_sorted`/`binary_search`/`deduped`) over the ADR-0009
/// allocator core, and the generic higher-order combinators
/// (`fold`/`map_into`/`filter_into`/`any`/`all`) over a first-class function
/// value (ADR-0008 Tier 1). Entirely executable.
pub const COLLECTIONS: Module = Module {
    path: "std::collections",
    name: "std/collections.tuo",
    source: include_str!("std/collections.tuo"),
};

/// `std::math` — integer arithmetic (`pow`/`gcd`/`lcm`/`modulo`/`is_even`/
/// `is_odd`) and IEEE-754 `Float` arithmetic (`fabs`/`fmin`/`fmax`/`floor`/
/// `ceil`/`round`/`sqrt`) over the scalar core. Entirely executable.
pub const MATH: Module = Module {
    path: "std::math",
    name: "std/math.tuo",
    source: include_str!("std/math.tuo"),
};

/// `std::bits` — fixed-width bit and byte-order operations over `Int`
/// (ADR-0019 Stage B): width masking (`low32`/`low8`), the **modular**
/// arithmetic hashes are defined over (`add32`/`mul32` — plain `+` traps on
/// overflow, which is wrong for a checksum), rotation and logical shifts
/// (`rotr32`/`rotl32`/`shr32`/`shl32`), big-endian assembly and splitting
/// (`be32`/`be16`/`byte_of_be32`/`byte_of_be16` — the wire-protocol framing
/// ADR-0019 was opened for), and inspection (`test_bit`/`count_ones32`).
/// Entirely executable.
pub const BITS: Module = Module {
    path: "std::bits",
    name: "std/bits.tuo",
    source: include_str!("std/bits.tuo"),
};

/// `std::sha512` — SHA-512 (FIPS 180-4), the hash Ed25519 is defined over:
/// the round constants and initial values, the sigma/Ch/Maj functions,
/// 128-byte padding with its **128-bit** length field, and `hash`. Entirely
/// executable, pinned against FIPS 180-4's **published** digests. Separate
/// from `std::crypto`'s SHA-256 because they are different algorithms, not
/// one parameterized by width. Its `add64`/`shr64` exist because tuonelang's
/// `Int` is a *signed* i64 whose `+` **traps** on overflow and whose `>>`
/// sign-extends, while SHA-512 is defined over wrapping 64-bit arithmetic
/// and logical shifts.
pub const SHA512: Module = Module {
    path: "std::sha512",
    name: "std/sha512.tuo",
    source: include_str!("std/sha512.tuo"),
};

/// `std::ed25519` — Ed25519 signature **verification** (RFC 8032), the
/// algorithm a TLS 1.3 server proves its identity with: twisted-Edwards
/// arithmetic in extended coordinates, point decompression with its curve
/// check, scalar reduction mod the group order, and `verify`. Entirely
/// executable; RFC 8032 §7.1's published vectors are pinned **natively**
/// (`ed25519_matches_rfc_8032`) since one verification exceeds the spec
/// sandbox's fuel. **Verify only, deliberately** — signing needs the secret
/// key handled in constant time, which `std::bignum` cannot provide, so
/// shipping a verifier alone is the honest subset. Not constant time, which
/// here costs nothing: every input to verification is public.
pub const ED25519: Module = Module {
    path: "std::ed25519",
    name: "std/ed25519.tuo",
    source: include_str!("std/ed25519.tuo"),
};

/// `std::tls` — the TLS 1.3 handshake and record layer (RFC 8446): record
/// framing, ClientHello parsing, ServerHello and the server's encrypted
/// flight, the transcript hash, the key schedule, and ChaCha20-Poly1305
/// record protection. Entirely executable; its key schedule is pinned against
/// **RFC 8448's published** secrets and its whole server flight against a
/// live **OpenSSL** client (`tls_server_completes_handshake_with_openssl`).
/// Every reader is **total** — a ClientHello arrives unauthenticated, and
/// tuonelang traps on overflow and out-of-bounds indexing, so a parser that
/// can trap is a remote denial of service. Supports X25519 + Ed25519 +
/// ChaCha20-Poly1305 only; no resumption, 0-RTT, client certificates,
/// HelloRetryRequest, or X.509 chain validation.
pub const TLS: Module = Module {
    path: "std::tls",
    name: "std/tls.tuo",
    source: include_str!("std/tls.tuo"),
};

/// `std::der` — DER decoding (ITU-T X.690) and the X.509 certificate fields
/// TLS needs (RFC 5280): bounds-checked tag/length headers, sequence
/// traversal, BIT STRING unwrapping, and the Ed25519 `subjectPublicKeyInfo`,
/// signature and signed-region extractors. Entirely executable, pinned
/// against a **real OpenSSL-emitted certificate** rather than bytes the
/// module invented. Every function is **total**: certificates arrive
/// unauthenticated, and tuonelang traps on overflow and out-of-bounds
/// indexing, so a parser that can trap is a remote denial of service. It is
/// a *decoder*, not a *validator* — it checks no signature, no date, and no
/// chain.
pub const DER: Module = Module {
    path: "std::der",
    name: "std/der.tuo",
    source: include_str!("std/der.tuo"),
};

/// `std::hkdf` — HMAC-based key derivation (RFC 5869) and TLS 1.3's key
/// schedule (RFC 8446 §7.1): `extract`/`expand`, TLS's `HKDF-Expand-Label`
/// and `Derive-Secret` with their `"tls13 "` domain separation, the
/// early/handshake/master secret chain, and the per-record traffic key, IV
/// and nonce. Entirely executable, pinned against RFC 5869's **published**
/// vectors — a key schedule that agrees only with itself yields a connection
/// where every record fails to decrypt. **Not constant time** (it builds on
/// `std::crypto`'s SHA-256/HMAC).
pub const HKDF: Module = Module {
    path: "std::hkdf",
    name: "std/hkdf.tuo",
    source: include_str!("std/hkdf.tuo"),
};

/// `std::x25519` — the X25519 Diffie-Hellman function (RFC 7748), TLS 1.3's
/// key-exchange primitive: field arithmetic mod 2^255 - 19, the Montgomery
/// ladder, scalar/u decoding with RFC 7748's mandatory clamping, and
/// `scalar_mult`/`public_key`. Entirely executable. The published RFC vector
/// is pinned **natively** (`x25519_matches_rfc_7748`) rather than in a spec:
/// one scalar multiplication is 255 ladder steps of 255-bit bignum
/// arithmetic, which exceeds the spec sandbox's fuel. **Not constant time**
/// — the ladder's structure is uniform, but `std::bignum` underneath is not,
/// so this is correctness without a timing guarantee.
pub const X25519: Module = Module {
    path: "std::x25519",
    name: "std/x25519.tuo",
    source: include_str!("std/x25519.tuo"),
};

/// `std::chacha` — the ChaCha20 stream cipher and Poly1305 authenticator
/// (RFC 8439), the AEAD half of a TLS 1.3 record layer: the quarter round and
/// block function, keystream application (`apply`, its own inverse), and the
/// Poly1305 one-time authenticator. Entirely executable, and pinned against
/// the RFC's **published** test vectors rather than its own reasoning — a
/// cipher that merely agrees with itself is worthless on the wire. Chosen
/// over AES because ChaCha20 is defined over the add/XOR/rotate operations
/// ADR-0019 gave the language, so it needs no lookup table and therefore has
/// no data-dependent memory access; AES in a high-level language is the
/// textbook cache-timing vulnerability. **Not** marked `#[constant_time]` —
/// see the module header for the exact scope of that claim.
pub const CHACHA: Module = Module {
    path: "std::chacha",
    name: "std/chacha.tuo",
    source: include_str!("std/chacha.tuo"),
};

/// `std::bignum` — arbitrary-precision non-negative integer arithmetic over
/// 28-bit limbs (ADR-0019 successor work): construction/normalization,
/// comparison and `bit_length`, `add`/`sub` (saturating at zero — there is no
/// sign), schoolbook `mul` and `mul_small`, bit shifts, division by a small
/// `Int`, and the text/byte conversions cryptographic protocols transmit
/// large numbers in (`to_decimal`/`from_decimal`, `to_hex`,
/// `from_be_bytes`/`to_be_bytes`). Entirely executable. **Not constant time**
/// — safe for public values, unsafe for secrets.
pub const BIGNUM: Module = Module {
    path: "std::bignum",
    name: "std/bignum.tuo",
    source: include_str!("std/bignum.tuo"),
};

/// `std::ct` — the branchless subset, for code whose timing must not reveal
/// its data (ADR-0020 Stage A): masks and branchless choice (`mask`/`select`),
/// fixed-time tests (`nonzero`/`is_zero`/`eq`/`ne`/`is_negative`/`lt`/`gt`),
/// and the array operations that scan rather than index
/// (`select_array`/`bytes_eq` — the fixed-time tag comparison whose
/// early-returning form is a textbook vulnerability). Entirely executable and
/// self-contained.
/// Written with no arithmetic whose overflow check could depend on secret
/// data, since a trap is a branch. It provides primitives that are branchless
/// **as emitted today**, pinned by a disassembly test on both backends — not a
/// constant-time guarantee, which awaits ADR-0020 Stage C.
pub const CT: Module = Module {
    path: "std::ct",
    name: "std/ct.tuo",
    source: include_str!("std/ct.tuo"),
};

/// `std::crypto` — cryptographic hashing and the primitives built on it
/// (ADR-0019 Stage B): SHA-256 (`sha256`/`sha256_bytes`), HMAC-SHA-256
/// (`hmac_sha256`), PBKDF2-HMAC-SHA-256 (`pbkdf2_sha256` — SCRAM's key
/// derivation), Base64 (`base64_encode`/`base64_decode`), hex rendering
/// (`to_hex`), the byte/text bridge (`bytes_of_str`/`str_of_bytes`), the
/// constant-time `verify` (delegating to `std::ct::bytes_eq`, so the safe
/// comparison is the convenient one and `==` on a tag is never the obvious
/// spelling), and the SCRAM-SHA-256 client exchange
/// (`scram_salted_password`/`scram_client_proof`/`scram_server_signature`,
/// RFC 7677 — what a PostgreSQL connector authenticates with), and — the last
/// ADR-0019 item — `md5`/`md5_password`, the legacy
/// `AuthenticationMD5Password` shim, shipped **documented as broken for
/// security** (practical collisions since 2004) and present only because old
/// servers still offer it, never as a hashing choice.
/// Entirely executable, and uniquely in this catalog its specs assert
/// **published** vectors (FIPS 180-4, RFC 4231, RFC 4648, RFC 1321) rather than the
/// module's own reasoning; RFC 7677's own vector needs 4096 PBKDF2 iterations,
/// more than the spec sandbox's fuel allows, so it is pinned by the native
/// `crypto_cross_check.rs` instead. It is a hashing library, not a TLS stack:
/// it authenticates but does not encrypt.
pub const CRYPTO: Module = Module {
    path: "std::crypto",
    name: "std/crypto.tuo",
    source: include_str!("std/crypto.tuo"),
};

/// `std::str` — string algorithms over the byte-level `Str`/`String` builtins:
/// search (`index_of_byte`/`find`/`contains`/`starts_with`/`ends_with`/
/// `count`), trimming (`trim`), ASCII case/classification (`to_upper`/
/// `to_lower`/`is_digit`/`is_space`), and the number⇄decimal-text conversions
/// (`parse_int`/`to_string`, `parse_float`/`float_to_string`). Entirely
/// executable.
pub const STR: Module = Module {
    path: "std::str",
    name: "std/str.tuo",
    source: include_str!("std/str.tuo"),
};

/// `std::io` — the I/O error vocabulary (executable), and the real
/// `print`/`println`/`read_line` I/O over the `std::rt` primitives (effect
/// tier — `read_line` builds an owned `String` from bytes read, ADR-0009).
pub const IO: Module = Module {
    path: "std::io",
    name: "std/io.tuo",
    source: include_str!("std/io.tuo"),
};

/// `std::json` — JSON decoding, navigation, and rendering over an index
/// arena (ADR-0016). Entirely executable.
pub const JSON: Module = Module {
    path: "std::json",
    name: "std/json.tuo",
    source: include_str!("std/json.tuo"),
};

/// `std::fs` — the file-system error vocabulary and path helpers (executable),
/// and the real disk operations `read`/`write`/`exists`/`remove` over the
/// ADR-0013 primitives (effect tier).
pub const FS: Module = Module {
    path: "std::fs",
    name: "std/fs.tuo",
    source: include_str!("std/fs.tuo"),
};

/// `std::net` — pure socket-outcome classification (executable), and the
/// real TCP operations `listen`/`bound_port`/`accept`/`connect`/`close` over
/// the ADR-0014 primitives (effect tier).
pub const NET: Module = Module {
    path: "std::net",
    name: "std/net.tuo",
    source: include_str!("std/net.tuo"),
};

/// `std::time` — `Duration` arithmetic, rendering, and `elapsed` (executable),
/// and the real `now` over `std::rt::now_nanos` (effect tier, ADR-0013).
pub const TIME: Module = Module {
    path: "std::time",
    name: "std/time.tuo",
    source: include_str!("std/time.tuo"),
};

/// `std::process` — `ExitStatus` reasoning (executable), and the real `exit`,
/// `arg_count`, and `arg` over the `std::rt` primitives (effect tier;
/// argv since ADR-0013).
pub const PROCESS: Module = Module {
    path: "std::process",
    name: "std/process.tuo",
    source: include_str!("std/process.tuo"),
};

/// `std::sync` — the pure state models of a latch and a lock (executable),
/// and structured fork-join (`par_map`, ADR-0007) plus channels and mutexes
/// (ADR-0015) as the effect tier.
pub const SYNC: Module = Module {
    path: "std::sync",
    name: "std/sync.tuo",
    source: include_str!("std/sync.tuo"),
};

/// `std::test` — assertion-helper predicates for use inside specs. Entirely
/// executable.
pub const TEST: Module = Module {
    path: "std::test",
    name: "std/test.tuo",
    source: include_str!("std/test.tuo"),
};

/// Every standard-library module, in dependency-friendly order (`core` first).
///
/// The order is stable: hosts that load the whole library into one program can
/// rely on it, and it is a topological order of the catalog's declared
/// dependency graph — `std::crypto` uses `std::bits`, so a module always
/// follows what it needs. Every other module stands alone. The graph is
/// declared and proven acyclic by `tuo-cli`'s `stdlib.rs`.
pub const MODULES: &[Module] = &[
    CORE,
    COLLECTIONS,
    MATH,
    BITS,
    BIGNUM,
    CHACHA,
    X25519,
    HKDF,
    DER,
    SHA512,
    ED25519,
    TLS,
    CT,
    CRYPTO,
    STR,
    JSON,
    IO,
    FS,
    NET,
    TIME,
    PROCESS,
    SYNC,
    TEST,
];

/// Look up a module by its dotted path (e.g. `"std::core"`).
#[must_use]
pub fn module(path: &str) -> Option<Module> {
    MODULES.iter().copied().find(|m| m.path == path)
}
