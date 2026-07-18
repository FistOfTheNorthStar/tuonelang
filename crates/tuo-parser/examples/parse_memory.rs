//! Heap-use measurement for the parser architecture decision gate: parses
//! the fixed corpus (`benchmarks/compiler/parser/`) with both engines under
//! the dhat profiling allocator and reports peak and total heap per run.
//! Results feed `specification/adr/ADR-parser-strategy.md`.
//!
//! Run with: `cargo run -p tuo-parser --release --example parse_memory`

// This is a measurement tool; printing the results is its purpose.
#![allow(clippy::print_stdout)]

use tuo_source::SourceMap;

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

const CORPUS: &[(&str, &str)] = &[
    (
        "clean",
        include_str!("../../../benchmarks/compiler/parser/clean.tuo"),
    ),
    (
        "expr_heavy",
        include_str!("../../../benchmarks/compiler/parser/expr_heavy.tuo"),
    ),
    (
        "error_heavy",
        include_str!("../../../benchmarks/compiler/parser/error_heavy.tuo"),
    ),
];

fn main() {
    println!("file        engine       peak_bytes  peak_blocks  total_bytes  total_blocks");
    for (name, text) in CORPUS {
        for engine in ["chumsky", "handwritten"] {
            // Set the source up outside the profiled region so only the
            // parse itself is measured.
            let mut map = SourceMap::new();
            let file = map.intern_file("mem.tuo");
            let id = map.add_source(file, *text).expect("corpus fits");
            let source = map.source(id);

            let profiler = dhat::Profiler::builder().testing().build();
            let result = match engine {
                "chumsky" => tuo_parser::oracle::parse(source),
                _ => tuo_parser::parse(source),
            };
            let stats = dhat::HeapStats::get();
            let functions = result
                .tree
                .root
                .descendants_of_kind(tuo_syntax::SyntaxKind::FunctionItem)
                .len();
            let errors = result
                .tree
                .root
                .descendants_of_kind(tuo_syntax::SyntaxKind::Error)
                .len();
            println!(
                "{name:<11} {engine:<12} {:>10}  {:>11}  {:>11}  {:>12}  diags {} fns {functions} error-islands {errors}",
                stats.max_bytes,
                stats.max_blocks,
                stats.total_bytes,
                stats.total_blocks,
                result.all_diagnostics().len(),
            );
            drop(result);
            drop(profiler);
        }
    }
}
