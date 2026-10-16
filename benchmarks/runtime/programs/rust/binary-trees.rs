//! binary-trees — the equivalent-semantics Rust peer for the tuonelang
//! binary-trees workload (the Benchmarks Game's binary-trees): a stretch tree
//! of depth MAX + 1, a long-lived tree of depth MAX, and 2^(MAX - d + 4) trees
//! of each depth d = 4, 6, ..., MAX, every node its own `Box`. A walk counts a
//! tree's nodes and consumes it, freeing each node as it is counted, exactly
//! as the tuonelang walk does. Exit byte: the summed node counts % 256.

const MIN_DEPTH: i64 = 4;
const MAX_DEPTH: i64 = 16;

struct Node {
    left: Option<Box<Node>>,
    right: Option<Box<Node>>,
}

fn make(depth: i64) -> Node {
    if depth == 0 {
        return Node { left: None, right: None };
    }
    Node {
        left: Some(Box::new(make(depth - 1))),
        right: Some(Box::new(make(depth - 1))),
    }
}

fn check(node: Node) -> i64 {
    let left = node.left.map_or(0, |b| check(*b));
    let right = node.right.map_or(0, |b| check(*b));
    1 + left + right
}

fn main() {
    let mut total = check(make(MAX_DEPTH + 1));
    let long_lived = make(MAX_DEPTH);
    let mut depth = MIN_DEPTH;
    while depth <= MAX_DEPTH {
        let iterations = 1i64 << (MAX_DEPTH - depth + MIN_DEPTH);
        for _ in 0..iterations {
            total += check(make(depth));
        }
        depth += 2;
    }
    total += check(long_lived);
    std::process::exit((total % 256) as i32);
}
