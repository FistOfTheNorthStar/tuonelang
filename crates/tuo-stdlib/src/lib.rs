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

/// `std::ed25519` — Ed25519 signatures (RFC 8032), the algorithm a TLS 1.3
/// server proves its identity with. **Verification** is twisted-Edwards
/// arithmetic over `std::bignum` (point decompression with its curve check,
/// scalar reduction mod the group order, `verify`) — not constant time,
/// which costs nothing there since every input is public. **Signing**
/// (`sign`, `public_key`) touches the secret key, so since ADR-0022 it runs
/// on `std::fe25519` points and fixed 21-bit-limb scalars modulo the group
/// order, every operation the secret passes through `#[constant_time]`.
/// Entirely executable; RFC 8032 §7.1's published vectors are pinned
/// natively for both halves (`ed25519_matches_rfc_8032`,
/// `ed25519_signs_rfc_8032_vectors`, the latter also against OpenSSL).
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
/// can trap is a remote denial of service. The client's Finished is verified
/// against the transcript before the switch to application keys (RFC 8446
/// §4.4.4) and a small-order X25519 key share is refused (§7.4.2), each
/// pinned natively by a test that sends the forbidden input
/// (`tls_server_verifies_the_client_finished`,
/// `tls_rejects_small_order_key_shares`). Supports X25519 + Ed25519 +
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

/// `std::fe25519` — fixed-width arithmetic in GF(2^255 - 19) (ADR-0022): a
/// ten-limb `Fe` struct (radix 2^25.5) with `add`/`sub`/`mul`/`square`/
/// `mul_small`/`select`/`freeze` written as straight-line code over
/// `std::int`'s wrapping operations and marked `#[constant_time]`, so the
/// compiler verifies they never branch on, index by, or trap on a value;
/// plus `invert` and the byte conversions, whose loops run fixed counts.
/// Entirely executable. The constant-time field `std::x25519`'s ladder runs
/// on, and a separate type from `std::bignum`'s variable-time `Array[Int]`
/// so the two cannot be confused.
pub const FE25519: Module = Module {
    path: "std::fe25519",
    name: "std/fe25519.tuo",
    source: include_str!("std/fe25519.tuo"),
};

/// `std::x25519` — the X25519 Diffie-Hellman function (RFC 7748), TLS 1.3's
/// key-exchange primitive: the Montgomery ladder over `std::fe25519`,
/// RFC 7748's mandatory clamping, and `scalar_mult`/`public_key`, plus the
/// older `std::bignum` field arithmetic `std::ed25519` builds on. Entirely
/// executable, with RFC 7748's published vectors asserted in its specs and
/// natively (`x25519_matches_rfc_7748`). The ladder and its swap are
/// `#[constant_time]`; the bignum `field_*` functions are variable time and
/// serve only public values.
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
/// textbook cache-timing vulnerability. Poly1305 is fixed-limb arithmetic
/// whose block step and final reduction carry `#[constant_time]` (ADR-0022);
/// ChaCha20's own functions loop and index, so they stay unmarked — see the
/// module header for the exact scope of that claim.
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
    FE25519,
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

/// The catalog's declared intra-library dependency edges, as
/// `(module, every module it may use)` — each list is the module's whole
/// closure, so loading a module with exactly its list is enough to compile it.
///
/// Most modules stand alone. `std::crypto` was the first that did not: its
/// algorithms are *defined* over the fixed-width bit and byte-order operations
/// `std::bits` owns (ADR-0019 Stage B), and the alternative — a second copy of
/// `rotr32`/`add32`/`be32` inside `std::crypto` — would be two implementations
/// of one specification, free to drift apart. One audited copy is worth the
/// edge, and the same reasoning gives every other entry here.
///
/// This table is the *whole* permitted graph: a module absent from it must
/// compile alone. `tuo-cli/tests/stdlib.rs` checks every module with exactly
/// its listed dependencies (so an undeclared use fails to resolve) and proves
/// the graph acyclic; hosts use it to load what a program's modules need
/// ([`referenced_by`]).
pub const DEPENDENCIES: &[(&str, &[&str])] = &[
    ("std::crypto", &["std::bits", "std::ct"]),
    // ChaCha20 is defined over 32-bit add/XOR/rotate, which is exactly what
    // `std::bits` owns — reimplementing them here would be a second copy free
    // to drift from the one `std::crypto` already depends on.
    // `std::crypto` for the constant-time tag comparison (`verify`), which
    // must have one implementation rather than a re-derived copy in the
    // module that most needs it to be right; `std::crypto` in turn brings
    // `std::ct` and `std::bits`.
    ("std::chacha", &["std::bits", "std::ct", "std::crypto"]),
    // `std::ct` for the ladder's conditional swap: exchanging the two points
    // on a bit of the secret scalar is the textbook X25519 timing leak, so the
    // selection is the shared branchless one rather than a re-derived `if`.
    // The constant-time field takes `std::ct::select` for its swap rather
    // than a second copy of the mask.
    ("std::fe25519", &["std::ct"]),
    ("std::x25519", &["std::bignum", "std::ct", "std::fe25519"]),
    // HKDF is HMAC-SHA256 in a loop, so it depends on `std::crypto` (which
    // brings `std::bits` and `std::ct`) rather than carrying a second copy.
    ("std::hkdf", &["std::bits", "std::ct", "std::crypto"]),
    // Ed25519 reuses `std::x25519`'s bignum field arithmetic (same prime)
    // for verification and `std::fe25519` for signing, rather than shipping
    // second copies, and is defined over SHA-512.
    (
        "std::ed25519",
        &[
            "std::bits",
            "std::ct",
            "std::bignum",
            "std::fe25519",
            "std::x25519",
            "std::sha512",
        ],
    ),
    // The protocol layer composes every primitive below it.
    (
        "std::tls",
        &[
            "std::bits",
            "std::ct",
            "std::bignum",
            "std::crypto",
            "std::chacha",
            "std::hkdf",
            "std::fe25519",
            "std::x25519",
        ],
    ),
];

/// The modules `path` may use (its declared closure), or an empty slice for a
/// module that stands alone.
#[must_use]
pub fn dependencies(path: &str) -> &'static [&'static str] {
    DEPENDENCIES
        .iter()
        .find(|(name, _)| *name == path)
        .map_or(&[], |(_, deps)| *deps)
}

/// The catalog modules a program written as `programs` needs loaded beside it
/// (ADR-0038): every module whose path the text names (`std::core::abs(x)`,
/// `import std::io;`), together with its [declared dependencies](dependencies),
/// in catalog order — minus any module the program declares itself (a program
/// carrying its own copy of `std::io` keeps it, rather than meeting a second).
///
/// The scan is lexical and deliberately generous: a path mentioned inside a
/// comment or a string also loads its module. Loading a module that goes
/// unused costs a little checking time and nothing else — every module checks
/// cleanly, and only what `main` reaches is compiled natively — whereas a
/// missed module would turn a correct call into a resolution error.
#[must_use]
pub fn referenced_by<'a>(programs: impl IntoIterator<Item = &'a str>) -> Vec<Module> {
    let mut named: Vec<&'static str> = Vec::new();
    let mut declared: Vec<String> = Vec::new();
    for text in programs {
        for line in text.lines() {
            if let Some(rest) = line.trim_start().strip_prefix("module ") {
                if let Some(path) = rest.trim().strip_suffix(';') {
                    declared.push(path.trim().to_owned());
                }
            }
        }
        let mut rest = text;
        while let Some(at) = rest.find("std::") {
            let after = &rest[at + "std::".len()..];
            let end = after
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(after.len());
            let path = &rest[at..at + "std::".len() + end];
            if let Some(module) = MODULES.iter().find(|m| m.path == path) {
                if !named.contains(&module.path) {
                    named.push(module.path);
                }
            }
            rest = after;
        }
    }
    let mut needed: Vec<&'static str> = Vec::new();
    for path in named {
        for dependency in dependencies(path).iter().chain(std::iter::once(&path)) {
            if !needed.contains(dependency) {
                needed.push(dependency);
            }
        }
    }
    MODULES
        .iter()
        .copied()
        .filter(|m| needed.contains(&m.path) && !declared.iter().any(|d| d == m.path))
        .collect()
}

/// One public function of the catalog, as its module declares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Export {
    /// The module's path, e.g. `"std::core"`.
    pub module: &'static str,
    /// The function's name, e.g. `"abs"`.
    pub name: String,
    /// The declaration as written, on one line: `fn abs(take n: Int) -> Int`.
    pub declaration: String,
}

/// Every `pub fn` the catalog declares, in catalog order.
///
/// Read from the module sources' declarations — the same text a caller types
/// and the brief prints — so a host can answer "which module has `sum`?"
/// without compiling the library. That matters since ADR-0038: a module is
/// loaded only once a program names it, and the moment a tool is asked where
/// a function lives is exactly the moment the program does not name it yet.
#[must_use]
pub fn exports() -> Vec<Export> {
    let mut out = Vec::new();
    for module in MODULES {
        let mut lines = module.source.lines();
        while let Some(line) = lines.next() {
            let Some(rest) = line.strip_prefix("pub fn ") else {
                continue;
            };
            // A long signature continues over several lines up to its `{`.
            let mut declaration = format!("fn {rest}");
            while !declaration.trim_end().ends_with('{') {
                let Some(next) = lines.next() else { break };
                declaration.push(' ');
                declaration.push_str(next.trim());
            }
            let declaration = declaration
                .trim_end()
                .trim_end_matches('{')
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .replace("( ", "(")
                .replace(", )", ")")
                .replace(",)", ")");
            // An omitted return type is `()`; say so, as the brief does. The
            // check looks past the parameter list's closing parenthesis, since
            // a function-typed parameter has an `->` of its own.
            let mut depth = 0_i32;
            let after_params = declaration.char_indices().find_map(|(i, c)| {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(i + 1);
                        }
                    }
                    _ => {}
                }
                None
            });
            let declaration = match after_params {
                Some(end) if !declaration[end..].contains("->") => {
                    format!("{} -> ()", declaration[..end].trim_end())
                }
                _ => declaration,
            };
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            out.push(Export {
                module: module.path,
                name,
                declaration,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::referenced_by;

    #[test]
    fn every_export_is_one_normalized_declaration() {
        let exports = super::exports();
        assert!(exports.len() > 400, "{} exports", exports.len());
        let abs = exports
            .iter()
            .find(|e| e.module == "std::core" && e.name == "abs")
            .expect("std::core::abs");
        assert_eq!(abs.declaration, "fn abs(take n: Int) -> Int");
        // A declaration written over several lines comes back on one.
        let seal = exports
            .iter()
            .find(|e| e.module == "std::chacha" && e.name == "aead_seal")
            .expect("std::chacha::aead_seal");
        assert!(!seal.declaration.contains('\n') && !seal.declaration.contains("  "));
        assert!(
            seal.declaration
                .starts_with("fn aead_seal(in key: Array[Int],"),
            "{}",
            seal.declaration
        );
        assert!(
            seal.declaration.ends_with(") -> Array[Int]"),
            "{}",
            seal.declaration
        );
        // An omitted return type reads as `()`, but a function-typed
        // parameter's own `->` does not count as the return type.
        let push = exports
            .iter()
            .find(|e| e.module == "std::str" && e.name == "push")
            .expect("std::str::push");
        assert_eq!(
            push.declaration,
            "fn push(mut b: String, in piece: Str) -> ()"
        );
        let fold = exports
            .iter()
            .find(|e| e.module == "std::collections" && e.name == "fold")
            .expect("std::collections::fold");
        assert!(
            fold.declaration.ends_with(") -> Int"),
            "{}",
            fold.declaration
        );
    }

    fn paths(text: &str) -> Vec<&'static str> {
        referenced_by([text]).iter().map(|m| m.path).collect()
    }

    #[test]
    fn a_named_module_is_loaded_with_its_dependencies() {
        assert_eq!(
            paths("fn f() -> Int { std::core::abs(0 - 1) }"),
            ["std::core"]
        );
        assert_eq!(
            paths("import std::hkdf;"),
            ["std::bits", "std::hkdf", "std::ct", "std::crypto"]
        );
    }

    #[test]
    fn builtin_only_paths_load_nothing() {
        assert!(paths("std::array::push(xs, 1); std::string::len(s); std::rt::exit(0)").is_empty());
    }

    #[test]
    fn a_module_the_program_declares_is_not_loaded_twice() {
        let text = "module std::io;\npub fn print(in t: Str) -> Int { 0 }";
        assert!(paths(text).is_empty());
        assert_eq!(
            paths("module app;\nfn f() -> Int { std::io::x() }"),
            ["std::io"]
        );
    }

    #[test]
    fn a_longer_name_is_not_mistaken_for_a_shorter_module() {
        // `std::coreutils` is not `std::core`.
        assert!(paths("std::coreutils::f()").is_empty());
    }
}
