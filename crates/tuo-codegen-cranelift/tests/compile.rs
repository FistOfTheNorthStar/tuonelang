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
fn a_projected_place_makes_a_body_unsupported() {
    // A body that reads a struct field (a projected place) is outside the
    // scalar subset; the backend must refuse rather than mis-lower. Model it as
    // `fn main() -> Int { _0.0 }` over a tuple local — verified MIR would guard
    // this, but the backend's own rejection is what we assert.
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
    let error = CraneliftBackend::new()
        .compile(
            &program,
            &TypeckResult::default(),
            "main",
            EntryAbi::IntReturn,
            &TargetSpec::host(),
        )
        .expect_err("a projected place is outside the scalar subset");
    assert_eq!(error.kind, CodegenErrorKind::Unsupported);
}
