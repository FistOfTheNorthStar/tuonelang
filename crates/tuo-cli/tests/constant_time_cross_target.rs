//! Is `std::ct` still branchless on an architecture the developer is not
//! running?
//!
//! `constant_time.rs` disassembles a real binary and proves the ten marked
//! primitives contain no conditional branch — but only for the host, which on
//! this workspace's development machines is ARM64. That left ADR-0020's central
//! claim resting on one architecture, and the gap was recorded in the ADR as
//! "cross-target behavior is unverified".
//!
//! The gap turned out to be narrower than recorded. The claim was that the
//! compiler targets the host only, so the question could not be asked; in fact
//! `TargetSpec` carries an arbitrary triple and both backends simply *refused*
//! a non-host one. The ABI is identical across the 64-bit targets v0 defines
//! (`POINTER_SIZE` is 8 for all of them), so LLVM can emit a correct object for
//! another 64-bit triple — it is *linking* that needs a cross-linker the
//! workspace does not have.
//!
//! So this suite emits an object rather than a binary, and disassembles that.
//! The property being checked is a property of the emitted instructions, which
//! an object file carries in full; running it is not required to read it.
//!
//! Why this matters concretely: x86-64 and ARM64 have different conditional
//! primitives, and LLVM's O2 chooses between a branch and a conditional move
//! per architecture. A masking idiom that stays branchless on one can become a
//! `cmov` — or a real branch — on the other. Only measurement settles it.

use std::path::{Path, PathBuf};
use std::process::Command;

use tuo_codegen::{CodegenBackend, EntryAbi, TargetSpec};
use tuo_codegen_llvm::LlvmBackend;
// The stage crates reach this test through the compiler facade, which is the
// dependency edge the CLI is allowed to have.
use tuo_compiler::ast::Ast;
use tuo_compiler::source::SourceMap;
use tuo_compiler::{hir, mir, parser};

/// The non-host 64-bit triples checked here.
///
/// Both are targets a tuonelang program would plausibly ship to, and both
/// differ from the development host's architecture or platform, so between
/// them they cover the x86-64 instruction selection this suite exists to
/// inspect.
const CROSS_TARGETS: &[&str] = &["x86_64-unknown-linux-gnu", "x86_64-apple-darwin"];

/// A scratch directory for one case.
fn scratch_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("constant_time_cross")
        .join(name);
    std::fs::create_dir_all(&dir).expect("scratch dir is creatable");
    dir
}

/// The `#[constant_time]`-marked functions of `std::ct`, as a compilable
/// module.
///
/// Extracted from the **real catalog source** rather than transcribed, so this
/// cannot drift from what actually ships: if a primitive gains a branch, or a
/// new one is added, or the attribute is removed from one, this picks up the
/// change automatically. A hand-written copy would keep passing while the
/// shipped library regressed.
///
/// Every marked function is emitted along with `module ct;` so the driver can
/// call them by path. Specs are dropped (they are not code under test here)
/// and the two unmarked scans are dropped by construction — they carry no
/// attribute, so the scan below never reaches them.
fn marked_primitives_source() -> String {
    let module = tuo_stdlib::module("std::ct").expect("std::ct is a catalog module");
    let mut out = String::from("module ct;\n\n");
    let mut found = 0;

    let lines: Vec<&str> = module.source.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() == "#[constant_time]" {
            // The attribute is followed by the `pub fn` line and then the body
            // up to the closing brace at column zero.
            let mut j = i + 1;
            while j < lines.len() && !lines[j].starts_with("pub fn") {
                j += 1;
            }
            let start = j;
            while j < lines.len() && lines[j] != "}" {
                j += 1;
            }
            assert!(
                j < lines.len(),
                "a marked function must have a closing brace"
            );
            out.push_str("#[constant_time]\n");
            for line in &lines[start..=j] {
                out.push_str(line);
                out.push('\n');
            }
            out.push('\n');
            found += 1;
            i = j + 1;
            continue;
        }
        i += 1;
    }

    // `std::ct` documents ten marked scalar primitives. If the catalog changes
    // this number the test should be updated deliberately, not silently pass
    // over a smaller set.
    assert_eq!(
        found, 10,
        "expected the ten `#[constant_time]` primitives in std::ct, found {found}; \
         the catalog changed and this test needs updating"
    );
    out
}

/// Emit a relocatable object for `triple` from a program that calls into
/// `std::ct`, or `None` if this LLVM has no such target.
fn emit_object(dir: &Path, triple: &str) -> Option<PathBuf> {
    let mut map = SourceMap::new();
    let mut ids = Vec::new();

    // Only the ten `#[constant_time]` primitives, copied out of the catalog
    // module rather than compiling all of it.
    //
    // This is the load-bearing decision in this file. `std::ct` also contains
    // `select_array` and `bytes_eq`, which are **deliberately unmarked**: a
    // scan needs a loop, a loop is a backward conditional branch, and their
    // documented guarantee is only that control flow depends on array
    // *lengths* (public) rather than contents. Compiling the whole module and
    // asserting "no function branches" would fail on those two — and, worse, a
    // version of this test that special-cased them by symbol could not tell
    // them apart once names are gone, since the object carries only
    // `tuo_fn_<n>`. Compiling exactly the marked set removes the ambiguity:
    // every function in the object is one that claims to be branchless, so any
    // branch is a real violation.
    let primitives = marked_primitives_source();
    let file = map.intern_file("ct_primitives.tuo");
    ids.push(
        map.add_source(file, primitives.as_str())
            .expect("the primitive subset is not too large"),
    );

    // Call every marked primitive so none is dead-stripped before it can be
    // inspected. `mask` and `select` are the ones an optimizer is most likely
    // to rewrite into a conditional, but the whole set is checked.
    let driver = "fn main() -> Int {\n\
                  \x20   var total = 0;\n\
                  \x20   total = total + ct::mask(1);\n\
                  \x20   total = total + ct::select(1, 10, 20);\n\
                  \x20   total = total + ct::nonzero(7);\n\
                  \x20   total = total + ct::is_zero(7);\n\
                  \x20   total = total + ct::eq(3, 3);\n\
                  \x20   total = total + ct::ne(3, 4);\n\
                  \x20   total = total + ct::is_negative(0 - 1);\n\
                  \x20   total = total + ct::lt(1, 2);\n\
                  \x20   total = total + ct::gt(2, 1);\n\
                  \x20   total = total + ct::highest_bit(9);\n\
                  \x20   total & 1\n\
                  }\n";
    let file = map.intern_file("driver.tuo");
    ids.push(map.add_source(file, driver).expect("the driver fits"));

    let check = tuo_compiler::check_sources(&map, &ids);
    assert!(
        !check.has_errors(),
        "the cross-target probe program must compile: {:?}",
        check.diagnostics
    );

    let parses: Vec<_> = ids
        .iter()
        .map(|&id| parser::parse(map.source(id)))
        .collect();
    let asts: Vec<Ast<'_>> = parses
        .iter()
        .zip(&ids)
        .map(|(parse, &id)| Ast::new(&parse.tree, map.source(id).text()))
        .collect();
    let hir = hir::lower(&asts, &check.resolution);
    let mut program = mir::lower(&hir, &check.resolution, &check.types);
    assert!(
        mir::verify(&program, &check.types).is_empty(),
        "lowered MIR must verify"
    );
    mir::optimize(&mut program, &check.types);

    let target = TargetSpec {
        triple: triple.to_owned(),
    };
    let artifact = match LlvmBackend::release().compile(
        &program,
        &check.types,
        "main",
        EntryAbi::IntReturn,
        &target,
    ) {
        Ok(artifact) => artifact,
        // An LLVM built without this target is a skip, not a failure: the
        // property is real, this host just cannot inspect it.
        Err(_) => return None,
    };

    let path = dir.join(format!("{}.o", triple.replace(['-', '.'], "_")));
    std::fs::write(&path, &artifact.bytes).expect("write the object");
    Some(path)
}

/// Disassemble `object`, or `None` when no disassembler is installed.
fn disassemble(object: &Path) -> Option<String> {
    for tool in ["objdump", "llvm-objdump", "/usr/bin/objdump"] {
        let out = Command::new(tool)
            .arg("-d")
            .arg("--no-show-raw-insn")
            .arg(object)
            .output();
        if let Ok(out) = out {
            if out.status.success() {
                return Some(String::from_utf8_lossy(&out.stdout).into_owned());
            }
        }
    }
    None
}

/// The mnemonic of a disassembly line.
///
/// `objdump` emits `<address>:\t<mnemonic>\t<operands>` with
/// `--no-show-raw-insn`, so the mnemonic is the second tab-separated field's
/// first word. Pinned by a test below against real output, because a parser
/// that silently returned the wrong field would make every check here pass
/// vacuously — which is exactly the bug this file's host-side sibling caught.
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

/// Is this a conditional branch — a jump whose target depends on a comparison?
///
/// x86-64 spells these `j<cc>` (`je`, `jne`, `jl`, …) but not `jmp`; ARM64 uses
/// `b.<cond>` and the compare-and-branch family. An unconditional branch is
/// fine: it reveals nothing about the data.
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

/// The disassembly split into (symbol, body lines).
fn functions(disassembly: &str) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for line in disassembly.lines() {
        if line.contains('<') && line.trim_end().ends_with(">:") {
            let name = line
                .rsplit('<')
                .next()
                .unwrap_or("")
                .trim_end_matches(">:")
                .to_owned();
            out.push((name, Vec::new()));
        } else if let Some((_, body)) = out.last_mut() {
            if !line.trim().is_empty() {
                body.push(line.to_owned());
            }
        }
    }
    out
}

/// Does this function body reach a trap? Both backends call the runtime
/// handler by symbol.
fn reaches_a_trap(body: &[String]) -> bool {
    body.iter().any(|line| line.contains("tuo_rt_trap"))
}

/// Every `#[constant_time]` primitive is branch-free in the code emitted for a
/// **non-host** 64-bit target.
///
/// This is the check ADR-0020 could not make when Stage B landed. It is the
/// same property `constant_time.rs` proves on the host, asked of an
/// architecture whose conditional instructions and optimizer choices differ.
#[expect(
    clippy::print_stderr,
    reason = "records a skip when the host cannot emit or disassemble for a target; \
a silent pass would claim a property that was never checked"
)]
#[test]
fn the_marked_primitives_are_branch_free_on_a_non_host_target() {
    let mut checked = 0;
    for triple in CROSS_TARGETS {
        let dir = scratch_dir(triple);
        let Some(object) = emit_object(&dir, triple) else {
            eprintln!("SKIPPED ({triple}): this LLVM cannot emit for that target");
            continue;
        };
        let Some(disassembly) = disassemble(&object) else {
            eprintln!("SKIPPED ({triple}): no objdump on this host");
            continue;
        };

        let mut seen = 0;
        for (name, body) in functions(&disassembly) {
            // Generated functions are named `tuo_fn_<n>`; the source symbol
            // name does not survive into the object, so the primitives cannot
            // be picked out individually. That is fine and in fact stronger:
            // the probe program is `std::ct` plus a driver that only calls into
            // it, so *every* generated function here is either a marked
            // primitive or the driver, and the driver is trivially branchless
            // too. Checking all of them therefore checks the primitives with no
            // way to accidentally skip one.
            // Mach-O prefixes symbols with an underscore, ELF does not, so
            // both spellings are accepted.
            let generated = name.starts_with("tuo_fn_") || name.starts_with("_tuo_fn_");
            if !generated || body.is_empty() {
                continue;
            }
            seen += 1;
            // A trap edge would itself be a data-dependent branch, and the
            // checker forbids trapping arithmetic in a marked function, so a
            // marked primitive must reach no trap on any target.
            assert!(
                !reaches_a_trap(&body),
                "{triple}: `{name}` reaches a trap; a trap check is a branch on its operand"
            );
            for line in &body {
                let m = mnemonic(line);
                assert!(
                    !is_conditional_branch(&m),
                    "{triple}: `{name}` emits the conditional branch `{m}`, so its timing \
                     can depend on the data:\n  {}",
                    line.trim()
                );
            }
        }
        // `std::ct` has ten marked primitives plus two unmarked scans and two
        // helpers; a handful of them may be inlined away, but finding only a
        // couple would mean the probe program was optimized into nothing and
        // the pass below proved nothing.
        assert!(
            seen >= 8,
            "{triple}: only {seen} generated functions were found in the disassembly, so \
             almost nothing was actually checked; the probe program or the symbol matching \
             is wrong"
        );
        checked += 1;
    }
    if checked == 0 {
        eprintln!("SKIPPED: no cross-target object could be emitted and disassembled on this host");
    }
}

/// The mnemonic parser really finds branches in real `objdump` output.
///
/// Without this the suite above could pass by reading the wrong field and
/// finding no branch anywhere — reporting a security property it never
/// checked. These are real disassembly lines from both architectures.
#[test]
fn mnemonic_parsing_finds_real_branches() {
    // x86-64, the architecture this suite exists to inspect.
    assert_eq!(mnemonic("  401136:\tje     401140 <main+0x1a>"), "je");
    assert_eq!(mnemonic("  40113c:\tjmp    401150 <main+0x2a>"), "jmp");
    assert_eq!(mnemonic("  401130:\tcmovne %eax,%edx"), "cmovne");
    assert_eq!(mnemonic("  401120:\tmov    %rsp,%rbp"), "mov");
    // ARM64, for parity with the host-side suite.
    assert_eq!(mnemonic("       8: \tb.ne\t0x18 <_f+0x18>"), "b.ne");
    assert_eq!(mnemonic("       c: \tand\tx8, x9, x10"), "and");

    assert!(is_conditional_branch("je"));
    assert!(is_conditional_branch("jne"));
    assert!(is_conditional_branch("b.ne"));
    assert!(!is_conditional_branch("jmp"));
    assert!(!is_conditional_branch("mov"));
    assert!(!is_conditional_branch("ret"));
    // A conditional MOVE is not a branch: it is the branchless form an
    // optimizer produces, and rejecting it would fail the very rewrite that
    // preserves the property.
    assert!(!is_conditional_branch("cmovne"));
}
