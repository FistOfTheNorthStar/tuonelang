//! The HIR data structures: an owned, desugared semantic tree.
//!
//! Every node is independent of the parser's types — no CST nodes, no
//! borrowed views — and carries the exact [`Span`] of the source it was
//! lowered from. Names are represented as resolved [`Res`] values keyed by
//! the stable [`SymbolId`]s of name resolution, never as raw strings
//! (except where meaning depends on types — field and method names — which
//! resolution deliberately leaves to the type checker).

use tuo_resolve::SymbolId;
use tuo_source::Span;

/// What a name in the program refers to.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Res {
    /// A declared entity, by its stable semantic ID.
    Symbol(SymbolId),
    /// A builtin type name the language provides without a declaration
    /// (`I64`, `Bool`, `Array`, …), kept as written.
    Builtin(String),
    /// The `Self` type inside an `impl` or `interface`.
    SelfType,
    /// A name resolution could not resolve (or a malformed tree); poison,
    /// already diagnosed upstream.
    Err,
}

/// A lowered program snapshot: every item of every file, in program order.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Hir {
    /// The items, flattened across files in program order. Imports and
    /// `module` declarations are resolved away — they do not appear.
    pub items: Vec<Item>,
}

/// One lowered top-level item.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Item {
    /// A function definition or signature.
    Fn(Function),
    /// A `struct` declaration.
    Struct(StructDef),
    /// An `enum` declaration.
    Enum(EnumDef),
    /// An `interface` declaration.
    Interface(InterfaceDef),
    /// An `impl … for …` block.
    Impl(ImplDef),
    /// A `const` item.
    Const(ConstDef),
    /// A `spec` block.
    Spec(SpecDef),
}

/// A function definition (or body-less interface signature).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Function {
    /// The function's symbol.
    pub symbol: Res,
    /// The whole declaration's span.
    pub span: Span,
    /// Declared type parameters, with `where`-clause bounds on a plain
    /// type parameter merged into that parameter's bound list.
    pub type_params: Vec<TypeParam>,
    /// The parameters (receiver first when present), in source order.
    pub params: Vec<Param>,
    /// The return type; an omitted `-> ()` is made explicit here.
    pub ret: Ty,
    /// The body (`None` for interface signatures).
    pub body: Option<Block>,
}

/// One declared type parameter.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TypeParam {
    /// The parameter's symbol.
    pub symbol: Res,
    /// The declaration's span.
    pub span: Span,
    /// Interface bounds, inline and `where`-clause alike, in source order.
    pub bounds: Vec<Res>,
}

/// The declared passing mode of a parameter (Constitution §9); an
/// unwritten mode means [`ParamMode::In`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ParamMode {
    /// `in` — read access (the default).
    In,
    /// `mut` — exclusive mutable access.
    Mut,
    /// `take` — ownership transfer.
    Take,
}

impl ParamMode {
    /// The mode keyword.
    #[must_use]
    pub const fn keyword(self) -> &'static str {
        match self {
            Self::In => "in",
            Self::Mut => "mut",
            Self::Take => "take",
        }
    }
}

/// One function parameter (or the `self` receiver).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Param {
    /// The parameter's symbol.
    pub symbol: Res,
    /// The declaration's span.
    pub span: Span,
    /// The passing mode; explicit even when the source omitted it.
    pub mode: ParamMode,
    /// Is this the `self` receiver?
    pub is_receiver: bool,
    /// The declared type (`None` for the receiver, whose type is `Self`).
    pub ty: Option<Ty>,
}

/// One `name: Type` field of a struct or enum variant. Fields have no
/// symbols of their own — their meaning is type-dependent — so the name is
/// kept as written.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Field {
    /// The field name.
    pub name: String,
    /// The name's span.
    pub span: Span,
    /// The declared type.
    pub ty: Option<Ty>,
}

/// A `struct` declaration.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StructDef {
    /// The struct's symbol.
    pub symbol: Res,
    /// The whole declaration's span.
    pub span: Span,
    /// Declared type parameters.
    pub type_params: Vec<TypeParam>,
    /// The fields, in source order.
    pub fields: Vec<Field>,
}

/// An `enum` declaration.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EnumDef {
    /// The enum's symbol.
    pub symbol: Res,
    /// The whole declaration's span.
    pub span: Span,
    /// Declared type parameters.
    pub type_params: Vec<TypeParam>,
    /// The variants, in source order.
    pub variants: Vec<Variant>,
}

/// One enum variant.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Variant {
    /// The variant's symbol.
    pub symbol: Res,
    /// The declaration's span.
    pub span: Span,
    /// The payload fields (empty for a bare variant).
    pub fields: Vec<Field>,
}

/// An `interface` declaration.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct InterfaceDef {
    /// The interface's symbol.
    pub symbol: Res,
    /// The whole declaration's span.
    pub span: Span,
    /// Declared type parameters.
    pub type_params: Vec<TypeParam>,
    /// The members — signatures and default-bodied functions — in source
    /// order.
    pub members: Vec<Function>,
}

/// An `impl Interface for Type { … }` block. Impl blocks declare no symbol
/// of their own.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ImplDef {
    /// The whole block's span.
    pub span: Span,
    /// Declared type parameters.
    pub type_params: Vec<TypeParam>,
    /// The implemented interface.
    pub interface: Res,
    /// The interface's type arguments.
    pub interface_args: Vec<Ty>,
    /// The implementing type (after `for`).
    pub target: Option<Ty>,
    /// The implemented functions, in source order.
    pub functions: Vec<Function>,
}

/// A `const NAME: Type = value;` item (top-level or block-local).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ConstDef {
    /// The constant's symbol.
    pub symbol: Res,
    /// The whole declaration's span.
    pub span: Span,
    /// The declared type.
    pub ty: Option<Ty>,
    /// The value expression.
    pub value: Option<Expr>,
}

/// A `spec` block.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SpecDef {
    /// The spec's own stable symbol (a `Spec`-kind symbol), if resolution
    /// assigned one. A spec's *name* is a target reference, never a binding,
    /// so this — not the name — is the spec's identity; it distinguishes two
    /// specs that share a name or target (ADR-0002).
    pub spec_symbol: Option<SymbolId>,
    /// The whole block's span.
    pub span: Span,
    /// The spec's name as written (identifier or string form).
    pub name: String,
    /// The targeted function symbol, for identifier-named specs that
    /// resolved (the *same* symbol calls to that function resolve to).
    pub target: Option<SymbolId>,
    /// The statements and clauses, in source order.
    pub statements: Vec<SpecStmt>,
}

/// One statement inside a `spec` body.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SpecStmt {
    /// A `given name: Type = value, …;` clause.
    Given(Vec<GivenBinding>),
    /// A `when …;` clause: a binding or an expression statement.
    When(Stmt),
    /// A `then expr;` clause.
    Then(Expr),
    /// An `assert expr;` clause.
    Assert(Expr),
    /// A plain statement.
    Stmt(Stmt),
}

/// One binding of a `given` clause.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GivenBinding {
    /// The bound local's symbol.
    pub symbol: Res,
    /// The binding's span.
    pub span: Span,
    /// The declared type.
    pub ty: Option<Ty>,
    /// The initializer, if present.
    pub init: Option<Expr>,
}

/// A memory wrapper (Constitution §25).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Wrapper {
    /// `Box[T]` — unique owned heap allocation.
    Box,
    /// `Shared[T]` — shared ownership.
    Shared,
    /// `Weak[T]` — non-owning handle to a `Shared`.
    Weak,
}

impl Wrapper {
    /// The surface-syntax name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Box => "Box",
            Self::Shared => "Shared",
            Self::Weak => "Weak",
        }
    }
}

/// A reference to a type, with its names resolved.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Ty {
    /// What the reference is.
    pub kind: TyKind,
    /// Where it was written (the enclosing construct's span for the
    /// synthesized `-> ()` of a function without a return type).
    pub span: Span,
}

/// The kinds of type reference.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TyKind {
    /// The unit type `()`.
    Unit,
    /// The `Self` type.
    SelfType,
    /// A named type with optional type arguments. Module qualification and
    /// import aliases are resolved away — only the target remains.
    Path {
        /// The resolved name.
        res: Res,
        /// The type arguments, in source order.
        args: Vec<Ty>,
    },
    /// A memory wrapper type: `Box[…]`, `Shared[…]`, or `Weak[…]`.
    Wrapper {
        /// Which wrapper.
        wrapper: Wrapper,
        /// The wrapped type arguments, in source order.
        args: Vec<Ty>,
    },
    /// A fixed-capacity array type `[T; N]` (ADR-0004 Stage 2). The length
    /// is the normalized digit string (separators removed), exactly like
    /// [`Lit::Int`] — value parsing and overflow are a type-checker concern
    /// per the grammar's superset rule.
    FixedArray {
        /// The element type.
        element: Box<Ty>,
        /// The length digits as written, separators removed.
        len: String,
    },
    /// A malformed type reference; poison.
    Err,
}

/// A `{ … }` block: statements followed by an optional tail expression. A
/// final semicolon-less block-form expression statement is promoted to the
/// tail — the two spellings lower identically.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Block {
    /// The statements, in source order. Lone `;` statements are dropped.
    pub stmts: Vec<Stmt>,
    /// The tail expression, if the block produces a value.
    pub tail: Option<Box<Expr>>,
    /// The whole block's span.
    pub span: Span,
}

/// One lowered statement.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Stmt {
    /// What the statement is.
    pub kind: StmtKind,
    /// The statement's span.
    pub span: Span,
}

/// The kinds of statement.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum StmtKind {
    /// A `let` or `var` binding.
    Binding(BindingDef),
    /// A block-local `const` item.
    Const(ConstDef),
    /// An expression statement.
    Expr(Expr),
    /// Material skipped during error recovery; poison.
    Err,
}

/// A `let`/`var` binding.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BindingDef {
    /// Is this a `var` (mutable) binding?
    pub mutable: bool,
    /// The bound pattern.
    pub pat: Pat,
    /// The declared type, if annotated.
    pub ty: Option<Ty>,
    /// The initializer, if present.
    pub init: Option<Expr>,
}

/// A lowered pattern.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Pat {
    /// What the pattern is.
    pub kind: PatKind,
    /// The pattern's span.
    pub span: Span,
}

/// The kinds of pattern.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PatKind {
    /// The wildcard `_`.
    Wildcard,
    /// A literal pattern.
    Lit(Lit),
    /// A binding pattern, by the symbol of the local it introduces.
    Binding(Res),
    /// A constructor pattern (`Some { value }`, `Shape::Circle { .. }`).
    /// Shorthand fields are expanded to explicit `name: binding` form.
    Ctor {
        /// The resolved variant or struct.
        ctor: Res,
        /// The destructured fields.
        fields: Vec<FieldPat>,
        /// Does the field block end in `..`?
        rest: bool,
    },
    /// An or-pattern: two or more alternatives.
    Or(Vec<Pat>),
    /// A malformed pattern; poison.
    Err,
}

/// One field of a constructor pattern, in explicit `name: pattern` form.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FieldPat {
    /// The field name (type-dependent; resolved by the type checker).
    pub name: String,
    /// The name's span.
    pub span: Span,
    /// The sub-pattern.
    pub pat: Pat,
}

/// A literal value, normalized: numeric literals have their `_` separators
/// removed; character and string literals keep their escape sequences as
/// written (escape semantics belong to later stages).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Lit {
    /// The unit literal `()`.
    Unit,
    /// A boolean literal.
    Bool(bool),
    /// An integer literal (digits as written, separators removed).
    Int(String),
    /// A floating-point literal (digits as written, separators removed).
    Float(String),
    /// A character literal (contents between the quotes).
    Char(String),
    /// A string literal (contents between the quotes).
    Str(String),
}

/// A prefix operator.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnOp {
    /// Arithmetic negation `-`.
    Neg,
    /// Logical negation `!`.
    Not,
    /// An explicit `move`.
    Move,
}

impl UnOp {
    /// The operator as written.
    #[must_use]
    pub const fn symbol(self) -> &'static str {
        match self {
            Self::Neg => "-",
            Self::Not => "!",
            Self::Move => "move",
        }
    }
}

/// An infix operator.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BinOp {
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/`
    Div,
    /// `%`
    Rem,
    /// `==`
    Eq,
    /// `!=`
    Ne,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// `&&`
    And,
    /// `||`
    Or,
}

impl BinOp {
    /// The operator as written.
    #[must_use]
    pub const fn symbol(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
            Self::Rem => "%",
            Self::Eq => "==",
            Self::Ne => "!=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::And => "&&",
            Self::Or => "||",
        }
    }
}

/// A lowered expression.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Expr {
    /// What the expression is.
    pub kind: ExprKind,
    /// The expression's span. Parenthesized groups are unwrapped during
    /// lowering, so `(x)` carries the span of `x`.
    pub span: Span,
}

/// The kinds of expression.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ExprKind {
    /// A literal.
    Lit(Lit),
    /// A resolved name use. Module qualification and import aliases are
    /// resolved away — only the target remains.
    Path {
        /// The resolved name.
        res: Res,
        /// Turbofish type arguments (`::[T]`), if written.
        args: Vec<Ty>,
    },
    /// A struct literal, with shorthand fields expanded.
    StructLit {
        /// The resolved struct or variant.
        ctor: Res,
        /// Explicit type arguments, if written.
        args: Vec<Ty>,
        /// The field initializers, in source order.
        fields: Vec<FieldInit>,
    },
    /// A prefix operator application.
    Unary {
        /// The operator.
        op: UnOp,
        /// The operand.
        operand: Box<Expr>,
    },
    /// An infix operator application.
    Binary {
        /// The operator.
        op: BinOp,
        /// The left operand.
        lhs: Box<Expr>,
        /// The right operand.
        rhs: Box<Expr>,
    },
    /// A range `lo .. hi`.
    Range {
        /// The lower bound.
        lo: Box<Expr>,
        /// The upper bound.
        hi: Box<Expr>,
    },
    /// An assignment.
    Assign {
        /// The assigned place.
        target: Box<Expr>,
        /// The assigned value.
        value: Box<Expr>,
    },
    /// A call.
    Call {
        /// The callee.
        callee: Box<Expr>,
        /// The arguments, in source order.
        args: Vec<Expr>,
    },
    /// A method call (the name is type-dependent; resolved by the type
    /// checker).
    MethodCall {
        /// The receiver.
        receiver: Box<Expr>,
        /// The method name as written.
        name: String,
        /// The arguments, in source order.
        args: Vec<Expr>,
    },
    /// A field access (the name is type-dependent; resolved by the type
    /// checker).
    Field {
        /// The receiver.
        receiver: Box<Expr>,
        /// The field name or tuple index as written.
        name: String,
    },
    /// An array literal in list form: `[a, b, c]` (0+ elements). The
    /// element count is the resulting fixed array's length (ADR-0004
    /// Stage 2).
    Array(Vec<Expr>),
    /// An array literal in repeat form: `[value; len]` (ADR-0004 Stage 2).
    /// The length is the normalized digit string (separators removed),
    /// exactly like [`Lit::Int`].
    ArrayRepeat {
        /// The repeated value.
        value: Box<Expr>,
        /// The length digits as written, separators removed.
        len: String,
    },
    /// An index: `base[index]`.
    Index {
        /// The indexed expression.
        base: Box<Expr>,
        /// The index.
        index: Box<Expr>,
    },
    /// Error propagation: `inner?`.
    Try(Box<Expr>),
    /// A cast: `inner as Type`.
    Cast {
        /// The cast value.
        value: Box<Expr>,
        /// The target type.
        ty: Ty,
    },
    /// An `if` with optional `else`; an `else if` chain is a nested `If` in
    /// the `els` position.
    If {
        /// The condition.
        cond: Box<Expr>,
        /// The `then` block.
        then: Block,
        /// The `else` branch (a `Block` or nested `If` expression).
        els: Option<Box<Expr>>,
    },
    /// A `match`.
    Match {
        /// The scrutinee.
        scrutinee: Box<Expr>,
        /// The arms, in source order.
        arms: Vec<Arm>,
    },
    /// A `while` loop.
    While {
        /// The condition.
        cond: Box<Expr>,
        /// The body.
        body: Block,
    },
    /// A (possibly labeled) `loop`.
    Loop {
        /// The label, if any (including the `'`).
        label: Option<String>,
        /// The body.
        body: Block,
    },
    /// A `for pattern in iterable { … }` loop.
    For {
        /// The binding pattern.
        pat: Pat,
        /// The iterated expression.
        iter: Box<Expr>,
        /// The body.
        body: Block,
    },
    /// An `unsafe { … }` block.
    Unsafe(Block),
    /// A `return` with optional value.
    Return(Option<Box<Expr>>),
    /// A `break` with optional label and value.
    Break {
        /// The target label, if any (including the `'`).
        label: Option<String>,
        /// The break value, if any.
        value: Option<Box<Expr>>,
    },
    /// A `continue` with optional label.
    Continue {
        /// The target label, if any (including the `'`).
        label: Option<String>,
    },
    /// A block used as an expression.
    Block(Block),
    /// A malformed expression; poison.
    Err,
}

/// One field initializer of a struct literal, in explicit `name: value`
/// form.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FieldInit {
    /// The field name (type-dependent; resolved by the type checker).
    pub name: String,
    /// The name's span.
    pub span: Span,
    /// The initializer value; for shorthand `{ x }` this is the resolved
    /// path expression `x`.
    pub value: Expr,
}

/// One `pattern (if guard)? => value` arm of a `match`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Arm {
    /// The arm's pattern.
    pub pat: Pat,
    /// The `if` guard, if present.
    pub guard: Option<Expr>,
    /// The arm's value.
    pub value: Expr,
    /// The whole arm's span.
    pub span: Span,
}
