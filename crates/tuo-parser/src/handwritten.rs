//! The handwritten recursive-descent + Pratt engine behind [`crate::parse`].
//!
//! Chosen at the parser architecture decision gate
//! (`specification/adr/ADR-parser-strategy.md`) on measured throughput,
//! allocation, and recovery-quality results. It implements the normative
//! grammar (`specification/grammar.ebnf`), produces the lossless
//! [`SyntaxTree`], recovers at the language's synchronization points (`;`,
//! `}`, item-leading keywords), and emits the same diagnostics as the
//! retained Chumsky oracle ([`crate::oracle`]) — parity is enforced by the
//! `oracle_parity` test suite, which compares both engines tree-for-tree and
//! diagnostic-for-diagnostic on the whole fixture corpus.
//!
//! # Shape
//!
//! One `struct Parser` holds the trivia-stripped token stream and a cursor.
//! Each grammar production is one method building a [`SyntaxNode`] whose
//! children are the matched tokens (by index into the lossless stream) and
//! sub-nodes, in source order. Backtracking mirrors the Chumsky grammar's
//! PEG semantics: every speculative alternative saves a [`Parser::mark`] and
//! rolls back cursor **and** diagnostics on failure, so a failed alternative
//! leaves no trace. Binary operators use Pratt-style precedence-climbing
//! loops instead of a combinator tower.

use tuo_diagnostics::Diagnostic;
use tuo_lexer::{LexResult, TokenKind as K};
use tuo_source::SourceText;
use tuo_syntax::{SyntaxElement, SyntaxKind, SyntaxNode, SyntaxTree};

use crate::ParseResult;
use crate::grammar::ITEM_START;
use crate::parse::{byte_span, code, depth_guard};
use crate::stream::{Tok, kind_name, parse_stream};

/// Parse one immutable source snapshot with the handwritten engine.
///
/// This is what [`crate::parse`] runs: never panics, always consumes the
/// whole file, recovers at `;` / `}` / item-leading keywords, and yields a
/// fully reconstructable tree with one diagnostic per recovery skip.
#[must_use]
pub(crate) fn parse(source: &SourceText) -> ParseResult {
    let lex = tuo_lexer::lex(source);
    let toks = parse_stream(&lex);
    let lex = match depth_guard(lex, &toks, source) {
        Ok(lex) => lex,
        Err(result) => return result,
    };
    let mut parser = Parser {
        toks: &toks,
        pos: 0,
        lex: &lex,
        source,
        diagnostics: Vec::new(),
    };
    let root = parser.source_file();
    let mut diagnostics = parser.diagnostics;
    diagnostics.sort_by_key(|d| d.primary_span.range().start());
    ParseResult {
        tree: SyntaxTree { root, lex },
        diagnostics,
    }
}

/// A production failed; the caller decides whether to backtrack or recover.
struct Fail;

/// The result of one production attempt.
type R<T = SyntaxElement> = Result<T, Fail>;

/// A child list under construction.
type Els = Vec<SyntaxElement>;

/// A backtracking point: cursor position plus diagnostics length, so a
/// failed speculative alternative also retracts anything it reported.
type Mark = (usize, usize);

/// Wrap children in a node element.
fn node(kind: SyntaxKind, children: Els) -> SyntaxElement {
    SyntaxElement::Node(SyntaxNode::new(kind, children))
}

/// Token kinds that begin a block-form expression (one that can stand as a
/// statement without a trailing `;`).
const fn is_block_form(kind: K) -> bool {
    matches!(
        kind,
        K::OpenBrace
            | K::KwIf
            | K::KwMatch
            | K::KwWhile
            | K::KwLoop
            | K::LifetimeLabel
            | K::KwFor
            | K::KwUnsafe
    )
}

/// The recursive-descent state: stream, cursor, and collected diagnostics.
struct Parser<'a> {
    toks: &'a [Tok],
    pos: usize,
    lex: &'a LexResult,
    source: &'a SourceText,
    diagnostics: Vec<Diagnostic>,
}

impl Parser<'_> {
    // ----- cursor primitives ---------------------------------------------

    fn peek(&self) -> K {
        self.toks.get(self.pos).map_or(K::Eof, |tk| tk.kind)
    }

    fn peek2(&self) -> K {
        self.toks.get(self.pos + 1).map_or(K::Eof, |tk| tk.kind)
    }

    fn at(&self, kind: K) -> bool {
        self.peek() == kind
    }

    fn bump(&mut self) -> SyntaxElement {
        let el = SyntaxElement::Token(self.toks[self.pos].index);
        self.pos += 1;
        el
    }

    fn expect(&mut self, kind: K, out: &mut Els) -> R<()> {
        if self.at(kind) {
            out.push(self.bump());
            Ok(())
        } else {
            Err(Fail)
        }
    }

    fn mark(&self) -> Mark {
        (self.pos, self.diagnostics.len())
    }

    fn rollback(&mut self, mark: Mark) {
        self.pos = mark.0;
        self.diagnostics.truncate(mark.1);
    }

    /// Record one `P0002` recovery diagnostic for `count` skipped tokens
    /// starting at parse-stream position `start` (same wording, label, and
    /// note as the Chumsky engine's recovery path).
    fn skip_report(&mut self, what: &str, start: usize, count: usize) {
        let first = kind_name(self.toks[start].kind);
        let primary = byte_span(start, self.toks, self.lex, self.source);
        self.diagnostics.push(
            Diagnostic::error(
                code(2),
                format!("malformed {what}: skipped {count} token(s) starting with {first}"),
                primary,
            )
            .with_primary_label("skipped during error recovery")
            .with_note(
                "the parser resynchronized at the next `;`, `}`, or item keyword and continued",
            ),
        );
    }

    /// `item ("," item)* (",")?` keeping the separator tokens. With
    /// `required`, the list must have at least one item. Mirrors the Chumsky
    /// `comma_list` / `comma_list1` combinators, including the backtracking:
    /// a comma not followed by an item is consumed as the trailing comma.
    fn comma_list(
        &mut self,
        out: &mut Els,
        required: bool,
        mut item: impl FnMut(&mut Self) -> R,
    ) -> R<()> {
        let mark = self.mark();
        match item(self) {
            Ok(el) => out.push(el),
            Err(Fail) => {
                self.rollback(mark);
                return if required { Err(Fail) } else { Ok(()) };
            }
        }
        while self.at(K::Comma) {
            let mark = self.mark();
            let comma = self.bump();
            match item(self) {
                Ok(el) => {
                    out.push(comma);
                    out.push(el);
                }
                Err(Fail) => {
                    self.rollback(mark);
                    out.push(self.bump());
                    break;
                }
            }
        }
        Ok(())
    }

    // ----- the file (B) ---------------------------------------------------

    fn source_file(&mut self) -> SyntaxNode {
        let mut children = Els::new();
        if self.at(K::KwModule) {
            let mark = self.mark();
            match self.module_decl() {
                Ok(el) => children.push(el),
                Err(Fail) => self.rollback(mark),
            }
        }
        while !self.at(K::Eof) {
            let mark = self.mark();
            match self.item() {
                Ok(el) => children.push(el),
                Err(Fail) => {
                    self.rollback(mark);
                    children.push(self.item_recovery());
                }
            }
        }
        SyntaxNode::new(SyntaxKind::SourceFile, children)
    }

    /// Item-level recovery: consume at least one token, then skip until the
    /// next item-leading keyword (grammar NOTES §2).
    fn item_recovery(&mut self) -> SyntaxElement {
        let start = self.pos;
        let mut els = vec![self.bump()];
        while !self.at(K::Eof) && !ITEM_START.contains(&self.peek()) {
            els.push(self.bump());
        }
        self.skip_report("item", start, self.pos - start);
        node(SyntaxKind::Error, els)
    }

    fn item(&mut self) -> R {
        let mut prefix = Els::new();
        while self.at(K::DocComment) {
            prefix.push(self.bump());
        }
        if self.at(K::KwPub) {
            prefix.push(self.bump());
        }
        let body = match self.peek() {
            K::KwImport => self.import_item()?,
            K::KwFn => self.function_item()?,
            K::KwStruct => self.struct_item()?,
            K::KwEnum => self.enum_item()?,
            K::KwInterface => self.interface_item()?,
            K::KwImpl => self.impl_item()?,
            K::KwConst => self.const_item()?,
            K::KwSpec => self.spec_item()?,
            _ => return Err(Fail),
        };
        // Docs and `pub` belong inside the item node they precede.
        let SyntaxElement::Node(mut item) = body else {
            unreachable!("item bodies are always nodes")
        };
        prefix.append(&mut item.children);
        item.children = prefix;
        Ok(SyntaxElement::Node(item))
    }

    // ----- imports & module (B, C) ----------------------------------------

    fn module_decl(&mut self) -> R {
        let mut els = Els::new();
        self.expect(K::KwModule, &mut els)?;
        els.push(self.path_node()?);
        self.expect(K::Semi, &mut els)?;
        Ok(node(SyntaxKind::ModuleDecl, els))
    }

    /// `IDENT { :: IDENT }` — stops before a `::` not followed by an
    /// identifier (e.g. the `::{…}` of an import group).
    fn path_node(&mut self) -> R {
        let mut els = Els::new();
        self.expect(K::Ident, &mut els)?;
        while self.at(K::ColonColon) && self.peek2() == K::Ident {
            els.push(self.bump());
            els.push(self.bump());
        }
        Ok(node(SyntaxKind::Path, els))
    }

    fn import_item(&mut self) -> R {
        let mut els = Els::new();
        self.expect(K::KwImport, &mut els)?;
        els.push(self.path_node()?);
        if self.at(K::ColonColon) {
            let mark = self.mark();
            let sep = self.bump();
            match self.import_group() {
                Ok(group) => {
                    els.push(sep);
                    els.push(group);
                }
                Err(Fail) => self.rollback(mark),
            }
        }
        if self.at(K::KwAs) {
            els.push(self.bump());
            self.expect(K::Ident, &mut els)?;
        }
        self.expect(K::Semi, &mut els)?;
        Ok(node(SyntaxKind::ImportItem, els))
    }

    fn import_group(&mut self) -> R {
        let mut els = Els::new();
        self.expect(K::OpenBrace, &mut els)?;
        self.comma_list(&mut els, true, Self::import_leaf)?;
        self.expect(K::CloseBrace, &mut els)?;
        Ok(node(SyntaxKind::ImportGroup, els))
    }

    fn import_leaf(&mut self) -> R {
        let mut els = Els::new();
        self.expect(K::Ident, &mut els)?;
        if self.at(K::KwAs) {
            els.push(self.bump());
            self.expect(K::Ident, &mut els)?;
        }
        Ok(node(SyntaxKind::ImportLeaf, els))
    }

    // ----- declarations (E) -----------------------------------------------

    fn function_item(&mut self) -> R {
        let mut els = Els::new();
        self.fn_header(&mut els)?;
        els.push(self.block()?);
        Ok(node(SyntaxKind::FunctionItem, els))
    }

    fn fn_header(&mut self, els: &mut Els) -> R<()> {
        self.expect(K::KwFn, els)?;
        self.expect(K::Ident, els)?;
        if self.at(K::OpenBracket) {
            let mark = self.mark();
            match self.generic_params() {
                Ok(params) => els.push(params),
                Err(Fail) => self.rollback(mark),
            }
        }
        els.push(self.param_list()?);
        if self.at(K::ThinArrow) {
            let mut ret = vec![self.bump()];
            ret.push(self.ty()?);
            els.push(node(SyntaxKind::ReturnType, ret));
        }
        if self.at(K::KwWhere) {
            els.push(self.where_clause()?);
        }
        Ok(())
    }

    fn param_list(&mut self) -> R {
        let mut els = Els::new();
        self.expect(K::OpenParen, &mut els)?;
        self.comma_list(&mut els, false, Self::param)?;
        self.expect(K::CloseParen, &mut els)?;
        Ok(node(SyntaxKind::ParamList, els))
    }

    /// One parameter: a mode keyword, then either the receiver (`self`) or
    /// `IDENT : type`.
    fn param(&mut self) -> R {
        let mode = match self.peek() {
            K::KwIn | K::KwMut | K::KwTake => self.bump(),
            _ => return Err(Fail),
        };
        if self.at(K::KwSelfValue) {
            return Ok(node(SyntaxKind::Receiver, vec![mode, self.bump()]));
        }
        let mut els = vec![mode];
        self.expect(K::Ident, &mut els)?;
        self.expect(K::Colon, &mut els)?;
        els.push(self.ty()?);
        Ok(node(SyntaxKind::Param, els))
    }

    fn generic_params(&mut self) -> R {
        let mut els = Els::new();
        self.expect(K::OpenBracket, &mut els)?;
        self.comma_list(&mut els, true, Self::generic_param)?;
        self.expect(K::CloseBracket, &mut els)?;
        Ok(node(SyntaxKind::GenericParams, els))
    }

    fn generic_param(&mut self) -> R {
        let mut els = Els::new();
        self.expect(K::Ident, &mut els)?;
        if self.at(K::Colon) {
            let mark = self.mark();
            let colon = self.bump();
            match self.bound_list() {
                Ok(bounds) => {
                    els.push(colon);
                    els.push(bounds);
                }
                Err(Fail) => self.rollback(mark),
            }
        }
        Ok(node(SyntaxKind::GenericParam, els))
    }

    fn bound_list(&mut self) -> R {
        let mut els = vec![self.type_path()?];
        while self.at(K::Plus) {
            let mark = self.mark();
            let plus = self.bump();
            match self.type_path() {
                Ok(path) => {
                    els.push(plus);
                    els.push(path);
                }
                Err(Fail) => {
                    self.rollback(mark);
                    break;
                }
            }
        }
        Ok(node(SyntaxKind::BoundList, els))
    }

    fn where_clause(&mut self) -> R {
        let mut els = Els::new();
        self.expect(K::KwWhere, &mut els)?;
        self.comma_list(&mut els, true, Self::where_pred)?;
        Ok(node(SyntaxKind::WhereClause, els))
    }

    fn where_pred(&mut self) -> R {
        let mut els = vec![self.ty()?];
        self.expect(K::Colon, &mut els)?;
        els.push(self.bound_list()?);
        Ok(node(SyntaxKind::WherePred, els))
    }

    fn struct_item(&mut self) -> R {
        let mut els = Els::new();
        self.expect(K::KwStruct, &mut els)?;
        self.expect(K::Ident, &mut els)?;
        if self.at(K::OpenBracket) {
            let mark = self.mark();
            match self.generic_params() {
                Ok(params) => els.push(params),
                Err(Fail) => self.rollback(mark),
            }
        }
        if self.at(K::KwWhere) {
            els.push(self.where_clause()?);
        }
        els.push(self.field_block()?);
        Ok(node(SyntaxKind::StructItem, els))
    }

    fn field_block(&mut self) -> R {
        let mut els = Els::new();
        self.expect(K::OpenBrace, &mut els)?;
        self.comma_list(&mut els, false, Self::field_decl)?;
        self.expect(K::CloseBrace, &mut els)?;
        Ok(node(SyntaxKind::FieldBlock, els))
    }

    fn field_decl(&mut self) -> R {
        let mut els = Els::new();
        while self.at(K::DocComment) {
            els.push(self.bump());
        }
        if self.at(K::KwPub) {
            els.push(self.bump());
        }
        self.expect(K::Ident, &mut els)?;
        self.expect(K::Colon, &mut els)?;
        els.push(self.ty()?);
        Ok(node(SyntaxKind::FieldDecl, els))
    }

    fn enum_item(&mut self) -> R {
        let mut els = Els::new();
        self.expect(K::KwEnum, &mut els)?;
        self.expect(K::Ident, &mut els)?;
        if self.at(K::OpenBracket) {
            let mark = self.mark();
            match self.generic_params() {
                Ok(params) => els.push(params),
                Err(Fail) => self.rollback(mark),
            }
        }
        if self.at(K::KwWhere) {
            els.push(self.where_clause()?);
        }
        self.expect(K::OpenBrace, &mut els)?;
        self.comma_list(&mut els, false, Self::variant_decl)?;
        self.expect(K::CloseBrace, &mut els)?;
        Ok(node(SyntaxKind::EnumItem, els))
    }

    fn variant_decl(&mut self) -> R {
        let mut els = Els::new();
        while self.at(K::DocComment) {
            els.push(self.bump());
        }
        self.expect(K::Ident, &mut els)?;
        if self.at(K::OpenBrace) {
            let mark = self.mark();
            match self.field_block() {
                Ok(fields) => els.push(fields),
                Err(Fail) => self.rollback(mark),
            }
        }
        Ok(node(SyntaxKind::VariantDecl, els))
    }

    fn interface_item(&mut self) -> R {
        let mut els = Els::new();
        self.expect(K::KwInterface, &mut els)?;
        self.expect(K::Ident, &mut els)?;
        if self.at(K::OpenBracket) {
            let mark = self.mark();
            match self.generic_params() {
                Ok(params) => els.push(params),
                Err(Fail) => self.rollback(mark),
            }
        }
        if self.at(K::KwWhere) {
            els.push(self.where_clause()?);
        }
        self.expect(K::OpenBrace, &mut els)?;
        loop {
            let mark = self.mark();
            match self.interface_member(&mut els) {
                Ok(()) => {}
                Err(Fail) => {
                    self.rollback(mark);
                    break;
                }
            }
        }
        self.expect(K::CloseBrace, &mut els)?;
        Ok(node(SyntaxKind::InterfaceItem, els))
    }

    /// Doc comments, then one function: with a body it is a
    /// [`SyntaxKind::FunctionItem`], with `;` a
    /// [`SyntaxKind::FunctionSignature`]. The docs land as direct children
    /// of the interface node, like the Chumsky grammar's flattening.
    fn interface_member(&mut self, out: &mut Els) -> R<()> {
        let mut docs = Els::new();
        while self.at(K::DocComment) {
            docs.push(self.bump());
        }
        let mut header = Els::new();
        self.fn_header(&mut header)?;
        let member = if self.at(K::Semi) {
            header.push(self.bump());
            node(SyntaxKind::FunctionSignature, header)
        } else {
            header.push(self.block()?);
            node(SyntaxKind::FunctionItem, header)
        };
        out.append(&mut docs);
        out.push(member);
        Ok(())
    }

    fn impl_item(&mut self) -> R {
        let mut els = Els::new();
        self.expect(K::KwImpl, &mut els)?;
        if self.at(K::OpenBracket) {
            let mark = self.mark();
            match self.generic_params() {
                Ok(params) => els.push(params),
                Err(Fail) => self.rollback(mark),
            }
        }
        els.push(self.type_path()?);
        if self.at(K::OpenBracket) {
            let mark = self.mark();
            match self.type_args() {
                Ok(args) => els.push(args),
                Err(Fail) => self.rollback(mark),
            }
        }
        self.expect(K::KwFor, &mut els)?;
        els.push(self.ty()?);
        if self.at(K::KwWhere) {
            els.push(self.where_clause()?);
        }
        self.expect(K::OpenBrace, &mut els)?;
        loop {
            let mark = self.mark();
            match self.impl_member(&mut els) {
                Ok(()) => {}
                Err(Fail) => {
                    self.rollback(mark);
                    break;
                }
            }
        }
        self.expect(K::CloseBrace, &mut els)?;
        Ok(node(SyntaxKind::ImplItem, els))
    }

    /// Doc comments and optional `pub`, then a function item, flattened as
    /// direct children of the impl node.
    fn impl_member(&mut self, out: &mut Els) -> R<()> {
        let mut prefix = Els::new();
        while self.at(K::DocComment) {
            prefix.push(self.bump());
        }
        if self.at(K::KwPub) {
            prefix.push(self.bump());
        }
        let member = self.function_item()?;
        out.append(&mut prefix);
        out.push(member);
        Ok(())
    }

    fn const_item(&mut self) -> R {
        let mut els = Els::new();
        self.expect(K::KwConst, &mut els)?;
        self.expect(K::Ident, &mut els)?;
        self.expect(K::Colon, &mut els)?;
        els.push(self.ty()?);
        self.expect(K::Eq, &mut els)?;
        els.push(self.expr(false)?);
        self.expect(K::Semi, &mut els)?;
        Ok(node(SyntaxKind::ConstItem, els))
    }

    // ----- specs (I) ------------------------------------------------------

    fn spec_item(&mut self) -> R {
        let mut els = Els::new();
        self.expect(K::KwSpec, &mut els)?;
        match self.peek() {
            K::Ident | K::StringLiteral => els.push(self.bump()),
            _ => return Err(Fail),
        }
        self.expect(K::OpenBrace, &mut els)?;
        loop {
            if self.at(K::CloseBrace) || self.at(K::Eof) {
                break;
            }
            let mark = self.mark();
            match self.spec_statement() {
                Ok(el) => els.push(el),
                Err(Fail) => {
                    self.rollback(mark);
                    // Spec recovery: skip to `;` (consumed) or stop at `}`.
                    let start = self.pos;
                    let mut skipped = Els::new();
                    while !matches!(self.peek(), K::Semi | K::CloseBrace | K::Eof) {
                        skipped.push(self.bump());
                    }
                    if skipped.is_empty() {
                        return Err(Fail);
                    }
                    self.skip_report("statement", start, self.pos - start);
                    if self.at(K::Semi) {
                        skipped.push(self.bump());
                    }
                    els.push(node(SyntaxKind::Error, skipped));
                }
            }
        }
        self.expect(K::CloseBrace, &mut els)?;
        Ok(node(SyntaxKind::SpecItem, els))
    }

    fn spec_statement(&mut self) -> R {
        match self.peek() {
            K::KwGiven => self.given_clause(),
            K::KwWhen => self.when_clause(),
            K::KwThen => self.simple_clause(K::KwThen, SyntaxKind::ThenClause),
            K::KwAssert => self.simple_clause(K::KwAssert, SyntaxKind::AssertClause),
            K::KwLet => self.binding_stmt(K::KwLet, SyntaxKind::LetStatement),
            K::KwVar => self.binding_stmt(K::KwVar, SyntaxKind::VarStatement),
            _ => self.expr_statement(),
        }
    }

    fn given_clause(&mut self) -> R {
        let mut els = Els::new();
        self.expect(K::KwGiven, &mut els)?;
        self.comma_list(&mut els, true, Self::given_binding)?;
        self.expect(K::Semi, &mut els)?;
        Ok(node(SyntaxKind::GivenClause, els))
    }

    fn given_binding(&mut self) -> R {
        let mut els = Els::new();
        self.expect(K::Ident, &mut els)?;
        self.expect(K::Colon, &mut els)?;
        els.push(self.ty()?);
        if self.at(K::Eq) {
            let mark = self.mark();
            let eq = self.bump();
            match self.expr(false) {
                Ok(init) => {
                    els.push(eq);
                    els.push(init);
                }
                Err(Fail) => self.rollback(mark),
            }
        }
        Ok(node(SyntaxKind::GivenBinding, els))
    }

    fn when_clause(&mut self) -> R {
        let mut els = Els::new();
        self.expect(K::KwWhen, &mut els)?;
        let inner = match self.peek() {
            K::KwLet => self.when_binding(K::KwLet, SyntaxKind::LetStatement)?,
            K::KwVar => self.when_binding(K::KwVar, SyntaxKind::VarStatement)?,
            _ => self.expr(false)?,
        };
        els.push(inner);
        self.expect(K::Semi, &mut els)?;
        Ok(node(SyntaxKind::WhenClause, els))
    }

    /// The `when let` / `when var` inner binding: initializer required, no
    /// trailing `;` of its own.
    fn when_binding(&mut self, kw: K, kind: SyntaxKind) -> R {
        let mut els = Els::new();
        self.expect(kw, &mut els)?;
        els.push(self.pattern()?);
        if self.at(K::Colon) {
            let mark = self.mark();
            let colon = self.bump();
            match self.ty() {
                Ok(ty) => {
                    els.push(colon);
                    els.push(ty);
                }
                Err(Fail) => self.rollback(mark),
            }
        }
        self.expect(K::Eq, &mut els)?;
        els.push(self.expr(false)?);
        Ok(node(kind, els))
    }

    /// `then expr ;` / `assert expr ;`.
    fn simple_clause(&mut self, kw: K, kind: SyntaxKind) -> R {
        let mut els = Els::new();
        self.expect(kw, &mut els)?;
        els.push(self.expr(false)?);
        self.expect(K::Semi, &mut els)?;
        Ok(node(kind, els))
    }

    // ----- statements & blocks (F) ----------------------------------------

    fn binding_stmt(&mut self, kw: K, kind: SyntaxKind) -> R {
        let mut els = Els::new();
        self.expect(kw, &mut els)?;
        els.push(self.pattern()?);
        if self.at(K::Colon) {
            let mark = self.mark();
            let colon = self.bump();
            match self.ty() {
                Ok(ty) => {
                    els.push(colon);
                    els.push(ty);
                }
                Err(Fail) => self.rollback(mark),
            }
        }
        if self.at(K::Eq) {
            let mark = self.mark();
            let eq = self.bump();
            match self.expr(false) {
                Ok(init) => {
                    els.push(eq);
                    els.push(init);
                }
                Err(Fail) => self.rollback(mark),
            }
        }
        self.expect(K::Semi, &mut els)?;
        Ok(node(kind, els))
    }

    /// An expression statement: a block-form expression stands alone; any
    /// other expression needs a `;`.
    fn expr_statement(&mut self) -> R {
        if is_block_form(self.peek()) {
            let el = self.block_expression()?;
            return Ok(node(SyntaxKind::ExpressionStatement, vec![el]));
        }
        let mut els = vec![self.expr(false)?];
        self.expect(K::Semi, &mut els)?;
        Ok(node(SyntaxKind::ExpressionStatement, els))
    }

    fn block(&mut self) -> R {
        let mut els = Els::new();
        self.expect(K::OpenBrace, &mut els)?;
        let mut tail: Option<SyntaxElement> = None;
        loop {
            if self.at(K::CloseBrace) || self.at(K::Eof) {
                break;
            }
            let mark = self.mark();
            let attempt: R = match self.peek() {
                K::KwLet => self.binding_stmt(K::KwLet, SyntaxKind::LetStatement),
                K::KwVar => self.binding_stmt(K::KwVar, SyntaxKind::VarStatement),
                K::KwConst => self.const_item(),
                K::Semi => Ok(node(SyntaxKind::EmptyStatement, vec![self.bump()])),
                k if is_block_form(k) => self.expr_statement(),
                _ => match self.expr(false) {
                    Ok(expr) if self.at(K::Semi) => {
                        let semi = self.bump();
                        Ok(node(SyntaxKind::ExpressionStatement, vec![expr, semi]))
                    }
                    Ok(expr) if self.at(K::CloseBrace) => {
                        tail = Some(expr);
                        break;
                    }
                    _ => Err(Fail),
                },
            };
            match attempt {
                Ok(el) => els.push(el),
                Err(Fail) => {
                    self.rollback(mark);
                    // Statement recovery: the skipped tokens must end at a
                    // consumed `;` — otherwise a valid tail expression could
                    // be swallowed, so fall through to tail position.
                    let start = self.pos;
                    let mut end = self.pos;
                    while end < self.toks.len()
                        && !matches!(self.toks[end].kind, K::Semi | K::CloseBrace)
                    {
                        end += 1;
                    }
                    if end > start && end < self.toks.len() && self.toks[end].kind == K::Semi {
                        self.skip_report("statement", start, end - start);
                        let skipped: Els = self.toks[start..=end]
                            .iter()
                            .map(|tk| SyntaxElement::Token(tk.index))
                            .collect();
                        self.pos = end + 1;
                        els.push(node(SyntaxKind::Error, skipped));
                    } else {
                        break;
                    }
                }
            }
        }
        // Tail position: an expression directly before the `}`, or tail
        // recovery running up to (not including) the `}`.
        if tail.is_none() && !self.at(K::CloseBrace) && !self.at(K::Eof) {
            let mark = self.mark();
            match self.expr(false) {
                Ok(expr) if self.at(K::CloseBrace) => tail = Some(expr),
                _ => {
                    self.rollback(mark);
                    let start = self.pos;
                    let mut skipped = Els::new();
                    while !matches!(self.peek(), K::Semi | K::CloseBrace | K::Eof) {
                        skipped.push(self.bump());
                    }
                    if !skipped.is_empty() {
                        self.skip_report("statement", start, self.pos - start);
                        els.push(node(SyntaxKind::Error, skipped));
                    }
                }
            }
        }
        if let Some(expr) = tail {
            els.push(expr);
        }
        self.expect(K::CloseBrace, &mut els)?;
        Ok(node(SyntaxKind::Block, els))
    }

    // ----- expressions (G): Pratt-style precedence climbing ---------------

    /// One expression. With `ns` ("no struct"), struct literals are not
    /// allowed at any level of this expression's own precedence tower (the
    /// scrutinee positions of `if`/`while`/`for`/`match`); nested
    /// parenthesized/braced positions re-allow them.
    fn expr(&mut self, ns: bool) -> R {
        // Assignment is right-associative.
        let lhs = self.or_expr(ns)?;
        if self.at(K::Eq) {
            let mark = self.mark();
            let eq = self.bump();
            if let Ok(rhs) = self.expr(ns) {
                return Ok(node(SyntaxKind::AssignExpr, vec![lhs, eq, rhs]));
            }
            self.rollback(mark);
        }
        Ok(lhs)
    }

    /// One left-associative binary level over `next`.
    fn binary_level(&mut self, ns: bool, ops: &[K], next: fn(&mut Self, bool) -> R) -> R {
        let mut lhs = next(self, ns)?;
        while ops.contains(&self.peek()) {
            let mark = self.mark();
            let op = self.bump();
            match next(self, ns) {
                Ok(rhs) => lhs = node(SyntaxKind::BinaryExpr, vec![lhs, op, rhs]),
                Err(Fail) => {
                    self.rollback(mark);
                    break;
                }
            }
        }
        Ok(lhs)
    }

    fn or_expr(&mut self, ns: bool) -> R {
        self.binary_level(ns, &[K::PipePipe], Self::and_expr)
    }

    fn and_expr(&mut self, ns: bool) -> R {
        self.binary_level(ns, &[K::AmpAmp], Self::compare_expr)
    }

    /// Comparison is non-associative: at most one operator per level.
    fn compare_expr(&mut self, ns: bool) -> R {
        let lhs = self.range_expr(ns)?;
        if matches!(
            self.peek(),
            K::EqEq | K::NotEq | K::Lt | K::Gt | K::LtEq | K::GtEq
        ) {
            let mark = self.mark();
            let op = self.bump();
            if let Ok(rhs) = self.range_expr(ns) {
                return Ok(node(SyntaxKind::BinaryExpr, vec![lhs, op, rhs]));
            }
            self.rollback(mark);
        }
        Ok(lhs)
    }

    /// Ranges do not chain: at most one `..` per level.
    fn range_expr(&mut self, ns: bool) -> R {
        let lhs = self.add_expr(ns)?;
        if self.at(K::DotDot) {
            let mark = self.mark();
            let op = self.bump();
            if let Ok(rhs) = self.add_expr(ns) {
                return Ok(node(SyntaxKind::RangeExpr, vec![lhs, op, rhs]));
            }
            self.rollback(mark);
        }
        Ok(lhs)
    }

    fn add_expr(&mut self, ns: bool) -> R {
        self.binary_level(ns, &[K::Plus, K::Minus], Self::mul_expr)
    }

    fn mul_expr(&mut self, ns: bool) -> R {
        self.binary_level(ns, &[K::Star, K::Slash, K::Percent], Self::unary_expr)
    }

    fn unary_expr(&mut self, ns: bool) -> R {
        if matches!(self.peek(), K::Minus | K::Bang | K::KwMove) {
            let op = self.bump();
            let inner = self.unary_expr(ns)?;
            return Ok(node(SyntaxKind::UnaryExpr, vec![op, inner]));
        }
        self.postfix_expr(ns)
    }

    fn postfix_expr(&mut self, ns: bool) -> R {
        let mut lhs = self.primary(ns)?;
        loop {
            match self.peek() {
                K::Question => {
                    let question = self.bump();
                    lhs = node(SyntaxKind::TryExpr, vec![lhs, question]);
                }
                K::Dot if matches!(self.peek2(), K::Ident | K::IntLiteral) => {
                    let dot = self.bump();
                    let name = self.bump();
                    if self.at(K::OpenParen) {
                        let mark = self.mark();
                        match self.call_args() {
                            Ok(args) => {
                                lhs = node(SyntaxKind::MethodCallExpr, vec![lhs, dot, name, args]);
                                continue;
                            }
                            Err(Fail) => self.rollback(mark),
                        }
                    }
                    lhs = node(SyntaxKind::FieldExpr, vec![lhs, dot, name]);
                }
                K::OpenParen => {
                    let mark = self.mark();
                    match self.call_args() {
                        Ok(args) => lhs = node(SyntaxKind::CallExpr, vec![lhs, args]),
                        Err(Fail) => {
                            self.rollback(mark);
                            break;
                        }
                    }
                }
                K::OpenBracket => {
                    let mark = self.mark();
                    let open = self.bump();
                    match self.expr(false) {
                        Ok(index) if self.at(K::CloseBracket) => {
                            let close = self.bump();
                            lhs = node(SyntaxKind::IndexExpr, vec![lhs, open, index, close]);
                        }
                        _ => {
                            self.rollback(mark);
                            break;
                        }
                    }
                }
                K::KwAs => {
                    let mark = self.mark();
                    let kw = self.bump();
                    match self.ty() {
                        Ok(ty) => lhs = node(SyntaxKind::CastExpr, vec![lhs, kw, ty]),
                        Err(Fail) => {
                            self.rollback(mark);
                            break;
                        }
                    }
                }
                _ => break,
            }
        }
        Ok(lhs)
    }

    fn call_args(&mut self) -> R {
        let mut els = Els::new();
        self.expect(K::OpenParen, &mut els)?;
        self.comma_list(&mut els, false, |p| p.expr(false))?;
        self.expect(K::CloseParen, &mut els)?;
        Ok(node(SyntaxKind::CallArgs, els))
    }

    fn primary(&mut self, ns: bool) -> R {
        match self.peek() {
            K::IntLiteral
            | K::FloatLiteral
            | K::BoolLiteral
            | K::CharLiteral
            | K::StringLiteral => Ok(node(SyntaxKind::Literal, vec![self.bump()])),
            K::OpenParen if self.peek2() == K::CloseParen => {
                Ok(node(SyntaxKind::Literal, vec![self.bump(), self.bump()]))
            }
            K::OpenParen => {
                let mut els = vec![self.bump()];
                els.push(self.expr(false)?);
                self.expect(K::CloseParen, &mut els)?;
                Ok(node(SyntaxKind::GroupExpr, els))
            }
            K::Ident | K::KwSelfType => {
                if !ns {
                    let mark = self.mark();
                    if let Ok(lit) = self.struct_literal() {
                        return Ok(lit);
                    }
                    self.rollback(mark);
                }
                self.path_expr()
            }
            K::KwSelfValue => self.path_expr(),
            // A `[` at primary position can only open an array literal:
            // `base[i]` indexing is a postfix op after a completed primary
            // (ADR-0004 Stage 2).
            K::OpenBracket => self.array_literal(),
            k if is_block_form(k) => self.block_expression(),
            K::KwReturn => {
                let mut els = vec![self.bump()];
                let mark = self.mark();
                match self.expr(false) {
                    Ok(inner) => els.push(inner),
                    Err(Fail) => self.rollback(mark),
                }
                Ok(node(SyntaxKind::ReturnExpr, els))
            }
            K::KwBreak => {
                let mut els = vec![self.bump()];
                if self.at(K::LifetimeLabel) {
                    els.push(self.bump());
                }
                let mark = self.mark();
                match self.expr(false) {
                    Ok(inner) => els.push(inner),
                    Err(Fail) => self.rollback(mark),
                }
                Ok(node(SyntaxKind::BreakExpr, els))
            }
            K::KwContinue => {
                let mut els = vec![self.bump()];
                if self.at(K::LifetimeLabel) {
                    els.push(self.bump());
                }
                Ok(node(SyntaxKind::ContinueExpr, els))
            }
            _ => Err(Fail),
        }
    }

    /// `[a, b, c]` (0+ elements, trailing comma allowed, `[]` legal) or the
    /// repeat form `[x; N]` where `N` is an `INT_LITERAL` token — mirroring
    /// the Chumsky grammar's ordered choice (repeat form first, then the
    /// comma list), so both engines build the same tree (ADR-0004 Stage 2).
    fn array_literal(&mut self) -> R {
        let mut els = Els::new();
        self.expect(K::OpenBracket, &mut els)?;
        // Repeat form `[x; N]`: speculative, exactly like the oracle's
        // first alternative — a failed attempt leaves no trace.
        let mark = self.mark();
        if let Ok(value) = self.expr(false) {
            if self.at(K::Semi) {
                els.push(value);
                els.push(self.bump());
                self.expect(K::IntLiteral, &mut els)?;
                self.expect(K::CloseBracket, &mut els)?;
                return Ok(node(SyntaxKind::ArrayLiteral, els));
            }
        }
        self.rollback(mark);
        // List form: a possibly-empty comma list of expressions.
        self.comma_list(&mut els, false, |p| p.expr(false))?;
        self.expect(K::CloseBracket, &mut els)?;
        Ok(node(SyntaxKind::ArrayLiteral, els))
    }

    fn struct_literal(&mut self) -> R {
        let mut els = vec![self.type_path()?];
        if self.at(K::OpenBracket) {
            let mark = self.mark();
            match self.type_args() {
                Ok(args) => els.push(args),
                Err(Fail) => self.rollback(mark),
            }
        }
        self.expect(K::OpenBrace, &mut els)?;
        self.comma_list(&mut els, false, Self::field_init)?;
        self.expect(K::CloseBrace, &mut els)?;
        Ok(node(SyntaxKind::StructLiteral, els))
    }

    fn field_init(&mut self) -> R {
        let mut els = Els::new();
        self.expect(K::Ident, &mut els)?;
        if self.at(K::Colon) {
            let mark = self.mark();
            let colon = self.bump();
            match self.expr(false) {
                Ok(init) => {
                    els.push(colon);
                    els.push(init);
                }
                Err(Fail) => self.rollback(mark),
            }
        }
        Ok(node(SyntaxKind::FieldInit, els))
    }

    fn path_expr(&mut self) -> R {
        let mut els = Els::new();
        match self.peek() {
            K::KwSelfType | K::KwSelfValue | K::Ident => els.push(self.bump()),
            _ => return Err(Fail),
        }
        while self.at(K::ColonColon) && self.peek2() == K::Ident {
            els.push(self.bump());
            els.push(self.bump());
        }
        if self.at(K::ColonColon) {
            let mark = self.mark();
            let sep = self.bump();
            match self.type_args() {
                Ok(args) => els.push(node(SyntaxKind::Turbofish, vec![sep, args])),
                Err(Fail) => self.rollback(mark),
            }
        }
        Ok(node(SyntaxKind::PathExpr, els))
    }

    fn block_expression(&mut self) -> R {
        match self.peek() {
            K::OpenBrace => self.block(),
            K::KwIf => self.if_expr(),
            K::KwMatch => self.match_expr(),
            K::KwWhile => {
                let mut els = vec![self.bump()];
                els.push(self.expr(true)?);
                els.push(self.block()?);
                Ok(node(SyntaxKind::WhileExpr, els))
            }
            K::KwLoop => {
                let mut els = vec![self.bump()];
                els.push(self.block()?);
                Ok(node(SyntaxKind::LoopExpr, els))
            }
            K::LifetimeLabel => {
                let mut els = vec![self.bump()];
                self.expect(K::Colon, &mut els)?;
                self.expect(K::KwLoop, &mut els)?;
                els.push(self.block()?);
                Ok(node(SyntaxKind::LoopExpr, els))
            }
            K::KwFor => {
                let mut els = vec![self.bump()];
                els.push(self.pattern()?);
                self.expect(K::KwIn, &mut els)?;
                els.push(self.expr(true)?);
                els.push(self.block()?);
                Ok(node(SyntaxKind::ForExpr, els))
            }
            K::KwUnsafe => {
                let mut els = vec![self.bump()];
                els.push(self.block()?);
                Ok(node(SyntaxKind::UnsafeExpr, els))
            }
            _ => Err(Fail),
        }
    }

    fn if_expr(&mut self) -> R {
        let mut els = Els::new();
        self.expect(K::KwIf, &mut els)?;
        els.push(self.expr(true)?);
        els.push(self.block()?);
        if self.at(K::KwElse) {
            let mark = self.mark();
            let kw = self.bump();
            let alternative = if self.at(K::KwIf) {
                self.if_expr()
            } else {
                self.block()
            };
            match alternative {
                Ok(alt) => {
                    els.push(kw);
                    els.push(alt);
                }
                Err(Fail) => self.rollback(mark),
            }
        }
        Ok(node(SyntaxKind::IfExpr, els))
    }

    fn match_expr(&mut self) -> R {
        let mut els = Els::new();
        self.expect(K::KwMatch, &mut els)?;
        els.push(self.expr(true)?);
        self.expect(K::OpenBrace, &mut els)?;
        self.comma_list(&mut els, false, Self::match_arm)?;
        self.expect(K::CloseBrace, &mut els)?;
        Ok(node(SyntaxKind::MatchExpr, els))
    }

    fn match_arm(&mut self) -> R {
        let mut els = vec![self.pattern()?];
        if self.at(K::KwIf) {
            let mark = self.mark();
            let kw = self.bump();
            match self.expr(false) {
                Ok(guard) => {
                    els.push(kw);
                    els.push(guard);
                }
                Err(Fail) => self.rollback(mark),
            }
        }
        self.expect(K::FatArrow, &mut els)?;
        els.push(self.expr(false)?);
        Ok(node(SyntaxKind::MatchArm, els))
    }

    // ----- types (D) ------------------------------------------------------

    fn ty(&mut self) -> R {
        match self.peek() {
            K::OpenParen => {
                let mut els = vec![self.bump()];
                self.expect(K::CloseParen, &mut els)?;
                Ok(node(SyntaxKind::UnitType, els))
            }
            K::KwBox | K::KwShared | K::KwWeak => {
                let mut els = vec![self.bump()];
                els.push(self.type_args()?);
                Ok(node(SyntaxKind::WrapperType, els))
            }
            K::Ident | K::KwSelfType => {
                let mut els = vec![self.type_path()?];
                if self.at(K::OpenBracket) {
                    let mark = self.mark();
                    match self.type_args() {
                        Ok(args) => els.push(args),
                        Err(Fail) => self.rollback(mark),
                    }
                }
                Ok(node(SyntaxKind::PathType, els))
            }
            // `[` uniquely selects the fixed-capacity array type `[T; N]`
            // — no other type starts with a bracket (`TypeArguments` only
            // ever follow a path/wrapper head). ADR-0004 Stage 2.
            K::OpenBracket => {
                let mut els = vec![self.bump()];
                els.push(self.ty()?);
                self.expect(K::Semi, &mut els)?;
                self.expect(K::IntLiteral, &mut els)?;
                self.expect(K::CloseBracket, &mut els)?;
                Ok(node(SyntaxKind::FixedArrayType, els))
            }
            // `fn` at type position uniquely selects the function type
            // `fn(mode T, …) -> R` (ADR-0008 Tier 1). Modes are mandatory and
            // the return arrow+type are always written.
            K::KwFn => {
                let mut els = vec![self.bump()];
                self.expect(K::OpenParen, &mut els)?;
                self.comma_list(&mut els, false, Self::fn_type_param)?;
                self.expect(K::CloseParen, &mut els)?;
                let mut ret = Els::new();
                self.expect(K::ThinArrow, &mut ret)?;
                ret.push(self.ty()?);
                els.push(node(SyntaxKind::ReturnType, ret));
                Ok(node(SyntaxKind::FnType, els))
            }
            _ => Err(Fail),
        }
    }

    /// One function-type parameter: a mode keyword (`in`/`mut`/`take`) then a
    /// type, with no name (ADR-0008 Tier 1). Unlike a declaration parameter it
    /// has no `IDENT :`.
    fn fn_type_param(&mut self) -> R {
        let mode = match self.peek() {
            K::KwIn | K::KwMut | K::KwTake => self.bump(),
            _ => return Err(Fail),
        };
        let mut els = vec![mode];
        els.push(self.ty()?);
        Ok(node(SyntaxKind::FnTypeParam, els))
    }

    fn type_path(&mut self) -> R {
        let mut els = Els::new();
        match self.peek() {
            K::KwSelfType | K::Ident => els.push(self.bump()),
            _ => return Err(Fail),
        }
        while self.at(K::ColonColon) && self.peek2() == K::Ident {
            els.push(self.bump());
            els.push(self.bump());
        }
        Ok(node(SyntaxKind::TypePath, els))
    }

    fn type_args(&mut self) -> R {
        let mut els = Els::new();
        self.expect(K::OpenBracket, &mut els)?;
        self.comma_list(&mut els, true, Self::ty)?;
        self.expect(K::CloseBracket, &mut els)?;
        Ok(node(SyntaxKind::TypeArguments, els))
    }

    // ----- patterns (H) ---------------------------------------------------

    fn pattern(&mut self) -> R {
        let first = self.pattern_primary()?;
        if !self.at(K::Pipe) {
            return Ok(first);
        }
        let mut els = vec![first];
        while self.at(K::Pipe) {
            let mark = self.mark();
            let pipe = self.bump();
            match self.pattern_primary() {
                Ok(alt) => {
                    els.push(pipe);
                    els.push(alt);
                }
                Err(Fail) => {
                    self.rollback(mark);
                    break;
                }
            }
        }
        if els.len() == 1 {
            Ok(els.pop().expect("list holds the first pattern"))
        } else {
            Ok(node(SyntaxKind::OrPattern, els))
        }
    }

    fn pattern_primary(&mut self) -> R {
        match self.peek() {
            K::IntLiteral
            | K::FloatLiteral
            | K::BoolLiteral
            | K::CharLiteral
            | K::StringLiteral => Ok(node(SyntaxKind::LiteralPattern, vec![self.bump()])),
            K::OpenParen if self.peek2() == K::CloseParen => Ok(node(
                SyntaxKind::LiteralPattern,
                vec![self.bump(), self.bump()],
            )),
            K::Underscore => Ok(node(SyntaxKind::WildcardPattern, vec![self.bump()])),
            K::OpenParen => {
                let mut els = vec![self.bump()];
                els.push(self.pattern()?);
                self.expect(K::CloseParen, &mut els)?;
                Ok(node(SyntaxKind::GroupPattern, els))
            }
            K::Ident | K::KwSelfType => {
                // A path with a field block (the type path becomes a node)…
                let mark = self.mark();
                if let Ok(path) = self.type_path() {
                    if self.at(K::OpenBrace) {
                        if let Ok(fields) = self.field_pattern_block() {
                            return Ok(node(SyntaxKind::PathPattern, vec![path, fields]));
                        }
                    }
                }
                self.rollback(mark);
                // …or a multi-segment path (raw tokens), or a binding.
                let first_kind = self.peek();
                let first = self.bump();
                let mut segments = Els::new();
                while self.at(K::ColonColon) && self.peek2() == K::Ident {
                    segments.push(self.bump());
                    segments.push(self.bump());
                }
                if !segments.is_empty() {
                    let mut els = vec![first];
                    els.append(&mut segments);
                    return Ok(node(SyntaxKind::PathPattern, els));
                }
                if first_kind == K::Ident {
                    Ok(node(SyntaxKind::BindingPattern, vec![first]))
                } else {
                    Err(Fail)
                }
            }
            _ => Err(Fail),
        }
    }

    /// `{ .. }` | `{ field (, field)* (, ..)? ,? }` | `{ }`.
    fn field_pattern_block(&mut self) -> R {
        let mut els = Els::new();
        self.expect(K::OpenBrace, &mut els)?;
        match self.peek() {
            K::DotDot => {
                els.push(node(SyntaxKind::RestPattern, vec![self.bump()]));
                if self.at(K::Comma) {
                    els.push(self.bump());
                }
            }
            K::CloseBrace => {}
            _ => {
                els.push(self.field_pattern()?);
                while self.at(K::Comma) {
                    let mark = self.mark();
                    let comma = self.bump();
                    if self.at(K::DotDot) {
                        els.push(comma);
                        els.push(node(SyntaxKind::RestPattern, vec![self.bump()]));
                        if self.at(K::Comma) {
                            els.push(self.bump());
                        }
                        break;
                    }
                    match self.field_pattern() {
                        Ok(field) => {
                            els.push(comma);
                            els.push(field);
                        }
                        Err(Fail) => {
                            self.rollback(mark);
                            els.push(self.bump());
                            break;
                        }
                    }
                }
            }
        }
        self.expect(K::CloseBrace, &mut els)?;
        Ok(node(SyntaxKind::FieldPatternBlock, els))
    }

    fn field_pattern(&mut self) -> R {
        let mut els = Els::new();
        self.expect(K::Ident, &mut els)?;
        if self.at(K::Colon) {
            let mark = self.mark();
            let colon = self.bump();
            match self.pattern() {
                Ok(inner) => {
                    els.push(colon);
                    els.push(inner);
                }
                Err(Fail) => self.rollback(mark),
            }
        }
        Ok(node(SyntaxKind::FieldPattern, els))
    }
}
