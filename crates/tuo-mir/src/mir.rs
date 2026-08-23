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
    /// Read the place's value, leaving it initialized. Emitted for `Copy`
    /// types (the copy is bitwise-equivalent and independent) — and, since
    /// ADR-0009, as the documented **borrow-read** of a `String` operand of
    /// an `Eq`/`Ne` binary (`specification/mir.md` §5.3): the read leaves
    /// the place initialized and untouched, and observably copies nothing.
    /// Everywhere else a non-`Copy` value is read through a *place*
    /// ([`Rvalue::Len`], [`Rvalue::Discriminant`], a [`Rvalue::HeapOp`]
    /// subject, a borrow [`Arg`]) or consumed by [`Operand::Move`].
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
    /// A **function value** (ADR-0008 Tier 1): a compile-time-known code
    /// pointer naming the top-level function `SymbolId`. Its type is the
    /// referenced function's signature-as-function-type (`Ty::Fn`). `Copy`,
    /// non-heap, never traps; produced only by using a bare `fn` name in
    /// value position.
    Fn(SymbolId),
}

/// The target of a [`Statement::Call`] (ADR-0008 Tier 1).
#[derive(Clone, Debug)]
pub enum Callee {
    /// A direct call to a named function symbol.
    Direct(SymbolId),
    /// An indirect call through a function-value operand (of function type).
    Indirect(Operand),
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
    /// no unwinding (Constitution §24). The callee is a [`Callee`]:
    /// `Direct` names a function symbol, `Indirect` calls through a
    /// function-value operand (ADR-0008 Tier 1). The two forms share every
    /// other mechanism — argument passing, borrows, the destination — only
    /// the target differs.
    Call {
        /// The destination of the return value.
        dest: Place,
        /// The call target: a direct symbol or an indirect function value.
        callee: Callee,
        /// The arguments, in declaration order.
        args: Vec<Arg>,
    },
    /// Perform one host effect (ADR-0006 Stage A) and store its `I64`
    /// result into `dest`. Calls to the `std::rt` builtins lower to this
    /// statement; there is no other way to construct one, and it is the
    /// **only** MIR construct whose meaning involves the host — see
    /// [`EffectOp`] for the per-op semantics and `specification/mir.md`
    /// §4.2 for the normative text. The reference interpreter **never
    /// performs an effect**: the spec-purity gate (`R0007`) makes this
    /// statement unreachable under the spec runner, and actually executing
    /// one there is an interpreter internal error, never a silent no-op
    /// and never real I/O. Natively (ADR-0006 Stage B) both backends lower
    /// it to a direct call to the matching `tuo_rt_*` runtime symbol.
    Effect {
        /// Which host effect.
        op: EffectOp,
        /// The arguments, in the builtin's declaration order (see
        /// [`EffectOp`]). Since ADR-0009 these are call-style [`Arg`]s so
        /// an effect can *borrow* a non-`Copy` value:
        /// [`EffectOp::WriteString`]'s `String` is an [`Arg::Borrow`]
        /// place, lent read-only for the statement exactly as an `in` call
        /// argument is; every other effect argument is an [`Arg::Value`]
        /// operand. The per-op argument kinds are fixed and verified
        /// (`M0011`).
        args: Vec<Arg>,
        /// The destination of the `I64` result. For [`EffectOp::Exit`] it
        /// is never observably written — control does not continue — but
        /// carrying it keeps the statement uniform with the other effects
        /// (the surface builtin is declared `-> Int`).
        dest: Place,
    },
    /// Mutate the owned `String`/`Array[I64]` at `target` **in place**
    /// (ADR-0009 Stage A) and store the op's result into `dest`. Calls to
    /// the mutating `std::string`/`std::array` builtins lower to this
    /// statement; there is no other way to construct one. `target` is
    /// semantically a `mut` borrow for the statement's duration: it must be
    /// initialized before and remains initialized after (the verifier's
    /// dataflow treats it like an [`Arg::BorrowMut`] call argument). A
    /// `HeapMutate` is **pure computation** — growth is allocation, not I/O
    /// — so specs may reach it; [`HeapMutOp::PushByte`] traps
    /// `InvalidByte` on an out-of-range byte (see [`HeapMutOp`] and
    /// `specification/mir.md` §4.3). Optimization passes never eliminate
    /// one: the mutation of `target` is observable and `PushByte` can trap.
    /// Native lowering is ADR-0009 Stage B; until then both backends refuse
    /// the statement.
    HeapMutate {
        /// Which in-place mutation.
        op: HeapMutOp,
        /// The mutated `String`/`Array[I64]` place (a `mut` borrow).
        target: Place,
        /// The operands, in the builtin's declaration order after the
        /// `mut` subject (see [`HeapMutOp`]).
        args: Vec<Operand>,
        /// The destination of the op's result: `()` for the push/append
        /// ops, `Option[I64]` for [`HeapMutOp::Pop`].
        dest: Place,
    },
    /// Destroy the value held by `place`, which must be initialized.
    /// Drop glue is compiler-generated only (no user destructors in v0,
    /// ADR-0003): a `Box` frees its allocation after dropping the
    /// contents; a `Shared` decrements its count and destroys the
    /// contents (and both counts' storage) when it reaches zero; a `Weak`
    /// releases its handle; an `Array` drops elements front to back, then
    /// the storage; a `[T; N]` drops elements front to back and frees
    /// nothing (its storage is inline); a `String` frees its buffer; a
    /// struct or enum drops
    /// its (active variant's) fields in declaration order; every `Copy`
    /// value's drop is a no-op. After the statement the place is
    /// uninitialized.
    Drop {
        /// The dropped place.
        place: Place,
    },
}

/// One host effect (ADR-0006 Stage A): the runtime seam behind the
/// `std::rt` builtins. Every effect stores an `I64` result; none traps.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EffectOp {
    /// `write(fd: I64, text: Str) -> I64` — write the `Str`'s bytes to
    /// file descriptor `fd`; the result is the number of bytes written, or
    /// a negative value on host error. Never traps.
    Write,
    /// `read_byte(fd: I64) -> I64` — read one byte from `fd`; the result
    /// is `0..=255`, `-1` on end of input, or another negative value on
    /// host error. Never traps.
    ReadByte,
    /// `exit(code: I64) -> I64` — terminate the process with `code & 0xff`
    /// as the exit status. **Never returns**: the destination is never
    /// observably written and the statements after it are never executed
    /// (natively, `tuo_rt_exit` does not return). It is a statement, not a
    /// terminator, so lowering stays uniform with the other effects — see
    /// `specification/mir.md` §4.2 for the full rationale.
    Exit,
    /// `write_string(fd: I64, s: borrow String) -> I64` — write the owned
    /// `String`'s bytes to file descriptor `fd`; the result is the number
    /// of bytes written, or a negative value on host error. The `String`
    /// argument is an [`Arg::Borrow`] place — lent read-only for the
    /// statement, consumed by nothing. Never traps. (ADR-0009 Stage A;
    /// native lowering is Stage B.)
    WriteString,
    /// `par_map(f: fn(take I64) -> I64, workers: I64, tasks: borrow
    /// Array[I64]) -> Array[I64]` — apply the function value to every task
    /// on `workers` OS threads (round-robin: task `i` on thread
    /// `i % workers`; `workers < 1` behaves as 1), joining every thread
    /// before the statement completes, and store the results **in task
    /// order** into the `Array[I64]` destination (the one effect whose
    /// destination is not `I64`). Argument order matches the lowering's
    /// operands-then-borrow convention: the two by-value operands (`f`,
    /// `workers`), then the borrowed tasks place. Deterministic in its
    /// result whenever `f` is pure; never traps. (ADR-0007.)
    ParMap,
    /// `now_nanos() -> I64` — the monotonic clock, in nanoseconds since an
    /// arbitrary process-local epoch; only differences are meaningful.
    /// Never traps. (ADR-0013.)
    NowNanos,
    /// `arg_count() -> I64` — the number of process arguments, including
    /// the program name (argv\[0\]). Never traps. (ADR-0013.)
    ArgCount,
    /// `arg_byte(i: I64, j: I64) -> I64` — byte `j` (`0..=255`) of process
    /// argument `i`, or `-1` when `i` is out of range or `j` is past that
    /// argument's end. Never traps. (ADR-0013.)
    ArgByte,
    /// `open(path: Str, mode: I64) -> I64` — open the file at `path`; the
    /// result is a file descriptor (`>= 0`), `-2` when the path does not
    /// exist, or another negative value on host error (an unknown `mode`
    /// included). Modes: `0` read, `1` write (create + truncate), `2`
    /// append (create). Never traps. (ADR-0013.)
    Open,
    /// `close(fd: I64) -> I64` — close `fd`; `0` on success, negative on
    /// host error. Never traps. (ADR-0013.)
    Close,
    /// `remove_file(path: Str) -> I64` — remove the file at `path`; `0` on
    /// success, `-2` when it does not exist, another negative value on
    /// other host errors. Never traps. (ADR-0013.)
    RemoveFile,
}

impl EffectOp {
    /// The stable lowercase name (`write`, `read_byte`, `exit`,
    /// `write_string`).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Write => "write",
            Self::ReadByte => "read_byte",
            Self::Exit => "exit",
            Self::WriteString => "write_string",
            Self::ParMap => "par_map",
            Self::NowNanos => "now_nanos",
            Self::ArgCount => "arg_count",
            Self::ArgByte => "arg_byte",
            Self::Open => "open",
            Self::Close => "close",
            Self::RemoveFile => "remove_file",
        }
    }

    /// How many arguments the op takes.
    #[must_use]
    pub const fn arg_count(self) -> usize {
        match self {
            Self::NowNanos | Self::ArgCount => 0,
            Self::ReadByte | Self::Exit | Self::Close | Self::RemoveFile => 1,
            Self::Write | Self::WriteString | Self::ArgByte | Self::Open => 2,
            Self::ParMap => 3,
        }
    }
}

/// One in-place heap mutation over an owned `String` or growable
/// `Array[I64]` (ADR-0009 Stage A): the MIR form of the mutating
/// `std::string`/`std::array` builtins. See [`Statement::HeapMutate`] and
/// `specification/mir.md` §4.3.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HeapMutOp {
    /// `push_byte(target: mut String, b: I64)` — append the byte `b`.
    /// **Traps `InvalidByte`** when `b < 0` or `b > 255`; the byte range is
    /// enforced, never silently masked.
    PushByte,
    /// `append(target: mut String, t: Str)` — append `t`'s bytes. Never
    /// traps.
    Append,
    /// `push(target: mut Array[I64], v: I64)` — append the element `v`.
    /// Never traps.
    Push,
    /// `pop(target: mut Array[I64]) -> Option[I64]` — remove and return
    /// the last element (`Some`), or `None` when empty. Never traps. It
    /// both mutates and produces a value, which is exactly why it is a
    /// statement-level mutator and not an rvalue: an rvalue must not
    /// mutate a place.
    Pop,
    /// `insert(target: mut Map[K, V], k: K, v: V) -> Option[V]` — insert or
    /// overwrite `k → v`, producing the **previous** value for `k` (`Some`)
    /// or `None` when `k` was absent (ADR-0011). A fresh key appends to the
    /// map's insertion order; an overwrite keeps the key's position. Never
    /// traps. Like [`HeapMutOp::Pop`], it both mutates and produces — hence
    /// a statement-level mutator.
    MapInsert,
    /// `remove(target: mut Map[K, V], k: K) -> Option[V]` — remove `k`,
    /// producing its value (`Some`) or `None` when absent (ADR-0011). The
    /// remaining entries keep their relative insertion order. Never traps.
    MapRemove,
}

impl HeapMutOp {
    /// The stable lowercase name (`push_byte`, `append`, `push`, `pop`,
    /// `map_insert`, `map_remove`).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::PushByte => "push_byte",
            Self::Append => "append",
            Self::Push => "push",
            Self::Pop => "pop",
            Self::MapInsert => "map_insert",
            Self::MapRemove => "map_remove",
        }
    }

    /// How many operands the op takes (after the `mut` target place).
    #[must_use]
    pub const fn arg_count(self) -> usize {
        match self {
            Self::PushByte | Self::Append | Self::Push | Self::MapRemove => 1,
            Self::Pop => 0,
            Self::MapInsert => 2,
        }
    }
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
    /// The length of the **growable** `Array[T]` at `place`, as a `Usize`.
    /// A fixed `[T; N]`'s length is a compile-time constant: lowering
    /// emits `Const N : Usize` instead, and the verifier rejects `Len` of
    /// a fixed-array place (ADR-0004 Stage 2).
    Len(Place),
    /// A pure byte-level string operation (ADR-0006 Stage A); calls to the
    /// `std::str` builtins lower to it. Operand counts and types are fixed
    /// per op and verified (`M0010`); see [`StrOp`] for semantics.
    /// [`StrOp::ByteAt`] and [`StrOp::Slice`] carry their trap semantics
    /// **in the operation** — a *statement-level* deterministic
    /// `IndexOutOfBounds` abort on an out-of-range argument, the same
    /// source of abort as trapping integer arithmetic ([`BinOp`]), not an
    /// explicit [`Terminator::Assert`]. Array indexing uses an `Assert`
    /// because its bound is already a MIR value lowering compares against;
    /// a `Str`'s byte length is a runtime property of the operand itself,
    /// so the bound lives in the op (spelling it as asserts would cost a
    /// `StrOp::Len` temp and two compares per use for the same observable
    /// behavior). The interpreter and verifier agree on this encoding.
    StrOp {
        /// Which string operation.
        op: StrOp,
        /// The operands, in the builtin's declaration order (see
        /// [`StrOp`]).
        args: Vec<Operand>,
    },
    /// A pure, value-producing heap operation over an owned `String` or
    /// growable `Array[I64]` (ADR-0009 Stage A); calls to the non-mutating
    /// `std::string`/`std::array` builtins lower to it. When the op reads
    /// an existing heap value, that value is the **place** `subject` — a
    /// non-consuming borrow-read, the same discipline [`Rvalue::Len`] and
    /// [`Rvalue::Discriminant`] use — while the `Str`/integer inputs are
    /// ordinary operands. Subject presence/type, operand counts/types, and
    /// the destination type are fixed per op and verified (`M0012`).
    /// [`HeapOp::StringByteAt`], [`HeapOp::StringSlice`], and
    /// [`HeapOp::ArrayGet`] carry their `IndexOutOfBounds` trap **in the
    /// operation**, exactly as [`StrOp`] does; nothing else traps. Never
    /// constant-folded (the trapping ops for the usual reason, the
    /// constructors because a heap value has no constant form); native
    /// lowering is ADR-0009 Stage B, and until then both backends refuse
    /// every `HeapOp`.
    HeapOp {
        /// Which heap operation.
        op: HeapOp,
        /// The borrowed heap-value subject (`in String` / `in Array[I64]`),
        /// for the ops that read an existing value; `None` for the
        /// constructors and the `Str`-only ops.
        subject: Option<Place>,
        /// The remaining operands (`Str` views and integers), in the
        /// builtin's declaration order (see [`HeapOp`]).
        args: Vec<Operand>,
    },
}

/// A pure, value-producing heap operation (ADR-0009 Stage A). Let `len(x)`
/// be the byte/element length of the subject. See
/// `specification/mir.md` §5.7 for the normative table; the `String` byte
/// operations follow `std::str`'s byte-level contract (a slice may split a
/// multi-byte code point — `String` bytes are UTF-8 by convention, not by
/// invariant).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HeapOp {
    /// `empty() -> String` — the empty owned string. Never traps.
    StringEmpty,
    /// `from_str(s: Str) -> String` — `s`'s bytes copied into a new owned
    /// buffer. Never traps.
    StringFromStr,
    /// `concat(a: Str, b: Str) -> String` — `a`'s bytes then `b`'s, in a
    /// new owned buffer. Never traps.
    StringConcat,
    /// `len(subject: String) -> I64` — `len(subject)`. Never traps.
    StringLen,
    /// `byte_at(subject: String, i: I64) -> I64` — the byte (`0..=255`) at
    /// `i`. **Traps `IndexOutOfBounds`** when `i < 0` or
    /// `i >= len(subject)`.
    StringByteAt,
    /// `slice(subject: String, a: I64, b: I64) -> String` — the byte range
    /// `[a, b)` **copied out** as a new owned `String` (never an aliasing
    /// view). **Traps `IndexOutOfBounds`** unless
    /// `0 <= a <= b <= len(subject)`.
    StringSlice,
    /// `as_str(subject: String) -> Str` — a borrowed `Str` view of the whole
    /// subject (the `{ptr, len}` prefix of its header), **no copy** (ADR-0010).
    /// The result aliases the subject's bytes; the ownership checker keeps the
    /// subject shared-borrowed for the view's lifetime, so the native lowering
    /// (Stage B) needs no defensive copy. Never traps.
    StringAsStr,
    /// `empty() -> Array[I64]` — the empty growable array. Never traps.
    ArrayEmpty,
    /// `len(subject: Array[I64]) -> I64` — `len(subject)`. Never traps.
    /// Produces `I64` (the surface `-> Int` contract), unlike
    /// [`Rvalue::Len`]'s `Usize` for the indexing path — both remain, with
    /// different types and different jobs.
    ArrayLen,
    /// `get(subject: Array[I64], i: I64) -> I64` — the element at `i`.
    /// **Traps `IndexOutOfBounds`** when `i < 0` or `i >= len(subject)`.
    ArrayGet,
    /// `empty() -> Map[K, V]` — the empty hash map (ADR-0011). Never traps.
    MapEmpty,
    /// `get(subject: Map[K, V], k: K) -> Option[V]` — the value for `k`
    /// (`Some`) or `None`. Never traps.
    MapGet,
    /// `contains_key(subject: Map[K, V], k: K) -> Bool` — is `k` present?
    /// Never traps.
    MapContainsKey,
    /// `len(subject: Map[K, V]) -> I64` — the entry count. Never traps.
    MapLen,
    /// `keys(subject: Map[K, V]) -> Array[K]` — a new array of the keys in
    /// **insertion order** (the deterministic order ADR-0011 fixes; a
    /// removed key's successors keep their relative order). Never traps.
    MapKeys,
}

impl HeapOp {
    /// The stable lowercase name, qualified by value kind
    /// (`string_empty`, …, `array_get`) so the rendered MIR is unambiguous.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::StringEmpty => "string_empty",
            Self::StringFromStr => "string_from_str",
            Self::StringConcat => "string_concat",
            Self::StringLen => "string_len",
            Self::StringByteAt => "string_byte_at",
            Self::StringSlice => "string_slice",
            Self::StringAsStr => "string_as_str",
            Self::ArrayEmpty => "array_empty",
            Self::ArrayLen => "array_len",
            Self::ArrayGet => "array_get",
            Self::MapEmpty => "map_empty",
            Self::MapGet => "map_get",
            Self::MapContainsKey => "map_contains_key",
            Self::MapLen => "map_len",
            Self::MapKeys => "map_keys",
        }
    }

    /// Whether the op reads an existing heap value through a `subject`
    /// place (`Some` in [`Rvalue::HeapOp`]), as opposed to constructing
    /// one from operands alone.
    #[must_use]
    pub const fn has_subject(self) -> bool {
        matches!(
            self,
            Self::StringLen
                | Self::StringByteAt
                | Self::StringSlice
                | Self::StringAsStr
                | Self::ArrayLen
                | Self::ArrayGet
                | Self::MapGet
                | Self::MapContainsKey
                | Self::MapLen
                | Self::MapKeys
        )
    }

    /// How many operands the op takes (after the optional subject).
    #[must_use]
    pub const fn arg_count(self) -> usize {
        match self {
            Self::StringEmpty
            | Self::StringLen
            | Self::StringAsStr
            | Self::ArrayEmpty
            | Self::ArrayLen
            | Self::MapEmpty
            | Self::MapLen
            | Self::MapKeys => 0,
            Self::StringFromStr
            | Self::StringByteAt
            | Self::ArrayGet
            | Self::MapGet
            | Self::MapContainsKey => 1,
            Self::StringConcat | Self::StringSlice => 2,
        }
    }
}

/// A pure byte-level string operation over the UTF-8 buffer of a `Str`
/// (ADR-0006 Stage A). Let `len(s)` be the byte length of `s`. These are
/// **byte** operations: a [`StrOp::Slice`] may split a multi-byte code
/// point — the documented v0 contract (`specification/mir.md` §5.6).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StrOp {
    /// `len(s: Str) -> I64` — `len(s)`. Never traps.
    Len,
    /// `byte_at(s: Str, index: I64) -> I64` — the byte value (`0..=255`)
    /// at `index`. **Traps `IndexOutOfBounds`** when `index < 0` or
    /// `index >= len(s)`.
    ByteAt,
    /// `slice(s: Str, start: I64, end: I64) -> Str` — the byte range
    /// `[start, end)` of `s`. **Traps `IndexOutOfBounds`** unless
    /// `0 <= start <= end <= len(s)`.
    Slice,
}

impl StrOp {
    /// The stable lowercase name (`len`, `byte_at`, `slice`).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Len => "len",
            Self::ByteAt => "byte_at",
            Self::Slice => "slice",
        }
    }

    /// How many operands the op takes.
    #[must_use]
    pub const fn arg_count(self) -> usize {
        match self {
            Self::Len => 1,
            Self::ByteAt => 2,
            Self::Slice => 3,
        }
    }
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
    /// A fixed-size array value `[element; len]`. The operands are the
    /// `len` elements in index order. Carrying both `element` and `len`
    /// keeps verification local (no destination-type inference) and makes
    /// the rendered MIR self-describing (ADR-0004 Stage 2).
    Array {
        /// The element type.
        element: Ty,
        /// The number of elements; always equals the operand count.
        len: u64,
    },
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
