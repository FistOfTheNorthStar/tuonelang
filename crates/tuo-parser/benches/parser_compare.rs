//! Parser architecture decision gate: Chumsky engine vs. handwritten
//! recursive-descent + Pratt engine over the fixed corpus in
//! `benchmarks/compiler/parser/`. Results feed
//! `specification/adr/ADR-parser-strategy.md`.
//!
//! Both engines run behind the same interface and include lexing (the public
//! `parse` contract); the `lex_only` baseline isolates the lexer's share so
//! the parser-only difference can be derived.

// criterion_group! expands to undocumented public items.
#![allow(missing_docs)]

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use tuo_source::{SourceId, SourceMap};

/// The fixed benchmark corpus (checked in; see benchmarks/compiler/parser/).
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

fn interned(text: &str) -> (SourceMap, SourceId) {
    let mut map = SourceMap::new();
    let file = map.intern_file("bench.tuo");
    let id = map.add_source(file, text).expect("corpus fits");
    (map, id)
}

fn bench_engines(c: &mut Criterion) {
    for (name, text) in CORPUS {
        let mut group = c.benchmark_group(format!("parse/{name}"));
        group.throughput(Throughput::Bytes(text.len() as u64));

        // Repeated parse: the same retained snapshot parsed again and again
        // (the IDE-facing pattern until incremental reparsing exists).
        let (map, id) = interned(text);
        let source = map.source(id);
        group.bench_function("chumsky/repeated", |b| {
            b.iter(|| tuo_parser::oracle::parse(source));
        });
        group.bench_function("handwritten/repeated", |b| {
            b.iter(|| tuo_parser::parse(source));
        });

        // Cold parse: a fresh source map interned per iteration (the batch
        // compile pattern).
        group.bench_function("chumsky/cold", |b| {
            b.iter_batched(
                || interned(text),
                |(map, id)| tuo_parser::oracle::parse(map.source(id)),
                criterion::BatchSize::SmallInput,
            );
        });
        group.bench_function("handwritten/cold", |b| {
            b.iter_batched(
                || interned(text),
                |(map, id)| tuo_parser::parse(map.source(id)),
                criterion::BatchSize::SmallInput,
            );
        });

        // Baseline: lexing alone, to isolate the parser's share.
        group.bench_function("lex_only", |b| {
            b.iter(|| tuo_lexer::lex(source));
        });

        group.finish();
    }
}

criterion_group!(benches, bench_engines);
criterion_main!(benches);
