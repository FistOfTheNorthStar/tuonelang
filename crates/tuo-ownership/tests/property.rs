//! Property tests over simple control-flow combinations.
//!
//! Exhaustively enumerates every small program over a fixed alphabet of
//! ownership-relevant statements (move, read, reinitialize-from-a-spare)
//! composed through `if`/`else` and `while`, and checks the **soundness
//! direction** of the checker against a path-enumeration oracle:
//!
//! > if the ownership checker accepts a program, then no dynamic execution
//! > path of that program uses a moved or unavailable value.
//!
//! The converse is *not* asserted — the v0 model is deliberately
//! conservative (no hidden drop flags, loop back-edge joins), so it rejects
//! some dynamically safe programs. The suite also asserts the checker is
//! deterministic and that the enumeration is non-vacuous (it must contain
//! both accepted and rejected programs).
//!
//! The oracle enumerates every path: both branches of each `if`, and 0, 1,
//! or 2 iterations of each `while` — enough to expose every loop-carried
//! misuse expressible at this program size.

use std::collections::BTreeSet;

use tuo_ast::Ast;
use tuo_source::SourceMap;

/// One statement of the generated mini-language, operating on an owned box
/// `x` and two one-shot spares `s1`/`s2`.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Stmt {
    /// `consume(x);` — moves `x`.
    Consume,
    /// `peek(x);` — reads `x`.
    Peek,
    /// `x = sN;` — reinitializes `x` by moving spare `N` (0-indexed).
    Reinit(usize),
    /// `if flag { … } else { … }`.
    If(Vec<Stmt>, Vec<Stmt>),
    /// `while flag { … }`.
    While(Vec<Stmt>),
}

const SPARES: usize = 2;

/// Statement cost: every node costs 1.
fn cost(stmt: &Stmt) -> usize {
    match stmt {
        Stmt::Consume | Stmt::Peek | Stmt::Reinit(_) => 1,
        Stmt::If(a, b) => 1 + list_cost(a) + list_cost(b),
        Stmt::While(a) => 1 + list_cost(a),
    }
}

fn list_cost(stmts: &[Stmt]) -> usize {
    stmts.iter().map(cost).sum()
}

/// Every single statement of cost <= `budget`.
fn gen_stmts(budget: usize) -> Vec<Stmt> {
    let mut out = Vec::new();
    if budget == 0 {
        return out;
    }
    out.push(Stmt::Consume);
    out.push(Stmt::Peek);
    for spare in 0..SPARES {
        out.push(Stmt::Reinit(spare));
    }
    for split in 0..budget {
        for then in gen_lists(split) {
            if list_cost(&then) != split {
                continue;
            }
            for els in gen_lists(budget - 1 - split) {
                out.push(Stmt::If(then.clone(), els));
            }
        }
    }
    for body in gen_lists(budget - 1) {
        out.push(Stmt::While(body));
    }
    out
}

/// Every statement list of cost <= `budget` (including the empty list).
fn gen_lists(budget: usize) -> Vec<Vec<Stmt>> {
    let mut out = vec![Vec::new()];
    for head_cost in 1..=budget {
        for head in gen_stmts(head_cost) {
            if cost(&head) != head_cost {
                continue;
            }
            for tail in gen_lists(budget - head_cost) {
                let mut list = vec![head.clone()];
                list.extend(tail);
                out.push(list);
            }
        }
    }
    out
}

// ----------------------------------------------------------------------
// The oracle: exhaustive dynamic-path enumeration
// ----------------------------------------------------------------------

/// The dynamic availability of the tracked values on one path.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct State {
    x: bool,
    spares: [bool; SPARES],
}

fn exec(stmts: &[Stmt], state: State, safe: &mut bool) -> BTreeSet<State> {
    let mut states = BTreeSet::from([state]);
    for stmt in stmts {
        let mut next = BTreeSet::new();
        for current in states {
            next.extend(step(stmt, current, safe));
        }
        states = next;
    }
    states
}

fn step(stmt: &Stmt, mut state: State, safe: &mut bool) -> BTreeSet<State> {
    match stmt {
        Stmt::Consume => {
            if !state.x {
                *safe = false;
            }
            state.x = false;
            BTreeSet::from([state])
        }
        Stmt::Peek => {
            if !state.x {
                *safe = false;
            }
            BTreeSet::from([state])
        }
        Stmt::Reinit(spare) => {
            if !state.spares[*spare] {
                *safe = false;
            }
            state.spares[*spare] = false;
            state.x = true;
            BTreeSet::from([state])
        }
        Stmt::If(then, els) => {
            let mut out = exec(then, state, safe);
            out.extend(exec(els, state, safe));
            out
        }
        Stmt::While(body) => {
            // 0, 1, or 2 iterations — the second exposes every loop-carried
            // misuse expressible at this size.
            let mut out = BTreeSet::from([state]);
            let once = exec(body, state, safe);
            for mid in &once {
                out.extend(exec(body, *mid, safe));
            }
            out.extend(once);
            out
        }
    }
}

// ----------------------------------------------------------------------
// Rendering and checking
// ----------------------------------------------------------------------

fn render(stmts: &[Stmt], indent: usize, out: &mut String) {
    let pad = "    ".repeat(indent);
    for stmt in stmts {
        match stmt {
            Stmt::Consume => out.push_str(&format!("{pad}consume(x);\n")),
            Stmt::Peek => out.push_str(&format!("{pad}peek(x);\n")),
            Stmt::Reinit(spare) => out.push_str(&format!("{pad}x = s{};\n", spare + 1)),
            Stmt::If(then, els) => {
                out.push_str(&format!("{pad}if flag {{\n"));
                render(then, indent + 1, out);
                out.push_str(&format!("{pad}}} else {{\n"));
                render(els, indent + 1, out);
                out.push_str(&format!("{pad}}}\n"));
            }
            Stmt::While(body) => {
                out.push_str(&format!("{pad}while flag {{\n"));
                render(body, indent + 1, out);
                out.push_str(&format!("{pad}}}\n"));
            }
        }
    }
}

fn source_of(stmts: &[Stmt]) -> String {
    let mut body = String::new();
    render(stmts, 1, &mut body);
    format!(
        "fn consume(take b: Box[Int]) {{ }}\n\
         fn peek(in b: Box[Int]) {{ }}\n\
         fn case(take x: Box[Int], take s1: Box[Int], take s2: Box[Int], in flag: Bool) {{\n\
         {body}}}\n"
    )
}

/// Run the full pipeline; the generated programs must be front-end clean by
/// construction, so any front-end diagnostic is a generator bug.
fn ownership_diagnostics(source: &str) -> Vec<String> {
    let mut map = SourceMap::new();
    let file = map.intern_file("generated.tuo");
    let id = map.add_source(file, source).expect("source fits");
    let parse = tuo_parser::parse(map.source(id));
    assert_eq!(
        parse.diagnostics,
        vec![],
        "generator produced a parse error:\n{source}"
    );
    let asts = [Ast::new(&parse.tree, source)];
    let resolution = tuo_resolve::resolve(&asts);
    assert_eq!(
        resolution.diagnostics(),
        &[],
        "generator produced a resolution error:\n{source}"
    );
    let types = tuo_types::check(&asts, &resolution);
    assert_eq!(
        types.diagnostics(),
        &[],
        "generator produced a type error:\n{source}"
    );
    tuo_ownership::check(&asts, &resolution, &types)
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code.to_string())
        .collect()
}

#[test]
fn accepted_control_flow_programs_are_dynamically_safe() {
    let programs = gen_lists(4);
    assert!(
        programs.len() > 1_000,
        "enumeration shrank unexpectedly ({} programs)",
        programs.len()
    );
    let mut accepted = 0usize;
    let mut rejected = 0usize;
    for (index, program) in programs.iter().enumerate() {
        let source = source_of(program);
        let diagnostics = ownership_diagnostics(&source);
        if index % 97 == 0 {
            // Determinism spot-check: same input, same output.
            assert_eq!(
                diagnostics,
                ownership_diagnostics(&source),
                "checker is nondeterministic on:\n{source}"
            );
        }
        let mut safe = true;
        exec(
            program,
            State {
                x: true,
                spares: [true; SPARES],
            },
            &mut safe,
        );
        if diagnostics.is_empty() {
            accepted += 1;
            assert!(
                safe,
                "UNSOUND: the checker accepted a program with a dynamically unsafe path:\n\
                 {source}"
            );
        } else {
            rejected += 1;
        }
    }
    // Non-vacuousness: the sweep must exercise both outcomes.
    assert!(
        accepted > 100,
        "suspiciously few accepted programs ({accepted})"
    );
    assert!(
        rejected > 100,
        "suspiciously few rejected programs ({rejected})"
    );
}
