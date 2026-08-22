//! MIR → LLVM IR lowering for the v0 scalar core.
//!
//! This is the LLVM counterpart of the Cranelift backend's `lower` module, and
//! it is written to match it — and therefore the reference interpreter —
//! instruction for instruction. [`lower_program`] declares every function (so
//! direct calls resolve), then defines each body by walking its basic blocks
//! and translating each statement and terminator to LLVM IR that computes
//! exactly what the interpreter computes. Anything outside the scalar subset is
//! reported as [`CodegenError::unsupported`]; the lowering never emits code
//! whose meaning it is unsure of.
//!
//! # Trapping semantics
//!
//! Integer arithmetic traps on overflow, and division/remainder trap on a zero
//! divisor and on `MIN / -1` (Constitution §24). Overflow is detected with
//! LLVM's `llvm.{s,u}{add,sub,mul}.with.overflow` intrinsics (the same checks
//! the Cranelift backend gets from its overflow-flagged instructions); each
//! check branches to a dedicated trap block that calls the runtime
//! [`TRAP_SYMBOL`](tuo_runtime::TRAP_SYMBOL) with the matching
//! [`TrapCode`](tuo_runtime::TrapCode) and then reaches `unreachable` — the
//! runtime call never returns. This mirrors the interpreter's deterministic
//! abort.
//!
//! # Locals
//!
//! Each MIR local is classified once, up front (see [`LocalKind`]):
//!
//! - a **scalar** local (bool/char/int/float) is an `alloca` of a single
//!   scalar, read with `load` and written with `store`; `mem2reg` promotes it
//!   to a register;
//! - a **unit** local carries no value;
//! - an **aggregate** local (a Stage-1 product type, or an ADR-0004 Stage 2
//!   fixed array `[T; N]`) is an `alloca` of the aggregate's exact byte layout,
//!   force-aligned to the ABI alignment, whose fields/elements are accessed by
//!   GEP-ing a byte offset from the ABI (see [`tuo_runtime::abi`]);
//! - a **borrow-mode (`in`/`mut`) parameter** is a pointer to caller-owned
//!   memory; reads and writes go through the pointer directly.
//!
//! A local whose type is a heap **wrapper** (`Box`/`Shared`/`Weak`) still makes
//! the whole function unsupported (they await a later ADR). The owned `String`
//! and the growable `Array[Int]`, by contrast, are lowered since ADR-0009 Stage
//! B: their three-word `{ptr, len, cap}` header is an ordinary aggregate held in
//! an alloca (moved by memcpy of the header, passed by-pointer, returned by
//! sret), and the buffer it points at is real heap memory acquired through
//! `tuo_rt_alloc` and freed by the `Drop` glue — see the "Heap values" section
//! below. Aggregate lowering follows ADR-0004 and is
//! byte-for-byte identical to the Cranelift backend: scalar leaves,
//! by-pointer/sret call ABI, every size/offset from the runtime ABI. Fixed
//! arrays are laid out inline — element `i` at `i × stride(T)` — and indexed
//! by unchecked address arithmetic, because MIR asserts the bounds
//! (`Assert { IndexOutOfBounds }`) before every `Projection::Index` use.
//!
//! # Heap values (ADR-0009 Stage B)
//!
//! Mirrors the Cranelift backend's heap-value lowering decision for decision (and
//! therefore the reference interpreter's `eval_heap_op`/`exec_heap_mutate`): an
//! owned `String` and a growable `Array[Int]` are three-word `{ptr, len, cap}`
//! headers whose header lives in an alloca and whose buffer is separate heap
//! memory. **Empty** writes `{ptr = ZERO_SIZE_SENTINEL, len = 0, cap = 0}` (never
//! dereferenced, never freed); **constructors** (`from_str`, `concat`, `slice`)
//! and **growth** (`push`/`append`/`push_byte`) call `tuo_rt_alloc` and copy;
//! growth is *alloc-new + copy + dealloc-old* here (the C shim stays
//! acquire/release only), doubling capacity, freeing the old buffer only when the
//! old `cap != 0`; **reads** (`len`/`byte_at`/`get`) load the length/element,
//! bounds-checking `byte_at`/`get` through the shared `guard()` `IndexOutOfBounds`
//! path; `push_byte` traps `InvalidByte` before touching memory; and **`Drop`** of
//! a `String`/`Array[Int]` frees the buffer with `tuo_rt_dealloc(ptr, cap ×
//! stride, align)`, guarded on `cap != 0`. Only length, contents, and `pop`'s
//! `Option` are observable, so any doubling policy agrees with the interpreter; a
//! move de-initializes the moved-from place, so a buffer is freed exactly once.
//!
//! # Strings and effects (ADR-0006 Stage B)
//!
//! Mirrors the Cranelift backend's `lower` module decision for decision. A
//! `Str` is an ordinary two-word aggregate — the `{u8 *ptr, usize len}` fat
//! pointer of `specification/abi.md` ("Slices"), laid out by
//! [`tuo_runtime::abi::layout_of`] — and flows through the existing aggregate
//! machinery unchanged. A `Const::Str`'s bytes are emitted once per module
//! into a private, unnamed-addr, read-only global (identical literals
//! deduplicated) and the constant materializes as `{data address, len}`; an
//! empty literal carries `len = 0` and a fixed non-null pointer that is never
//! dereferenced. `Str` equality is byte-wise (lengths equal AND bytes equal,
//! via the C library's `memcmp` when the lengths match), the `std::str` byte
//! operations ([`Rvalue::StrOp`]) trap `IndexOutOfBounds` exactly as the
//! interpreter's `eval_str_op` does, and a host effect
//! ([`Statement::Effect`]) is a direct call to the matching
//! [`tuo_runtime::effect`] symbol (`tuo_rt_write`/`tuo_rt_read_byte`/
//! `tuo_rt_exit` — the last never returns, so the block is terminated with
//! `unreachable`, the same shape the trap path uses).
//!
//! # Borrow-mode calling convention
//!
//! Pinned identically in the Cranelift backend's `lower` module (see
//! `specification/abi.md`, "Passing modes"):
//!
//! - the **caller** passes the ADDRESS of the argument place as a pointer
//!   argument — every local is already an `alloca` here, so a scalar root's
//!   own alloca is that address (its escape into the call is exactly what
//!   stops `mem2reg` promoting it, mirroring the Cranelift backend's explicit
//!   slot demotion); an aggregate passes its alloca;
//! - the **callee** receives that pointer and reads/writes through it
//!   directly: **no copy-in and no copy-back**. The interpreter's
//!   copy-in/copy-back is observably identical because the borrow checker
//!   forbids aliasing (any number of `in` XOR one `mut`) and the borrow lives
//!   only for the call;
//! - forwarding a borrowed parameter as another `in`/`mut` argument passes the
//!   pointer value itself;
//! - a unit-typed borrow occupies no ABI slot, like every unit value;
//! - `take` parameters and returns are unchanged.

use std::collections::HashMap;

use inkwell::AddressSpace;
use inkwell::FloatPredicate;
use inkwell::IntPredicate;
use inkwell::basic_block::BasicBlock as LlvmBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::intrinsics::Intrinsic;
use inkwell::module::{Linkage, Module};
use inkwell::types::{BasicMetadataTypeEnum, BasicType as _, BasicTypeEnum, IntType};
use inkwell::values::{
    BasicMetadataValueEnum, BasicValue, BasicValueEnum, FunctionValue, GlobalValue, IntValue,
    PointerValue,
};

use tuo_codegen::CodegenError;
use tuo_mir::{
    Arg, BinOp, Callee, CastKind, Const, EffectOp, Function, HeapMutOp, HeapOp, Operand, PassMode,
    Place, Program, Projection, Rvalue, Statement, StrOp, Terminator, Trap, UnOp,
};
use tuo_resolve::SymbolId;
use tuo_runtime::abi::{
    Layout, POINTER_SIZE, layout_of, struct_field_offsets, variant_field_offsets,
};
use tuo_runtime::{TRAP_SYMBOL, TrapCode, alloc, effect, map};
use tuo_types::{FloatKind, IntKind, ParamMode, Ty, TypeckResult};

use crate::abi::{float_type, int_type, int_width_bits, is_signed, scalar_type};

/// The byte offset of the `len` word inside a `Str` fat pointer — one pointer
/// word past the data pointer (`{u8 *ptr, usize len}`, `specification/abi.md`
/// "Slices").
const STR_LEN_OFFSET: u64 = POINTER_SIZE;

/// The byte offset of the `len` word inside a `String`/`Array` header
/// (`{ptr, len, cap}`, `specification/abi.md`; the `ptr` word is at offset 0,
/// loaded/stored through the header base directly).
const HDR_LEN_OFFSET: u64 = POINTER_SIZE;
/// The byte offset of the `cap` word inside a `String`/`Array` header.
const HDR_CAP_OFFSET: u64 = 2 * POINTER_SIZE;

/// How a MIR local is stored by the backend, decided once up front from its
/// declared type. Parallels the Cranelift backend's `LocalKind` so the two make
/// identical ABI choices.
enum LocalKind<'ctx> {
    /// A scalar (bool/char/int/float) `alloca` of the given scalar type.
    Scalar(PointerValue<'ctx>, BasicTypeEnum<'ctx>),
    /// A `Unit` local (or a zero-sized aggregate): carries no value, no slot.
    Unit,
    /// A Stage-1 aggregate `alloca` (byte array, ABI-aligned); `ty` is the
    /// local's declared type, source of truth for field offsets.
    Aggregate {
        /// The pointer to the aggregate's byte storage.
        ptr: PointerValue<'ctx>,
        /// The aggregate's declared type.
        ty: Ty,
    },
    /// A borrow-mode (`in`/`mut`) parameter: a pointer to caller-owned memory,
    /// kept in a pointer-cell `alloca` (`mem2reg` promotes it). Scalar reads
    /// load through the pointer, `mut` scalar writes store through it,
    /// projections use it as the base address, and forwarding it as another
    /// borrow argument passes the pointer value itself. There is **no**
    /// copy-in and **no** copy-back.
    Borrowed {
        /// The alloca holding the incoming caller address.
        cell: PointerValue<'ctx>,
        /// The parameter's declared type (source of truth for offsets).
        ty: Ty,
    },
}

/// The storage classification of a local, from its declared type and the ABI.
/// Identical rules to the Cranelift backend's `classify_storage`.
enum Storage {
    Scalar,
    Unit,
    Aggregate(Layout),
}

/// Classify a local's declared type into its backend storage. See [`Storage`].
///
/// # Errors
///
/// [`CodegenError::unsupported`] if the type is a `Box`/`Shared`/`Weak` heap
/// wrapper (refused **here**, before any layout query, so the refusal names
/// the concrete type) or has no v0 runtime layout. `Str` is *not* refused
/// (ADR-0006 Stage B): it is an ordinary two-word aggregate whose bytes live
/// in static data; `String`/`Array` are three-word headers with real drop glue
/// (ADR-0009 and the ADR-0012 owned-element increment).
fn classify_storage(ty: &Ty, types: &TypeckResult, context: &str) -> Result<Storage, CodegenError> {
    // Wrapper values have an ABI *layout* (a pointer), but the backend has no
    // lowering to give them meaning yet. Refuse them at classification time
    // with a message naming the type and the road back, so they can never
    // wander into an internal invariant error downstream.
    if let Some(refusal) = heap_type_refusal(ty, context) {
        return Err(CodegenError::unsupported(refusal));
    }
    if scalar_type_is_some(ty) {
        return Ok(Storage::Scalar);
    }
    if matches!(ty, Ty::Unit) {
        return Ok(Storage::Unit);
    }
    match layout_of(ty, types) {
        Ok(layout) if layout.size == 0 => Ok(Storage::Unit),
        Ok(layout) => Ok(Storage::Aggregate(layout)),
        Err(error) => Err(CodegenError::unsupported(format!(
            "`{context}` uses a type the LLVM backend does not lower yet: {error}"
        ))),
    }
}

/// The clean refusal message for a heap-owning type the backend does not lower
/// yet, or `None` if `ty` is not one. Mirrors the Cranelift backend's
/// `heap_type_refusal` word for word (bar the backend name), so the two
/// backends refuse the same boundary with the same explanation. `Str` is no
/// longer here (ADR-0006 Stage B lowers it as a two-word aggregate), and since
/// ADR-0009 Stage B neither are the owned `String` and the growable
/// `Array[Int]` — they are three-word `{ptr, len, cap}` aggregates backed by
/// real heap memory. Only the memory wrappers (`Box`/`Shared`/`Weak`) remain
/// refused, awaiting a later ADR.
fn heap_type_refusal(ty: &Ty, context: &str) -> Option<String> {
    match ty {
        Ty::Wrapper(kind, _) => Some(format!(
            "`{context}` uses a `{}[T]` heap wrapper, which the LLVM backend does not \
             lower yet (heap wrappers await a later ADR); the interpreter \
             (`tuo spec`/`tuo verify`) remains the reference",
            kind.name()
        )),
        _ => None,
    }
}

/// Whether `ty` transitively owns heap — a `String`, growable `Array`, a
/// `Box`/`Shared`/`Weak`, or a struct/enum any of whose fields does. Such a
/// type needs a deep copy on read-out (`emit_heap_glue` with
/// [`HeapGlue::DeepFixup`]) and recursive drop glue ([`HeapGlue::DropInPlace`]).
/// Mirrors the Cranelift backend's `ty_owns_heap` so both walk the identical
/// set and the three-way differential stays consistent. `Str` is not heap-owning.
fn ty_owns_heap(ty: &Ty, types: &TypeckResult) -> bool {
    match ty {
        Ty::String | Ty::Array(_) | Ty::Map(..) | Ty::Wrapper(..) => true,
        Ty::Struct(symbol, targs) => types.struct_shape(*symbol).is_some_and(|shape| {
            shape_field_owns_heap(&shape.fields, &shape.type_params, targs, types)
        }),
        Ty::Enum(symbol, targs) => types.enum_shape(*symbol).is_some_and(|shape| {
            shape
                .variants
                .iter()
                .any(|(_, fields)| shape_field_owns_heap(fields, &shape.type_params, targs, types))
        }),
        Ty::Option(item) | Ty::Range(item) => ty_owns_heap(item, types),
        Ty::Result(a, b) => ty_owns_heap(a, types) || ty_owns_heap(b, types),
        Ty::Tuple(items) => items.iter().any(|item| ty_owns_heap(item, types)),
        Ty::FixedArray(item, _) => ty_owns_heap(item, types),
        _ => false,
    }
}

/// Do any of `fields` (under a struct/enum's `params` → `targs` substitution) own
/// heap? Fields carry declaration-form types (`Ty::Param`), substituted here.
fn shape_field_owns_heap(
    fields: &[(String, Ty)],
    params: &[SymbolId],
    targs: &[Ty],
    types: &TypeckResult,
) -> bool {
    fields
        .iter()
        .any(|(_, ty)| ty_owns_heap(&substitute_targs(ty, params, targs), types))
}

/// Substitute a struct/enum's type parameters into a field type. Mirrors the
/// Cranelift backend's helper of the same name.
fn substitute_targs(ty: &Ty, params: &[SymbolId], targs: &[Ty]) -> Ty {
    match ty {
        Ty::Param(symbol) => params
            .iter()
            .position(|p| p == symbol)
            .and_then(|i| targs.get(i).cloned())
            .unwrap_or_else(|| ty.clone()),
        Ty::Option(item) => Ty::Option(Box::new(substitute_targs(item, params, targs))),
        Ty::Array(item) => Ty::Array(Box::new(substitute_targs(item, params, targs))),
        Ty::FixedArray(item, n) => {
            Ty::FixedArray(Box::new(substitute_targs(item, params, targs)), *n)
        }
        Ty::Tuple(items) => Ty::Tuple(
            items
                .iter()
                .map(|item| substitute_targs(item, params, targs))
                .collect(),
        ),
        Ty::Result(a, b) => Ty::Result(
            Box::new(substitute_targs(a, params, targs)),
            Box::new(substitute_targs(b, params, targs)),
        ),
        Ty::Struct(sym, args) => Ty::Struct(
            *sym,
            args.iter()
                .map(|arg| substitute_targs(arg, params, targs))
                .collect(),
        ),
        Ty::Enum(sym, args) => Ty::Enum(
            *sym,
            args.iter()
                .map(|arg| substitute_targs(arg, params, targs))
                .collect(),
        ),
        Ty::Wrapper(kind, item) => {
            Ty::Wrapper(*kind, Box::new(substitute_targs(item, params, targs)))
        }
        other => other.clone(),
    }
}

/// Does `ty` transitively contain a `Box`/`Shared`/`Weak` heap wrapper?
/// Wrapper *values* are not lowered at all (they await their own ADR), so a
/// container element carrying one is refused rather than half-lowered. This is
/// narrower than [`ty_owns_heap`]: since the ADR-0012 owned-element increment,
/// `String` and nested `Array` elements get real deep-copy/drop glue and are no
/// longer refused. Mirrors the Cranelift backend's `ty_contains_wrapper`.
fn ty_contains_wrapper(ty: &Ty, types: &TypeckResult) -> bool {
    match ty {
        Ty::Wrapper(..) => true,
        Ty::Struct(symbol, targs) => types.struct_shape(*symbol).is_some_and(|shape| {
            shape.fields.iter().any(|(_, field)| {
                ty_contains_wrapper(&substitute_targs(field, &shape.type_params, targs), types)
            })
        }),
        Ty::Enum(symbol, targs) => types.enum_shape(*symbol).is_some_and(|shape| {
            shape.variants.iter().any(|(_, fields)| {
                fields.iter().any(|(_, field)| {
                    ty_contains_wrapper(&substitute_targs(field, &shape.type_params, targs), types)
                })
            })
        }),
        Ty::Option(item) | Ty::Range(item) | Ty::Array(item) | Ty::FixedArray(item, _) => {
            ty_contains_wrapper(item, types)
        }
        Ty::Result(a, b) => ty_contains_wrapper(a, types) || ty_contains_wrapper(b, types),
        Ty::Tuple(items) => items.iter().any(|item| ty_contains_wrapper(item, types)),
        _ => false,
    }
}

/// Refuse a growable-`Array` element type the native path does not lower.
///
/// Since the ADR-0012 owned-element increment the native path lowers the whole
/// checker-accepted element set: no-heap elements via element-size-aware
/// load/store and memcpy, and heap-owning elements (`String`, a struct/enum
/// carrying one) with a deep copy on `get` and per-element drop glue
/// (`emit_heap_glue`). Only an element containing a `Box`/`Shared`/`Weak` heap
/// wrapper is still refused — wrapper values are not lowered anywhere (their
/// own ADR). Mirrors the Cranelift backend so both refuse the identical set and
/// the three-way differential stays consistent.
///
/// # Errors
///
/// [`CodegenError::unsupported`] for a wrapper-containing element type.
fn require_native_array_element(element: &Ty, types: &TypeckResult) -> Result<(), CodegenError> {
    if ty_contains_wrapper(element, types) {
        Err(CodegenError::unsupported(format!(
            "the native backend does not lower an array element containing a \
             `Box`/`Shared`/`Weak` heap wrapper; `Array[{element:?}]` awaits the wrapper ADR — \
             use `tuo spec`/`tuo verify` to run it on the reference interpreter"
        )))
    } else {
        Ok(())
    }
}

/// Which heap glue `emit_heap_glue` walks a value with (ADR-0012 owned-element
/// increment). Both walks visit exactly the heap-owning parts of a value, so
/// they can never disagree about what owns a buffer. Mirrors the Cranelift
/// backend's `HeapGlue`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum HeapGlue {
    /// The value at the address is a fresh *bitwise* copy whose heap headers
    /// still alias the original owner's buffers: replace each with a freshly
    /// allocated copy, making the value an independent owner — the native
    /// mirror of the interpreter's `Value::clone` on `array::get`.
    DeepFixup,
    /// Free every buffer the value at the address owns (elements first, then
    /// the containing storage) — the native mirror of the interpreter's
    /// de-initializing drop, where dropping `Value::Array` recursively frees.
    DropInPlace,
}

/// Whether `ty` is a scalar the backend maps to an LLVM scalar, without needing
/// a `Context`. Mirrors `scalar_type`'s domain (bool/char/int/float, plus the
/// function-value code pointer, ADR-0008 Tier 1).
fn scalar_type_is_some(ty: &Ty) -> bool {
    matches!(
        ty,
        Ty::Bool | Ty::Char | Ty::Int(_) | Ty::Float(_) | Ty::Fn(_)
    )
}

/// The linkage a compiled tuonelang function gets: external, so the entry (and
/// the `main` shim) are visible to the linker and inter-function calls resolve.
const FUNCTION_LINKAGE: Linkage = Linkage::External;

/// Declare then define every lowerable function of `program` into `module`.
///
/// Returns the [`FunctionValue`] each MIR function was declared as, keyed by its
/// stable symbol, so the caller can find the entry to build the `main` shim.
///
/// # Errors
///
/// [`CodegenError::unsupported`] if any function reachable in the program uses
/// a feature outside the v0 scalar subset; [`CodegenError::backend`] on an
/// internal LLVM builder failure.
pub(crate) fn lower_program<'ctx>(
    ctx: &'ctx Context,
    module: &Module<'ctx>,
    program: &Program,
    types: &TypeckResult,
) -> Result<HashMap<SymbolId, FunctionValue<'ctx>>, CodegenError> {
    // Pass 1: declare every function so direct calls can reference them before
    // their bodies are defined.
    let mut ids: HashMap<SymbolId, FunctionValue<'ctx>> = HashMap::new();
    for function in &program.functions {
        let fn_type = function_type(ctx, function, types)?;
        let name = mangle(function.symbol);
        let value = module.add_function(&name, fn_type, Some(FUNCTION_LINKAGE));
        ids.insert(function.symbol, value);
    }

    // Pass 2: define each body. The string-literal global pool is shared
    // across bodies so identical literals dedupe to one read-only global per
    // module (ADR-0006 Stage B).
    let builder = ctx.create_builder();
    let mut str_globals: HashMap<Vec<u8>, GlobalValue<'ctx>> = HashMap::new();
    for function in &program.functions {
        let mut lowering = Lowering::new(
            ctx,
            module,
            &builder,
            &ids,
            function,
            &program.functions,
            types,
            &mut str_globals,
        )?;
        lowering.run()?;
    }
    Ok(ids)
}

/// The LLVM function type of a MIR function (v0 ABI, Stage-1 aggregates).
///
/// Applies ADR-0004 Stage 1's two aggregate rules, identically to the Cranelift
/// backend's `function_signature`:
/// - an **aggregate return** prepends an sret `ptr` at argument index 0 and the
///   native return type becomes `void`;
/// - an **aggregate parameter** is a `ptr` to a caller-owned copy.
///
/// Every size/align derives from [`tuo_runtime::abi`], never from LLVM's native
/// small-struct classification.
fn function_type<'ctx>(
    ctx: &'ctx Context,
    function: &Function,
    types: &TypeckResult,
) -> Result<inkwell::types::FunctionType<'ctx>, CodegenError> {
    let params = function
        .params
        .iter()
        .enumerate()
        .map(|(index, mode)| (*mode, function.locals[index].ty.clone()));
    fn_type_from_parts(ctx, &function.ret, params, types, &function.name)
}

/// Build the LLVM `FunctionType` for a `(ret, [(mode, ty)])` calling contract.
///
/// This is the single source of truth for the v0 call ABI, shared by the direct
/// path ([`function_type`], over a `Function`'s declared params) and the
/// indirect path (over the callee value's `Ty::Fn` modes+types), so the two can
/// never drift — ADR-0008 Stage B requires the indirect-call convention to be
/// byte-identical to the direct one. Applies the two aggregate rules: an
/// aggregate return is an sret out-pointer prepended at index 0 (return becomes
/// `void`), and an aggregate/borrow parameter is a `ptr`. `context` names the
/// owner for a diagnostic on a non-scalar-by-value type.
fn fn_type_from_parts<'ctx>(
    ctx: &'ctx Context,
    ret: &Ty,
    params: impl Iterator<Item = (PassMode, Ty)>,
    types: &TypeckResult,
    context: &str,
) -> Result<inkwell::types::FunctionType<'ctx>, CodegenError> {
    let ptr_ty = ctx.ptr_type(AddressSpace::default());
    let mut param_tys: Vec<BasicMetadataTypeEnum<'ctx>> = Vec::new();

    // sret hidden out-pointer is ALWAYS argument index 0 (prepended).
    let ret_storage = classify_storage(ret, types, context)?;
    if matches!(ret_storage, Storage::Aggregate(_)) {
        param_tys.push(ptr_ty.into());
    }

    // Parameters, in declaration order after the (optional) sret pointer.
    // Borrow-mode calling convention (identical in the Cranelift backend's
    // `signature_from_parts`, per `specification/abi.md` "Passing modes"): an
    // `in`/`mut` parameter arrives as a **pointer to the caller's place** —
    // scalar or aggregate alike — read (and, for `mut`, written) through
    // directly, with no copy-in and no copy-back. A `take` parameter is
    // unchanged: scalar by value, aggregate by pointer to a caller-owned copy,
    // unit occupying no ABI slot (in any mode).
    for (mode, ty) in params {
        let storage = classify_storage(&ty, types, context)?;
        match (mode, storage) {
            (_, Storage::Unit) => {}
            (PassMode::Value, Storage::Scalar) => {
                param_tys.push(require_scalar(ctx, &ty, context)?.into());
            }
            (PassMode::Value, Storage::Aggregate(_))
            | (PassMode::Borrow | PassMode::BorrowMut, _) => param_tys.push(ptr_ty.into()),
        }
    }

    // Return: scalar by value; aggregate is void (written through sret); unit is
    // void.
    Ok(match ret_storage {
        Storage::Scalar => require_scalar(ctx, ret, context)?.fn_type(&param_tys, false),
        Storage::Unit | Storage::Aggregate(_) => ctx.void_type().fn_type(&param_tys, false),
    })
}

/// The MIR [`PassMode`] a function-type parameter's [`ParamMode`] corresponds
/// to. A function value's per-argument borrow discipline at an indirect call
/// site is driven by these modes exactly as a direct call's is by its declared
/// `PassMode`s (ADR-0008 Tier 1); the two vocabularies map one-to-one.
fn pass_mode_of(mode: ParamMode) -> PassMode {
    match mode {
        ParamMode::Take => PassMode::Value,
        ParamMode::In => PassMode::Borrow,
        ParamMode::Mut => PassMode::BorrowMut,
    }
}

/// The function-type [`ParamMode`] a MIR [`PassMode`] corresponds to — the
/// inverse of [`pass_mode_of`]. Used to reconstruct a `Const::Fn`'s `Ty::Fn`
/// from the referenced function's MIR signature.
fn param_mode_of(mode: PassMode) -> ParamMode {
    match mode {
        PassMode::Value => ParamMode::Take,
        PassMode::Borrow => ParamMode::In,
        PassMode::BorrowMut => ParamMode::Mut,
    }
}

/// The scalar LLVM type of `ty`, or an unsupported error naming `context`.
fn require_scalar<'ctx>(
    ctx: &'ctx Context,
    ty: &Ty,
    context: &str,
) -> Result<BasicTypeEnum<'ctx>, CodegenError> {
    scalar_type(ctx, ty).ok_or_else(|| {
        CodegenError::unsupported(format!(
            "`{context}` uses a non-scalar type the LLVM backend does not lower yet"
        ))
    })
}

/// The per-function lowering state.
struct Lowering<'a, 'ctx> {
    ctx: &'ctx Context,
    module: &'a Module<'ctx>,
    builder: &'a Builder<'ctx>,
    ids: &'a HashMap<SymbolId, FunctionValue<'ctx>>,
    function: &'a Function,
    /// Every function in the program, so a direct call can read its callee's
    /// return type and name for the aggregate call ABI.
    functions: &'a [Function],
    types: &'a TypeckResult,
    /// The opaque pointer type (for slot addresses and aggregate args).
    ptr_ty: inkwell::types::PointerType<'ctx>,
    value: FunctionValue<'ctx>,
    /// The LLVM block for each MIR block index.
    blocks: Vec<LlvmBlock<'ctx>>,
    /// How each MIR local is stored (index = local id): scalar alloca, unit, or
    /// aggregate byte alloca. Filled during `run`.
    kinds: Vec<LocalKind<'ctx>>,
    /// The module's string-literal global pool: the read-only global emitted
    /// for each distinct literal's bytes, shared across function bodies so
    /// identical literals dedupe (ADR-0006 Stage B).
    str_globals: &'a mut HashMap<Vec<u8>, GlobalValue<'ctx>>,
}

impl<'a, 'ctx> Lowering<'a, 'ctx> {
    #[expect(
        clippy::too_many_arguments,
        reason = "a private plumbing seam mirroring the Cranelift backend's `Lowering::new`; \
                  bundling these into a context struct would only move the argument list"
    )]
    fn new(
        ctx: &'ctx Context,
        module: &'a Module<'ctx>,
        builder: &'a Builder<'ctx>,
        ids: &'a HashMap<SymbolId, FunctionValue<'ctx>>,
        function: &'a Function,
        functions: &'a [Function],
        types: &'a TypeckResult,
        str_globals: &'a mut HashMap<Vec<u8>, GlobalValue<'ctx>>,
    ) -> Result<Self, CodegenError> {
        // Classify every local up front so an unsupported (Stage-2) type fails
        // before any IR is built. Concrete allocas are created in `run`.
        for local in &function.locals {
            classify_storage(&local.ty, types, &function.name)?;
        }
        let value = ids[&function.symbol];
        Ok(Self {
            ctx,
            module,
            builder,
            ids,
            function,
            functions,
            types,
            ptr_ty: ctx.ptr_type(AddressSpace::default()),
            value,
            blocks: Vec::new(),
            kinds: Vec::new(),
            str_globals,
        })
    }

    /// Lower the whole body.
    fn run(&mut self) -> Result<(), CodegenError> {
        // Create an LLVM block per MIR block.
        self.blocks = (0..self.function.blocks.len())
            .map(|index| {
                self.ctx
                    .append_basic_block(self.value, &format!("bb{index}"))
            })
            .collect();

        // Entry block: allocate a slot per local, then seed parameters. All
        // allocas live at the top of block 0 so `mem2reg` can promote them.
        let entry = self.blocks[0];
        self.builder.position_at_end(entry);

        // Allocate storage for every local: a scalar `alloca` of its scalar
        // type; a unit local carries no value; an aggregate `alloca` of its
        // exact byte layout, force-aligned to the ABI alignment; a borrow-mode
        // parameter a pointer-cell `alloca` holding the caller's address
        // (seeded below). Unlike the Cranelift backend, no explicit slot
        // demotion is needed for borrowed-out scalar locals: every scalar is
        // already an alloca, and its address escaping into a call is exactly
        // what keeps `mem2reg` from promoting it.
        self.kinds = Vec::with_capacity(self.function.locals.len());
        for (index, local) in self.function.locals.iter().enumerate() {
            let storage = classify_storage(&local.ty, self.types, &self.function.name)?;
            let borrow_param = self
                .function
                .params
                .get(index)
                .is_some_and(|mode| *mode != PassMode::Value);
            let kind = match storage {
                // A unit-typed local carries no value in every mode (a borrow
                // of a unit place likewise occupies no ABI slot).
                Storage::Unit => LocalKind::Unit,
                _ if borrow_param => {
                    let cell = self
                        .builder
                        .build_alloca(self.ptr_ty, &format!("brw{index}"))
                        .map_err(builder_err("allocating a borrow pointer cell"))?;
                    LocalKind::Borrowed {
                        cell,
                        ty: local.ty.clone(),
                    }
                }
                Storage::Scalar => {
                    let ty = require_scalar(self.ctx, &local.ty, &self.function.name)?;
                    let ptr = self
                        .builder
                        .build_alloca(ty, &format!("local{index}"))
                        .map_err(builder_err("allocating a scalar slot"))?;
                    LocalKind::Scalar(ptr, ty)
                }
                Storage::Aggregate(layout) => {
                    let ptr = self.alloca_aggregate(layout, index)?;
                    LocalKind::Aggregate {
                        ptr,
                        ty: local.ty.clone(),
                    }
                }
            };
            self.kinds.push(kind);
        }

        // If this function returns an aggregate, native parameter 0 is the sret
        // out-pointer; the MIR parameters start at native index 1.
        let returns_aggregate = matches!(
            classify_storage(&self.function.ret, self.types, &self.function.name)?,
            Storage::Aggregate(_)
        );
        let mut native = u32::from(returns_aggregate);

        // Seed each MIR parameter local from its native parameter. A scalar is
        // stored into its slot; an aggregate `take` parameter's incoming
        // pointer is memcpy'd into the callee's own slot (owned-copy
        // semantics); a borrow-mode parameter's incoming pointer is kept as-is
        // (no copy-in — it aliases the caller's place); a unit parameter
        // occupies no native slot.
        for index in 0..self.function.params.len() {
            match &self.kinds[index] {
                LocalKind::Scalar(ptr, _) => {
                    let ptr = *ptr;
                    let param = self
                        .value
                        .get_nth_param(native)
                        .ok_or_else(|| CodegenError::backend("missing LLVM parameter"))?;
                    self.builder
                        .build_store(ptr, param)
                        .map_err(builder_err("storing a parameter"))?;
                    native += 1;
                }
                LocalKind::Unit => {}
                LocalKind::Aggregate { ptr, ty } => {
                    let dest = *ptr;
                    let ty = ty.clone();
                    let src = self
                        .value
                        .get_nth_param(native)
                        .ok_or_else(|| CodegenError::backend("missing LLVM parameter"))?
                        .into_pointer_value();
                    native += 1;
                    let layout = self.layout(&ty)?;
                    self.emit_memcpy(dest, src, layout)?;
                }
                LocalKind::Borrowed { cell, .. } => {
                    let cell = *cell;
                    let addr = self
                        .value
                        .get_nth_param(native)
                        .ok_or_else(|| CodegenError::backend("missing LLVM parameter"))?
                        .into_pointer_value();
                    self.builder
                        .build_store(cell, addr)
                        .map_err(builder_err("storing a borrow pointer"))?;
                    native += 1;
                }
            }
        }

        // Lower each block's statements and terminator. Block 0's body follows
        // the allocas/parameter stores in the same LLVM block.
        for (index, block) in self.function.blocks.iter().enumerate() {
            self.builder.position_at_end(self.blocks[index]);
            for statement in &block.statements {
                self.lower_statement(statement)?;
            }
            self.lower_terminator(&block.terminator)?;
        }
        Ok(())
    }

    // ----- statements -----

    fn lower_statement(&mut self, statement: &Statement) -> Result<(), CodegenError> {
        match statement {
            Statement::Assign { place, rvalue } => self.lower_assign(place, rvalue),
            Statement::Call { dest, callee, args } => self.lower_call(dest, callee, args),
            // A host effect (`std::rt`, ADR-0006 Stage B): a direct call to
            // the matching `tuo_runtime::effect` symbol, which the CLI links
            // into every built binary alongside the trap shim.
            Statement::Effect { op, args, dest } => self.lower_effect(*op, args, dest),
            // An in-place heap mutation (ADR-0009 Stage B): mutate the owned
            // `String`/`Array[Int]` at `target` through its header, growing the
            // buffer when needed, and store the op's result into `dest`.
            Statement::HeapMutate {
                op,
                target,
                args,
                dest,
            } => self.lower_heap_mutate(*op, target, args, dest),
            Statement::Drop { place } => self.lower_drop(place),
        }
    }

    /// Lower `place = rvalue`, splitting aggregate from scalar exactly as the
    /// Cranelift backend does.
    fn lower_assign(&mut self, place: &Place, rvalue: &Rvalue) -> Result<(), CodegenError> {
        match rvalue {
            Rvalue::Aggregate { kind, fields } => self.lower_aggregate(place, kind, fields),
            // `slice` yields a `Str` (a two-word aggregate), so it is
            // materialized in place into the destination's byte storage rather
            // than through the scalar rvalue path (ADR-0006 Stage B).
            Rvalue::StrOp {
                op: StrOp::Slice,
                args,
            } => self.lower_str_slice(place, args),
            // A heap op that *produces* an owned `String`/`Array[Int]` (a
            // three-word header) is materialized in place into the
            // destination's header storage (ADR-0009 Stage B); the scalar-valued
            // reads take the scalar rvalue path below.
            Rvalue::HeapOp { op, subject, args } if heap_op_produces_aggregate(*op) => {
                self.lower_heap_op_aggregate(place, *op, subject.as_ref(), args)
            }
            // `array::get` whose element is an aggregate (`Str`/struct, ADR-0012
            // Stage B) produces an aggregate: read it into the dest slot by
            // memcpy rather than returning a scalar register value.
            Rvalue::HeapOp {
                op: HeapOp::ArrayGet,
                subject: Some(subject),
                args,
            } if self.array_get_produces_aggregate(subject)? => {
                self.lower_array_get_aggregate(place, subject, args)
            }
            // A unit-valued copy/move (a unit local, or a zero-sized aggregate
            // such as `[T; 0]`) carries no bytes: nothing to emit.
            Rvalue::Use(operand) if self.operand_is_unit(operand) => Ok(()),
            Rvalue::Use(operand) if self.operand_is_aggregate(operand) => {
                let (dest_addr, dest_ty) = self.place_address(place)?;
                let layout = self.layout(&dest_ty)?;
                let src_addr = self.operand_aggregate_address(operand)?;
                self.emit_memcpy(dest_addr, src_addr, layout)
            }
            _ => {
                let value = self.lower_rvalue(rvalue)?;
                self.write_place(place, value)
            }
        }
    }

    fn lower_call(
        &mut self,
        dest: &Place,
        callee: &Callee,
        args: &[Arg],
    ) -> Result<(), CodegenError> {
        // Resolve the call target. A `Direct` call names a function symbol and
        // calls its `FunctionValue`. An `Indirect` call (ADR-0008 Tier 1) loads
        // a runtime code-pointer value from the callee operand and calls it
        // through a `FunctionType` derived from the callee's `Ty::Fn`. Both
        // share the *entire* argument/sret/borrow marshalling and return
        // handling below — the only difference is the call instruction.
        //
        // The callee's return type drives the sret decision, so resolve it here.
        // The interpreter evaluates the indirect callee operand before its
        // arguments; match that ordering by loading the pointer first.
        enum CallTarget<'ctx> {
            Direct(FunctionValue<'ctx>),
            Indirect(PointerValue<'ctx>, inkwell::types::FunctionType<'ctx>),
        }
        let (target, callee_ret): (CallTarget<'ctx>, Ty) = match callee {
            Callee::Direct(symbol) => {
                let Some(&callee_fn) = self.ids.get(symbol) else {
                    return Err(CodegenError::unsupported(
                        "call to a function outside the lowered program (v0 has no external \
                         calls)",
                    ));
                };
                let callee_mir = self.function_named(*symbol)?;
                (CallTarget::Direct(callee_fn), callee_mir.ret.clone())
            }
            Callee::Indirect(operand) => {
                // The callee value's `Ty::Fn` carries the whole calling
                // contract (param modes+types and the return type).
                let Some(Ty::Fn(fn_ty)) = self.operand_ty(operand) else {
                    return Err(CodegenError::backend(
                        "an indirect call's callee operand is not of function type (verified MIR \
                         guarantees it is)",
                    ));
                };
                let callee_ptr = self.lower_operand(operand)?.into_pointer_value();
                let fn_type = fn_type_from_parts(
                    self.ctx,
                    &fn_ty.ret,
                    fn_ty
                        .params
                        .iter()
                        .map(|p| (pass_mode_of(p.mode), p.ty.clone())),
                    self.types,
                    &self.function.name,
                )?;
                (CallTarget::Indirect(callee_ptr, fn_type), fn_ty.ret.clone())
            }
        };

        let dest_ty = self.place_type(dest);
        let ret_is_aggregate = matches!(
            classify_storage(&callee_ret, self.types, &self.function.name)?,
            Storage::Aggregate(_)
        );

        let mut arg_values: Vec<BasicMetadataValueEnum<'ctx>> = Vec::with_capacity(args.len() + 1);

        // sret out-pointer, if the callee returns an aggregate.
        let sret = if ret_is_aggregate {
            let layout = self.layout(&callee_ret)?;
            let (ptr, addr) = self.sret_destination(dest, &dest_ty, layout)?;
            arg_values.push(addr.into());
            Some((ptr, layout))
        } else {
            None
        };

        for arg in args {
            match arg {
                Arg::Value(operand) => {
                    if self.operand_is_unit(operand) {
                        // A unit argument carries no native value.
                    } else if self.operand_is_aggregate(operand) {
                        let addr = self.materialize_aggregate_arg(operand)?;
                        arg_values.push(addr.into());
                    } else {
                        arg_values.push(self.lower_operand(operand)?.into());
                    }
                }
                Arg::Borrow(place) | Arg::BorrowMut(place) => {
                    // Borrow-mode argument: pass the ADDRESS of the caller's
                    // place (the callee reads/writes through it directly; no
                    // copy-in, no copy-back). A unit-typed borrow occupies no
                    // ABI slot, matching the signature builder.
                    let ty = self.place_type(place);
                    match classify_storage(&ty, self.types, &self.function.name)? {
                        Storage::Unit => {}
                        Storage::Scalar | Storage::Aggregate(_) => {
                            let addr = self.borrow_address(place)?;
                            arg_values.push(addr.into());
                        }
                    }
                }
            }
        }

        let call = match target {
            CallTarget::Direct(callee_fn) => self
                .builder
                .build_call(callee_fn, &arg_values, "call")
                .map_err(builder_err("emitting a call"))?,
            CallTarget::Indirect(callee_ptr, fn_type) => self
                .builder
                .build_indirect_call(fn_type, callee_ptr, &arg_values, "call")
                .map_err(builder_err("emitting an indirect call"))?,
        };

        if let Some((ptr, layout)) = sret {
            // The callee wrote the aggregate result into the sret slot. If dest
            // is projected, copy from the temporary into the projected address.
            if !dest.projection.is_empty() {
                let (dest_addr, _leaf) = self.place_address(dest)?;
                self.emit_memcpy(dest_addr, ptr, layout)?;
            }
        } else if let inkwell::values::ValueKind::Basic(value) = call.try_as_basic_value() {
            // A unit-returning callee yields no result; a scalar callee yields
            // one, stored into the (scalar) destination.
            self.write_place(dest, value)?;
        }
        Ok(())
    }

    /// The sret destination for a call returning an aggregate: `(ptr, address)`.
    /// A bare aggregate local reuses its own alloca; a projected destination gets
    /// a fresh temporary alloca copied into the projected address after the call.
    fn sret_destination(
        &mut self,
        dest: &Place,
        _dest_ty: &Ty,
        layout: Layout,
    ) -> Result<(PointerValue<'ctx>, PointerValue<'ctx>), CodegenError> {
        if dest.projection.is_empty() {
            if let LocalKind::Aggregate { ptr, .. } = &self.kinds[dest.local.0 as usize] {
                let ptr = *ptr;
                return Ok((ptr, ptr));
            }
        }
        let ptr = self.alloca_aggregate(layout, usize::MAX)?;
        Ok((ptr, ptr))
    }

    /// Materialize an aggregate call argument into a caller-owned temporary alloca
    /// and return its pointer, per the by-pointer call ABI.
    fn materialize_aggregate_arg(
        &mut self,
        operand: &Operand,
    ) -> Result<PointerValue<'ctx>, CodegenError> {
        let ty = self
            .operand_ty(operand)
            .ok_or_else(|| CodegenError::backend("aggregate argument has no static type"))?;
        let layout = self.layout(&ty)?;
        let src = self.operand_aggregate_address(operand)?;
        let dest = self.alloca_aggregate(layout, usize::MAX)?;
        self.emit_memcpy(dest, src, layout)?;
        Ok(dest)
    }

    /// The address of `place` for a borrow-mode (`in`/`mut`) call argument.
    ///
    /// A bare local resolves to its own storage: a scalar's or aggregate's
    /// alloca, or — when forwarding a borrowed parameter — the incoming
    /// pointer value itself. A projected place resolves through the shared
    /// address walk.
    fn borrow_address(&mut self, place: &Place) -> Result<PointerValue<'ctx>, CodegenError> {
        if place.projection.is_empty() {
            return match &self.kinds[place.local.0 as usize] {
                LocalKind::Scalar(ptr, _) | LocalKind::Aggregate { ptr, .. } => Ok(*ptr),
                LocalKind::Borrowed { .. } => self.borrowed_addr(place.local.0 as usize),
                LocalKind::Unit => Err(CodegenError::backend(
                    "address of a unit local requested for a borrow argument",
                )),
            };
        }
        let (addr, _leaf) = self.place_address(place)?;
        Ok(addr)
    }

    /// The caller address held by the borrowed parameter at `index` (a load of
    /// its pointer cell).
    fn borrowed_addr(&mut self, index: usize) -> Result<PointerValue<'ctx>, CodegenError> {
        let LocalKind::Borrowed { cell, .. } = &self.kinds[index] else {
            return Err(CodegenError::backend(
                "borrowed_addr called on a non-borrowed local",
            ));
        };
        Ok(self
            .builder
            .build_load(self.ptr_ty, *cell, "brw_addr")
            .map_err(builder_err("loading a borrow pointer"))?
            .into_pointer_value())
    }

    /// The MIR function with symbol `callee`, for its return type and name.
    fn function_named(&self, callee: SymbolId) -> Result<&'a Function, CodegenError> {
        self.functions
            .iter()
            .find(|f| f.symbol == callee)
            .ok_or_else(|| {
                CodegenError::unsupported(
                    "call to a function outside the lowered program (v0 has no external calls)",
                )
            })
    }

    // ----- terminators -----

    fn lower_terminator(&mut self, terminator: &Terminator) -> Result<(), CodegenError> {
        match terminator {
            Terminator::Return(operand) => {
                match classify_storage(&self.function.ret, self.types, &self.function.name)? {
                    Storage::Unit => {
                        self.builder
                            .build_return(None)
                            .map_err(builder_err("emitting a unit return"))?;
                    }
                    Storage::Scalar => {
                        let value = self.lower_operand(operand)?;
                        self.builder
                            .build_return(Some(&value as &dyn BasicValue))
                            .map_err(builder_err("emitting a return"))?;
                    }
                    Storage::Aggregate(layout) => {
                        // Copy the operand's aggregate into the sret out-pointer
                        // (native parameter 0), then return void.
                        let src = self.operand_aggregate_address(operand)?;
                        let sret = self
                            .value
                            .get_nth_param(0)
                            .ok_or_else(|| CodegenError::backend("missing sret parameter"))?
                            .into_pointer_value();
                        self.emit_memcpy(sret, src, layout)?;
                        self.builder
                            .build_return(None)
                            .map_err(builder_err("emitting an aggregate return"))?;
                    }
                }
                Ok(())
            }
            Terminator::Goto(target) => {
                let block = self.blocks[target.0 as usize];
                self.builder
                    .build_unconditional_branch(block)
                    .map_err(builder_err("emitting a goto"))?;
                Ok(())
            }
            Terminator::Branch {
                cond,
                then_block,
                else_block,
            } => {
                let value = self.lower_operand(cond)?.into_int_value();
                // The condition is a `Bool` (i8, 0/1); LLVM branches want an i1.
                let cond_i1 = self.truthy(value)?;
                let then_b = self.blocks[then_block.0 as usize];
                let else_b = self.blocks[else_block.0 as usize];
                self.builder
                    .build_conditional_branch(cond_i1, then_b, else_b)
                    .map_err(builder_err("emitting a branch"))?;
                Ok(())
            }
            Terminator::Switch {
                discr,
                arms,
                otherwise,
            } => {
                let value = self.lower_operand(discr)?.into_int_value();
                let otherwise_block = self.blocks[otherwise.0 as usize];
                let cases: Vec<(IntValue<'ctx>, LlvmBlock<'ctx>)> = arms
                    .iter()
                    .map(|(case_value, target)| {
                        // Each case constant has the discriminant's own type; MIR
                        // guarantees arm values fit that type.
                        let konst = value.get_type().const_int(*case_value as u64, false);
                        (konst, self.blocks[target.0 as usize])
                    })
                    .collect();
                self.builder
                    .build_switch(value, otherwise_block, &cases)
                    .map_err(builder_err("emitting a switch"))?;
                Ok(())
            }
            Terminator::Assert { cond, trap, target } => {
                let value = self.lower_operand(cond)?.into_int_value();
                let cond_i1 = self.truthy(value)?;
                let ok_block = self.blocks[target.0 as usize];
                let trap_block = self.ctx.append_basic_block(self.value, "assert_fail");
                self.builder
                    .build_conditional_branch(cond_i1, ok_block, trap_block)
                    .map_err(builder_err("emitting an assert"))?;
                self.builder.position_at_end(trap_block);
                self.emit_trap_call(trap_code_of(*trap))?;
                Ok(())
            }
            Terminator::Trap(trap) => self.emit_trap_call(trap_code_of(*trap)),
        }
    }

    // ----- rvalues & operands -----

    fn lower_rvalue(&mut self, rvalue: &Rvalue) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        match rvalue {
            Rvalue::Use(operand) => self.lower_operand(operand),
            Rvalue::Unary { op, operand } => {
                let value = self.lower_operand(operand)?;
                self.lower_unary(*op, operand, value)
            }
            Rvalue::Binary { op, lhs, rhs } => {
                // `Str` operands take their own byte-wise equality path (the
                // type checker forbids ordering on `Str`, so only `Eq`/`Ne`
                // can reach here — ADR-0006 Stage B).
                if matches!(self.operand_ty(lhs), Some(Ty::Str)) {
                    return self.lower_str_equality(*op, lhs, rhs);
                }
                let l = self.lower_operand(lhs)?;
                let r = self.lower_operand(rhs)?;
                self.lower_binary(*op, lhs, l, r)
            }
            Rvalue::Cast { kind, operand, to } => {
                let value = self.lower_operand(operand)?;
                self.lower_cast(*kind, operand, value, to)
            }
            Rvalue::Aggregate { .. } => Err(CodegenError::backend(
                "aggregate construction reached the scalar rvalue path",
            )),
            Rvalue::Discriminant(place) => Ok(self.lower_discriminant(place)?.into()),
            // The scalar-valued `std::str` byte operations (ADR-0006 Stage B).
            // `slice` yields a `Str` aggregate and is handled by
            // `lower_assign`; reaching it here is a lowering fault.
            Rvalue::StrOp { op, args } => match op {
                StrOp::Len => {
                    let s = args
                        .first()
                        .ok_or_else(|| CodegenError::backend("str len is missing its operand"))?;
                    // The fat pointer's `len` word IS the result (`I64`).
                    let (_ptr, len) = self.str_operand_parts(s)?;
                    Ok(len.into())
                }
                StrOp::ByteAt => self.lower_str_byte_at(args),
                StrOp::Slice => Err(CodegenError::backend(
                    "Str slice reached the scalar rvalue path (it is materialized by \
                     `lower_assign`)",
                )),
            },
            // The scalar-valued ADR-0009 heap reads (`string_len`,
            // `string_byte_at`, `array_len`, `array_get`). The aggregate-
            // producing heap ops are materialized by `lower_assign` and never
            // reach here.
            Rvalue::HeapOp { op, subject, args } => {
                if heap_op_produces_aggregate(*op) {
                    return Err(CodegenError::backend(
                        "an aggregate-producing heap op reached the scalar rvalue path (it is \
                         materialized by `lower_assign`)",
                    ));
                }
                self.lower_heap_op_scalar(*op, subject.as_ref(), args)
            }
            // `Len` applies only to the growable `Array[T]` (a `[T; N]`'s
            // length is lowered as a constant and the MIR verifier rejects
            // `Len` of a fixed-array place), so refusing it entirely stays
            // sound for fixed arrays.
            Rvalue::Len(_) => Err(CodegenError::unsupported(
                "the growable `Array[T]` (and its `Len`) is not lowered by the LLVM backend yet",
            )),
        }
    }

    fn lower_operand(&mut self, operand: &Operand) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        match operand {
            Operand::Copy(place) | Operand::Move(place) => self.read_place(place),
            Operand::Const(constant) => self.lower_const(constant),
        }
    }

    fn lower_const(&mut self, constant: &Const) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        match constant {
            Const::Bool(b) => Ok(self.ctx.i8_type().const_int(u64::from(*b), false).into()),
            Const::Char(c) => Ok(self
                .ctx
                .i32_type()
                .const_int(u64::from(u32::from(*c)), false)
                .into()),
            Const::Int(value, kind) => {
                let ty = int_type(self.ctx, *kind);
                // Reinterpret the mathematical value's low bits at the target
                // width; MIR guarantees it is in range for the kind. `const_int`
                // takes the low 64 bits and (with sign_extend=false) zero-fills
                // above the type width, so the stored bit pattern is exactly the
                // value's two's-complement representation at that width.
                Ok(ty.const_int(*value as u64, false).into())
            }
            // The MIR constant is stored at f64 width, already normalized to
            // its kind's precision, so materializing an `F32` by rounding the
            // f64 payload to f32 is exact.
            Const::Float(value, kind) => Ok(float_type(self.ctx, *kind).const_float(*value).into()),
            Const::Unit => Err(CodegenError::unsupported(
                "a unit constant has no scalar representation to place in a value context",
            )),
            // A `Str` constant is a two-word aggregate, materialized through
            // the aggregate/static-data machinery (`operand_aggregate_address`,
            // `str_operand_parts`); it never has a single-scalar value.
            Const::Str(_) => Err(CodegenError::backend(
                "a `Str` constant reached the scalar constant path (it is materialized via \
                 static data by the aggregate machinery)",
            )),
            // A function value (ADR-0008 Tier 1) is the address of the named
            // top-level function — a pointer-width code pointer. The function's
            // `FunctionValue` *is* a global; its pointer value is the code
            // address, which flows through all scalar machinery (locals, moves
            // are copies since it is `Copy`, params, returns) unchanged.
            Const::Fn(symbol) => {
                let Some(&callee_fn) = self.ids.get(symbol) else {
                    return Err(CodegenError::unsupported(
                        "a function value naming a function outside the lowered program (v0 has \
                         no external functions)",
                    ));
                };
                Ok(callee_fn.as_global_value().as_pointer_value().into())
            }
        }
    }

    // ----- arithmetic -----

    fn lower_unary(
        &mut self,
        op: UnOp,
        operand: &Operand,
        value: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        match op {
            UnOp::Not => {
                // Boolean negation: xor with 1 (values are 0/1).
                let value = value.into_int_value();
                let one = value.get_type().const_int(1, false);
                Ok(self
                    .builder
                    .build_xor(value, one, "not")
                    .map_err(builder_err("emitting boolean not"))?
                    .into())
            }
            UnOp::Neg if matches!(self.operand_ty(operand), Some(Ty::Float(_))) => {
                // Float negation flips the sign bit (IEEE 754, works on NaN
                // too) and never traps — exactly the interpreter's `-v`.
                Ok(self
                    .builder
                    .build_float_neg(value.into_float_value(), "fneg")
                    .map_err(builder_err("emitting float negation"))?
                    .into())
            }
            UnOp::Neg => {
                let value = value.into_int_value();
                let kind = self.operand_int_kind(operand)?;
                // Integer negation traps on MIN (two's complement, §24): -MIN is
                // not representable. Detect it as `value == MIN`.
                let ty = int_type(self.ctx, kind);
                let min = min_const(ty, kind);
                let is_min = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, value, min, "is_min")
                    .map_err(builder_err("comparing against MIN"))?;
                self.guard(is_min, TrapCode::IntegerOverflow)?;
                Ok(self
                    .builder
                    .build_int_neg(value, "neg")
                    .map_err(builder_err("emitting negation"))?
                    .into())
            }
        }
    }

    fn lower_binary(
        &mut self,
        op: BinOp,
        lhs: &Operand,
        l: BasicValueEnum<'ctx>,
        r: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        // Floats take their own IEEE-754 path (never trapping); everything
        // else reduces to integer compares/arithmetic.
        if matches!(self.operand_ty(lhs), Some(Ty::Float(_))) {
            return self.lower_float_binary(op, l.into_float_value(), r.into_float_value());
        }
        let (l, r) = (l.into_int_value(), r.into_int_value());
        // Comparisons on chars/bools/ints all reduce to integer compares; the
        // arithmetic operators are integer-only in the supported subset.
        if let Some(pred) = comparison_pred(op, self.operand_signed(lhs)) {
            let cmp = self
                .builder
                .build_int_compare(pred, l, r, "cmp")
                .map_err(builder_err("emitting a comparison"))?;
            // `icmp` yields an i1; widen to the backend's `Bool` (i8, 0/1) so it
            // shares the scalar representation the rest of the lowering expects.
            return Ok(self
                .builder
                .build_int_z_extend(cmp, self.ctx.i8_type(), "cmp_bool")
                .map_err(builder_err("widening a comparison result"))?
                .into());
        }

        let kind = self.operand_int_kind(lhs)?;
        Ok(match op {
            BinOp::Add => self.checked_arith(kind, l, r, ArithOp::Add)?,
            BinOp::Sub => self.checked_arith(kind, l, r, ArithOp::Sub)?,
            BinOp::Mul => self.checked_arith(kind, l, r, ArithOp::Mul)?,
            BinOp::Div => self.checked_divrem(kind, l, r, true)?,
            BinOp::Rem => self.checked_divrem(kind, l, r, false)?,
            // Comparisons handled above; reaching here is impossible.
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                return Err(CodegenError::backend(
                    "comparison fell through arithmetic path",
                ));
            }
        }
        .into())
    }

    /// Lower a float binary operation: IEEE 754 (round to nearest even), never
    /// trapping — `x / 0.0` is an infinity or NaN, exactly as the interpreter
    /// computes with Rust `f64`/`f32` arithmetic. `Rem` is LLVM's `frem`,
    /// which has C `fmod` semantics (the sign of the dividend) — Rust `%`.
    ///
    /// Comparisons use [`FloatPredicate`]: the four orderings and equality are
    /// **ordered** (`O*`, false when either side is NaN) and inequality is
    /// **unordered-or-unequal** (`UNE`, true on NaN) — exactly Rust's float
    /// comparison semantics, which the interpreter uses. Mirrors the Cranelift
    /// backend's `lower_float_binary` decision for decision.
    fn lower_float_binary(
        &mut self,
        op: BinOp,
        l: inkwell::values::FloatValue<'ctx>,
        r: inkwell::values::FloatValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        if let Some(pred) = float_comparison_pred(op) {
            let cmp = self
                .builder
                .build_float_compare(pred, l, r, "fcmp")
                .map_err(builder_err("emitting a float comparison"))?;
            // `fcmp` yields an i1; widen to the backend's `Bool` (i8, 0/1).
            return Ok(self
                .builder
                .build_int_z_extend(cmp, self.ctx.i8_type(), "fcmp_bool")
                .map_err(builder_err("widening a float comparison result"))?
                .into());
        }
        Ok(match op {
            BinOp::Add => self
                .builder
                .build_float_add(l, r, "fadd")
                .map_err(builder_err("emitting float addition"))?,
            BinOp::Sub => self
                .builder
                .build_float_sub(l, r, "fsub")
                .map_err(builder_err("emitting float subtraction"))?,
            BinOp::Mul => self
                .builder
                .build_float_mul(l, r, "fmul")
                .map_err(builder_err("emitting float multiplication"))?,
            BinOp::Div => self
                .builder
                .build_float_div(l, r, "fdiv")
                .map_err(builder_err("emitting float division"))?,
            BinOp::Rem => self
                .builder
                .build_float_rem(l, r, "frem")
                .map_err(builder_err("emitting float remainder"))?,
            // Comparisons handled above; reaching here is impossible.
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                return Err(CodegenError::backend(
                    "float comparison fell through the arithmetic path",
                ));
            }
        }
        .into())
    }

    /// Lower a trapping add/sub/mul: use LLVM's overflow-checking intrinsic,
    /// trap if the overflow flag is set, and yield the wrapped result. This is
    /// the same behavior the Cranelift backend gets from its overflow-flagged
    /// instructions, so both agree with the interpreter.
    fn checked_arith(
        &mut self,
        kind: IntKind,
        l: IntValue<'ctx>,
        r: IntValue<'ctx>,
        op: ArithOp,
    ) -> Result<IntValue<'ctx>, CodegenError> {
        let signed = is_signed(kind);
        let ty = int_type(self.ctx, kind);
        let intrinsic_name = match (op, signed) {
            (ArithOp::Add, true) => "llvm.sadd.with.overflow",
            (ArithOp::Add, false) => "llvm.uadd.with.overflow",
            (ArithOp::Sub, true) => "llvm.ssub.with.overflow",
            (ArithOp::Sub, false) => "llvm.usub.with.overflow",
            (ArithOp::Mul, true) => "llvm.smul.with.overflow",
            (ArithOp::Mul, false) => "llvm.umul.with.overflow",
        };
        let intrinsic = Intrinsic::find(intrinsic_name).ok_or_else(|| {
            CodegenError::backend(format!("missing LLVM intrinsic {intrinsic_name}"))
        })?;
        let decl = intrinsic
            .get_declaration(self.module, &[ty.into()])
            .ok_or_else(|| {
                CodegenError::backend(format!("declaring LLVM intrinsic {intrinsic_name}"))
            })?;
        let call = self
            .builder
            .build_call(decl, &[l.into(), r.into()], "arith")
            .map_err(builder_err("calling an overflow intrinsic"))?;
        let agg = call.try_as_basic_value().unwrap_basic().into_struct_value();
        let result = self
            .builder
            .build_extract_value(agg, 0, "arith_val")
            .map_err(builder_err("extracting the arithmetic result"))?
            .into_int_value();
        let overflow = self
            .builder
            .build_extract_value(agg, 1, "arith_ovf")
            .map_err(builder_err("extracting the overflow flag"))?
            .into_int_value();
        self.guard(overflow, TrapCode::IntegerOverflow)?;
        Ok(result)
    }

    /// Lower a trapping div (`is_div`) or rem: trap on a zero divisor, and on
    /// the signed `MIN / -1` overflow case (matching the interpreter).
    fn checked_divrem(
        &mut self,
        kind: IntKind,
        l: IntValue<'ctx>,
        r: IntValue<'ctx>,
        is_div: bool,
    ) -> Result<IntValue<'ctx>, CodegenError> {
        let ty = int_type(self.ctx, kind);
        let signed = is_signed(kind);

        // Divisor == 0 → division-by-zero trap.
        let zero = ty.const_zero();
        let is_zero = self
            .builder
            .build_int_compare(IntPredicate::EQ, r, zero, "is_zero")
            .map_err(builder_err("comparing the divisor to zero"))?;
        self.guard(is_zero, TrapCode::DivisionByZero)?;

        if signed {
            // MIN / -1 (and MIN % -1) overflow → integer-overflow trap.
            let min = min_const(ty, kind);
            let neg_one = ty.const_all_ones();
            let l_is_min = self
                .builder
                .build_int_compare(IntPredicate::EQ, l, min, "l_is_min")
                .map_err(builder_err("comparing the dividend to MIN"))?;
            let r_is_neg_one = self
                .builder
                .build_int_compare(IntPredicate::EQ, r, neg_one, "r_is_neg1")
                .map_err(builder_err("comparing the divisor to -1"))?;
            let both = self
                .builder
                .build_and(l_is_min, r_is_neg_one, "min_div_neg1")
                .map_err(builder_err("combining the overflow conditions"))?;
            self.guard(both, TrapCode::IntegerOverflow)?;
            if is_div {
                self.builder
                    .build_int_signed_div(l, r, "sdiv")
                    .map_err(builder_err("emitting signed division"))
            } else {
                self.builder
                    .build_int_signed_rem(l, r, "srem")
                    .map_err(builder_err("emitting signed remainder"))
            }
        } else if is_div {
            self.builder
                .build_int_unsigned_div(l, r, "udiv")
                .map_err(builder_err("emitting unsigned division"))
        } else {
            self.builder
                .build_int_unsigned_rem(l, r, "urem")
                .map_err(builder_err("emitting unsigned remainder"))
        }
    }

    fn lower_cast(
        &mut self,
        kind: CastKind,
        operand: &Operand,
        value: BasicValueEnum<'ctx>,
        to: &Ty,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        match kind {
            CastKind::IntToInt => {
                let Ty::Int(target) = to else {
                    return Err(CodegenError::backend("int-to-int cast to a non-integer"));
                };
                let from = self.operand_int_kind(operand)?;
                Ok(self
                    .resize_int(value.into_int_value(), from, *target)?
                    .into())
            }
            CastKind::IntToFloat => {
                let Ty::Float(target) = to else {
                    return Err(CodegenError::backend("int-to-float cast to a non-float"));
                };
                // Round to nearest even, by the SOURCE's signedness. The
                // interpreter computes `v as f64` and then re-rounds an `F32`
                // (`normalize_float`); this takes the same literal two-step
                // int→f64→f32 path, mirroring the Cranelift backend. (The
                // double rounding is provably innocuous — 53 ≥ 2·24+2 — so it
                // equals a direct int→f32 conversion; the two-step form keeps
                // the correspondence to the interpreter self-evident.)
                let from = self.operand_int_kind(operand)?;
                let value = value.into_int_value();
                let wide = if is_signed(from) {
                    self.builder
                        .build_signed_int_to_float(value, self.ctx.f64_type(), "sitofp")
                        .map_err(builder_err("emitting a signed int-to-float cast"))?
                } else {
                    self.builder
                        .build_unsigned_int_to_float(value, self.ctx.f64_type(), "uitofp")
                        .map_err(builder_err("emitting an unsigned int-to-float cast"))?
                };
                Ok(match target {
                    FloatKind::F64 => wide.into(),
                    FloatKind::F32 => self
                        .builder
                        .build_float_trunc(wide, self.ctx.f32_type(), "fptrunc")
                        .map_err(builder_err("re-rounding an int-to-F32 cast"))?
                        .into(),
                })
            }
            CastKind::FloatToInt => {
                let Ty::Int(target) = to else {
                    return Err(CodegenError::backend("float-to-int cast to a non-integer"));
                };
                // The `llvm.fptosi.sat`/`llvm.fptoui.sat` intrinsics truncate
                // toward zero, saturate to the TARGET's range, and map NaN to
                // 0 — exactly the interpreter's `saturating_float_to_int` and
                // the Cranelift backend's fcvt_to_{s,u}int_sat. Never traps
                // (unlike plain `fptosi`, which is poison out of range).
                self.saturating_float_to_int_cast(value.into_float_value(), *target)
            }
            CastKind::FloatToFloat => {
                let Ty::Float(target) = to else {
                    return Err(CodegenError::backend("float-to-float cast to a non-float"));
                };
                let Some(Ty::Float(from)) = self.operand_ty(operand) else {
                    return Err(CodegenError::backend("float-to-float cast of a non-float"));
                };
                let value = value.into_float_value();
                // IEEE 754 conversion: exact when widening, round to nearest
                // even when narrowing — the interpreter's `normalize_float`.
                Ok(match (from, target) {
                    (FloatKind::F32, FloatKind::F64) => self
                        .builder
                        .build_float_ext(value, self.ctx.f64_type(), "fpext")
                        .map_err(builder_err("emitting a float widening cast"))?
                        .into(),
                    (FloatKind::F64, FloatKind::F32) => self
                        .builder
                        .build_float_trunc(value, self.ctx.f32_type(), "fptrunc")
                        .map_err(builder_err("emitting a float narrowing cast"))?
                        .into(),
                    (FloatKind::F32, FloatKind::F32) | (FloatKind::F64, FloatKind::F64) => {
                        value.into()
                    }
                })
            }
        }
    }

    /// Lower a saturating float→int cast through the `llvm.fptosi.sat` /
    /// `llvm.fptoui.sat` intrinsic (by the TARGET's signedness), overloaded on
    /// the concrete (int, float) type pair.
    fn saturating_float_to_int_cast(
        &mut self,
        value: inkwell::values::FloatValue<'ctx>,
        target: IntKind,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let intrinsic_name = if is_signed(target) {
            "llvm.fptosi.sat"
        } else {
            "llvm.fptoui.sat"
        };
        let intrinsic = Intrinsic::find(intrinsic_name).ok_or_else(|| {
            CodegenError::backend(format!("missing LLVM intrinsic {intrinsic_name}"))
        })?;
        let int_ty = int_type(self.ctx, target);
        let float_ty = value.get_type();
        let decl = intrinsic
            .get_declaration(self.module, &[int_ty.into(), float_ty.into()])
            .ok_or_else(|| {
                CodegenError::backend(format!("declaring LLVM intrinsic {intrinsic_name}"))
            })?;
        let call = self
            .builder
            .build_call(decl, &[value.into()], "fptoint_sat")
            .map_err(builder_err("calling a saturating float-to-int intrinsic"))?;
        Ok(call.try_as_basic_value().unwrap_basic())
    }

    /// Resize an integer from `from` to `target` width with two's-complement
    /// wrapping (truncate when narrowing; sign/zero-extend from the source's
    /// signedness when widening) — matching the interpreter's `wrap_int` and the
    /// Cranelift backend.
    fn resize_int(
        &mut self,
        value: IntValue<'ctx>,
        from: IntKind,
        target: IntKind,
    ) -> Result<IntValue<'ctx>, CodegenError> {
        let from_bits = int_width_bits(from);
        let to_bits = int_width_bits(target);
        let to_ty = int_type(self.ctx, target);
        if to_bits == from_bits {
            Ok(value)
        } else if to_bits < from_bits {
            self.builder
                .build_int_truncate(value, to_ty, "trunc")
                .map_err(builder_err("truncating an integer cast"))
        } else if is_signed(from) {
            self.builder
                .build_int_s_extend(value, to_ty, "sext")
                .map_err(builder_err("sign-extending an integer cast"))
        } else {
            self.builder
                .build_int_z_extend(value, to_ty, "zext")
                .map_err(builder_err("zero-extending an integer cast"))
        }
    }

    // ----- places -----

    /// Read the scalar value of `place`. An empty projection on a scalar local
    /// loads its alloca; a non-empty projection resolves to a leaf byte address
    /// (a scalar leaf in Stage 1) and loads it.
    fn read_place(&mut self, place: &Place) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        if place.projection.is_empty() {
            let index = place.local.0 as usize;
            return match &self.kinds[index] {
                LocalKind::Scalar(ptr, ty) => self
                    .builder
                    .build_load(*ty, *ptr, "load")
                    .map_err(builder_err("loading a local")),
                LocalKind::Borrowed { ty, .. } => {
                    // A scalar borrowed parameter: load through the caller's
                    // pointer. (A whole-aggregate read goes through the
                    // aggregate path, exactly as for `Aggregate` locals.)
                    let ty = ty.clone();
                    let scalar =
                        require_scalar(self.ctx, &ty, &self.function.name).map_err(|_| {
                            CodegenError::backend(
                                "reading a whole aggregate as a scalar (should go through the \
                                 aggregate path)",
                            )
                        })?;
                    let addr = self.borrowed_addr(index)?;
                    self.builder
                        .build_load(scalar, addr, "brw_load")
                        .map_err(builder_err("loading through a borrow pointer"))
                }
                LocalKind::Unit => Err(CodegenError::backend(
                    "reading a scalar value from a unit local",
                )),
                LocalKind::Aggregate { .. } => Err(CodegenError::backend(
                    "reading a whole aggregate as a scalar (should go through the aggregate path)",
                )),
            };
        }
        let (addr, leaf_ty) = self.place_address(place)?;
        let scalar = require_scalar(self.ctx, &leaf_ty, &self.function.name)?;
        self.builder
            .build_load(scalar, addr, "fload")
            .map_err(builder_err("loading a projected field"))
    }

    /// Write the scalar `value` to `place`. An empty projection on a scalar local
    /// stores its alloca; a non-empty projection resolves to a leaf byte address
    /// (a scalar leaf in Stage 1) and stores there. A unit destination carries no
    /// value.
    fn write_place(
        &mut self,
        place: &Place,
        value: BasicValueEnum<'ctx>,
    ) -> Result<(), CodegenError> {
        if place.projection.is_empty() {
            let index = place.local.0 as usize;
            return match &self.kinds[index] {
                LocalKind::Scalar(ptr, _) => {
                    self.builder
                        .build_store(*ptr, value)
                        .map_err(builder_err("storing a local"))?;
                    Ok(())
                }
                LocalKind::Borrowed { .. } => {
                    // A `mut` scalar parameter: store through the caller's
                    // pointer, so the write is visible to the caller with no
                    // copy-back.
                    let addr = self.borrowed_addr(index)?;
                    self.builder
                        .build_store(addr, value)
                        .map_err(builder_err("storing through a borrow pointer"))?;
                    Ok(())
                }
                LocalKind::Unit => Ok(()),
                LocalKind::Aggregate { .. } => Err(CodegenError::backend(
                    "writing a scalar into a whole aggregate local (should go through the \
                     aggregate path)",
                )),
            };
        }
        let (addr, _leaf_ty) = self.place_address(place)?;
        self.builder
            .build_store(addr, value)
            .map_err(builder_err("storing a projected field"))?;
        Ok(())
    }

    /// The shared address walk: resolve `place` to a `(byte address, leaf type)`
    /// pair by GEP-ing each projection's ABI offset from the root aggregate's
    /// pointer. Byte-identical in structure to the Cranelift backend's
    /// `place_address`. A `Field`/`VariantField` step advances by a constant
    /// ABI offset; an `Index` step advances by the runtime index value times
    /// the element stride — **unchecked**, because MIR asserts the bounds
    /// (`Assert { IndexOutOfBounds }`) before every `Index` use, exactly like
    /// the interpreter's post-check access.
    fn place_address(&mut self, place: &Place) -> Result<(PointerValue<'ctx>, Ty), CodegenError> {
        let local = place.local.0 as usize;
        let (mut addr, mut cur_ty) = match &self.kinds[local] {
            LocalKind::Aggregate { ptr, ty } => (*ptr, ty.clone()),
            // A borrowed parameter's incoming pointer is the base address of
            // the caller's place; projections advance from it exactly as from
            // a local alloca.
            LocalKind::Borrowed { ty, .. } => {
                let ty = ty.clone();
                (self.borrowed_addr(local)?, ty)
            }
            // A zero-sized aggregate root (e.g. `[T; 0]`) classifies as unit
            // storage yet may still be projected in MIR — only in code the
            // preceding bounds `Assert` makes unreachable at runtime (any index
            // into a zero-length array traps first). Give it a null base; no
            // load/store computed from it can ever execute.
            LocalKind::Unit if !place.projection.is_empty() => (
                self.ptr_ty.const_null(),
                self.function.locals[local].ty.clone(),
            ),
            _ => {
                return Err(CodegenError::backend("projection on a non-aggregate local"));
            }
        };

        for step in &place.projection {
            match step {
                Projection::Field(index) => {
                    let offsets = struct_field_offsets(&cur_ty, self.types)
                        .map_err(|error| self.layout_error(error))?;
                    let i = *index as usize;
                    let offset = *offsets.get(i).ok_or_else(|| {
                        CodegenError::backend("struct/tuple field index out of range")
                    })?;
                    cur_ty = self.field_type(&cur_ty, i)?;
                    addr = self.byte_gep(addr, offset)?;
                }
                Projection::VariantField { variant, field } => {
                    let offsets = variant_field_offsets(&cur_ty, *variant as usize, self.types)
                        .map_err(|error| self.layout_error(error))?;
                    let f = *field as usize;
                    let offset = *offsets
                        .get(f)
                        .ok_or_else(|| CodegenError::backend("variant field index out of range"))?;
                    cur_ty = self.variant_field_type(&cur_ty, *variant as usize, f)?;
                    addr = self.byte_gep(addr, offset)?;
                }
                Projection::Index(index_local) => {
                    // Only the fixed `[T; N]` is indexable natively; element
                    // `i` lives at `i × stride(T)` (the ABI's inline layout).
                    let element = match &cur_ty {
                        Ty::FixedArray(element, _) => (**element).clone(),
                        _ => {
                            return Err(CodegenError::unsupported(
                                "indexing the growable `Array[T]` is not lowered by the LLVM \
                                 backend (only the fixed `[T; N]` is)",
                            ));
                        }
                    };
                    let stride = self.layout(&element)?.stride();
                    // The index is a `Usize` scalar local, pre-asserted in
                    // bounds by the MIR before this use.
                    let index_value = match &self.kinds[index_local.0 as usize] {
                        LocalKind::Scalar(ptr, scalar_ty) => self
                            .builder
                            .build_load(*scalar_ty, *ptr, "idx")
                            .map_err(builder_err("loading an array index"))?
                            .into_int_value(),
                        _ => {
                            return Err(CodegenError::backend("array index local is not a scalar"));
                        }
                    };
                    addr = self.dynamic_byte_gep(addr, index_value, stride)?;
                    cur_ty = element;
                }
            }
        }
        Ok((addr, cur_ty))
    }

    /// Advance a byte pointer by `offset` bytes. Computed as
    /// `inttoptr(ptrtoint(base) + offset)` — pointer arithmetic without an
    /// `unsafe` GEP (`unsafe_code` is forbidden workspace-wide). The offset is a
    /// byte displacement from the ABI, so no LLVM struct type is needed; the
    /// result addresses the same bytes an i8-GEP would.
    fn byte_gep(
        &mut self,
        base: PointerValue<'ctx>,
        offset: u64,
    ) -> Result<PointerValue<'ctx>, CodegenError> {
        if offset == 0 {
            return Ok(base);
        }
        // Pointer-width integer (POINTER_SIZE = 8 on the supported hosts).
        let usize_ty = self.ctx.i64_type();
        let base_int = self
            .builder
            .build_ptr_to_int(base, usize_ty, "base_int")
            .map_err(builder_err("converting a base pointer to an integer"))?;
        let off = usize_ty.const_int(offset, false);
        let sum = self
            .builder
            .build_int_add(base_int, off, "field_int")
            .map_err(builder_err("adding a field offset"))?;
        self.builder
            .build_int_to_ptr(sum, self.ptr_ty, "field")
            .map_err(builder_err("converting a field integer back to a pointer"))
    }

    /// Advance a byte pointer by a **runtime** element index times a constant
    /// `stride`. The dynamic counterpart of [`Self::byte_gep`], used only by
    /// `Projection::Index`: `inttoptr(ptrtoint(base) + index × stride)`.
    fn dynamic_byte_gep(
        &mut self,
        base: PointerValue<'ctx>,
        index: IntValue<'ctx>,
        stride: u64,
    ) -> Result<PointerValue<'ctx>, CodegenError> {
        let usize_ty = self.ctx.i64_type();
        let base_int = self
            .builder
            .build_ptr_to_int(base, usize_ty, "base_int")
            .map_err(builder_err("converting a base pointer to an integer"))?;
        let stride = usize_ty.const_int(stride, false);
        let scaled = self
            .builder
            .build_int_mul(index, stride, "elem_off")
            .map_err(builder_err("scaling an array index by the element stride"))?;
        let sum = self
            .builder
            .build_int_add(base_int, scaled, "elem_int")
            .map_err(builder_err("adding an element offset"))?;
        self.builder
            .build_int_to_ptr(sum, self.ptr_ty, "elem")
            .map_err(builder_err(
                "converting an element integer back to a pointer",
            ))
    }

    /// Materialize a `Rvalue::Aggregate` in place into `dest`'s byte storage.
    /// Byte-identical to the Cranelift backend's `lower_aggregate`. A
    /// fixed-array aggregate takes its own path (element `i` at `i × stride`).
    fn lower_aggregate(
        &mut self,
        dest: &Place,
        kind: &tuo_mir::AggregateKind,
        fields: &[Operand],
    ) -> Result<(), CodegenError> {
        let (ty, variant) = match kind {
            tuo_mir::AggregateKind::Adt { ty, variant } => (ty.clone(), *variant),
            tuo_mir::AggregateKind::Range => {
                return Err(CodegenError::unsupported(
                    "range construction is not lowered by the LLVM backend (Stage 2)",
                ));
            }
            tuo_mir::AggregateKind::Array { element, len } => {
                return self.lower_array_aggregate(dest, element, *len, fields);
            }
        };

        let base = self.aggregate_dest_address(dest)?;

        // Enum-like: store the u32 tag at offset 0. Struct/tuple: no tag.
        let enum_like = matches!(ty, Ty::Enum(..) | Ty::Option(_) | Ty::Result(..));
        if enum_like {
            let tag = self.ctx.i32_type().const_int(u64::from(variant), false);
            self.builder
                .build_store(base, tag)
                .map_err(builder_err("storing an enum discriminant"))?;
        }

        let offsets = if enum_like {
            variant_field_offsets(&ty, variant as usize, self.types)
                .map_err(|error| self.layout_error(error))?
        } else {
            struct_field_offsets(&ty, self.types).map_err(|error| self.layout_error(error))?
        };
        if fields.len() != offsets.len() {
            return Err(CodegenError::backend(
                "aggregate field count does not match the ABI offset count",
            ));
        }

        for (i, operand) in fields.iter().enumerate() {
            if self.operand_is_unit(operand) {
                continue;
            }
            let field_addr = self.byte_gep(base, offsets[i])?;
            if self.operand_is_aggregate(operand) {
                let field_ty = self
                    .operand_ty(operand)
                    .ok_or_else(|| CodegenError::backend("aggregate field has no static type"))?;
                let layout = self.layout(&field_ty)?;
                let src = self.operand_aggregate_address(operand)?;
                self.emit_memcpy(field_addr, src, layout)?;
            } else {
                let value = self.lower_operand(operand)?;
                self.builder
                    .build_store(field_addr, value)
                    .map_err(builder_err("storing an aggregate field"))?;
            }
        }
        Ok(())
    }

    /// Materialize an `AggregateKind::Array { element, len }` — a fixed-size
    /// array value `[element; len]` — into `dest`'s byte storage: the operands
    /// are the `len` elements in index order, and element `i` is stored at byte
    /// offset `i × stride(element)` per the runtime ABI's inline layout
    /// (`abi::layout_of`; no header, no allocation). A scalar element is a
    /// scalar store; a nested-aggregate element is a whole-aggregate memcpy —
    /// mirroring `lower_aggregate`'s field dispatch. Byte-identical to the
    /// Cranelift backend's `lower_array_aggregate`.
    fn lower_array_aggregate(
        &mut self,
        dest: &Place,
        element: &Ty,
        len: u64,
        fields: &[Operand],
    ) -> Result<(), CodegenError> {
        if fields.len() as u64 != len {
            return Err(CodegenError::backend(
                "fixed-array operand count does not match its length",
            ));
        }
        let elem_layout = self.layout(element)?;
        let stride = elem_layout.stride();
        // A zero-sized array (`len == 0`, or a zero-sized element type) has no
        // bytes to write and its destination classifies as unit storage — the
        // construction is a no-op (element operands are `Copy`/`Move`/`Const`
        // places, which carry no side effect to preserve).
        if len == 0 || stride == 0 {
            return Ok(());
        }
        let base = self.aggregate_dest_address(dest)?;
        for (i, operand) in fields.iter().enumerate() {
            let offset = (i as u64)
                .checked_mul(stride)
                .ok_or_else(|| CodegenError::backend("array element offset overflows"))?;
            let element_addr = self.byte_gep(base, offset)?;
            if self.operand_is_aggregate(operand) {
                let src = self.operand_aggregate_address(operand)?;
                self.emit_memcpy(element_addr, src, elem_layout)?;
            } else {
                let value = self.lower_operand(operand)?;
                self.builder
                    .build_store(element_addr, value)
                    .map_err(builder_err("storing an array element"))?;
            }
        }
        Ok(())
    }

    /// The base address to materialize an aggregate into, for an `Assign` whose
    /// destination is an aggregate.
    fn aggregate_dest_address(&mut self, dest: &Place) -> Result<PointerValue<'ctx>, CodegenError> {
        if dest.projection.is_empty() {
            return match &self.kinds[dest.local.0 as usize] {
                LocalKind::Aggregate { ptr, .. } => Ok(*ptr),
                // A whole-aggregate write to a `mut` borrowed parameter goes
                // straight through the caller's pointer (no copy-back).
                LocalKind::Borrowed { .. } => self.borrowed_addr(dest.local.0 as usize),
                _ => Err(CodegenError::backend(
                    "aggregate rvalue assigned to a non-aggregate local",
                )),
            };
        }
        let (addr, _leaf) = self.place_address(dest)?;
        Ok(addr)
    }

    /// Lower `Rvalue::Discriminant`: load the u32 tag at offset 0 and zero-extend
    /// to a `Usize` (i64), matching the interpreter's `Value::Int(discr, Usize)`.
    fn lower_discriminant(&mut self, place: &Place) -> Result<IntValue<'ctx>, CodegenError> {
        let base = if place.projection.is_empty() {
            match &self.kinds[place.local.0 as usize] {
                LocalKind::Aggregate { ptr, .. } => *ptr,
                // The discriminant of a borrowed enum parameter reads through
                // the caller's pointer.
                LocalKind::Borrowed { .. } => self.borrowed_addr(place.local.0 as usize)?,
                _ => {
                    return Err(CodegenError::backend(
                        "discriminant of a non-aggregate local",
                    ));
                }
            }
        } else {
            let (addr, _leaf) = self.place_address(place)?;
            addr
        };
        let tag = self
            .builder
            .build_load(self.ctx.i32_type(), base, "discr")
            .map_err(builder_err("loading a discriminant"))?
            .into_int_value();
        self.builder
            .build_int_z_extend(tag, self.ctx.i64_type(), "discr_usize")
            .map_err(builder_err("zero-extending a discriminant"))
    }

    // ----- strings & effects (ADR-0006 Stage B) -----

    /// The `{ptr, len}` pair of a `Str` literal: the address of its read-only
    /// global (private, unnamed-addr, constant — deduplicated per module,
    /// keyed by the emitted contents) and its byte length as an `i64`
    /// constant. An empty literal still gets a real one-byte global so its
    /// address is a fixed, non-null, aligned pointer — never dereferenced,
    /// because its `len` is 0.
    fn str_const_parts(
        &mut self,
        text: &str,
    ) -> Result<(PointerValue<'ctx>, IntValue<'ctx>), CodegenError> {
        let contents: Vec<u8> = if text.is_empty() {
            vec![0]
        } else {
            text.as_bytes().to_vec()
        };
        let ptr = if let Some(global) = self.str_globals.get(&contents) {
            global.as_pointer_value()
        } else {
            let count = u32::try_from(contents.len())
                .map_err(|_| CodegenError::backend("string literal longer than u32::MAX bytes"))?;
            let ty = self.ctx.i8_type().array_type(count);
            let global = self.module.add_global(ty, None, "tuo_str");
            global.set_initializer(&self.ctx.const_string(&contents, false));
            global.set_constant(true);
            global.set_linkage(Linkage::Private);
            global.set_unnamed_addr(true);
            global.set_alignment(1);
            self.str_globals.insert(contents, global);
            global.as_pointer_value()
        };
        let len = u64::try_from(text.len())
            .map_err(|_| CodegenError::backend("string literal longer than u64::MAX bytes"))?;
        let len = self.ctx.i64_type().const_int(len, false);
        Ok((ptr, len))
    }

    /// The `{ptr, len}` fat-pointer fields of a `Str`-typed operand: a literal
    /// yields its global's address and constant length directly; a place loads
    /// the two words from its aggregate storage (the shared address
    /// machinery). Mirrors the Cranelift backend's `str_operand_parts`.
    fn str_operand_parts(
        &mut self,
        operand: &Operand,
    ) -> Result<(PointerValue<'ctx>, IntValue<'ctx>), CodegenError> {
        if let Operand::Const(Const::Str(text)) = operand {
            let text = text.clone();
            return self.str_const_parts(&text);
        }
        let addr = self.operand_aggregate_address(operand)?;
        let ptr = self
            .builder
            .build_load(self.ptr_ty, addr, "str_ptr")
            .map_err(builder_err("loading a Str data pointer"))?
            .into_pointer_value();
        let len_addr = self.byte_gep(addr, STR_LEN_OFFSET)?;
        let len = self
            .builder
            .build_load(self.ctx.i64_type(), len_addr, "str_len")
            .map_err(builder_err("loading a Str length"))?
            .into_int_value();
        Ok((ptr, len))
    }

    /// Lower `Eq`/`Ne` on `Str` operands: byte-wise equality, exactly as the
    /// interpreter compares its byte buffers — lengths equal AND bytes equal.
    /// The byte compare (the C library's `memcmp`, imported like the trap
    /// symbol) runs only when the lengths match, so comparing `len_a` bytes of
    /// both sides is always in bounds; a zero-length pair is equal without
    /// dereferencing (`memcmp` with a zero count reads nothing). The type
    /// checker forbids ordering on `Str`, so no other operator can reach here.
    /// Mirrors the Cranelift backend's `lower_str_equality` decision for
    /// decision.
    fn lower_str_equality(
        &mut self,
        op: BinOp,
        lhs: &Operand,
        rhs: &Operand,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        if !matches!(op, BinOp::Eq | BinOp::Ne) {
            return Err(CodegenError::backend(
                "a non-equality comparison on `Str` operands (the type checker forbids \
                 ordering on `Str`)",
            ));
        }
        let (ptr_a, len_a) = self.str_operand_parts(lhs)?;
        let (ptr_b, len_b) = self.str_operand_parts(rhs)?;

        let lens_eq = self
            .builder
            .build_int_compare(IntPredicate::EQ, len_a, len_b, "str_len_eq")
            .map_err(builder_err("comparing Str lengths"))?;
        let start_block = self
            .builder
            .get_insert_block()
            .ok_or_else(|| CodegenError::backend("Str equality lowered outside a block"))?;
        let cmp_block = self.ctx.append_basic_block(self.value, "str_cmp");
        let join_block = self.ctx.append_basic_block(self.value, "str_eq_join");
        self.builder
            .build_conditional_branch(lens_eq, cmp_block, join_block)
            .map_err(builder_err("branching on the Str length compare"))?;

        self.builder.position_at_end(cmp_block);
        let memcmp = self.memcmp_function();
        let call = self
            .builder
            .build_call(
                memcmp,
                &[ptr_a.into(), ptr_b.into(), len_a.into()],
                "memcmp",
            )
            .map_err(builder_err("calling memcmp"))?;
        let verdict = call.try_as_basic_value().unwrap_basic().into_int_value();
        let zero = self.ctx.i32_type().const_zero();
        let bytes_eq = self
            .builder
            .build_int_compare(IntPredicate::EQ, verdict, zero, "str_bytes_eq")
            .map_err(builder_err("comparing the memcmp verdict to zero"))?;
        self.builder
            .build_unconditional_branch(join_block)
            .map_err(builder_err("joining the Str equality branches"))?;

        self.builder.position_at_end(join_block);
        let phi = self
            .builder
            .build_phi(self.ctx.bool_type(), "str_eq")
            .map_err(builder_err("merging the Str equality verdict"))?;
        let false_i1 = self.ctx.bool_type().const_zero();
        phi.add_incoming(&[(&false_i1, start_block), (&bytes_eq, cmp_block)]);
        let eq_i1 = phi.as_basic_value().into_int_value();
        // Widen to the backend's `Bool` (i8, 0/1), then invert for `Ne`.
        let eq = self
            .builder
            .build_int_z_extend(eq_i1, self.ctx.i8_type(), "str_eq_bool")
            .map_err(builder_err("widening the Str equality verdict"))?;
        Ok(match op {
            BinOp::Ne => {
                let one = self.ctx.i8_type().const_int(1, false);
                self.builder
                    .build_xor(eq, one, "str_ne")
                    .map_err(builder_err("inverting the Str equality verdict"))?
                    .into()
            }
            _ => eq.into(),
        })
    }

    /// Lower `byte_at(s, index)`: a deterministic `IndexOutOfBounds` trap
    /// unless `0 <= index < len(s)` (`specification/mir.md` §5.6, exactly the
    /// interpreter's `eval_str_op`), then load the byte at `ptr + index` and
    /// zero-extend it to the `i64` the destination expects. One unsigned
    /// compare implements both bounds: a negative index reinterprets as a
    /// huge unsigned value, and `len` is never negative. Mirrors the
    /// Cranelift backend's `lower_str_byte_at`.
    fn lower_str_byte_at(
        &mut self,
        args: &[Operand],
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let [s, index] = args else {
            return Err(CodegenError::backend("byte_at expects exactly 2 operands"));
        };
        let (ptr, len) = self.str_operand_parts(s)?;
        let index = self.lower_operand(index)?.into_int_value();
        let oob = self
            .builder
            .build_int_compare(IntPredicate::UGE, index, len, "str_oob")
            .map_err(builder_err("bounds-checking a Str byte index"))?;
        self.guard(oob, TrapCode::IndexOutOfBounds)?;
        let addr = self.dynamic_byte_gep(ptr, index, 1)?;
        let byte = self
            .builder
            .build_load(self.ctx.i8_type(), addr, "str_byte")
            .map_err(builder_err("loading a Str byte"))?
            .into_int_value();
        Ok(self
            .builder
            .build_int_z_extend(byte, self.ctx.i64_type(), "str_byte_i64")
            .map_err(builder_err("widening a Str byte"))?
            .into())
    }

    /// Lower `dest = slice(s, start, end)`: a deterministic `IndexOutOfBounds`
    /// trap unless `0 <= start <= end <= len(s)` (`specification/mir.md`
    /// §5.6), then write the derived fat pointer `{ptr + start, end - start}`
    /// into the destination's aggregate storage. Two unsigned compares cover
    /// all four bounds: `start >u end` catches `start > end` and a negative
    /// `start`; `end >u len` catches `end > len` and a negative `end`. The
    /// source's words are loaded before the destination is written, so
    /// `s = slice(s, ..)` re-slicing in place is sound. Mirrors the Cranelift
    /// backend's `lower_str_slice`.
    fn lower_str_slice(&mut self, dest: &Place, args: &[Operand]) -> Result<(), CodegenError> {
        let [s, start, end] = args else {
            return Err(CodegenError::backend("slice expects exactly 3 operands"));
        };
        let (ptr, len) = self.str_operand_parts(s)?;
        let start = self.lower_operand(start)?.into_int_value();
        let end = self.lower_operand(end)?.into_int_value();
        let bad_order = self
            .builder
            .build_int_compare(IntPredicate::UGT, start, end, "str_bad_order")
            .map_err(builder_err("comparing the slice bounds to each other"))?;
        self.guard(bad_order, TrapCode::IndexOutOfBounds)?;
        let bad_end = self
            .builder
            .build_int_compare(IntPredicate::UGT, end, len, "str_bad_end")
            .map_err(builder_err("comparing the slice end to the length"))?;
        self.guard(bad_end, TrapCode::IndexOutOfBounds)?;

        let new_ptr = self.dynamic_byte_gep(ptr, start, 1)?;
        let new_len = self
            .builder
            .build_int_sub(end, start, "str_slice_len")
            .map_err(builder_err("computing the slice length"))?;
        let base = self.aggregate_dest_address(dest)?;
        self.builder
            .build_store(base, new_ptr)
            .map_err(builder_err("storing the slice data pointer"))?;
        let len_addr = self.byte_gep(base, STR_LEN_OFFSET)?;
        self.builder
            .build_store(len_addr, new_len)
            .map_err(builder_err("storing the slice length"))?;
        Ok(())
    }

    /// Lower one host effect (`Statement::Effect`, ADR-0006 Stage B): a direct
    /// call to the matching [`tuo_runtime::effect`] symbol, with the `i64`
    /// result stored into `dest`. `exit` never returns, so after its call the
    /// block is terminated with `unreachable` (the same shape the trap path
    /// uses) and the (dead) remainder of the MIR block is lowered into a fresh
    /// unreachable block — LLVM's verifier does not constrain unreachable
    /// code, and none of it ever executes. Mirrors the Cranelift backend's
    /// `lower_effect`.
    fn lower_effect(
        &mut self,
        op: EffectOp,
        args: &[Arg],
        dest: &Place,
    ) -> Result<(), CodegenError> {
        // Every ADR-0006 effect argument is a by-value operand; the borrow
        // form only appears with `WriteString`, which is refused below.
        let value_arg = |arg: &Arg| -> Result<Operand, CodegenError> {
            match arg {
                Arg::Value(operand) => Ok(operand.clone()),
                Arg::Borrow(_) | Arg::BorrowMut(_) => Err(CodegenError::backend(
                    "an ADR-0006 effect argument must be a by-value operand",
                )),
            }
        };
        match op {
            EffectOp::Write => {
                let [fd, text] = args else {
                    return Err(CodegenError::backend("write expects exactly 2 arguments"));
                };
                let fd = self.lower_operand(&value_arg(fd)?)?;
                let (ptr, len) = self.str_operand_parts(&value_arg(text)?)?;
                let call = self
                    .builder
                    .build_call(
                        self.effect_function(op),
                        &[fd.into(), ptr.into(), len.into()],
                        "rt_write",
                    )
                    .map_err(builder_err("calling the write effect"))?;
                let result = call.try_as_basic_value().unwrap_basic();
                self.write_place(dest, result)
            }
            EffectOp::ReadByte => {
                let [fd] = args else {
                    return Err(CodegenError::backend(
                        "read_byte expects exactly 1 argument",
                    ));
                };
                let fd = self.lower_operand(&value_arg(fd)?)?;
                let call = self
                    .builder
                    .build_call(self.effect_function(op), &[fd.into()], "rt_read_byte")
                    .map_err(builder_err("calling the read_byte effect"))?;
                let result = call.try_as_basic_value().unwrap_basic();
                self.write_place(dest, result)
            }
            // `write_string(fd, in s: String)` (ADR-0009 Stage B): load the
            // `{ptr, len}` from the borrowed `String` header and call the same
            // `tuo_rt_write` symbol `write` uses.
            EffectOp::WriteString => {
                let [fd, s] = args else {
                    return Err(CodegenError::backend(
                        "write_string expects exactly 2 arguments",
                    ));
                };
                let fd = self.lower_operand(&value_arg(fd)?)?;
                let header = match s {
                    Arg::Borrow(place) => self.borrow_address(place)?,
                    Arg::Value(_) | Arg::BorrowMut(_) => {
                        return Err(CodegenError::backend(
                            "write_string's String argument must be an `in` borrow",
                        ));
                    }
                };
                let ptr = self
                    .builder
                    .build_load(self.ptr_ty, header, "ws_ptr")
                    .map_err(builder_err("loading the String data pointer"))?;
                let len_addr = self.byte_gep(header, HDR_LEN_OFFSET)?;
                let len = self
                    .builder
                    .build_load(self.ctx.i64_type(), len_addr, "ws_len")
                    .map_err(builder_err("loading the String length"))?;
                let call = self
                    .builder
                    .build_call(
                        self.effect_function(EffectOp::Write),
                        &[fd.into(), ptr.into(), len.into()],
                        "rt_write_string",
                    )
                    .map_err(builder_err("calling write_string"))?;
                let result = call.try_as_basic_value().unwrap_basic();
                self.write_place(dest, result)
            }
            EffectOp::Exit => {
                let [code] = args else {
                    return Err(CodegenError::backend("exit expects exactly 1 argument"));
                };
                let code = self.lower_operand(&value_arg(code)?)?;
                self.builder
                    .build_call(self.effect_function(op), &[code.into()], "")
                    .map_err(builder_err("calling the exit effect"))?;
                // The call never returns; `dest` is never observably written.
                self.builder
                    .build_unreachable()
                    .map_err(builder_err("emitting unreachable after exit"))?;
                let dead = self.ctx.append_basic_block(self.value, "after_exit");
                self.builder.position_at_end(dead);
                Ok(())
            }
            // `par_map(f, workers, borrow tasks)` (ADR-0007): read the tasks
            // array's `{ptr, len}` from the borrowed header, pass the code
            // pointer, count, and worker count to the runtime's fork-join
            // shim, and let it write the fresh `Array[Int]` header straight
            // into the destination.
            EffectOp::ParMap => {
                let [f, workers, tasks] = args else {
                    return Err(CodegenError::backend("par_map expects exactly 3 arguments"));
                };
                let f = self.lower_operand(&value_arg(f)?)?;
                let workers = self.lower_operand(&value_arg(workers)?)?;
                let header = match tasks {
                    Arg::Borrow(place) => self.borrow_address(place)?,
                    Arg::Value(_) | Arg::BorrowMut(_) => {
                        return Err(CodegenError::backend(
                            "par_map's tasks argument must be an `in` borrow",
                        ));
                    }
                };
                let ptr = self
                    .builder
                    .build_load(self.ptr_ty, header, "pm_tasks_ptr")
                    .map_err(builder_err("loading the tasks data pointer"))?;
                let len_addr = self.byte_gep(header, HDR_LEN_OFFSET)?;
                let len = self
                    .builder
                    .build_load(self.ctx.i64_type(), len_addr, "pm_tasks_len")
                    .map_err(builder_err("loading the tasks length"))?;
                let dest_addr = self.aggregate_dest_address(dest)?;
                self.builder
                    .build_call(
                        self.effect_function(op),
                        &[
                            f.into(),
                            ptr.into(),
                            len.into(),
                            workers.into(),
                            dest_addr.into(),
                        ],
                        "",
                    )
                    .map_err(builder_err("calling the par_map effect"))?;
                Ok(())
            }
        }
    }

    /// Declare (once per module, on demand) and return the runtime effect
    /// symbol for `op`, mirroring how the trap symbol is imported. The CLI
    /// links the effect C shim into every built binary, so the symbols
    /// resolve.
    fn effect_function(&self, op: EffectOp) -> FunctionValue<'ctx> {
        let i64_ty = self.ctx.i64_type();
        let (name, fn_type) = match op {
            EffectOp::Write => (
                effect::WRITE_SYMBOL,
                i64_ty.fn_type(&[i64_ty.into(), self.ptr_ty.into(), i64_ty.into()], false),
            ),
            EffectOp::ReadByte => (
                effect::READ_BYTE_SYMBOL,
                i64_ty.fn_type(&[i64_ty.into()], false),
            ),
            EffectOp::Exit => (
                effect::EXIT_SYMBOL,
                self.ctx.void_type().fn_type(&[i64_ty.into()], false),
            ),
            // `write_string` writes bytes through the same `tuo_rt_write`
            // symbol as `write` (its lowering passes `EffectOp::Write` here);
            // this arm maps to the write symbol so it is never a landmine.
            EffectOp::WriteString => (
                effect::WRITE_SYMBOL,
                i64_ty.fn_type(&[i64_ty.into(), self.ptr_ty.into(), i64_ty.into()], false),
            ),
            // `par_map(f, tasks_ptr, n, workers, out_hdr)` — void; the result
            // array header is written through the out pointer (ADR-0007).
            EffectOp::ParMap => (
                effect::PAR_MAP_SYMBOL,
                self.ctx.void_type().fn_type(
                    &[
                        self.ptr_ty.into(),
                        self.ptr_ty.into(),
                        i64_ty.into(),
                        i64_ty.into(),
                        self.ptr_ty.into(),
                    ],
                    false,
                ),
            ),
        };
        if let Some(existing) = self.module.get_function(name) {
            return existing;
        }
        self.module
            .add_function(name, fn_type, Some(Linkage::External))
    }

    /// Declare (once per module, on demand) and return the C library's
    /// `memcmp` for `Str` equality (`int memcmp(const void *, const void *,
    /// size_t)`), mirroring how the trap symbol is imported. The platform `cc`
    /// link resolves it from libc on every supported host.
    fn memcmp_function(&self) -> FunctionValue<'ctx> {
        if let Some(existing) = self.module.get_function("memcmp") {
            return existing;
        }
        let fn_type = self.ctx.i32_type().fn_type(
            &[
                self.ptr_ty.into(),
                self.ptr_ty.into(),
                self.ctx.i64_type().into(),
            ],
            false,
        );
        self.module
            .add_function("memcmp", fn_type, Some(Linkage::External))
    }

    // ----- heap values (ADR-0009 Stage B) -----

    /// Declare (once per module, on demand) and return `tuo_rt_alloc(size,
    /// align) -> ptr`, mirroring how the trap/effect symbols are imported. The
    /// CLI links the allocator C shim into every built binary.
    fn alloc_function(&self) -> FunctionValue<'ctx> {
        if let Some(existing) = self.module.get_function(alloc::ALLOC_SYMBOL) {
            return existing;
        }
        let i64_ty = self.ctx.i64_type();
        let fn_type = self.ptr_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        self.module
            .add_function(alloc::ALLOC_SYMBOL, fn_type, Some(Linkage::External))
    }

    /// Declare (once per module, on demand) and return `tuo_rt_dealloc(ptr,
    /// size, align)`.
    fn dealloc_function(&self) -> FunctionValue<'ctx> {
        if let Some(existing) = self.module.get_function(alloc::DEALLOC_SYMBOL) {
            return existing;
        }
        let i64_ty = self.ctx.i64_type();
        let fn_type = self
            .ctx
            .void_type()
            .fn_type(&[self.ptr_ty.into(), i64_ty.into(), i64_ty.into()], false);
        self.module
            .add_function(alloc::DEALLOC_SYMBOL, fn_type, Some(Linkage::External))
    }

    /// Call `tuo_rt_alloc(bytes, align)` and return the buffer pointer (never
    /// null — the runtime traps on OOM).
    fn rt_alloc(
        &self,
        bytes: IntValue<'ctx>,
        align: u64,
    ) -> Result<PointerValue<'ctx>, CodegenError> {
        let align = self.ctx.i64_type().const_int(align, false);
        let call = self
            .builder
            .build_call(
                self.alloc_function(),
                &[bytes.into(), align.into()],
                "rt_alloc",
            )
            .map_err(builder_err("calling the runtime alloc"))?;
        Ok(call
            .try_as_basic_value()
            .unwrap_basic()
            .into_pointer_value())
    }

    /// Call `tuo_rt_dealloc(ptr, bytes, align)`.
    fn rt_dealloc(
        &self,
        ptr: PointerValue<'ctx>,
        bytes: IntValue<'ctx>,
        align: u64,
    ) -> Result<(), CodegenError> {
        let align = self.ctx.i64_type().const_int(align, false);
        self.builder
            .build_call(
                self.dealloc_function(),
                &[ptr.into(), bytes.into(), align.into()],
                "",
            )
            .map_err(builder_err("calling the runtime dealloc"))?;
        Ok(())
    }

    /// The fixed non-null sentinel pointer for an empty (zero-capacity) heap
    /// value, matching `alloc::ZERO_SIZE_SENTINEL`.
    fn zero_size_sentinel(&self) -> Result<PointerValue<'ctx>, CodegenError> {
        let sentinel = self
            .ctx
            .i64_type()
            .const_int(alloc::ZERO_SIZE_SENTINEL as u64, false);
        self.builder
            .build_int_to_ptr(sentinel, self.ptr_ty, "sentinel")
            .map_err(builder_err("materializing the zero-size sentinel"))
    }

    /// A whole-header address for `place`: a bare `String`/`Array` local's
    /// alloca, a projected header, or a borrowed header parameter.
    fn header_address(&mut self, place: &Place) -> Result<PointerValue<'ctx>, CodegenError> {
        self.aggregate_dest_address(place)
    }

    /// Load the `{ptr, len, cap}` words of a header at `base`.
    fn load_header(
        &mut self,
        base: PointerValue<'ctx>,
    ) -> Result<(PointerValue<'ctx>, IntValue<'ctx>, IntValue<'ctx>), CodegenError> {
        let ptr = self
            .builder
            .build_load(self.ptr_ty, base, "hdr_ptr")
            .map_err(builder_err("loading the header pointer"))?
            .into_pointer_value();
        let len_addr = self.byte_gep(base, HDR_LEN_OFFSET)?;
        let len = self
            .builder
            .build_load(self.ctx.i64_type(), len_addr, "hdr_len")
            .map_err(builder_err("loading the header length"))?
            .into_int_value();
        let cap_addr = self.byte_gep(base, HDR_CAP_OFFSET)?;
        let cap = self
            .builder
            .build_load(self.ctx.i64_type(), cap_addr, "hdr_cap")
            .map_err(builder_err("loading the header capacity"))?
            .into_int_value();
        Ok((ptr, len, cap))
    }

    /// Store the `{ptr, len, cap}` words of a header at `base`.
    fn store_header(
        &mut self,
        base: PointerValue<'ctx>,
        ptr: PointerValue<'ctx>,
        len: IntValue<'ctx>,
        cap: IntValue<'ctx>,
    ) -> Result<(), CodegenError> {
        self.builder
            .build_store(base, ptr)
            .map_err(builder_err("storing the header pointer"))?;
        let len_addr = self.byte_gep(base, HDR_LEN_OFFSET)?;
        self.builder
            .build_store(len_addr, len)
            .map_err(builder_err("storing the header length"))?;
        let cap_addr = self.byte_gep(base, HDR_CAP_OFFSET)?;
        self.builder
            .build_store(cap_addr, cap)
            .map_err(builder_err("storing the header capacity"))?;
        Ok(())
    }

    /// The element stride of a heap value's buffer: `1` for a `String`,
    /// `stride(Int)` for an `Array[Int]`.
    fn heap_stride(&self, place: &Place) -> Result<u64, CodegenError> {
        match self.place_type(place) {
            Ty::String => Ok(1),
            Ty::Array(element) => {
                require_native_array_element(&element, self.types)?;
                Ok(self.layout(&element)?.stride())
            }
            // A map's entry stride is fixed by its key kind (ADR-0011): the
            // dense entries the `tuo_rt_map_*` shim maintains.
            Ty::Map(key, _) => Ok(map_entry_stride(&key)),
            other => Err(CodegenError::backend(format!(
                "a heap operation targeted a non-heap type: {other:?}"
            ))),
        }
    }

    /// The element type of a growable `Array[T]` header place (ADR-0012).
    fn array_element_ty(&self, place: &Place) -> Result<Ty, CodegenError> {
        match self.place_type(place) {
            Ty::Array(element) => Ok((*element).clone()),
            other => Err(CodegenError::backend(format!(
                "an array element was requested from a non-array place: {other:?}"
            ))),
        }
    }

    /// Does `array::get` on this array place produce an aggregate element
    /// (`Str`/struct), rather than a scalar? (ADR-0012 Stage B.)
    fn array_get_produces_aggregate(&self, subject: &Place) -> Result<bool, CodegenError> {
        Ok(!scalar_type_is_some(&self.array_element_ty(subject)?))
    }

    /// Lower `array::get` whose element is an aggregate: bounds-check, then memcpy
    /// `stride` bytes from `ptr + index*stride` into the destination slot
    /// (ADR-0012 Stage B). The read is a **copy** (the array retains its
    /// element); for a heap-owning element (`String`, a struct/enum carrying
    /// one) the shallow byte copy is then deep-fixed-up — every owned buffer in
    /// the copy is replaced with a fresh allocation — so the result is an
    /// independent owner, exactly the interpreter's `elements[index].clone()`
    /// (ADR-0012 owned-element increment).
    fn lower_array_get_aggregate(
        &mut self,
        dest: &Place,
        subject: &Place,
        args: &[Operand],
    ) -> Result<(), CodegenError> {
        let element = self.array_element_ty(subject)?;
        let header = self.header_address(subject)?;
        let (ptr, len, _cap) = self.load_header(header)?;
        let index = self.heap_index_arg(args)?;
        let oob = self
            .builder
            .build_int_compare(IntPredicate::UGE, index, len, "get_oob")
            .map_err(builder_err("bounds-checking an array index"))?;
        self.guard(oob, TrapCode::IndexOutOfBounds)?;
        let stride = self.heap_stride(subject)?;
        let src = self.dynamic_byte_gep(ptr, index, stride)?;
        let dest_addr = self.aggregate_dest_address(dest)?;
        let layout = self.layout(&element)?;
        self.emit_memcpy(dest_addr, src, layout)?;
        if ty_owns_heap(&element, self.types) {
            self.emit_heap_glue(&element, dest_addr, HeapGlue::DeepFixup)?;
        }
        Ok(())
    }

    /// The dealloc alignment of a heap value's buffer.
    fn heap_align(&self, place: &Place) -> Result<u64, CodegenError> {
        match self.place_type(place) {
            Ty::String => Ok(1),
            Ty::Array(element) => Ok(self.layout(&element)?.align),
            Ty::Map(..) => Ok(8),
            other => Err(CodegenError::backend(format!(
                "a heap operation targeted a non-heap type: {other:?}"
            ))),
        }
    }

    /// A runtime `memcpy` of `count` bytes from `src` to `dest` (non-
    /// overlapping; `count` is a runtime value). Buffers are byte-aligned.
    fn heap_memcpy(
        &self,
        dest: PointerValue<'ctx>,
        src: PointerValue<'ctx>,
        count: IntValue<'ctx>,
    ) -> Result<(), CodegenError> {
        self.builder
            .build_memcpy(dest, 1, src, 1, count)
            .map_err(builder_err("copying heap bytes"))?;
        Ok(())
    }

    /// Materialize an aggregate-producing heap op into `dest`'s three-word
    /// header storage, matching the interpreter's `eval_heap_op`.
    fn lower_heap_op_aggregate(
        &mut self,
        dest: &Place,
        op: HeapOp,
        subject: Option<&Place>,
        args: &[Operand],
    ) -> Result<(), CodegenError> {
        match op {
            HeapOp::StringEmpty | HeapOp::ArrayEmpty | HeapOp::MapEmpty => {
                // Refuse a wrapper-containing array element up front (wrapper
                // values are not lowered anywhere) so no native path
                // half-builds an unsupported array. (A map's key/value pair is
                // already pinned to the v0 surface by the type checker.)
                if let Ty::Array(element) = self.place_type(dest) {
                    require_native_array_element(&element, self.types)?;
                }
                let base = self.aggregate_dest_address(dest)?;
                let sentinel = self.zero_size_sentinel()?;
                let zero = self.ctx.i64_type().const_zero();
                self.store_header(base, sentinel, zero, zero)
            }
            HeapOp::StringFromStr => {
                let s = args
                    .first()
                    .ok_or_else(|| CodegenError::backend("string_from_str is missing its Str"))?;
                let (src_ptr, len) = self.str_operand_parts(s)?;
                self.build_owned_from_bytes(dest, src_ptr, len)
            }
            HeapOp::StringConcat => {
                let [a, b] = args else {
                    return Err(CodegenError::backend(
                        "string_concat expects 2 Str operands",
                    ));
                };
                let (ptr_a, len_a) = self.str_operand_parts(a)?;
                let (ptr_b, len_b) = self.str_operand_parts(b)?;
                let total = self
                    .builder
                    .build_int_add(len_a, len_b, "concat_len")
                    .map_err(builder_err("computing the concat length"))?;
                let buf = self.rt_alloc(total, 1)?;
                self.heap_memcpy(buf, ptr_a, len_a)?;
                let dest_b = self.dynamic_byte_gep(buf, len_a, 1)?;
                self.heap_memcpy(dest_b, ptr_b, len_b)?;
                let base = self.aggregate_dest_address(dest)?;
                self.store_header(base, buf, total, total)
            }
            HeapOp::StringSlice => {
                let subject = subject.ok_or_else(|| {
                    CodegenError::backend("string_slice is missing its String subject")
                })?;
                let [a, b] = args else {
                    return Err(CodegenError::backend(
                        "string_slice expects 2 index operands",
                    ));
                };
                let header = self.header_address(subject)?;
                let (src_ptr, len, _cap) = self.load_header(header)?;
                let start = self.lower_operand(a)?.into_int_value();
                let end = self.lower_operand(b)?.into_int_value();
                let bad_order = self
                    .builder
                    .build_int_compare(IntPredicate::UGT, start, end, "slice_bad_order")
                    .map_err(builder_err("comparing the slice bounds"))?;
                self.guard(bad_order, TrapCode::IndexOutOfBounds)?;
                let bad_end = self
                    .builder
                    .build_int_compare(IntPredicate::UGT, end, len, "slice_bad_end")
                    .map_err(builder_err("comparing the slice end"))?;
                self.guard(bad_end, TrapCode::IndexOutOfBounds)?;
                let count = self
                    .builder
                    .build_int_sub(end, start, "slice_len")
                    .map_err(builder_err("computing the slice length"))?;
                let range_ptr = self.dynamic_byte_gep(src_ptr, start, 1)?;
                self.build_owned_from_bytes(dest, range_ptr, count)
            }
            HeapOp::StringAsStr => {
                // A borrowed `{ptr, len}` view of the subject `String`'s live
                // bytes — **zero-copy**: the two header words are copied, the
                // buffer never is (ADR-0010). The ownership checker's O0011
                // rule keeps the view from outliving (or aliasing a mutation
                // of) the `String`, so the pointer cannot dangle.
                let subject = subject.ok_or_else(|| {
                    CodegenError::backend("string_as_str is missing its String subject")
                })?;
                let header = self.header_address(subject)?;
                let (ptr, len, _cap) = self.load_header(header)?;
                let base = self.aggregate_dest_address(dest)?;
                self.builder
                    .build_store(base, ptr)
                    .map_err(builder_err("storing the view data pointer"))?;
                let len_addr = self.byte_gep(base, STR_LEN_OFFSET)?;
                self.builder
                    .build_store(len_addr, len)
                    .map_err(builder_err("storing the view length"))?;
                Ok(())
            }
            HeapOp::MapGet => {
                // `get(in Map, k) -> Option[Int]`: the shim probes the table
                // into a `{found, value}` out buffer; the Option destination
                // is materialized from it (ADR-0011).
                let subject = subject
                    .ok_or_else(|| CodegenError::backend("map_get is missing its Map subject"))?;
                let header = self.header_address(subject)?;
                let out = self.map_out_addr()?;
                let key_is_str = self.map_key_is_str(subject)?;
                let key = args
                    .first()
                    .ok_or_else(|| CodegenError::backend("map_get is missing its key"))?;
                let mut call_args: Vec<BasicMetadataValueEnum<'ctx>> = vec![header.into()];
                if key_is_str {
                    let (kp, kn) = self.str_operand_parts(key)?;
                    call_args.push(kp.into());
                    call_args.push(kn.into());
                } else {
                    call_args.push(self.lower_operand(key)?.into());
                }
                call_args.push(out.into());
                let symbol = if key_is_str {
                    map::MAP_STR_GET_SYMBOL
                } else {
                    map::MAP_INT_GET_SYMBOL
                };
                self.call_map_shim(symbol, &call_args)?;
                self.write_option_int_dest(dest, out)
            }
            HeapOp::MapKeys => {
                // `keys(in Map) -> Array[K]`: the shim allocates the fresh
                // keys buffer and writes the three-word array header straight
                // into the destination (insertion order, ADR-0011).
                let subject = subject
                    .ok_or_else(|| CodegenError::backend("map_keys is missing its Map subject"))?;
                let header = self.header_address(subject)?;
                let dest_addr = self.aggregate_dest_address(dest)?;
                let symbol = if self.map_key_is_str(subject)? {
                    map::MAP_STR_KEYS_SYMBOL
                } else {
                    map::MAP_INT_KEYS_SYMBOL
                };
                self.call_map_shim(symbol, &[header.into(), dest_addr.into()])
            }
            _ => Err(CodegenError::backend(
                "a non-aggregate heap op reached `lower_heap_op_aggregate`",
            )),
        }
    }

    /// The `Str`-vs-`Int` key kind of a map place (ADR-0011): decides which
    /// `tuo_rt_map_*` symbol family a lowering calls.
    fn map_key_is_str(&self, place: &Place) -> Result<bool, CodegenError> {
        match self.place_type(place) {
            Ty::Map(key, _) => Ok(matches!(*key, Ty::Str)),
            other => Err(CodegenError::backend(format!(
                "a map operation targeted a non-map place: {other:?}"
            ))),
        }
    }

    /// A fresh two-word `{found, value}` out buffer for the map shim calls.
    fn map_out_addr(&mut self) -> Result<PointerValue<'ctx>, CodegenError> {
        self.builder
            .build_alloca(self.ctx.i64_type().array_type(2), "map_out")
            .map_err(builder_err("allocating the map out buffer"))
    }

    /// Declare (idempotently) and call a `tuo_rt_map_*` shim function: the
    /// signature is derived from the argument values (pointers and `i64`s),
    /// and every shim returns void — results come back through the out
    /// buffer or a written header.
    fn call_map_shim(
        &mut self,
        symbol: &str,
        args: &[BasicMetadataValueEnum<'ctx>],
    ) -> Result<(), CodegenError> {
        let function = match self.module.get_function(symbol) {
            Some(existing) => existing,
            None => {
                let param_types: Vec<BasicMetadataTypeEnum<'ctx>> = args
                    .iter()
                    .map(|arg| {
                        if arg.is_pointer_value() {
                            self.ptr_ty.into()
                        } else {
                            self.ctx.i64_type().into()
                        }
                    })
                    .collect();
                let fn_type = self.ctx.void_type().fn_type(&param_types, false);
                self.module
                    .add_function(symbol, fn_type, Some(Linkage::External))
            }
        };
        self.builder
            .build_call(function, args, "map_shim")
            .map_err(builder_err("calling the map runtime"))?;
        Ok(())
    }

    /// Materialize an `Option[Int]` destination from a map shim's two-word
    /// `{found, value}` out buffer: tag = `1 - found` (`Some` is variant 0,
    /// `None` variant 1), payload = the value word (deterministically zero
    /// when absent, so no branch is needed).
    fn write_option_int_dest(
        &mut self,
        dest: &Place,
        out: PointerValue<'ctx>,
    ) -> Result<(), CodegenError> {
        let dest_ty = self.place_type(dest);
        let dest_base = self.aggregate_dest_address(dest)?;
        let i64_ty = self.ctx.i64_type();
        let found = self
            .builder
            .build_load(i64_ty, out, "map_found")
            .map_err(builder_err("loading the map found flag"))?
            .into_int_value();
        let value_addr = self.byte_gep(out, 8)?;
        let value = self
            .builder
            .build_load(i64_ty, value_addr, "map_value")
            .map_err(builder_err("loading the map value"))?
            .into_int_value();
        let one = i64_ty.const_int(1, false);
        let tag64 = self
            .builder
            .build_int_sub(one, found, "map_tag64")
            .map_err(builder_err("computing the Option tag"))?;
        let tag = self
            .builder
            .build_int_truncate(tag64, self.ctx.i32_type(), "map_tag")
            .map_err(builder_err("narrowing the Option tag"))?;
        self.builder
            .build_store(dest_base, tag)
            .map_err(builder_err("storing the Option tag"))?;
        let payload_offsets = variant_field_offsets(&dest_ty, 0, self.types)
            .map_err(|error| self.layout_error(error))?;
        let payload_offset = *payload_offsets
            .first()
            .ok_or_else(|| CodegenError::backend("Option Some payload has no field"))?;
        let payload_addr = self.byte_gep(dest_base, payload_offset)?;
        self.builder
            .build_store(payload_addr, value)
            .map_err(builder_err("storing the Option payload"))?;
        Ok(())
    }

    /// Build an owned `String` in `dest` from `count` bytes at `src`: alloc
    /// `count` bytes (align 1), copy them in, header `{buf, count, count}`.
    fn build_owned_from_bytes(
        &mut self,
        dest: &Place,
        src: PointerValue<'ctx>,
        count: IntValue<'ctx>,
    ) -> Result<(), CodegenError> {
        let buf = self.rt_alloc(count, 1)?;
        self.heap_memcpy(buf, src, count)?;
        let base = self.aggregate_dest_address(dest)?;
        self.store_header(base, buf, count, count)
    }

    /// Lower a scalar-valued heap read (`string_len`, `string_byte_at`,
    /// `array_len`, `array_get`) to an `i64`, matching the interpreter.
    fn lower_heap_op_scalar(
        &mut self,
        op: HeapOp,
        subject: Option<&Place>,
        args: &[Operand],
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let subject = subject
            .ok_or_else(|| CodegenError::backend("a heap read is missing its subject place"))?;
        let header = self.header_address(subject)?;
        let (ptr, len, _cap) = self.load_header(header)?;
        match op {
            HeapOp::StringLen | HeapOp::ArrayLen | HeapOp::MapLen => Ok(len.into()),
            HeapOp::MapContainsKey => {
                // `contains_key` is `get` with the value discarded: probe via
                // the shim's out buffer and produce the `found` word as Bool.
                let out = self.map_out_addr()?;
                let key_is_str = self.map_key_is_str(subject)?;
                let key = args
                    .first()
                    .ok_or_else(|| CodegenError::backend("map_contains_key is missing its key"))?;
                let mut call_args: Vec<BasicMetadataValueEnum<'ctx>> = vec![header.into()];
                if key_is_str {
                    let (kp, kn) = self.str_operand_parts(key)?;
                    call_args.push(kp.into());
                    call_args.push(kn.into());
                } else {
                    call_args.push(self.lower_operand(key)?.into());
                }
                call_args.push(out.into());
                let symbol = if key_is_str {
                    map::MAP_STR_GET_SYMBOL
                } else {
                    map::MAP_INT_GET_SYMBOL
                };
                self.call_map_shim(symbol, &call_args)?;
                let found = self
                    .builder
                    .build_load(self.ctx.i64_type(), out, "map_found")
                    .map_err(builder_err("loading the map found flag"))?
                    .into_int_value();
                let truthy = self
                    .builder
                    .build_int_compare(
                        IntPredicate::NE,
                        found,
                        self.ctx.i64_type().const_zero(),
                        "map_contains",
                    )
                    .map_err(builder_err("testing the map found flag"))?;
                // Bool storage is i8, matching every other Bool producer.
                Ok(self
                    .builder
                    .build_int_z_extend(truthy, self.ctx.i8_type(), "map_contains_i8")
                    .map_err(builder_err("widening the contains verdict"))?
                    .into())
            }
            HeapOp::StringByteAt => {
                let index = self.heap_index_arg(args)?;
                let oob = self
                    .builder
                    .build_int_compare(IntPredicate::UGE, index, len, "byte_oob")
                    .map_err(builder_err("bounds-checking a String byte index"))?;
                self.guard(oob, TrapCode::IndexOutOfBounds)?;
                let addr = self.dynamic_byte_gep(ptr, index, 1)?;
                let byte = self
                    .builder
                    .build_load(self.ctx.i8_type(), addr, "byte")
                    .map_err(builder_err("loading a String byte"))?
                    .into_int_value();
                Ok(self
                    .builder
                    .build_int_z_extend(byte, self.ctx.i64_type(), "byte_i64")
                    .map_err(builder_err("widening a String byte"))?
                    .into())
            }
            HeapOp::ArrayGet => {
                let index = self.heap_index_arg(args)?;
                let oob = self
                    .builder
                    .build_int_compare(IntPredicate::UGE, index, len, "get_oob")
                    .map_err(builder_err("bounds-checking an array index"))?;
                self.guard(oob, TrapCode::IndexOutOfBounds)?;
                let stride = self.heap_stride(subject)?;
                let addr = self.dynamic_byte_gep(ptr, index, stride)?;
                // Scalar element read: load the element's own register width
                // (`i64` for `Int`, `i8` for `Bool`, …), ADR-0012. An aggregate
                // element never reaches here — the rvalue dispatch routes it to
                // `lower_array_get_aggregate`.
                let element = self.array_element_ty(subject)?;
                let load_ty = scalar_type(self.ctx, &element).ok_or_else(|| {
                    CodegenError::backend("a scalar array_get reached a non-scalar element")
                })?;
                Ok(self
                    .builder
                    .build_load(load_ty, addr, "elem")
                    .map_err(builder_err("loading an array element"))?)
            }
            _ => Err(CodegenError::backend(
                "an aggregate-producing heap op reached `lower_heap_op_scalar`",
            )),
        }
    }

    /// The single integer index operand of a `byte_at`/`get`/`push`/`push_byte`.
    fn heap_index_arg(&mut self, args: &[Operand]) -> Result<IntValue<'ctx>, CodegenError> {
        let index = args
            .first()
            .ok_or_else(|| CodegenError::backend("a heap index op is missing its operand"))?;
        Ok(self.lower_operand(index)?.into_int_value())
    }

    /// Lower `Statement::HeapMutate`, matching the interpreter's
    /// `exec_heap_mutate`. `push_byte` traps `InvalidByte` before any state
    /// change; growth is alloc-new + copy + dealloc-old.
    fn lower_heap_mutate(
        &mut self,
        op: HeapMutOp,
        target: &Place,
        args: &[Operand],
        dest: &Place,
    ) -> Result<(), CodegenError> {
        let stride = self.heap_stride(target)?;
        let align = self.heap_align(target)?;
        match op {
            HeapMutOp::PushByte => {
                let byte = self.heap_index_arg(args)?;
                let limit = self.ctx.i64_type().const_int(255, false);
                let too_big = self
                    .builder
                    .build_int_compare(IntPredicate::UGT, byte, limit, "byte_range")
                    .map_err(builder_err("range-checking a pushed byte"))?;
                self.guard(too_big, TrapCode::InvalidByte)?;
                let one = self.ctx.i64_type().const_int(1, false);
                let buf = self.ensure_capacity(target, one, stride, align)?;
                let header = self.header_address(target)?;
                let (_ptr, len, _cap) = self.load_header(header)?;
                let addr = self.dynamic_byte_gep(buf, len, 1)?;
                let byte8 = self
                    .builder
                    .build_int_truncate(byte, self.ctx.i8_type(), "byte8")
                    .map_err(builder_err("truncating a pushed byte"))?;
                self.builder
                    .build_store(addr, byte8)
                    .map_err(builder_err("storing a pushed byte"))?;
                self.bump_len(header, len, one)?;
                Ok(())
            }
            HeapMutOp::Append => {
                let t = args
                    .first()
                    .ok_or_else(|| CodegenError::backend("append is missing its Str"))?;
                let (t_ptr, t_len) = self.str_operand_parts(t)?;
                let buf = self.ensure_capacity(target, t_len, stride, align)?;
                let header = self.header_address(target)?;
                let (_ptr, len, _cap) = self.load_header(header)?;
                let dest_addr = self.dynamic_byte_gep(buf, len, 1)?;
                self.heap_memcpy(dest_addr, t_ptr, t_len)?;
                self.bump_len(header, len, t_len)?;
                Ok(())
            }
            HeapMutOp::Push => {
                // A scalar element is a single store of its register width; an
                // aggregate element (`Str`/struct) is a memcpy of `stride` bytes
                // from the operand's slot (ADR-0012 Stage B).
                let element = self.array_element_ty(target)?;
                let one = self.ctx.i64_type().const_int(1, false);
                let buf = self.ensure_capacity(target, one, stride, align)?;
                let header = self.header_address(target)?;
                let (_ptr, len, _cap) = self.load_header(header)?;
                let addr = self.dynamic_byte_gep(buf, len, stride)?;
                let arg = args
                    .first()
                    .ok_or_else(|| CodegenError::backend("push is missing its element"))?;
                if scalar_type_is_some(&element) {
                    let v = self.lower_operand(arg)?;
                    self.builder
                        .build_store(addr, v)
                        .map_err(builder_err("storing a pushed element"))?;
                } else {
                    let src = self.operand_aggregate_address(arg)?;
                    let layout = self.layout(&element)?;
                    self.emit_memcpy(addr, src, layout)?;
                }
                self.bump_len(header, len, one)?;
                Ok(())
            }
            HeapMutOp::Pop => self.lower_array_pop(target, stride, dest),
            HeapMutOp::MapInsert | HeapMutOp::MapRemove => {
                // The whole table transition lives in the `tuo_rt_map_*` shim
                // (ADR-0011): pass the header, the key (and value for insert),
                // and a two-word out buffer `{found, previous}`, then
                // materialize the `Option[Int]` destination from it.
                let header = self.header_address(target)?;
                let out = self.map_out_addr()?;
                let key_is_str = self.map_key_is_str(target)?;
                let insert = matches!(op, HeapMutOp::MapInsert);
                let key = args
                    .first()
                    .ok_or_else(|| CodegenError::backend("a map mutator is missing its key"))?;
                let mut call_args: Vec<BasicMetadataValueEnum<'ctx>> = vec![header.into()];
                if key_is_str {
                    let (kp, kn) = self.str_operand_parts(key)?;
                    call_args.push(kp.into());
                    call_args.push(kn.into());
                } else {
                    call_args.push(self.lower_operand(key)?.into());
                }
                if insert {
                    let value = args.get(1).ok_or_else(|| {
                        CodegenError::backend("a map insert is missing its value")
                    })?;
                    call_args.push(self.lower_operand(value)?.into());
                }
                call_args.push(out.into());
                let symbol = match (insert, key_is_str) {
                    (true, false) => map::MAP_INT_INSERT_SYMBOL,
                    (true, true) => map::MAP_STR_INSERT_SYMBOL,
                    (false, false) => map::MAP_INT_REMOVE_SYMBOL,
                    (false, true) => map::MAP_STR_REMOVE_SYMBOL,
                };
                self.call_map_shim(symbol, &call_args)?;
                self.write_option_int_dest(dest, out)
            }
        }
    }

    /// Store `len + delta` into the header's `len` word.
    fn bump_len(
        &mut self,
        header: PointerValue<'ctx>,
        len: IntValue<'ctx>,
        delta: IntValue<'ctx>,
    ) -> Result<(), CodegenError> {
        let new_len = self
            .builder
            .build_int_add(len, delta, "new_len")
            .map_err(builder_err("incrementing the header length"))?;
        let len_addr = self.byte_gep(header, HDR_LEN_OFFSET)?;
        self.builder
            .build_store(len_addr, new_len)
            .map_err(builder_err("storing the new length"))?;
        Ok(())
    }

    /// Ensure `target`'s buffer has room for `extra` more elements: if `len +
    /// extra > cap`, allocate `max(needed, cap*2, 1)` capacity, copy the live
    /// `len × stride` bytes, free the old buffer (only when the old `cap != 0`),
    /// and update the header's `ptr`/`cap`. Returns the (possibly new) buffer
    /// pointer, `len` unchanged. Mirrors the Cranelift backend's
    /// `ensure_capacity`.
    fn ensure_capacity(
        &mut self,
        target: &Place,
        extra: IntValue<'ctx>,
        stride: u64,
        align: u64,
    ) -> Result<PointerValue<'ctx>, CodegenError> {
        let header = self.header_address(target)?;
        let (ptr, len, cap) = self.load_header(header)?;
        let i64_ty = self.ctx.i64_type();
        let needed = self
            .builder
            .build_int_add(len, extra, "needed")
            .map_err(builder_err("computing the needed capacity"))?;
        let fits = self
            .builder
            .build_int_compare(IntPredicate::ULE, needed, cap, "fits")
            .map_err(builder_err("comparing needed to capacity"))?;

        let start_block = self
            .builder
            .get_insert_block()
            .ok_or_else(|| CodegenError::backend("ensure_capacity outside a block"))?;
        let grow_block = self.ctx.append_basic_block(self.value, "grow");
        let done_block = self.ctx.append_basic_block(self.value, "grow_done");
        self.builder
            .build_conditional_branch(fits, done_block, grow_block)
            .map_err(builder_err("branching on capacity"))?;

        // Grow: new_cap = max(needed, cap*2, 1).
        self.builder.position_at_end(grow_block);
        let two = i64_ty.const_int(2, false);
        let doubled = self
            .builder
            .build_int_mul(cap, two, "doubled")
            .map_err(builder_err("doubling the capacity"))?;
        let need_ge_double = self
            .builder
            .build_int_compare(IntPredicate::UGE, needed, doubled, "need_ge_double")
            .map_err(builder_err("comparing needed to doubled"))?;
        let max1 = self
            .builder
            .build_select(need_ge_double, needed, doubled, "cap_max")
            .map_err(builder_err("selecting the grown capacity"))?
            .into_int_value();
        let one = i64_ty.const_int(1, false);
        let ge_one = self
            .builder
            .build_int_compare(IntPredicate::UGE, max1, one, "ge_one")
            .map_err(builder_err("comparing the grown capacity to one"))?;
        let new_cap = self
            .builder
            .build_select(ge_one, max1, one, "new_cap")
            .map_err(builder_err("flooring the grown capacity at one"))?
            .into_int_value();
        let stride_c = i64_ty.const_int(stride, false);
        let new_bytes = self
            .builder
            .build_int_mul(new_cap, stride_c, "new_bytes")
            .map_err(builder_err("computing the new byte size"))?;
        let new_buf = self.rt_alloc(new_bytes, align)?;
        let live_bytes = self
            .builder
            .build_int_mul(len, stride_c, "live_bytes")
            .map_err(builder_err("computing the live byte count"))?;
        self.heap_memcpy(new_buf, ptr, live_bytes)?;
        // Free the old buffer only when the old cap != 0.
        let old_has_buf = self
            .builder
            .build_int_compare(IntPredicate::NE, cap, i64_ty.const_zero(), "old_has_buf")
            .map_err(builder_err("checking the old capacity"))?;
        let free_block = self.ctx.append_basic_block(self.value, "grow_free");
        let after_free = self.ctx.append_basic_block(self.value, "grow_after_free");
        self.builder
            .build_conditional_branch(old_has_buf, free_block, after_free)
            .map_err(builder_err("branching on the old capacity"))?;
        self.builder.position_at_end(free_block);
        let old_bytes = self
            .builder
            .build_int_mul(cap, stride_c, "old_bytes")
            .map_err(builder_err("computing the old byte size"))?;
        self.rt_dealloc(ptr, old_bytes, align)?;
        self.builder
            .build_unconditional_branch(after_free)
            .map_err(builder_err("joining after the free"))?;
        self.builder.position_at_end(after_free);
        // Write the new ptr/cap into the header (len unchanged).
        self.builder
            .build_store(header, new_buf)
            .map_err(builder_err("storing the grown buffer pointer"))?;
        let cap_addr = self.byte_gep(header, HDR_CAP_OFFSET)?;
        self.builder
            .build_store(cap_addr, new_cap)
            .map_err(builder_err("storing the grown capacity"))?;
        self.builder
            .build_unconditional_branch(done_block)
            .map_err(builder_err("joining the grow path"))?;
        let grow_exit = after_free;

        // Merge the kept and grown buffer pointers.
        self.builder.position_at_end(done_block);
        let phi = self
            .builder
            .build_phi(self.ptr_ty, "buf")
            .map_err(builder_err("merging the buffer pointer"))?;
        phi.add_incoming(&[(&ptr, start_block), (&new_buf, grow_exit)]);
        Ok(phi.as_basic_value().into_pointer_value())
    }

    /// Lower `pop(target: mut Array[Int]) -> Option[Int]`: `None` (variant 1)
    /// when empty, else `len -= 1`, load the last element, `Some { value }`
    /// (variant 0). Never traps; the buffer is not shrunk.
    fn lower_array_pop(
        &mut self,
        target: &Place,
        stride: u64,
        dest: &Place,
    ) -> Result<(), CodegenError> {
        let header = self.header_address(target)?;
        let (ptr, len, _cap) = self.load_header(header)?;
        let dest_ty = self.place_type(dest);
        let dest_base = self.aggregate_dest_address(dest)?;
        let is_empty = self
            .builder
            .build_int_compare(
                IntPredicate::EQ,
                len,
                self.ctx.i64_type().const_zero(),
                "empty",
            )
            .map_err(builder_err("checking whether the array is empty"))?;

        let none_block = self.ctx.append_basic_block(self.value, "pop_none");
        let some_block = self.ctx.append_basic_block(self.value, "pop_some");
        let join = self.ctx.append_basic_block(self.value, "pop_join");
        self.builder
            .build_conditional_branch(is_empty, none_block, some_block)
            .map_err(builder_err("branching on emptiness"))?;

        // None = variant 1, empty payload.
        self.builder.position_at_end(none_block);
        let none_tag = self.ctx.i32_type().const_int(1, false);
        self.builder
            .build_store(dest_base, none_tag)
            .map_err(builder_err("storing the None tag"))?;
        self.builder
            .build_unconditional_branch(join)
            .map_err(builder_err("joining the None arm"))?;

        // Some = variant 0: len -= 1, load, store tag + payload.
        self.builder.position_at_end(some_block);
        let one = self.ctx.i64_type().const_int(1, false);
        let new_len = self
            .builder
            .build_int_sub(len, one, "pop_len")
            .map_err(builder_err("decrementing the length"))?;
        let len_addr = self.byte_gep(header, HDR_LEN_OFFSET)?;
        self.builder
            .build_store(len_addr, new_len)
            .map_err(builder_err("storing the decremented length"))?;
        let elem_addr = self.dynamic_byte_gep(ptr, new_len, stride)?;
        let some_tag = self.ctx.i32_type().const_zero();
        self.builder
            .build_store(dest_base, some_tag)
            .map_err(builder_err("storing the Some tag"))?;
        let payload_offsets = variant_field_offsets(&dest_ty, 0, self.types)
            .map_err(|error| self.layout_error(error))?;
        let payload_offset = *payload_offsets
            .first()
            .ok_or_else(|| CodegenError::backend("Option Some payload has no field"))?;
        let payload_addr = self.byte_gep(dest_base, payload_offset)?;
        // Move the popped element into the `Some` payload: a scalar is a
        // load-and-store of its width; an aggregate element is a memcpy of
        // `stride` bytes (ADR-0012 Stage B). `pop` moves the element out.
        let element = self.array_element_ty(target)?;
        if let Some(load_ty) = scalar_type(self.ctx, &element) {
            let value = self
                .builder
                .build_load(load_ty, elem_addr, "popped")
                .map_err(builder_err("loading the popped element"))?;
            self.builder
                .build_store(payload_addr, value)
                .map_err(builder_err("storing the Some payload"))?;
        } else {
            let layout = self.layout(&element)?;
            self.emit_memcpy(payload_addr, elem_addr, layout)?;
        }
        self.builder
            .build_unconditional_branch(join)
            .map_err(builder_err("joining the Some arm"))?;

        self.builder.position_at_end(join);
        Ok(())
    }

    /// Lower `Statement::Drop` (ADR-0009 Stage B; recursive since the ADR-0012
    /// owned-element increment). A value that owns no heap drops as a no-op; a
    /// heap-owning value (a `String`, an `Array`, or an aggregate carrying one)
    /// is walked by `emit_heap_glue`, which frees element buffers before the
    /// containing buffer — the native mirror of the interpreter's
    /// de-initializing drop of a recursive `Value`. The moved-from place is
    /// de-initialized by MIR, so a buffer is freed exactly once.
    fn lower_drop(&mut self, place: &Place) -> Result<(), CodegenError> {
        let ty = self.place_type(place);
        if !ty_owns_heap(&ty, self.types) {
            // Scalars, Str, plain aggregates, fixed arrays: no heap to free.
            return Ok(());
        }
        let base = self.header_address(place)?;
        self.emit_heap_glue(&ty, base, HeapGlue::DropInPlace)
    }

    /// Walk the heap-owning parts of the value of type `ty` at `addr`, applying
    /// `glue` (deep-copy fixup or drop) to each owned buffer, recursively —
    /// the single traversal both `array::get`'s deep copy and `Drop` use, so
    /// the two can never disagree about ownership. A no-op for a type that owns
    /// no heap. Mirrors the Cranelift backend's `emit_heap_glue`.
    ///
    /// # Errors
    ///
    /// [`CodegenError::unsupported`] for a `Box`/`Shared`/`Weak` wrapper (not
    /// lowered anywhere); [`CodegenError::backend`] on a layout failure.
    fn emit_heap_glue(
        &mut self,
        ty: &Ty,
        addr: PointerValue<'ctx>,
        glue: HeapGlue,
    ) -> Result<(), CodegenError> {
        if !ty_owns_heap(ty, self.types) {
            return Ok(());
        }
        match ty {
            Ty::String => match glue {
                HeapGlue::DeepFixup => {
                    // Replace the aliased buffer with a fresh copy of the live
                    // `len` bytes; the copy's capacity is its length.
                    let (ptr, len, _cap) = self.load_header(addr)?;
                    let buf = self.rt_alloc(len, 1)?;
                    self.heap_memcpy(buf, ptr, len)?;
                    self.store_header(addr, buf, len, len)
                }
                HeapGlue::DropInPlace => {
                    let (ptr, _len, cap) = self.load_header(addr)?;
                    self.emit_buffer_free(ptr, cap, 1, 1)
                }
            },
            Ty::Array(element) => {
                let stride = self.layout(element)?.stride();
                let align = self.layout(element)?.align;
                let stride_c = self.ctx.i64_type().const_int(stride, false);
                match glue {
                    HeapGlue::DeepFixup => {
                        // Fresh buffer for the live `len` elements, shallow-copy
                        // them, then fix each copied element up in turn.
                        let (ptr, len, _cap) = self.load_header(addr)?;
                        let bytes = self
                            .builder
                            .build_int_mul(len, stride_c, "fixup_bytes")
                            .map_err(builder_err("computing the deep-copy byte size"))?;
                        let buf = self.rt_alloc(bytes, align)?;
                        self.heap_memcpy(buf, ptr, bytes)?;
                        self.store_header(addr, buf, len, len)?;
                        if ty_owns_heap(element, self.types) {
                            self.emit_element_loop(buf, len, stride, element, glue)?;
                        }
                        Ok(())
                    }
                    HeapGlue::DropInPlace => {
                        // Elements first (front to back, like the interpreter's
                        // `Vec` drop), then the buffer itself.
                        let (ptr, len, cap) = self.load_header(addr)?;
                        if ty_owns_heap(element, self.types) {
                            self.emit_element_loop(ptr, len, stride, element, glue)?;
                        }
                        self.emit_buffer_free(ptr, cap, stride, align)
                    }
                }
            }
            Ty::Struct(..) | Ty::Tuple(..) => {
                for (offset, field_ty) in self.heap_struct_fields(ty)? {
                    let field_addr = self.byte_gep(addr, offset)?;
                    self.emit_heap_glue(&field_ty, field_addr, glue)?;
                }
                Ok(())
            }
            Ty::Map(key, _) => match glue {
                // A map cannot be an array element (the checker's ADR-0011
                // surface refuses it), so the deep-copy fixup can never reach
                // one; refusing keeps the walk honest if that ever changes.
                HeapGlue::DeepFixup => Err(CodegenError::backend(
                    "deep-copy fixup reached a Map value (maps are not array elements in v0)",
                )),
                // The whole block (index + entries) is freed by the shim,
                // which alone knows the internal layout (ADR-0011).
                HeapGlue::DropInPlace => {
                    let stride = self.ctx.i64_type().const_int(map_entry_stride(key), false);
                    self.call_map_shim(map::MAP_DROP_SYMBOL, &[addr.into(), stride.into()])
                }
            },
            Ty::Enum(..) | Ty::Option(_) | Ty::Result(..) => self.emit_variant_glue(ty, addr, glue),
            Ty::FixedArray(element, count) => {
                let stride = self.layout(element)?.stride();
                for index in 0..*count {
                    let element_addr = self.byte_gep(addr, index * stride)?;
                    self.emit_heap_glue(element, element_addr, glue)?;
                }
                Ok(())
            }
            Ty::Wrapper(kind, _) => Err(CodegenError::unsupported(format!(
                "the native backend does not lower a `{}[T]` heap wrapper value \
                 (heap wrappers await a later ADR); the interpreter remains the reference",
                kind.name()
            ))),
            other => Err(CodegenError::backend(format!(
                "heap glue reached a type that cannot own heap: {other:?}"
            ))),
        }
    }

    /// The heap-owning fields of a struct/tuple as `(byte offset, field type)`
    /// pairs, targs substituted — the fields `emit_heap_glue` must visit.
    fn heap_struct_fields(&self, ty: &Ty) -> Result<Vec<(u64, Ty)>, CodegenError> {
        let offsets = struct_field_offsets(ty, self.types).map_err(|e| self.layout_error(e))?;
        let field_tys: Vec<Ty> = match ty {
            Ty::Struct(symbol, targs) => {
                let shape = self
                    .types
                    .struct_shape(*symbol)
                    .ok_or_else(|| CodegenError::backend("heap glue on an unknown struct"))?;
                shape
                    .fields
                    .iter()
                    .map(|(_, field)| substitute_targs(field, &shape.type_params, targs))
                    .collect()
            }
            Ty::Tuple(items) => items.clone(),
            other => {
                return Err(CodegenError::backend(format!(
                    "struct heap glue on a non-struct type: {other:?}"
                )));
            }
        };
        Ok(offsets
            .into_iter()
            .zip(field_tys)
            .filter(|(_, field_ty)| ty_owns_heap(field_ty, self.types))
            .collect())
    }

    /// The variant payload field types of an enum-shaped type (`Enum`,
    /// `Option`, `Result`), variant-indexed in the ABI's declaration order
    /// (`Some`/`Ok` = 0, `None`/`Err` = 1), targs substituted.
    fn variant_payloads(&self, ty: &Ty) -> Result<Vec<Vec<Ty>>, CodegenError> {
        match ty {
            Ty::Enum(symbol, targs) => {
                let shape = self
                    .types
                    .enum_shape(*symbol)
                    .ok_or_else(|| CodegenError::backend("heap glue on an unknown enum"))?;
                Ok(shape
                    .variants
                    .iter()
                    .map(|(_, fields)| {
                        fields
                            .iter()
                            .map(|(_, field)| substitute_targs(field, &shape.type_params, targs))
                            .collect()
                    })
                    .collect())
            }
            Ty::Option(item) => Ok(vec![vec![(**item).clone()], Vec::new()]),
            Ty::Result(ok, err) => Ok(vec![vec![(**ok).clone()], vec![(**err).clone()]]),
            other => Err(CodegenError::backend(format!(
                "variant heap glue on a non-enum type: {other:?}"
            ))),
        }
    }

    /// Apply `glue` to the heap-owning payload fields of the *live* variant of
    /// the enum-shaped value at `addr`: load the `u32` discriminant, then a
    /// chain of compare-and-branch arms, one per variant that carries a
    /// heap-owning field (variants without one need no code).
    fn emit_variant_glue(
        &mut self,
        ty: &Ty,
        addr: PointerValue<'ctx>,
        glue: HeapGlue,
    ) -> Result<(), CodegenError> {
        let payloads = self.variant_payloads(ty)?;
        let disc = self
            .builder
            .build_load(self.ctx.i32_type(), addr, "glue_disc")
            .map_err(builder_err("loading the glue discriminant"))?
            .into_int_value();
        let join = self.ctx.append_basic_block(self.value, "glue_join");
        for (variant, fields) in payloads.iter().enumerate() {
            let heap_fields: Vec<(u64, Ty)> = {
                let offsets = variant_field_offsets(ty, variant, self.types)
                    .map_err(|e| self.layout_error(e))?;
                offsets
                    .into_iter()
                    .zip(fields.iter().cloned())
                    .filter(|(_, field_ty)| ty_owns_heap(field_ty, self.types))
                    .collect()
            };
            if heap_fields.is_empty() {
                continue;
            }
            let variant_tag = u64::try_from(variant)
                .map_err(|_| CodegenError::backend("variant index exceeds u64"))?;
            let tag = self.ctx.i32_type().const_int(variant_tag, false);
            let is_live = self
                .builder
                .build_int_compare(IntPredicate::EQ, disc, tag, "glue_live")
                .map_err(builder_err("comparing the glue discriminant"))?;
            let glue_block = self.ctx.append_basic_block(self.value, "glue_variant");
            let next_block = self.ctx.append_basic_block(self.value, "glue_next");
            self.builder
                .build_conditional_branch(is_live, glue_block, next_block)
                .map_err(builder_err("branching on the glue discriminant"))?;
            self.builder.position_at_end(glue_block);
            for (offset, field_ty) in heap_fields {
                let field_addr = self.byte_gep(addr, offset)?;
                self.emit_heap_glue(&field_ty, field_addr, glue)?;
            }
            self.builder
                .build_unconditional_branch(join)
                .map_err(builder_err("joining a glue variant arm"))?;
            self.builder.position_at_end(next_block);
        }
        self.builder
            .build_unconditional_branch(join)
            .map_err(builder_err("joining the glue fall-through"))?;
        self.builder.position_at_end(join);
        Ok(())
    }

    /// A counted loop applying `glue` to each of the `len` elements of type
    /// `element` in the buffer at `buf` (`stride` bytes apart) — the one place
    /// codegen emits a genuine back-edge. The induction index is a phi merging
    /// the entry zero with the incremented value from the (possibly extended)
    /// body-end block. Mirrors the Cranelift backend's `emit_element_loop`.
    fn emit_element_loop(
        &mut self,
        buf: PointerValue<'ctx>,
        len: IntValue<'ctx>,
        stride: u64,
        element: &Ty,
        glue: HeapGlue,
    ) -> Result<(), CodegenError> {
        let i64_ty = self.ctx.i64_type();
        let entry = self
            .builder
            .get_insert_block()
            .ok_or_else(|| CodegenError::backend("element loop outside a block"))?;
        let header = self.ctx.append_basic_block(self.value, "elem_header");
        let body = self.ctx.append_basic_block(self.value, "elem_body");
        let exit = self.ctx.append_basic_block(self.value, "elem_exit");
        self.builder
            .build_unconditional_branch(header)
            .map_err(builder_err("entering the element loop"))?;

        self.builder.position_at_end(header);
        let phi = self
            .builder
            .build_phi(i64_ty, "elem_idx")
            .map_err(builder_err("merging the element index"))?;
        let zero = i64_ty.const_zero();
        phi.add_incoming(&[(&zero, entry)]);
        let index = phi.as_basic_value().into_int_value();
        let done = self
            .builder
            .build_int_compare(IntPredicate::UGE, index, len, "elem_done")
            .map_err(builder_err("comparing the element index"))?;
        self.builder
            .build_conditional_branch(done, exit, body)
            .map_err(builder_err("branching on the element index"))?;

        self.builder.position_at_end(body);
        let element_addr = self.dynamic_byte_gep(buf, index, stride)?;
        self.emit_heap_glue(element, element_addr, glue)?;
        let one = i64_ty.const_int(1, false);
        let next = self
            .builder
            .build_int_add(index, one, "elem_next")
            .map_err(builder_err("incrementing the element index"))?;
        // The glue may have opened further blocks; the back-edge comes from
        // wherever the body actually ends.
        let body_end = self
            .builder
            .get_insert_block()
            .ok_or_else(|| CodegenError::backend("element loop body lost its block"))?;
        self.builder
            .build_unconditional_branch(header)
            .map_err(builder_err("closing the element loop"))?;
        phi.add_incoming(&[(&next, body_end)]);

        self.builder.position_at_end(exit);
        Ok(())
    }

    /// Free a heap buffer of `cap × stride` bytes at `ptr`, guarded on
    /// `cap != 0` (an empty sentinel is never freed).
    fn emit_buffer_free(
        &mut self,
        ptr: PointerValue<'ctx>,
        cap: IntValue<'ctx>,
        stride: u64,
        align: u64,
    ) -> Result<(), CodegenError> {
        let has_buffer = self
            .builder
            .build_int_compare(
                IntPredicate::NE,
                cap,
                self.ctx.i64_type().const_zero(),
                "has_buf",
            )
            .map_err(builder_err("checking the drop capacity"))?;
        let free_block = self.ctx.append_basic_block(self.value, "drop_free");
        let after = self.ctx.append_basic_block(self.value, "drop_after");
        self.builder
            .build_conditional_branch(has_buffer, free_block, after)
            .map_err(builder_err("branching on the drop capacity"))?;
        self.builder.position_at_end(free_block);
        let stride_c = self.ctx.i64_type().const_int(stride, false);
        let bytes = self
            .builder
            .build_int_mul(cap, stride_c, "drop_bytes")
            .map_err(builder_err("computing the drop byte size"))?;
        self.rt_dealloc(ptr, bytes, align)?;
        self.builder
            .build_unconditional_branch(after)
            .map_err(builder_err("joining after the drop free"))?;
        self.builder.position_at_end(after);
        Ok(())
    }

    // ----- traps -----

    /// If `condition` (an i1 or i8 boolean) is true, trap with `code`: branch to
    /// a fresh trap block that calls the runtime and reaches `unreachable`;
    /// otherwise continue in a fresh block. Leaves the builder positioned at the
    /// continuation.
    fn guard(&mut self, condition: IntValue<'ctx>, code: TrapCode) -> Result<(), CodegenError> {
        let cond_i1 = self.truthy(condition)?;
        let trap_block = self.ctx.append_basic_block(self.value, "trap");
        let continue_block = self.ctx.append_basic_block(self.value, "cont");
        self.builder
            .build_conditional_branch(cond_i1, trap_block, continue_block)
            .map_err(builder_err("branching to a trap check"))?;
        self.builder.position_at_end(trap_block);
        self.emit_trap_call(code)?;
        self.builder.position_at_end(continue_block);
        Ok(())
    }

    /// Emit a call to the runtime trap symbol with `code`, then `unreachable`
    /// (the call never returns). Terminates the current block.
    fn emit_trap_call(&mut self, code: TrapCode) -> Result<(), CodegenError> {
        let trap_fn = self.trap_function();
        let code_value = self.ctx.i32_type().const_int(code.as_i32() as u64, false);
        self.builder
            .build_call(trap_fn, &[code_value.into()], "")
            .map_err(builder_err("calling the runtime trap"))?;
        self.builder
            .build_unreachable()
            .map_err(builder_err("emitting unreachable after a trap"))?;
        Ok(())
    }

    /// Declare (once per module, on demand) and return the runtime trap symbol,
    /// a `void (i32)` external function.
    fn trap_function(&self) -> FunctionValue<'ctx> {
        if let Some(existing) = self.module.get_function(TRAP_SYMBOL) {
            return existing;
        }
        let fn_type = self
            .ctx
            .void_type()
            .fn_type(&[self.ctx.i32_type().into()], false);
        self.module
            .add_function(TRAP_SYMBOL, fn_type, Some(Linkage::External))
    }

    /// Coerce a scalar boolean value (an i1 or the backend's i8 `Bool`) to an
    /// i1 for use as a branch/select condition.
    fn truthy(&self, value: IntValue<'ctx>) -> Result<IntValue<'ctx>, CodegenError> {
        if value.get_type().get_bit_width() == 1 {
            return Ok(value);
        }
        let zero = value.get_type().const_zero();
        self.builder
            .build_int_compare(IntPredicate::NE, value, zero, "truthy")
            .map_err(builder_err("coercing a boolean to i1"))
    }

    // ----- operand type helpers -----

    /// The integer kind of a scalar operand, if it is an integer. Comparisons
    /// and arithmetic use it for signedness and bounds.
    fn operand_int_kind(&self, operand: &Operand) -> Result<IntKind, CodegenError> {
        match self.operand_ty(operand) {
            Some(Ty::Int(kind)) => Ok(kind),
            _ => Err(CodegenError::backend(
                "expected an integer operand in the arithmetic path",
            )),
        }
    }

    /// Whether an operand's integer type is signed (chars/bools compare
    /// unsigned).
    fn operand_signed(&self, operand: &Operand) -> bool {
        matches!(self.operand_ty(operand), Some(Ty::Int(kind)) if is_signed(kind))
    }

    /// The static type of an operand, from the local's declared type (following
    /// projections to the leaf field type) or the constant's kind. Non-primitive
    /// constants return `None`.
    fn operand_ty(&self, operand: &Operand) -> Option<Ty> {
        match operand {
            Operand::Copy(place) | Operand::Move(place) => Some(self.place_type(place)),
            Operand::Const(Const::Int(_, kind)) => Some(Ty::Int(*kind)),
            Operand::Const(Const::Float(_, kind)) => Some(Ty::Float(*kind)),
            Operand::Const(Const::Bool(_)) => Some(Ty::Bool),
            Operand::Const(Const::Char(_)) => Some(Ty::Char),
            Operand::Const(Const::Str(_)) => Some(Ty::Str),
            // A function-value constant (ADR-0008 Tier 1): its type is the
            // referenced function's signature-as-function-type, reconstructed
            // from that function's MIR. Copy propagation may forward a
            // `const fn` straight into an indirect callee position, so the
            // indirect-call path must be able to type it here.
            Operand::Const(Const::Fn(symbol)) => self.fn_ty_of(*symbol),
            _ => None,
        }
    }

    /// The `Ty::Fn` of the top-level function named `symbol`, reconstructed from
    /// its MIR signature (param modes+types and return type). Used to type a
    /// `Const::Fn` operand — in particular an indirect callee that copy
    /// propagation forwarded a `const fn` into. `None` if the function is not in
    /// the lowered program.
    fn fn_ty_of(&self, symbol: SymbolId) -> Option<Ty> {
        let function = self.functions.iter().find(|f| f.symbol == symbol)?;
        let params = function
            .params
            .iter()
            .enumerate()
            .map(|(index, mode)| tuo_types::FnParam {
                mode: param_mode_of(*mode),
                ty: function.locals[index].ty.clone(),
            })
            .collect();
        Some(Ty::Fn(Box::new(tuo_types::FnTy {
            params,
            ret: function.ret.clone(),
        })))
    }

    // ----- aggregate helpers -----

    /// The declared type of `place`, following its projections to the leaf.
    fn place_type(&self, place: &Place) -> Ty {
        let mut cur = self.function.locals[place.local.0 as usize].ty.clone();
        for step in &place.projection {
            cur = match step {
                Projection::Field(i) => self.field_type(&cur, *i as usize).unwrap_or(Ty::Error),
                Projection::VariantField { variant, field } => self
                    .variant_field_type(&cur, *variant as usize, *field as usize)
                    .unwrap_or(Ty::Error),
                // Indexing yields the element type (growable or fixed; only
                // the fixed `[T; N]` is lowered, but the type walk is total).
                Projection::Index(_) => match cur {
                    Ty::Array(element) | Ty::FixedArray(element, _) => (*element).clone(),
                    _ => Ty::Error,
                },
            };
        }
        cur
    }

    /// The i-th field type of a struct/tuple `cur_ty` (declaration order).
    fn field_type(&self, cur_ty: &Ty, i: usize) -> Result<Ty, CodegenError> {
        match cur_ty {
            Ty::Tuple(fields) => fields
                .get(i)
                .cloned()
                .ok_or_else(|| CodegenError::backend("tuple field index out of range")),
            Ty::Struct(symbol, _) => {
                let shape = self
                    .types
                    .struct_shape(*symbol)
                    .ok_or_else(|| CodegenError::backend("field of an unknown struct"))?;
                shape
                    .fields
                    .get(i)
                    .map(|(_, ty)| ty.clone())
                    .ok_or_else(|| CodegenError::backend("struct field index out of range"))
            }
            _ => Err(CodegenError::backend(
                "field projection on a non-struct, non-tuple type",
            )),
        }
    }

    /// The payload field type of variant `variant`, field `field` of an
    /// enum-like `cur_ty`.
    fn variant_field_type(
        &self,
        cur_ty: &Ty,
        variant: usize,
        field: usize,
    ) -> Result<Ty, CodegenError> {
        match cur_ty {
            Ty::Enum(symbol, _) => {
                let shape = self
                    .types
                    .enum_shape(*symbol)
                    .ok_or_else(|| CodegenError::backend("variant field of an unknown enum"))?;
                let (_, payload) = shape
                    .variants
                    .get(variant)
                    .ok_or_else(|| CodegenError::backend("enum variant index out of range"))?;
                payload
                    .get(field)
                    .map(|(_, ty)| ty.clone())
                    .ok_or_else(|| CodegenError::backend("variant field index out of range"))
            }
            // `Some` = variant 0 (payload at field 0), `None` = variant 1
            // (empty), matching the interpreter/MIR and the runtime ABI.
            Ty::Option(inner) => match (variant, field) {
                (0, 0) => Ok((**inner).clone()),
                _ => Err(CodegenError::backend("bad Option variant field")),
            },
            Ty::Result(ok, err) => match (variant, field) {
                (0, 0) => Ok((**ok).clone()),
                (1, 0) => Ok((**err).clone()),
                _ => Err(CodegenError::backend("bad Result variant field")),
            },
            _ => Err(CodegenError::backend(
                "variant-field projection on a non-enum type",
            )),
        }
    }

    /// Whether an operand refers to an aggregate value: a place whose leaf type
    /// (the declared type of a bare local, or the projected leaf — e.g. a
    /// struct-typed array element) classifies as aggregate storage, or a `Str`
    /// literal (a two-word aggregate materialized from static data,
    /// ADR-0006 Stage B).
    fn operand_is_aggregate(&self, operand: &Operand) -> bool {
        match operand {
            Operand::Copy(place) | Operand::Move(place) => {
                matches!(
                    classify_storage(&self.place_type(place), self.types, &self.function.name),
                    Ok(Storage::Aggregate(_))
                )
            }
            Operand::Const(Const::Str(_)) => true,
            _ => false,
        }
    }

    /// Whether an operand is unit-valued (a unit constant or a bare-local place
    /// of unit/zero-sized type).
    fn operand_is_unit(&self, operand: &Operand) -> bool {
        match operand {
            Operand::Const(Const::Unit) => true,
            Operand::Copy(place) | Operand::Move(place) if place.projection.is_empty() => {
                matches!(
                    classify_storage(
                        &self.function.locals[place.local.0 as usize].ty,
                        self.types,
                        &self.function.name,
                    ),
                    Ok(Storage::Unit)
                )
            }
            _ => false,
        }
    }

    /// The base pointer of the aggregate an operand names: a `Copy`/`Move` of a
    /// bare aggregate local, or of a projected place whose leaf is an aggregate
    /// (e.g. a struct-typed array element), resolved through the shared address
    /// walk. A move leaves the source husk untouched: a v0 aggregate owns no
    /// heap, so no zeroing is needed (the ownership checker forbids re-reading
    /// a moved value).
    fn operand_aggregate_address(
        &mut self,
        operand: &Operand,
    ) -> Result<PointerValue<'ctx>, CodegenError> {
        match operand {
            Operand::Copy(place) | Operand::Move(place) if place.projection.is_empty() => {
                match &self.kinds[place.local.0 as usize] {
                    LocalKind::Aggregate { ptr, .. } => Ok(*ptr),
                    // A whole-aggregate read of a borrowed parameter copies
                    // from the caller's memory through the incoming pointer.
                    LocalKind::Borrowed { .. } => self.borrowed_addr(place.local.0 as usize),
                    _ => Err(CodegenError::backend(
                        "aggregate address requested for a non-aggregate local",
                    )),
                }
            }
            Operand::Copy(place) | Operand::Move(place) => {
                let (addr, _leaf) = self.place_address(place)?;
                Ok(addr)
            }
            // A `Str` literal (ADR-0006 Stage B): materialize its `{ptr, len}`
            // fat pointer into a fresh temporary alloca and hand that alloca's
            // address to the aggregate machinery, so a literal flows through
            // copies, call arguments, and returns exactly like any aggregate
            // place. Mirrors the Cranelift backend's temporary-slot arm.
            Operand::Const(Const::Str(text)) => {
                let text = text.clone();
                let layout = self.layout(&Ty::Str)?;
                let addr = self.alloca_aggregate(layout, usize::MAX)?;
                let (ptr, len) = self.str_const_parts(&text)?;
                self.builder
                    .build_store(addr, ptr)
                    .map_err(builder_err("storing a Str literal's data pointer"))?;
                let len_addr = self.byte_gep(addr, STR_LEN_OFFSET)?;
                self.builder
                    .build_store(len_addr, len)
                    .map_err(builder_err("storing a Str literal's length"))?;
                Ok(addr)
            }
            Operand::Const(_) => Err(CodegenError::backend(
                "aggregate address requested for a non-place operand",
            )),
        }
    }

    /// The runtime ABI layout of `ty`, or a backend error (the type was already
    /// classified as an aggregate, so this should not fail).
    fn layout(&self, ty: &Ty) -> Result<Layout, CodegenError> {
        layout_of(ty, self.types).map_err(|error| self.layout_error(error))
    }

    /// Turn an ABI layout error into a backend error.
    fn layout_error(&self, error: tuo_runtime::abi::LayoutError) -> CodegenError {
        CodegenError::backend(format!("ABI layout failed during lowering: {error}"))
    }

    /// Allocate an aggregate slot of `layout`: an `[size x i8]` alloca whose
    /// alignment is forced to the ABI alignment (LLVM's default alloca align may
    /// be smaller, which would make aligned field loads UB under O2). `index` is
    /// only for a readable name.
    fn alloca_aggregate(
        &self,
        layout: Layout,
        index: usize,
    ) -> Result<PointerValue<'ctx>, CodegenError> {
        let i8 = self.ctx.i8_type();
        // The element count must be a pointer-width constant: an i8-typed count
        // would silently truncate any aggregate larger than 255 bytes.
        let size = self.ctx.i64_type().const_int(layout.size, false);
        let name = if index == usize::MAX {
            "agg_tmp".to_owned()
        } else {
            format!("agg{index}")
        };
        let ptr = self
            .builder
            .build_array_alloca(i8, size, &name)
            .map_err(builder_err("allocating an aggregate slot"))?;
        // Force the alloca's alignment to the ABI alignment.
        let align = u32::try_from(layout.align)
            .map_err(|_| CodegenError::backend("aggregate alignment exceeds u32"))?;
        ptr.as_instruction()
            .ok_or_else(|| CodegenError::backend("aggregate alloca has no instruction"))?
            .set_alignment(align)
            .map_err(|error| {
                CodegenError::backend(format!("setting aggregate alloca alignment: {error}"))
            })?;
        Ok(ptr)
    }

    /// Copy `layout.size` bytes from `src` to `dest` (both aggregate pointers)
    /// using the aggregate's ABI alignment. A zero-sized copy is a no-op.
    fn emit_memcpy(
        &self,
        dest: PointerValue<'ctx>,
        src: PointerValue<'ctx>,
        layout: Layout,
    ) -> Result<(), CodegenError> {
        if layout.size == 0 {
            return Ok(());
        }
        let align = u32::try_from(layout.align)
            .map_err(|_| CodegenError::backend("aggregate alignment exceeds u32"))?;
        let size = self.ctx.i64_type().const_int(layout.size, false);
        self.builder
            .build_memcpy(dest, align, src, align, size)
            .map_err(builder_err("copying an aggregate"))?;
        Ok(())
    }
}

/// A trapping arithmetic operation.
#[derive(Clone, Copy)]
enum ArithOp {
    Add,
    Sub,
    Mul,
}

/// The minimum value of an integer kind as an LLVM constant of type `ty`.
///
/// For a signed kind this is `1 << (bits-1)` (the two's-complement minimum); for
/// an unsigned kind it is `0`. Only the negation/div overflow checks consult it,
/// exactly where the Cranelift backend consults its `int_bounds` minimum.
fn min_const<'ctx>(ty: IntType<'ctx>, kind: IntKind) -> IntValue<'ctx> {
    if is_signed(kind) {
        let bits = ty.get_bit_width();
        // MIN = 1 << (bits - 1), the sign bit set and all others clear.
        ty.const_int(1u64 << (bits - 1), false)
    } else {
        ty.const_zero()
    }
}

/// The LLVM condition predicate for a comparison operator, or `None` if `op` is
/// not a comparison. `signed` selects signed vs unsigned ordering.
fn comparison_pred(op: BinOp, signed: bool) -> Option<IntPredicate> {
    Some(match op {
        BinOp::Eq => IntPredicate::EQ,
        BinOp::Ne => IntPredicate::NE,
        BinOp::Lt if signed => IntPredicate::SLT,
        BinOp::Lt => IntPredicate::ULT,
        BinOp::Le if signed => IntPredicate::SLE,
        BinOp::Le => IntPredicate::ULE,
        BinOp::Gt if signed => IntPredicate::SGT,
        BinOp::Gt => IntPredicate::UGT,
        BinOp::Ge if signed => IntPredicate::SGE,
        BinOp::Ge => IntPredicate::UGE,
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => return None,
    })
}

/// The LLVM float predicate for a comparison operator, or `None` if `op` is
/// not a comparison. Equality and the four orderings are **ordered** (`O*`,
/// false when either operand is NaN) and inequality is
/// **unordered-or-unequal** (`UNE`, true on NaN) — exactly Rust's float
/// comparisons, which the interpreter uses. Mirrors the Cranelift backend's
/// `float_comparison_code`.
fn float_comparison_pred(op: BinOp) -> Option<FloatPredicate> {
    Some(match op {
        BinOp::Eq => FloatPredicate::OEQ,
        BinOp::Ne => FloatPredicate::UNE,
        BinOp::Lt => FloatPredicate::OLT,
        BinOp::Le => FloatPredicate::OLE,
        BinOp::Gt => FloatPredicate::OGT,
        BinOp::Ge => FloatPredicate::OGE,
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => return None,
    })
}

/// The runtime trap code for a MIR terminator trap.
fn trap_code_of(trap: Trap) -> TrapCode {
    match trap {
        Trap::IndexOutOfBounds => TrapCode::IndexOutOfBounds,
        Trap::Unreachable => TrapCode::Unreachable,
    }
}

/// Whether a `Rvalue::HeapOp` produces an aggregate value — an owned
/// `String`/`Array` three-word header, or `as_str`'s borrowed two-word `Str`
/// view — materialized by `lower_heap_op_aggregate`, rather than a scalar
/// `i64` (the reads, taken by `lower_heap_op_scalar`). Matches the Cranelift
/// backend and the interpreter's split in `eval_heap_op`.
fn heap_op_produces_aggregate(op: HeapOp) -> bool {
    matches!(
        op,
        HeapOp::StringEmpty
            | HeapOp::StringFromStr
            | HeapOp::StringConcat
            | HeapOp::StringSlice
            | HeapOp::StringAsStr
            | HeapOp::ArrayEmpty
            | HeapOp::MapEmpty
            | HeapOp::MapGet
            | HeapOp::MapKeys
    )
}

/// The dense-entry stride of a map's table, fixed by its key kind
/// (ADR-0011): `{key, value}` two words for an `Int` key, `{ptr, len, value}`
/// three words for a `Str` key. Mirrors `tuo_runtime::map`'s constants.
fn map_entry_stride(key: &Ty) -> u64 {
    if matches!(key, Ty::Str) {
        map::STR_ENTRY_STRIDE
    } else {
        map::INT_ENTRY_STRIDE
    }
}

/// The exported symbol name of a MIR function, from its stable symbol id.
///
/// Identical to the Cranelift backend's mangling (`tuo_fn_<n>`), so a program
/// built by either backend uses the same internal symbol names — and the same
/// `main` shim entry point.
fn mangle(symbol: SymbolId) -> String {
    format!("tuo_fn_{}", symbol.as_u32())
}

/// Map an inkwell [`BuilderError`](inkwell::builder::BuilderError) to a backend
/// [`CodegenError`], naming the operation that failed. A builder error here is a
/// backend fault (misplaced builder, malformed IR request), never a user error.
fn builder_err(what: &'static str) -> impl Fn(inkwell::builder::BuilderError) -> CodegenError {
    move |error| CodegenError::backend(format!("LLVM builder failed while {what}: {error}"))
}
