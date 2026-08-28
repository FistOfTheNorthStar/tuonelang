//! MIR → Cranelift IR lowering for the v0 scalar core.
//!
//! [`lower_program`] declares every function (so direct calls resolve), then
//! defines each body by walking its basic blocks and translating each
//! statement and terminator to Cranelift IR that computes exactly what the
//! reference interpreter computes. Anything outside the scalar subset is
//! reported as [`CodegenError::unsupported`]; the lowering never emits code
//! whose meaning it is unsure of.
//!
//! # Trapping semantics
//!
//! Integer arithmetic traps on overflow, and division/remainder trap on a zero
//! divisor and on `MIN / -1` (Constitution §24). Each check is lowered as a
//! branch to a dedicated trap block that calls the runtime
//! [`TRAP_SYMBOL`](tuo_runtime::TRAP_SYMBOL) with the matching
//! [`TrapCode`](tuo_runtime::TrapCode) and then reaches an unreachable
//! terminator — the runtime call never returns. This mirrors the interpreter's
//! deterministic abort.
//!
//! # Locals
//!
//! Each MIR local is classified once, up front (see [`LocalKind`]):
//!
//! - a **scalar** local (bool/char/int/float) becomes a Cranelift [`Variable`],
//!   read and written by index, held in a register;
//! - a scalar local whose address a **borrow-mode call argument** takes is
//!   demoted to an explicit [`StackSlot`] (reads load, writes store), so the
//!   callee's pointer aliases real caller memory;
//! - a **unit** local carries no value;
//! - an **aggregate** local (a Stage-1 product type — struct/tuple/enum whose
//!   transitive fields are all scalars — or an ADR-0004 Stage 2 fixed array
//!   `[T; N]`) gets one explicit [`StackSlot`], and its fields/elements are
//!   accessed by computing a byte address into the slot from the runtime ABI's
//!   offsets (see [`tuo_runtime::abi`]);
//! - a **borrow-mode (`in`/`mut`) parameter** is a pointer to caller-owned
//!   memory; reads and writes go through the pointer directly.
//!
//! A local whose type is a heap **wrapper** (`Box`/`Shared`/`Weak`) still makes
//! the whole function unsupported (they await a later ADR). The owned `String`
//! and the growable `Array[Int]`, by contrast, are lowered since ADR-0009 Stage
//! B: their three-word `{ptr, len, cap}` header is an ordinary aggregate held in
//! a slot (moved by memcpy of the header, passed by-pointer, returned by sret),
//! and the buffer it points at is real heap memory acquired through
//! `tuo_rt_alloc` and freed by the `Drop` glue — see the "Heap values" section
//! below. Aggregate lowering follows ADR-0004: scalar
//! leaves, by-pointer/sret call ABI. Fixed arrays are laid out inline —
//! element `i` at `i × stride(T)` — and indexed by unchecked address
//! arithmetic, because MIR asserts the bounds (`Assert { IndexOutOfBounds }`)
//! before every `Projection::Index` use.
//!
//! # Heap values (ADR-0009 Stage B)
//!
//! An owned `String` and a growable `Array[Int]` are three-word
//! `{ptr, len, cap}` headers (`specification/abi.md`) whose header lives in a
//! slot and whose buffer is separate heap memory. The lowering matches the
//! reference interpreter's `eval_heap_op`/`exec_heap_mutate` operation for
//! operation:
//!
//! - **Empty** (`string_empty`/`array_empty`) writes `{ptr = ZERO_SIZE_SENTINEL,
//!   len = 0, cap = 0}` — a fixed non-null pointer never dereferenced (`len` is
//!   0) and never freed (`cap` is 0), the same sentinel discipline empty `Str`
//!   literals use.
//! - **Constructors** (`from_str`, `concat`, `slice`, and every grow) call
//!   `tuo_rt_alloc(bytes, align)` and `memcpy` the source bytes in.
//! - **Growth** (`push`/`append`/`push_byte`) is *alloc-new + copy + dealloc-old*
//!   in this backend — the C shim stays acquire/release only. The new capacity
//!   is `max(1, cap * 2)` (or exactly the needed length for a bulk `append`);
//!   the old buffer is freed only when the old `cap != 0` (never the sentinel).
//! - **Reads** (`len`, `byte_at`, `get`) load the `len`/element, bounds-checking
//!   `byte_at`/`get`/`string_byte_at`/`array_get` through the shared `guard()`
//!   `IndexOutOfBounds` path exactly as the interpreter traps.
//! - `push_byte` traps `InvalidByte` (via the same guard path) when the byte is
//!   outside `0..=255`, before touching any memory.
//! - **`Drop`** of a `String`/`Array[Int]` frees the buffer with
//!   `tuo_rt_dealloc(ptr, cap × stride, align)`, guarded on `cap != 0`.
//!
//! Only `len`, contents, and `pop`'s `Option` are observable; the capacity and
//! the buffer's identity are not, so any doubling policy agrees with the
//! interpreter. A move de-initializes the moved-from place in MIR, so the header
//! is memcpy'd once and freed exactly once — no double-free.
//!
//! # Strings and effects (ADR-0006 Stage B)
//!
//! A `Str` is an ordinary two-word aggregate — the `{u8 *ptr, usize len}` fat
//! pointer of `specification/abi.md` ("Slices"), laid out by
//! [`tuo_runtime::abi::layout_of`] — and flows through the existing aggregate
//! machinery unchanged: stack slots, memcpy moves, by-pointer `take`
//! parameters, sret returns, borrow-mode pointers. A `Const::Str`'s bytes are
//! emitted once per module into read-only static data (identical literals
//! deduplicated) and the constant materializes as `{data address, len}`; an
//! empty literal carries `len = 0` and a fixed non-null pointer that is never
//! dereferenced. `Str` equality is byte-wise (lengths equal AND bytes equal,
//! via the C library's `memcmp` when the lengths match), the `std::str` byte
//! operations ([`Rvalue::StrOp`]) trap `IndexOutOfBounds` exactly as the
//! interpreter's `eval_str_op` does, and a host effect
//! ([`Statement::Effect`]) is a direct call to the matching
//! [`tuo_runtime::effect`] symbol (`tuo_rt_write`/`tuo_rt_read_byte`/
//! `tuo_rt_exit` — the last never returns, so the block is terminated with
//! the same unreachable shape the trap path uses).
//!
//! # Borrow-mode calling convention
//!
//! Pinned here and implemented **identically** in the LLVM backend (see
//! `specification/abi.md`, "Passing modes"):
//!
//! - the **caller** passes the ADDRESS of the argument place as a pointer
//!   argument — for a scalar root local that local is demoted to slot storage
//!   so it has an address; an aggregate already lives in a slot;
//! - the **callee** receives that pointer and reads/writes through it
//!   directly: **no copy-in and no copy-back**. The interpreter's
//!   copy-in/copy-back is observably identical because the borrow checker
//!   forbids aliasing (any number of `in` XOR one `mut`) and the borrow lives
//!   only for the call;
//! - forwarding a borrowed parameter as another `in`/`mut` argument passes the
//!   pointer value itself;
//! - a unit-typed borrow occupies no ABI slot, like every unit value;
//! - `take` parameters and returns are unchanged.

use std::collections::{HashMap, HashSet};

use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::stackslot::{StackSlotData, StackSlotKind};
use cranelift_codegen::ir::{
    AbiParam, InstBuilder, MemFlags, SigRef, Signature, StackSlot, Type, Value as ClifValue, types,
};
use cranelift_codegen::isa::{CallConv, TargetFrontendConfig};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Switch, Variable};
use cranelift_module::{DataDescription, DataId, FuncId, Linkage, Module};
use cranelift_object::ObjectModule;

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
use crate::{CodegenCtx, FUNCTION_LINKAGE};

/// The byte offset of the `len` word inside a `Str` fat pointer — one pointer
/// word past the data pointer (`{u8 *ptr, usize len}`, `specification/abi.md`
/// "Slices").
const STR_LEN_OFFSET: i32 = POINTER_SIZE as i32;

/// The byte offset of the `ptr` word inside a `String`/`Array` header
/// (`{ptr, len, cap}`, `specification/abi.md`).
const HDR_PTR_OFFSET: i32 = 0;
/// The byte offset of the `len` word inside a `String`/`Array` header.
const HDR_LEN_OFFSET: i32 = POINTER_SIZE as i32;
/// The byte offset of the `cap` word inside a `String`/`Array` header.
const HDR_CAP_OFFSET: i32 = 2 * POINTER_SIZE as i32;

/// How a MIR local is stored by the backend, decided once up front from its
/// declared type. This is the third outcome added to the original scalar/unit
/// pair: an aggregate local backed by an explicit stack slot.
enum LocalKind {
    /// A scalar (bool/char/int/float) held in an SSA [`Variable`] of the given
    /// type.
    Scalar(Variable, Type),
    /// A scalar local forced into an explicit stack slot because a borrow-mode
    /// call argument takes its address. Reads load and writes store through the
    /// slot, so a callee's pointer aliases this local's real memory.
    ScalarSlot(StackSlot, Type),
    /// A `Unit` local (or a zero-sized aggregate): carries no value, no slot.
    Unit,
    /// A Stage-1 aggregate held in an explicit stack slot; `ty` is the local's
    /// declared type, from which field offsets are computed via the ABI.
    Aggregate {
        /// The stack slot holding the aggregate's bytes.
        slot: StackSlot,
        /// The aggregate's declared type (source of truth for offsets).
        ty: Ty,
    },
    /// A borrow-mode (`in`/`mut`) parameter: a pointer to caller-owned memory,
    /// held in a pointer-typed [`Variable`]. Scalar reads load through the
    /// pointer, `mut` scalar writes store through it, projections use it as the
    /// base address, and forwarding it as another borrow argument passes the
    /// pointer value itself. There is **no** copy-in and **no** copy-back.
    Borrowed {
        /// The pointer-typed variable holding the incoming address.
        var: Variable,
        /// The parameter's declared type (source of truth for offsets).
        ty: Ty,
    },
}

/// The storage classification of a local, from its declared type and the ABI.
///
/// Mirrors the LLVM backend's `classify_storage` exactly so the two backends make
/// identical ABI choices. Returns:
/// - `Ok(None)` — a scalar (the caller resolves its register type) or unit;
/// - `Ok(Some(layout))` — a Stage-1 aggregate with a non-zero layout;
/// - `Err(..)` — the local's type has no v0 layout (Stage 2 / unsupported).
///
/// A type that `scalar_type` maps is a scalar; `Ty::Unit` is unit; anything else
/// whose [`layout_of`] succeeds and is non-zero is an aggregate; a zero-sized
/// aggregate is treated as unit (matching the interpreter's `Value::Unit`).
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
    if scalar_type(ty).is_some() {
        return Ok(Storage::Scalar);
    }
    if matches!(ty, Ty::Unit) {
        return Ok(Storage::Unit);
    }
    // Not scalar, not unit: it must be an aggregate with a real layout — a
    // scalar-leaf product type (Stage 1), a fixed array `[T; N]` (Stage 2),
    // or the `Str` fat pointer (ADR-0006 Stage B).
    match layout_of(ty, types) {
        Ok(layout) if layout.size == 0 => Ok(Storage::Unit),
        Ok(layout) => Ok(Storage::Aggregate(layout)),
        Err(error) => Err(CodegenError::unsupported(format!(
            "`{context}` uses a type the Cranelift backend does not lower yet: {error}"
        ))),
    }
}

/// The clean refusal message for a heap-owning type the backend does not lower
/// yet, or `None` if `ty` is not one. Mirrors the LLVM backend's
/// `heap_type_refusal` word for word (bar the backend name), so the two
/// backends refuse the same boundary with the same explanation. `Str` is no
/// longer here (ADR-0006 Stage B lowers it as a two-word aggregate), and since
/// ADR-0009 Stage B neither are the owned `String` and the growable
/// `Array[Int]` — they are three-word `{ptr, len, cap}` aggregates backed by
/// real heap memory (the buffer the header points at). Only the memory wrappers
/// (`Box`/`Shared`/`Weak`) remain refused, awaiting a later ADR.
fn heap_type_refusal(ty: &Ty, context: &str) -> Option<String> {
    match ty {
        Ty::Wrapper(kind, _) => Some(format!(
            "`{context}` uses a `{}[T]` heap wrapper, which the Cranelift backend does not \
             lower yet (heap wrappers await a later ADR); the interpreter \
             (`tuo spec`/`tuo verify`) remains the reference",
            kind.name()
        )),
        _ => None,
    }
}

/// Whether `ty` transitively **owns heap** — a `String`, a growable `Array`, a
/// `Box`/`Shared`/`Weak` wrapper, or a struct/enum any of whose fields does.
/// Such a type needs a deep copy on read-out (`emit_heap_glue` with
/// [`HeapGlue::DeepFixup`]) and recursive drop glue ([`HeapGlue::DropInPlace`]),
/// matching the interpreter's `Value::clone` and de-initializing drop.
/// `Str` is **not** heap-owning (it is a borrowed fat pointer), so an
/// `Array[Str]` needs neither.
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

/// Substitute a struct/enum's type parameters into a field type. A minimal
/// positional substitution mirroring the type checker's `substitute`.
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
/// longer refused.
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
/// The type checker widened `Array[T]` to more element types (ADR-0012), and
/// since the owned-element increment the native path lowers the whole
/// checker-accepted set: no-heap elements (`Int`/`Bool`/`Str`/plain aggregates)
/// via element-size-aware load/store and memcpy, and heap-owning elements
/// (`String`, a struct/enum carrying one) with a deep copy on `get` and
/// per-element drop glue (`emit_heap_glue`), matching the interpreter's
/// `Value::clone` semantics. Only an element containing a `Box`/`Shared`/`Weak`
/// heap wrapper is still refused — wrapper values are not lowered anywhere
/// (their own ADR) — with an honest `unsupported`, never a mis-compile.
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
/// they can never disagree about what owns a buffer.
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

/// Declare then define every lowerable function of `program` into `module`.
///
/// # Errors
///
/// [`CodegenError::unsupported`] if any function reachable in the program uses
/// a feature outside the v0 scalar subset; [`CodegenError::backend`] on a
/// Cranelift failure.
pub(crate) fn lower_program(
    module: &mut ObjectModule,
    program: &Program,
    types: &TypeckResult,
) -> Result<HashMap<SymbolId, FuncId>, CodegenError> {
    // Pass 1: declare every function so direct calls can reference them before
    // their bodies are defined.
    let mut ids: HashMap<SymbolId, FuncId> = HashMap::new();
    for function in &program.functions {
        let signature = function_signature(module, function, types)?;
        let symbol_name = mangle(function.symbol);
        let id = module
            .declare_function(&symbol_name, FUNCTION_LINKAGE, &signature)
            .map_err(|error| {
                CodegenError::backend(format!("declaring `{}`: {error}", function.name))
            })?;
        ids.insert(function.symbol, id);
    }

    // Pass 2: define each body. The string-literal data pool is shared across
    // bodies so identical literals dedupe to one static data object per module.
    let mut ctx = CodegenCtx::new(module);
    let mut builder_ctx = FunctionBuilderContext::new();
    let mut str_data: HashMap<Vec<u8>, DataId> = HashMap::new();
    for function in &program.functions {
        define_function(
            module,
            &mut ctx,
            &mut builder_ctx,
            &ids,
            function,
            &program.functions,
            types,
            &mut str_data,
        )?;
    }
    Ok(ids)
}

/// The Cranelift signature of a MIR function (v0 ABI, Stage-1 aggregates).
///
/// Applies the two aggregate calling-convention rules of ADR-0004 Stage 1,
/// identically to the LLVM backend's `function_type`:
/// - an **aggregate return** is an sret hidden out-pointer, *prepended* as
///   argument index 0, and the native return type becomes void;
/// - an **aggregate parameter** is passed as a pointer to a caller-owned copy.
///
/// Every size/align derives from [`tuo_runtime::abi`], never from Cranelift's
/// native small-struct classification.
fn function_signature(
    module: &ObjectModule,
    function: &Function,
    types: &TypeckResult,
) -> Result<Signature, CodegenError> {
    let params = function
        .params
        .iter()
        .enumerate()
        .map(|(index, mode)| (*mode, function.locals[index].ty.clone()));
    signature_from_parts(
        module.isa().default_call_conv(),
        module.isa().pointer_type(),
        &function.ret,
        params,
        types,
        &function.name,
    )
}

/// Build the Cranelift signature for a `(ret, [(mode, ty)])` calling contract.
///
/// This is the single source of truth for the v0 call ABI, shared by the direct
/// path ([`function_signature`], over a `Function`'s declared params) and the
/// indirect path (over the callee value's `Ty::Fn` modes+types), so the two can
/// never drift — ADR-0008 Stage B requires the indirect-call convention to be
/// byte-identical to the direct one. Applies the two aggregate rules: an
/// aggregate return is an sret out-pointer prepended at index 0 (return becomes
/// void), and an aggregate/borrow parameter is passed by pointer. `context`
/// names the owner for a diagnostic on a non-scalar-by-value type.
fn signature_from_parts(
    call_conv: CallConv,
    pointer_type: Type,
    ret: &Ty,
    params: impl Iterator<Item = (PassMode, Ty)>,
    types: &TypeckResult,
    context: &str,
) -> Result<Signature, CodegenError> {
    let mut signature = Signature::new(call_conv);

    // Classify the return: an aggregate return prepends an sret pointer and the
    // function returns void; a scalar return is by value; unit is no value.
    let ret_storage = classify_storage(ret, types, context)?;
    if matches!(ret_storage, Storage::Aggregate(_)) {
        // sret hidden out-pointer is ALWAYS argument index 0 (prepended).
        signature.params.push(AbiParam::new(pointer_type));
    }

    // Parameters, in declaration order after the (optional) sret pointer.
    // Borrow-mode calling convention (identical in the LLVM backend's
    // `function_type`, per `specification/abi.md` "Passing modes"): an
    // `in`/`mut` parameter arrives as a **pointer to the caller's place** —
    // scalar or aggregate alike — read (and, for `mut`, written) through
    // directly, with no copy-in and no copy-back. A `take` parameter is
    // unchanged: scalar by value, aggregate by pointer to a caller-owned copy,
    // unit occupying no ABI slot (in any mode).
    for (mode, ty) in params {
        let storage = classify_storage(&ty, types, context)?;
        match (mode, storage) {
            (_, Storage::Unit) => {}
            (PassMode::Value, Storage::Scalar) => signature
                .params
                .push(AbiParam::new(require_scalar(&ty, context)?)),
            (PassMode::Value, Storage::Aggregate(_))
            | (PassMode::Borrow | PassMode::BorrowMut, _) => {
                signature.params.push(AbiParam::new(pointer_type));
            }
        }
    }

    // Return value: scalar by value; aggregate is void (written through sret);
    // unit is no value.
    match ret_storage {
        Storage::Scalar => signature
            .returns
            .push(AbiParam::new(require_scalar(ret, context)?)),
        Storage::Unit | Storage::Aggregate(_) => {}
    }
    Ok(signature)
}

/// The MIR [`PassMode`] a function-type parameter's [`ParamMode`] corresponds
/// to. A function value's per-argument borrow discipline at an indirect call
/// site is driven by these modes exactly as a direct call's is by its
/// declared `PassMode`s (ADR-0008 Tier 1); the two vocabularies map one-to-one.
fn pass_mode_of(mode: ParamMode) -> PassMode {
    match mode {
        ParamMode::Take => PassMode::Value,
        ParamMode::In => PassMode::Borrow,
        ParamMode::Mut => PassMode::BorrowMut,
    }
}

/// The function-type [`ParamMode`] a MIR [`PassMode`] corresponds to — the
/// inverse of [`pass_mode_of`]. Used to reconstruct a `Const::Fn`'s
/// `Ty::Fn` from the referenced function's MIR signature.
fn param_mode_of(mode: PassMode) -> ParamMode {
    match mode {
        PassMode::Value => ParamMode::Take,
        PassMode::Borrow => ParamMode::In,
        PassMode::BorrowMut => ParamMode::Mut,
    }
}

/// The scalar Cranelift type of `ty`, or an unsupported error naming `context`.
fn require_scalar(ty: &Ty, context: &str) -> Result<Type, CodegenError> {
    scalar_type(ty).ok_or_else(|| {
        CodegenError::unsupported(format!(
            "`{context}` uses a non-scalar type the Cranelift backend does not lower yet"
        ))
    })
}

/// Define one function body.
#[expect(
    clippy::too_many_arguments,
    reason = "a private plumbing seam between `lower_program` and `Lowering::new`; bundling \
              these into a context struct would only move the argument list"
)]
fn define_function(
    module: &mut ObjectModule,
    ctx: &mut CodegenCtx,
    builder_ctx: &mut FunctionBuilderContext,
    ids: &HashMap<SymbolId, FuncId>,
    function: &Function,
    functions: &[Function],
    types: &TypeckResult,
    str_data: &mut HashMap<Vec<u8>, DataId>,
) -> Result<(), CodegenError> {
    let signature = function_signature(module, function, types)?;
    let self_id = ids[&function.symbol];

    ctx.context_mut().func.signature = signature;
    let mut lowering = Lowering::new(
        module,
        ctx.context_mut(),
        builder_ctx,
        ids,
        function,
        functions,
        types,
        str_data,
    )?;
    lowering.run()?;
    lowering.finish();

    module
        .define_function(self_id, ctx.context_mut())
        .map_err(|error| CodegenError::backend(format!("defining `{}`: {error}", function.name)))?;
    ctx.clear(module);
    Ok(())
}

/// The per-function lowering state.
struct Lowering<'a> {
    builder: FunctionBuilder<'a>,
    module: &'a mut ObjectModule,
    ids: &'a HashMap<SymbolId, FuncId>,
    function: &'a Function,
    /// Every function in the program, so a direct call can read its callee's
    /// return type and name for the aggregate call ABI.
    functions: &'a [Function],
    types: &'a TypeckResult,
    /// The pointer type of the target (for slot addresses and aggregate args).
    pointer_type: Type,
    /// The frontend config, needed by `emit_small_memory_copy`.
    frontend_config: TargetFrontendConfig,
    /// The Cranelift block for each MIR block index.
    blocks: Vec<cranelift_codegen::ir::Block>,
    /// How each MIR local is stored (index = local id): scalar variable, unit,
    /// or aggregate stack slot. Filled during `run` once the builder exists.
    kinds: Vec<LocalKind>,
    /// The module's string-literal data pool: the static data object emitted
    /// for each distinct literal's bytes, shared across function bodies so
    /// identical literals dedupe (ADR-0006 Stage B).
    str_data: &'a mut HashMap<Vec<u8>, DataId>,
}

impl<'a> Lowering<'a> {
    #[expect(
        clippy::too_many_arguments,
        reason = "a private plumbing seam mirroring `define_function`; bundling these into a \
                  context struct would only move the argument list"
    )]
    fn new(
        module: &'a mut ObjectModule,
        context: &'a mut cranelift_codegen::Context,
        builder_ctx: &'a mut FunctionBuilderContext,
        ids: &'a HashMap<SymbolId, FuncId>,
        function: &'a Function,
        functions: &'a [Function],
        types: &'a TypeckResult,
        str_data: &'a mut HashMap<Vec<u8>, DataId>,
    ) -> Result<Self, CodegenError> {
        // Classify every local up front so an unsupported (Stage-2) type fails
        // before any IR is built. The concrete slots/variables are created in
        // `run`, once the builder exists.
        for local in &function.locals {
            classify_storage(&local.ty, types, &function.name)?;
        }

        let pointer_type = module.isa().pointer_type();
        let frontend_config = module.isa().frontend_config();
        let builder = FunctionBuilder::new(&mut context.func, builder_ctx);
        Ok(Self {
            builder,
            module,
            ids,
            function,
            functions,
            types,
            pointer_type,
            frontend_config,
            blocks: Vec::new(),
            kinds: Vec::new(),
            str_data,
        })
    }

    /// Lower the whole body.
    fn run(&mut self) -> Result<(), CodegenError> {
        // Create a Cranelift block per MIR block.
        self.blocks = (0..self.function.blocks.len())
            .map(|_| self.builder.create_block())
            .collect();

        // Pre-scan for locals whose address a borrow-mode call argument takes:
        // those must be memory-backed even when scalar, so the callee's pointer
        // aliases real caller memory. Aggregates already live in slots.
        let borrowed_roots: HashSet<u32> = self
            .function
            .blocks
            .iter()
            .flat_map(|block| &block.statements)
            .filter_map(|statement| match statement {
                Statement::Call { args, .. } => Some(args),
                _ => None,
            })
            .flatten()
            .filter_map(|arg| match arg {
                Arg::Borrow(place) | Arg::BorrowMut(place) => Some(place.local.0),
                Arg::Value(_) => None,
            })
            .collect();

        // Classify and allocate storage for every local: a scalar gets an SSA
        // Variable (or a stack slot, if borrowed); a unit local carries no
        // value; an aggregate local gets one explicit stack slot addressed by
        // its ABI layout; a borrow-mode parameter gets a pointer-typed Variable
        // holding the caller's address (seeded below).
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
                _ if borrow_param => LocalKind::Borrowed {
                    var: self.builder.declare_var(self.pointer_type),
                    ty: local.ty.clone(),
                },
                Storage::Scalar => {
                    // `require_scalar` cannot fail here (classify said scalar).
                    let ty = require_scalar(&local.ty, &self.function.name)?;
                    if borrowed_roots.contains(&(index as u32)) {
                        let layout = self.layout(&local.ty)?;
                        LocalKind::ScalarSlot(self.new_temp_slot(layout)?, ty)
                    } else {
                        LocalKind::Scalar(self.builder.declare_var(ty), ty)
                    }
                }
                Storage::Aggregate(layout) => {
                    let slot = self.builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        u32::try_from(layout.size).map_err(|_| {
                            CodegenError::backend("aggregate stack slot larger than 4 GiB")
                        })?,
                        log2_align(layout.align),
                    ));
                    LocalKind::Aggregate {
                        slot,
                        ty: local.ty.clone(),
                    }
                }
            };
            self.kinds.push(kind);
        }

        // Entry block: append the parameters and seed the parameter locals.
        let entry = self.blocks[0];
        self.builder.append_block_params_for_function_params(entry);
        self.builder.switch_to_block(entry);
        let param_values: Vec<ClifValue> = self.builder.block_params(entry).to_vec();

        // If this function returns an aggregate, native parameter 0 is the sret
        // out-pointer (not a MIR local); the MIR parameters start at native
        // index 1. Otherwise they start at 0.
        let returns_aggregate = matches!(
            classify_storage(&self.function.ret, self.types, &self.function.name)?,
            Storage::Aggregate(_)
        );
        let mut native = usize::from(returns_aggregate);

        // Seed each MIR parameter local from its native parameter. A scalar is
        // defined directly (or stored into its slot, if borrowed onward); an
        // aggregate `take` parameter's incoming pointer is copied into the
        // callee's own slot (owned-copy semantics); a borrow-mode parameter's
        // incoming pointer is kept as-is (no copy-in — it aliases the caller's
        // place); a unit parameter occupies no native slot.
        for index in 0..self.function.params.len() {
            match &self.kinds[index] {
                LocalKind::Scalar(var, _) => {
                    let var = *var;
                    let value = param_values[native];
                    self.builder.def_var(var, value);
                    native += 1;
                }
                LocalKind::ScalarSlot(slot, _) => {
                    let slot = *slot;
                    let value = param_values[native];
                    self.builder.ins().stack_store(value, slot, 0);
                    native += 1;
                }
                LocalKind::Unit => {}
                LocalKind::Aggregate { slot, ty } => {
                    let slot = *slot;
                    let ty = ty.clone();
                    let src = param_values[native];
                    native += 1;
                    let layout = self.layout(&ty)?;
                    let dest = self.builder.ins().stack_addr(self.pointer_type, slot, 0);
                    self.emit_memcpy(dest, src, layout);
                }
                LocalKind::Borrowed { var, .. } => {
                    let var = *var;
                    let addr = param_values[native];
                    self.builder.def_var(var, addr);
                    native += 1;
                }
            }
        }

        // Seed non-parameter scalar locals with a zero so a Variable is always
        // defined before use on every path (the ownership checker guarantees no
        // *semantic* read of an uninitialized local, but Cranelift's SSA
        // construction still requires a definition to exist). Stack slots
        // (aggregate or scalar) need no such seeding — a slot is always
        // addressable.
        for index in self.function.params.len()..self.function.locals.len() {
            if let LocalKind::Scalar(var, ty) = &self.kinds[index] {
                let (var, ty) = (*var, *ty);
                let zero = if ty == types::F32 {
                    self.builder.ins().f32const(0.0)
                } else if ty == types::F64 {
                    self.builder.ins().f64const(0.0)
                } else {
                    self.builder.ins().iconst(ty, 0)
                };
                self.builder.def_var(var, zero);
            }
        }

        // Lower each block's statements and terminator. Block 0's body follows
        // the parameter seeding in the same Cranelift block.
        for (index, block) in self.function.blocks.iter().enumerate() {
            if index != 0 {
                let clif_block = self.blocks[index];
                self.builder.switch_to_block(clif_block);
            }
            for statement in &block.statements {
                self.lower_statement(statement)?;
            }
            self.lower_terminator(&block.terminator)?;
        }

        self.builder.seal_all_blocks();
        Ok(())
    }

    /// Finalize the builder (consumes the lowering, which owns it).
    fn finish(self) {
        self.builder.finalize();
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

    /// Lower `place = rvalue`. An aggregate rvalue (`Aggregate`) is materialized
    /// in place into the destination's byte storage; a whole-aggregate `Use`
    /// (copy/move of an aggregate place) is a memcpy between slots; every scalar
    /// rvalue (including `Discriminant`, which yields a `Usize` scalar) takes the
    /// scalar path and is stored with `write_place`.
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
            // reads (`len`/`byte_at`/`get`) take the scalar rvalue path below.
            Rvalue::HeapOp { op, subject, args } if heap_op_produces_aggregate(*op) => {
                self.lower_heap_op_aggregate(place, *op, subject.as_ref(), args)
            }
            // `array::get` whose element is an aggregate (`Str`/`String`/struct,
            // ADR-0012 Stage B) produces an aggregate: read it into the dest slot
            // by memcpy rather than returning a scalar register value.
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
                // Whole-aggregate copy/move: memcpy from the source place's slot
                // into the destination's byte storage.
                let (dest_addr, dest_ty) = self.place_address(place)?;
                let layout = self.layout(&dest_ty)?;
                let src_addr = self.operand_aggregate_address(operand)?;
                self.emit_memcpy(dest_addr, src_addr, layout);
                Ok(())
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
        // references it as a fixed `FuncRef`. An `Indirect` call (ADR-0008
        // Tier 1) loads a runtime code-pointer value from the callee operand and
        // calls through an explicitly-built signature derived from the callee's
        // `Ty::Fn`. Both share the *entire* argument/sret/borrow marshalling and
        // return handling below — the only difference is the call instruction.
        //
        // The callee's return type drives the sret decision, so resolve it here.
        // The interpreter evaluates the indirect callee operand before its
        // arguments; match that ordering by loading the pointer first.
        enum CallTarget {
            Direct(FuncId),
            Indirect(ClifValue, SigRef),
        }
        let (target, callee_ret): (CallTarget, Ty) = match callee {
            Callee::Direct(symbol) => {
                let Some(&callee_id) = self.ids.get(symbol) else {
                    return Err(CodegenError::unsupported(
                        "call to a function outside the lowered program (v0 has no external \
                         calls)",
                    ));
                };
                let callee_fn = self.function_named(*symbol)?;
                (CallTarget::Direct(callee_id), callee_fn.ret.clone())
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
                let callee_val = self.lower_operand(operand)?;
                let call_conv = self.module.isa().default_call_conv();
                let signature = signature_from_parts(
                    call_conv,
                    self.pointer_type,
                    &fn_ty.ret,
                    fn_ty
                        .params
                        .iter()
                        .map(|p| (pass_mode_of(p.mode), p.ty.clone())),
                    self.types,
                    &self.function.name,
                )?;
                let sig_ref = self.builder.import_signature(signature);
                (CallTarget::Indirect(callee_val, sig_ref), fn_ty.ret.clone())
            }
        };

        // Does the callee return an aggregate? If so, the first native argument
        // is an sret out-pointer to caller-owned destination storage.
        let dest_ty = self.place_type(dest);
        let ret_is_aggregate = matches!(
            classify_storage(&callee_ret, self.types, &self.function.name)?,
            Storage::Aggregate(_)
        );

        let mut arg_values: Vec<ClifValue> = Vec::with_capacity(args.len() + 1);

        // Allocate the sret destination up front (a temporary slot unless dest is
        // a bare aggregate local, in which case its own slot is the destination).
        let sret_slot = if ret_is_aggregate {
            let layout = self.layout(&callee_ret)?;
            let (slot, addr) = self.sret_destination(dest, &dest_ty, layout)?;
            arg_values.push(addr);
            Some((slot, layout))
        } else {
            None
        };

        // Marshal each argument. A scalar `Value` arg is passed by value; an
        // aggregate `Value` arg is materialized into a caller temporary and its
        // address passed. Unit arguments occupy no native slot.
        for arg in args {
            match arg {
                Arg::Value(operand) => {
                    if self.operand_is_unit(operand) {
                        // A unit argument carries no native value.
                    } else if self.operand_is_aggregate(operand) {
                        let addr = self.materialize_aggregate_arg(operand)?;
                        arg_values.push(addr);
                    } else {
                        arg_values.push(self.lower_operand(operand)?);
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
                            arg_values.push(addr);
                        }
                    }
                }
            }
        }

        let call = match target {
            CallTarget::Direct(callee_id) => {
                let func_ref = self
                    .module
                    .declare_func_in_func(callee_id, self.builder.func);
                self.builder.ins().call(func_ref, &arg_values)
            }
            CallTarget::Indirect(callee_val, sig_ref) => {
                self.builder
                    .ins()
                    .call_indirect(sig_ref, callee_val, &arg_values)
            }
        };

        if let Some((slot, layout)) = sret_slot {
            // The callee wrote the aggregate result into the sret slot. If `dest`
            // is a bare aggregate local, that slot *is* its storage and nothing
            // more is needed. If `dest` is projected, copy from the temporary
            // slot into the projected address.
            if !dest.projection.is_empty() {
                let src = self.builder.ins().stack_addr(self.pointer_type, slot, 0);
                let (dest_addr, _leaf) = self.place_address(dest)?;
                self.emit_memcpy(dest_addr, src, layout);
            }
        } else {
            let results = self.builder.inst_results(call);
            // A unit-returning callee yields no result; a scalar callee yields
            // one, stored into the (scalar) destination.
            if let Some(&result) = results.first() {
                self.write_place(dest, result)?;
            }
        }
        Ok(())
    }

    /// The sret destination for a call returning an aggregate: `(slot, address)`.
    /// When `dest` is a bare aggregate local, its own slot is reused (no post-call
    /// copy). Otherwise a fresh temporary slot is allocated and returned so the
    /// caller can copy it into the projected destination after the call.
    fn sret_destination(
        &mut self,
        dest: &Place,
        dest_ty: &Ty,
        layout: Layout,
    ) -> Result<(StackSlot, ClifValue), CodegenError> {
        if dest.projection.is_empty() {
            if let LocalKind::Aggregate { slot, .. } = &self.kinds[dest.local.0 as usize] {
                let slot = *slot;
                let addr = self.builder.ins().stack_addr(self.pointer_type, slot, 0);
                return Ok((slot, addr));
            }
        }
        // Projected (or otherwise) destination: use a fresh temporary slot.
        let _ = dest_ty;
        let slot = self.new_temp_slot(layout)?;
        let addr = self.builder.ins().stack_addr(self.pointer_type, slot, 0);
        Ok((slot, addr))
    }

    /// Materialize an aggregate call argument into a caller-owned temporary slot
    /// and return its address, per the by-pointer call ABI. The operand is a
    /// `Copy`/`Move` of an aggregate place; memcpy its slot into the temporary so
    /// the callee's copy-in cannot alias the caller's live value.
    fn materialize_aggregate_arg(&mut self, operand: &Operand) -> Result<ClifValue, CodegenError> {
        let ty = self
            .operand_ty(operand)
            .ok_or_else(|| CodegenError::backend("aggregate argument has no static type"))?;
        let layout = self.layout(&ty)?;
        let src = self.operand_aggregate_address(operand)?;
        let slot = self.new_temp_slot(layout)?;
        let dest = self.builder.ins().stack_addr(self.pointer_type, slot, 0);
        self.emit_memcpy(dest, src, layout);
        Ok(dest)
    }

    /// The address of `place` for a borrow-mode (`in`/`mut`) call argument.
    ///
    /// A bare local resolves to its own storage: a borrowed-root scalar's slot
    /// (the pre-scan forced it into memory), an aggregate's slot, or — when
    /// forwarding a borrowed parameter — the incoming pointer value itself. A
    /// projected place resolves through the shared address walk.
    fn borrow_address(&mut self, place: &Place) -> Result<ClifValue, CodegenError> {
        if place.projection.is_empty() {
            return match &self.kinds[place.local.0 as usize] {
                LocalKind::ScalarSlot(slot, _) => {
                    Ok(self.builder.ins().stack_addr(self.pointer_type, *slot, 0))
                }
                LocalKind::Aggregate { slot, .. } => {
                    Ok(self.builder.ins().stack_addr(self.pointer_type, *slot, 0))
                }
                LocalKind::Borrowed { var, .. } => Ok(self.builder.use_var(*var)),
                LocalKind::Scalar(..) => Err(CodegenError::backend(
                    "borrowed scalar local was not slot-backed (pre-scan invariant)",
                )),
                LocalKind::Unit => Err(CodegenError::backend(
                    "address of a unit local requested for a borrow argument",
                )),
            };
        }
        let (addr, _leaf) = self.place_address(place)?;
        Ok(addr)
    }

    /// Allocate a fresh temporary stack slot of `layout`.
    fn new_temp_slot(&mut self, layout: Layout) -> Result<StackSlot, CodegenError> {
        Ok(self.builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            u32::try_from(layout.size)
                .map_err(|_| CodegenError::backend("aggregate temporary larger than 4 GiB"))?,
            log2_align(layout.align),
        )))
    }

    /// The MIR function with symbol `callee`, needed to read its return type and
    /// name for the call ABI. All calls are direct and internal in v0.
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
                        self.builder.ins().return_(&[]);
                    }
                    Storage::Scalar => {
                        let value = self.lower_operand(operand)?;
                        self.builder.ins().return_(&[value]);
                    }
                    Storage::Aggregate(layout) => {
                        // Copy the operand's aggregate value into the sret
                        // out-pointer (native parameter 0), then return void.
                        let src = self.operand_aggregate_address(operand)?;
                        let sret = self.builder.block_params(self.blocks[0])[0];
                        self.emit_memcpy(sret, src, layout);
                        self.builder.ins().return_(&[]);
                    }
                }
                Ok(())
            }
            Terminator::Goto(target) => {
                let block = self.blocks[target.0 as usize];
                self.builder.ins().jump(block, &[]);
                Ok(())
            }
            Terminator::Branch {
                cond,
                then_block,
                else_block,
            } => {
                let value = self.lower_operand(cond)?;
                let then_b = self.blocks[then_block.0 as usize];
                let else_b = self.blocks[else_block.0 as usize];
                self.builder.ins().brif(value, then_b, &[], else_b, &[]);
                Ok(())
            }
            Terminator::Switch {
                discr,
                arms,
                otherwise,
            } => {
                let value = self.lower_operand(discr)?;
                // Cranelift's Switch needs a fixed-width integer; the discr is
                // an integer-classed scalar (int, char, or bool). Widen bool
                // (i8) uniformly by using the value's own type.
                let mut switch = Switch::new();
                for (case_value, target) in arms {
                    let block = self.blocks[target.0 as usize];
                    // MIR guarantees arm values fit the operand's type.
                    switch.set_entry(*case_value as u128, block);
                }
                let otherwise_block = self.blocks[otherwise.0 as usize];
                switch.emit(&mut self.builder, value, otherwise_block);
                Ok(())
            }
            Terminator::Assert { cond, trap, target } => {
                let value = self.lower_operand(cond)?;
                let ok_block = self.blocks[target.0 as usize];
                let trap_block = self.builder.create_block();
                self.builder
                    .ins()
                    .brif(value, ok_block, &[], trap_block, &[]);
                self.builder.switch_to_block(trap_block);
                self.builder.seal_block(trap_block);
                self.emit_trap_call(trap_code_of(*trap));
                Ok(())
            }
            Terminator::Trap(trap) => {
                self.emit_trap_call(trap_code_of(*trap));
                Ok(())
            }
        }
    }

    // ----- rvalues & operands -----

    fn lower_rvalue(&mut self, rvalue: &Rvalue) -> Result<ClifValue, CodegenError> {
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
            Rvalue::Discriminant(place) => self.lower_discriminant(place),
            // `Len` applies only to the growable `Array[T]` (a `[T; N]`'s
            // length is lowered as a constant and the MIR verifier rejects
            // `Len` of a fixed-array place), so refusing it entirely stays
            // sound for fixed arrays.
            Rvalue::Len(_) => Err(CodegenError::unsupported(
                "the growable `Array[T]` (and its `Len`) is not lowered by the Cranelift \
                 backend yet",
            )),
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
                    Ok(len)
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
        }
    }

    fn lower_operand(&mut self, operand: &Operand) -> Result<ClifValue, CodegenError> {
        match operand {
            Operand::Copy(place) | Operand::Move(place) => self.read_place(place),
            Operand::Const(constant) => self.lower_const(constant),
        }
    }

    fn lower_const(&mut self, constant: &Const) -> Result<ClifValue, CodegenError> {
        match constant {
            Const::Bool(b) => Ok(self.builder.ins().iconst(types::I8, i64::from(*b))),
            Const::Char(c) => Ok(self
                .builder
                .ins()
                .iconst(types::I32, i64::from(u32::from(*c)))),
            Const::Int(value, kind) => {
                let ty = int_type(*kind);
                // Reinterpret the mathematical value's low bits at the target
                // width; MIR guarantees it is in range for the kind.
                let bits = *value as i64;
                Ok(self.builder.ins().iconst(ty, bits))
            }
            // The MIR constant is stored at f64 width, already normalized to
            // its kind's precision, so materializing an `F32` by rounding the
            // f64 payload to f32 is exact.
            Const::Float(value, FloatKind::F32) => Ok(self.builder.ins().f32const(*value as f32)),
            Const::Float(value, FloatKind::F64) => Ok(self.builder.ins().f64const(*value)),
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
            // top-level function — a pointer-width code pointer. Declare the
            // callee into this function and materialize its address as a scalar
            // value; it then flows through all scalar machinery (locals, moves
            // are copies since it is `Copy`, params, returns) unchanged.
            Const::Fn(symbol) => {
                let Some(&callee_id) = self.ids.get(symbol) else {
                    return Err(CodegenError::unsupported(
                        "a function value naming a function outside the lowered program (v0 has \
                         no external functions)",
                    ));
                };
                let func_ref = self
                    .module
                    .declare_func_in_func(callee_id, self.builder.func);
                Ok(self.builder.ins().func_addr(self.pointer_type, func_ref))
            }
        }
    }

    // ----- arithmetic -----

    fn lower_unary(
        &mut self,
        op: UnOp,
        operand: &Operand,
        value: ClifValue,
    ) -> Result<ClifValue, CodegenError> {
        match op {
            UnOp::Not => {
                // Boolean negation: xor with 1 (values are 0/1).
                Ok(self.builder.ins().bxor_imm(value, 1))
            }
            UnOp::Neg if matches!(self.operand_ty(operand), Some(Ty::Float(_))) => {
                // Float negation flips the sign bit (IEEE 754, works on NaN
                // too) and never traps — exactly the interpreter's `-v`.
                Ok(self.builder.ins().fneg(value))
            }
            UnOp::Neg => {
                let kind = self.operand_int_kind(operand)?;
                // Integer negation traps on MIN (two's complement, §24).
                let (min, _max) = int_bounds(kind);
                let ty = int_type(kind);
                let min_const = self.builder.ins().iconst(ty, min);
                let is_min = self.builder.ins().icmp(IntCC::Equal, value, min_const);
                self.guard(is_min, TrapCode::IntegerOverflow);
                Ok(self.builder.ins().ineg(value))
            }
        }
    }

    fn lower_binary(
        &mut self,
        op: BinOp,
        lhs: &Operand,
        l: ClifValue,
        r: ClifValue,
    ) -> Result<ClifValue, CodegenError> {
        // Floats take their own IEEE-754 path (never trapping); everything
        // else reduces to integer compares/arithmetic.
        if let Some(Ty::Float(kind)) = self.operand_ty(lhs) {
            return self.lower_float_binary(op, kind, l, r);
        }
        // Comparisons on chars/bools/ints all reduce to integer compares; the
        // arithmetic operators are integer-only in the supported subset.
        if let Some(cc) = comparison_code(op, self.operand_signed(lhs)) {
            // Cranelift's `icmp` yields an `i8` boolean (0 or 1), which is
            // exactly the backend's `Bool` representation — no widening needed.
            return Ok(self.builder.ins().icmp(cc, l, r));
        }

        let kind = self.operand_int_kind(lhs)?;
        match op {
            BinOp::Add => Ok(self.checked_arith(kind, l, r, ArithOp::Add)),
            BinOp::Sub => Ok(self.checked_arith(kind, l, r, ArithOp::Sub)),
            BinOp::Mul => Ok(self.checked_arith(kind, l, r, ArithOp::Mul)),
            BinOp::Div => Ok(self.checked_divrem(kind, l, r, true)),
            BinOp::Rem => Ok(self.checked_divrem(kind, l, r, false)),
            // Comparisons handled above; reaching here is impossible.
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => Err(
                CodegenError::backend("comparison fell through arithmetic path"),
            ),
        }
    }

    /// Lower a float binary operation: IEEE 754 (round to nearest even), never
    /// trapping — `x / 0.0` is an infinity or NaN, exactly as the interpreter
    /// computes with Rust `f64`/`f32` arithmetic. `Rem` has C `fmod` semantics
    /// (the sign of the dividend), which is Rust `%` — Cranelift has no `frem`
    /// instruction, so it is lowered as a call to the C library's
    /// `fmod`/`fmodf` (the exact function Rust's `%` lowers to).
    ///
    /// Comparisons use Cranelift's [`FloatCC`]: the four orderings are
    /// *ordered* (false when either side is NaN) and `NotEqual` is
    /// unordered-or-unequal (true on NaN) — exactly Rust's float comparison
    /// semantics, which the interpreter uses.
    fn lower_float_binary(
        &mut self,
        op: BinOp,
        kind: FloatKind,
        l: ClifValue,
        r: ClifValue,
    ) -> Result<ClifValue, CodegenError> {
        if let Some(cc) = float_comparison_code(op) {
            // `fcmp` yields an `i8` boolean (0 or 1), the backend's `Bool`.
            return Ok(self.builder.ins().fcmp(cc, l, r));
        }
        Ok(match op {
            BinOp::Add => self.builder.ins().fadd(l, r),
            BinOp::Sub => self.builder.ins().fsub(l, r),
            BinOp::Mul => self.builder.ins().fmul(l, r),
            BinOp::Div => self.builder.ins().fdiv(l, r),
            BinOp::Rem => {
                let func_ref = self.fmod_func_ref(kind);
                let call = self.builder.ins().call(func_ref, &[l, r]);
                self.builder.inst_results(call)[0]
            }
            // Comparisons handled above; reaching here is impossible.
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                return Err(CodegenError::backend(
                    "float comparison fell through the arithmetic path",
                ));
            }
        })
    }

    /// Declare (on demand) and reference the C library's `fmod` (f64) or
    /// `fmodf` (f32) for [`BinOp::Rem`] on floats, mirroring how the runtime
    /// trap symbol is imported. The CLI links with `-lm` so the symbol resolves
    /// on every supported host.
    fn fmod_func_ref(&mut self, kind: FloatKind) -> cranelift_codegen::ir::FuncRef {
        let float_ty = float_type(kind);
        let mut signature = Signature::new(CallConv::triple_default(self.module.isa().triple()));
        signature.params.push(AbiParam::new(float_ty));
        signature.params.push(AbiParam::new(float_ty));
        signature.returns.push(AbiParam::new(float_ty));
        let name = match kind {
            FloatKind::F32 => "fmodf",
            FloatKind::F64 => "fmod",
        };
        let id = self
            .module
            .declare_function(name, Linkage::Import, &signature)
            .expect("declaring the libm fmod symbol");
        self.module.declare_func_in_func(id, self.builder.func)
    }

    /// Lower a trapping add/sub/mul: compute at full width, then trap if the
    /// result is out of the kind's range. For the register width equal to the
    /// kind width, Cranelift's overflow-flagged ops would suffice; to keep one
    /// uniform rule across widths, we compute in a wider space by range-checking
    /// against the kind bounds using the operation's overflow instruction.
    fn checked_arith(
        &mut self,
        kind: IntKind,
        l: ClifValue,
        r: ClifValue,
        op: ArithOp,
    ) -> ClifValue {
        let signed = is_signed(kind);
        // Use Cranelift's carry/overflow-producing instructions where the
        // register width matches the kind width (the common case for i8..i64).
        let (result, overflow) = match (op, signed) {
            (ArithOp::Add, true) => self.builder.ins().sadd_overflow(l, r),
            (ArithOp::Add, false) => self.builder.ins().uadd_overflow(l, r),
            (ArithOp::Sub, true) => self.builder.ins().ssub_overflow(l, r),
            (ArithOp::Sub, false) => self.builder.ins().usub_overflow(l, r),
            (ArithOp::Mul, true) => self.builder.ins().smul_overflow(l, r),
            (ArithOp::Mul, false) => self.builder.ins().umul_overflow(l, r),
        };
        self.guard(overflow, TrapCode::IntegerOverflow);
        result
    }

    /// Lower a trapping div (`is_div`) or rem: trap on a zero divisor, and on
    /// the signed `MIN / -1` overflow case.
    fn checked_divrem(
        &mut self,
        kind: IntKind,
        l: ClifValue,
        r: ClifValue,
        is_div: bool,
    ) -> ClifValue {
        let ty = int_type(kind);
        let signed = is_signed(kind);

        // Divisor == 0 → division-by-zero trap.
        let zero = self.builder.ins().iconst(ty, 0);
        let is_zero = self.builder.ins().icmp(IntCC::Equal, r, zero);
        self.guard(is_zero, TrapCode::DivisionByZero);

        if signed {
            // MIN / -1 (and MIN % -1) overflow → integer-overflow trap.
            let (min, _max) = int_bounds(kind);
            let min_const = self.builder.ins().iconst(ty, min);
            let neg_one = self.builder.ins().iconst(ty, -1);
            let l_is_min = self.builder.ins().icmp(IntCC::Equal, l, min_const);
            let r_is_neg_one = self.builder.ins().icmp(IntCC::Equal, r, neg_one);
            let both = self.builder.ins().band(l_is_min, r_is_neg_one);
            self.guard(both, TrapCode::IntegerOverflow);
            if is_div {
                self.builder.ins().sdiv(l, r)
            } else {
                self.builder.ins().srem(l, r)
            }
        } else if is_div {
            self.builder.ins().udiv(l, r)
        } else {
            self.builder.ins().urem(l, r)
        }
    }

    fn lower_cast(
        &mut self,
        kind: CastKind,
        operand: &Operand,
        value: ClifValue,
        to: &Ty,
    ) -> Result<ClifValue, CodegenError> {
        match kind {
            CastKind::IntToInt => {
                let Ty::Int(target) = to else {
                    return Err(CodegenError::backend("int-to-int cast to a non-integer"));
                };
                let from = self.operand_int_kind(operand)?;
                Ok(self.resize_int(value, from, *target))
            }
            CastKind::IntToFloat => {
                let Ty::Float(target) = to else {
                    return Err(CodegenError::backend("int-to-float cast to a non-float"));
                };
                // Round to nearest even, by the SOURCE's signedness. The
                // interpreter computes `v as f64` and then re-rounds an `F32`
                // (`normalize_float`); this takes the same literal two-step
                // int→f64→f32 path. (The double rounding is provably
                // innocuous — 53 ≥ 2·24+2 — so it equals a direct int→f32
                // conversion; the two-step form keeps the correspondence to
                // the interpreter self-evident.)
                let from = self.operand_int_kind(operand)?;
                let wide = if is_signed(from) {
                    self.builder.ins().fcvt_from_sint(types::F64, value)
                } else {
                    self.builder.ins().fcvt_from_uint(types::F64, value)
                };
                Ok(match target {
                    FloatKind::F64 => wide,
                    FloatKind::F32 => self.builder.ins().fdemote(types::F32, wide),
                })
            }
            CastKind::FloatToInt => {
                let Ty::Int(target) = to else {
                    return Err(CodegenError::backend("float-to-int cast to a non-integer"));
                };
                // Cranelift's saturating conversions truncate toward zero,
                // saturate to the TARGET's range, and map NaN to 0 — exactly
                // the interpreter's `saturating_float_to_int`. Never traps.
                let to_ty = int_type(*target);
                Ok(if is_signed(*target) {
                    self.builder.ins().fcvt_to_sint_sat(to_ty, value)
                } else {
                    self.builder.ins().fcvt_to_uint_sat(to_ty, value)
                })
            }
            CastKind::FloatToFloat => {
                let Ty::Float(target) = to else {
                    return Err(CodegenError::backend("float-to-float cast to a non-float"));
                };
                let Some(Ty::Float(from)) = self.operand_ty(operand) else {
                    return Err(CodegenError::backend("float-to-float cast of a non-float"));
                };
                // IEEE 754 conversion: exact when widening, round to nearest
                // even when narrowing — the interpreter's `normalize_float`.
                Ok(match (from, target) {
                    (FloatKind::F32, FloatKind::F64) => {
                        self.builder.ins().fpromote(types::F64, value)
                    }
                    (FloatKind::F64, FloatKind::F32) => {
                        self.builder.ins().fdemote(types::F32, value)
                    }
                    (FloatKind::F32, FloatKind::F32) | (FloatKind::F64, FloatKind::F64) => value,
                })
            }
        }
    }

    /// Resize an integer from `from` to `target` width with two's-complement
    /// wrapping (truncate when narrowing; sign/zero-extend from the source's
    /// signedness when widening) — matching the interpreter's `wrap_int`.
    fn resize_int(&mut self, value: ClifValue, from: IntKind, target: IntKind) -> ClifValue {
        let from_bits = int_width_bits(from);
        let to_bits = int_width_bits(target);
        let to_ty = int_type(target);
        if to_bits == from_bits {
            value
        } else if to_bits < from_bits {
            self.builder.ins().ireduce(to_ty, value)
        } else if is_signed(from) {
            self.builder.ins().sextend(to_ty, value)
        } else {
            self.builder.ins().uextend(to_ty, value)
        }
    }

    // ----- places -----

    /// Read the scalar value of `place`. An empty projection on a scalar local
    /// reads its SSA variable; a non-empty projection resolves to a leaf byte
    /// address (which must be a scalar leaf in Stage 1) and loads it.
    fn read_place(&mut self, place: &Place) -> Result<ClifValue, CodegenError> {
        if place.projection.is_empty() {
            let local = place.local.0 as usize;
            return match &self.kinds[local] {
                LocalKind::Scalar(var, _) => Ok(self.builder.use_var(*var)),
                LocalKind::ScalarSlot(slot, ty) => {
                    let (slot, ty) = (*slot, *ty);
                    Ok(self.builder.ins().stack_load(ty, slot, 0))
                }
                LocalKind::Borrowed { var, ty } => {
                    // A scalar borrowed parameter: load through the caller's
                    // pointer. (A whole-aggregate read goes through the
                    // aggregate path, exactly as for `Aggregate` locals.)
                    let var = *var;
                    let ty = ty.clone();
                    let clif = require_scalar(&ty, &self.function.name).map_err(|_| {
                        CodegenError::backend(
                            "reading a whole aggregate as a scalar (should go through the \
                             aggregate path)",
                        )
                    })?;
                    let addr = self.builder.use_var(var);
                    Ok(self.builder.ins().load(clif, MemFlags::trusted(), addr, 0))
                }
                LocalKind::Unit => Err(CodegenError::backend(
                    "reading a scalar value from a unit local",
                )),
                LocalKind::Aggregate { .. } => Err(CodegenError::backend(
                    "reading a whole aggregate as a scalar (should go through the aggregate path)",
                )),
            };
        }
        // Projected read: walk to the leaf address and load the scalar there.
        let (addr, leaf_ty) = self.place_address(place)?;
        let clif = require_scalar(&leaf_ty, &self.function.name)?;
        Ok(self.builder.ins().load(clif, MemFlags::trusted(), addr, 0))
    }

    /// Write the scalar `value` to `place`. An empty projection on a scalar local
    /// defines its SSA variable; a non-empty projection resolves to a leaf byte
    /// address (a scalar leaf in Stage 1) and stores there. A unit destination
    /// carries no value.
    fn write_place(&mut self, place: &Place, value: ClifValue) -> Result<(), CodegenError> {
        if place.projection.is_empty() {
            let local = place.local.0 as usize;
            return match &self.kinds[local] {
                LocalKind::Scalar(var, _) => {
                    self.builder.def_var(*var, value);
                    Ok(())
                }
                LocalKind::ScalarSlot(slot, _) => {
                    let slot = *slot;
                    self.builder.ins().stack_store(value, slot, 0);
                    Ok(())
                }
                LocalKind::Borrowed { var, .. } => {
                    // A `mut` scalar parameter: store through the caller's
                    // pointer, so the write is visible to the caller with no
                    // copy-back.
                    let var = *var;
                    let addr = self.builder.use_var(var);
                    self.builder
                        .ins()
                        .store(MemFlags::trusted(), value, addr, 0);
                    Ok(())
                }
                // A unit-typed destination carries no value; skip it.
                LocalKind::Unit => Ok(()),
                LocalKind::Aggregate { .. } => Err(CodegenError::backend(
                    "writing a scalar into a whole aggregate local (should go through the \
                     aggregate path)",
                )),
            };
        }
        let (addr, _leaf_ty) = self.place_address(place)?;
        self.builder
            .ins()
            .store(MemFlags::trusted(), value, addr, 0);
        Ok(())
    }

    /// The shared address walk: resolve `place` to a `(byte address, leaf type)`
    /// pair by starting at the root local's slot base and advancing by each
    /// projection's ABI offset. A `Field`/`VariantField` step advances by a
    /// constant ABI offset; an `Index` step advances by the runtime index value
    /// times the element stride — **unchecked**, because MIR asserts the bounds
    /// (`Assert { IndexOutOfBounds }`) before every `Index` use, exactly like
    /// the interpreter's post-check access.
    ///
    /// This is the single place both scalar field read/write and whole-aggregate
    /// operations compute a field address, so the two code paths cannot diverge.
    fn place_address(&mut self, place: &Place) -> Result<(ClifValue, Ty), CodegenError> {
        let local = place.local.0 as usize;
        let (mut addr, mut cur_ty) = match &self.kinds[local] {
            LocalKind::Aggregate { slot, ty } => {
                let base = self.builder.ins().stack_addr(self.pointer_type, *slot, 0);
                (base, ty.clone())
            }
            // A borrowed parameter's incoming pointer is the base address of
            // the caller's place; projections advance from it exactly as from
            // a local slot.
            LocalKind::Borrowed { var, ty } => {
                let (var, ty) = (*var, ty.clone());
                (self.builder.use_var(var), ty)
            }
            // A zero-sized aggregate root (e.g. `[T; 0]`) classifies as unit
            // storage yet may still be projected in MIR — only in code the
            // preceding bounds `Assert` makes unreachable at runtime (any index
            // into a zero-length array traps first). Give it a null base; no
            // load/store computed from it can ever execute.
            LocalKind::Unit if !place.projection.is_empty() => (
                self.builder.ins().iconst(self.pointer_type, 0),
                self.function.locals[local].ty.clone(),
            ),
            LocalKind::Scalar(..) | LocalKind::ScalarSlot(..) | LocalKind::Unit
                if place.projection.is_empty() =>
            {
                return Err(CodegenError::backend(
                    "place_address called on a non-aggregate local with no projection",
                ));
            }
            LocalKind::Scalar(..) | LocalKind::ScalarSlot(..) | LocalKind::Unit => {
                // A projection on a scalar local is impossible in well-formed
                // MIR; refuse rather than mis-address.
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
                    let offset = i64::try_from(offset)
                        .map_err(|_| CodegenError::backend("field offset exceeds i64"))?;
                    addr = self.builder.ins().iadd_imm(addr, offset);
                }
                Projection::VariantField { variant, field } => {
                    let offsets = variant_field_offsets(&cur_ty, *variant as usize, self.types)
                        .map_err(|error| self.layout_error(error))?;
                    let f = *field as usize;
                    let offset = *offsets
                        .get(f)
                        .ok_or_else(|| CodegenError::backend("variant field index out of range"))?;
                    cur_ty = self.variant_field_type(&cur_ty, *variant as usize, f)?;
                    let offset = i64::try_from(offset)
                        .map_err(|_| CodegenError::backend("field offset exceeds i64"))?;
                    addr = self.builder.ins().iadd_imm(addr, offset);
                }
                Projection::Index(index_local) => {
                    // Only the fixed `[T; N]` is indexable natively; element
                    // `i` lives at `i × stride(T)` (the ABI's inline layout).
                    let element = match &cur_ty {
                        Ty::FixedArray(element, _) => (**element).clone(),
                        _ => {
                            return Err(CodegenError::unsupported(
                                "indexing the growable `Array[T]` is not lowered by the \
                                 Cranelift backend (only the fixed `[T; N]` is)",
                            ));
                        }
                    };
                    let stride = i64::try_from(self.layout(&element)?.stride())
                        .map_err(|_| CodegenError::backend("element stride exceeds i64"))?;
                    // The index is a `Usize` scalar local, pre-asserted in
                    // bounds by the MIR before this use.
                    let index_value = match &self.kinds[index_local.0 as usize] {
                        LocalKind::Scalar(var, _) => self.builder.use_var(*var),
                        LocalKind::ScalarSlot(slot, ty) => {
                            let (slot, ty) = (*slot, *ty);
                            self.builder.ins().stack_load(ty, slot, 0)
                        }
                        _ => {
                            return Err(CodegenError::backend("array index local is not a scalar"));
                        }
                    };
                    let scaled = self.builder.ins().imul_imm(index_value, stride);
                    addr = self.builder.ins().iadd(addr, scaled);
                    cur_ty = element;
                }
            }
        }
        Ok((addr, cur_ty))
    }

    /// Materialize a `Rvalue::Aggregate` in place into `dest`'s byte storage.
    ///
    /// For an enum-like type the u32 discriminant is stored at offset 0 first;
    /// for a struct/tuple no tag is written. Each field operand is then stored at
    /// its ABI offset — a scalar field with a scalar store, a nested-aggregate
    /// field with a whole-aggregate memcpy from its source slot. A fixed-array
    /// aggregate takes its own path (element `i` at `i × stride`).
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
                    "range construction is not lowered by the Cranelift backend (Stage 2)",
                ));
            }
            tuo_mir::AggregateKind::Array { element, len } => {
                return self.lower_array_aggregate(dest, element, *len, fields);
            }
        };

        // The destination base address (bare aggregate local, or a projected
        // whole-aggregate slot).
        let base = self.aggregate_dest_address(dest)?;

        // Enum-like: store the u32 tag at offset 0. Struct/tuple: no tag.
        let enum_like = matches!(ty, Ty::Enum(..) | Ty::Option(_) | Ty::Result(..));
        if enum_like {
            let tag = self.builder.ins().iconst(types::I32, i64::from(variant));
            self.builder.ins().store(MemFlags::trusted(), tag, base, 0);
        }

        // Field offsets: struct/tuple vs enum variant.
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
            // A unit / zero-sized field contributes no store.
            if self.operand_is_unit(operand) {
                continue;
            }
            let offset = i64::try_from(offsets[i])
                .map_err(|_| CodegenError::backend("field offset exceeds i64"))?;
            let field_addr = self.builder.ins().iadd_imm(base, offset);
            if self.operand_is_aggregate(operand) {
                // Nested aggregate field: memcpy from its source slot.
                let field_ty = self
                    .operand_ty(operand)
                    .ok_or_else(|| CodegenError::backend("aggregate field has no static type"))?;
                let layout = self.layout(&field_ty)?;
                let src = self.operand_aggregate_address(operand)?;
                self.emit_memcpy(field_addr, src, layout);
            } else {
                // Scalar field: evaluate and store.
                let value = self.lower_operand(operand)?;
                self.builder
                    .ins()
                    .store(MemFlags::trusted(), value, field_addr, 0);
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
    /// mirroring `lower_aggregate`'s field dispatch.
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
                .and_then(|offset| i64::try_from(offset).ok())
                .ok_or_else(|| CodegenError::backend("array element offset exceeds i64"))?;
            let element_addr = self.builder.ins().iadd_imm(base, offset);
            if self.operand_is_aggregate(operand) {
                let src = self.operand_aggregate_address(operand)?;
                self.emit_memcpy(element_addr, src, elem_layout);
            } else {
                let value = self.lower_operand(operand)?;
                self.builder
                    .ins()
                    .store(MemFlags::trusted(), value, element_addr, 0);
            }
        }
        Ok(())
    }

    /// The base address to materialize an aggregate into, for an `Assign` whose
    /// destination is an aggregate. A bare aggregate local uses its slot; a
    /// projected destination resolves through the address walk.
    fn aggregate_dest_address(&mut self, dest: &Place) -> Result<ClifValue, CodegenError> {
        if dest.projection.is_empty() {
            return match &self.kinds[dest.local.0 as usize] {
                LocalKind::Aggregate { slot, .. } => {
                    Ok(self.builder.ins().stack_addr(self.pointer_type, *slot, 0))
                }
                // A whole-aggregate write to a `mut` borrowed parameter goes
                // straight through the caller's pointer (no copy-back).
                LocalKind::Borrowed { var, .. } => Ok(self.builder.use_var(*var)),
                _ => Err(CodegenError::backend(
                    "aggregate rvalue assigned to a non-aggregate local",
                )),
            };
        }
        let (addr, _leaf) = self.place_address(dest)?;
        Ok(addr)
    }

    /// Lower `Rvalue::Discriminant`: load the u32 tag at offset 0 of the enum
    /// value and zero-extend it to a `Usize` (i64), matching the interpreter's
    /// `Value::Int(discr, Usize)`.
    fn lower_discriminant(&mut self, place: &Place) -> Result<ClifValue, CodegenError> {
        let base = if place.projection.is_empty() {
            match &self.kinds[place.local.0 as usize] {
                LocalKind::Aggregate { slot, .. } => {
                    self.builder.ins().stack_addr(self.pointer_type, *slot, 0)
                }
                // The discriminant of a borrowed enum parameter reads through
                // the caller's pointer.
                LocalKind::Borrowed { var, .. } => self.builder.use_var(*var),
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
        // Load the u32 tag, then zero-extend to i64 (Usize). Never truncate.
        let tag = self
            .builder
            .ins()
            .load(types::I32, MemFlags::trusted(), base, 0);
        Ok(self.builder.ins().uextend(types::I64, tag))
    }

    // ----- strings & effects (ADR-0006 Stage B) -----

    /// The static data object holding a string literal's bytes, deduplicated
    /// per module (keyed by the emitted contents). An empty literal still gets
    /// a real one-byte data object so its address is a fixed, non-null,
    /// aligned pointer — never dereferenced, because its `len` is 0.
    fn str_const_data(&mut self, text: &str) -> Result<DataId, CodegenError> {
        let contents: Vec<u8> = if text.is_empty() {
            vec![0]
        } else {
            text.as_bytes().to_vec()
        };
        if let Some(&id) = self.str_data.get(&contents) {
            return Ok(id);
        }
        let id = self
            .module
            .declare_anonymous_data(false, false)
            .map_err(|error| CodegenError::backend(format!("declaring string data: {error}")))?;
        let mut description = DataDescription::new();
        description.define(contents.clone().into_boxed_slice());
        description.set_align(1);
        self.module
            .define_data(id, &description)
            .map_err(|error| CodegenError::backend(format!("defining string data: {error}")))?;
        self.str_data.insert(contents, id);
        Ok(id)
    }

    /// The `{ptr, len}` pair of a `Str` literal: the address of its static
    /// data and its byte length as an `I64` constant.
    fn str_const_parts(&mut self, text: &str) -> Result<(ClifValue, ClifValue), CodegenError> {
        let data_id = self.str_const_data(text)?;
        let sym = self.module.declare_data_in_func(data_id, self.builder.func);
        let ptr = self.builder.ins().symbol_value(self.pointer_type, sym);
        let len = i64::try_from(text.len())
            .map_err(|_| CodegenError::backend("string literal longer than i64::MAX bytes"))?;
        let len = self.builder.ins().iconst(types::I64, len);
        Ok((ptr, len))
    }

    /// The `{ptr, len}` fat-pointer fields of a `Str`-typed operand: a literal
    /// yields its static data address and constant length directly; a place
    /// loads the two words from its aggregate storage (the shared address
    /// machinery).
    fn str_operand_parts(
        &mut self,
        operand: &Operand,
    ) -> Result<(ClifValue, ClifValue), CodegenError> {
        if let Operand::Const(Const::Str(text)) = operand {
            let text = text.clone();
            return self.str_const_parts(&text);
        }
        let addr = self.operand_aggregate_address(operand)?;
        let ptr = self
            .builder
            .ins()
            .load(self.pointer_type, MemFlags::trusted(), addr, 0);
        let len = self
            .builder
            .ins()
            .load(types::I64, MemFlags::trusted(), addr, STR_LEN_OFFSET);
        Ok((ptr, len))
    }

    /// Lower `Eq`/`Ne` on `Str` operands: byte-wise equality, exactly as the
    /// interpreter compares its byte buffers — lengths equal AND bytes equal.
    /// The byte compare (the C library's `memcmp`, imported like `fmod`) runs
    /// only when the lengths match, so comparing `len_a` bytes of both sides
    /// is always in bounds; a zero-length pair is equal without dereferencing
    /// (`memcmp` with a zero count reads nothing). The type checker forbids
    /// ordering on `Str`, so no other operator can reach here.
    fn lower_str_equality(
        &mut self,
        op: BinOp,
        lhs: &Operand,
        rhs: &Operand,
    ) -> Result<ClifValue, CodegenError> {
        if !matches!(op, BinOp::Eq | BinOp::Ne) {
            return Err(CodegenError::backend(
                "a non-equality comparison on `Str` operands (the type checker forbids \
                 ordering on `Str`)",
            ));
        }
        let (ptr_a, len_a) = self.str_operand_parts(lhs)?;
        let (ptr_b, len_b) = self.str_operand_parts(rhs)?;

        // join(i8): the equality verdict — false from the length mismatch
        // edge, the memcmp verdict from the byte-compare edge.
        let join = self.builder.create_block();
        self.builder.append_block_param(join, types::I8);
        let cmp_block = self.builder.create_block();

        let lens_eq = self.builder.ins().icmp(IntCC::Equal, len_a, len_b);
        let false_value = self.builder.ins().iconst(types::I8, 0);
        self.builder
            .ins()
            .brif(lens_eq, cmp_block, &[], join, &[false_value.into()]);

        self.builder.switch_to_block(cmp_block);
        self.builder.seal_block(cmp_block);
        let func_ref = self.memcmp_func_ref();
        let call = self.builder.ins().call(func_ref, &[ptr_a, ptr_b, len_a]);
        let verdict = self.builder.inst_results(call)[0];
        let zero = self.builder.ins().iconst(types::I32, 0);
        let bytes_eq = self.builder.ins().icmp(IntCC::Equal, verdict, zero);
        self.builder.ins().jump(join, &[bytes_eq.into()]);

        self.builder.switch_to_block(join);
        self.builder.seal_block(join);
        let eq = self.builder.block_params(join)[0];
        Ok(match op {
            BinOp::Ne => self.builder.ins().bxor_imm(eq, 1),
            _ => eq,
        })
    }

    /// Lower `byte_at(s, index)`: a deterministic `IndexOutOfBounds` trap
    /// unless `0 <= index < len(s)` (`specification/mir.md` §5.6, exactly the
    /// interpreter's `eval_str_op`), then load the byte at `ptr + index` and
    /// zero-extend it to the `I64` the destination expects. One unsigned
    /// compare implements both bounds: a negative index reinterprets as a
    /// huge unsigned value, and `len` is never negative.
    fn lower_str_byte_at(&mut self, args: &[Operand]) -> Result<ClifValue, CodegenError> {
        let [s, index] = args else {
            return Err(CodegenError::backend("byte_at expects exactly 2 operands"));
        };
        let (ptr, len) = self.str_operand_parts(s)?;
        let index = self.lower_operand(index)?;
        let oob = self
            .builder
            .ins()
            .icmp(IntCC::UnsignedGreaterThanOrEqual, index, len);
        self.guard(oob, TrapCode::IndexOutOfBounds);
        let addr = self.builder.ins().iadd(ptr, index);
        let byte = self
            .builder
            .ins()
            .load(types::I8, MemFlags::trusted(), addr, 0);
        Ok(self.builder.ins().uextend(types::I64, byte))
    }

    /// Lower `dest = slice(s, start, end)`: a deterministic `IndexOutOfBounds`
    /// trap unless `0 <= start <= end <= len(s)` (`specification/mir.md`
    /// §5.6), then write the derived fat pointer `{ptr + start, end - start}`
    /// into the destination's aggregate storage. Two unsigned compares cover
    /// all four bounds: `start >u end` catches `start > end` and a negative
    /// `start`; `end >u len` catches `end > len` and a negative `end`. The
    /// source's words are loaded before the destination is written, so
    /// `s = slice(s, ..)` re-slicing in place is sound.
    fn lower_str_slice(&mut self, dest: &Place, args: &[Operand]) -> Result<(), CodegenError> {
        let [s, start, end] = args else {
            return Err(CodegenError::backend("slice expects exactly 3 operands"));
        };
        let (ptr, len) = self.str_operand_parts(s)?;
        let start = self.lower_operand(start)?;
        let end = self.lower_operand(end)?;
        let bad_order = self
            .builder
            .ins()
            .icmp(IntCC::UnsignedGreaterThan, start, end);
        self.guard(bad_order, TrapCode::IndexOutOfBounds);
        let bad_end = self
            .builder
            .ins()
            .icmp(IntCC::UnsignedGreaterThan, end, len);
        self.guard(bad_end, TrapCode::IndexOutOfBounds);

        let new_ptr = self.builder.ins().iadd(ptr, start);
        let new_len = self.builder.ins().isub(end, start);
        let base = self.aggregate_dest_address(dest)?;
        self.builder
            .ins()
            .store(MemFlags::trusted(), new_ptr, base, 0);
        self.builder
            .ins()
            .store(MemFlags::trusted(), new_len, base, STR_LEN_OFFSET);
        Ok(())
    }

    /// Lower one host effect (`Statement::Effect`, ADR-0006 Stage B): a direct
    /// call to the matching [`tuo_runtime::effect`] symbol, with the `I64`
    /// result stored into `dest`. `exit` never returns, so after its call the
    /// block is terminated with the same unreachable shape the trap path uses,
    /// and the (dead) remainder of the MIR block is lowered into a fresh
    /// unreachable block — cranelift-frontend fills variable uses there with
    /// placeholder zeros, and none of it ever executes.
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
                let func_ref = self.effect_func_ref(op);
                let call = self.builder.ins().call(func_ref, &[fd, ptr, len]);
                let result = self.builder.inst_results(call)[0];
                self.write_place(dest, result)
            }
            EffectOp::ReadByte => {
                let [fd] = args else {
                    return Err(CodegenError::backend(
                        "read_byte expects exactly 1 argument",
                    ));
                };
                let fd = self.lower_operand(&value_arg(fd)?)?;
                let func_ref = self.effect_func_ref(op);
                let call = self.builder.ins().call(func_ref, &[fd]);
                let result = self.builder.inst_results(call)[0];
                self.write_place(dest, result)
            }
            // `write_string(fd, in s: String)` (ADR-0009 Stage B): load the
            // `{ptr, len}` from the borrowed `String` header and call the same
            // `tuo_rt_write` symbol `write` uses (the runtime writes bytes; it
            // neither knows nor cares whether they came from a `Str` view or an
            // owned buffer).
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
                let ptr = self.builder.ins().load(
                    self.pointer_type,
                    MemFlags::trusted(),
                    header,
                    HDR_PTR_OFFSET,
                );
                let len = self.builder.ins().load(
                    types::I64,
                    MemFlags::trusted(),
                    header,
                    HDR_LEN_OFFSET,
                );
                let func_ref = self.effect_func_ref(EffectOp::Write);
                let call = self.builder.ins().call(func_ref, &[fd, ptr, len]);
                let result = self.builder.inst_results(call)[0];
                self.write_place(dest, result)
            }
            EffectOp::Exit => {
                let [code] = args else {
                    return Err(CodegenError::backend("exit expects exactly 1 argument"));
                };
                let code = self.lower_operand(&value_arg(code)?)?;
                let func_ref = self.effect_func_ref(op);
                self.builder.ins().call(func_ref, &[code]);
                // The call never returns; `dest` is never observably written.
                self.builder
                    .ins()
                    .trap(cranelift_codegen::ir::TrapCode::user(1).unwrap());
                let dead = self.builder.create_block();
                self.builder.switch_to_block(dead);
                self.builder.seal_block(dead);
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
                let ptr = self.builder.ins().load(
                    self.pointer_type,
                    MemFlags::trusted(),
                    header,
                    HDR_PTR_OFFSET,
                );
                let len = self.builder.ins().load(
                    types::I64,
                    MemFlags::trusted(),
                    header,
                    HDR_LEN_OFFSET,
                );
                let dest_addr = self.aggregate_dest_address(dest)?;
                let func_ref = self.effect_func_ref(op);
                self.builder
                    .ins()
                    .call(func_ref, &[f, ptr, len, workers, dest_addr]);
                Ok(())
            }
            // ADR-0013: the OS-boundary effects. Nullary clock/argv-count
            // queries, the two-index argv byte read, and the file
            // open/close/remove calls — every one a plain call whose `I64`
            // result lands in `dest`; a `Str` path passes as `{ptr, len}`
            // exactly as `write`'s text does.
            EffectOp::NowNanos | EffectOp::ArgCount => {
                let func_ref = self.effect_func_ref(op);
                let call = self.builder.ins().call(func_ref, &[]);
                let result = self.builder.inst_results(call)[0];
                self.write_place(dest, result)
            }
            EffectOp::ArgByte => {
                let [i, j] = args else {
                    return Err(CodegenError::backend(
                        "arg_byte expects exactly 2 arguments",
                    ));
                };
                let i = self.lower_operand(&value_arg(i)?)?;
                let j = self.lower_operand(&value_arg(j)?)?;
                let func_ref = self.effect_func_ref(op);
                let call = self.builder.ins().call(func_ref, &[i, j]);
                let result = self.builder.inst_results(call)[0];
                self.write_place(dest, result)
            }
            EffectOp::Open => {
                let [path, mode] = args else {
                    return Err(CodegenError::backend("open expects exactly 2 arguments"));
                };
                let (ptr, len) = self.str_operand_parts(&value_arg(path)?)?;
                let mode = self.lower_operand(&value_arg(mode)?)?;
                let func_ref = self.effect_func_ref(op);
                let call = self.builder.ins().call(func_ref, &[ptr, len, mode]);
                let result = self.builder.inst_results(call)[0];
                self.write_place(dest, result)
            }
            EffectOp::Close => {
                let [fd] = args else {
                    return Err(CodegenError::backend("close expects exactly 1 argument"));
                };
                let fd = self.lower_operand(&value_arg(fd)?)?;
                let func_ref = self.effect_func_ref(op);
                let call = self.builder.ins().call(func_ref, &[fd]);
                let result = self.builder.inst_results(call)[0];
                self.write_place(dest, result)
            }
            EffectOp::RemoveFile => {
                let [path] = args else {
                    return Err(CodegenError::backend(
                        "remove_file expects exactly 1 argument",
                    ));
                };
                let (ptr, len) = self.str_operand_parts(&value_arg(path)?)?;
                let func_ref = self.effect_func_ref(op);
                let call = self.builder.ins().call(func_ref, &[ptr, len]);
                let result = self.builder.inst_results(call)[0];
                self.write_place(dest, result)
            }
            // ADR-0014: the socket effects — descriptor producers on the
            // same seam; a `Str` host passes as `{ptr, len}` like a path.
            EffectOp::Listen
            | EffectOp::BoundPort
            | EffectOp::Accept
            | EffectOp::Listen6
            | EffectOp::PeerFamily
            | EffectOp::UdpBind
            | EffectOp::UdpPeerPort => {
                let [scalar] = args else {
                    return Err(CodegenError::backend(
                        "listen/bound_port/accept/listen6/peer_family expects \
                         exactly 1 argument",
                    ));
                };
                let scalar = self.lower_operand(&value_arg(scalar)?)?;
                let func_ref = self.effect_func_ref(op);
                let call = self.builder.ins().call(func_ref, &[scalar]);
                let result = self.builder.inst_results(call)[0];
                self.write_place(dest, result)
            }
            EffectOp::Connect => {
                let [host, port] = args else {
                    return Err(CodegenError::backend("connect expects exactly 2 arguments"));
                };
                let (ptr, len) = self.str_operand_parts(&value_arg(host)?)?;
                let port = self.lower_operand(&value_arg(port)?)?;
                let func_ref = self.effect_func_ref(op);
                let call = self.builder.ins().call(func_ref, &[ptr, len, port]);
                let result = self.builder.inst_results(call)[0];
                self.write_place(dest, result)
            }
            // ADR-0017: the bounded-wait forms — the same operands as their
            // blocking counterparts plus a trailing millisecond deadline.
            EffectOp::AcceptTimeout | EffectOp::ReadByteTimeout => {
                let [fd, ms] = args else {
                    return Err(CodegenError::backend(
                        "accept_timeout/read_byte_timeout expects exactly 2 arguments",
                    ));
                };
                let fd = self.lower_operand(&value_arg(fd)?)?;
                let ms = self.lower_operand(&value_arg(ms)?)?;
                let func_ref = self.effect_func_ref(op);
                let call = self.builder.ins().call(func_ref, &[fd, ms]);
                let result = self.builder.inst_results(call)[0];
                self.write_place(dest, result)
            }
            EffectOp::ConnectTimeout => {
                let [host, port, ms] = args else {
                    return Err(CodegenError::backend(
                        "connect_timeout expects exactly 3 arguments",
                    ));
                };
                let (ptr, len) = self.str_operand_parts(&value_arg(host)?)?;
                let port = self.lower_operand(&value_arg(port)?)?;
                let ms = self.lower_operand(&value_arg(ms)?)?;
                let func_ref = self.effect_func_ref(op);
                let call = self.builder.ins().call(func_ref, &[ptr, len, port, ms]);
                let result = self.builder.inst_results(call)[0];
                self.write_place(dest, result)
            }
            // ADR-0017: UDP — two-scalar recv/index, and the four-operand
            // send (two of them `Str`, so six machine arguments).
            EffectOp::UdpRecv | EffectOp::UdpByteAt => {
                let [fd, second] = args else {
                    return Err(CodegenError::backend(
                        "udp_recv/udp_byte_at expects exactly 2 arguments",
                    ));
                };
                let fd = self.lower_operand(&value_arg(fd)?)?;
                let second = self.lower_operand(&value_arg(second)?)?;
                let func_ref = self.effect_func_ref(op);
                let call = self.builder.ins().call(func_ref, &[fd, second]);
                let result = self.builder.inst_results(call)[0];
                self.write_place(dest, result)
            }
            EffectOp::UdpSend => {
                let [fd, host, port, bytes] = args else {
                    return Err(CodegenError::backend(
                        "udp_send expects exactly 4 arguments",
                    ));
                };
                let fd = self.lower_operand(&value_arg(fd)?)?;
                let (hptr, hlen) = self.str_operand_parts(&value_arg(host)?)?;
                let port = self.lower_operand(&value_arg(port)?)?;
                let (bptr, blen) = self.str_operand_parts(&value_arg(bytes)?)?;
                let func_ref = self.effect_func_ref(op);
                let call = self
                    .builder
                    .ins()
                    .call(func_ref, &[fd, hptr, hlen, port, bptr, blen]);
                let result = self.builder.inst_results(call)[0];
                self.write_place(dest, result)
            }
            // ADR-0015: channels and mutexes — nullary constructors and
            // plain scalar calls, every result an `I64` handle or status.
            EffectOp::ChanNew | EffectOp::MutexNew => {
                let func_ref = self.effect_func_ref(op);
                let call = self.builder.ins().call(func_ref, &[]);
                let result = self.builder.inst_results(call)[0];
                self.write_place(dest, result)
            }
            EffectOp::ChanSend => {
                let [ch, v] = args else {
                    return Err(CodegenError::backend(
                        "chan_send expects exactly 2 arguments",
                    ));
                };
                let ch = self.lower_operand(&value_arg(ch)?)?;
                let v = self.lower_operand(&value_arg(v)?)?;
                let func_ref = self.effect_func_ref(op);
                let call = self.builder.ins().call(func_ref, &[ch, v]);
                let result = self.builder.inst_results(call)[0];
                self.write_place(dest, result)
            }
            EffectOp::ChanRecv
            | EffectOp::ChanClose
            | EffectOp::MutexLock
            | EffectOp::MutexUnlock => {
                let [handle] = args else {
                    return Err(CodegenError::backend(
                        "a channel/mutex effect expects exactly 1 argument",
                    ));
                };
                let handle = self.lower_operand(&value_arg(handle)?)?;
                let func_ref = self.effect_func_ref(op);
                let call = self.builder.ins().call(func_ref, &[handle]);
                let result = self.builder.inst_results(call)[0];
                self.write_place(dest, result)
            }
        }
    }

    /// Declare (on demand) and reference the runtime effect symbol for `op`,
    /// mirroring how the trap and `fmod` symbols are imported. The CLI links
    /// the effect C shim into every built binary, so the symbols resolve.
    fn effect_func_ref(&mut self, op: EffectOp) -> cranelift_codegen::ir::FuncRef {
        let mut signature = Signature::new(CallConv::triple_default(self.module.isa().triple()));
        let name = match op {
            EffectOp::Write => {
                signature.params.push(AbiParam::new(types::I64));
                signature.params.push(AbiParam::new(self.pointer_type));
                signature.params.push(AbiParam::new(types::I64));
                signature.returns.push(AbiParam::new(types::I64));
                effect::WRITE_SYMBOL
            }
            EffectOp::ReadByte => {
                signature.params.push(AbiParam::new(types::I64));
                signature.returns.push(AbiParam::new(types::I64));
                effect::READ_BYTE_SYMBOL
            }
            EffectOp::Exit => {
                signature.params.push(AbiParam::new(types::I64));
                effect::EXIT_SYMBOL
            }
            // `write_string` writes bytes through the same `tuo_rt_write`
            // symbol as `write` (its lowering passes `EffectOp::Write` here,
            // extracting the `{ptr, len}` from the borrowed header); this arm
            // maps to the write symbol so it is never a landmine.
            EffectOp::WriteString => {
                signature.params.push(AbiParam::new(types::I64));
                signature.params.push(AbiParam::new(self.pointer_type));
                signature.params.push(AbiParam::new(types::I64));
                signature.returns.push(AbiParam::new(types::I64));
                effect::WRITE_SYMBOL
            }
            // `par_map(f, tasks_ptr, n, workers, out_hdr)` — void; the result
            // array header is written through the out pointer (ADR-0007).
            EffectOp::ParMap => {
                signature.params.push(AbiParam::new(self.pointer_type)); // f
                signature.params.push(AbiParam::new(self.pointer_type)); // tasks
                signature.params.push(AbiParam::new(types::I64)); // n
                signature.params.push(AbiParam::new(types::I64)); // workers
                signature.params.push(AbiParam::new(self.pointer_type)); // out
                effect::PAR_MAP_SYMBOL
            }
            // ADR-0013: the OS-boundary effect symbols.
            EffectOp::NowNanos => {
                signature.returns.push(AbiParam::new(types::I64));
                effect::NOW_NANOS_SYMBOL
            }
            EffectOp::ArgCount => {
                signature.returns.push(AbiParam::new(types::I64));
                effect::ARG_COUNT_SYMBOL
            }
            EffectOp::ArgByte => {
                signature.params.push(AbiParam::new(types::I64));
                signature.params.push(AbiParam::new(types::I64));
                signature.returns.push(AbiParam::new(types::I64));
                effect::ARG_BYTE_SYMBOL
            }
            EffectOp::Open => {
                signature.params.push(AbiParam::new(self.pointer_type));
                signature.params.push(AbiParam::new(types::I64));
                signature.params.push(AbiParam::new(types::I64));
                signature.returns.push(AbiParam::new(types::I64));
                effect::OPEN_SYMBOL
            }
            EffectOp::Close => {
                signature.params.push(AbiParam::new(types::I64));
                signature.returns.push(AbiParam::new(types::I64));
                effect::CLOSE_SYMBOL
            }
            EffectOp::RemoveFile => {
                signature.params.push(AbiParam::new(self.pointer_type));
                signature.params.push(AbiParam::new(types::I64));
                signature.returns.push(AbiParam::new(types::I64));
                effect::REMOVE_FILE_SYMBOL
            }
            // ADR-0014: the socket effect symbols.
            EffectOp::Listen => {
                signature.params.push(AbiParam::new(types::I64));
                signature.returns.push(AbiParam::new(types::I64));
                effect::LISTEN_SYMBOL
            }
            EffectOp::BoundPort => {
                signature.params.push(AbiParam::new(types::I64));
                signature.returns.push(AbiParam::new(types::I64));
                effect::BOUND_PORT_SYMBOL
            }
            EffectOp::Accept => {
                signature.params.push(AbiParam::new(types::I64));
                signature.returns.push(AbiParam::new(types::I64));
                effect::ACCEPT_SYMBOL
            }
            EffectOp::Connect => {
                signature.params.push(AbiParam::new(self.pointer_type));
                signature.params.push(AbiParam::new(types::I64));
                signature.params.push(AbiParam::new(types::I64));
                signature.returns.push(AbiParam::new(types::I64));
                effect::CONNECT_SYMBOL
            }
            // ADR-0017: the IPv6 server-side symbols.
            EffectOp::Listen6 => {
                signature.params.push(AbiParam::new(types::I64));
                signature.returns.push(AbiParam::new(types::I64));
                effect::LISTEN6_SYMBOL
            }
            EffectOp::PeerFamily => {
                signature.params.push(AbiParam::new(types::I64));
                signature.returns.push(AbiParam::new(types::I64));
                effect::PEER_FAMILY_SYMBOL
            }
            // ADR-0017: the UDP effect symbols.
            EffectOp::UdpBind => {
                signature.params.push(AbiParam::new(types::I64));
                signature.returns.push(AbiParam::new(types::I64));
                effect::UDP_BIND_SYMBOL
            }
            EffectOp::UdpPeerPort => {
                signature.params.push(AbiParam::new(types::I64));
                signature.returns.push(AbiParam::new(types::I64));
                effect::UDP_PEER_PORT_SYMBOL
            }
            EffectOp::UdpRecv => {
                signature.params.push(AbiParam::new(types::I64));
                signature.params.push(AbiParam::new(types::I64));
                signature.returns.push(AbiParam::new(types::I64));
                effect::UDP_RECV_SYMBOL
            }
            EffectOp::UdpByteAt => {
                signature.params.push(AbiParam::new(types::I64));
                signature.params.push(AbiParam::new(types::I64));
                signature.returns.push(AbiParam::new(types::I64));
                effect::UDP_BYTE_AT_SYMBOL
            }
            EffectOp::UdpSend => {
                signature.params.push(AbiParam::new(types::I64));
                signature.params.push(AbiParam::new(self.pointer_type));
                signature.params.push(AbiParam::new(types::I64));
                signature.params.push(AbiParam::new(types::I64));
                signature.params.push(AbiParam::new(self.pointer_type));
                signature.params.push(AbiParam::new(types::I64));
                signature.returns.push(AbiParam::new(types::I64));
                effect::UDP_SEND_SYMBOL
            }
            // ADR-0017: the bounded-wait effect symbols.
            EffectOp::AcceptTimeout => {
                signature.params.push(AbiParam::new(types::I64));
                signature.params.push(AbiParam::new(types::I64));
                signature.returns.push(AbiParam::new(types::I64));
                effect::ACCEPT_TIMEOUT_SYMBOL
            }
            EffectOp::ReadByteTimeout => {
                signature.params.push(AbiParam::new(types::I64));
                signature.params.push(AbiParam::new(types::I64));
                signature.returns.push(AbiParam::new(types::I64));
                effect::READ_BYTE_TIMEOUT_SYMBOL
            }
            EffectOp::ConnectTimeout => {
                signature.params.push(AbiParam::new(self.pointer_type));
                signature.params.push(AbiParam::new(types::I64));
                signature.params.push(AbiParam::new(types::I64));
                signature.params.push(AbiParam::new(types::I64));
                signature.returns.push(AbiParam::new(types::I64));
                effect::CONNECT_TIMEOUT_SYMBOL
            }
            // ADR-0015: the channel and mutex effect symbols.
            EffectOp::ChanNew => {
                signature.returns.push(AbiParam::new(types::I64));
                effect::CHAN_NEW_SYMBOL
            }
            EffectOp::ChanSend => {
                signature.params.push(AbiParam::new(types::I64));
                signature.params.push(AbiParam::new(types::I64));
                signature.returns.push(AbiParam::new(types::I64));
                effect::CHAN_SEND_SYMBOL
            }
            EffectOp::ChanRecv => {
                signature.params.push(AbiParam::new(types::I64));
                signature.returns.push(AbiParam::new(types::I64));
                effect::CHAN_RECV_SYMBOL
            }
            EffectOp::ChanClose => {
                signature.params.push(AbiParam::new(types::I64));
                signature.returns.push(AbiParam::new(types::I64));
                effect::CHAN_CLOSE_SYMBOL
            }
            EffectOp::MutexNew => {
                signature.returns.push(AbiParam::new(types::I64));
                effect::MUTEX_NEW_SYMBOL
            }
            EffectOp::MutexLock => {
                signature.params.push(AbiParam::new(types::I64));
                signature.returns.push(AbiParam::new(types::I64));
                effect::MUTEX_LOCK_SYMBOL
            }
            EffectOp::MutexUnlock => {
                signature.params.push(AbiParam::new(types::I64));
                signature.returns.push(AbiParam::new(types::I64));
                effect::MUTEX_UNLOCK_SYMBOL
            }
        };
        let id = self
            .module
            .declare_function(name, Linkage::Import, &signature)
            .expect("declaring an effect runtime symbol");
        self.module.declare_func_in_func(id, self.builder.func)
    }

    /// Declare (on demand) and reference the C library's `memcmp` for `Str`
    /// equality, mirroring how `fmod` is imported (`int memcmp(const void *,
    /// const void *, size_t)`). The platform `cc` link resolves it from libc
    /// on every supported host.
    fn memcmp_func_ref(&mut self) -> cranelift_codegen::ir::FuncRef {
        let mut signature = Signature::new(CallConv::triple_default(self.module.isa().triple()));
        signature.params.push(AbiParam::new(self.pointer_type));
        signature.params.push(AbiParam::new(self.pointer_type));
        signature.params.push(AbiParam::new(types::I64));
        signature.returns.push(AbiParam::new(types::I32));
        let id = self
            .module
            .declare_function("memcmp", Linkage::Import, &signature)
            .expect("declaring the libc memcmp symbol");
        self.module.declare_func_in_func(id, self.builder.func)
    }

    // ----- heap values (ADR-0009 Stage B) -----

    /// Declare (on demand) and reference `tuo_rt_alloc(size, align) -> ptr`,
    /// mirroring how the trap/effect symbols are imported. The CLI links the
    /// allocator C shim into every built binary, so the symbol resolves.
    fn alloc_func_ref(&mut self) -> cranelift_codegen::ir::FuncRef {
        let mut signature = Signature::new(CallConv::triple_default(self.module.isa().triple()));
        signature.params.push(AbiParam::new(self.pointer_type)); // size (usize)
        signature.params.push(AbiParam::new(self.pointer_type)); // align (usize)
        signature.returns.push(AbiParam::new(self.pointer_type));
        let id = self
            .module
            .declare_function(alloc::ALLOC_SYMBOL, Linkage::Import, &signature)
            .expect("declaring the runtime alloc symbol");
        self.module.declare_func_in_func(id, self.builder.func)
    }

    /// Declare (on demand) and reference `tuo_rt_dealloc(ptr, size, align)`.
    fn dealloc_func_ref(&mut self) -> cranelift_codegen::ir::FuncRef {
        let mut signature = Signature::new(CallConv::triple_default(self.module.isa().triple()));
        signature.params.push(AbiParam::new(self.pointer_type)); // ptr
        signature.params.push(AbiParam::new(self.pointer_type)); // size (usize)
        signature.params.push(AbiParam::new(self.pointer_type)); // align (usize)
        let id = self
            .module
            .declare_function(alloc::DEALLOC_SYMBOL, Linkage::Import, &signature)
            .expect("declaring the runtime dealloc symbol");
        self.module.declare_func_in_func(id, self.builder.func)
    }

    /// Call `tuo_rt_alloc(bytes, align)` and return the buffer pointer. `bytes`
    /// is a runtime `usize` value; `align` is a compile-time constant (1 for a
    /// `String` byte buffer, `align_of(T)` for an `Array` element buffer). The
    /// runtime never returns null (it traps on OOM), so the result is a live
    /// pointer.
    fn rt_alloc(&mut self, bytes: ClifValue, align: i64) -> ClifValue {
        let align = self.builder.ins().iconst(self.pointer_type, align);
        let func_ref = self.alloc_func_ref();
        let call = self.builder.ins().call(func_ref, &[bytes, align]);
        self.builder.inst_results(call)[0]
    }

    /// Call `tuo_rt_dealloc(ptr, bytes, align)`. The C shim frees the block; a
    /// zero-`bytes` block is the sentinel and is a no-op there, but callers
    /// additionally guard `cap != 0` before ever reaching a dealloc so a
    /// sentinel is never passed.
    fn rt_dealloc(&mut self, ptr: ClifValue, bytes: ClifValue, align: i64) {
        let align = self.builder.ins().iconst(self.pointer_type, align);
        let func_ref = self.dealloc_func_ref();
        self.builder.ins().call(func_ref, &[ptr, bytes, align]);
    }

    /// The fixed non-null sentinel pointer for an empty (zero-capacity) heap
    /// value, matching `alloc::ZERO_SIZE_SENTINEL`. It is never dereferenced
    /// (`len` is 0) and never freed (`cap` is 0).
    fn zero_size_sentinel(&mut self) -> ClifValue {
        self.builder
            .ins()
            .iconst(self.pointer_type, alloc::ZERO_SIZE_SENTINEL as i64)
    }

    /// A whole-header address for `place`: a bare `String`/`Array` local's slot,
    /// a projected header, or a borrowed header parameter.
    fn header_address(&mut self, place: &Place) -> Result<ClifValue, CodegenError> {
        self.aggregate_dest_address(place)
    }

    /// Load the `{ptr, len, cap}` words of a `String`/`Array` header at `base`.
    fn load_header(&mut self, base: ClifValue) -> (ClifValue, ClifValue, ClifValue) {
        let ptr =
            self.builder
                .ins()
                .load(self.pointer_type, MemFlags::trusted(), base, HDR_PTR_OFFSET);
        let len = self
            .builder
            .ins()
            .load(types::I64, MemFlags::trusted(), base, HDR_LEN_OFFSET);
        let cap = self
            .builder
            .ins()
            .load(types::I64, MemFlags::trusted(), base, HDR_CAP_OFFSET);
        (ptr, len, cap)
    }

    /// Store the `{ptr, len, cap}` words of a `String`/`Array` header at `base`.
    fn store_header(&mut self, base: ClifValue, ptr: ClifValue, len: ClifValue, cap: ClifValue) {
        self.builder
            .ins()
            .store(MemFlags::trusted(), ptr, base, HDR_PTR_OFFSET);
        self.builder
            .ins()
            .store(MemFlags::trusted(), len, base, HDR_LEN_OFFSET);
        self.builder
            .ins()
            .store(MemFlags::trusted(), cap, base, HDR_CAP_OFFSET);
    }

    /// The element type of a growable `Array[T]` header place. Errors if the
    /// place is not an array (a `String`/scalar), which the callers never pass.
    fn array_element_ty(&self, place: &Place) -> Result<Ty, CodegenError> {
        match self.place_type(place) {
            Ty::Array(element) => Ok((*element).clone()),
            other => Err(CodegenError::backend(format!(
                "an array element was requested from a non-array place: {other:?}"
            ))),
        }
    }

    /// The element stride of a growable heap value's buffer: `1` for a
    /// `String`'s bytes, `stride(Int)` (8) for an `Array[Int]`. Derived from the
    /// header place's declared type.
    fn heap_stride(&self, place: &Place) -> Result<i64, CodegenError> {
        match self.place_type(place) {
            Ty::String => Ok(1),
            Ty::Array(element) => {
                require_native_array_element(&element, self.types)?;
                i64::try_from(self.layout(&element)?.stride())
                    .map_err(|_| CodegenError::backend("array element stride exceeds i64"))
            }
            // A map's entry stride is fixed by its key kind (ADR-0011): the
            // dense entries the `tuo_rt_map_*` shim maintains.
            Ty::Map(key, _) => Ok(map_entry_stride(&key)),
            other => Err(CodegenError::backend(format!(
                "a heap operation targeted a non-heap type: {other:?}"
            ))),
        }
    }

    /// The dealloc alignment of a heap value's buffer: `1` for a `String`,
    /// `align(Int)` for an `Array[Int]` (matching `specification/abi.md`).
    fn heap_align(&self, place: &Place) -> Result<i64, CodegenError> {
        match self.place_type(place) {
            Ty::String => Ok(1),
            Ty::Array(element) => i64::try_from(self.layout(&element)?.align)
                .map_err(|_| CodegenError::backend("array element align exceeds i64")),
            Ty::Map(..) => Ok(8),
            other => Err(CodegenError::backend(format!(
                "a heap operation targeted a non-heap type: {other:?}"
            ))),
        }
    }

    /// Materialize an aggregate-producing heap op (`string_empty`,
    /// `string_from_str`, `string_concat`, `string_slice`, `array_empty`) into
    /// `dest`'s three-word header storage, matching the interpreter's
    /// `eval_heap_op`. The subject-reading ops (`string_slice`) are handled
    /// here; the scalar reads go through `lower_heap_op_scalar`.
    fn lower_heap_op_aggregate(
        &mut self,
        dest: &Place,
        op: HeapOp,
        subject: Option<&Place>,
        args: &[Operand],
    ) -> Result<(), CodegenError> {
        match op {
            HeapOp::StringEmpty | HeapOp::ArrayEmpty | HeapOp::MapEmpty => {
                // `{ptr = sentinel, len = 0, cap = 0}` — never dereferenced,
                // never freed. For an array, refuse a wrapper-containing
                // element up front (wrapper values are not lowered anywhere)
                // so no native path half-builds an unsupported array. (A map's
                // key/value pair is already pinned to the v0 surface by the
                // type checker.)
                if let Ty::Array(element) = self.place_type(dest) {
                    require_native_array_element(&element, self.types)?;
                }
                let base = self.aggregate_dest_address(dest)?;
                let sentinel = self.zero_size_sentinel();
                let zero = self.builder.ins().iconst(types::I64, 0);
                self.store_header(base, sentinel, zero, zero);
                Ok(())
            }
            HeapOp::StringFromStr => {
                // alloc `len` bytes, copy the Str's `{ptr, len}` in, header
                // `{buf, len, len}`.
                let s = args
                    .first()
                    .ok_or_else(|| CodegenError::backend("string_from_str is missing its Str"))?;
                let (src_ptr, len) = self.str_operand_parts(s)?;
                self.build_owned_from_bytes(dest, src_ptr, len)
            }
            HeapOp::StringConcat => {
                // alloc `la + lb`, copy `a` then `b`, header `{buf, la+lb, la+lb}`.
                let [a, b] = args else {
                    return Err(CodegenError::backend(
                        "string_concat expects 2 Str operands",
                    ));
                };
                let (ptr_a, len_a) = self.str_operand_parts(a)?;
                let (ptr_b, len_b) = self.str_operand_parts(b)?;
                let total = self.builder.ins().iadd(len_a, len_b);
                let buf = self.rt_alloc(total, 1);
                self.builder
                    .call_memcpy(self.frontend_config, buf, ptr_a, len_a);
                // Copy `b` at `buf + len_a`.
                let dest_b = self.builder.ins().iadd(buf, len_a);
                self.builder
                    .call_memcpy(self.frontend_config, dest_b, ptr_b, len_b);
                let base = self.aggregate_dest_address(dest)?;
                self.store_header(base, buf, total, total);
                Ok(())
            }
            HeapOp::StringSlice => {
                // bounds-check 0 <= a <= b <= len, alloc (b - a), copy the
                // range, header `{buf, b-a, b-a}` — a COPY (owned String out).
                let subject = subject.ok_or_else(|| {
                    CodegenError::backend("string_slice is missing its String subject")
                })?;
                let [a, b] = args else {
                    return Err(CodegenError::backend(
                        "string_slice expects 2 index operands",
                    ));
                };
                let header = self.header_address(subject)?;
                let (src_ptr, len, _cap) = self.load_header(header);
                let start = self.lower_operand(a)?;
                let end = self.lower_operand(b)?;
                // `start >u end` catches `start > end` and negative `start`;
                // `end >u len` catches `end > len` and negative `end` — exactly
                // the interpreter's `0 <= a <= b <= len`.
                let bad_order = self
                    .builder
                    .ins()
                    .icmp(IntCC::UnsignedGreaterThan, start, end);
                self.guard(bad_order, TrapCode::IndexOutOfBounds);
                let bad_end = self
                    .builder
                    .ins()
                    .icmp(IntCC::UnsignedGreaterThan, end, len);
                self.guard(bad_end, TrapCode::IndexOutOfBounds);
                let count = self.builder.ins().isub(end, start);
                let range_ptr = self.builder.ins().iadd(src_ptr, start);
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
                let (ptr, len, _cap) = self.load_header(header);
                let base = self.aggregate_dest_address(dest)?;
                self.builder.ins().store(MemFlags::trusted(), ptr, base, 0);
                self.builder
                    .ins()
                    .store(MemFlags::trusted(), len, base, STR_LEN_OFFSET);
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
                let mut call_args = vec![header];
                if key_is_str {
                    let (kp, kn) = self.str_operand_parts(key)?;
                    call_args.push(kp);
                    call_args.push(kn);
                } else {
                    call_args.push(self.lower_operand(key)?);
                }
                call_args.push(out);
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
                self.call_map_shim(symbol, &[header, dest_addr])
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

    /// A fresh two-word `{found, value}` out buffer for the map shim calls,
    /// as a stack address.
    fn map_out_addr(&mut self) -> Result<ClifValue, CodegenError> {
        let slot = self.new_temp_slot(Layout::words(2))?;
        Ok(self.builder.ins().stack_addr(self.pointer_type, slot, 0))
    }

    /// Declare (idempotently) and call a `tuo_rt_map_*` shim function: every
    /// parameter is pointer-width (headers, keys, byte pointers, lengths,
    /// values, out buffers all pass as one register each), and every shim
    /// returns void — results come back through the out buffer or a written
    /// header.
    fn call_map_shim(&mut self, symbol: &str, args: &[ClifValue]) -> Result<(), CodegenError> {
        let mut signature = Signature::new(CallConv::triple_default(self.module.isa().triple()));
        for _ in args {
            signature.params.push(AbiParam::new(self.pointer_type));
        }
        let id = self
            .module
            .declare_function(symbol, Linkage::Import, &signature)
            .map_err(|error| {
                CodegenError::backend(format!("declaring the map runtime symbol: {error}"))
            })?;
        let func_ref = self.module.declare_func_in_func(id, self.builder.func);
        self.builder.ins().call(func_ref, args);
        Ok(())
    }

    /// Materialize an `Option[Int]` destination from a map shim's two-word
    /// `{found, value}` out buffer: tag = `1 - found` (`Some` is variant 0,
    /// `None` variant 1), payload = the value word (deterministically zero
    /// when absent, so no branch is needed).
    fn write_option_int_dest(&mut self, dest: &Place, out: ClifValue) -> Result<(), CodegenError> {
        let dest_ty = self.place_type(dest);
        let dest_base = self.aggregate_dest_address(dest)?;
        let found = self
            .builder
            .ins()
            .load(types::I64, MemFlags::trusted(), out, 0);
        let value = self
            .builder
            .ins()
            .load(types::I64, MemFlags::trusted(), out, 8);
        let one = self.builder.ins().iconst(types::I64, 1);
        let tag64 = self.builder.ins().isub(one, found);
        let tag = self.builder.ins().ireduce(types::I32, tag64);
        self.builder
            .ins()
            .store(MemFlags::trusted(), tag, dest_base, 0);
        let payload_offsets = variant_field_offsets(&dest_ty, 0, self.types)
            .map_err(|error| self.layout_error(error))?;
        let payload_offset = *payload_offsets
            .first()
            .ok_or_else(|| CodegenError::backend("Option Some payload has no field"))?;
        let payload_offset = i32::try_from(payload_offset)
            .map_err(|_| CodegenError::backend("Option payload offset exceeds i32"))?;
        self.builder
            .ins()
            .store(MemFlags::trusted(), value, dest_base, payload_offset);
        Ok(())
    }

    /// Build an owned `String` in `dest` from `count` bytes at `src`: alloc
    /// `count` bytes (align 1), copy them in, and store the header
    /// `{buf, count, count}`. `src` may point into `dest`'s own old buffer
    /// (`string_slice` of a `String`), so the source bytes are read *into a
    /// fresh buffer* — never aliasing the header being overwritten.
    fn build_owned_from_bytes(
        &mut self,
        dest: &Place,
        src: ClifValue,
        count: ClifValue,
    ) -> Result<(), CodegenError> {
        let buf = self.rt_alloc(count, 1);
        self.builder
            .call_memcpy(self.frontend_config, buf, src, count);
        let base = self.aggregate_dest_address(dest)?;
        self.store_header(base, buf, count, count);
        Ok(())
    }

    /// Lower a scalar-valued heap read (`string_len`, `string_byte_at`,
    /// `array_len`, `array_get`) to an `I64`, matching the interpreter's
    /// `eval_heap_op`: the length ops load the `len` word; the element reads
    /// bounds-check through the `guard()` `IndexOutOfBounds` path (one unsigned
    /// compare covers `i < 0` and `i >= len`) then load the byte/element.
    fn lower_heap_op_scalar(
        &mut self,
        op: HeapOp,
        subject: Option<&Place>,
        args: &[Operand],
    ) -> Result<ClifValue, CodegenError> {
        let subject = subject
            .ok_or_else(|| CodegenError::backend("a heap read is missing its subject place"))?;
        let header = self.header_address(subject)?;
        let (ptr, len, _cap) = self.load_header(header);
        match op {
            HeapOp::StringLen | HeapOp::ArrayLen | HeapOp::MapLen => Ok(len),
            HeapOp::MapContainsKey => {
                // `contains_key` is `get` with the value discarded: probe via
                // the shim's out buffer and produce the `found` word as Bool.
                let out = self.map_out_addr()?;
                let key_is_str = self.map_key_is_str(subject)?;
                let key = args
                    .first()
                    .ok_or_else(|| CodegenError::backend("map_contains_key is missing its key"))?;
                let mut call_args = vec![header];
                if key_is_str {
                    let (kp, kn) = self.str_operand_parts(key)?;
                    call_args.push(kp);
                    call_args.push(kn);
                } else {
                    call_args.push(self.lower_operand(key)?);
                }
                call_args.push(out);
                let symbol = if key_is_str {
                    map::MAP_STR_GET_SYMBOL
                } else {
                    map::MAP_INT_GET_SYMBOL
                };
                self.call_map_shim(symbol, &call_args)?;
                let found = self
                    .builder
                    .ins()
                    .load(types::I64, MemFlags::trusted(), out, 0);
                Ok(self.builder.ins().icmp_imm(IntCC::NotEqual, found, 0))
            }
            HeapOp::StringByteAt => {
                let index = self.heap_index_arg(args)?;
                let oob = self
                    .builder
                    .ins()
                    .icmp(IntCC::UnsignedGreaterThanOrEqual, index, len);
                self.guard(oob, TrapCode::IndexOutOfBounds);
                let addr = self.builder.ins().iadd(ptr, index);
                let byte = self
                    .builder
                    .ins()
                    .load(types::I8, MemFlags::trusted(), addr, 0);
                Ok(self.builder.ins().uextend(types::I64, byte))
            }
            HeapOp::ArrayGet => {
                let index = self.heap_index_arg(args)?;
                let oob = self
                    .builder
                    .ins()
                    .icmp(IntCC::UnsignedGreaterThanOrEqual, index, len);
                self.guard(oob, TrapCode::IndexOutOfBounds);
                // Scalar element read: address = ptr + index * stride, load the
                // element's own register width (`I64` for `Int`, `I8` for `Bool`,
                // …), ADR-0012. An aggregate element never reaches here — it is
                // routed to `lower_array_get_aggregate` by the rvalue dispatch.
                let stride = self.heap_stride(subject)?;
                let offset = self.builder.ins().imul_imm(index, stride);
                let addr = self.builder.ins().iadd(ptr, offset);
                let element = self.array_element_ty(subject)?;
                let load_ty = scalar_type(&element).ok_or_else(|| {
                    CodegenError::backend("a scalar array_get reached a non-scalar element")
                })?;
                Ok(self
                    .builder
                    .ins()
                    .load(load_ty, MemFlags::trusted(), addr, 0))
            }
            _ => Err(CodegenError::backend(
                "an aggregate-producing heap op reached `lower_heap_op_scalar`",
            )),
        }
    }

    /// Does `array::get` on this array place produce an aggregate element
    /// (`Str`/`String`/struct), rather than a scalar? (ADR-0012 Stage B.)
    fn array_get_produces_aggregate(&self, subject: &Place) -> Result<bool, CodegenError> {
        let element = self.array_element_ty(subject)?;
        Ok(scalar_type(&element).is_none())
    }

    /// Lower `array::get` whose element is an aggregate: bounds-check, then memcpy
    /// `stride` bytes from `ptr + index*stride` into the destination slot. Mirrors
    /// the scalar `get` path but reads an aggregate the way `Use` of an aggregate
    /// does. The read is a **copy** (the array retains its element); for a
    /// heap-owning element (`String`, a struct/enum carrying one) the shallow
    /// byte copy is then deep-fixed-up — every owned buffer in the copy is
    /// replaced with a fresh allocation — so the result is an independent owner,
    /// exactly the interpreter's `elements[index].clone()` (ADR-0012
    /// owned-element increment).
    fn lower_array_get_aggregate(
        &mut self,
        dest: &Place,
        subject: &Place,
        args: &[Operand],
    ) -> Result<(), CodegenError> {
        let element = self.array_element_ty(subject)?;
        let header = self.header_address(subject)?;
        let (ptr, len, _cap) = self.load_header(header);
        let index = self.heap_index_arg(args)?;
        let oob = self
            .builder
            .ins()
            .icmp(IntCC::UnsignedGreaterThanOrEqual, index, len);
        self.guard(oob, TrapCode::IndexOutOfBounds);
        let stride = self.heap_stride(subject)?;
        let offset = self.builder.ins().imul_imm(index, stride);
        let src = self.builder.ins().iadd(ptr, offset);
        let dest_addr = self.aggregate_dest_address(dest)?;
        let layout = self.layout(&element)?;
        self.emit_memcpy(dest_addr, src, layout);
        if ty_owns_heap(&element, self.types) {
            self.emit_heap_glue(&element, dest_addr, HeapGlue::DeepFixup)?;
        }
        Ok(())
    }

    /// The single integer index operand of a `byte_at`/`get` heap read.
    fn heap_index_arg(&mut self, args: &[Operand]) -> Result<ClifValue, CodegenError> {
        let index = args
            .first()
            .ok_or_else(|| CodegenError::backend("a heap index op is missing its index"))?;
        self.lower_operand(index)
    }

    /// Lower `Statement::HeapMutate` (`push_byte`/`append`/`push`/`pop`),
    /// mutating `target`'s buffer in place through its header and storing the
    /// op's result into `dest`, matching the interpreter's `exec_heap_mutate`.
    /// `push_byte` traps `InvalidByte` *before* any state change; growth is
    /// alloc-new + copy + dealloc-old, freeing the old buffer only when the old
    /// `cap != 0` (never the sentinel).
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
                // Trap `InvalidByte` unless 0 <= b <= 255 (unsigned: b >u 255),
                // before touching any memory.
                let byte = self.heap_index_arg(args)?;
                let too_big = self
                    .builder
                    .ins()
                    .icmp_imm(IntCC::UnsignedGreaterThan, byte, 255);
                self.guard(too_big, TrapCode::InvalidByte);
                let one = self.builder.ins().iconst(types::I64, 1);
                let buf = self.ensure_capacity(target, one, stride, align)?;
                // Store the byte at buf + len, then len += 1.
                let header = self.header_address(target)?;
                let (_ptr, len, _cap) = self.load_header(header);
                let addr = self.builder.ins().iadd(buf, len);
                let byte8 = self.builder.ins().ireduce(types::I8, byte);
                self.builder
                    .ins()
                    .store(MemFlags::trusted(), byte8, addr, 0);
                let new_len = self.builder.ins().iadd_imm(len, 1);
                self.builder
                    .ins()
                    .store(MemFlags::trusted(), new_len, header, HDR_LEN_OFFSET);
                self.write_unit_dest(dest)
            }
            HeapMutOp::Append => {
                // Grow to len + t.len if needed, copy t's bytes, len += t.len.
                let t = args
                    .first()
                    .ok_or_else(|| CodegenError::backend("append is missing its Str"))?;
                let (t_ptr, t_len) = self.str_operand_parts(t)?;
                let buf = self.ensure_capacity(target, t_len, stride, align)?;
                let header = self.header_address(target)?;
                let (_ptr, len, _cap) = self.load_header(header);
                let dest_addr = self.builder.ins().iadd(buf, len);
                self.builder
                    .call_memcpy(self.frontend_config, dest_addr, t_ptr, t_len);
                let new_len = self.builder.ins().iadd(len, t_len);
                self.builder
                    .ins()
                    .store(MemFlags::trusted(), new_len, header, HDR_LEN_OFFSET);
                self.write_unit_dest(dest)
            }
            HeapMutOp::Push => {
                // Grow by one element, write the element at buf + len*stride,
                // len += 1. A scalar element is a single store of its register
                // width; an aggregate element (`Str`/`String`/struct) is a memcpy
                // of `stride` bytes from the operand's slot (ADR-0012 Stage B).
                let element = self.array_element_ty(target)?;
                let one = self.builder.ins().iconst(types::I64, 1);
                let buf = self.ensure_capacity(target, one, stride, align)?;
                let header = self.header_address(target)?;
                let (_ptr, len, _cap) = self.load_header(header);
                let offset = self.builder.ins().imul_imm(len, stride);
                let addr = self.builder.ins().iadd(buf, offset);
                let arg = args
                    .first()
                    .ok_or_else(|| CodegenError::backend("push is missing its element"))?;
                if let Some(scalar) = scalar_type(&element) {
                    let v = self.lower_operand(arg)?;
                    self.builder.ins().store(MemFlags::trusted(), v, addr, 0);
                    let _ = scalar; // the operand already carries the right width
                } else {
                    // Aggregate element: memcpy its bytes from the source slot.
                    let src = self.operand_aggregate_address(arg)?;
                    let layout = self.layout(&element)?;
                    self.emit_memcpy(addr, src, layout);
                }
                let new_len = self.builder.ins().iadd_imm(len, 1);
                self.builder
                    .ins()
                    .store(MemFlags::trusted(), new_len, header, HDR_LEN_OFFSET);
                self.write_unit_dest(dest)
            }
            HeapMutOp::Pop => self.lower_array_pop(target, stride, dest),
            HeapMutOp::Set => {
                // ADR-0016: bounds-check against `len` (set never grows),
                // drop the old element's owned buffers in place, then write
                // the new element — a scalar store or a `stride`-byte memcpy
                // — matching the interpreter's `elements[index] = value`.
                let element = self.array_element_ty(target)?;
                let header = self.header_address(target)?;
                let (ptr, len, _cap) = self.load_header(header);
                let index = self.heap_index_arg(args)?;
                let oob = self
                    .builder
                    .ins()
                    .icmp(IntCC::UnsignedGreaterThanOrEqual, index, len);
                self.guard(oob, TrapCode::IndexOutOfBounds);
                let offset = self.builder.ins().imul_imm(index, stride);
                let addr = self.builder.ins().iadd(ptr, offset);
                let arg = args
                    .get(1)
                    .ok_or_else(|| CodegenError::backend("set is missing its element"))?;
                if scalar_type(&element).is_some() {
                    let v = self.lower_operand(arg)?;
                    self.builder.ins().store(MemFlags::trusted(), v, addr, 0);
                } else {
                    if ty_owns_heap(&element, self.types) {
                        self.emit_heap_glue(&element, addr, HeapGlue::DropInPlace)?;
                    }
                    let src = self.operand_aggregate_address(arg)?;
                    let layout = self.layout(&element)?;
                    self.emit_memcpy(addr, src, layout);
                }
                self.write_unit_dest(dest)
            }
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
                let mut call_args = vec![header];
                if key_is_str {
                    let (kp, kn) = self.str_operand_parts(key)?;
                    call_args.push(kp);
                    call_args.push(kn);
                } else {
                    call_args.push(self.lower_operand(key)?);
                }
                if insert {
                    let value = args.get(1).ok_or_else(|| {
                        CodegenError::backend("a map insert is missing its value")
                    })?;
                    call_args.push(self.lower_operand(value)?);
                }
                call_args.push(out);
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

    /// Ensure `target`'s buffer has room for `extra` more elements (bytes for a
    /// `String`, `stride`-wide elements for an `Array`): if `len + extra > cap`,
    /// allocate a new buffer of `max(len + extra, cap * 2, 1)` capacity, copy the
    /// live `len × stride` bytes over, free the old buffer (only when the old
    /// `cap != 0`), and update the header's `ptr`/`cap` in place. Returns the
    /// (possibly new) buffer pointer, with `len` unchanged. Capacity and buffer
    /// identity are unobservable, so the doubling policy is invisible.
    fn ensure_capacity(
        &mut self,
        target: &Place,
        extra: ClifValue,
        stride: i64,
        align: i64,
    ) -> Result<ClifValue, CodegenError> {
        let header = self.header_address(target)?;
        let (ptr, len, cap) = self.load_header(header);
        let needed = self.builder.ins().iadd(len, extra);
        // fits = needed <= cap ; if it fits, keep the buffer.
        let fits = self
            .builder
            .ins()
            .icmp(IntCC::UnsignedLessThanOrEqual, needed, cap);

        let grow_block = self.builder.create_block();
        let done_block = self.builder.create_block();
        self.builder
            .append_block_param(done_block, self.pointer_type);
        self.builder
            .ins()
            .brif(fits, done_block, &[ptr.into()], grow_block, &[]);

        // Grow: new_cap = max(needed, cap*2, 1).
        self.builder.switch_to_block(grow_block);
        self.builder.seal_block(grow_block);
        let doubled = self.builder.ins().imul_imm(cap, 2);
        let mut new_cap = self.builder.ins().umax(needed, doubled);
        let one = self.builder.ins().iconst(types::I64, 1);
        new_cap = self.builder.ins().umax(new_cap, one);
        // Allocate new_cap * stride bytes.
        let new_bytes = self.builder.ins().imul_imm(new_cap, stride);
        let new_buf = self.rt_alloc(new_bytes, align);
        // Copy the live len * stride bytes from the old buffer.
        let live_bytes = self.builder.ins().imul_imm(len, stride);
        self.builder
            .call_memcpy(self.frontend_config, new_buf, ptr, live_bytes);
        // Free the old buffer only when the old cap != 0 (never the sentinel).
        let old_cap_zero = self.builder.ins().icmp_imm(IntCC::Equal, cap, 0);
        let free_block = self.builder.create_block();
        let after_free = self.builder.create_block();
        self.builder
            .ins()
            .brif(old_cap_zero, after_free, &[], free_block, &[]);
        self.builder.switch_to_block(free_block);
        self.builder.seal_block(free_block);
        let old_bytes = self.builder.ins().imul_imm(cap, stride);
        self.rt_dealloc(ptr, old_bytes, align);
        self.builder.ins().jump(after_free, &[]);
        self.builder.switch_to_block(after_free);
        self.builder.seal_block(after_free);
        // Write the new ptr/cap into the header (len unchanged).
        self.builder
            .ins()
            .store(MemFlags::trusted(), new_buf, header, HDR_PTR_OFFSET);
        self.builder
            .ins()
            .store(MemFlags::trusted(), new_cap, header, HDR_CAP_OFFSET);
        self.builder.ins().jump(done_block, &[new_buf.into()]);

        self.builder.switch_to_block(done_block);
        self.builder.seal_block(done_block);
        Ok(self.builder.block_params(done_block)[0])
    }

    /// Lower `pop(target: mut Array[Int]) -> Option[Int]`: if `len == 0` store
    /// `None` (variant 1, empty) into `dest`; else `len -= 1`, load the last
    /// element, and store `Some { value }` (variant 0, payload at the ABI's
    /// variant-field offset). Never traps; the buffer is not shrunk (capacity is
    /// unobservable). `dest` is an `Option[Int]` aggregate.
    fn lower_array_pop(
        &mut self,
        target: &Place,
        stride: i64,
        dest: &Place,
    ) -> Result<(), CodegenError> {
        let header = self.header_address(target)?;
        let (ptr, len, _cap) = self.load_header(header);
        let dest_ty = self.place_type(dest);
        let dest_base = self.aggregate_dest_address(dest)?;
        let is_empty = self.builder.ins().icmp_imm(IntCC::Equal, len, 0);

        let none_block = self.builder.create_block();
        let some_block = self.builder.create_block();
        let join = self.builder.create_block();
        self.builder
            .ins()
            .brif(is_empty, none_block, &[], some_block, &[]);

        // None = variant 1, empty payload: store the tag only.
        self.builder.switch_to_block(none_block);
        self.builder.seal_block(none_block);
        let none_tag = self.builder.ins().iconst(types::I32, 1);
        self.builder
            .ins()
            .store(MemFlags::trusted(), none_tag, dest_base, 0);
        self.builder.ins().jump(join, &[]);

        // Some = variant 0, payload at the ABI offset: len -= 1, load the last
        // element, store the tag then the payload.
        self.builder.switch_to_block(some_block);
        self.builder.seal_block(some_block);
        let new_len = self.builder.ins().iadd_imm(len, -1);
        self.builder
            .ins()
            .store(MemFlags::trusted(), new_len, header, HDR_LEN_OFFSET);
        let offset = self.builder.ins().imul_imm(new_len, stride);
        let elem_addr = self.builder.ins().iadd(ptr, offset);
        let some_tag = self.builder.ins().iconst(types::I32, 0);
        self.builder
            .ins()
            .store(MemFlags::trusted(), some_tag, dest_base, 0);
        let payload_offsets = variant_field_offsets(&dest_ty, 0, self.types)
            .map_err(|error| self.layout_error(error))?;
        let payload_offset = *payload_offsets
            .first()
            .ok_or_else(|| CodegenError::backend("Option Some payload has no field"))?;
        let payload_offset = i64::try_from(payload_offset)
            .map_err(|_| CodegenError::backend("Option payload offset exceeds i64"))?;
        let payload_addr = self.builder.ins().iadd_imm(dest_base, payload_offset);
        // Move the popped element into the `Some` payload. A scalar element is a
        // load-and-store of its register width; an aggregate element (`Str`/
        // struct) is a memcpy of `stride` bytes (ADR-0012 Stage B). `pop` moves
        // the element out (no aliasing), so no copy semantics are needed.
        let element = self.array_element_ty(target)?;
        if let Some(load_ty) = scalar_type(&element) {
            let value = self
                .builder
                .ins()
                .load(load_ty, MemFlags::trusted(), elem_addr, 0);
            self.builder
                .ins()
                .store(MemFlags::trusted(), value, payload_addr, 0);
        } else {
            let layout = self.layout(&element)?;
            self.emit_memcpy(payload_addr, elem_addr, layout);
        }
        self.builder.ins().jump(join, &[]);

        self.builder.switch_to_block(join);
        self.builder.seal_block(join);
        Ok(())
    }

    /// A `()`-typed `HeapMutate` destination carries no value: nothing to
    /// write. (Kept explicit so the push/append paths read clearly.)
    fn write_unit_dest(&mut self, _dest: &Place) -> Result<(), CodegenError> {
        Ok(())
    }

    /// Lower `Statement::Drop` (ADR-0009 Stage B; recursive since the ADR-0012
    /// owned-element increment). A value that owns no heap drops as a no-op; a
    /// heap-owning value (a `String`, an `Array`, or an aggregate carrying one)
    /// is walked by `emit_heap_glue`, which frees element buffers before the
    /// containing buffer — the native mirror of the interpreter's
    /// de-initializing drop of a recursive `Value`. The moved-from place is
    /// de-initialized by MIR and never dropped, so a buffer is freed exactly
    /// once.
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
    /// no heap.
    ///
    /// # Errors
    ///
    /// [`CodegenError::unsupported`] for a `Box`/`Shared`/`Weak` wrapper (not
    /// lowered anywhere); [`CodegenError::backend`] on a layout failure.
    fn emit_heap_glue(
        &mut self,
        ty: &Ty,
        addr: ClifValue,
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
                    let (ptr, len, _cap) = self.load_header(addr);
                    let buf = self.rt_alloc(len, 1);
                    self.builder
                        .call_memcpy(self.frontend_config, buf, ptr, len);
                    self.store_header(addr, buf, len, len);
                    Ok(())
                }
                HeapGlue::DropInPlace => {
                    let (ptr, _len, cap) = self.load_header(addr);
                    self.emit_buffer_free(ptr, cap, 1, 1);
                    Ok(())
                }
            },
            Ty::Array(element) => {
                let stride = i64::try_from(self.layout(element)?.stride())
                    .map_err(|_| CodegenError::backend("array element stride exceeds i64"))?;
                let align = i64::try_from(self.layout(element)?.align)
                    .map_err(|_| CodegenError::backend("array element align exceeds i64"))?;
                match glue {
                    HeapGlue::DeepFixup => {
                        // Fresh buffer for the live `len` elements, shallow-copy
                        // them, then fix each copied element up in turn.
                        let (ptr, len, _cap) = self.load_header(addr);
                        let bytes = self.builder.ins().imul_imm(len, stride);
                        let buf = self.rt_alloc(bytes, align);
                        self.builder
                            .call_memcpy(self.frontend_config, buf, ptr, bytes);
                        self.store_header(addr, buf, len, len);
                        if ty_owns_heap(element, self.types) {
                            self.emit_element_loop(buf, len, stride, element, glue)?;
                        }
                        Ok(())
                    }
                    HeapGlue::DropInPlace => {
                        // Elements first (front to back, like the interpreter's
                        // `Vec` drop), then the buffer itself.
                        let (ptr, len, cap) = self.load_header(addr);
                        if ty_owns_heap(element, self.types) {
                            self.emit_element_loop(ptr, len, stride, element, glue)?;
                        }
                        self.emit_buffer_free(ptr, cap, stride, align);
                        Ok(())
                    }
                }
            }
            Ty::Struct(..) | Ty::Tuple(..) => {
                for (offset, field_ty) in self.heap_struct_fields(ty)? {
                    let field_addr = self.builder.ins().iadd_imm(addr, offset);
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
                    let stride = self.builder.ins().iconst(types::I64, map_entry_stride(key));
                    self.call_map_shim(map::MAP_DROP_SYMBOL, &[addr, stride])
                }
            },
            Ty::Enum(..) | Ty::Option(_) | Ty::Result(..) => self.emit_variant_glue(ty, addr, glue),
            Ty::FixedArray(element, count) => {
                let stride = i64::try_from(self.layout(element)?.stride())
                    .map_err(|_| CodegenError::backend("array element stride exceeds i64"))?;
                for index in 0..*count {
                    let offset = i64::try_from(index)
                        .map_err(|_| CodegenError::backend("fixed-array index exceeds i64"))?
                        * stride;
                    let element_addr = self.builder.ins().iadd_imm(addr, offset);
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
    fn heap_struct_fields(&self, ty: &Ty) -> Result<Vec<(i64, Ty)>, CodegenError> {
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
        offsets
            .into_iter()
            .zip(field_tys)
            .filter(|(_, field_ty)| ty_owns_heap(field_ty, self.types))
            .map(|(offset, field_ty)| {
                i64::try_from(offset)
                    .map(|offset| (offset, field_ty))
                    .map_err(|_| CodegenError::backend("field offset exceeds i64"))
            })
            .collect()
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
        addr: ClifValue,
        glue: HeapGlue,
    ) -> Result<(), CodegenError> {
        let payloads = self.variant_payloads(ty)?;
        let disc = self
            .builder
            .ins()
            .load(types::I32, MemFlags::trusted(), addr, 0);
        let join = self.builder.create_block();
        for (variant, fields) in payloads.iter().enumerate() {
            let heap_fields: Vec<(i64, Ty)> = {
                let offsets = variant_field_offsets(ty, variant, self.types)
                    .map_err(|e| self.layout_error(e))?;
                offsets
                    .into_iter()
                    .zip(fields.iter().cloned())
                    .filter(|(_, field_ty)| ty_owns_heap(field_ty, self.types))
                    .map(|(offset, field_ty)| {
                        i64::try_from(offset)
                            .map(|offset| (offset, field_ty))
                            .map_err(|_| CodegenError::backend("variant offset exceeds i64"))
                    })
                    .collect::<Result<_, _>>()?
            };
            if heap_fields.is_empty() {
                continue;
            }
            let variant_tag = i64::try_from(variant)
                .map_err(|_| CodegenError::backend("variant index exceeds i64"))?;
            let is_live = self.builder.ins().icmp_imm(IntCC::Equal, disc, variant_tag);
            let glue_block = self.builder.create_block();
            let next_block = self.builder.create_block();
            self.builder
                .ins()
                .brif(is_live, glue_block, &[], next_block, &[]);
            self.builder.switch_to_block(glue_block);
            self.builder.seal_block(glue_block);
            for (offset, field_ty) in heap_fields {
                let field_addr = self.builder.ins().iadd_imm(addr, offset);
                self.emit_heap_glue(&field_ty, field_addr, glue)?;
            }
            self.builder.ins().jump(join, &[]);
            self.builder.switch_to_block(next_block);
            self.builder.seal_block(next_block);
        }
        self.builder.ins().jump(join, &[]);
        self.builder.switch_to_block(join);
        self.builder.seal_block(join);
        Ok(())
    }

    /// A counted loop applying `glue` to each of the `len` elements of type
    /// `element` in the buffer at `buf` (`stride` bytes apart) — the one place
    /// codegen emits a genuine back-edge. The loop header's induction index is
    /// a block parameter; the header is sealed only after the back-edge jump.
    fn emit_element_loop(
        &mut self,
        buf: ClifValue,
        len: ClifValue,
        stride: i64,
        element: &Ty,
        glue: HeapGlue,
    ) -> Result<(), CodegenError> {
        let header_block = self.builder.create_block();
        self.builder.append_block_param(header_block, types::I64);
        let body_block = self.builder.create_block();
        let exit_block = self.builder.create_block();
        let zero = self.builder.ins().iconst(types::I64, 0);
        self.builder.ins().jump(header_block, &[zero.into()]);

        self.builder.switch_to_block(header_block);
        let index = self.builder.block_params(header_block)[0];
        let done = self
            .builder
            .ins()
            .icmp(IntCC::UnsignedGreaterThanOrEqual, index, len);
        self.builder
            .ins()
            .brif(done, exit_block, &[], body_block, &[]);

        self.builder.switch_to_block(body_block);
        self.builder.seal_block(body_block);
        let offset = self.builder.ins().imul_imm(index, stride);
        let element_addr = self.builder.ins().iadd(buf, offset);
        self.emit_heap_glue(element, element_addr, glue)?;
        let next = self.builder.ins().iadd_imm(index, 1);
        self.builder.ins().jump(header_block, &[next.into()]);
        self.builder.seal_block(header_block);

        self.builder.switch_to_block(exit_block);
        self.builder.seal_block(exit_block);
        Ok(())
    }

    /// Free a heap buffer of `cap × stride` bytes at `ptr`, guarded on
    /// `cap != 0` (an empty sentinel is never freed).
    fn emit_buffer_free(&mut self, ptr: ClifValue, cap: ClifValue, stride: i64, align: i64) {
        let has_buffer = self.builder.ins().icmp_imm(IntCC::NotEqual, cap, 0);
        let free_block = self.builder.create_block();
        let after = self.builder.create_block();
        self.builder
            .ins()
            .brif(has_buffer, free_block, &[], after, &[]);
        self.builder.switch_to_block(free_block);
        self.builder.seal_block(free_block);
        let bytes = self.builder.ins().imul_imm(cap, stride);
        self.rt_dealloc(ptr, bytes, align);
        self.builder.ins().jump(after, &[]);
        self.builder.switch_to_block(after);
        self.builder.seal_block(after);
    }

    // ----- traps -----

    /// If `condition` is true, trap with `code`: branch to a fresh trap block
    /// that calls the runtime and never returns; otherwise fall through to a
    /// fresh continuation block.
    fn guard(&mut self, condition: ClifValue, code: TrapCode) {
        let trap_block = self.builder.create_block();
        let continue_block = self.builder.create_block();
        self.builder
            .ins()
            .brif(condition, trap_block, &[], continue_block, &[]);
        self.builder.switch_to_block(trap_block);
        self.builder.seal_block(trap_block);
        self.emit_trap_call(code);
        self.builder.switch_to_block(continue_block);
        self.builder.seal_block(continue_block);
    }

    /// Emit a call to the runtime trap symbol with `code`, then an unreachable
    /// terminator (the call never returns). Terminates the current block.
    fn emit_trap_call(&mut self, code: TrapCode) {
        let func_ref = self.trap_func_ref();
        let code_value = self
            .builder
            .ins()
            .iconst(types::I32, i64::from(code.as_i32()));
        self.builder.ins().call(func_ref, &[code_value]);
        self.builder
            .ins()
            .trap(cranelift_codegen::ir::TrapCode::user(1).unwrap());
    }

    /// Declare (once per body, on demand) and reference the runtime trap symbol.
    fn trap_func_ref(&mut self) -> cranelift_codegen::ir::FuncRef {
        let mut signature = Signature::new(CallConv::triple_default(self.module.isa().triple()));
        signature.params.push(AbiParam::new(types::I32));
        let trap_id = self
            .module
            .declare_function(TRAP_SYMBOL, Linkage::Import, &signature)
            .expect("declaring the runtime trap symbol");
        self.module.declare_func_in_func(trap_id, self.builder.func)
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

    /// Whether an operand is unit-valued (a unit constant, or a bare-local place
    /// of unit/zero-sized type). Such an operand contributes no native value.
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

    /// The base address of the aggregate an operand names: a `Copy`/`Move` of a
    /// bare aggregate local's slot, or of a projected place whose leaf is an
    /// aggregate (e.g. a struct-typed array element), resolved through the
    /// shared address walk. Used for whole-aggregate copies/moves. A move
    /// leaves the source husk untouched: a v0 aggregate owns no heap, so no
    /// zeroing is needed (the ownership checker forbids re-reading a moved value).
    fn operand_aggregate_address(&mut self, operand: &Operand) -> Result<ClifValue, CodegenError> {
        match operand {
            Operand::Copy(place) | Operand::Move(place) if place.projection.is_empty() => {
                match &self.kinds[place.local.0 as usize] {
                    LocalKind::Aggregate { slot, .. } => {
                        Ok(self.builder.ins().stack_addr(self.pointer_type, *slot, 0))
                    }
                    // A whole-aggregate read of a borrowed parameter copies
                    // from the caller's memory through the incoming pointer.
                    LocalKind::Borrowed { var, .. } => Ok(self.builder.use_var(*var)),
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
            // fat pointer into a fresh temporary slot and hand that slot's
            // address to the aggregate machinery, so a literal flows through
            // copies, call arguments, and returns exactly like any aggregate
            // place.
            Operand::Const(Const::Str(text)) => {
                let text = text.clone();
                let layout = self.layout(&Ty::Str)?;
                let slot = self.new_temp_slot(layout)?;
                let addr = self.builder.ins().stack_addr(self.pointer_type, slot, 0);
                let (ptr, len) = self.str_const_parts(&text)?;
                self.builder.ins().store(MemFlags::trusted(), ptr, addr, 0);
                self.builder
                    .ins()
                    .store(MemFlags::trusted(), len, addr, STR_LEN_OFFSET);
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

    /// Turn an ABI [`LayoutError`](tuo_runtime::abi::LayoutError) into a backend
    /// error. Reaching one after classification means the MIR named a shape the
    /// ABI cannot lay out — a backend/verifier fault, not a user error.
    fn layout_error(&self, error: tuo_runtime::abi::LayoutError) -> CodegenError {
        CodegenError::backend(format!("ABI layout failed during lowering: {error}"))
    }

    /// Copy `layout.size` bytes from `src` to `dest`, both aggregate slot
    /// addresses, using the aggregate's alignment. A zero-sized copy is a no-op.
    fn emit_memcpy(&mut self, dest: ClifValue, src: ClifValue, layout: Layout) {
        if layout.size == 0 {
            return;
        }
        let align = u8::try_from(layout.align).unwrap_or(u8::MAX);
        self.builder.emit_small_memory_copy(
            self.frontend_config,
            dest,
            src,
            layout.size,
            align,
            align,
            true,
            MemFlags::trusted(),
        );
    }
}

/// A trapping arithmetic operation.
#[derive(Clone, Copy)]
enum ArithOp {
    Add,
    Sub,
    Mul,
}

/// The Cranelift condition code for a comparison operator, or `None` if `op` is
/// not a comparison. `signed` selects signed vs unsigned ordering.
fn comparison_code(op: BinOp, signed: bool) -> Option<IntCC> {
    Some(match op {
        BinOp::Eq => IntCC::Equal,
        BinOp::Ne => IntCC::NotEqual,
        BinOp::Lt if signed => IntCC::SignedLessThan,
        BinOp::Lt => IntCC::UnsignedLessThan,
        BinOp::Le if signed => IntCC::SignedLessThanOrEqual,
        BinOp::Le => IntCC::UnsignedLessThanOrEqual,
        BinOp::Gt if signed => IntCC::SignedGreaterThan,
        BinOp::Gt => IntCC::UnsignedGreaterThan,
        BinOp::Ge if signed => IntCC::SignedGreaterThanOrEqual,
        BinOp::Ge => IntCC::UnsignedGreaterThanOrEqual,
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => return None,
    })
}

/// The Cranelift float condition code for a comparison operator, or `None` if
/// `op` is not a comparison. The four orderings are **ordered** (false when
/// either operand is NaN) and `NotEqual` is **unordered-or-unequal** (true on
/// NaN) — exactly Rust's float comparisons, which the interpreter uses.
fn float_comparison_code(op: BinOp) -> Option<FloatCC> {
    Some(match op {
        BinOp::Eq => FloatCC::Equal,
        BinOp::Ne => FloatCC::NotEqual,
        BinOp::Lt => FloatCC::LessThan,
        BinOp::Le => FloatCC::LessThanOrEqual,
        BinOp::Gt => FloatCC::GreaterThan,
        BinOp::Ge => FloatCC::GreaterThanOrEqual,
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => return None,
    })
}

/// The base-2 logarithm of a power-of-two alignment, as the `align_shift` a
/// Cranelift [`StackSlotData`] expects. The ABI guarantees `align` is a power of
/// two ≥ 1, so `trailing_zeros` is exact.
fn log2_align(align: u64) -> u8 {
    // A power-of-two u64's trailing-zero count is at most 63.
    u8::try_from(align.max(1).trailing_zeros()).unwrap_or(0)
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
/// view — materialized in place by `lower_heap_op_aggregate`, rather than a
/// scalar `I64` (the length/element reads, taken by `lower_heap_op_scalar`).
/// Matches the interpreter's split in `eval_heap_op`.
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
fn map_entry_stride(key: &Ty) -> i64 {
    if matches!(key, Ty::Str) {
        #[expect(clippy::cast_possible_wrap, reason = "the stride constant is tiny")]
        {
            map::STR_ENTRY_STRIDE as i64
        }
    } else {
        #[expect(clippy::cast_possible_wrap, reason = "the stride constant is tiny")]
        {
            map::INT_ENTRY_STRIDE as i64
        }
    }
}

/// The inclusive `(min, max)` bounds of an integer kind, as `i64` (the widest
/// register the backend uses). `Usize`/`U64`'s max exceeds `i64`, so its upper
/// bound is represented by `-1` reinterpreted at 64 bits — but the backend only
/// consults `min` for the negation/overflow checks, where this is exact.
fn int_bounds(kind: IntKind) -> (i64, i64) {
    match kind {
        IntKind::I8 => (i64::from(i8::MIN), i64::from(i8::MAX)),
        IntKind::I16 => (i64::from(i16::MIN), i64::from(i16::MAX)),
        IntKind::I32 => (i64::from(i32::MIN), i64::from(i32::MAX)),
        IntKind::I64 | IntKind::Isize => (i64::MIN, i64::MAX),
        IntKind::U8 => (0, i64::from(u8::MAX)),
        IntKind::U16 => (0, i64::from(u16::MAX)),
        IntKind::U32 => (0, i64::from(u32::MAX)),
        IntKind::U64 | IntKind::Usize => (0, -1),
    }
}

/// The exported symbol name of a MIR function, from its stable symbol id.
///
/// It is derived only from the symbol (its identity), never the source name, so
/// two functions can never collide and the name is stable across renames.
fn mangle(symbol: SymbolId) -> String {
    format!("tuo_fn_{}", symbol.as_u32())
}

/// Emit a native `main` that calls the entry function and returns its value as
/// the process exit status.
///
/// `entry_id` is the [`FuncId`] the entry was declared under in this module (so
/// the call resolves directly, with no symbol-name juggling), and `entry_kind`
/// is its integer return kind (the caller checked the entry is nullary and
/// returns an integer). `main`'s standard signature is `() -> i32`; the entry's
/// return is truncated/extended to `i32`, exactly as a C `main` return becomes
/// the exit code — and exactly the value the interpreter observes running the
/// same entry.
pub(crate) fn emit_main_shim(
    module: &mut ObjectModule,
    entry_id: FuncId,
    entry_kind: IntKind,
) -> Result<(), CodegenError> {
    let call_conv = module.isa().default_call_conv();
    let mut main_sig = Signature::new(call_conv);
    main_sig.returns.push(AbiParam::new(types::I32));

    let main_id = module
        .declare_function("main", Linkage::Export, &main_sig)
        .map_err(|error| CodegenError::backend(format!("declaring main: {error}")))?;

    let mut ctx = module.make_context();
    ctx.func.signature = main_sig;

    let mut builder_ctx = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
    let block = builder.create_block();
    builder.switch_to_block(block);
    builder.seal_block(block);

    let entry_ref = module.declare_func_in_func(entry_id, builder.func);
    let call = builder.ins().call(entry_ref, &[]);
    let value = builder.inst_results(call)[0];

    // Reduce/extend the entry's integer return to i32 for the exit code. A
    // narrower signed source sign-extends; a narrower unsigned source
    // zero-extends; a wider source truncates. (The observable exit code is the
    // low bits regardless, matching a C `main`.)
    let from_ty = int_type(entry_kind);
    let ret = if from_ty == types::I32 {
        value
    } else if from_ty.bits() > 32 {
        builder.ins().ireduce(types::I32, value)
    } else if is_signed(entry_kind) {
        builder.ins().sextend(types::I32, value)
    } else {
        builder.ins().uextend(types::I32, value)
    };
    builder.ins().return_(&[ret]);
    builder.finalize();

    module
        .define_function(main_id, &mut ctx)
        .map_err(|error| CodegenError::backend(format!("defining main: {error}")))?;
    module.clear_context(&mut ctx);
    Ok(())
}
