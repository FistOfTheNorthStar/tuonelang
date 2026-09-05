//! The dogfooding examples, kept honest by the real compiler.
//!
//! Prompt 39 built five real programs under top-level `examples/` (see
//! `DOGFOODING.md`). This suite drives the **real `tuo` binary** over every one
//! of them and asserts the exact contract each example's own doc comment and
//! specs promise, so the committed examples can never silently rot into
//! programs the compiler no longer accepts — the same discipline the shipped
//! corpus and the benchmark task-sets hold themselves to.
//!
//! For each example it asserts:
//!
//! * `tuo check` accepts it (front end: parse → resolve → type-check →
//!   ownership, specs included);
//! * `tuo test --manifest <pkg>` runs its specs green with **no** failures (the
//!   package is real, its manifest resolves, and every `spec` passes); and
//! * for the programs whose logic lives in the runnable core, `tuo run`
//!   compiles-links-runs them to the exact exit byte each example documents —
//!   proving "it runs natively" is a fact, not a claim. Since ADR-0006 landed
//!   the effect boundary, two examples also pin their **observable stdout**
//!   byte-for-byte: `cli-stats` prints its report through `std::io::println`
//!   (the ADR's "a CLI example gains a real println whose output is observed
//!   by a test" oracle) and `http-service` prints its response status line
//!   through its `std::rt::write` shell.
//!
//! `cli-stats` consumes the standard library as input: its `src/std_io.tuo` is
//! a verbatim copy of `tuo-stdlib`'s `std::io` module, and this suite pins the
//! copy byte-for-byte against the catalog so the two can never drift.
//!
//! The multi-package `workspace/app` binary is validated via
//! `tuo build --manifest … -o …` then executed, because `tuo run` is file-based
//! and has no package-aware form (DOGFOODING.md finding D-7).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The repository root, reached from this crate's manifest directory.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The absolute path to one example's directory under `examples/`.
fn example_dir(name: &str) -> PathBuf {
    repo_root().join("examples").join(name)
}

/// Run `tuo` with `args` and return the captured output.
fn tuo(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tuo"))
        .args(args)
        .output()
        .expect("the tuo binary runs")
}

/// Assert a command exited zero, printing both streams on failure.
fn expect_ok(output: &Output, what: &str) {
    assert!(
        output.status.success(),
        "{what} failed (status {:?})\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

/// `tuo check <srcs…>` must accept the program (all files form one program).
fn assert_checks(srcs: &[&Path]) {
    let mut args = vec!["check".to_string()];
    args.extend(srcs.iter().map(|s| s.display().to_string()));
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = tuo(&arg_refs);
    expect_ok(&out, &format!("tuo check {args:?}"));
}

/// `tuo test --manifest <pkg>` must run every spec green. We assert on success
/// status *and* on the absence of any failure marker in the human report, so a
/// silently-skipped or failing spec cannot pass this gate.
fn assert_specs_green(pkg: &Path) {
    let out = tuo(&["test", "--manifest", &pkg.display().to_string()]);
    expect_ok(&out, &format!("tuo test --manifest {}", pkg.display()));
    let report = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    // A green run reports `N passed, 0 failed` and names no FAILED spec. Both
    // are asserted so a silently-skipped or failing spec cannot slip through.
    assert!(
        !report.contains("FAILED"),
        "a spec was reported FAILED for {}:\n{report}",
        pkg.display()
    );
    assert!(
        report.contains("0 failed"),
        "expected `0 failed` in the spec report for {}:\n{report}",
        pkg.display()
    );
}

/// `tuo run <srcs…>` must compile, link, run, and exit with `expected` (the
/// documented observable exit byte). Returns the output so callers can also
/// assert the program's observable stdout.
fn assert_runs_to(srcs: &[&Path], expected: i32) -> Output {
    let mut args = vec!["run".to_string()];
    args.extend(srcs.iter().map(|s| s.display().to_string()));
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = tuo(&arg_refs);
    let code = out.status.code();
    assert_eq!(
        code,
        Some(expected),
        "tuo run {args:?} exited {code:?}, expected {expected}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stderr),
    );
    out
}

/// As `assert_runs_to`, but with the program's working directory set to `cwd`.
/// `file-report` really creates and removes a file relative to the working
/// directory, so it is run inside a scratch directory rather than the
/// repository, and the test can then assert the scratch directory is left
/// clean.
fn assert_runs_to_in(srcs: &[&Path], expected: i32, cwd: &Path) -> Output {
    let mut args = vec!["run".to_string()];
    args.extend(srcs.iter().map(|s| s.display().to_string()));
    let out = Command::new(env!("CARGO_BIN_EXE_tuo"))
        .args(&args)
        .current_dir(cwd)
        .output()
        .expect("the tuo binary runs");
    let code = out.status.code();
    assert_eq!(
        code,
        Some(expected),
        "tuo run {args:?} (in {}) exited {code:?}, expected {expected}\nstderr:\n{}",
        cwd.display(),
        String::from_utf8_lossy(&out.stderr),
    );
    out
}

/// cli-stats: a runnable command-line statistics tool. Since ADR-0006 it
/// prints its report through `std::io::println` (consuming the stdlib module
/// as input); stdout is the exact four-line report and exit = 18.
#[test]
fn cli_stats_checks_specs_runs_and_prints_its_report() {
    let dir = example_dir("cli-stats");
    let main = dir.join("src/main.tuo");
    let std_io = dir.join("src/std_io.tuo");
    assert_checks(&[&main, &std_io]);
    assert_specs_green(&dir);
    let out = assert_runs_to(&[&main, &std_io], 18);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "count 7\nmean 12\nsd 6\nreport 18\n",
        "cli-stats must print its documented report, byte for byte"
    );
}

/// cli-stats consumes the standard library as input: its vendored
/// `src/std_io.tuo` must equal the `tuo-stdlib` catalog's `std::io` module
/// byte-for-byte, so the example can never drift from the shipped library.
#[test]
fn cli_stats_vendored_std_io_matches_the_catalog() {
    let vendored = example_dir("cli-stats").join("src/std_io.tuo");
    let on_disk = std::fs::read_to_string(&vendored)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", vendored.display()));
    assert_eq!(
        on_disk,
        tuo_stdlib::IO.source,
        "examples/cli-stats/src/std_io.tuo drifted from tuo-stdlib's std/io.tuo; \
         re-copy the catalog module"
    );
}

/// postgres-auth vendors four catalog modules, and each must equal the
/// `tuo-stdlib` original byte-for-byte.
///
/// The pin matters more here than elsewhere: `std_crypto.tuo` and
/// `std_ct.tuo` carry the code that decides whether an authentication proof is
/// correct and whether a signature comparison leaks timing. A vendored copy
/// that silently diverged from the catalog would keep passing this example's
/// own specs while no longer being the library the rest of the suite verifies.
#[test]
fn postgres_auth_vendored_modules_match_the_catalog() {
    let dir = example_dir("postgres-auth");
    for (file, module) in [
        ("std_bits.tuo", tuo_stdlib::BITS),
        ("std_ct.tuo", tuo_stdlib::CT),
        ("std_crypto.tuo", tuo_stdlib::CRYPTO),
        ("std_str.tuo", tuo_stdlib::STR),
    ] {
        let vendored = dir.join("src").join(file);
        let on_disk = std::fs::read_to_string(&vendored)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", vendored.display()));
        assert_eq!(
            on_disk, module.source,
            "examples/postgres-auth/src/{file} drifted from tuo-stdlib's {}; \
             re-copy the catalog module",
            module.name
        );
    }
}

/// data-pipeline: a runnable record-processing pipeline. Since ADR-0009 landed
/// the allocator, the file carries the **growable-collection oracle** — it
/// `push`es the filtered subset (a data-dependent size a fixed `[Int; N]`
/// cannot express) into a heap-backed `Array[Int]` and folds it with the
/// **generic higher-order fold** (`fold(items, 0, add)`, `add` passed as a
/// function value — the ADR-0008 oracle). Since ADR-0012 landed the
/// element-generic array surface, `main` answers the query through the
/// **record-struct oracle**: the batch decodes into `Record` structs with
/// owned `String` labels held in an `Array[Record]`, filtered and folded via
/// the struct instantiation of the generic fold (`fold_records` over an
/// `in Record` function value) — so `run` exercises the owned-element
/// deep-copy `get`, the recursive drop glue, a native indirect call, and the
/// ADR-0010 `String`→`Str` composition (`label_is`/`label_len` over `as_str`).
/// Its specs pin every path equal to the streaming fold. `sum_records(2)` =
/// `sum_collected(2)` = `total_for(2)` = 400, exit = 400 & 0xff = 144
/// (unchanged).
#[test]
fn data_pipeline_checks_specs_and_runs() {
    let dir = example_dir("data-pipeline");
    let main = dir.join("src/main.tuo");
    assert_checks(&[&main]);
    assert_specs_green(&dir);
    assert_runs_to(&[&main], 144);
}

/// http-service: the pure parsing/routing core is spec-checked, and since
/// ADR-0006 the demo `main` runs natively — it parses "GET /health HTTP/1.1",
/// prints the response status line through its `std::rt::write` shell, and
/// exits with the routed status code (200).
#[test]
fn http_service_parses_routes_prints_and_runs() {
    let dir = example_dir("http-service");
    let main = dir.join("src/main.tuo");
    assert_checks(&[&main]);
    assert_specs_green(&dir);
    let out = assert_runs_to(&[&main], 200);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "HTTP/1.1 200 OK\n",
        "http-service must print its documented response line, byte for byte"
    );
}

/// concurrent-worker: since ADR-0007 the pool **runs live** — `main` computes
/// the makespan through the pure scheduling model AND through a real
/// `std::rt::par_map` fork-join (one OS thread per worker, each running the
/// model's own `worker_load` over its round-robin partition), and exits with
/// the model's answer (15) only when the live parallel run agrees. This
/// native run is exactly the ADR's "the deterministic scheduling model is
/// the oracle" constraint, executed: a scheduling bug in the primitive would
/// break the agreement and flip the exit to 0.
#[test]
fn concurrent_worker_scheduling_core_checks_specs_and_runs() {
    let dir = example_dir("concurrent-worker");
    let main = dir.join("src/main.tuo");
    assert_checks(&[&main]);
    assert_specs_green(&dir);
    assert_runs_to(&[&main], 15);
}

/// router: the declarative dispatch table. Its route registry is a runtime
/// `Map[Str, Int]` and its handlers are function values in a fixed
/// `[fn(in Request) -> Int; 4]` — the **split table** forced by ADR-0012 (a
/// growable `Array[Route]` holding a function-valued field is refused), which
/// is this example's documented finding. `main` resolves each path at runtime
/// and calls through the slot it lands on, so the native run exercises a real
/// indirect call (ADR-0008 Tier 1) that no pass can fold back into a direct
/// one. Everything is pure, so the whole router is spec-checked; exit = 74.
#[test]
fn router_dispatch_table_checks_specs_and_runs() {
    let dir = example_dir("router");
    let main = dir.join("src/main.tuo");
    assert_checks(&[&main]);
    assert_specs_green(&dir);
    assert_runs_to(&[&main], 74);
}

/// log-analytics: the keyed-aggregation tool. It scans a literal access log
/// byte by byte, builds its columns in growable `Array[Int]`s, and folds the
/// whole log into a `Map[Int, Int]` in ONE pass (ADR-0011) — pinned by its
/// specs against an independent per-status re-scan, so the map path and a
/// naive path must agree. Writing it surfaced a real backend boundary, which
/// the example documents in place rather than hiding: the Cranelift debug
/// backend does not lower `Rvalue::Len`, so neither `std::array::len(xs)` nor
/// `for x in xs` over a *growable* array compiles there (both work on LLVM and
/// in the interpreter), and the runtime paths therefore carry their lengths
/// explicitly. Exit = 42.
#[test]
fn log_analytics_checks_specs_and_runs() {
    let dir = example_dir("log-analytics");
    let main = dir.join("src/main.tuo");
    assert_checks(&[&main]);
    assert_specs_green(&dir);
    assert_runs_to(&[&main], 42);
}

/// log-analytics stays inside the subset **both** backends lower: the same
/// program must reach the same answer under the optimizing LLVM release
/// backend as under the Cranelift debug backend. This is the example's own
/// guard on the `Rvalue::Len` finding above — if a runtime path regained a
/// `for` over a growable array it would still pass `--release` while failing
/// the debug build, so both are asserted.
#[test]
fn log_analytics_runs_the_same_on_both_backends() {
    let main = example_dir("log-analytics").join("src/main.tuo");
    let out = tuo(&["run", "--release", &main.display().to_string()]);
    assert_eq!(
        out.status.code(),
        Some(42),
        "log-analytics under --release exited {:?}, expected 42\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr),
    );
}

/// file-report: the disk roundtrip. Its report *content* is rendered into an
/// owned `String` by pure, spec-checked code; its effect tier really writes
/// that file, reads it back a byte at a time, compares it byte-for-byte, and
/// removes it (ADR-0013's `open`/`close`/`remove_file` on the ADR-0006
/// descriptor seam). Effectful functions can carry no spec (`R0007`), so this
/// test is their pin: the program is run in a scratch directory, its stdout is
/// asserted byte-for-byte, and the exit byte is the number of report lines it
/// verified *through the disk* — reachable only if every I/O step succeeded.
#[test]
fn file_report_checks_specs_runs_and_roundtrips_a_real_file() {
    let dir = example_dir("file-report");
    let main = dir.join("src/main.tuo");
    assert_checks(&[&main]);
    assert_specs_green(&dir);

    // A scratch directory of its own: the program writes a file relative to
    // the working directory, so it must not run in the repository.
    let scratch = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("dogfood_examples")
        .join("file-report");
    std::fs::create_dir_all(&scratch).expect("scratch dir is creatable");

    let out = assert_runs_to_in(&[&main], 7, &scratch);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "samples 7\ntotal 8762\nmean 1251\npeak 1650\ntrough 875\nspread 775\nbusy 3\nverified 7\n",
        "file-report must print its documented report and verdict, byte for byte"
    );

    // The program removes its own scratch file, so running it leaves nothing
    // behind — the property that makes it safe to re-run in CI.
    assert!(
        !scratch.join("file-report-output.txt").exists(),
        "file-report left its scratch file behind; it must clean up after itself"
    );
}

/// postgres-auth: the PostgreSQL v3 authentication handshake, computed end to
/// end in tuonelang.
///
/// This is ADR-0019's motivating case as a whole program rather than one
/// assertion. The connector was the first dogfooding target the language could
/// not express *at all* — the wire protocol is big-endian length prefixes and
/// its authentication is rotations and xors, and v0 had neither operator. The
/// program frames a startup packet, parses the server's SASL challenge off the
/// wire format, derives the SCRAM-SHA-256 client proof, and verifies the
/// server's signature with the constant-time `std::crypto::verify`.
///
/// The exit byte is load-bearing: 4 of it counts SCRAM handshake steps that
/// each compare against **RFC 7677's published vector**, 40 counts a framing
/// round-trip, and 4 counts the legacy MD5 challenge against its own pinned
/// vector. A wrong proof, a misparsed attribute, or an off-by-one in the
/// self-counting length field all lower it, so 48 is reachable only if both
/// authentication paths and the framing are right. This caught a real bug while being written: the
/// `n=` username field was hardcoded empty, which is invisible to every
/// structural check but produces a proof the server rejects.
#[test]
fn postgres_auth_checks_specs_and_completes_the_rfc_7677_handshake() {
    let dir = example_dir("postgres-auth");
    let sources: Vec<PathBuf> = ["main", "std_bits", "std_ct", "std_crypto", "std_str"]
        .iter()
        .map(|name| dir.join(format!("src/{name}.tuo")))
        .collect();
    let refs: Vec<&Path> = sources.iter().map(PathBuf::as_path).collect();
    assert_checks(&refs);
    assert_specs_green(&dir);
    assert_runs_to(&refs, 48);
}

/// postgres-client: the connector talking to a **real PostgreSQL server** over
/// TCP — the other half of `postgres-auth`, which computes the same
/// authentication hermetically against RFC 7677's published vector.
///
/// Together they are the dogfooding target ADR-0019 was opened for. This one
/// connects, performs the SCRAM-SHA-256 exchange, verifies the server's
/// signature with the constant-time `std::crypto::verify` (ADR-0020), runs
/// `SELECT 42`, and decodes the answer out of a `DataRow` frame.
///
/// It then exercises the **extended query protocol**
/// (`Parse`/`Bind`/`Describe`/`Execute`/`Sync`) with the value sent as a
/// parameter rather than interpolated into SQL — including one containing SQL,
/// which must come back as literal text, proving the server treated it as data
/// and never parsed it. Finally it reads a three-column row's type OIDs off the
/// `RowDescription` and checks each against its type map, so decoding follows
/// what the server said rather than an assumption.
///
/// **This test skips when no server is reachable**, and that is a deliberate
/// trade rather than a hole. A developer without a local PostgreSQL must still
/// get a green suite, so the program returns 3 for "could not connect" and the
/// test treats only that one value as a skip. Every other outcome is a real
/// failure with a real diagnosis: the program encodes *which* step failed in
/// its exit byte (10 + the negative step number), so a broken proof reports 20
/// and a rejected server signature reports 21 rather than a bare mismatch.
///
/// The protocol layer is pure and spec-checked regardless of whether a server
/// exists, so `check` and `test` below always run; only the live exchange is
/// conditional.
///
/// To run the live path locally:
/// ```text
/// initdb -D /tmp/tuopg/data -U tuo_admin --auth-host=scram-sha-256 --pwfile=<(echo adminpw)
/// pg_ctl -D /tmp/tuopg/data -o "-p 55432 -k /tmp/tuopg" -l /tmp/tuopg/log start
/// PGPASSWORD=adminpw psql -h 127.0.0.1 -p 55432 -U tuo_admin -d postgres \
///   -c "CREATE ROLE tuo_test LOGIN PASSWORD 'tuo_secret';" \
///   -c "CREATE DATABASE tuo_testdb OWNER tuo_test;"
/// ```
#[test]
#[expect(
    clippy::print_stderr,
    reason = "diagnostic note when a machine has no PostgreSQL; keeps the test green there"
)]
fn postgres_client_checks_specs_and_authenticates_against_a_live_server() {
    let dir = example_dir("postgres-client");
    let sources: Vec<PathBuf> = ["main", "std_bits", "std_ct", "std_crypto", "std_str"]
        .iter()
        .map(|name| dir.join(format!("src/{name}.tuo")))
        .collect();
    let refs: Vec<&Path> = sources.iter().map(PathBuf::as_path).collect();

    // The protocol layer is pure, so these hold with or without a server.
    assert_checks(&refs);
    assert_specs_green(&dir);

    let mut args = vec!["run".to_string()];
    args.extend(refs.iter().map(|s| s.display().to_string()));
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = tuo(&arg_refs);
    match out.status.code() {
        Some(42) => {}
        Some(3) => eprintln!(
            "note: no PostgreSQL reachable on 127.0.0.1:55432;              the live SCRAM exchange was skipped (see this test's doc comment              for how to start one)"
        ),
        other => panic!(
            "postgres-client reached a live server but failed at step {}: exit {other:?} (20 = the client's proof was rejected, 21 = the server's signature failed verification, 5 = the simple query returned the wrong answer, 6 = the bound parameter did not round-trip, 7 = a parameter containing SQL was not returned as literal text, 8 = a column's type OID was not recognized from the RowDescription); stderr:\n{}",
            other.map_or(-1, |c| c - 10),
            String::from_utf8_lossy(&out.stderr),
        ),
    }
}

/// postgres-client vendors the same four catalog modules as postgres-auth, and
/// each must equal the `tuo-stdlib` original byte-for-byte.
#[test]
fn postgres_client_vendored_modules_match_the_catalog() {
    let dir = example_dir("postgres-client");
    for (file, module) in [
        ("std_bits.tuo", tuo_stdlib::BITS),
        ("std_ct.tuo", tuo_stdlib::CT),
        ("std_crypto.tuo", tuo_stdlib::CRYPTO),
        ("std_str.tuo", tuo_stdlib::STR),
    ] {
        let vendored = dir.join("src").join(file);
        let on_disk = std::fs::read_to_string(&vendored)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", vendored.display()));
        assert_eq!(
            on_disk, module.source,
            "examples/postgres-client/src/{file} drifted from tuo-stdlib's {}; \
             re-copy the catalog module",
            module.name
        );
    }
}

/// The medium multi-package workspace: `app → geometry → numeric`. Every
/// package's specs are green, and the whole-graph binary builds and executes to
/// the documented exit 26. Built (not `run`) because `tuo run` is file-based.
#[test]
fn workspace_graph_checks_specs_and_builds_to_a_runnable_binary() {
    let numeric = example_dir("workspace/numeric");
    let geometry = example_dir("workspace/geometry");
    let app = example_dir("workspace/app");

    // Each package checks and its specs pass (app's run covers the graph).
    for pkg in [&numeric, &geometry, &app] {
        let out = tuo(&["check", "--manifest", &pkg.display().to_string()]);
        expect_ok(&out, &format!("tuo check --manifest {}", pkg.display()));
    }
    assert_specs_green(&app);

    // Build the whole-graph binary and execute it: it must exit 26.
    let out_bin = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("dogfood_examples")
        .join("workspace-app");
    if let Some(parent) = out_bin.parent() {
        std::fs::create_dir_all(parent).expect("scratch dir is creatable");
    }
    let build = tuo(&[
        "build",
        "--manifest",
        &app.display().to_string(),
        "-o",
        &out_bin.display().to_string(),
    ]);
    expect_ok(&build, "tuo build --manifest workspace/app");

    let run = Command::new(&out_bin)
        .output()
        .expect("the built workspace binary runs");
    assert_eq!(
        run.status.code(),
        Some(26),
        "the workspace binary exited {:?}, expected 26",
        run.status.code(),
    );
}

/// gguf-reader: the GGUF container format's header, metadata table, and tensor
/// descriptors — the structural walk a model loader performs before it touches
/// a single weight.
///
/// This example was written to answer a question empirically rather than to
/// exercise a feature: every other wire-format example in this tree is
/// **big-endian** (PostgreSQL's frame length, the SHA-256 block, the
/// `wire-decode` workload), because ADR-0019 Stage A was opened to serve a
/// network protocol. GGUF is the opposite case — a **little-endian, 64-bit,
/// memory-mappable file format** — so it tests whether that operator surface
/// generalizes past the protocol it was opened for, or had been fitted to it.
/// It generalizes: `le16_at`/`le32_at`/`le64_at` are written in the shipped
/// `<<`/`>>`/`&`/`|` with no new language feature, which is why this example
/// produced **no ADR**.
///
/// It did find one real boundary, and the specs pin it as a *documented limit*
/// rather than letting it become silent corruption: GGUF's counts, lengths and
/// offsets are `u64` while `Int` is signed `I64`, so a value with bit 63 set
/// has no representation. `le64_at` refuses such a value with `-1` instead of
/// wrapping it to a negative length that a caller would then use as an array
/// bound. Every value below 2^63 — i.e. every real model file, 2^63 bytes being
/// 9.2 exabytes — round-trips exactly.
///
/// The parser is pure and spec-checked, so the program is hermetic: it parses a
/// GGUF file it builds byte-by-byte from the format's own encoding rules, and
/// its exit byte is the number of tensor descriptors whose name, shape, and
/// file-absolute data offset were all recovered — 2 for the fixture.
#[test]
fn gguf_reader_checks_specs_and_walks_a_container() {
    let dir = example_dir("gguf-reader");
    let sources: Vec<PathBuf> = ["main", "std_str"]
        .iter()
        .map(|name| dir.join(format!("src/{name}.tuo")))
        .collect();
    let refs: Vec<&Path> = sources.iter().map(PathBuf::as_path).collect();
    assert_checks(&refs);
    assert_specs_green(&dir);
    assert_runs_to(&refs, 2);
}

/// gguf-reader vendors the catalog's `std::str`, which must equal the
/// `tuo-stdlib` original byte-for-byte.
#[test]
fn gguf_reader_vendored_module_matches_the_catalog() {
    let vendored = example_dir("gguf-reader").join("src/std_str.tuo");
    let on_disk = std::fs::read_to_string(&vendored)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", vendored.display()));
    assert_eq!(
        on_disk,
        tuo_stdlib::STR.source,
        "examples/gguf-reader/src/std_str.tuo drifted from tuo-stdlib's std/str.tuo; \
         re-copy the catalog module"
    );
}

/// The fixture's byte layout is cross-checked against an **independent**
/// implementation of the GGUF v3 spec (a short Python script using `struct`,
/// recorded in the example's README workflow), so the example cannot be
/// self-consistently wrong: its builder and its parser could share a bug, but
/// they cannot also share one with a third implementation written from the
/// format spec alone.
///
/// The independent implementation gives: descriptor table ends at byte 209,
/// the data blob begins at 224 (209 rounded up to the declared alignment of
/// 32), tensor 0 sits at 224, tensor 1 at 736 (224 + its blob-relative 512),
/// and tensor 0 has 16x8 = 128 elements. Those exact numbers are asserted here
/// through the real compiler, which pins the alignment rule and the
/// blob-relative-to-file-absolute composition — the format's two classic bug
/// sources — against an outside authority rather than against this example's
/// own reasoning.
#[test]
fn gguf_reader_agrees_with_an_independent_implementation() {
    let dir = example_dir("gguf-reader");
    let probe = dir.join("src/cross_check.tuo");
    let main = dir.join("src/main.tuo");
    let std_str = dir.join("src/std_str.tuo");
    assert_runs_to(&[&probe, &main, &std_str], 100);
}

/// The whole point of this example is bit manipulation, which is exactly where
/// two backends are most likely to disagree — shift semantics, masking, and the
/// 64-bit assembly in `le64_at` are all places an optimizer can differ from a
/// straightforward lowering. So the same container walk is asserted to reach
/// the same answer under the optimizing LLVM release backend as under the
/// Cranelift debug backend.
#[test]
fn gguf_reader_runs_the_same_on_both_backends() {
    let dir = example_dir("gguf-reader");
    let main = dir.join("src/main.tuo");
    let std_str = dir.join("src/std_str.tuo");
    let out = tuo(&[
        "run",
        "--release",
        &main.display().to_string(),
        &std_str.display().to_string(),
    ]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "gguf-reader under --release exited {:?}, expected 2\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr),
    );
}
