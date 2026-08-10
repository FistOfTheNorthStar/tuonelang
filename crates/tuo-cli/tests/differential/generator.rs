//! A deterministic generator of small, well-typed programs in the backend's
//! scalar subset.
//!
//! Random differential testing is only useful if every generated program is one
//! both engines are *obliged* to run: accepted by the front end and inside the
//! subset the Cranelift backend lowers. This generator guarantees both **by
//! construction** rather than by generate-and-filter — every program it emits
//! type-checks and stays within integers, arithmetic, comparison, `if`/`else`,
//! direct calls, and (ADR-0004 Stage 2) fixed-capacity `[Int; N]` arrays:
//! literal and repeat construction, constant in-bounds indexing, and `for`
//! folds — including a wide flat literal and the zero-length repeat form. It
//! never emits a float, a string, a growable `Array[T]`, or anything else the
//! backend refuses, so a divergence is always a real correctness finding and
//! never a rejected program.
//!
//! # Determinism
//!
//! The generator is a pure function of its `u64` seed: same seed, same program,
//! on every host and every run. It uses a small SplitMix64 PRNG rather than the
//! `rand` crate or any wall-clock/entropy source, so a failing seed reproduces
//! exactly and the CI gate is stable. Callers sweep a fixed range of seeds.
//!
//! # Termination
//!
//! A generated function may only call functions defined *before* it, so the call
//! graph is a DAG and no recursion is possible — every program halts well within
//! the interpreter's default fuel. (Recursion is exercised by the hand-written
//! `codegen_differential` fixtures, which can bound their own depth; the random
//! corpus trades it away for a guarantee of termination.)

// Test-support module reached via `#[path]`; `pub` is the intended visibility.
#![allow(unreachable_pub)]

use std::fmt::Write as _;

/// A deterministic SplitMix64 PRNG — pure, seedable, host-independent.
struct Rng {
    state: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// The next 64 pseudo-random bits (SplitMix64).
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value in `0..bound` (`bound` must be non-zero).
    fn below(&mut self, bound: u32) -> u32 {
        (self.next_u64() % u64::from(bound)) as u32
    }

    /// A small signed integer literal, biased toward the edge cases that expose
    /// backend/interpreter arithmetic disagreements (0, 1, -1, small magnitudes).
    fn literal(&mut self) -> i64 {
        match self.below(8) {
            0 => 0,
            1 => 1,
            2 => -1,
            3 => 2,
            4 => i64::from(self.below(1000)),
            5 => -i64::from(self.below(1000)),
            6 => i64::from(self.below(64)),
            _ => -i64::from(self.below(64)),
        }
    }
}

/// Configuration bounding the size of a generated program.
struct Bounds {
    /// Maximum expression nesting depth.
    max_depth: u32,
    /// Number of helper functions (besides `main`).
    helpers: u32,
    /// Parameters per helper.
    params: u32,
}

impl Bounds {
    /// Small programs: deep enough to nest branches and calls, small enough to
    /// stay fast and to shrink quickly when one diverges.
    fn small(rng: &mut Rng) -> Self {
        Self {
            max_depth: 3 + rng.below(3), // 3..=5
            helpers: rng.below(3),       // 0..=2
            params: 1 + rng.below(3),    // 1..=3
        }
    }
}

/// The names in scope while generating one function body: its parameters and any
/// `let` locals introduced so far. Every name is `Int`-typed, so referencing any
/// of them is well-typed. `fresh` numbers array/fold bindings (which are *not*
/// `Int`-typed or not in expression scope, so they never enter `names`) uniquely
/// across the whole body, so nested array forms cannot collide or shadow.
struct Scope {
    names: Vec<String>,
    fresh: u32,
}

/// Generate a complete, well-typed program from `seed`.
///
/// The result always defines `fn main() -> Int` and type-checks; every
/// expression is `Int`-typed (comparisons appear only as `if` conditions).
#[must_use]
pub fn program(seed: u64) -> String {
    let mut rng = Rng::new(seed);
    let bounds = Bounds::small(&mut rng);

    let mut out = String::new();
    // Helpers first, each able to call the ones before it (a DAG → terminates).
    let mut callable: Vec<(String, u32)> = Vec::new();
    for index in 0..bounds.helpers {
        let name = format!("f{index}");
        let arity = bounds.params;
        let mut scope = Scope {
            names: (0..arity).map(|p| format!("p{p}")).collect(),
            fresh: 0,
        };
        let params = scope
            .names
            .iter()
            .map(|n| format!("take {n}: Int"))
            .collect::<Vec<_>>()
            .join(", ");
        let body = expr(&mut rng, &mut scope, &callable, bounds.max_depth);
        // `params` already carries the `take` on each parameter.
        writeln!(out, "fn {name}({params}) -> Int {{ {body} }}").expect("string write");
        callable.push((name, arity));
    }

    // `main` takes no parameters and returns Int.
    let mut scope = Scope {
        names: Vec::new(),
        fresh: 0,
    };
    let body = expr(&mut rng, &mut scope, &callable, bounds.max_depth);
    writeln!(out, "fn main() -> Int {{ {body} }}").expect("string write");
    out
}

/// Generate an `Int`-typed expression at the given remaining `depth`.
fn expr(rng: &mut Rng, scope: &mut Scope, callable: &[(String, u32)], depth: u32) -> String {
    // At depth 0 only produce a leaf, so the tree stays bounded.
    if depth == 0 {
        return leaf(rng, scope);
    }
    match rng.below(7) {
        // Leaf: literal or an in-scope name.
        0 => leaf(rng, scope),
        // A fixed-array form (construct + index or construct + fold).
        6 => array_form(rng, scope, callable, depth),
        // Binary arithmetic (may trap; both engines must trap together).
        1 | 2 => {
            let op = ["+", "-", "*", "/", "%"][rng.below(5) as usize];
            let l = expr(rng, scope, callable, depth - 1);
            let r = expr(rng, scope, callable, depth - 1);
            format!("({l} {op} {r})")
        }
        // `if cond { a } else { b }` — the condition is an Int comparison (Bool),
        // both arms Int, so the whole expression is Int-typed.
        3 => {
            let cmp = ["==", "!=", "<", "<=", ">", ">="][rng.below(6) as usize];
            let cl = expr(rng, scope, callable, depth - 1);
            let cr = expr(rng, scope, callable, depth - 1);
            let a = expr(rng, scope, callable, depth - 1);
            let b = expr(rng, scope, callable, depth - 1);
            format!("if {cl} {cmp} {cr} {{ {a} }} else {{ {b} }}")
        }
        // A `let` binding then a body that may use it (introduces a local).
        4 => {
            let value = expr(rng, scope, callable, depth - 1);
            let name = format!("v{}", scope.names.len());
            scope.names.push(name.clone());
            let body = expr(rng, scope, callable, depth - 1);
            scope.names.pop();
            format!("{{ let {name}: Int = {value}; {body} }}")
        }
        // A call to an earlier helper, if any exists; else fall back to a leaf.
        _ => {
            if callable.is_empty() {
                leaf(rng, scope)
            } else {
                let (name, arity) = &callable[rng.below(callable.len() as u32) as usize];
                let args = (0..*arity)
                    .map(|_| expr(rng, scope, callable, depth - 1))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{name}({args})")
            }
        }
    }
}

/// An `Int`-typed fixed-array form (ADR-0004 Stage 2): construct a small
/// `[Int; N]` — literal or repeat form — then either index it at a constant
/// in-bounds position or fold it with a counted `for` loop. Element
/// expressions recurse, so arrays nest inside arithmetic and vice versa; the
/// bindings introduced here are never pushed into `scope.names` (the array is
/// not `Int`-typed and the fold accumulator lives only inside its block), so
/// every emitted reference stays well-typed by construction.
fn array_form(rng: &mut Rng, scope: &mut Scope, callable: &[(String, u32)], depth: u32) -> String {
    let id = scope.fresh;
    scope.fresh += 1;
    match rng.below(4) {
        // Literal construction, constant in-bounds index: always defined.
        0 => {
            let n = 1 + rng.below(4); // 1..=4 elements
            let k = rng.below(n);
            let elements = (0..n)
                .map(|_| expr(rng, scope, callable, depth - 1))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{ let a{id}: [Int; {n}] = [{elements}]; a{id}[{k}] }}")
        }
        // Literal construction folded by a counted `for` loop.
        1 => {
            let n = 1 + rng.below(3); // 1..=3 elements
            let elements = (0..n)
                .map(|_| expr(rng, scope, callable, depth - 1))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "{{ var t{id} = 0; for x{id} in [{elements}] {{ t{id} = t{id} + x{id}; }} t{id} }}"
            )
        }
        // Repeat construction folded by a counted `for` loop. `n == 0` is
        // deliberately in range: the zero-length array still evaluates its
        // element once and the loop body never runs.
        2 => {
            let n = rng.below(4); // 0..=3 repeats
            let value = expr(rng, scope, callable, depth - 1);
            format!(
                "{{ let a{id} = [{value}; {n}]; var t{id} = 0; \
                 for x{id} in a{id} {{ t{id} = t{id} + x{id}; }} t{id} }}"
            )
        }
        // A wide, flat literal (a shallow but broad tree) with a constant
        // in-bounds index, pinning the many-element construction path.
        _ => {
            let n = 5 + rng.below(12); // 5..=16 elements
            let k = rng.below(n);
            let elements = (0..n)
                .map(|_| leaf(rng, scope))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{ let a{id}: [Int; {n}] = [{elements}]; a{id}[{k}] }}")
        }
    }
}

/// A leaf `Int` expression: an in-scope name (when available) or a literal.
fn leaf(rng: &mut Rng, scope: &Scope) -> String {
    if !scope.names.is_empty() && rng.below(2) == 0 {
        scope.names[rng.below(scope.names.len() as u32) as usize].clone()
    } else {
        let value = rng.literal();
        // Parenthesize negatives so they compose without a stray unary-minus
        // parse in argument or operator position.
        if value < 0 {
            format!("({value})")
        } else {
            value.to_string()
        }
    }
}
