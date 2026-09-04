//! TDG-owned token structures.
//!
//! These types are the lexer's entire public vocabulary. The lexing engine
//! behind them (currently [Logos](https://crates.io/crates/logos)) is an
//! implementation detail of [`crate::lex`] and none of its types appear here.

use tuo_source::{Span, TextRange};

use tuo_diagnostics::Diagnostic;

/// The closed set of token classes the lexer emits.
///
/// This is the terminal alphabet of the concrete grammar
/// (`specification/grammar.ebnf` section (A)) plus the **trivia** kinds
/// ([`Whitespace`](Self::Whitespace), [`LineComment`](Self::LineComment))
/// that the lossless syntax layer retains for formatting and tooling. Doc
/// comments are *not* trivia: they are captured and attached to the following
/// item (§3).
///
/// `true` and `false` are keywords of the language (§4) but surface here as
/// [`BoolLiteral`](Self::BoolLiteral), exactly as the grammar's terminal set
/// specifies (§10).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TokenKind {
    // --- trivia (retained for the lossless syntax layer) ---
    /// A run of spaces and line feeds (§5: whitespace is never significant).
    Whitespace,
    /// A `// …` comment, up to but excluding the newline. Trivia (§3).
    LineComment,

    // --- captured comments ---
    /// A `/// …` doc comment, attached to the following item (§3).
    DocComment,

    // --- identifiers ---
    /// An identifier: `xid_start xid_continue*`, in NFC, not a keyword (§2).
    Ident,
    /// The lone `_` wildcard — reserved, not an identifier (§2).
    Underscore,

    // --- literals (§10, §11) ---
    /// An integer literal: decimal, `0x`, `0o`, or `0b`, optionally suffixed.
    IntLiteral,
    /// A floating-point literal: `1.5`, `1.5e3`, `2e8`, optionally suffixed.
    FloatLiteral,
    /// `true` or `false` (keywords surfacing as one literal class, §10).
    BoolLiteral,
    /// A character literal: one Unicode scalar or escape between `'` quotes.
    CharLiteral,
    /// A string literal: `"…"` with escapes; single-line, no interpolation.
    StringLiteral,

    // --- keywords (§4: fully reserved, no contextual keywords) ---
    /// The `module` keyword.
    KwModule,
    /// The `import` keyword.
    KwImport,
    /// The `pub` keyword.
    KwPub,
    /// The `fn` keyword.
    KwFn,
    /// The `let` keyword.
    KwLet,
    /// The `var` keyword.
    KwVar,
    /// The `const` keyword.
    KwConst,
    /// The `struct` keyword.
    KwStruct,
    /// The `enum` keyword.
    KwEnum,
    /// The `interface` keyword.
    KwInterface,
    /// The `impl` keyword.
    KwImpl,
    /// The `where` keyword.
    KwWhere,
    /// The `if` keyword.
    KwIf,
    /// The `else` keyword.
    KwElse,
    /// The `match` keyword.
    KwMatch,
    /// The `for` keyword.
    KwFor,
    /// The `in` keyword.
    KwIn,
    /// The `while` keyword.
    KwWhile,
    /// The `loop` keyword.
    KwLoop,
    /// The `break` keyword.
    KwBreak,
    /// The `continue` keyword.
    KwContinue,
    /// The `return` keyword.
    KwReturn,
    /// The `take` keyword.
    KwTake,
    /// The `mut` keyword.
    KwMut,
    /// The `move` keyword.
    KwMove,
    /// The `Box` keyword.
    KwBox,
    /// The `Shared` keyword.
    KwShared,
    /// The `Weak` keyword.
    KwWeak,
    /// The `spec` keyword.
    KwSpec,
    /// The `given` keyword.
    KwGiven,
    /// The `when` keyword.
    KwWhen,
    /// The `then` keyword.
    KwThen,
    /// The `assert` keyword (reserved for spec assertions, §27).
    KwAssert,
    /// The `unsafe` keyword.
    KwUnsafe,
    /// The `as` keyword.
    KwAs,
    /// The `self` keyword (value receiver, §19).
    KwSelfValue,
    /// The `Self` keyword (the implementing type, §19).
    KwSelfType,

    // --- punctuation and operators (§6, maximal munch) ---
    /// `->`
    ThinArrow,
    /// `=>`
    FatArrow,
    /// `::`
    ColonColon,
    /// `..`
    DotDot,
    /// `==`
    EqEq,
    /// `!=`
    NotEq,
    /// `<` (comparison only — generics use `[T]`, §18)
    Lt,
    /// `>` (comparison only)
    Gt,
    /// `<=`
    LtEq,
    /// `>=`
    GtEq,
    /// `&&`
    AmpAmp,
    /// `||`
    PipePipe,
    /// `|` — the pattern-alternative separator (§17) and, since ADR-0019,
    /// the bitwise-or operator. One token, disambiguated by grammatical
    /// context: `pattern` and the expression chain are disjoint productions,
    /// so the parser always knows which it is reading.
    Pipe,
    /// `&` — bitwise and (ADR-0019).
    Amp,
    /// `^` — bitwise xor (ADR-0019).
    Caret,
    /// `~` — bitwise not, unary (ADR-0019).
    Tilde,
    /// `#` — introduces an attribute, as `#[name]` (ADR-0020 Stage C).
    ///
    /// Only ever appears immediately before `[` in an attribute prefix; it is
    /// not an operator and has no meaning in an expression.
    Hash,
    /// `<<` — left shift (ADR-0019).
    LtLt,
    /// `>>` — right shift (ADR-0019); arithmetic on signed operands,
    /// logical on unsigned.
    GtGt,
    /// `!`
    Bang,
    /// `+`
    Plus,
    /// `-`
    Minus,
    /// `*`
    Star,
    /// `/`
    Slash,
    /// `%`
    Percent,
    /// `=`
    Eq,
    /// `?`
    Question,
    /// `.`
    Dot,
    /// `(`
    OpenParen,
    /// `)`
    CloseParen,
    /// `{`
    OpenBrace,
    /// `}`
    CloseBrace,
    /// `[`
    OpenBracket,
    /// `]`
    CloseBracket,
    /// `,`
    Comma,
    /// `;`
    Semi,
    /// `:`
    Colon,

    // --- experimental (§16) ---
    /// A loop label: `'` followed by an identifier. Experimental — the sigil
    /// may change within v0.
    LifetimeLabel,

    // --- synthetic ---
    /// A synthetic error token covering text the lexer diagnosed and skipped
    /// past (§9: the lexer keeps going and resynchronizes).
    Error,
    /// End of input: a zero-width token at the end of the source.
    Eof,
}

impl TokenKind {
    /// Is this token trivia the parser skips entirely (whitespace or a `//`
    /// line comment)? Doc comments are **not** trivia — they are captured and
    /// attached to the next item.
    #[must_use]
    pub const fn is_trivia(self) -> bool {
        matches!(self, Self::Whitespace | Self::LineComment)
    }

    /// Is this one of the reserved keywords (§4)?
    ///
    /// `true`/`false` answer `false` here: they surface as
    /// [`TokenKind::BoolLiteral`], which is a literal class, not a keyword
    /// token.
    #[must_use]
    pub const fn is_keyword(self) -> bool {
        matches!(
            self,
            Self::KwModule
                | Self::KwImport
                | Self::KwPub
                | Self::KwFn
                | Self::KwLet
                | Self::KwVar
                | Self::KwConst
                | Self::KwStruct
                | Self::KwEnum
                | Self::KwInterface
                | Self::KwImpl
                | Self::KwWhere
                | Self::KwIf
                | Self::KwElse
                | Self::KwMatch
                | Self::KwFor
                | Self::KwIn
                | Self::KwWhile
                | Self::KwLoop
                | Self::KwBreak
                | Self::KwContinue
                | Self::KwReturn
                | Self::KwTake
                | Self::KwMut
                | Self::KwMove
                | Self::KwBox
                | Self::KwShared
                | Self::KwWeak
                | Self::KwSpec
                | Self::KwGiven
                | Self::KwWhen
                | Self::KwThen
                | Self::KwAssert
                | Self::KwUnsafe
                | Self::KwAs
                | Self::KwSelfValue
                | Self::KwSelfType
        )
    }

    /// Is this one of the literal classes (§10)?
    #[must_use]
    pub const fn is_literal(self) -> bool {
        matches!(
            self,
            Self::IntLiteral
                | Self::FloatLiteral
                | Self::BoolLiteral
                | Self::CharLiteral
                | Self::StringLiteral
        )
    }

    /// The keyword token for `lexeme`, if it is one of the reserved words
    /// (§4). `true`/`false` map to [`TokenKind::BoolLiteral`].
    #[must_use]
    pub fn from_keyword(lexeme: &str) -> Option<Self> {
        Some(match lexeme {
            "module" => Self::KwModule,
            "import" => Self::KwImport,
            "pub" => Self::KwPub,
            "fn" => Self::KwFn,
            "let" => Self::KwLet,
            "var" => Self::KwVar,
            "const" => Self::KwConst,
            "struct" => Self::KwStruct,
            "enum" => Self::KwEnum,
            "interface" => Self::KwInterface,
            "impl" => Self::KwImpl,
            "where" => Self::KwWhere,
            "if" => Self::KwIf,
            "else" => Self::KwElse,
            "match" => Self::KwMatch,
            "for" => Self::KwFor,
            "in" => Self::KwIn,
            "while" => Self::KwWhile,
            "loop" => Self::KwLoop,
            "break" => Self::KwBreak,
            "continue" => Self::KwContinue,
            "return" => Self::KwReturn,
            "take" => Self::KwTake,
            "mut" => Self::KwMut,
            "move" => Self::KwMove,
            "Box" => Self::KwBox,
            "Shared" => Self::KwShared,
            "Weak" => Self::KwWeak,
            "spec" => Self::KwSpec,
            "given" => Self::KwGiven,
            "when" => Self::KwWhen,
            "then" => Self::KwThen,
            "assert" => Self::KwAssert,
            "unsafe" => Self::KwUnsafe,
            "as" => Self::KwAs,
            "true" | "false" => Self::BoolLiteral,
            "self" => Self::KwSelfValue,
            "Self" => Self::KwSelfType,
            _ => return None,
        })
    }
}

/// One token: a kind plus its exact source location.
///
/// The span is byte-exact: for any lexed source, the non-[`Eof`]
/// (`TokenKind::Eof`) tokens tile the input text completely and in order —
/// nothing, including whitespace and comments, is dropped. That property is
/// what the lossless syntax layer builds on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Token {
    /// What the token is.
    pub kind: TokenKind,
    /// Exactly where it is, anchored to the source snapshot it was lexed from.
    pub span: Span,
}

impl Token {
    /// The token's byte range within its source.
    #[must_use]
    pub fn range(&self) -> TextRange {
        self.span.range()
    }

    /// Shorthand for [`TokenKind::is_trivia`].
    #[must_use]
    pub fn is_trivia(&self) -> bool {
        self.kind.is_trivia()
    }
}

/// Everything the lexer produced for one source snapshot.
///
/// Lexing never aborts: on malformed input it emits [`TokenKind::Error`]
/// tokens plus [`Diagnostic`]s and keeps going (§9), so the parser always
/// receives a complete stream.
#[derive(Clone, Debug)]
pub struct LexResult {
    /// Every token, in source order, ending with [`TokenKind::Eof`]. Trivia
    /// included.
    pub tokens: Vec<Token>,
    /// Lexical diagnostics (`Lxxxx` codes), in source order.
    pub diagnostics: Vec<Diagnostic>,
}

impl LexResult {
    /// The tokens the parser consumes: everything except trivia
    /// (doc comments stay — they attach to items).
    pub fn parser_tokens(&self) -> impl Iterator<Item = &Token> {
        self.tokens.iter().filter(|token| !token.is_trivia())
    }

    /// Did lexing produce any diagnostics?
    #[must_use]
    pub fn has_errors(&self) -> bool {
        !self.diagnostics.is_empty()
    }
}
