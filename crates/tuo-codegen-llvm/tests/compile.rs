//! Backend-level tests: MIR → object, without invoking a linker.
//!
//! These check the [`CodegenBackend`] contract directly against the LLVM
//! backend, mirroring the Cranelift backend's `tests/compile.rs` — that a
//! supported program (floats and borrow-mode calls included) yields an object,
//! and that a heap-backed type is refused at classification time with a clean
//! message naming the concrete type. End-to-end *execution* agreement with the
//! interpreter is covered by the differential suites in `tuo-cli`; here we
//! exercise only the object-emission seam, so the tests need no `cc` on the
//! machine.

use tuo_codegen::{CodegenBackend, CodegenErrorKind, EntryAbi, TargetSpec};
use tuo_codegen_llvm::LlvmBackend;
use tuo_mir::{BasicBlock, Const, Function, LocalDecl, Operand, Place, Program, Terminator};
use tuo_resolve::SymbolId;
use tuo_source::{SourceId, Span, TextRange};
use tuo_types::{Ty, TypeckResult};

/// A dummy span for hand-built MIR.
fn span() -> Span {
    Span::new(
        SourceId::from_raw(0),
        TextRange::new(0, 1).expect("forward range"),
    )
}

#[test]
fn a_float_program_now_compiles() {
    // Native float support: a Float local, float arithmetic, and the
    // saturating float→int cast all lower. Model
    // `fn main() -> Int { let f = 2.5 + 0.5; f as Int }`.
    use tuo_mir::{CastKind, Rvalue};
    use tuo_types::FloatKind;
    let function = Function {
        symbol: SymbolId::from_raw(0),
        name: "main".to_owned(),
        params: Vec::new(),
        locals: vec![
            LocalDecl {
                ty: Ty::int(),
                name: None,
                span: span(),
            },
            LocalDecl {
                ty: Ty::float(),
                name: None,
                span: span(),
            },
        ],
        blocks: vec![BasicBlock {
            statements: vec![
                tuo_mir::Statement::Assign {
                    place: Place::local(tuo_mir::LocalId(1)),
                    rvalue: Rvalue::Binary {
                        op: tuo_mir::BinOp::Add,
                        lhs: Operand::Const(Const::Float(2.5, FloatKind::F64)),
                        rhs: Operand::Const(Const::Float(0.5, FloatKind::F64)),
                    },
                },
                tuo_mir::Statement::Assign {
                    place: Place::local(tuo_mir::LocalId(0)),
                    rvalue: Rvalue::Cast {
                        kind: CastKind::FloatToInt,
                        operand: Operand::Copy(Place::local(tuo_mir::LocalId(1))),
                        to: Ty::int(),
                    },
                },
            ],
            terminator: Terminator::Return(Operand::Copy(Place::local(tuo_mir::LocalId(0)))),
        }],
        ret: Ty::int(),
        span: span(),
    };
    let program = Program {
        functions: vec![function],
        skipped: Vec::new(),
    };
    LlvmBackend::new()
        .compile(
            &program,
            &TypeckResult::default(),
            "main",
            EntryAbi::IntReturn,
            &TargetSpec::host(),
        )
        .expect("a float program now compiles");
}

#[test]
fn a_borrow_mode_call_now_compiles() {
    // Borrow-mode call arguments: `fn reader(in x: Int) -> Int { x }` called
    // with `Arg::Borrow` of a caller local. The caller passes the local's
    // address and the callee reads through the pointer.
    use tuo_mir::{Arg, PassMode, Rvalue};
    let reader = Function {
        symbol: SymbolId::from_raw(1),
        name: "reader".to_owned(),
        params: vec![PassMode::Borrow],
        locals: vec![LocalDecl {
            ty: Ty::int(),
            name: None,
            span: span(),
        }],
        blocks: vec![BasicBlock {
            statements: Vec::new(),
            terminator: Terminator::Return(Operand::Copy(Place::local(tuo_mir::LocalId(0)))),
        }],
        ret: Ty::int(),
        span: span(),
    };
    let main = Function {
        symbol: SymbolId::from_raw(0),
        name: "main".to_owned(),
        params: Vec::new(),
        locals: vec![
            LocalDecl {
                ty: Ty::int(),
                name: None,
                span: span(),
            },
            LocalDecl {
                ty: Ty::int(),
                name: None,
                span: span(),
            },
        ],
        blocks: vec![BasicBlock {
            statements: vec![
                tuo_mir::Statement::Assign {
                    place: Place::local(tuo_mir::LocalId(1)),
                    rvalue: Rvalue::Use(Operand::Const(Const::Int(41, tuo_types::IntKind::I64))),
                },
                tuo_mir::Statement::Call {
                    dest: Place::local(tuo_mir::LocalId(0)),
                    callee: SymbolId::from_raw(1),
                    args: vec![Arg::Borrow(Place::local(tuo_mir::LocalId(1)))],
                },
            ],
            terminator: Terminator::Return(Operand::Copy(Place::local(tuo_mir::LocalId(0)))),
        }],
        ret: Ty::int(),
        span: span(),
    };
    let program = Program {
        functions: vec![main, reader],
        skipped: Vec::new(),
    };
    LlvmBackend::new()
        .compile(
            &program,
            &TypeckResult::default(),
            "main",
            EntryAbi::IntReturn,
            &TargetSpec::host(),
        )
        .expect("a borrow-mode call now compiles");
}

/// A `main` whose only extra local has type `ty`; used to pin the
/// classification-time refusals for heap-backed types.
fn main_with_local(ty: Ty) -> Program {
    let function = Function {
        symbol: SymbolId::from_raw(0),
        name: "main".to_owned(),
        params: Vec::new(),
        locals: vec![
            LocalDecl {
                ty: Ty::int(),
                name: None,
                span: span(),
            },
            LocalDecl {
                ty,
                name: None,
                span: span(),
            },
        ],
        blocks: vec![BasicBlock {
            statements: Vec::new(),
            terminator: Terminator::Return(Operand::Const(Const::Int(0, tuo_types::IntKind::I64))),
        }],
        ret: Ty::int(),
        span: span(),
    };
    Program {
        functions: vec![function],
        skipped: Vec::new(),
    }
}

#[test]
fn heap_types_are_refused_at_classification_time_with_a_clean_message() {
    // `Str`, `String`, the growable `Array[T]`, and the heap wrappers all have
    // an ABI layout (their headers), so without the explicit gate they would
    // slip past classification and die later on an internal invariant error.
    // The gate must refuse them up front, naming the concrete type and the
    // road back to the interpreter — with the same wording as the Cranelift
    // backend (bar the backend name).
    use tuo_types::WrapperKind;
    for (ty, marker) in [
        (Ty::Str, "a `Str` value"),
        (Ty::String, "a `String` value"),
        (Ty::Array(Box::new(Ty::int())), "the growable `Array[T]`"),
        (
            Ty::Wrapper(WrapperKind::Box, Box::new(Ty::int())),
            "`Box[T]` heap wrapper",
        ),
        (
            Ty::Wrapper(WrapperKind::Shared, Box::new(Ty::int())),
            "`Shared[T]` heap wrapper",
        ),
        (
            Ty::Wrapper(WrapperKind::Weak, Box::new(Ty::int())),
            "`Weak[T]` heap wrapper",
        ),
    ] {
        let program = main_with_local(ty);
        let error = LlvmBackend::new()
            .compile(
                &program,
                &TypeckResult::default(),
                "main",
                EntryAbi::IntReturn,
                &TargetSpec::host(),
            )
            .expect_err("a heap-backed type must be refused, never lowered");
        assert_eq!(error.kind, CodegenErrorKind::Unsupported);
        assert!(
            error.message.contains(marker),
            "the refusal should name the concrete type ({marker}); got: {}",
            error.message
        );
        assert!(
            error.message.contains("does not lower yet")
                && error.message.contains("remains the reference"),
            "the refusal should state the road back to the interpreter; got: {}",
            error.message
        );
    }
}

// ---------------------------------------------------------------------------
// ADR-0006 Stage A: Effect statements and StrOp rvalues are refused (interim)
// ---------------------------------------------------------------------------

/// `fn main() -> Int { _0 = effect read_byte(const 0); return copy _0 }` —
/// well-formed, verifiable MIR (all locals are scalars), so it reaches the
/// statement lowering and must be *refused* there, never mis-compiled,
/// until ADR-0006 Stage B lands the native effect lowering.
#[test]
fn an_effect_statement_is_refused_until_stage_b() {
    use tuo_mir::{EffectOp, Statement};
    let function = Function {
        symbol: SymbolId::from_raw(0),
        name: "main".to_owned(),
        params: Vec::new(),
        locals: vec![LocalDecl {
            ty: Ty::int(),
            name: None,
            span: span(),
        }],
        blocks: vec![BasicBlock {
            statements: vec![Statement::Effect {
                op: EffectOp::ReadByte,
                args: vec![Operand::Const(Const::Int(0, tuo_types::IntKind::I64))],
                dest: Place::local(tuo_mir::LocalId(0)),
            }],
            terminator: Terminator::Return(Operand::Copy(Place::local(tuo_mir::LocalId(0)))),
        }],
        ret: Ty::int(),
        span: span(),
    };
    let program = Program {
        functions: vec![function],
        skipped: Vec::new(),
    };
    let error = LlvmBackend::new()
        .compile(
            &program,
            &TypeckResult::default(),
            "main",
            EntryAbi::IntReturn,
            &TargetSpec::host(),
        )
        .expect_err("an effect must be refused, never mis-compiled");
    assert_eq!(error.kind, CodegenErrorKind::Unsupported);
    assert!(
        error.message.contains("std::rt::read_byte") && error.message.contains("ADR-0006 Stage B"),
        "the refusal names the effect and the road forward: {}",
        error.message
    );
}

/// `_0 = str_len(const "x")` over scalar locals: reaches the rvalue lowering
/// and must be refused there until Stage B (with `Str`'s value layout).
#[test]
fn a_str_op_rvalue_is_refused_until_stage_b() {
    use tuo_mir::{Rvalue, Statement, StrOp};
    let function = Function {
        symbol: SymbolId::from_raw(0),
        name: "main".to_owned(),
        params: Vec::new(),
        locals: vec![LocalDecl {
            ty: Ty::int(),
            name: None,
            span: span(),
        }],
        blocks: vec![BasicBlock {
            statements: vec![Statement::Assign {
                place: Place::local(tuo_mir::LocalId(0)),
                rvalue: Rvalue::StrOp {
                    op: StrOp::Len,
                    args: vec![Operand::Const(Const::Str("x".to_owned()))],
                },
            }],
            terminator: Terminator::Return(Operand::Copy(Place::local(tuo_mir::LocalId(0)))),
        }],
        ret: Ty::int(),
        span: span(),
    };
    let program = Program {
        functions: vec![function],
        skipped: Vec::new(),
    };
    let error = LlvmBackend::new()
        .compile(
            &program,
            &TypeckResult::default(),
            "main",
            EntryAbi::IntReturn,
            &TargetSpec::host(),
        )
        .expect_err("a string op must be refused, never mis-compiled");
    assert_eq!(error.kind, CodegenErrorKind::Unsupported);
    assert!(
        error.message.contains("std::str::len") && error.message.contains("ADR-0006 Stage B"),
        "the refusal names the operation and the road forward: {}",
        error.message
    );
}
