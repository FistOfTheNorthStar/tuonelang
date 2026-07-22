//! The MIR data structures: a typed control-flow IR with fully defined
//! per-instruction semantics.
//!
//! A [`Function`] is a set of typed [locals](LocalDecl) and a graph of
//! [basic blocks](BasicBlock). Each block is a straight-line sequence of
//! [statements](Statement) closed by exactly one [`Terminator`]. Execution
//! starts at block 0 with the parameter locals holding the arguments and
//! every other local **uninitialized**; reading an uninitialized or moved
//! local is statically impossible in lowered MIR (the ownership checker
//! ran first), and no instruction below defines a meaning for it.
//!
//! Everything observable is defined here — evaluation order is the textual
//! order of statements, traps are deterministic aborts (Constitution §24:
//! no unwinding, destructors do not run) — so the interpreter and every
//! native backend implement exactly this document, and nothing else.

use tuo_resolve::SymbolId;
use tuo_source::Span;
use tuo_types::{FloatKind, IntKind, Ty};

/// A local slot of a function, by index into [`Function::locals`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct LocalId(pub u32);

/// A basic block of a function, by index into [`Function::blocks`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BlockId(pub u32);

/// A lowered program snapshot: the MIR of every lowerable function body.
#[derive(Clone, Debug, Default)]
pub struct Program {
    /// The lowered functions, in program order.
    pub functions: Vec<Function>,
    /// Function bodies the v0 lowering does not cover, with the reason.
    /// A skipped body is *absent*, never silently mis-lowered.
    pub skipped: Vec<Skipped>,
}

/// A function body the v0 lowering declined, and why.
#[derive(Clone, Debug)]
pub struct Skipped {
    /// The function's name (`{impl}` for impl functions, which have no
    /// resolved symbol until the trait system lands).
    pub name: String,
    /// The reason lowering declined (one of the documented v0 limits).
    pub reason: String,
    /// The declaration's span.
    pub span: Span,
}

/// How a parameter receives its argument (Constitution §9).
///
/// Borrow modes are the only borrow formers of the language: the borrow
/// exists exactly for the duration of the call. A `Borrow`/`BorrowMut`
/// local *aliases* the caller's place — reads read through it, and (for
/// `BorrowMut`) writes write through it; the callee never drops it. A
/// `Value` parameter is owned by the callee like any other local.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PassMode {
    /// `take` — the argument is moved (or copied, if `Copy`) into the
    /// parameter local; the callee owns it.
    Value,
    /// `in` — the parameter aliases the argument place, read-only.
    Borrow,
    /// `mut` — the parameter aliases the argument place exclusively;
    /// writes through it are visible to the caller after the call.
    BorrowMut,
}

/// The MIR of one function body.
#[derive(Clone, Debug)]
pub struct Function {
    /// The function's symbol.
    pub symbol: SymbolId,
    /// The function's name (for rendering; the symbol is the identity).
    pub name: String,
    /// The passing mode of each parameter. Locals `0 .. params.len()`
    /// are the parameters, in declaration order.
    pub params: Vec<PassMode>,
    /// Every local slot: parameters first, then user locals and compiler
    /// temporaries in creation order.
    pub locals: Vec<LocalDecl>,
    /// The basic blocks; block 0 is the entry.
    pub blocks: Vec<BasicBlock>,
    /// The declared return type.
    pub ret: Ty,
    /// The declaration's span.
    pub span: Span,
}

/// One local slot: a typed storage location live for the whole call.
#[derive(Clone, Debug)]
pub struct LocalDecl {
    /// The slot's type.
    pub ty: Ty,
    /// The source name, for user bindings and parameters (`None` for
    /// compiler temporaries).
    pub name: Option<String>,
    /// The declaring span (the whole expression for temporaries).
    pub span: Span,
}

/// A straight-line statement sequence closed by one terminator.
#[derive(Clone, Debug)]
pub struct BasicBlock {
    /// The statements, executed in order.
    pub statements: Vec<Statement>,
    /// The closing control transfer.
    pub terminator: Terminator,
}

/// A memory location: a local plus a (possibly empty) projection path.
///
/// Places are the only way MIR names memory. There is no address-of and
/// no pointer arithmetic; aliasing exists solely through borrow-mode
/// parameters ([`PassMode`]).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Place {
    /// The root local.
    pub local: LocalId,
    /// The projections, applied left to right.
    pub projection: Vec<Projection>,
}

impl Place {
    /// The bare place of `local`.
    #[must_use]
    pub fn local(local: LocalId) -> Self {
        Self {
            local,
            projection: Vec::new(),
        }
    }
}

/// One step of a place's projection path.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Projection {
    /// The `index`-th field (declaration order) of a struct, tuple, or
    /// range value (a range has field 0 = start, field 1 = end).
    Field(u32),
    /// The `field`-th payload field of enum variant `variant`
    /// (declaration-order indices). Defined only while the place's
    /// discriminant *is* `variant`; lowering guards every use with a
    /// discriminant test, so backends may assume it.
    VariantField {
        /// The variant's declaration-order index.
        variant: u32,
        /// The payload field's declaration-order index.
        field: u32,
    },
    /// The array element at the `Usize` value held by `index`. Defined
    /// only for in-bounds values; lowering emits an explicit
    /// [`Terminator::Assert`] bounds check before every use.
    Index(LocalId),
}

/// A value read: how an instruction obtains each input.
#[derive(Clone, Debug)]
pub enum Operand {
    /// Read the place's value, leaving it initialized. Emitted only for
    /// `Copy` types; the copy is bitwise-equivalent and independent.
    Copy(Place),
    /// Read the place's value and end its initialization: the place's
    /// contents must not be read or dropped again until reassigned. The
    /// move is a transfer, never a deep copy.
    Move(Place),
    /// A compile-time constant.
    Const(Const),
}

/// A compile-time constant value.
#[derive(Clone, Debug)]
pub enum Const {
    /// The unit value `()`.
    Unit,
    /// A boolean.
    Bool(bool),
    /// An integer of the given kind; the value is always in the kind's
    /// range (lowering rejects out-of-range literals).
    Int(i128, IntKind),
    /// A float of the given kind (an `F32` is stored at `f64` precision
    /// and rounds to nearest even when materialized).
    Float(f64, FloatKind),
    /// A Unicode scalar value.
    Char(char),
    /// A `Str` literal: an immutable view of static UTF-8 text.
    Str(String),
}

/// One statement. Statements never branch; a statement that traps
/// (arithmetic, per [`BinOp`]) aborts the program deterministically.
#[derive(Clone, Debug)]
pub enum Statement {
    /// Evaluate `rvalue` and store the result into `place`, which must be
    /// uninitialized (or `Copy`): drops of a previously held value are
    /// always explicit, earlier `Drop` statements.
    Assign {
        /// The destination place.
        place: Place,
        /// The computed value.
        rvalue: Rvalue,
    },
    /// Call `callee` with `args` and store its return value into `dest`.
    /// Arguments are evaluated before the call (each `Arg` names its
    /// passing semantics); the call returns normally or aborts — there is
    /// no unwinding (Constitution §24). Calls are always direct: v0 has
    /// no function-typed values in MIR.
    Call {
        /// The destination of the return value.
        dest: Place,
        /// The called function's symbol.
        callee: SymbolId,
        /// The arguments, in declaration order.
        args: Vec<Arg>,
    },
    /// Destroy the value held by `place`, which must be initialized.
    /// Drop glue is compiler-generated only (no user destructors in v0,
    /// ADR-0003): a `Box` frees its allocation after dropping the
    /// contents; a `Shared` decrements its count and destroys the
    /// contents (and both counts' storage) when it reaches zero; a `Weak`
    /// releases its handle; an `Array` drops elements front to back, then
    /// the storage; a `String` frees its buffer; a struct or enum drops
    /// its (active variant's) fields in declaration order; every `Copy`
    /// value's drop is a no-op. After the statement the place is
    /// uninitialized.
    Drop {
        /// The dropped place.
        place: Place,
    },
}

/// One call argument, carrying its passing semantics (must agree with the
/// callee's [`PassMode`] for that parameter).
#[derive(Clone, Debug)]
pub enum Arg {
    /// Pass by value (a `take` parameter, or any `Copy` argument).
    Value(Operand),
    /// Lend `place` read-only for the duration of the call.
    Borrow(Place),
    /// Lend `place` exclusively for the duration of the call; the callee
    /// may write through it.
    BorrowMut(Place),
}

/// A computed value. Every rvalue is evaluated left to right and produces
/// exactly one value of a statically known type.
#[derive(Clone, Debug)]
pub enum Rvalue {
    /// The operand's value, unchanged.
    Use(Operand),
    /// A unary operation; see [`UnOp`] for semantics.
    Unary {
        /// The operator.
        op: UnOp,
        /// The operand.
        operand: Operand,
    },
    /// A binary operation; see [`BinOp`] for semantics. Both operands
    /// have the same type.
    Binary {
        /// The operator.
        op: BinOp,
        /// The left operand.
        lhs: Operand,
        /// The right operand.
        rhs: Operand,
    },
    /// A numeric conversion; see [`CastKind`] for the exact semantics of
    /// each direction. Never traps.
    Cast {
        /// Which conversion.
        kind: CastKind,
        /// The converted value.
        operand: Operand,
        /// The target type.
        to: Ty,
    },
    /// Construct an aggregate from `fields`, in declaration order.
    Aggregate {
        /// What is being constructed.
        kind: AggregateKind,
        /// The field values, in declaration order of the constructed
        /// struct or variant (start then end for a range).
        fields: Vec<Operand>,
    },
    /// The discriminant of the enum (or `Option`/`Result`) value at
    /// `place`, as a `Usize`: the declaration-order index of the active
    /// variant (`Some` = 0, `None` = 1; `Ok` = 0, `Err` = 1).
    Discriminant(Place),
    /// The length of the array at `place`, as a `Usize`.
    Len(Place),
}

/// A unary operator.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnOp {
    /// Arithmetic negation of a signed integer or float. An integer
    /// negation **traps** on the minimum value (two's complement, §24);
    /// float negation flips the sign bit (IEEE 754, never traps).
    Neg,
    /// Boolean negation.
    Not,
}

/// A binary operator. Integer arithmetic is two's complement with
/// **trapping overflow** (Constitution §24): a trap is a deterministic
/// abort, exactly as if a failed [`Terminator::Assert`] were reached.
/// Float arithmetic is IEEE 754 (round to nearest even) and never traps.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BinOp {
    /// Addition. Integer: traps on overflow. Float: IEEE 754.
    Add,
    /// Subtraction. Integer: traps on overflow. Float: IEEE 754.
    Sub,
    /// Multiplication. Integer: traps on overflow. Float: IEEE 754.
    Mul,
    /// Division. Integer: truncates toward zero; traps on a zero divisor
    /// and on overflow (`MIN / -1`). Float: IEEE 754 (`x / 0.0` is an
    /// infinity or NaN, no trap).
    Div,
    /// Remainder. Integer: sign of the dividend (truncated division);
    /// traps on a zero divisor and on `MIN % -1`. Float: IEEE 754
    /// remainder with the sign of the dividend (as `fmod`).
    Rem,
    /// Equality. Integers, floats (IEEE: `NaN == x` is `false`), `Bool`,
    /// `Char` (scalar values), and `Str` (byte-wise content equality).
    Eq,
    /// The negation of [`BinOp::Eq`] (so `NaN != x` is `true`).
    Ne,
    /// Less-than. Integers by signed/unsigned value, `Char` by scalar
    /// value, floats by IEEE ordered comparison (`false` if either is
    /// NaN).
    Lt,
    /// Less-or-equal; same domains and NaN behavior as [`BinOp::Lt`].
    Le,
    /// Greater-than; same domains and NaN behavior as [`BinOp::Lt`].
    Gt,
    /// Greater-or-equal; same domains and NaN behavior as [`BinOp::Lt`].
    Ge,
}

/// The direction of a [`Rvalue::Cast`]. All four are total functions —
/// casts never trap.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CastKind {
    /// Integer to integer: wrap to the target width (truncate the two's
    /// complement representation; sign- or zero-extend from the source's
    /// signedness when widening).
    IntToInt,
    /// Integer to float: round to nearest even.
    IntToFloat,
    /// Float to integer: truncate toward zero, then **saturate** to the
    /// target's range; NaN converts to 0.
    FloatToInt,
    /// Float to float: IEEE 754 conversion (round to nearest even when
    /// narrowing).
    FloatToFloat,
}

/// What a [`Rvalue::Aggregate`] constructs.
#[derive(Clone, Debug)]
pub enum AggregateKind {
    /// A struct, enum-variant, `Option`, or `Result` value of type `ty`
    /// with active variant `variant` (declaration-order index; 0 for a
    /// struct). The operands are the variant's payload fields in
    /// declaration order.
    Adt {
        /// The constructed type.
        ty: Ty,
        /// The active variant's declaration-order index.
        variant: u32,
    },
    /// A range value from two operands, start then end.
    Range,
}

/// The closing control transfer of a basic block.
#[derive(Clone, Debug)]
pub enum Terminator {
    /// Return the operand to the caller.
    Return(Operand),
    /// Jump unconditionally.
    Goto(BlockId),
    /// Two-way branch on a `Bool` operand.
    Branch {
        /// The branched-on value.
        cond: Operand,
        /// Taken when `cond` is `true`.
        then_block: BlockId,
        /// Taken when `cond` is `false`.
        else_block: BlockId,
    },
    /// Multi-way branch on an integer-classed operand (an integer,
    /// `Char` as its scalar value, or a discriminant). Exactly one edge
    /// is taken: the first arm whose value equals the operand, else
    /// `otherwise`.
    Switch {
        /// The switched-on value.
        discr: Operand,
        /// `(value, target)` arms; values are pairwise distinct.
        arms: Vec<(i128, BlockId)>,
        /// Taken when no arm matches.
        otherwise: BlockId,
    },
    /// An explicit runtime check: if `cond` is `true`, continue at
    /// `target`; otherwise **trap** — abort deterministically with
    /// `trap`, without unwinding (§24).
    Assert {
        /// The checked condition.
        cond: Operand,
        /// The abort cause when the check fails.
        trap: Trap,
        /// The continuation when the check passes.
        target: BlockId,
    },
    /// Abort deterministically with `trap`. Emitted where control can
    /// only arrive through a compiler bug (e.g. after a `match` whose
    /// arms the front end proved exhaustive), never for reachable
    /// user-visible behavior.
    Trap(Trap),
}

/// The cause of a deterministic abort.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Trap {
    /// An array index was out of bounds.
    IndexOutOfBounds,
    /// Control reached a point the front end proved unreachable.
    Unreachable,
}

impl Trap {
    /// The stable rendering of the cause.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::IndexOutOfBounds => "index_out_of_bounds",
            Self::Unreachable => "unreachable",
        }
    }
}
