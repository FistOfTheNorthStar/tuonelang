//! Every allocation a `Map[_, String]` program makes is freed exactly once
//! (ADR-0023 Stage B2.2).
//!
//! The three-way fixture proves the engines *agree*, and a double free there
//! aborts, but a leak is silent: a map drop that skipped its values, or a
//! discarded overwrite result nobody freed, still exits with the right code.
//! This suite counts instead. It compiles a program to an object through the
//! compiler facade and a backend (as `constant_time_cross_target.rs` does),
//! then links it with the real trap/effect/map C runtime and a **test-only
//! counting allocator** standing in for the `alloc` shim. At exit the
//! allocator reports how many real blocks it handed out and took back; the
//! two must be equal, and non-zero.
//!
//! The program inserts, overwrites (discarding one result and reading
//! another), removes (both ways), reads hits and misses, and drops non-empty
//! maps with holes, on both key kinds, over many rounds.

use std::path::{Path, PathBuf};
use std::process::Command;

use tuo_codegen::{CodegenBackend, EntryAbi, TargetSpec};
use tuo_codegen_cranelift::CraneliftBackend;
use tuo_codegen_llvm::LlvmBackend;
// The stage crates reach this test through the compiler facade, which is the
// dependency edge the CLI is allowed to have.
use tuo_compiler::ast::Ast;
use tuo_compiler::source::SourceMap;
use tuo_compiler::{hir, mir, parser};
use tuo_runtime::alloc::ZERO_SIZE_SENTINEL;

/// The program under count. `round` returns 30 when every read agrees, so
/// 100 rounds sum to 3000 and the exit is `3000 % 256 = 184`.
const PROGRAM: &str = r#"
fn letters(take n: Int) -> String {
    var s = std::string::empty();
    var i = 0;
    while i < n {
        std::string::push_byte(s, 97 + i);
        i = i + 1;
    }
    s
}

fn int_round() -> Int {
    var m = std::map::empty();
    let _ = std::map::insert(m, 1, letters(3));
    let _ = std::map::insert(m, 2, letters(5));
    let _ = std::map::insert(m, 3, letters(7));
    let _ = std::map::insert(m, 4, letters(2));
    var total = 0;
    // A discarded overwrite, then a read one.
    let _ = std::map::insert(m, 2, letters(6));
    match std::map::insert(m, 3, letters(4)) {
        Some { value } => {
            total = total + std::string::len(value);
        },
        None => {},
    }
    // A discarded removal, then a read one: two holes.
    let _ = std::map::remove(m, 1);
    match std::map::remove(m, 4) {
        Some { value } => {
            total = total + std::string::len(value);
        },
        None => {},
    }
    // Hits and misses.
    match std::map::get(m, 2) {
        Some { value } => {
            total = total + std::string::len(value);
        },
        None => {},
    }
    match std::map::get(m, 1) {
        Some { value } => {
            total = total + 100 + std::string::len(value);
        },
        None => {},
    }
    // `m` drops here holding keys 2 and 3, after two holes.
    total
}

fn str_round() -> Int {
    var m = std::map::empty();
    let _ = std::map::insert(m, "a", letters(1));
    let _ = std::map::insert(m, "b", letters(2));
    let _ = std::map::insert(m, "c", letters(3));
    var total = 0;
    let _ = std::map::insert(m, "a", letters(4));
    let _ = std::map::remove(m, "b");
    match std::map::get(m, "c") {
        Some { value } => {
            total = total + std::string::len(value);
        },
        None => {},
    }
    match std::map::get(m, "b") {
        Some { value } => {
            total = total + 100 + std::string::len(value);
        },
        None => {},
    }
    // `m` drops here holding "a" and "c", after a hole.
    total
}

fn main() -> Int {
    // int_round: 7 + 2 + 6 = 15; str_round: 3; and an empty map's drop.
    var total = 0;
    var i = 0;
    while i < 100 {
        total = total + int_round() + str_round() + 12;
        var empty: Map[Int, String] = std::map::empty();
        total = total + std::map::len(empty);
        i = i + 1;
    }
    total % 256
}
"#;

/// The expected exit: 100 rounds of `15 + 3 + 12 = 30`, modulo 256.
const EXPECTED_EXIT: i32 = 3000 % 256;

/// The counting allocator: the `alloc` shim's contract (a zero-size request
/// is the sentinel, freeing the sentinel or null is a no-op), plus two
/// counters reported on stderr when the process exits.
fn counting_alloc_c_source() -> String {
    format!(
        "#include <stdio.h>\n\
         #include <stdlib.h>\n\
         \n\
         static long tuo_test_allocs = 0;\n\
         static long tuo_test_deallocs = 0;\n\
         \n\
         void *tuo_rt_alloc(size_t size, size_t align) {{\n\
         \x20   (void)align;\n\
         \x20   if (size == 0) return (void *){ZERO_SIZE_SENTINEL};\n\
         \x20   void *p = malloc(size);\n\
         \x20   if (p == NULL) abort();\n\
         \x20   tuo_test_allocs++;\n\
         \x20   return p;\n\
         }}\n\
         \n\
         void tuo_rt_dealloc(void *ptr, size_t size, size_t align) {{\n\
         \x20   (void)size;\n\
         \x20   (void)align;\n\
         \x20   if (ptr == NULL || ptr == (void *){ZERO_SIZE_SENTINEL}) return;\n\
         \x20   tuo_test_deallocs++;\n\
         \x20   free(ptr);\n\
         }}\n\
         \n\
         __attribute__((destructor)) static void tuo_test_report(void) {{\n\
         \x20   fprintf(stderr, \"tuo-balance allocs=%ld deallocs=%ld\\n\",\n\
         \x20           tuo_test_allocs, tuo_test_deallocs);\n\
         }}\n"
    )
}

/// A fresh scratch directory for one backend's case.
fn scratch_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("map_string_balance")
        .join(name);
    std::fs::create_dir_all(&dir).expect("scratch dir is creatable");
    dir
}

/// Compile [`PROGRAM`] with `backend` to a relocatable object in `dir`.
fn emit_object(dir: &Path, backend: &dyn CodegenBackend) -> PathBuf {
    let mut map = SourceMap::new();
    let file = map.intern_file("balance.tuo");
    let id = map.add_source(file, PROGRAM).expect("the program fits");
    let ids = [id];
    let check = tuo_compiler::check_sources(&map, &ids);
    assert!(
        !check.has_errors(),
        "the balance program must compile: {:?}",
        check.diagnostics
    );
    let parse = parser::parse(map.source(id));
    let asts = [Ast::new(&parse.tree, map.source(id).text())];
    let hir = hir::lower(&asts, &check.resolution);
    let mut program = mir::lower(&hir, &check.resolution, &check.types);
    assert!(
        mir::verify(&program, &check.types).is_empty(),
        "lowered MIR must verify"
    );
    mir::optimize(&mut program, &check.types);
    let artifact = backend
        .compile(
            &program,
            &check.types,
            "main",
            EntryAbi::IntReturn,
            &TargetSpec::host(),
        )
        .expect("the balance program lowers natively");
    let path = dir.join("balance.o");
    std::fs::write(&path, &artifact.bytes).expect("write the object");
    path
}

/// Link `object` with the real trap/effect/map runtime and the counting
/// allocator, run it, and return `(allocs, deallocs)` after checking its exit.
fn run_counted(dir: &Path, object: &Path) -> (u64, u64) {
    let sources = [
        ("trap", tuo_runtime::trap_runtime_c_source()),
        ("effect", tuo_runtime::effect::effect_runtime_c_source()),
        ("map", tuo_runtime::map::map_runtime_c_source()),
        ("counting_alloc", counting_alloc_c_source()),
    ];
    let mut shims = Vec::new();
    for (name, source) in sources {
        let path = dir.join(format!("{name}.c"));
        std::fs::write(&path, source).expect("write a runtime source");
        shims.push(path);
    }
    let exe = dir.join("balance");
    let status = Command::new("cc")
        .arg("-O2")
        .arg(object)
        .args(&shims)
        .arg("-lm")
        .arg("-pthread")
        .arg("-o")
        .arg(&exe)
        .status()
        .expect("launch cc");
    assert!(status.success(), "linking the balance program failed");
    let output = Command::new(&exe)
        .output()
        .expect("run the balance program");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(EXPECTED_EXIT),
        "the balance program's exit is wrong; stderr: {stderr}"
    );
    let report = stderr
        .lines()
        .find_map(|line| line.strip_prefix("tuo-balance "))
        .unwrap_or_else(|| panic!("the counting allocator did not report; stderr: {stderr}"));
    let count = |field: &str| -> u64 {
        report
            .split_whitespace()
            .find_map(|part| part.strip_prefix(field))
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(|| panic!("no `{field}` in the report `{report}`"))
    };
    (count("allocs="), count("deallocs="))
}

fn assert_balanced(name: &str, backend: &dyn CodegenBackend) {
    let dir = scratch_dir(name);
    let object = emit_object(&dir, backend);
    let (allocs, deallocs) = run_counted(&dir, &object);
    assert!(allocs > 0, "{name}: the program allocated nothing");
    assert_eq!(
        allocs, deallocs,
        "{name}: {allocs} allocations but {deallocs} deallocations"
    );
}

#[test]
fn cranelift_frees_every_map_string_allocation_exactly_once() {
    assert_balanced("cranelift", &CraneliftBackend::new());
}

/// The LLVM backend unoptimized. Its release pipeline inlines the allocator
/// seam (`available_externally`), so an optimized object calls `malloc`
/// directly and the counting allocator would not see every allocation. At
/// `OptLevel::None` nothing is inlined: every allocation and free goes through
/// the `tuo_rt_alloc`/`tuo_rt_dealloc` symbols, so the count is exact. The
/// map lowering being counted is the same at both levels.
#[test]
fn llvm_frees_every_map_string_allocation_exactly_once() {
    assert_balanced("llvm", &LlvmBackend::new());
}
