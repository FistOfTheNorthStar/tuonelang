//! The closed set of syntax-node kinds.

/// The kind of a [`crate::SyntaxNode`].
///
/// One variant per production of the concrete grammar
/// (`specification/grammar.ebnf` sections (B)–(I)), plus [`Error`](Self::Error)
/// for material retained during parse-error recovery. Token-level
/// classification lives in `tuo_lexer::TokenKind`; this enum only names
/// *structure*.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SyntaxKind {
    /// A whole compilation unit: `[module_decl] { item }` (B).
    SourceFile,
    /// `module path ;` (B).
    ModuleDecl,
    /// `import import_path [as IDENT] ;` (C).
    ImportItem,
    /// `{ leaf, leaf }` inside an import path (C).
    ImportGroup,
    /// `IDENT [as IDENT]` inside an import group (C).
    ImportLeaf,
    /// `segment :: segment :: …` (C).
    Path,

    /// `fn name [generics] (params) [-> type] [where] block` (E).
    FunctionItem,
    /// `fn name [generics] (params) [-> type] [where]` with no body (E).
    FunctionSignature,
    /// `[T, U: Bound]` (E).
    GenericParams,
    /// One `T` or `T: Bound` (E).
    GenericParam,
    /// `Bound + Bound` (E).
    BoundList,
    /// `where pred, pred` (E).
    WhereClause,
    /// `type : bound_list` (E).
    WherePred,
    /// `(a: T, mut b: U)`'s parenthesised list (E).
    ParamList,
    /// One `mode IDENT : type` parameter (E).
    Param,
    /// A `mode self` receiver (E).
    Receiver,
    /// `-> type` (E).
    ReturnType,

    /// `struct Name [generics] [where] { fields }` (E).
    StructItem,
    /// `{ field, field }` of a struct or enum-variant payload (E).
    FieldBlock,
    /// One `[pub] name : type` field (E).
    FieldDecl,
    /// `enum Name [generics] [where] { variants }` (E).
    EnumItem,
    /// One enum variant: `Name [field_block]` (E).
    VariantDecl,
    /// `interface Name [generics] [where] { members }` (E).
    InterfaceItem,
    /// `impl [generics] Path [args] for type [where] { members }` (E).
    ImplItem,
    /// `const NAME : type = expr ;` (E).
    ConstItem,

    /// `spec name { statements }` (I).
    SpecItem,
    /// `given binding, binding ;` (I).
    GivenClause,
    /// One `IDENT : type [= expr]` spec input (I).
    GivenBinding,
    /// `when (let-binding | expression) ;` (I).
    WhenClause,
    /// `then expression ;` (I).
    ThenClause,
    /// `assert expression ;` (I).
    AssertClause,

    /// A named type, possibly generic: `std::io::File`, `Option[T]` (D).
    PathType,
    /// The type path head `Self | IDENT { :: IDENT }` (D).
    TypePath,
    /// `( )` — the unit type (D).
    UnitType,
    /// `Box[T]`, `Shared[T]`, `Weak[T]` (D).
    WrapperType,
    /// `[ type ; INT ]` — the fixed-capacity array type `[T; N]`
    /// (D, `[EXPERIMENTAL]` ADR-0004).
    FixedArrayType,
    /// `fn ( [param_mode type {, param_mode type}] ) -> type` — the type of a
    /// non-capturing function value (D, `[EXPERIMENTAL]` ADR-0008 Tier 1).
    FnType,
    /// `param_mode type` — one parameter of a `FnType` (mode + type, no name)
    /// (D, `[EXPERIMENTAL]` ADR-0008 Tier 1).
    FnTypeParam,
    /// `[type, type]` applied to a type or path (D).
    TypeArguments,

    /// `{ statements [tail] }` (F).
    Block,
    /// `let pattern [: type] [= expr] ;` (F).
    LetStatement,
    /// `var pattern [: type] [= expr] ;` (F).
    VarStatement,
    /// An expression used as a statement (F).
    ExpressionStatement,
    /// A lone `;` (F).
    EmptyStatement,

    /// `lhs = rhs` (G).
    AssignExpr,
    /// One `lhs op rhs` for `|| && + - * / %` and comparisons (G).
    BinaryExpr,
    /// `a .. b` (G).
    RangeExpr,
    /// Prefix `-`, `!`, or `move` (G).
    UnaryExpr,
    /// `callee(args)` (G).
    CallExpr,
    /// `receiver.name(args)` (G).
    MethodCallExpr,
    /// `value.field` or `value.0` (G).
    FieldExpr,
    /// `value[index]` (G).
    IndexExpr,
    /// `value?` (G).
    TryExpr,
    /// `value as type` (G).
    CastExpr,
    /// `(args)` of a call (G).
    CallArgs,
    /// A value path: `self`, `Point`, `std::mem::swap`, with optional
    /// turbofish (G).
    PathExpr,
    /// `:: [T]` explicit type arguments at a value path (G).
    Turbofish,
    /// `[ expr, … ]` or `[ expr ; INT ]` — one kind for both array-literal
    /// forms; the `Semi` token distinguishes them losslessly
    /// (G, `[EXPERIMENTAL]` ADR-0004).
    ArrayLiteral,
    /// `Point { x: 1, y }` (G).
    StructLiteral,
    /// One `name : expr` or shorthand `name` initializer (G).
    FieldInit,
    /// An `INT/FLOAT/BOOL/CHAR/STRING` literal token, or `()` (G).
    Literal,
    /// `( expr )` grouping (G).
    GroupExpr,
    /// `if cond block [else …]` (G).
    IfExpr,
    /// `match scrutinee { arms }` (G).
    MatchExpr,
    /// One `pattern [if guard] => body` arm (G).
    MatchArm,
    /// `while cond block` (G).
    WhileExpr,
    /// `['label:] loop block` (G).
    LoopExpr,
    /// `for pattern in iter block` (G).
    ForExpr,
    /// `unsafe block` (G).
    UnsafeExpr,
    /// `return [expr]` (G).
    ReturnExpr,
    /// `break ['label] [expr]` (G).
    BreakExpr,
    /// `continue ['label]` (G).
    ContinueExpr,

    /// `p | p | p` (H).
    OrPattern,
    /// A literal pattern: `0`, `true`, `'a'`, `"s"` (H).
    LiteralPattern,
    /// `_` (H).
    WildcardPattern,
    /// A single identifier binding (H).
    BindingPattern,
    /// A multi-segment path or a path with a field block:
    /// `Some { value }`, `Color::Red` (H).
    PathPattern,
    /// `{ field, field [, ..] }` of a path pattern (H).
    FieldPatternBlock,
    /// One `name : pattern` or shorthand `name` (H).
    FieldPattern,
    /// The `..` "rest" marker inside a field pattern block (H).
    RestPattern,
    /// `( pattern )` grouping (H).
    GroupPattern,

    /// Tokens skipped during error recovery, retained for tooling. The
    /// parser records a diagnostic whenever it emits one of these.
    Error,
}

impl SyntaxKind {
    /// Is this node an item-level construct (B/E/I)?
    #[must_use]
    pub const fn is_item(self) -> bool {
        matches!(
            self,
            Self::ImportItem
                | Self::FunctionItem
                | Self::StructItem
                | Self::EnumItem
                | Self::InterfaceItem
                | Self::ImplItem
                | Self::ConstItem
                | Self::SpecItem
        )
    }

    /// Is this node an expression (G)?
    #[must_use]
    pub const fn is_expression(self) -> bool {
        matches!(
            self,
            Self::AssignExpr
                | Self::ArrayLiteral
                | Self::BinaryExpr
                | Self::RangeExpr
                | Self::UnaryExpr
                | Self::CallExpr
                | Self::MethodCallExpr
                | Self::FieldExpr
                | Self::IndexExpr
                | Self::TryExpr
                | Self::CastExpr
                | Self::PathExpr
                | Self::StructLiteral
                | Self::Literal
                | Self::GroupExpr
                | Self::IfExpr
                | Self::MatchExpr
                | Self::WhileExpr
                | Self::LoopExpr
                | Self::ForExpr
                | Self::UnsafeExpr
                | Self::ReturnExpr
                | Self::BreakExpr
                | Self::ContinueExpr
                | Self::Block
        )
    }
}
