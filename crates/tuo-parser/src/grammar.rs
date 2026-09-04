//! The Chumsky grammar: `specification/grammar.ebnf` sections (B)–(I).
//!
//! Nothing Chumsky-specific escapes this module — the crate's public surface
//! speaks [`tuo_syntax`] and [`tuo_diagnostics`] types only.
//!
//! # Shape
//!
//! Each production builds a [`SyntaxNode`] whose children are the matched
//! tokens (by index into the lossless stream) and sub-nodes, in source
//! order — nothing matched is dropped, which is what makes the resulting
//! tree reconstructable.
//!
//! # Error recovery
//!
//! Recovery is encoded as ordinary low-priority grammar alternatives that
//! skip to the deliberate synchronization points of the language (`;`, `}`,
//! and item-leading keywords — grammar NOTES §2), collect the skipped tokens
//! under a [`SyntaxKind::Error`] node, and emit a diagnostic via `validate`.
//! Because braces and semicolons are mandatory in tuonelang, these points are
//! reliable; recovery never guesses from indentation.

use chumsky::prelude::*;

use tuo_lexer::TokenKind as K;
use tuo_syntax::{SyntaxElement, SyntaxKind, SyntaxNode};

use crate::stream::{Tok, kind_name, t};

/// The parser input: the trivia-stripped token stream.
pub(crate) type Stream<'a> = &'a [Tok];
/// The chumsky extra bundle: rich errors over [`Tok`].
pub(crate) type Extra<'a> = extra::Err<Rich<'a, Tok>>;
/// A boxed sub-parser producing one syntax element.
type BP<'a> = Boxed<'a, 'a, Stream<'a>, SyntaxElement, Extra<'a>>;

/// Anything that can pour its matched elements into a child list, in order.
/// Implemented for elements, options, vectors, and (nested) pairs so that
/// `a.then(b).then(c)`'s output flattens without hand-written plumbing.
trait Els {
    fn collect_into(self, out: &mut Vec<SyntaxElement>);
}

impl Els for SyntaxElement {
    fn collect_into(self, out: &mut Vec<SyntaxElement>) {
        out.push(self);
    }
}

impl<T: Els> Els for Option<T> {
    fn collect_into(self, out: &mut Vec<SyntaxElement>) {
        if let Some(inner) = self {
            inner.collect_into(out);
        }
    }
}

impl<T: Els> Els for Vec<T> {
    fn collect_into(self, out: &mut Vec<SyntaxElement>) {
        for item in self {
            item.collect_into(out);
        }
    }
}

impl<A: Els, B: Els> Els for (A, B) {
    fn collect_into(self, out: &mut Vec<SyntaxElement>) {
        self.0.collect_into(out);
        self.1.collect_into(out);
    }
}

/// Flatten anything [`Els`] into a child vector.
fn children(els: impl Els) -> Vec<SyntaxElement> {
    let mut out = Vec::new();
    els.collect_into(&mut out);
    out
}

/// Build a node element from flattened children.
fn make(kind: SyntaxKind, els: impl Els) -> SyntaxElement {
    SyntaxElement::Node(SyntaxNode::new(kind, children(els)))
}

/// Wrap a parser's (flattenable) output in a node of `kind`.
macro_rules! node {
    ($kind:expr, $parser:expr $(,)?) => {
        $parser.map(move |els| make($kind, els))
    };
}

/// A single token of `kind` as a syntax element.
///
/// Matches by kind but outputs the **matched input token**, whose index
/// points at the real token in the lossless stream (`just` would output the
/// comparison pattern instead, losing the index).
fn tok<'a>(kind: K) -> impl Parser<'a, Stream<'a>, SyntaxElement, Extra<'a>> + Clone {
    any()
        .filter(move |matched: &Tok| matched.kind == kind)
        .map(elt)
        .labelled(kind_name(kind))
}

/// Tok → element.
fn elt(matched: Tok) -> SyntaxElement {
    SyntaxElement::Token(matched.index)
}

/// `item ("," item)* (",")?` — a possibly-empty comma-separated list that
/// **keeps** its separator tokens (losslessness forbids `separated_by`).
fn comma_list<'a>(item: BP<'a>) -> Boxed<'a, 'a, Stream<'a>, Vec<SyntaxElement>, Extra<'a>> {
    item.clone()
        .then(
            tok(K::Comma)
                .then(item)
                .repeated()
                .collect::<Vec<_>>()
                .then(tok(K::Comma).or_not()),
        )
        .map(children)
        .or_not()
        .map(Option::unwrap_or_default)
        .boxed()
}

/// Like [`comma_list`], but the list must have at least one item.
fn comma_list1<'a>(item: BP<'a>) -> Boxed<'a, 'a, Stream<'a>, Vec<SyntaxElement>, Extra<'a>> {
    item.clone()
        .then(
            tok(K::Comma)
                .then(item)
                .repeated()
                .collect::<Vec<_>>()
                .then(tok(K::Comma).or_not()),
        )
        .map(children)
        .boxed()
}

/// The item-leading token kinds (grammar NOTES §2): recovery stops before
/// these so a broken item cannot swallow the next one.
pub(crate) const ITEM_START: [K; 12] = [
    K::DocComment,
    K::Hash,
    K::KwPub,
    K::KwImport,
    K::KwFn,
    K::KwStruct,
    K::KwEnum,
    K::KwInterface,
    K::KwImpl,
    K::KwConst,
    K::KwSpec,
    K::KwModule,
];

/// The complete grammar: a parser for one source file.
pub(crate) fn parser<'a>() -> Boxed<'a, 'a, Stream<'a>, SyntaxNode, Extra<'a>> {
    // ----- types (D) ------------------------------------------------------
    let ty = recursive(|ty| {
        let ty: BP<'a> = ty.boxed();
        let type_path = type_path();
        let type_args = type_args(ty.clone());
        let unit_type = node!(
            SyntaxKind::UnitType,
            tok(K::OpenParen).then(tok(K::CloseParen))
        );
        let wrapper = node!(
            SyntaxKind::WrapperType,
            one_of([t(K::KwBox), t(K::KwShared), t(K::KwWeak)])
                .map(elt)
                .then(type_args.clone()),
        );
        let path_type = node!(SyntaxKind::PathType, type_path.then(type_args.or_not()),);
        // `[T; N]` — the fixed-capacity array type; `[` uniquely selects it
        // at type position (ADR-0004 Stage 2).
        let fixed_array = node!(
            SyntaxKind::FixedArrayType,
            tok(K::OpenBracket)
                .then(ty.clone())
                .then(tok(K::Semi))
                .then(tok(K::IntLiteral))
                .then(tok(K::CloseBracket)),
        );
        // `fn(mode T, …) -> R` — the function type (ADR-0008 Tier 1). Modes
        // are mandatory; the return arrow+type are always written and wrapped
        // in a `ReturnType` node, mirroring a function declaration's.
        let fn_mode = one_of([t(K::KwIn), t(K::KwMut), t(K::KwTake)]).map(elt);
        let fn_type_param = node!(SyntaxKind::FnTypeParam, fn_mode.then(ty.clone())).boxed();
        let fn_return_type =
            node!(SyntaxKind::ReturnType, tok(K::ThinArrow).then(ty.clone())).boxed();
        let fn_type = node!(
            SyntaxKind::FnType,
            tok(K::KwFn)
                .then(tok(K::OpenParen))
                .then(comma_list(fn_type_param))
                .then(tok(K::CloseParen))
                .then(fn_return_type),
        );
        choice((unit_type, wrapper, fixed_array, fn_type, path_type))
    })
    .boxed();
    let type_args_p = type_args(ty.clone());

    // ----- patterns (H) ---------------------------------------------------
    let pattern = recursive(|pat| {
        let pat: BP<'a> = pat.boxed();
        let literal_pat = node!(
            SyntaxKind::LiteralPattern,
            choice((
                one_of([
                    t(K::IntLiteral),
                    t(K::FloatLiteral),
                    t(K::BoolLiteral),
                    t(K::CharLiteral),
                    t(K::StringLiteral),
                ])
                .map(elt)
                .map(children),
                tok(K::OpenParen).then(tok(K::CloseParen)).map(children),
            )),
        );
        let wildcard = node!(SyntaxKind::WildcardPattern, tok(K::Underscore));
        let field_pattern = node!(
            SyntaxKind::FieldPattern,
            tok(K::Ident).then(tok(K::Colon).then(pat.clone()).or_not()),
        );
        let rest = node!(SyntaxKind::RestPattern, tok(K::DotDot));
        // `{ .. }` | `{ field (, field)* (, ..)? ,? }` | `{ }`
        let field_block_pattern = node!(
            SyntaxKind::FieldPatternBlock,
            tok(K::OpenBrace)
                .then(
                    choice((
                        rest.clone().then(tok(K::Comma).or_not()).map(children),
                        field_pattern
                            .clone()
                            .then(
                                tok(K::Comma)
                                    .then(field_pattern)
                                    .repeated()
                                    .collect::<Vec<_>>(),
                            )
                            .then(tok(K::Comma).then(rest).or_not())
                            .then(tok(K::Comma).or_not())
                            .map(children),
                    ))
                    .or_not(),
                )
                .then(tok(K::CloseBrace)),
        );
        // A path with a field block, a multi-segment path, or (last) a
        // single-identifier binding.
        let path_with_block = node!(
            SyntaxKind::PathPattern,
            type_path().then(field_block_pattern),
        );
        let multi_segment = node!(
            SyntaxKind::PathPattern,
            choice((tok(K::KwSelfType), tok(K::Ident))).then(
                tok(K::ColonColon)
                    .then(tok(K::Ident))
                    .repeated()
                    .at_least(1)
                    .collect::<Vec<_>>(),
            ),
        );
        let binding = node!(SyntaxKind::BindingPattern, tok(K::Ident));
        let group = node!(
            SyntaxKind::GroupPattern,
            tok(K::OpenParen).then(pat).then(tok(K::CloseParen)),
        );
        let primary = choice((
            literal_pat,
            wildcard,
            path_with_block,
            multi_segment,
            binding,
            group,
        ))
        .boxed();
        // Or-patterns: only wrap when there is a `|`.
        primary
            .clone()
            .then(tok(K::Pipe).then(primary).repeated().collect::<Vec<_>>())
            .map(|(first, rest)| {
                if rest.is_empty() {
                    first
                } else {
                    make(SyntaxKind::OrPattern, (first, rest))
                }
            })
    })
    .boxed();

    // ----- expressions (G): three mutually recursive roots ----------------
    let mut expr = Recursive::declare();
    let mut expr_ns = Recursive::declare();
    let mut block = Recursive::declare();
    let expr_b: BP<'a> = expr.clone().boxed();
    let expr_ns_b: BP<'a> = expr_ns.clone().boxed();
    let block_b: BP<'a> = block.clone().boxed();

    let label = tok(K::LifetimeLabel);

    let if_expr = recursive(|ife| {
        node!(
            SyntaxKind::IfExpr,
            tok(K::KwIf)
                .then(expr_ns_b.clone())
                .then(block_b.clone())
                .then(
                    tok(K::KwElse)
                        .then(choice((ife.boxed(), block_b.clone())))
                        .or_not(),
                ),
        )
    })
    .boxed();
    let while_expr = node!(
        SyntaxKind::WhileExpr,
        tok(K::KwWhile)
            .then(expr_ns_b.clone())
            .then(block_b.clone()),
    );
    let loop_expr = node!(
        SyntaxKind::LoopExpr,
        label
            .clone()
            .then(tok(K::Colon))
            .or_not()
            .then(tok(K::KwLoop))
            .then(block_b.clone()),
    );
    let for_expr = node!(
        SyntaxKind::ForExpr,
        tok(K::KwFor)
            .then(pattern.clone())
            .then(tok(K::KwIn))
            .then(expr_ns_b.clone())
            .then(block_b.clone()),
    );
    let unsafe_expr = node!(
        SyntaxKind::UnsafeExpr,
        tok(K::KwUnsafe).then(block_b.clone())
    );
    let match_arm = node!(
        SyntaxKind::MatchArm,
        pattern
            .clone()
            .then(tok(K::KwIf).then(expr_b.clone()).or_not())
            .then(tok(K::FatArrow))
            .then(expr_b.clone()),
    );
    let match_expr = node!(
        SyntaxKind::MatchExpr,
        tok(K::KwMatch)
            .then(expr_ns_b.clone())
            .then(tok(K::OpenBrace))
            .then(comma_list(match_arm.boxed()))
            .then(tok(K::CloseBrace)),
    );
    let block_expression = choice((
        block_b.clone(),
        if_expr,
        match_expr,
        while_expr,
        loop_expr,
        for_expr,
        unsafe_expr,
    ))
    .boxed();

    let literal = node!(
        SyntaxKind::Literal,
        one_of([
            t(K::IntLiteral),
            t(K::FloatLiteral),
            t(K::BoolLiteral),
            t(K::CharLiteral),
            t(K::StringLiteral),
        ])
        .map(elt),
    );
    let unit_literal = node!(
        SyntaxKind::Literal,
        tok(K::OpenParen).then(tok(K::CloseParen))
    );
    let turbofish = node!(
        SyntaxKind::Turbofish,
        tok(K::ColonColon).then(type_args_p.clone()),
    );
    let path_expr = node!(
        SyntaxKind::PathExpr,
        choice((tok(K::KwSelfType), tok(K::KwSelfValue), tok(K::Ident)))
            .then(
                tok(K::ColonColon)
                    .then(tok(K::Ident))
                    .repeated()
                    .collect::<Vec<_>>(),
            )
            .then(turbofish.or_not()),
    );
    let field_init = node!(
        SyntaxKind::FieldInit,
        tok(K::Ident).then(tok(K::Colon).then(expr_b.clone()).or_not()),
    );
    let struct_literal = node!(
        SyntaxKind::StructLiteral,
        type_path()
            .then(type_args_p.clone().or_not())
            .then(tok(K::OpenBrace))
            .then(comma_list(field_init.boxed()))
            .then(tok(K::CloseBrace)),
    );
    let group_expr = node!(
        SyntaxKind::GroupExpr,
        tok(K::OpenParen)
            .then(expr_b.clone())
            .then(tok(K::CloseParen)),
    );
    // `[a, b, c]` / `[x; N]` — a `[` at primary position can only open an
    // array literal (indexing is a postfix op). Repeat form first, exactly
    // as the handwritten engine speculates (ADR-0004 Stage 2).
    let array_literal = node!(
        SyntaxKind::ArrayLiteral,
        tok(K::OpenBracket)
            .then(choice((
                expr_b
                    .clone()
                    .then(tok(K::Semi))
                    .then(tok(K::IntLiteral))
                    .map(children)
                    .boxed(),
                comma_list(expr_b.clone()),
            )))
            .then(tok(K::CloseBracket)),
    );
    let return_expr = node!(
        SyntaxKind::ReturnExpr,
        tok(K::KwReturn).then(expr_b.clone().or_not()),
    );
    let break_expr = node!(
        SyntaxKind::BreakExpr,
        tok(K::KwBreak)
            .then(label.clone().or_not())
            .then(expr_b.clone().or_not()),
    );
    let continue_expr = node!(
        SyntaxKind::ContinueExpr,
        tok(K::KwContinue).then(label.or_not()),
    );

    let primary_full: BP<'a> = choice((
        literal,
        unit_literal.clone(),
        struct_literal.clone(),
        array_literal.clone(),
        path_expr.clone(),
        group_expr.clone(),
        block_expression.clone(),
        return_expr.clone(),
        break_expr.clone(),
        continue_expr.clone(),
    ))
    .boxed();
    let primary_ns: BP<'a> = choice((
        literal,
        unit_literal,
        array_literal,
        path_expr,
        group_expr,
        block_expression.clone(),
        return_expr,
        break_expr,
        continue_expr,
    ))
    .boxed();

    // Postfix operators (shared by both roots; nested positions like call
    // arguments and index expressions re-allow struct literals).
    enum Post {
        Try(SyntaxElement),
        Member(SyntaxElement, SyntaxElement, Option<SyntaxElement>),
        Call(SyntaxElement),
        Index(SyntaxElement, SyntaxElement, SyntaxElement),
        Cast(SyntaxElement, SyntaxElement),
    }
    let call_args = node!(
        SyntaxKind::CallArgs,
        tok(K::OpenParen)
            .then(comma_list(expr_b.clone()))
            .then(tok(K::CloseParen)),
    )
    .boxed();
    let post_op = choice((
        tok(K::Question).map(Post::Try),
        tok(K::Dot)
            .then(choice((tok(K::Ident), tok(K::IntLiteral))))
            .then(call_args.clone().or_not())
            .map(|((dot, name), args)| Post::Member(dot, name, args)),
        call_args.map(Post::Call),
        tok(K::OpenBracket)
            .then(expr_b.clone())
            .then(tok(K::CloseBracket))
            .map(|((open, index), close)| Post::Index(open, index, close)),
        tok(K::KwAs)
            .then(ty.clone())
            .map(|(kw, ty)| Post::Cast(kw, ty)),
    ))
    .boxed();
    let apply_post = |lhs: SyntaxElement, op: Post| match op {
        Post::Try(q) => make(SyntaxKind::TryExpr, (lhs, q)),
        Post::Member(dot, name, Some(args)) => {
            make(SyntaxKind::MethodCallExpr, (lhs, (dot, (name, args))))
        }
        Post::Member(dot, name, None) => make(SyntaxKind::FieldExpr, (lhs, (dot, name))),
        Post::Call(args) => make(SyntaxKind::CallExpr, (lhs, args)),
        Post::Index(open, index, close) => {
            make(SyntaxKind::IndexExpr, (lhs, (open, (index, close))))
        }
        Post::Cast(kw, ty) => make(SyntaxKind::CastExpr, (lhs, (kw, ty))),
    };

    // The precedence tower (G), assembled once per primary flavour.
    let tower = |primary: BP<'a>| -> BP<'a> {
        let postfix = primary
            .foldl(post_op.clone().repeated(), apply_post)
            .boxed();
        let unary = recursive(|unary| {
            choice((
                node!(
                    SyntaxKind::UnaryExpr,
                    one_of([t(K::Minus), t(K::Bang), t(K::Tilde), t(K::KwMove)])
                        .map(elt)
                        .then(unary),
                ),
                postfix,
            ))
        })
        .boxed();
        let binary = |atom: BP<'a>, ops: Vec<Tok>| -> BP<'a> {
            atom.clone()
                .foldl(
                    one_of(ops).map(elt).then(atom).repeated(),
                    |lhs, (op, rhs)| make(SyntaxKind::BinaryExpr, (lhs, (op, rhs))),
                )
                .boxed()
        };
        let mul = binary(unary, vec![t(K::Star), t(K::Slash), t(K::Percent)]);
        let add = binary(mul, vec![t(K::Plus), t(K::Minus)]);
        let range = add
            .clone()
            .then(tok(K::DotDot).then(add).or_not())
            .map(|(lhs, rhs)| match rhs {
                None => lhs,
                Some(rest) => make(SyntaxKind::RangeExpr, (lhs, rest)),
            })
            .boxed();
        // ADR-0019's bitwise levels sit between the shifts and comparison,
        // tightest first: `<<`/`>>`, then `&`, then `^`, then `|`. Must match
        // `handwritten::shift_expr`..`bit_or_expr` exactly — the oracle
        // differential compares the two parsers' trees.
        let shift = binary(range, vec![t(K::LtLt), t(K::GtGt)]);
        let bit_and = binary(shift, vec![t(K::Amp)]);
        let bit_xor = binary(bit_and, vec![t(K::Caret)]);
        let range = binary(bit_xor, vec![t(K::Pipe)]);
        // Comparison is non-associative: `a < b < c` is a parse error.
        let compare = range
            .clone()
            .then(
                one_of([
                    t(K::EqEq),
                    t(K::NotEq),
                    t(K::Lt),
                    t(K::Gt),
                    t(K::LtEq),
                    t(K::GtEq),
                ])
                .map(elt)
                .then(range)
                .or_not(),
            )
            .map(|(lhs, rhs)| match rhs {
                None => lhs,
                Some(rest) => make(SyntaxKind::BinaryExpr, (lhs, rest)),
            })
            .boxed();
        let and = binary(compare, vec![t(K::AmpAmp)]);
        let or = binary(and, vec![t(K::PipePipe)]);
        // Assignment is right-associative.
        recursive(|assign| {
            or.clone()
                .then(tok(K::Eq).then(assign).or_not())
                .map(|(lhs, rhs)| match rhs {
                    None => lhs,
                    Some(rest) => make(SyntaxKind::AssignExpr, (lhs, rest)),
                })
        })
        .boxed()
    };
    expr.define(tower(primary_full));
    expr_ns.define(tower(primary_ns));

    // ----- statements & blocks (F) ----------------------------------------
    let const_item = node!(
        SyntaxKind::ConstItem,
        tok(K::KwConst)
            .then(tok(K::Ident))
            .then(tok(K::Colon))
            .then(ty.clone())
            .then(tok(K::Eq))
            .then(expr_b.clone())
            .then(tok(K::Semi)),
    )
    .boxed();
    let binding_stmt = |kw: K, kind: SyntaxKind| {
        node!(
            kind,
            tok(kw)
                .then(pattern.clone())
                .then(tok(K::Colon).then(ty.clone()).or_not())
                .then(tok(K::Eq).then(expr_b.clone()).or_not())
                .then(tok(K::Semi)),
        )
        .boxed()
    };
    let let_stmt = binding_stmt(K::KwLet, SyntaxKind::LetStatement);
    let var_stmt = binding_stmt(K::KwVar, SyntaxKind::VarStatement);
    let empty_stmt = node!(SyntaxKind::EmptyStatement, tok(K::Semi)).boxed();
    let expr_stmt = choice((
        node!(SyntaxKind::ExpressionStatement, block_expression.clone()),
        node!(
            SyntaxKind::ExpressionStatement,
            expr_b.clone().then(tok(K::Semi))
        ),
    ))
    .boxed();
    let statement = choice((
        let_stmt.clone(),
        var_stmt.clone(),
        const_item.clone(),
        empty_stmt,
        expr_stmt.clone(),
    ))
    .boxed();

    // Statement-level recovery. Two flavours around the tail-expression
    // ambiguity: inside the statement loop, skipped tokens must end at a
    // consumed `;` (otherwise a valid tail expression would be swallowed);
    // in tail position, they run up to (not including) the closing `}`.
    let skipped = any()
        .filter(|tk: &Tok| tk.kind != K::Semi && tk.kind != K::CloseBrace)
        .repeated()
        .at_least(1)
        .collect::<Vec<Tok>>()
        .boxed();
    let report = |skipped: &[Tok],
                  span: SimpleSpan,
                  emitter: &mut chumsky::input::Emitter<Rich<'a, Tok>>| {
        emitter.emit(Rich::custom(
            span,
            format!(
                "malformed statement: skipped {} token(s) starting with {}",
                skipped.len(),
                kind_name(skipped[0].kind),
            ),
        ));
    };
    let stmt_recovery_semi = skipped
        .clone()
        .then(tok(K::Semi))
        .validate(move |(tokens, semi), extra, emitter| {
            report(&tokens, extra.span(), emitter);
            let mut els: Vec<SyntaxElement> = tokens.into_iter().map(elt).collect();
            els.push(semi);
            make(SyntaxKind::Error, els)
        })
        .boxed();
    let stmt_recovery_tail = skipped
        .clone()
        .validate(move |tokens, extra, emitter| {
            report(&tokens, extra.span(), emitter);
            make(
                SyntaxKind::Error,
                tokens.into_iter().map(elt).collect::<Vec<_>>(),
            )
        })
        .boxed();
    // Spec bodies have no tail expression, so one recovery flavour suffices:
    // skip to `;` (consumed) or stop before `}`.
    let stmt_recovery = skipped
        .then(tok(K::Semi).or_not())
        .validate(move |(tokens, semi), extra, emitter| {
            report(&tokens, extra.span(), emitter);
            let mut els: Vec<SyntaxElement> = tokens.into_iter().map(elt).collect();
            els.extend(semi);
            make(SyntaxKind::Error, els)
        })
        .boxed();

    // Block contents, iteratively: statements (with recovery), then an
    // optional tail expression, which must be immediately followed by the
    // closing `}` (checked with a non-consuming lookahead).
    let tail_expr = expr_b
        .clone()
        .then_ignore(any().filter(|tk: &Tok| tk.kind == K::CloseBrace).rewind());
    let block_inner = choice((statement.clone(), stmt_recovery_semi))
        .repeated()
        .collect::<Vec<_>>()
        .then(choice((tail_expr, stmt_recovery_tail)).or_not())
        .boxed();
    block.define(node!(
        SyntaxKind::Block,
        tok(K::OpenBrace).then(block_inner).then(tok(K::CloseBrace)),
    ));

    // ----- declarations (E) -----------------------------------------------
    let docs = tok(K::DocComment).repeated().collect::<Vec<_>>();
    // ADR-0020 Stage C: `#[IDENT]`. Recognizing the name is the checker's
    // job, so the grammar accepts any identifier here.
    let attribute = node!(
        SyntaxKind::Attribute,
        tok(K::Hash)
            .then(tok(K::OpenBracket))
            .then(tok(K::Ident))
            .then(tok(K::CloseBracket)),
    )
    .boxed();
    // An attribute may be followed by further doc comments, matching the
    // handwritten parser's loop exactly — the differential test requires the
    // two to agree on tree shape, not merely on acceptance.
    let attributes = attribute
        .then(tok(K::DocComment).repeated().collect::<Vec<_>>())
        .map(children)
        .repeated()
        .collect::<Vec<_>>()
        .map(|groups: Vec<Vec<SyntaxElement>>| groups.into_iter().flatten().collect::<Vec<_>>());
    let vis = tok(K::KwPub).or_not();
    let bound_list = node!(
        SyntaxKind::BoundList,
        type_path().then(
            tok(K::Plus)
                .then(type_path())
                .repeated()
                .collect::<Vec<_>>(),
        ),
    )
    .boxed();
    let generic_param = node!(
        SyntaxKind::GenericParam,
        tok(K::Ident).then(tok(K::Colon).then(bound_list.clone()).or_not()),
    );
    let generic_params = node!(
        SyntaxKind::GenericParams,
        tok(K::OpenBracket)
            .then(comma_list1(generic_param.boxed()))
            .then(tok(K::CloseBracket)),
    )
    .boxed();
    let where_pred = node!(
        SyntaxKind::WherePred,
        ty.clone().then(tok(K::Colon)).then(bound_list),
    );
    let where_clause = node!(
        SyntaxKind::WhereClause,
        tok(K::KwWhere).then(comma_list1(where_pred.boxed())),
    )
    .boxed();

    let mode = one_of([t(K::KwIn), t(K::KwMut), t(K::KwTake)]).map(elt);
    let receiver = node!(SyntaxKind::Receiver, mode.then(tok(K::KwSelfValue)));
    let normal_param = node!(
        SyntaxKind::Param,
        mode.then(tok(K::Ident))
            .then(tok(K::Colon))
            .then(ty.clone()),
    );
    let param_list = node!(
        SyntaxKind::ParamList,
        tok(K::OpenParen)
            .then(comma_list(choice((receiver, normal_param)).boxed()))
            .then(tok(K::CloseParen)),
    )
    .boxed();
    let return_type = node!(SyntaxKind::ReturnType, tok(K::ThinArrow).then(ty.clone())).boxed();

    let fn_header = tok(K::KwFn)
        .then(tok(K::Ident))
        .then(generic_params.clone().or_not())
        .then(param_list)
        .then(return_type.or_not())
        .then(where_clause.clone().or_not())
        .boxed();
    let function_item = node!(
        SyntaxKind::FunctionItem,
        fn_header.clone().then(block_b.clone())
    )
    .boxed();
    let function_signature =
        node!(SyntaxKind::FunctionSignature, fn_header.then(tok(K::Semi))).boxed();

    let field_decl = node!(
        SyntaxKind::FieldDecl,
        docs.clone()
            .then(vis.clone())
            .then(tok(K::Ident))
            .then(tok(K::Colon))
            .then(ty.clone()),
    );
    let field_block = node!(
        SyntaxKind::FieldBlock,
        tok(K::OpenBrace)
            .then(comma_list(field_decl.boxed()))
            .then(tok(K::CloseBrace)),
    )
    .boxed();
    let struct_item = node!(
        SyntaxKind::StructItem,
        tok(K::KwStruct)
            .then(tok(K::Ident))
            .then(generic_params.clone().or_not())
            .then(where_clause.clone().or_not())
            .then(field_block.clone()),
    )
    .boxed();
    let variant_decl = node!(
        SyntaxKind::VariantDecl,
        docs.clone().then(tok(K::Ident)).then(field_block.or_not()),
    );
    let enum_item = node!(
        SyntaxKind::EnumItem,
        tok(K::KwEnum)
            .then(tok(K::Ident))
            .then(generic_params.clone().or_not())
            .then(where_clause.clone().or_not())
            .then(tok(K::OpenBrace))
            .then(comma_list(variant_decl.boxed()))
            .then(tok(K::CloseBrace)),
    )
    .boxed();
    let interface_member = docs
        .clone()
        .then(choice((function_item.clone(), function_signature)))
        .map(children)
        .boxed();
    let interface_item = node!(
        SyntaxKind::InterfaceItem,
        tok(K::KwInterface)
            .then(tok(K::Ident))
            .then(generic_params.clone().or_not())
            .then(where_clause.clone().or_not())
            .then(tok(K::OpenBrace))
            .then(interface_member.repeated().collect::<Vec<_>>())
            .then(tok(K::CloseBrace)),
    )
    .boxed();
    let impl_member = docs
        .clone()
        .then(vis.clone())
        .then(function_item.clone())
        .map(children)
        .boxed();
    let impl_item = node!(
        SyntaxKind::ImplItem,
        tok(K::KwImpl)
            .then(generic_params.or_not())
            .then(type_path())
            .then(type_args_p.or_not())
            .then(tok(K::KwFor))
            .then(ty.clone())
            .then(where_clause.or_not())
            .then(tok(K::OpenBrace))
            .then(impl_member.repeated().collect::<Vec<_>>())
            .then(tok(K::CloseBrace)),
    )
    .boxed();

    // ----- imports & module (B, C) ----------------------------------------
    let path_node = node!(
        SyntaxKind::Path,
        tok(K::Ident).then(
            tok(K::ColonColon)
                .then(tok(K::Ident))
                .repeated()
                .collect::<Vec<_>>(),
        ),
    )
    .boxed();
    let import_leaf = node!(
        SyntaxKind::ImportLeaf,
        tok(K::Ident).then(tok(K::KwAs).then(tok(K::Ident)).or_not()),
    );
    let import_group = node!(
        SyntaxKind::ImportGroup,
        tok(K::OpenBrace)
            .then(comma_list1(import_leaf.boxed()))
            .then(tok(K::CloseBrace)),
    );
    let import_item = node!(
        SyntaxKind::ImportItem,
        tok(K::KwImport)
            .then(path_node.clone())
            .then(tok(K::ColonColon).then(import_group).or_not())
            .then(tok(K::KwAs).then(tok(K::Ident)).or_not())
            .then(tok(K::Semi)),
    )
    .boxed();
    let module_decl = node!(
        SyntaxKind::ModuleDecl,
        tok(K::KwModule).then(path_node).then(tok(K::Semi)),
    )
    .boxed();

    // ----- specs (I) ------------------------------------------------------
    let given_binding = node!(
        SyntaxKind::GivenBinding,
        tok(K::Ident)
            .then(tok(K::Colon))
            .then(ty.clone())
            .then(tok(K::Eq).then(expr_b.clone()).or_not()),
    );
    let given_clause = node!(
        SyntaxKind::GivenClause,
        tok(K::KwGiven)
            .then(comma_list1(given_binding.boxed()))
            .then(tok(K::Semi)),
    );
    let let_inner = |kw: K, kind: SyntaxKind| {
        node!(
            kind,
            tok(kw)
                .then(pattern.clone())
                .then(tok(K::Colon).then(ty.clone()).or_not())
                .then(tok(K::Eq))
                .then(expr_b.clone()),
        )
    };
    let when_clause = node!(
        SyntaxKind::WhenClause,
        tok(K::KwWhen)
            .then(choice((
                let_inner(K::KwLet, SyntaxKind::LetStatement).boxed(),
                let_inner(K::KwVar, SyntaxKind::VarStatement).boxed(),
                expr_b.clone(),
            )))
            .then(tok(K::Semi)),
    );
    let then_clause = node!(
        SyntaxKind::ThenClause,
        tok(K::KwThen).then(expr_b.clone()).then(tok(K::Semi)),
    );
    let assert_clause = node!(
        SyntaxKind::AssertClause,
        tok(K::KwAssert).then(expr_b.clone()).then(tok(K::Semi)),
    );
    let spec_statement = choice((
        given_clause,
        when_clause,
        then_clause,
        assert_clause,
        let_stmt,
        var_stmt,
        expr_stmt,
        stmt_recovery,
    ))
    .boxed();
    let spec_name = choice((tok(K::Ident), tok(K::StringLiteral)));
    let spec_item = node!(
        SyntaxKind::SpecItem,
        tok(K::KwSpec)
            .then(spec_name)
            .then(tok(K::OpenBrace))
            .then(spec_statement.repeated().collect::<Vec<_>>())
            .then(tok(K::CloseBrace)),
    )
    .boxed();

    // ----- items & the file (B) -------------------------------------------
    let item_body = choice((
        import_item,
        function_item,
        struct_item,
        enum_item,
        interface_item,
        impl_item,
        const_item,
        spec_item,
    ))
    .boxed();
    // Docs and `pub` belong inside the item node they precede.
    let item = docs
        .then(attributes)
        .then(vis)
        .then(item_body)
        .map(|(((docs, attributes), vis), body)| match body {
            SyntaxElement::Node(mut node) => {
                let mut prefix = children(((docs, attributes), vis));
                prefix.append(&mut node.children);
                node.children = prefix;
                SyntaxElement::Node(node)
            }
            token => token,
        })
        .boxed();

    // Item-level recovery: consume at least one token, then skip until the
    // next item-leading keyword (grammar NOTES §2).
    let item_recovery = any()
        .then(
            any()
                .filter(|tk: &Tok| !ITEM_START.contains(&tk.kind))
                .repeated()
                .collect::<Vec<_>>(),
        )
        .validate(|(first, rest): (Tok, Vec<Tok>), extra, emitter| {
            emitter.emit(Rich::custom(
                extra.span(),
                format!(
                    "malformed item: skipped {} token(s) starting with {}",
                    1 + rest.len(),
                    kind_name(first.kind),
                ),
            ));
            let mut els = vec![elt(first)];
            els.extend(rest.into_iter().map(elt));
            make(SyntaxKind::Error, els)
        })
        .boxed();

    node!(
        SyntaxKind::SourceFile,
        module_decl
            .or_not()
            .then(choice((item, item_recovery)).repeated().collect::<Vec<_>>()),
    )
    .then_ignore(end())
    .map(|el| match el {
        SyntaxElement::Node(node) => node,
        SyntaxElement::Token(_) => unreachable!("source file is always a node"),
    })
    .boxed()
}

/// `Self | IDENT { :: IDENT }` (D).
fn type_path<'a>() -> BP<'a> {
    node!(
        SyntaxKind::TypePath,
        choice((tok(K::KwSelfType), tok(K::Ident))).then(
            tok(K::ColonColon)
                .then(tok(K::Ident))
                .repeated()
                .collect::<Vec<_>>(),
        ),
    )
    .boxed()
}

/// `[ type (, type)* ,? ]` (D).
fn type_args<'a>(ty: BP<'a>) -> BP<'a> {
    node!(
        SyntaxKind::TypeArguments,
        tok(K::OpenBracket)
            .then(comma_list1(ty))
            .then(tok(K::CloseBracket)),
    )
    .boxed()
}
