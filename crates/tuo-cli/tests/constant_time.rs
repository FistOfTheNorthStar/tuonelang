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
/// The calls do not determine what ends up in the binary — the compiler emits
/// every function in a loaded module, reachable or not — but a driver is still
/// needed for the program to link.
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
/// `objdump` emits `<address>: <encoding>\t<mnemonic>\t<operands>`, so
/// splitting on tabs puts the address and encoding first and the mnemonic
/// second. Taking the wrong field here silently defeats every assertion built
/// on it — the mnemonic becomes an operand, which never looks like a branch —
/// so `mnemonic_parsing_finds_real_branches` pins it against real output.
fn mnemonic(line: &str) -> String {
    line.split('\t')
        .map(str::trim)
        .filter(|field| !field.is_empty())
        .nth(1)
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

    // The compiler emits *every* function in a loaded module, so the
    // primitives are picked out by symbol: `tuo_fn_<n>` numbers follow
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

/// Even a canonical, trivially-in-range loop keeps its bounds check.
///
/// This pins the *scope* of the limitation `std::ct` documents: the surviving
/// bounds check in the array scans is a property of the compiler, not of how
/// this module happens to be written. Without this, a reader could reasonably
/// assume the scans could be rewritten to avoid it, and waste effort trying.
///
/// This was measured rather than assumed, and it is why `std::ct` claims only
/// that the scans' control flow depends on array *lengths* — never on their
/// contents — instead of claiming they are branch-free.
#[test]
#[expect(
    clippy::print_stderr,
    reason = "records a skip when the host has no disassembler; a silent pass would \
claim a property that was never checked"
)]
fn the_bounds_check_limitation_is_the_compilers_not_this_modules() {
    let dir = scratch_dir("canonical_loop");
    let program = "\
fn total(in xs: Array[Int]) -> Int {
    var t = 0;
    var i = 0;
    while i < std::array::len(xs) {
        t = t + std::array::get(xs, i);
        i = i + 1;
    }
    t
}

fn main() -> Int {
    var xs = std::array::empty();
    std::array::push(xs, 1);
    total(xs) & 1
}
";
    let file = dir.join("canonical.tuo");
    std::fs::write(&file, program).expect("write the canonical-loop probe");
    let binary = dir.join("canonical");
    build(&[file], &binary, true);

    let Some(disassembly) = disassemble(&binary) else {
        eprintln!("SKIPPED: no objdump on this host.");
        return;
    };
    let traps = tuonelang_functions(&disassembly)
        .iter()
        .filter(|(_, body)| reaches_a_trap(body))
        .count();
    assert!(
        traps > 0,
        "the canonical `while i < len(xs)` loop no longer emits a bounds check — the compiler \
         has gained bounds-check elimination, which is good news that makes `std::ct`'s \
         documented loop caveat and ADR-0020's account of it out of date"
    );
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
