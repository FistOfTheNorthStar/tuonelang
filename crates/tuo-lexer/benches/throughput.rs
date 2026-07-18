//! Lexer throughput benchmark.
//!
//! Measures bytes/second over representative tuonelang source scaled to
//! ~1 MiB, plus two shape extremes (comment-heavy and token-dense input).
//! Run with `cargo bench -p tuo-lexer`. Numbers are machine-specific;
//! the crate makes no performance claim beyond what a run of this benchmark
//! shows.

// `criterion_group!` expands to an undocumented public function.
#![allow(missing_docs)]

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;
use tuo_lexer::lex;
use tuo_source::SourceMap;

/// A representative module: items, generics, literals, specs, comments.
const REPRESENTATIVE: &str = r#"module geometry

/// A point in the plane.
pub struct Point {
    x: F64,
    y: F64,
}

/// Scale a point by `factor`, e.g. 2.5 or 1_000.
pub fn scale(take p: Point, factor: F64) -> Point {
    // Componentwise multiplication; no aliasing.
    Point { x: p.x * factor, y: p.y * factor }
}

pub fn classify[T](in value: T, limit: Int) -> String where T: Ord {
    var total = 0x10u32;
    let label = "päivää\n";
    if total >= 0o17 && value != limit {
        return "big";
    }
    'outer: for i in 0 .. 100 {
        match i % 3 {
            0 => break 'outer,
            _ => continue,
        }
    }
    "small"
}

spec scale_doubles {
    given p = Point { x: 1.0, y: 2.0e0 };
    when scaled = scale(take p, 2.0);
    then assert scaled.x == 2.0;
}
"#;

/// Repeat `unit` until the result is at least `target_len` bytes.
fn scaled_to(unit: &str, target_len: usize) -> String {
    let mut text = String::with_capacity(target_len + unit.len());
    while text.len() < target_len {
        text.push_str(unit);
    }
    text
}

fn bench_throughput(c: &mut Criterion) {
    const MIB: usize = 1 << 20;
    let comment_heavy = scaled_to(
        "// a line comment that the lossless lexer must keep as a token\n",
        MIB,
    );
    let token_dense = scaled_to("x1+=2;y[0]==z.w&&a!=b||c..d::e=>f->g;\n", MIB);
    let inputs = [
        ("representative", scaled_to(REPRESENTATIVE, MIB)),
        ("comment_heavy", comment_heavy),
        ("token_dense", token_dense),
    ];

    let mut group = c.benchmark_group("lex_throughput");
    for (name, text) in inputs {
        let mut map = SourceMap::new();
        let file = map.intern_file("bench.tuo");
        let source = map
            .add_source(file, text.as_str())
            .expect("bench input fits");
        group.throughput(Throughput::Bytes(text.len() as u64));
        group.bench_function(name, |b| b.iter(|| lex(black_box(map.source(source)))));
    }
    group.finish();
}

criterion_group!(benches, bench_throughput);
criterion_main!(benches);
