//! Backend-level tests: MIR → object, without invoking a linker.
//!
//! These check the [`CodegenBackend`] contract directly against the Cranelift
//! backend — that a supported program yields a non-empty object exporting a
//! `main` entry, and that the structured errors fire for a missing entry, a
//! non-host target, and a program outside the scalar subset. End-to-end
//! *execution* agreement with the interpreter is covered by the differential
//! suite in `tuo-cli`; here we exercise only the object-emission seam, so the
//! tests need no `cc` on the machine.

use tuo_codegen::{CodegenBackend, CodegenErrorKind, EntryAbi, TargetSpec};
use tuo_codegen_cranelift::CraneliftBackend;
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

/// A minimal program: `fn main() -> Int { 42 }`, as a nullary entry returning a
/// constant integer — the shape the v0 entry ABI accepts.
fn constant_main(value: i128) -> Program {
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
            statements: Vec::new(),
            terminator: Terminator::Return(Operand::Const(Const::Int(
                value,
                tuo_types::IntKind::I64,
            ))),
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
fn a_supported_entry_compiles_to_a_main_object() {
    let program = constant_main(42);
    let types = TypeckResult::default();
    let artifact = CraneliftBackend::new()
        .compile(
            &program,
            &types,
            "main",
            EntryAbi::IntReturn,
            &TargetSpec::host(),
        )
        .expect("a nullary integer entry compiles");
    assert!(!artifact.bytes.is_empty(), "the backend emits object bytes");
    assert_eq!(
        artifact.entry_symbol, "main",
        "the produced object exports a C `main` entry"
    );
    assert_eq!(artifact.target, TargetSpec::host());
}

#[test]
fn a_missing_entry_is_a_distinct_error() {
    let program = constant_main(1);
    let error = CraneliftBackend::new()
        .compile(
            &program,
            &TypeckResult::default(),
            "nonexistent",
            EntryAbi::IntReturn,
            &TargetSpec::host(),
        )
        .expect_err("a missing entry cannot compile");
    assert_eq!(error.kind, CodegenErrorKind::NoSuchEntry);
}

#[test]
fn a_non_host_target_is_unsupported_in_v0() {
    let program = constant_main(1);
    let alien = TargetSpec {
        triple: "riscv64-unknown-none".to_owned(),
    };
    let error = CraneliftBackend::new()
        .compile(
            &program,
            &TypeckResult::default(),
            "main",
            EntryAbi::IntReturn,
            &alien,
        )
        .expect_err("v0 targets the host only");
    assert_eq!(error.kind, CodegenErrorKind::Unsupported);
}

#[test]
fn a_non_integer_entry_is_unsupported() {
    // An entry returning `()` does not match the exit-status ABI.
    let function = Function {
        symbol: SymbolId::from_raw(0),
        name: "main".to_owned(),
        params: Vec::new(),
        locals: vec![LocalDecl {
            ty: Ty::Unit,
            name: None,
            span: span(),
        }],
        blocks: vec![BasicBlock {
            statements: Vec::new(),
            terminator: Terminator::Return(Operand::Const(Const::Unit)),
        }],
        ret: Ty::Unit,
        span: span(),
    };
    let program = Program {
        functions: vec![function],
        skipped: Vec::new(),
    };
    let error = CraneliftBackend::new()
        .compile(
            &program,
            &TypeckResult::default(),
            "main",
            EntryAbi::IntReturn,
            &TargetSpec::host(),
        )
        .expect_err("a unit-returning entry is not a valid exit-status ABI");
    assert_eq!(error.kind, CodegenErrorKind::Unsupported);
}

#[test]
fn a_stage1_aggregate_field_read_now_compiles() {
    // ADR-0004 Stage 1: reading a scalar field of a product-type local is now
    // lowered. Model `fn main() -> Int { _1.0 }` over a tuple local: the backend
    // must accept it (the field lives in the local's stack slot), not refuse it.
    use tuo_mir::{Projection, Rvalue};
    let pair_ty = Ty::Tuple(vec![Ty::int(), Ty::int()]);
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
                ty: pair_ty,
                name: None,
                span: span(),
            },
        ],
        blocks: vec![BasicBlock {
            statements: vec![tuo_mir::Statement::Assign {
                place: Place::local(tuo_mir::LocalId(0)),
                rvalue: Rvalue::Use(Operand::Copy(Place {
                    local: tuo_mir::LocalId(1),
                    projection: vec![Projection::Field(0)],
                })),
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
    CraneliftBackend::new()
        .compile(
            &program,
            &TypeckResult::default(),
            "main",
            EntryAbi::IntReturn,
            &TargetSpec::host(),
        )
        .expect("a Stage-1 aggregate field read now compiles");
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
    CraneliftBackend::new()
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
    // address (forcing it into slot storage) and the callee reads through the
    // pointer.
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
    CraneliftBackend::new()
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
fn heap_wrapper_types_are_refused_at_classification_time_with_a_clean_message() {
    // The heap *wrappers* (`Box`/`Shared`/`Weak`) still have an ABI layout but
    // no lowering, so the gate must refuse them up front, naming the concrete
    // type and the road back to the interpreter. (`Str` is no longer refused
    // since ADR-0006 Stage B; the owned `String` and growable `Array[Int]` are
    // no longer refused since ADR-0009 Stage B — they lower as heap aggregates,
    // pinned by the positive test below and the differential suites.)
    use tuo_types::WrapperKind;
    for (ty, marker) in [
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
        let error = CraneliftBackend::new()
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

#[test]
fn an_array_index_projection_is_still_unsupported() {
    // Arrays are Stage 2: an `Index` projection must still be refused (never
    // silently lowered) even though struct/tuple/enum projections now work.
    use tuo_mir::{Projection, Rvalue};
    let array_ty = Ty::Array(Box::new(Ty::int()));
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
                ty: array_ty,
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
            statements: vec![tuo_mir::Statement::Assign {
                place: Place::local(tuo_mir::LocalId(0)),
                rvalue: Rvalue::Use(Operand::Copy(Place {
                    local: tuo_mir::LocalId(1),
                    projection: vec![Projection::Index(tuo_mir::LocalId(2))],
                })),
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
    let error = CraneliftBackend::new()
        .compile(
            &program,
            &TypeckResult::default(),
            "main",
            EntryAbi::IntReturn,
            &TargetSpec::host(),
        )
        .expect_err("array indexing is a Stage 2 concern and must be refused");
    assert_eq!(error.kind, CodegenErrorKind::Unsupported);
}

// ---------------------------------------------------------------------------
// ADR-0006 Stage B: Effect statements, StrOp rvalues, and Str values compile
// ---------------------------------------------------------------------------

/// `fn main() -> Int { _0 = effect read_byte(const 0); return copy _0 }` —
/// well-formed, verifiable MIR. Since ADR-0006 Stage B the effect lowers to a
/// direct call to the `tuo_rt_read_byte` runtime symbol, so the backend must
/// *accept* it and emit an object. (End-to-end behavior — the piped byte
/// echoing as the exit status — is pinned in `tuo-cli/tests/effects_native.rs`.)
#[test]
fn an_effect_statement_now_compiles() {
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
                args: vec![tuo_mir::Arg::Value(Operand::Const(Const::Int(
                    0,
                    tuo_types::IntKind::I64,
                )))],
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
    let artifact = CraneliftBackend::new()
        .compile(
            &program,
            &TypeckResult::default(),
            "main",
            EntryAbi::IntReturn,
            &TargetSpec::host(),
        )
        .expect("an effect statement compiles since ADR-0006 Stage B");
    assert!(!artifact.bytes.is_empty(), "the backend emits object bytes");
}

/// `_0 = str_len(const "x")` over scalar locals: since ADR-0006 Stage B the
/// literal's bytes go to static data and its length word is the result, so
/// the backend must *accept* it and emit an object. (Execution agreement with
/// the interpreter is pinned by the `str_*.tuo` differential fixtures.)
#[test]
fn a_str_op_rvalue_now_compiles() {
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
    let artifact = CraneliftBackend::new()
        .compile(
            &program,
            &TypeckResult::default(),
            "main",
            EntryAbi::IntReturn,
            &TargetSpec::host(),
        )
        .expect("a string operation compiles since ADR-0006 Stage B");
    assert!(!artifact.bytes.is_empty(), "the backend emits object bytes");
}

/// A `Str` local — the two-word `{ptr, len}` fat-pointer aggregate — now
/// classifies as ordinary aggregate storage: assigning a literal to it and
/// taking its length compiles. (`String` and friends stay refused above.)
#[test]
fn a_str_local_now_compiles() {
    use tuo_mir::{Rvalue, Statement, StrOp};
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
                ty: Ty::Str,
                name: None,
                span: span(),
            },
        ],
        blocks: vec![BasicBlock {
            statements: vec![
                Statement::Assign {
                    place: Place::local(tuo_mir::LocalId(1)),
                    rvalue: Rvalue::Use(Operand::Const(Const::Str("hé".to_owned()))),
                },
                Statement::Assign {
                    place: Place::local(tuo_mir::LocalId(0)),
                    rvalue: Rvalue::StrOp {
                        op: StrOp::Len,
                        args: vec![Operand::Copy(Place::local(tuo_mir::LocalId(1)))],
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
    CraneliftBackend::new()
        .compile(
            &program,
            &TypeckResult::default(),
            "main",
            EntryAbi::IntReturn,
            &TargetSpec::host(),
        )
        .expect("a Str local compiles since ADR-0006 Stage B");
}

// ---------------------------------------------------------------------------
// ADR-0009 Stage B: the allocator-core MIR forms now compile natively
// ---------------------------------------------------------------------------

/// A program that builds and mutates an owned `String` via ADR-0009's
/// `HeapOp`/`HeapMutate` forms, then reads its length back. Since Stage B the
/// backend must *accept* it (allocating through `tuo_rt_alloc`, growing the
/// buffer, and dropping it) and emit an object. (Execution agreement with the
/// interpreter is pinned by the `str_*`/`arr_*` differential fixtures.)
fn string_builder_main() -> Program {
    use tuo_mir::{HeapMutOp, HeapOp, LocalId, Rvalue, Statement};
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
                ty: Ty::String,
                name: None,
                span: span(),
            },
            LocalDecl {
                ty: Ty::Unit,
                name: None,
                span: span(),
            },
        ],
        blocks: vec![BasicBlock {
            statements: vec![
                Statement::Assign {
                    place: Place::local(LocalId(1)),
                    rvalue: Rvalue::HeapOp {
                        op: HeapOp::StringEmpty,
                        subject: None,
                        args: Vec::new(),
                    },
                },
                Statement::HeapMutate {
                    op: HeapMutOp::PushByte,
                    target: Place::local(LocalId(1)),
                    args: vec![Operand::Const(Const::Int(65, tuo_types::IntKind::I64))],
                    dest: Place::local(LocalId(2)),
                },
                Statement::Assign {
                    place: Place::local(LocalId(0)),
                    rvalue: Rvalue::HeapOp {
                        op: HeapOp::StringLen,
                        subject: Some(Place::local(LocalId(1))),
                        args: Vec::new(),
                    },
                },
                Statement::Drop {
                    place: Place::local(LocalId(1)),
                },
            ],
            terminator: Terminator::Return(Operand::Copy(Place::local(LocalId(0)))),
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
fn the_allocator_core_forms_now_compile() {
    let program = string_builder_main();
    let artifact = CraneliftBackend::new()
        .compile(
            &program,
            &TypeckResult::default(),
            "main",
            EntryAbi::IntReturn,
            &TargetSpec::host(),
        )
        .expect("the ADR-0009 heap forms compile natively since Stage B");
    assert!(!artifact.bytes.is_empty(), "the backend emits object bytes");
}

/// A growable `Array[Int]` built with `array_empty` + `push`, read with
/// `array_len`, and dropped. The backend must accept it and emit an object.
#[test]
fn a_growable_array_program_now_compiles() {
    use tuo_mir::{HeapMutOp, HeapOp, LocalId, Rvalue, Statement};
    let array_ty = Ty::Array(Box::new(Ty::int()));
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
                ty: array_ty,
                name: None,
                span: span(),
            },
            LocalDecl {
                ty: Ty::Unit,
                name: None,
                span: span(),
            },
        ],
        blocks: vec![BasicBlock {
            statements: vec![
                Statement::Assign {
                    place: Place::local(LocalId(1)),
                    rvalue: Rvalue::HeapOp {
                        op: HeapOp::ArrayEmpty,
                        subject: None,
                        args: Vec::new(),
                    },
                },
                Statement::HeapMutate {
                    op: HeapMutOp::Push,
                    target: Place::local(LocalId(1)),
                    args: vec![Operand::Const(Const::Int(7, tuo_types::IntKind::I64))],
                    dest: Place::local(LocalId(2)),
                },
                Statement::Assign {
                    place: Place::local(LocalId(0)),
                    rvalue: Rvalue::HeapOp {
                        op: HeapOp::ArrayLen,
                        subject: Some(Place::local(LocalId(1))),
                        args: Vec::new(),
                    },
                },
                Statement::Drop {
                    place: Place::local(LocalId(1)),
                },
            ],
            terminator: Terminator::Return(Operand::Copy(Place::local(LocalId(0)))),
        }],
        ret: Ty::int(),
        span: span(),
    };
    let program = Program {
        functions: vec![function],
        skipped: Vec::new(),
    };
    let artifact = CraneliftBackend::new()
        .compile(
            &program,
            &TypeckResult::default(),
            "main",
            EntryAbi::IntReturn,
            &TargetSpec::host(),
        )
        .expect("the growable Array[Int] compiles natively since ADR-0009 Stage B");
    assert!(!artifact.bytes.is_empty(), "the backend emits object bytes");
}

/// `write_string(fd, in s: String)` (ADR-0009 Stage B): the `EffectOp` now
/// lowers to a `tuo_rt_write` call over the borrowed header's `{ptr, len}`, so
/// the backend must accept it and emit an object.
#[test]
fn a_write_string_effect_now_compiles() {
    use tuo_mir::{Arg, EffectOp, HeapOp, LocalId, Rvalue, Statement};
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
                ty: Ty::String,
                name: None,
                span: span(),
            },
        ],
        blocks: vec![BasicBlock {
            statements: vec![
                Statement::Assign {
                    place: Place::local(LocalId(1)),
                    rvalue: Rvalue::HeapOp {
                        op: HeapOp::StringFromStr,
                        subject: None,
                        args: vec![Operand::Const(Const::Str("hi".to_owned()))],
                    },
                },
                Statement::Effect {
                    op: EffectOp::WriteString,
                    args: vec![
                        Arg::Value(Operand::Const(Const::Int(1, tuo_types::IntKind::I64))),
                        Arg::Borrow(Place::local(LocalId(1))),
                    ],
                    dest: Place::local(LocalId(0)),
                },
                Statement::Drop {
                    place: Place::local(LocalId(1)),
                },
            ],
            terminator: Terminator::Return(Operand::Copy(Place::local(LocalId(0)))),
        }],
        ret: Ty::int(),
        span: span(),
    };
    let program = Program {
        functions: vec![function],
        skipped: Vec::new(),
    };
    let artifact = CraneliftBackend::new()
        .compile(
            &program,
            &TypeckResult::default(),
            "main",
            EntryAbi::IntReturn,
            &TargetSpec::host(),
        )
        .expect("write_string compiles natively since ADR-0009 Stage B");
    assert!(!artifact.bytes.is_empty(), "the backend emits object bytes");
}
