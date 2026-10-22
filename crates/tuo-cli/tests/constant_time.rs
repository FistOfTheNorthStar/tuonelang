//! ADR-0020 Stage B: the `std::ct` primitives must still be **branchless in
//! the emitted machine code**.
//!
//! This is the load-bearing artifact of ADR-0020, and it exists because of a
//! real observation rather than a hypothetical one. Constant-time-ness is not
//! a property of a program's meaning — it is a property of the instructions a
//! compiler emits — so every stage between tuonelang source and machine code
//! is free to destroy it while preserving semantics. During ADR-0020's
//! investigation exactly that happened: a branchless `select` written as
//! `(a & m) | (b & ~m)` compiled under Cranelift to `and`/`bic`/`orr`, and
//! under LLVM at `O2` to a `csel` — the optimizer recognized the masking idiom
//! and rewrote it into a conditional. That particular rewrite is benign on
//! current ARM64 cores, where `csel` is itself constant time, but nothing
//! *asked* the optimizer to preserve the property, and nothing would have
//! noticed had it chosen a real branch instead.
//!
//! So the property is tested where it actually lives: each primitive is
//! compiled to a native binary through the real compiler, the emitted function
//! is disassembled, and its instruction stream is inspected. A future MIR
//! pass, LLVM upgrade, or backend change that turns one of these into a branch
//! fails this test, which is the entire point — `std::ct` claims only that it
//! is branchless *as emitted today*, and this is what stops that claim from
//! going stale silently.
//!
//! What this test does **not** prove, and the module documentation says so
//! too: that the emitted instructions execute in data-independent time on a
//! given microarchitecture. That is a hardware property, beyond a compiler's
//! reach and beyond this test's. A verified guarantee needs the marking
//! ADR-0020 defers to its Stage C.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A fresh scratch directory for one test.
///
/// Per test, not per file: cargo runs a file's tests in parallel threads, so a
/// shared directory has one thread deleting it while another writes into it.
fn scratch_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("constant_time")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch workspace is creatable");
    dir
}

/// The `std::ct` scalar primitives, in **declaration order**, which is how
/// they are matched to emitted symbols below.
///
/// The array-scanning functions (`select_array`, `bytes_eq`) are deliberately
/// absent: they contain loops, which are backward branches by construction,
/// and tuonelang's bounds checks add more. Their weaker — and true — property
/// is covered by `array_scans_*` below.
const BRANCH_FREE: &[&str] = &[
    "mask",
    "select",
    "nonzero",
    "is_zero",
    "eq",
    "ne",
    "is_negative",
    "lt",
    "gt",
];

/// Write `std::ct` into `dir`, plus a `main` calling each given expression.
///
/// The calls barely affect what ends up in the binary: both backends keep the
/// standalone body of every `#[constant_time]` function and of every function
/// `main` cannot reach (the LLVM backend's `keep_inspectable_bodies`), so the
/// whole module stays inspectable. A driver is still needed to link.
fn write_program(dir: &Path, calls: &[&str]) -> Vec<PathBuf> {
    // `std::ct` is self-contained, so this is the whole library it needs.
    let mut sources = Vec::new();
    let module = tuo_stdlib::module("std::ct").expect("std::ct is a catalog module");
    let file = dir.join(module.name.replace('/', "_"));
    std::fs::write(&file, module.source).expect("write std::ct");
    sources.push(file);

    let body = calls
        .iter()
        .map(|call| format!("    total = total + ({call});"))
        .collect::<Vec<_>>()
        .join("\n");
    let driver = format!("fn main() -> Int {{\n    var total = 0;\n{body}\n    total & 1\n}}\n");
    let file = dir.join("driver.tuo");
    std::fs::write(&file, driver).expect("write the driver");
    sources.push(file);
    sources
}

/// Build `sources` into a native binary at `out`, debug (Cranelift) or release
/// (LLVM).
fn build(sources: &[PathBuf], out: &Path, release: bool) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tuo"));
    command.arg("build").arg("-o").arg(out);
    if release {
        command.arg("--release");
    }
    let output = command
        .args(sources)
        .output()
        .expect("the compiler runs as a subprocess");
    assert!(
        output.status.success(),
        "building the constant-time probe failed ({}):\nstdout: {}\nstderr: {}",
        if release { "release" } else { "debug" },
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Disassemble `binary`, or `None` when no disassembler is available.
///
/// A missing `objdump` is a skip rather than a failure: the property is real
/// but the tool is not part of the toolchain the workspace pins, and the same
/// discipline the performance lab uses for its C peers applies — record the
/// skip, never fabricate a pass.
fn disassemble(binary: &Path) -> Option<String> {
    for tool in ["objdump", "llvm-objdump", "/usr/bin/objdump"] {
        let Ok(output) = Command::new(tool).arg("-d").arg(binary).output() else {
            continue;
        };
        if output.status.success() {
            return Some(String::from_utf8_lossy(&output.stdout).into_owned());
        }
    }
    None
}

/// Every generated tuonelang function in the binary, as
/// `(symbol, instruction lines)`.
///
/// Generated functions are named `tuo_fn_<n>`, so this ignores the linked C
/// runtime, which contains ordinary branching code and is not under test.
fn tuonelang_functions(disassembly: &str) -> Vec<(String, Vec<String>)> {
    let mut functions = Vec::new();
    let mut current: Option<(String, Vec<String>)> = None;
    for line in disassembly.lines() {
        if let Some(open) = line.find('<')
            && line.trim_end().ends_with(">:")
        {
            let symbol = line[open + 1..line.rfind('>').expect("a closing bracket")].to_string();
            if let Some(function) = current.take() {
                functions.push(function);
            }
            current = symbol
                .trim_start_matches('_')
                .starts_with("tuo_fn_")
                .then_some((symbol, Vec::new()));
            continue;
        }
        if let Some((_, body)) = current.as_mut()
            && line.contains('\t')
        {
            body.push(line.to_string());
        }
    }
    if let Some(function) = current.take() {
        functions.push(function);
    }
    functions
}

/// The mnemonic of one disassembled instruction line.
///
/// The two disassemblers lay a line out differently. LLVM's `objdump` (macOS)
/// writes `<address>: <encoding>\t<mnemonic>\t<operands>`, with the address
/// and encoding in one tab-separated field; GNU `objdump` (Linux) writes
/// `<address>:\t<encoding>\t<mnemonic> <operands>`, with the encoding in a
/// field of its own and the operands space-separated. So the mnemonic is the
/// first word of the first field that is neither the address nor raw
/// instruction bytes. Taking the wrong field silently defeats every assertion
/// built on it — the mnemonic becomes hex, which never looks like a branch, so
/// every branch-freedom check passes having checked nothing (which is what
/// happened on Linux until this parser learned GNU's layout) — so
/// `mnemonic_parsing_finds_real_branches` pins it against both.
fn mnemonic(line: &str) -> String {
    let is_encoding = |field: &str| {
        field.split_whitespace().all(|token| {
            matches!(token.len(), 2 | 4 | 8) && token.chars().all(|c| c.is_ascii_hexdigit())
        })
    };
    line.split('\t')
        .map(str::trim)
        .filter(|field| !field.is_empty())
        .filter(|field| !field.ends_with(':') && !is_encoding(field))
        .find(|field| !field.contains(": "))
        .unwrap_or("")
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_lowercase()
}

/// Is this instruction a conditional branch — a jump whose target depends on a
/// comparison?
///
/// Covers both architectures the workspace can host: ARM64 (`b.<cond>`,
/// `cbz`/`cbnz`, `tbz`/`tbnz`) and x86-64 (`j<cc>`, but not `jmp`). An
/// unconditional branch is fine: a `ret` or a tail jump reveals nothing about
/// the data.
fn is_conditional_branch(mnemonic: &str) -> bool {
    if matches!(mnemonic, "jmp" | "b" | "bl" | "br" | "blr" | "ret") {
        return false;
    }
    if mnemonic.starts_with("b.") || matches!(mnemonic, "cbz" | "cbnz" | "tbz" | "tbnz") {
        return true;
    }
    mnemonic.len() >= 2
        && mnemonic.starts_with('j')
        && mnemonic.chars().all(|c| c.is_ascii_alphabetic())
}

/// Does this function's body reach a trap?
///
/// Both backends set a trap code and call the runtime handler — Cranelift
/// indirectly through a register (`blr`), LLVM directly (`bl <_tuo_rt_trap>`)
/// — so the handler symbol, or an indirect call in a module that makes none of
/// its own, marks a trap edge.
fn reaches_a_trap(body: &[String]) -> bool {
    body.iter()
        .any(|line| line.contains("tuo_rt_trap") || mnemonic(line) == "blr")
}

/// Build the primitives on one backend and check their emitted control flow.
///
/// `strict` is the real distinction between the two backends, and it follows
/// from what each is for rather than from convenience:
///
/// * **LLVM (release)** is held to the strict rule — not one conditional
///   branch and not one trap edge in any primitive. This is the build a
///   release ships, so this is the guarantee that matters.
/// * **Cranelift (debug)** is checked for branches that are *not* trap guards.
///   It is deliberately an unoptimizing backend — the project's stated
///   position is that "the Cranelift backend emits unoptimized code" — so it
///   keeps every check tuonelang's semantics imply even when the operand is a
///   compile-time constant. `mask`'s `(bit << 63) >> 63` arrives with four
///   ADR-0019 shift-amount comparisons against the literal 63, and `lt`'s
///   subtraction keeps an overflow check that is provably unreachable. Those
///   branches test constants, not data, so they leak nothing; demanding their
///   absence would be demanding optimization from a backend that states it
///   does not optimize.
///
/// The relaxation is narrow and checked rather than waved through: a branch is
/// tolerated only in a function that actually reaches a trap. A data-dependent
/// branch in a trap-free function — the regression this file exists to catch —
/// fails on both backends.
#[expect(
    clippy::print_stderr,
    reason = "records a skip when the host has no disassembler; a silent pass would \
claim a property that was never checked"
)]
fn assert_branch_free(release: bool, strict: bool) {
    let label = if release { "release" } else { "debug" };
    let dir = scratch_dir(&format!("branch_free_{label}"));
    let sources = write_program(&dir, &["std::ct::select(1, 10, 20)"]);
    let binary = dir.join("probe");
    build(&sources, &binary, release);

    let Some(disassembly) = disassemble(&binary) else {
        eprintln!(
            "SKIPPED ({label}): no objdump on this host, so the emitted code could not be \
             inspected. The branchlessness property is untested here rather than confirmed."
        );
        return;
    };

    // The compiler keeps every primitive's standalone body (each is
    // `#[constant_time]`), so the primitives are picked out by symbol: `tuo_fn_<n>` numbers follow
    // declaration order, and `std::ct` declares its scalar primitives first.
    // `emitted_symbols_follow_declaration_order` pins that assumption.
    let functions = tuonelang_functions(&disassembly);
    assert!(
        functions.len() > BRANCH_FREE.len(),
        "expected the {} scalar primitives plus the scans in the disassembly, found {} \
         functions — the test would be vacuous",
        BRANCH_FREE.len(),
        functions.len()
    );

    for ((symbol, body), name) in functions.iter().zip(BRANCH_FREE) {
        let trap_guarded = !strict && reaches_a_trap(body);
        for line in body {
            let mnemonic = mnemonic(line);
            if !is_conditional_branch(&mnemonic) {
                continue;
            }
            assert!(
                trap_guarded,
                "{label}: `{symbol}` (std::ct::{name}) contains the conditional branch \
                 `{mnemonic}`, so a std::ct primitive is no longer branchless as \
                 emitted:\n  {}\n\nThis is the regression ADR-0020 Stage B exists to catch. \
                 Either the source stopped being branchless, or an optimizer rewrote it into a \
                 conditional.",
                line.trim()
            );
        }
        assert!(
            !strict || !reaches_a_trap(body),
            "{label}: `{symbol}` (std::ct::{name}) reaches a trap, so it can abort on some \
             input — and a trap check is a conditional branch on the data."
        );
    }
}

/// The primitives are branchless as emitted by the **release** backend (LLVM
/// at `O2`).
///
/// This is the half that matters most, and the strict one. LLVM is the
/// optimizer with both the license and the inclination to rewrite a masking
/// idiom into a conditional — it was observed doing exactly that during
/// ADR-0020's investigation — so a silent regression would appear here first.
#[test]
fn ct_primitives_are_branch_free_under_llvm() {
    assert_branch_free(true, true);
}

/// The primitives carry no data-dependent branch under the **debug** backend
/// (Cranelift).
///
/// Weaker than the release check by design: Cranelift does not optimize, so it
/// keeps constant-operand trap guards that LLVM folds away. What must hold on
/// both is that no branch depends on the data.
#[test]
fn ct_primitives_have_no_data_dependent_branch_under_cranelift() {
    assert_branch_free(false, false);
}

/// ADR-0031's five `std::int` operations — the builtins a `#[constant_time]`
/// function may call — compile to no conditional branch and no trap edge, on
/// **both** backends, strictly: unlike `+`, none of them has a trap check for
/// even an unoptimizing backend to keep. Each sits alone in a marked function
/// over parameters, so no operand is a constant either engine could fold.
#[expect(
    clippy::print_stderr,
    reason = "records a skip when the host has no disassembler; a silent pass would \
claim a property that was never checked"
)]
#[test]
fn the_integer_builtins_compile_without_branches_on_both_backends() {
    const OPS: [(&str, &str); 5] = [
        ("wrapping_add", "U64"),
        ("wrapping_sub", "U64"),
        ("wrapping_mul", "U64"),
        ("rotate_left", "Int"),
        ("rotate_right", "Int"),
    ];
    for release in [true, false] {
        let label = if release { "release" } else { "debug" };
        let dir = scratch_dir(&format!("int_builtins_{label}"));
        let mut program = String::new();
        for (op, amount) in OPS {
            program.push_str(&format!(
                "#[constant_time]\nfn {op}_ct(take x: U64, take n: {amount}) -> U64 {{\n    \
                 std::int::{op}(x, n)\n}}\n\n"
            ));
        }
        program.push_str(
            "fn main() -> Int {\n    let x: U64 = 3;\n    (wrapping_add_ct(x, 4) as Int) & 1\n}\n",
        );
        let source = dir.join("int_ops.tuo");
        std::fs::write(&source, program).expect("write the probe");
        let binary = dir.join("probe");
        build(&[source], &binary, release);
        let Some(disassembly) = disassemble(&binary) else {
            eprintln!("SKIPPED ({label}): no objdump on this host; the property is untested.");
            return;
        };
        // Symbols follow declaration order, so the five marked functions are
        // the first five generated ones (`main`, declared last, may be
        // inlined into the entry shim).
        let functions = tuonelang_functions(&disassembly);
        assert!(
            functions.len() >= OPS.len(),
            "{label}: expected the five probes, found {} functions",
            functions.len()
        );
        for ((symbol, body), (op, _)) in functions.iter().zip(OPS) {
            for line in body {
                assert!(
                    !is_conditional_branch(&mnemonic(line)),
                    "{label}: `{symbol}` (std::int::{op}) contains a conditional branch:\n  {}",
                    line.trim()
                );
            }
            assert!(
                !reaches_a_trap(body),
                "{label}: `{symbol}` (std::int::{op}) reaches a trap, which it never may"
            );
        }
    }
}

/// `std::fe25519`'s marked arithmetic — the field X25519's secret scalar is
/// multiplied in (ADR-0022) — is branch-free as emitted, held to the same two
/// rules as `std::ct`'s primitives: strictly on LLVM, and on Cranelift with no
/// branch except a trap guard.
///
/// The module is written first so its functions take the first `tuo_fn_<n>`
/// symbols, in declaration order; the marked ones are declared before the
/// first unmarked one (`square_times`), so they are exactly the leading
/// `FE_MARKED.len()` symbols.
#[expect(
    clippy::print_stderr,
    reason = "records a skip when the host has no disassembler; a silent pass would \
claim a property that was never checked"
)]
#[test]
fn field_arithmetic_is_branch_free_as_emitted() {
    const FE_MARKED: [&str; 11] = [
        "zero",
        "one",
        "from_int",
        "add",
        "sub",
        "carry",
        "mul",
        "square",
        "mul_small",
        "select",
        "freeze",
    ];
    let fe = tuo_stdlib::module("std::fe25519").expect("std::fe25519 is a catalog module");
    let declared: Vec<&str> = fe
        .source
        .lines()
        .filter_map(|line| {
            let line = line.strip_prefix("pub ").unwrap_or(line);
            let name = line.strip_prefix("fn ")?;
            Some(&name[..name.find('(')?])
        })
        .collect();
    assert_eq!(
        &declared[..FE_MARKED.len()],
        FE_MARKED,
        "the marked functions must lead the module, in this order"
    );
    for (release, strict) in [(true, true), (false, false)] {
        let label = if release { "release" } else { "debug" };
        let dir = scratch_dir(&format!("fe25519_{label}"));
        let fe_file = dir.join("std_fe25519.tuo");
        std::fs::write(&fe_file, fe.source).expect("write std::fe25519");
        let ct = tuo_stdlib::module("std::ct").expect("std::ct is a catalog module");
        let ct_file = dir.join("std_ct.tuo");
        std::fs::write(&ct_file, ct.source).expect("write std::ct");
        let driver = dir.join("driver.tuo");
        std::fs::write(
            &driver,
            "fn main() -> Int {\n    let x = std::fe25519::mul(std::fe25519::from_int(3), \
             std::fe25519::one());\n    std::array::get(std::fe25519::to_bytes(x), 0)\n}\n",
        )
        .expect("write the driver");
        let binary = dir.join("probe");
        build(&[fe_file, ct_file, driver], &binary, release);
        let Some(disassembly) = disassemble(&binary) else {
            eprintln!("SKIPPED ({label}): no objdump on this host; the property is untested.");
            return;
        };
        let functions = tuonelang_functions(&disassembly);
        assert!(
            functions.len() > FE_MARKED.len(),
            "{label}: expected the field functions in the disassembly, found {}",
            functions.len()
        );
        for ((symbol, body), name) in functions.iter().zip(FE_MARKED) {
            let trap_guarded = !strict && reaches_a_trap(body);
            for line in body {
                let mnemonic = mnemonic(line);
                assert!(
                    !is_conditional_branch(&mnemonic) || trap_guarded,
                    "{label}: `{symbol}` (std::fe25519::{name}) contains the conditional \
                     branch `{mnemonic}`:\n  {}",
                    line.trim()
                );
            }
            assert!(
                !strict || !reaches_a_trap(body),
                "{label}: `{symbol}` (std::fe25519::{name}) reaches a trap"
            );
        }
    }
}

/// The source text of the top-level item that starts at the line `head`
/// (and any `#[...]` line just above it), through its closing `}` in
/// column 0.
fn item_text<'a>(source: &'a str, head: &str) -> &'a str {
    let start = source
        .find(&format!("\n{head}"))
        .unwrap_or_else(|| panic!("`{head}` is declared"))
        + 1;
    let start = source[..start - 1]
        .rfind('\n')
        .filter(|&line| source[line + 1..start - 1].starts_with("#["))
        .map_or(start, |line| line + 1);
    let end = start
        + source[start..]
            .find("\n}\n")
            .unwrap_or_else(|| panic!("`{head}` is closed"))
        + 3;
    &source[start..end]
}

/// Compile a cut-down copy of the catalog module `module` holding only the
/// `structs` and the `marked` functions — each item's text copied verbatim,
/// and each function checked to carry `#[constant_time]` — together with
/// `dependencies` and `driver`, on both backends, and hold the marked
/// functions' emitted code to the same two rules as `std::fe25519`'s:
/// strictly branch- and trap-free on LLVM, and on Cranelift free of every
/// branch but a trap guard.
///
/// The cut-down module is written first, so its functions take the leading
/// `tuo_fn_<n>` symbols in declaration order; that is what lets a module that
/// interleaves marked and unmarked functions be checked at all.
#[expect(
    clippy::print_stderr,
    reason = "records a skip when the host has no disassembler; a silent pass would \
claim a property that was never checked"
)]
fn assert_marked_items_branch_free(
    module: &str,
    structs: &[&str],
    marked: &[&str],
    dependencies: &[&str],
    driver: &str,
) {
    let catalog = tuo_stdlib::module(module).expect("a catalog module");
    let mut probe = format!("module {module};\n\n");
    for name in structs {
        probe.push_str(item_text(catalog.source, &format!("pub struct {name} {{")));
        probe.push('\n');
    }
    for name in marked {
        let public = format!("pub fn {name}(");
        let head = if catalog.source.contains(&format!("\n{public}")) {
            public
        } else {
            format!("fn {name}(")
        };
        let text = item_text(catalog.source, &head);
        assert!(
            text.starts_with("#[constant_time]\n"),
            "{module}::{name} must carry #[constant_time]"
        );
        probe.push_str(text);
        probe.push('\n');
    }
    let slug = module.replace("::", "_");
    for (release, strict) in [(true, true), (false, false)] {
        let label = if release { "release" } else { "debug" };
        let dir = scratch_dir(&format!("{slug}_{label}"));
        let probe_file = dir.join(format!("{slug}.tuo"));
        std::fs::write(&probe_file, &probe).expect("write the probe module");
        let mut files = vec![probe_file];
        for dependency in dependencies {
            let source = tuo_stdlib::module(dependency)
                .expect("a catalog module")
                .source;
            let file = dir.join(format!("{}.tuo", dependency.replace("::", "_")));
            std::fs::write(&file, source).expect("write a dependency");
            files.push(file);
        }
        let driver_file = dir.join("driver.tuo");
        std::fs::write(&driver_file, driver).expect("write the driver");
        files.push(driver_file);
        let binary = dir.join("probe");
        build(&files, &binary, release);
        let Some(disassembly) = disassemble(&binary) else {
            eprintln!("SKIPPED ({label}): no objdump on this host; the property is untested.");
            return;
        };
        let functions = tuonelang_functions(&disassembly);
        assert!(
            functions.len() > marked.len(),
            "{label}: expected the {module} functions in the disassembly, found {}",
            functions.len()
        );
        for ((symbol, body), name) in functions.iter().zip(marked) {
            let trap_guarded = !strict && reaches_a_trap(body);
            for line in body {
                let mnemonic = mnemonic(line);
                assert!(
                    !is_conditional_branch(&mnemonic) || trap_guarded,
                    "{label}: `{symbol}` ({module}::{name}) contains the conditional \
                     branch `{mnemonic}`:\n  {}",
                    line.trim()
                );
            }
            assert!(
                !strict || !reaches_a_trap(body),
                "{label}: `{symbol}` ({module}::{name}) reaches a trap"
            );
        }
    }
}

/// `std::ed25519`'s marked signing arithmetic — the Edwards point operations
/// and the scalar reduction modulo L that the secret key and nonce pass
/// through (ADR-0022) — is branch-free as emitted.
#[test]
fn signing_arithmetic_is_branch_free_as_emitted() {
    assert_marked_items_branch_free(
        "std::ed25519",
        &["CtPoint", "Scalar", "WideScalar"],
        &[
            "ct_identity",
            "ct_base_point",
            "two_d",
            "ct_add",
            "ct_double",
            "ct_select",
            "ct_step",
            "reduce_wide",
            "mul_add",
        ],
        &["std::fe25519", "std::ct"],
        "fn main() -> Int {\n    let p = std::ed25519::ct_step(std::ed25519::ct_identity(), 1);\n    \
         let s = std::ed25519::mul_add(scalar(), scalar(), scalar());\n    \
         std::array::get(std::fe25519::to_bytes(p.y), 0) + s.s0\n}\n\n\
         fn scalar() -> std::ed25519::Scalar {\n    std::ed25519::Scalar { s0: 7, s1: 0, \
         s2: 0, s3: 0, s4: 0, s5: 0, s6: 0, s7: 0, s8: 0, s9: 0, s10: 0, s11: 0 }\n}\n",
    );
}

/// `std::chacha`'s Poly1305 block step and final reduction — the arithmetic
/// over the per-record one-time key and the plaintext (ADR-0022) — are
/// branch-free as emitted.
#[test]
fn poly1305_arithmetic_is_branch_free_as_emitted() {
    assert_marked_items_branch_free(
        "std::chacha",
        &["Poly"],
        &["poly_zero", "poly_step", "poly_finish"],
        &["std::ct"],
        "fn main() -> Int {\n    let r = std::chacha::Poly { l0: 3, l1: 0, l2: 0, l3: 0, \
         l4: 0 };\n    let h = std::chacha::poly_step(std::chacha::poly_zero(), r, r);\n    \
         std::chacha::poly_finish(h, r).l0\n}\n",
    );
}

/// The array scans agree with their specification on early-differing,
/// late-differing, and equal inputs.
///
/// `bytes_eq` has no `return` inside its loop — it accumulates into `diff` —
/// so an input differing in the first element is examined just as fully as one
/// differing in the last. That structure is the property; this test pins the
/// observable half of it.
#[test]
fn array_scans_do_not_branch_on_element_values() {
    let dir = scratch_dir("array_scans");
    let program = "\
fn main() -> Int {
    var out = 0;
    let early = std::ct::bytes_eq(std::ct::of3(9, 2, 3), std::ct::of3(1, 2, 3));
    let late = std::ct::bytes_eq(std::ct::of3(1, 2, 9), std::ct::of3(1, 2, 3));
    let same = std::ct::bytes_eq(std::ct::of3(1, 2, 3), std::ct::of3(1, 2, 3));
    if early == 0 {
        if late == 0 {
            if same == 1 {
                out = 42;
            }
        }
    }
    out
}
";
    let mut sources = write_program(&dir, &["0"]);
    let driver = sources.pop().expect("the driver is last");
    std::fs::write(&driver, program).expect("overwrite the driver");
    sources.push(driver);

    let binary = dir.join("scans");
    build(&sources, &binary, true);
    let status = Command::new(&binary)
        .status()
        .expect("the scan probe runs as a subprocess");
    assert_eq!(
        status.code(),
        Some(42),
        "bytes_eq disagreed with its specification on early-differing, late-differing, or \
         equal inputs"
    );
}

/// Whether a canonical loop keeps its bounds check is the backend's call, not
/// the module's.
///
/// This pins the *scope* of the limitation `std::ct` documents. The scans
/// cannot be marked `#[constant_time]` because they index (`T0018` is a rule
/// about the source), and their claim is the weaker one — control flow depends
/// only on array lengths — which holds whether or not a bounds check survives.
/// What survives is a property of the compiler: the unoptimizing Cranelift
/// build keeps every check, while the LLVM release build, since borrowed
/// parameters are `noalias`, can prove `i < len(xs)` keeps the index in range
/// and drops the check.
///
/// The probe is built so nothing else can explain the result: `total` is
/// never called, so its standalone body is kept and optimized with its array
/// an unknown parameter (nothing to constant-fold, nothing inlined away), and
/// the fold is `^`, which cannot trap — so the only possible trap is the
/// bounds check. If the release build ever keeps the check again, or the debug
/// build loses it, the documentation in `std::ct` and ADR-0020 is out of date.
#[test]
#[expect(
    clippy::print_stderr,
    reason = "records a skip when the host has no disassembler; a silent pass would \
claim a property that was never checked"
)]
fn canonical_loop_bounds_check_survives_only_the_unoptimizing_backend() {
    let program = "\
fn total(in xs: Array[Int]) -> Int {
    var t = 0;
    var i = 0;
    while i < std::array::len(xs) {
        t = t ^ std::array::get(xs, i);
        i = i + 1;
    }
    t
}

fn main() -> Int {
    0
}
";
    for release in [false, true] {
        let label = if release { "release" } else { "debug" };
        let dir = scratch_dir(&format!("canonical_loop_{label}"));
        let file = dir.join("canonical.tuo");
        std::fs::write(&file, program).expect("write the canonical-loop probe");
        let binary = dir.join("canonical");
        build(&[file], &binary, release);

        let Some(disassembly) = disassemble(&binary) else {
            eprintln!("SKIPPED: no objdump on this host.");
            return;
        };
        // `total` is declared first, so it is the first generated function.
        let functions = tuonelang_functions(&disassembly);
        let (symbol, body) = functions.first().expect("`total` is emitted");
        if release {
            assert!(
                !reaches_a_trap(body),
                "{label}: `{symbol}` (total) keeps its bounds check: LLVM no longer \
                 eliminates it in a canonical `while i < len(xs)` loop over a borrowed array, \
                 so `std::ct`'s account of the scans and ADR-0020's are out of date"
            );
        } else {
            assert!(
                reaches_a_trap(body),
                "{label}: `{symbol}` (total) lost its bounds check: the unoptimizing backend \
                 has started eliminating checks, so `std::ct`'s account of the scans and \
                 ADR-0020's are out of date"
            );
        }
    }
}

/// The naive mask idiom really does trap, which is why `std::ct::mask` does
/// not use it.
///
/// This pins the *motivation* for the module's central design choice. If
/// tuonelang ever stopped trapping on negation overflow, this test would fail
/// and `mask`'s shift-based derivation could be reconsidered — so the
/// reasoning in its doc comment cannot quietly become false.
#[test]
fn the_naive_mask_idiom_traps_so_the_shift_derivation_is_necessary() {
    let dir = scratch_dir("naive_mask_traps");
    let program = "\
fn naive_mask(take bit: Int) -> Int {
    0 - bit
}

fn main() -> Int {
    // i64::MIN, the value on which negation overflows.
    naive_mask(0 - 9223372036854775807 - 1)
}
";
    let file = dir.join("naive.tuo");
    std::fs::write(&file, program).expect("write the naive-mask probe");

    let output = Command::new(env!("CARGO_BIN_EXE_tuo"))
        .arg("run")
        .arg(&file)
        .output()
        .expect("the compiler runs as a subprocess");
    assert!(
        !output.status.success(),
        "negating i64::MIN was expected to trap, but the program succeeded — `std::ct::mask`'s \
         shift-based derivation was justified by that trap, so this reasoning needs revisiting"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("integer overflow"),
        "expected an integer-overflow trap, got: {stderr}"
    );
}

/// A `#[constant_time]` function stays out of line in a release build, so the
/// code that runs is the body the branch-freedom checks inspect.
///
/// LLVM inlines small functions freely, and these primitives are a handful of
/// instructions each. Inlined, the executed code would be a copy merged into
/// its caller, where the optimizer may combine it with the surrounding code —
/// exactly where a branch could reappear — and the standalone body the checks
/// verify would never run. The backend marks such functions `noinline`; this
/// pins that `main`'s call to `select` really is a call.
#[test]
#[expect(
    clippy::print_stderr,
    reason = "records a skip when the host has no disassembler; a silent pass would \
claim a property that was never checked"
)]
fn constant_time_functions_stay_out_of_line_under_llvm() {
    let dir = scratch_dir("out_of_line");
    let sources = write_program(&dir, &["std::ct::select(std::rt::arg_count(), 10, 20)"]);
    let binary = dir.join("probe");
    build(&sources, &binary, true);
    let Some(disassembly) = disassemble(&binary) else {
        eprintln!("SKIPPED: no objdump on this host.");
        return;
    };
    let functions = tuonelang_functions(&disassembly);
    let select_index = BRANCH_FREE
        .iter()
        .position(|name| *name == "select")
        .expect("select is a primitive");
    let (select, _) = &functions[select_index];
    let called = disassembly.lines().any(|line| {
        let op = mnemonic(line);
        matches!(op.as_str(), "bl" | "b" | "call" | "callq" | "jmp")
            && line.contains(&format!("<{select}>"))
    });
    let mentions: Vec<&str> = disassembly
        .lines()
        .filter(|line| line.contains(select.trim_start_matches('_')))
        .collect();
    let main_body: Vec<&str> = disassembly
        .lines()
        .skip_while(|line| {
            !line.trim_end().ends_with("<main>:") && !line.trim_end().ends_with("<_main>:")
        })
        .take_while(|line| !line.trim().is_empty())
        .collect();
    assert!(
        called,
        "`{select}` (std::ct::select) is never called: it was inlined into its caller, so \
         the code that runs is not the body the branch-freedom checks verify.\n\
         lines mentioning it:\n{}\nmain:\n{}",
        mentions.join("\n"),
        main_body.join("\n")
    );
}

/// The positional symbol mapping the branch-freedom tests rely on is real.
///
/// `tuo_fn_<n>` symbols are assigned in declaration order, which is what lets
/// those tests say "the first nine emitted functions are the scalar
/// primitives". That is an implementation detail rather than a promised
/// contract, so it is checked rather than assumed: if numbering ever stopped
/// following declaration order, they would silently start inspecting the wrong
/// functions.
#[test]
#[expect(
    clippy::print_stderr,
    reason = "records a skip when the host has no disassembler; a silent pass would \
claim a property that was never checked"
)]
fn emitted_symbols_follow_declaration_order() {
    let dir = scratch_dir("symbol_order");
    let sources = write_program(&dir, &["0"]);
    let binary = dir.join("order");
    build(&sources, &binary, true);

    let Some(disassembly) = disassemble(&binary) else {
        eprintln!("SKIPPED: no objdump on this host.");
        return;
    };

    let functions = tuonelang_functions(&disassembly);
    let numbers: Vec<u32> = functions
        .iter()
        .filter_map(|(symbol, _)| {
            symbol
                .trim_start_matches('_')
                .strip_prefix("tuo_fn_")?
                .parse()
                .ok()
        })
        .collect();
    assert_eq!(
        numbers.len(),
        functions.len(),
        "every inspected symbol should be a numbered generated function"
    );
    assert!(
        numbers.windows(2).all(|pair| pair[0] < pair[1]),
        "generated symbols are not in ascending order, so the positional mapping the \
         branch-freedom tests rely on is invalid: {numbers:?}"
    );
    assert!(
        functions.len() >= BRANCH_FREE.len() + 2,
        "expected at least the primitives and both scans, found {}",
        functions.len()
    );
}

/// The disassembly parsing actually recognizes branches.
///
/// This exists because it failed to, and nothing noticed. The first version of
/// `mnemonic` took the wrong tab-separated field and returned an *operand*
/// instead of the instruction — so `is_conditional_branch` was asked whether
/// `0x100000630` was a branch, always answered no, and every branch-freedom
/// assertion passed unconditionally. The bug surfaced only when a deliberately
/// branch-ful `select` was compiled and the tests still passed.
///
/// A test whose assertions cannot fail is worse than no test, because it
/// reports safety it never checked. These cases are real `objdump` output
/// lines, tabs included.
#[test]
fn mnemonic_parsing_finds_real_branches() {
    // ARM64, as emitted by the Cranelift backend for an `if`.
    let branch = "100000630: 54000060    \tb.eq\t0x10000063c <_tuo_fn_73+0x14>";
    assert_eq!(mnemonic(branch), "b.eq");
    assert!(is_conditional_branch(&mnemonic(branch)));

    // Ordinary data instructions must not be mistaken for branches.
    for line in [
        "100000628: aa0103e5    \tmov\tx5, x1",
        "10000062c: f100041f    \tcmp\tx0, #0x1",
        "1000005a8: 9a810040    \tcsel\tx0, x2, x1, eq",
        "100000634: 8a010109    \tand\tx9, x8, x1",
    ] {
        assert!(
            !is_conditional_branch(&mnemonic(line)),
            "`{}` was misread as a conditional branch",
            mnemonic(line)
        );
    }

    // Unconditional control flow is not a data-dependent branch.
    let call = "1000005fc: 94000024    \tbl\t0x10000068c <_tuo_rt_trap>";
    assert_eq!(mnemonic(call), "bl");
    assert!(!is_conditional_branch(&mnemonic(call)));

    // x86-64 as GNU objdump prints it on Linux: the encoding is a tab field
    // of its own, and the operands follow the mnemonic after spaces. These
    // are real lines from the CI runner's disassembly.
    let gnu_call = "    29e3:\te8 38 fa ff ff       \tcall   2420 <tuo_fn_75>";
    assert_eq!(mnemonic(gnu_call), "call");
    let gnu_jump = "    250a:\te9 11 ff ff ff       \tjmp    2420 <tuo_fn_75>";
    assert_eq!(mnemonic(gnu_jump), "jmp");
    assert!(!is_conditional_branch(&mnemonic(gnu_jump)));
    let gnu_branch = "    2433:\t74 0b                \tje     2440 <tuo_fn_75+0x20>";
    assert_eq!(mnemonic(gnu_branch), "je");
    assert!(is_conditional_branch(&mnemonic(gnu_branch)));
    let gnu_move = "    29d6:\tbe 0a 00 00 00       \tmov    $0xa,%esi";
    assert_eq!(mnemonic(gnu_move), "mov");
    // ...and without raw bytes (`--no-show-raw-insn`), on either tool.
    assert_eq!(mnemonic("    2923:\tcall   2420 <tuo_fn_75>"), "call");
    assert_eq!(mnemonic("1000006a8:     \tadds\tx0, x0, x10"), "adds");

    for name in [
        "cbz", "cbnz", "tbz", "tbnz", "b.lt", "b.ne", "je", "jne", "jl",
    ] {
        assert!(is_conditional_branch(name), "`{name}` should be a branch");
    }
    for name in ["jmp", "b", "bl", "br", "ret", "mov", "add"] {
        assert!(
            !is_conditional_branch(name),
            "`{name}` should not be a branch"
        );
    }
}

/// Every worked example in `std::ct`'s doc comments is accurate — the code
/// compiles as written, and the value in the trailing comment is what it
/// actually produces.
///
/// Doc examples are what a caller copies, so an example that does not compile
/// is worse than no example. This caught two real defects: three examples
/// called the module's own `of3`/`of2` unqualified (`of3(7, 8, 9)` rather than
/// `std::ct::of3(7, 8, 9)`), which does not resolve from a caller's scope, and
/// `bytes_eq`'s prose still described a loop bound that had since changed from
/// the first array's length to the shorter of the two.
#[test]
fn every_doc_example_in_std_ct_is_accurate() {
    let dir = scratch_dir("doc_examples");

    // Each value is the one written in that function's `# Example` comment.
    let program = "\
fn main() -> Int {
    if std::ct::mask(1) != 0 - 1 { return 1; }
    if std::ct::mask(0) != 0 { return 2; }
    if std::ct::select(1, 10, 20) != 10 { return 3; }
    if std::ct::nonzero(5) != 1 { return 4; }
    if std::ct::is_zero(0) != 1 { return 5; }
    if std::ct::eq(7, 7) != 1 { return 6; }
    if std::ct::ne(7, 9) != 1 { return 7; }
    if std::ct::is_negative(0 - 1) != 1 { return 8; }
    if std::ct::lt(3, 9) != 1 { return 9; }
    if std::ct::gt(9, 3) != 1 { return 10; }
    if std::ct::select_array(std::ct::of3(7, 8, 9), 1) != 8 { return 11; }
    if std::ct::bytes_eq(std::ct::of3(1, 2, 3), std::ct::of3(1, 2, 3)) != 1 { return 12; }
    if std::ct::element_or_zero(std::ct::of3(4, 5, 6), 0) != 4 { return 13; }
    if std::array::get(std::ct::of3(7, 8, 9), 0) != 7 { return 14; }
    if std::array::get(std::ct::of2(4, 5), 1) != 5 { return 15; }
    99
}
";
    let mut sources = write_program(&dir, &["0"]);
    let driver = sources.pop().expect("the driver is last");
    std::fs::write(&driver, program).expect("overwrite the driver");
    sources.push(driver);

    let binary = dir.join("docs");
    build(&sources, &binary, false);
    let status = Command::new(&binary)
        .status()
        .expect("the doc-example probe runs as a subprocess");
    assert_eq!(
        status.code(),
        Some(99),
        "a std::ct doc example does not produce its documented value (the exit status is the \
         1-based index of the first example that disagreed)"
    );
}

/// The Montgomery ladder's conditional swap and step are compiler-verified
/// constant time.
///
/// Behaviour cannot guard this: exchanging two points with an `if` and with a
/// mask compute exactly the same function, so every spec, every RFC 7748
/// vector, and every live handshake passes either way (verified when this was
/// a text check: reintroducing the branch left the whole suite green). The
/// swap was once written with exactly that `if` while its comment claimed
/// otherwise.
///
/// Since `std::fe25519` (ADR-0022), both functions are straight-line code and
/// carry `#[constant_time]`, so a branch on the scalar bit is a compile error
/// (`T0017`) and the stdlib suite's compile of the module is the guard. What
/// is left to pin is that the attribute stays — deleting it would silently
/// return the guard to nothing — and that the swap still selects all four
/// coordinates rather than being "branch-free" because the exchange was
/// deleted.
#[test]
fn the_ladder_is_compiler_verified_constant_time() {
    let module = tuo_stdlib::module("std::x25519").expect("std::x25519 is a catalog module");
    for header in ["pub fn conditional_swap(", "pub fn ladder_step("] {
        let marked = format!("#[constant_time]\n{header}");
        assert!(
            module.source.contains(&marked),
            "`std::x25519::{header}` must carry `#[constant_time]`: its inputs are bits of \
             the secret scalar, and no spec or RFC vector can tell a branching version from \
             a branchless one"
        );
    }
    let body = function_body(module.source, "pub fn conditional_swap(")
        .expect("std::x25519 defines conditional_swap");
    assert_eq!(
        body.matches("std::fe25519::select(").count(),
        4,
        "`std::x25519::conditional_swap` must select all four coordinates.\n\nbody:\n{body}"
    );
}

/// The text of the function whose declaration starts with `header`, from the
/// declaration to the closing brace at column 0.
///
/// A brace-matching parse would be more precise, but the catalog is formatted
/// canonically by `tuo fmt`, so a top-level `}` is always at column 0 and this
/// is enough to isolate one function.
fn function_body(source: &str, header: &str) -> Option<String> {
    let start = source.find(header)?;
    let rest = &source[start..];
    let end = rest.find("\n}\n")? + 3;
    Some(rest[..end].to_string())
}
