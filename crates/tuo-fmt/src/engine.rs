//! The formatting engine: a canonical-layout emitter over the lossless CST.
//!
//! The emitter walks the syntax tree and rebuilds the text with fixed v0
//! layout rules (4-space indentation, one canonical brace style — neither is
//! configurable). Token *lexemes* are never rewritten; the formatter only
//! decides the whitespace between them, normalizes separator commas, and
//! reproduces recovered [`SyntaxKind::Error`] material byte-for-byte.

use tuo_lexer::{Token, TokenKind};
use tuo_syntax::{SyntaxElement, SyntaxKind, SyntaxNode, SyntaxTree};

/// Format a parsed tree back to canonical text.
///
/// `text` must be the exact source the tree was parsed from.
pub(crate) fn format_tree(tree: &SyntaxTree, text: &str) -> String {
    let mut writer = Writer {
        text,
        tokens: &tree.lex.tokens,
        out: String::with_capacity(text.len() + text.len() / 8),
        indent: 0,
        brk: Brk::None,
        last: None,
    };
    source_file(&mut writer, &tree.root);
    writer.flush_eof_comments();
    let mut out = writer.out;
    // Exactly one trailing newline on non-empty output.
    while out.ends_with('\n') {
        out.pop();
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// The pending separator before the next emitted token.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Brk {
    /// Decide from the token-pair spacing rules.
    Auto,
    /// Force no separator.
    None,
    /// Force a single space.
    Space,
    /// Break the line.
    Newline,
    /// Break the line and leave one blank line.
    Blank,
}

impl Brk {
    fn rank(self) -> u8 {
        match self {
            Self::Auto | Self::None | Self::Space => 0,
            Self::Newline => 1,
            Self::Blank => 2,
        }
    }

    /// The stronger of two separators (line breaks win over inline forms).
    fn max(self, other: Self) -> Self {
        if other.rank() > self.rank() { other } else { self }
    }
}

struct Writer<'a> {
    text: &'a str,
    tokens: &'a [Token],
    out: String,
    indent: usize,
    brk: Brk,
    /// Stream index of the last emitted token.
    last: Option<u32>,
}

impl<'a> Writer<'a> {
    fn lexeme(&self, index: u32) -> &'a str {
        let range = self.tokens[index as usize].range();
        &self.text[range.start().as_usize()..range.end().as_usize()]
    }

    fn kind(&self, index: u32) -> TokenKind {
        self.tokens[index as usize].kind
    }

    fn token_start(&self, index: u32) -> usize {
        self.tokens[index as usize].range().start().as_usize()
    }

    fn token_end(&self, index: u32) -> usize {
        self.tokens[index as usize].range().end().as_usize()
    }

    /// Newlines in the original text between byte offsets `from..to`.
    fn newlines_between(&self, from: usize, to: usize) -> usize {
        self.text[from..to].matches('\n').count()
    }

    /// The `//` comments strictly between the last emitted token and `upto`.
    fn pending_comments(&self, upto: u32) -> Vec<u32> {
        let start = self.last.map_or(0, |index| index + 1);
        (start..upto)
            .filter(|&index| self.kind(index) == TokenKind::LineComment)
            .collect()
    }

    /// Where the next entity (comment or token) begins, for blank-line
    /// preservation between elements.
    fn next_entity_start(&self, token: u32) -> usize {
        self.pending_comments(token)
            .first()
            .map_or_else(|| self.token_start(token), |&c| self.token_start(c))
    }

    /// The break to use before an element whose first token is `token`:
    /// a newline, widened to one blank line if the author left one.
    fn line_break_before(&self, token: u32) -> Brk {
        let Some(last) = self.last else {
            return Brk::None;
        };
        let gap = self.newlines_between(self.token_end(last), self.next_entity_start(token));
        if gap >= 2 { Brk::Blank } else { Brk::Newline }
    }

    fn push_indent(&mut self) {
        for _ in 0..self.indent {
            self.out.push_str("    ");
        }
    }

    fn apply_break(&mut self, brk: Brk, next: TokenKind) {
        match brk {
            Brk::None => {}
            Brk::Space => self.out.push(' '),
            Brk::Newline => {
                self.out.push('\n');
                self.push_indent();
            }
            Brk::Blank => {
                self.out.push_str("\n\n");
                self.push_indent();
            }
            Brk::Auto => {
                let sep = auto_sep(self.last.map(|index| self.kind(index)), next);
                self.out.push_str(sep);
            }
        }
    }

    /// Emit the token at stream index `index`, flushing any `//` comments
    /// that precede it (trailing comments stay on the previous line;
    /// own-line comments keep their own line and up to one blank).
    fn write_token(&mut self, index: u32) {
        let mut brk = std::mem::replace(&mut self.brk, Brk::Auto);
        let mut prev_end = self.last.map(|last| self.token_end(last));
        for comment in self.pending_comments(index) {
            let start = self.token_start(comment);
            match prev_end {
                Some(end) if self.newlines_between(end, start) == 0 => {
                    // Trailing comment: attach to the current line; a line
                    // comment always ends its line.
                    self.out.push(' ');
                    self.out.push_str(self.lexeme(comment));
                    brk = brk.max(Brk::Newline);
                }
                _ => {
                    let gap = prev_end.map_or(0, |end| self.newlines_between(end, start));
                    let before = if gap >= 2 { Brk::Blank } else { Brk::Newline };
                    if prev_end.is_some() {
                        self.apply_break(brk.max(before), TokenKind::LineComment);
                    }
                    self.out.push_str(self.lexeme(comment));
                    brk = Brk::Newline;
                }
            }
            prev_end = Some(self.token_end(comment));
        }
        if brk.rank() >= Brk::Newline.rank() {
            // A comment (or the caller) broke the line; widen to a blank
            // line if the author left one between the last entity and the
            // token.
            if let Some(end) = prev_end {
                let gap = self.newlines_between(end, self.token_start(index));
                if gap >= 2 {
                    brk = Brk::Blank;
                }
            }
        }
        self.apply_break(brk, self.kind(index));
        self.out.push_str(self.lexeme(index));
        self.last = Some(index);
        // A doc comment is a whole-line token: nothing may share its line.
        if self.kind(index) == TokenKind::DocComment {
            self.brk = Brk::Newline;
        }
    }

    /// Emit recovered material byte-for-byte (interior trivia included):
    /// the conservative treatment of malformed regions.
    fn write_error_island(&mut self, node: &SyntaxNode) {
        let (Some(first), Some(last)) = (node.first_token(), node.last_token()) else {
            return;
        };
        // Flush comments before the island, then break onto its own line.
        let mut brk = std::mem::replace(&mut self.brk, Brk::Auto);
        let mut prev_end = self.last.map(|index| self.token_end(index));
        for comment in self.pending_comments(first) {
            let start = self.token_start(comment);
            match prev_end {
                Some(end) if self.newlines_between(end, start) == 0 => {
                    self.out.push(' ');
                    self.out.push_str(self.lexeme(comment));
                    brk = brk.max(Brk::Newline);
                }
                _ => {
                    if prev_end.is_some() {
                        self.apply_break(brk.max(Brk::Newline), TokenKind::LineComment);
                    }
                    self.out.push_str(self.lexeme(comment));
                    brk = Brk::Newline;
                }
            }
            prev_end = Some(self.token_end(comment));
        }
        if prev_end.is_some() {
            self.apply_break(brk.max(Brk::Newline), TokenKind::Error);
        }
        self.out
            .push_str(&self.text[self.token_start(first)..self.token_end(last)]);
        self.last = Some(last);
        self.brk = Brk::Auto;
    }

    /// Flush comments that trail the last syntactic token of the file.
    fn flush_eof_comments(&mut self) {
        let total = u32::try_from(self.tokens.len()).expect("token count fits in u32");
        let comments = self.pending_comments(total);
        let mut prev_end = self.last.map(|index| self.token_end(index));
        for comment in comments {
            let start = self.token_start(comment);
            match prev_end {
                Some(end) if self.newlines_between(end, start) == 0 => {
                    self.out.push(' ');
                    self.out.push_str(self.lexeme(comment));
                }
                _ => {
                    let gap = prev_end.map_or(0, |end| self.newlines_between(end, start));
                    if prev_end.is_some() {
                        let brk = if gap >= 2 { Brk::Blank } else { Brk::Newline };
                        self.apply_break(brk, TokenKind::LineComment);
                    }
                    self.out.push_str(self.lexeme(comment));
                }
            }
            prev_end = Some(self.token_end(comment));
            self.last = Some(comment);
        }
    }
}

/// The token-pair spacing rules for inline (single-line) layout.
fn auto_sep(prev: Option<TokenKind>, next: TokenKind) -> &'static str {
    use TokenKind::*;
    let Some(prev) = prev else {
        return "";
    };
    // Tokens that never take a space before them.
    if matches!(
        next,
        Comma | Semi | Question | Dot | ColonColon | Colon | CloseParen | CloseBracket
    ) {
        return "";
    }
    if next == CloseBrace && prev == OpenBrace {
        return "";
    }
    // Tokens that never take a space after them.
    if matches!(prev, OpenParen | OpenBracket | Dot | ColonColon | Bang) {
        return "";
    }
    // Call/index/generic openers glue to a completed operand on the left,
    // but keep a space after keywords and operators.
    if matches!(next, OpenParen | OpenBracket) {
        return if matches!(
            prev,
            Ident
                | KwSelfValue
                | KwSelfType
                | CloseParen
                | CloseBracket
                | CloseBrace
                | Question
                | IntLiteral
                | FloatLiteral
                | BoolLiteral
                | CharLiteral
                | StringLiteral
                | KwBox
                | KwShared
                | KwWeak
        ) {
            ""
        } else {
            " "
        };
    }
    " "
}

// ---------------------------------------------------------------------------
// Structure: which nodes lay out multiple lines, and how.
// ---------------------------------------------------------------------------

/// The whole file: the module declaration and items, one per line, blank
/// lines between them preserved (capped at one).
fn source_file(w: &mut Writer<'_>, root: &SyntaxNode) {
    for child in &root.children {
        if let SyntaxElement::Node(node) = child {
            w.brk = node
                .first_token()
                .map_or(Brk::None, |first| w.line_break_before(first));
            emit(w, node);
        }
    }
}

/// Dispatch one node to its layout.
fn emit(w: &mut Writer<'_>, node: &SyntaxNode) {
    match node.kind {
        SyntaxKind::Error => w.write_error_island(node),
        SyntaxKind::Block => block(w, node),
        SyntaxKind::StructItem => struct_item(w, node),
        SyntaxKind::EnumItem | SyntaxKind::MatchExpr => braced_body(w, node, true),
        SyntaxKind::InterfaceItem | SyntaxKind::ImplItem | SyntaxKind::SpecItem => {
            braced_body(w, node, false);
        }
        SyntaxKind::UnaryExpr => unary(w, node),
        SyntaxKind::ImportGroup => import_group(w, node),
        _ => inline(w, node),
    }
}

/// Emit a node's children on one line, applying the pair-spacing rules and
/// dropping trailing separator commas before a closing delimiter.
fn inline(w: &mut Writer<'_>, node: &SyntaxNode) {
    for (position, child) in node.children.iter().enumerate() {
        match child {
            SyntaxElement::Token(index) => {
                if w.kind(*index) == TokenKind::Comma && closes_next(w, node, position) {
                    continue;
                }
                w.write_token(*index);
            }
            SyntaxElement::Node(nested) => emit(w, nested),
        }
    }
}

/// Is the element after `position` a closing delimiter (so a comma here is a
/// trailing separator to drop in inline layout)?
fn closes_next(w: &Writer<'_>, node: &SyntaxNode, position: usize) -> bool {
    node.children.get(position + 1).is_some_and(|next| match next {
        SyntaxElement::Token(index) => matches!(
            w.kind(*index),
            TokenKind::CloseParen | TokenKind::CloseBracket | TokenKind::CloseBrace
        ),
        SyntaxElement::Node(_) => false,
    })
}

/// A `{ … }` code block: statements and the tail expression one per line.
fn block(w: &mut Writer<'_>, node: &SyntaxNode) {
    let children = &node.children;
    let Some((first, rest)) = children.split_first() else {
        return;
    };
    let SyntaxElement::Token(open) = first else {
        return inline(w, node);
    };
    w.write_token(*open);
    let Some((last, middle)) = rest.split_last() else {
        return;
    };
    if middle.is_empty() {
        // `{}` — the canonical empty block.
        if let SyntaxElement::Token(close) = last {
            w.brk = Brk::None;
            w.write_token(*close);
        }
        return;
    }
    w.indent += 1;
    for child in middle {
        line_element(w, child);
    }
    w.indent -= 1;
    if let SyntaxElement::Token(close) = last {
        w.brk = Brk::Newline;
        w.write_token(*close);
    }
}

/// Emit one element on its own line (blank lines preserved, capped at one).
fn line_element(w: &mut Writer<'_>, child: &SyntaxElement) {
    match child {
        SyntaxElement::Token(index) => {
            w.brk = w.line_break_before(*index).max(Brk::Newline);
            w.write_token(*index);
        }
        SyntaxElement::Node(nested) => {
            if let Some(first) = nested.first_token() {
                w.brk = w.line_break_before(first).max(Brk::Newline);
            }
            emit(w, nested);
        }
    }
}

/// A struct declaration: inline header, then its field block over lines.
fn struct_item(w: &mut Writer<'_>, node: &SyntaxNode) {
    for child in &node.children {
        match child {
            SyntaxElement::Token(index) => w.write_token(*index),
            SyntaxElement::Node(nested) if nested.kind == SyntaxKind::FieldBlock => {
                field_block_lines(w, nested);
            }
            SyntaxElement::Node(nested) => emit(w, nested),
        }
    }
}

/// A field block laid out one field per line with enforced trailing commas
/// (used by struct declarations; enum-variant payloads stay inline).
fn field_block_lines(w: &mut Writer<'_>, node: &SyntaxNode) {
    braced_lines(w, &node.children, true);
}

/// Inline header up to the `{`, then one element per line until the `}`.
/// With `commas`, every element gets a trailing separator comma.
fn braced_body(w: &mut Writer<'_>, node: &SyntaxNode, commas: bool) {
    braced_lines(w, &node.children, commas);
}

fn braced_lines(w: &mut Writer<'_>, children: &[SyntaxElement], commas: bool) {
    let open_at = children.iter().position(|child| match child {
        SyntaxElement::Token(index) => w.kind(*index) == TokenKind::OpenBrace,
        SyntaxElement::Node(_) => false,
    });
    let Some(open_at) = open_at else {
        // No brace (malformed shapes never reach here, but stay total).
        for child in children {
            match child {
                SyntaxElement::Token(index) => w.write_token(*index),
                SyntaxElement::Node(nested) => emit(w, nested),
            }
        }
        return;
    };
    // Header, inline.
    for child in &children[..=open_at] {
        match child {
            SyntaxElement::Token(index) => w.write_token(*index),
            SyntaxElement::Node(nested) => emit(w, nested),
        }
    }
    let body = &children[open_at + 1..children.len() - 1];
    if body.is_empty() {
        if let Some(SyntaxElement::Token(close)) = children.last() {
            w.brk = Brk::None;
            w.write_token(*close);
        }
        return;
    }
    w.indent += 1;
    let mut previous_was_pub = false;
    let mut body_iter = body.iter().peekable();
    while let Some(child) = body_iter.next() {
        match child {
            SyntaxElement::Token(index) => {
                let kind = w.kind(*index);
                if kind == TokenKind::Comma {
                    // Separator commas are re-emitted right after their
                    // element below; drop them here.
                    continue;
                }
                if previous_was_pub {
                    w.write_token(*index);
                } else {
                    w.brk = w.line_break_before(*index).max(Brk::Newline);
                    w.write_token(*index);
                }
                previous_was_pub = kind == TokenKind::KwPub;
            }
            SyntaxElement::Node(nested) => {
                if !previous_was_pub {
                    if let Some(first) = nested.first_token() {
                        w.brk = w.line_break_before(first).max(Brk::Newline);
                    }
                }
                emit(w, nested);
                previous_was_pub = false;
                if commas && nested.kind != SyntaxKind::Error {
                    // Enforce the trailing separator comma.
                    if let Some(SyntaxElement::Token(comma)) = body_iter.peek().copied()
                        && w.kind(*comma) == TokenKind::Comma
                    {
                        w.brk = Brk::None;
                        let comma = *comma;
                        body_iter.next();
                        w.write_token(comma);
                    } else {
                        w.out.push(',');
                    }
                }
            }
        }
    }
    w.indent -= 1;
    if let Some(SyntaxElement::Token(close)) = children.last() {
        w.brk = Brk::Newline;
        w.write_token(*close);
    }
}

/// A prefix operator: `-` and `!` glue to their operand; `move` keeps a
/// space.
fn unary(w: &mut Writer<'_>, node: &SyntaxNode) {
    let mut children = node.children.iter();
    if let Some(SyntaxElement::Token(op)) = children.next() {
        let glue = !matches!(w.kind(*op), TokenKind::KwMove);
        w.write_token(*op);
        if glue {
            w.brk = Brk::None;
        }
    }
    for child in children {
        match child {
            SyntaxElement::Token(index) => w.write_token(*index),
            SyntaxElement::Node(nested) => emit(w, nested),
        }
    }
}

/// An import group `::{a, b as c}`: braces glued tight to the leaves.
fn import_group(w: &mut Writer<'_>, node: &SyntaxNode) {
    for (position, child) in node.children.iter().enumerate() {
        match child {
            SyntaxElement::Token(index) => {
                let kind = w.kind(*index);
                if kind == TokenKind::Comma && closes_next(w, node, position) {
                    continue;
                }
                if kind == TokenKind::CloseBrace {
                    w.brk = Brk::None;
                }
                w.write_token(*index);
                if kind == TokenKind::OpenBrace {
                    w.brk = Brk::None;
                }
            }
            SyntaxElement::Node(nested) => emit(w, nested),
        }
    }
}
