//! The parser's view of the token stream.
//!
//! The lexer's stream is lossless (trivia included); the parser consumes the
//! trivia-stripped view, with each element remembering its index into the
//! full stream so the syntax tree can point back at exact tokens.

use std::fmt;

use tuo_lexer::{LexResult, TokenKind};

/// One parse-stream element: a token kind plus the token's index in the full
/// lossless stream.
///
/// Equality compares **kinds only** — the index is location bookkeeping.
/// This is what lets the grammar say "expect a `;`" by value while every
/// matched element still carries its exact position.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Tok {
    /// The token's kind.
    pub(crate) kind: TokenKind,
    /// Index into `LexResult::tokens`.
    pub(crate) index: u32,
}

impl PartialEq for Tok {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
    }
}

impl Eq for Tok {}

/// A pattern element for the grammar: `kind` with a placeholder index.
pub(crate) const fn t(kind: TokenKind) -> Tok {
    Tok {
        kind,
        index: u32::MAX,
    }
}

/// The trivia-stripped parse stream (doc comments included, EOF excluded).
pub(crate) fn parse_stream(lex: &LexResult) -> Vec<Tok> {
    lex.tokens
        .iter()
        .enumerate()
        .filter(|(_, token)| !token.is_trivia() && token.kind != TokenKind::Eof)
        .map(|(index, token)| Tok {
            kind: token.kind,
            index: u32::try_from(index).expect("token count fits in u32"),
        })
        .collect()
}

/// The user-facing name of a token kind, for diagnostics.
pub(crate) fn kind_name(kind: TokenKind) -> &'static str {
    use TokenKind::*;
    match kind {
        Whitespace => "whitespace",
        LineComment => "comment",
        DocComment => "doc comment",
        Ident => "identifier",
        Underscore => "`_`",
        IntLiteral => "integer literal",
        FloatLiteral => "float literal",
        BoolLiteral => "boolean literal",
        CharLiteral => "character literal",
        StringLiteral => "string literal",
        KwModule => "`module`",
        KwImport => "`import`",
        KwPub => "`pub`",
        KwFn => "`fn`",
        KwLet => "`let`",
        KwVar => "`var`",
        KwConst => "`const`",
        KwStruct => "`struct`",
        KwEnum => "`enum`",
        KwInterface => "`interface`",
        KwImpl => "`impl`",
        KwWhere => "`where`",
        KwIf => "`if`",
        KwElse => "`else`",
        KwMatch => "`match`",
        KwFor => "`for`",
        KwIn => "`in`",
        KwWhile => "`while`",
        KwLoop => "`loop`",
        KwBreak => "`break`",
        KwContinue => "`continue`",
        KwReturn => "`return`",
        KwTake => "`take`",
        KwMut => "`mut`",
        KwMove => "`move`",
        KwBox => "`Box`",
        KwShared => "`Shared`",
        KwWeak => "`Weak`",
        KwSpec => "`spec`",
        KwGiven => "`given`",
        KwWhen => "`when`",
        KwThen => "`then`",
        KwAssert => "`assert`",
        KwUnsafe => "`unsafe`",
        KwAs => "`as`",
        KwSelfValue => "`self`",
        KwSelfType => "`Self`",
        ThinArrow => "`->`",
        FatArrow => "`=>`",
        ColonColon => "`::`",
        DotDot => "`..`",
        EqEq => "`==`",
        NotEq => "`!=`",
        Lt => "`<`",
        Gt => "`>`",
        LtEq => "`<=`",
        GtEq => "`>=`",
        AmpAmp => "`&&`",
        PipePipe => "`||`",
        Pipe => "`|`",
        Bang => "`!`",
        Plus => "`+`",
        Minus => "`-`",
        Star => "`*`",
        Slash => "`/`",
        Percent => "`%`",
        Eq => "`=`",
        Question => "`?`",
        Dot => "`.`",
        OpenParen => "`(`",
        CloseParen => "`)`",
        OpenBrace => "`{`",
        CloseBrace => "`}`",
        OpenBracket => "`[`",
        CloseBracket => "`]`",
        Comma => "`,`",
        Semi => "`;`",
        Colon => "`:`",
        LifetimeLabel => "label",
        Error => "invalid token",
        Eof => "end of file",
    }
}

impl fmt::Display for Tok {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(kind_name(self.kind))
    }
}
