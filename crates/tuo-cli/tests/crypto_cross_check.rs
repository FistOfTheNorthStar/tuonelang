//! ADR-0019 Stage B's headline acceptance criterion: the tuonelang
//! `std::crypto::sha256` must agree with `tuo-package`'s **Rust** `sha256`,
//! byte for byte, on the same inputs.
//!
//! This is the test the ADR was opened for. The workspace has always contained
//! a hand-rolled SHA-256 (`crates/tuo-package/src/sha256.rs`, written in Rust
//! with `rotate_right`/`^`/`>>`/`&` precisely so the workspace need not take a
//! crypto dependency) — and before Stage A, tuonelang could not express that
//! function at all, because it had no bitwise operators. "The language cannot
//! reproduce its own package manager's checksum" was the sharpest statement of
//! the gap; this file is its refutation.
//!
//! The comparison is genuinely independent: one side is Rust running in this
//! test process, the other is a **native tuonelang binary** produced by the
//! real compiler and executed as a subprocess. Neither computes the other's
//! answer, so agreement is evidence rather than tautology.

use std::path::PathBuf;
use std::process::Command;

/// A fresh scratch directory for one test, cleared on each run.
///
/// The `name` is **per test, not per file**: cargo runs a file's tests in
/// parallel threads, so a directory shared between two tests has one thread
/// `remove_dir_all`-ing it while the other is still writing sources into it.
/// That produced an intermittent failure — the worst kind, since it passes on
/// a re-run and only bites in CI.
fn scratch_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("crypto_cross_check")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch workspace is creatable");
    dir
}

/// The inputs to cross-check. Chosen to cover the padding boundaries, since
/// that is where a hash implementation usually differs: the empty input, a
/// short one, one that lands exactly on a block boundary (55/56/64 bytes), and
/// one long enough to need a second compression block.
const INPUTS: &[&str] = &[
    "",
    "a",
    "abc",
    "hello world",
    "The quick brown fox jumps over the lazy dog",
    // 55 bytes: the largest input whose padding still fits one block.
    "0123456789012345678901234567890123456789012345678901234",
    // 56 bytes: the smallest input that forces a second block.
    "01234567890123456789012345678901234567890123456789012345",
    // 64 bytes: exactly one block before padding.
    "0123456789012345678901234567890123456789012345678901234567890123",
    // Two blocks and change.
    "abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu",
];

/// The tuonelang program: hash each input and print `<hex>` on its own line,
/// in the same order as [`INPUTS`].
fn program() -> String {
    let mut main = String::from("fn main() -> Int {\n");
    for input in INPUTS {
        // The inputs are ASCII with no escapes, so they embed directly.
        assert!(
            input.is_ascii() && !input.contains('"') && !input.contains('\\'),
            "a cross-check input must embed in a tuonelang literal without escaping"
        );
        main.push_str(&format!(
            "    std::io::println(std::string::as_str(std::crypto::sha256(\"{input}\")));\n"
        ));
    }
    main.push_str("    0\n}\n");
    main
}

#[test]
fn tuonelang_sha256_agrees_with_the_toolchains_rust_sha256() {
    let dir = scratch_dir("rust-cross-check");

    // The program plus every module it needs, as real catalog sources — so
    // this checks the *shipped* library, not a copy written for the test.
    let main_path = dir.join("main.tuo");
    std::fs::write(&main_path, program()).expect("write the cross-check program");
    let mut sources = vec![main_path];
    for path in ["std::crypto", "std::bits", "std::ct", "std::str", "std::io"] {
        let module = tuo_stdlib::module(path).expect("a catalog module");
        let file = dir.join(module.name.replace('/', "_"));
        std::fs::write(&file, module.source).expect("write a catalog module");
        sources.push(file);
    }

    let output = Command::new(env!("CARGO_BIN_EXE_tuo"))
        .arg("run")
        .args(&sources)
        .output()
        .expect("run the cross-check program");
    assert!(
        output.status.success(),
        "the cross-check program did not run:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("digest output is UTF-8");
    let produced: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        produced.len(),
        INPUTS.len(),
        "expected one digest per input, got:\n{stdout}"
    );

    for (input, tuonelang) in INPUTS.iter().zip(produced) {
        // `hex` takes the *input bytes* and returns the digest's hex form; it
        // is not a formatter over an already-computed digest.
        let rust = tuo_package::sha256::hex(input.as_bytes());
        assert_eq!(
            tuonelang,
            rust,
            "tuonelang and the toolchain's Rust SHA-256 disagree on an input of {} bytes",
            input.len()
        );
    }
}

/// A complete **SCRAM-SHA-256 client proof**, computed by a native tuonelang
/// binary and compared against RFC 7677 §3's published vector.
///
/// This is ADR-0019's motivating case reduced to one assertion. A PostgreSQL
/// server has defaulted `password_encryption` to `scram-sha-256` since version
/// 14, so this exchange — not the legacy MD5 challenge — is what a connector
/// must perform to authenticate against a current server. Computing it needs
/// essentially all of Stage B at once: Base64 (decoding the server's salt and
/// encoding the proof), PBKDF2-HMAC-SHA-256 over 4096 iterations, HMAC-SHA-256
/// twice, a bare SHA-256, and a byte-wise XOR — all of which rest on Stage A's
/// bitwise operators.
///
/// The expected value is **published**, not this implementation's output, so
/// agreement is evidence rather than self-agreement.
#[test]
fn a_native_scram_sha256_client_proof_matches_the_rfc_7677_vector() {
    let dir = scratch_dir("scram-proof");

    // RFC 7677 §3's exchange: user "user", password "pencil", the server's
    // salt and iteration count, and the auth message built from both nonces.
    let program = r#"module caller;

import std::crypto;

fn main() -> Int {
    // RFC 7677 section 3's exchange, assembled the way a connector assembles
    // it: the salt arrives base64-encoded in the server's first message, the
    // auth message is the three protocol messages joined by commas.
    let auth = "n=user,r=rOprNGfwEbeRWgbNEkqO,r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096,c=biws,r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0";
    let salt = std::crypto::base64_decode("W22ZaJ0SNY7soEsUEjb6gQ==");
    let salted = std::crypto::scram_salted_password("pencil", salt, 4096);

    let proof = std::crypto::scram_client_proof(salted, auth);
    if std::string::as_str(proof) != "dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ=" {
        return 1;
    }

    // The server signature the client must check before trusting the server.
    let server = std::crypto::scram_server_signature(salted, auth);
    if std::string::as_str(server) != "6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4=" {
        return 2;
    }

    // And it must be checked with the constant-time comparison, which is what
    // `verify` exists to make the convenient spelling.
    let expected = std::crypto::base64_decode("6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4=");
    if std::crypto::verify(std::crypto::base64_decode(std::string::as_str(server)), expected) == false {
        return 3;
    }
    // A wrong signature must be rejected.
    if std::crypto::verify(std::crypto::base64_decode(std::string::as_str(proof)), expected) {
        return 4;
    }
    42
}
"#;

    let main_path = dir.join("scram.tuo");
    std::fs::write(&main_path, program).expect("write the SCRAM program");
    let mut sources = vec![main_path];
    for path in ["std::crypto", "std::bits", "std::ct"] {
        let module = tuo_stdlib::module(path).expect("a catalog module");
        let file = dir.join(module.name.replace('/', "_"));
        std::fs::write(&file, module.source).expect("write a catalog module");
        sources.push(file);
    }

    let status = Command::new(env!("CARGO_BIN_EXE_tuo"))
        .arg("run")
        .args(&sources)
        .status()
        .expect("run the SCRAM program");
    assert_eq!(
        status.code(),
        Some(42),
        "the computed SCRAM-SHA-256 client proof does not match RFC 7677's published vector"
    );
}

/// Every worked example in `std::bits` and `std::crypto`'s doc comments is a
/// true statement, checked by really evaluating it.
///
/// The stdlib suite proves each public function *has* an example and that its
/// signature is real, but nothing checks an example's stated *value* — a doc
/// saying `// 1024` beside a call returning something else is exactly the kind
/// of confidently-wrong text the ADR-0018 cheatsheet exists to prevent, and it
/// would ship silently. These two modules are the ones where a wrong example
/// does the most damage: a caller who mis-implements a hash gets a plausible
/// digest, not an error.
///
/// This covers the two ADR-0019 modules only. A general extractor over all
/// fourteen catalog modules is the right eventual shape and is deliberately
/// not attempted here.
#[test]
fn every_doc_example_in_the_adr_0019_modules_is_accurate() {
    let dir = scratch_dir("doc-examples");

    let program = r#"module caller;

import std::crypto;

fn main() -> Int {
    // std::bits
    if std::bits::mask32() != 4294967295 { return 1; }
    if std::bits::low32(4294967296) != 0 { return 2; }
    if std::bits::low8(258) != 2 { return 3; }
    if std::bits::add32(4294967295, 1) != 0 { return 4; }
    if std::bits::mul32(65536, 65536) != 0 { return 5; }
    if std::bits::rotr32(1, 1) != 2147483648 { return 6; }
    if std::bits::rotl32(2147483648, 1) != 1 { return 7; }
    if std::bits::shr32(4294967295, 24) != 255 { return 8; }
    if std::bits::shl32(1, 31) != 2147483648 { return 9; }
    if std::bits::be32(0, 0, 1, 2) != 258 { return 10; }
    if std::bits::be16(1, 2) != 258 { return 11; }
    if std::bits::byte_of_be32(258, 3) != 2 { return 12; }
    if std::bits::byte_of_be16(258, 1) != 2 { return 13; }
    if !std::bits::test_bit(5, 2) { return 14; }
    if std::bits::count_ones32(255) != 8 { return 15; }

    // std::crypto
    if std::array::len(std::crypto::sha256_round_constants()) != 64 { return 20; }
    if std::array::len(std::crypto::sha256_pad("abc")) != 64 { return 21; }
    if std::array::get(std::crypto::sha256_bytes("abc"), 0) != 186 { return 22; }
    if std::string::as_str(std::crypto::sha256("abc")) != "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad" { return 23; }
    if std::string::as_str(std::crypto::to_hex(std::crypto::bytes_of_str("abc"))) != "616263" { return 24; }
    if std::crypto::hex_digit(10) != 97 { return 25; }
    if std::array::get(std::crypto::bytes_of_str("abc"), 0) != 97 { return 26; }
    if std::string::as_str(std::crypto::str_of_bytes(std::crypto::bytes_of_str("abc"))) != "abc" { return 27; }
    if std::array::len(std::crypto::pbkdf2_sha256("password", "salt", 1)) != 32 { return 28; }
    if std::crypto::base64_char(0) != 65 { return 29; }
    if std::crypto::base64_value(65) != 0 { return 30; }
    if std::string::as_str(std::crypto::base64_encode(std::crypto::bytes_of_str("Man"))) != "TWFu" { return 31; }
    if std::array::get(std::crypto::base64_decode("TWFu"), 0) != 77 { return 32; }
    if std::array::len(std::crypto::repeated_byte(11, 20)) != 20 { return 33; }

    // The HMAC example's stated first byte, 0xF7.
    let key = std::crypto::bytes_of_str("key");
    let msg = std::crypto::bytes_of_str("The quick brown fox jumps over the lazy dog");
    if std::array::get(std::crypto::hmac_sha256(key, msg), 0) != 247 { return 34; }

    42
}
"#;

    let main_path = dir.join("doc_examples.tuo");
    std::fs::write(&main_path, program).expect("write the doc-example program");
    let mut sources = vec![main_path];
    for path in ["std::crypto", "std::bits", "std::ct"] {
        let module = tuo_stdlib::module(path).expect("a catalog module");
        let file = dir.join(module.name.replace('/', "_"));
        std::fs::write(&file, module.source).expect("write a catalog module");
        sources.push(file);
    }

    let status = Command::new(env!("CARGO_BIN_EXE_tuo"))
        .arg("run")
        .args(&sources)
        .status()
        .expect("run the doc-example program");
    assert_eq!(
        status.code(),
        Some(42),
        "a doc example in std::bits or std::crypto states a value the function \
         does not produce (the exit code is the failing check's number)"
    );
}

/// `std::bignum`'s arithmetic agrees with an independent arbitrary-precision
/// implementation on values far beyond what `Int` can hold.
///
/// The expected values below were computed with Python's built-in integers and
/// are pasted in as literals, so this is a genuine cross-check rather than the
/// module agreeing with itself. (Writing them by hand was tried first and got
/// three of six wrong — which is exactly why the reference has to be a real
/// implementation, not arithmetic done in one's head.)
///
/// The operands are 30 and 20 decimal digits: comfortably past `Int`'s 19, so
/// every operation exercises multi-limb carry, borrow, and accumulation paths
/// that a single-limb value never reaches.
#[test]
fn bignum_arithmetic_agrees_with_an_independent_implementation() {
    let dir = scratch_dir("bignum-cross-check");

    let program = r#"module caller;

import std::bignum;

fn check(in got: String, in want: Str) -> Bool {
    std::string::as_str(got) == want
}

fn main() -> Int {
    let a = std::bignum::num_or_zero(std::bignum::from_decimal("123456789012345678901234567890"));
    let b = std::bignum::num_or_zero(std::bignum::from_decimal("98765432109876543210"));

    if !check(std::bignum::to_decimal(std::bignum::add(a, b)),
              "123456789111111111011111111100") { return 1; }
    if !check(std::bignum::to_decimal(std::bignum::sub(a, b)),
              "123456788913580246791358024680") { return 2; }
    if !check(std::bignum::to_decimal(std::bignum::mul(a, b)),
              "12193263113702179522496570642237463801111263526900") { return 3; }
    if !check(std::bignum::to_decimal(std::bignum::div_small(a, 999983)),
              "123458887813438507355859") { return 4; }
    if std::bignum::rem_small(a, 999983) != 617493 { return 5; }
    if !check(std::bignum::to_hex(a), "18ee90ff6c373e0ee4e3f0ad2") { return 6; }

    // 2^2047 is 256 bytes big-endian — the RSA-2048 modulus width that
    // motivated the module.
    if std::array::len(std::bignum::to_be_bytes(
        std::bignum::shift_left(std::bignum::from_int(1), 2047))) != 256 { return 7; }

    // A byte round-trip over a value with no small representation.
    if !std::bignum::equals(
        std::bignum::from_be_bytes(std::bignum::to_be_bytes(a)), a) { return 8; }

    42
}
"#;

    let main_path = dir.join("bignum.tuo");
    std::fs::write(&main_path, program).expect("write the bignum program");
    let module = tuo_stdlib::module("std::bignum").expect("a catalog module");
    let module_path = dir.join(module.name.replace('/', "_"));
    std::fs::write(&module_path, module.source).expect("write the catalog module");

    let status = Command::new(env!("CARGO_BIN_EXE_tuo"))
        .arg("run")
        .arg(&main_path)
        .arg(&module_path)
        .status()
        .expect("run the bignum program");
    assert_eq!(
        status.code(),
        Some(42),
        "std::bignum disagrees with the independent reference (the exit code is \
         the failing check's number)"
    );
}

/// `std::bignum` handles the operand sizes that justified building it.
///
/// The module exists because public-key cryptography is defined over numbers
/// `Int` cannot hold, so "it works on 30-digit values" is not the bar — these
/// are the actual sizes: X25519's prime 2^255 - 19 (32 bytes), an RSA-2048
/// modulus (256 bytes), and a 1024x1024-bit multiply, which is the core
/// operation of both RSA and Diffie-Hellman.
///
/// This does **not** claim the module is ready for those algorithms: modular
/// exponentiation and inversion are not implemented, and nothing here is
/// constant time. It claims only that the arithmetic does not fall over at
/// the widths they need.
#[test]
fn bignum_handles_cryptographic_operand_sizes() {
    let dir = scratch_dir("bignum-crypto-sizes");

    let program = r#"module caller;

import std::bignum;

fn main() -> Int {
    // X25519's prime: 2^255 - 19, which is 255 bits and 32 bytes.
    let p25519 = std::bignum::sub(std::bignum::shift_left(std::bignum::from_int(1), 255),
                                  std::bignum::from_int(19));
    if std::bignum::bit_length(p25519) != 255 { return 1; }
    if std::array::len(std::bignum::to_be_bytes(p25519)) != 32 { return 2; }

    // An RSA-2048 modulus: 2048 bits, 256 bytes.
    let rsa = std::bignum::sub(std::bignum::shift_left(std::bignum::from_int(1), 2048),
                               std::bignum::from_int(1));
    if std::bignum::bit_length(rsa) != 2048 { return 3; }
    if std::array::len(std::bignum::to_be_bytes(rsa)) != 256 { return 4; }

    // Two 1024-bit operands multiply to a 2047-bit product.
    let a = std::bignum::shift_left(std::bignum::from_int(1), 1023);
    if std::bignum::bit_length(std::bignum::mul(a, a)) != 2047 { return 5; }

    42
}
"#;

    let main_path = dir.join("sizes.tuo");
    std::fs::write(&main_path, program).expect("write the sizes program");
    let module = tuo_stdlib::module("std::bignum").expect("a catalog module");
    let module_path = dir.join(module.name.replace('/', "_"));
    std::fs::write(&module_path, module.source).expect("write the catalog module");

    let status = Command::new(env!("CARGO_BIN_EXE_tuo"))
        .arg("run")
        .arg(&main_path)
        .arg(&module_path)
        .status()
        .expect("run the sizes program");
    assert_eq!(
        status.code(),
        Some(42),
        "std::bignum fails at a cryptographic operand size (the exit code is \
         the failing check's number)"
    );
}
