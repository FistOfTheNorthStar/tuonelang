//! Expression-precedence tests: each expression's tree shape is asserted as
//! a compact s-expression, so associativity and binding strength are checked
//! structurally, not by eyeballing renders.

use tuo_parser::parse;
use tuo_source::SourceMap;
use tuo_syntax::{SyntaxElement, SyntaxKind, SyntaxNode, SyntaxTree};

/// Parse `text` as a function tail expression and return its s-expression.
fn expr_shape(text: &str) -> String {
    let source_text = format!("fn t() -> Int {{ {text} }}\n");
    let mut map = SourceMap::new();
    let file = map.intern_file("expr.tuo");
    let source = map
        .add_source(file, source_text.as_str())
        .expect("test source fits");
    let result = parse(map.source(source));
    assert!(
        !result.has_errors(),
        "`{text}` must parse cleanly, got {:#?}",
        result.all_diagnostics()
    );
    let blocks = result.tree.root.descendants_of_kind(SyntaxKind::Block);
    let block = blocks.first().expect("function has a block");
    assert_eq!(block.children.len(), 3, "block is {{ expr }}");
    let SyntaxElement::Node(expr) = &block.children[1] else {
        panic!("tail must be a node");
    };
    sexpr(expr, &result.tree, &source_text)
}

fn sexpr(node: &SyntaxNode, tree: &SyntaxTree, text: &str) -> String {
    let mut parts = vec![format!("{:?}", node.kind)];
    for child in &node.children {
        match child {
            SyntaxElement::Node(nested) => parts.push(sexpr(nested, tree, text)),
            SyntaxElement::Token(index) => {
                let range = tree.lex.tokens[*index as usize].range();
                parts.push(format!(
                    "'{}'",
                    &text[range.start().as_usize()..range.end().as_usize()]
                ));
            }
        }
    }
    format!("({})", parts.join(" "))
}

#[test]
fn multiplication_binds_tighter_than_addition() {
    assert_eq!(
        expr_shape("1 + 2 * 3"),
        "(BinaryExpr (Literal '1') '+' (BinaryExpr (Literal '2') '*' (Literal '3')))"
    );
    assert_eq!(
        expr_shape("1 * 2 + 3"),
        "(BinaryExpr (BinaryExpr (Literal '1') '*' (Literal '2')) '+' (Literal '3'))"
    );
}

#[test]
fn additive_operators_are_left_associative() {
    assert_eq!(
        expr_shape("a - b - c"),
        "(BinaryExpr (BinaryExpr (PathExpr 'a') '-' (PathExpr 'b')) '-' (PathExpr 'c'))"
    );
}

#[test]
fn unary_binds_tighter_than_binary() {
    assert_eq!(
        expr_shape("-x * y"),
        "(BinaryExpr (UnaryExpr '-' (PathExpr 'x')) '*' (PathExpr 'y'))"
    );
    assert_eq!(
        expr_shape("move a + b"),
        "(BinaryExpr (UnaryExpr 'move' (PathExpr 'a')) '+' (PathExpr 'b'))"
    );
}

#[test]
fn logical_layers_or_over_and_over_not() {
    assert_eq!(
        expr_shape("!a && b || c"),
        "(BinaryExpr (BinaryExpr (UnaryExpr '!' (PathExpr 'a')) '&&' (PathExpr 'b')) '||' (PathExpr 'c'))"
    );
}

#[test]
fn comparison_sits_between_range_and_logic() {
    assert_eq!(
        expr_shape("a + 1 < b && c"),
        "(BinaryExpr (BinaryExpr (BinaryExpr (PathExpr 'a') '+' (Literal '1')) '<' (PathExpr 'b')) '&&' (PathExpr 'c'))"
    );
}

#[test]
fn range_binds_looser_than_arithmetic() {
    assert_eq!(
        expr_shape("0 .. n + 1"),
        "(RangeExpr (Literal '0') '..' (BinaryExpr (PathExpr 'n') '+' (Literal '1')))"
    );
}

#[test]
fn assignment_is_right_associative_and_loosest() {
    assert_eq!(
        expr_shape("a = b = c || d"),
        "(AssignExpr (PathExpr 'a') '=' (AssignExpr (PathExpr 'b') '=' (BinaryExpr (PathExpr 'c') '||' (PathExpr 'd'))))"
    );
}

#[test]
fn postfix_operators_chain_left_to_right() {
    assert_eq!(
        expr_shape("x.f(1)?.g[0] as Int"),
        "(CastExpr (IndexExpr (FieldExpr (TryExpr (MethodCallExpr (PathExpr 'x') '.' 'f' \
         (CallArgs '(' (Literal '1') ')')) '?') '.' 'g') '[' (Literal '0') ']') 'as' \
         (PathType (TypePath 'Int')))"
    );
}

#[test]
fn grouping_overrides_precedence() {
    assert_eq!(
        expr_shape("(1 + 2) * 3"),
        "(BinaryExpr (GroupExpr '(' (BinaryExpr (Literal '1') '+' (Literal '2')) ')') '*' (Literal '3'))"
    );
}

#[test]
fn comparison_is_non_associative() {
    // `a < b < c` is a parse error by design (grammar (G)).
    let mut map = SourceMap::new();
    let file = map.intern_file("expr.tuo");
    let source = map
        .add_source(file, "fn t() -> Bool { a < b < c }\n")
        .expect("fits");
    let result = parse(map.source(source));
    assert!(result.has_errors(), "chained comparison must not parse");
}

// ----------------------------------------------------------------------------
// ADR-0019 — the bitwise levels.
//
// Precedence, loosest to tightest: `|`, `^`, `&`, the shifts, then the
// arithmetic operators. These are the conventional bindings, so an expression
// copied from C/Rust/Go parses the same way here.
// ----------------------------------------------------------------------------

#[test]
fn shifts_bind_tighter_than_bitwise_and() {
    assert_eq!(
        expr_shape("1 & 2 << 3"),
        "(BinaryExpr (Literal '1') '&' (BinaryExpr (Literal '2') '<<' (Literal '3')))"
    );
}

#[test]
fn bitwise_and_binds_tighter_than_xor_which_binds_tighter_than_or() {
    assert_eq!(
        expr_shape("1 | 2 ^ 3"),
        "(BinaryExpr (Literal '1') '|' (BinaryExpr (Literal '2') '^' (Literal '3')))"
    );
    assert_eq!(
        expr_shape("1 ^ 2 & 3"),
        "(BinaryExpr (Literal '1') '^' (BinaryExpr (Literal '2') '&' (Literal '3')))"
    );
}

#[test]
fn arithmetic_binds_tighter_than_the_shifts() {
    assert_eq!(
        expr_shape("1 << 2 + 3"),
        "(BinaryExpr (Literal '1') '<<' (BinaryExpr (Literal '2') '+' (Literal '3')))"
    );
}

#[test]
fn comparison_binds_looser_than_every_bitwise_operator() {
    // `a & b == c` groups as `(a & b) == c` — the conventional reading, and
    // the reason the bitwise levels sit between comparison and arithmetic.
    assert_eq!(
        expr_shape("1 & 2 == 3"),
        "(BinaryExpr (BinaryExpr (Literal '1') '&' (Literal '2')) '==' (Literal '3'))"
    );
}

#[test]
fn the_bitwise_levels_are_left_associative() {
    assert_eq!(
        expr_shape("1 | 2 | 3"),
        "(BinaryExpr (BinaryExpr (Literal '1') '|' (Literal '2')) '|' (Literal '3'))"
    );
    assert_eq!(
        expr_shape("1 >> 2 >> 3"),
        "(BinaryExpr (BinaryExpr (Literal '1') '>>' (Literal '2')) '>>' (Literal '3'))"
    );
}

#[test]
fn bitwise_not_is_a_prefix_operator() {
    assert_eq!(
        expr_shape("~1 & 2"),
        "(BinaryExpr (UnaryExpr '~' (Literal '1')) '&' (Literal '2'))"
    );
}

/// The load-bearing ADR-0019 disambiguation: `|` is one token serving two
/// roles. In a match arm's *pattern* position it separates alternatives; in
/// any *expression* position it is bitwise or. The two productions are
/// disjoint, so a single program can use both spellings without ambiguity —
/// if this ever regressed, `1 | 2` in a pattern would start parsing as an
/// expression and every match-with-alternatives program would break.
#[test]
fn pipe_is_pattern_alternation_in_patterns_and_bitwise_or_in_expressions() {
    let source = "fn t(take n: Int) -> Int {\n\
                  match n {\n\
                      1 | 2 => n | 8,\n\
                      _ => 0,\n\
                  }\n\
                  }\n";
    let mut map = SourceMap::new();
    let file = map.intern_file("pipe.tuo");
    let source_id = map.add_source(file, source).expect("test source fits");
    let result = parse(map.source(source_id));
    assert!(
        !result.has_errors(),
        "a program mixing both roles of `|` must parse cleanly, got {:#?}",
        result.all_diagnostics()
    );

    // The arm's pattern is an alternation node, NOT a binary expression...
    let alts = result
        .tree
        .root
        .descendants_of_kind(SyntaxKind::OrPattern)
        .len();
    assert_eq!(alts, 1, "`1 | 2` in pattern position must be one OrPattern");

    // ...while the arm's body is a binary expression using the same token.
    let binaries = result
        .tree
        .root
        .descendants_of_kind(SyntaxKind::BinaryExpr)
        .len();
    assert_eq!(
        binaries, 1,
        "`n | 8` in expression position must be one BinaryExpr"
    );
}
