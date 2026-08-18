//! LLVM backend for tuonelang — the release-grade native code generator.
//!
//! This crate lowers **verified** [`tuo_mir`] to native code with LLVM (via the
//! safe [`inkwell`] wrapper), implementing the tuonelang-owned
//! [`CodegenBackend`](tuo_codegen::CodegenBackend) interface. LLVM and inkwell
//! are wrapped entirely behind that interface: **no inkwell or LLVM type
//! appears in anything this crate exports**, so they cannot leak into MIR, type
//! checking, the CLI protocol, or the runtime's public surface. Everything that
//! crosses the boundary is a plain tuonelang value, exactly as with the
//! Cranelift backend.
//!
//! # Supported LLVM version
//!
//! LLVM is a large external toolchain with its own release cadence and API
//! breaks between majors; a backend must pin the major it builds against. This
//! crate is pinned to **LLVM 19** through inkwell's `llvm19-1` feature (declared
//! in `Cargo.toml`). Building it therefore requires an LLVM 19 development
//! install; `llvm-sys` (inkwell's `-sys` layer) locates it at build time from
//! `LLVM_SYS_191_PREFIX`, or from `llvm-config`/`llvm-config-19` on `PATH`, or
//! from a Homebrew `llvm@19`. Moving to a newer LLVM is a deliberate, tested
//! change: bump the inkwell feature here and update the toolchain the CI gate
//! installs, in one commit.
//!
//! # What it lowers (v0)
//!
//! The reference semantics of a program is what [`tuo_mir_interp`] computes for
//! its MIR; this backend must agree with the interpreter — and with the
//! Cranelift backend — instruction for instruction. It lowers the same
//! **runnable core**:
//!
//! - integer, boolean, `Char`, and IEEE-754 float values (scalars in virtual
//!   registers);
//! - integer arithmetic with **trapping overflow** (Constitution §24), and
//!   trapping division/remainder by zero and `MIN / -1`;
//! - float arithmetic (IEEE 754, never trapping; `%` is `frem`, C `fmod`
//!   semantics) and Rust-semantics float comparison (NaN: `==` false, `!=`
//!   true, orderings false);
//! - unary negation (trapping on integer `MIN`; sign-bit flip on floats) and
//!   boolean `not`;
//! - the full comparison suite;
//! - the four numeric cast directions (int↔int wrapping/extension,
//!   int→float round-to-nearest-even, float→int truncate-then-saturate with
//!   NaN → 0 via the `llvm.fpto{s,u}i.sat` intrinsics, float↔float IEEE
//!   conversion) — all total, never trapping;
//! - `Goto`, two-way `Branch`, multi-way `Switch`, `Return`;
//! - explicit `Assert` and `Trap` terminators, lowered to a call into the
//!   [runtime trap](tuo_runtime::TRAP_SYMBOL);
//! - direct calls and recursion, including borrow-mode (`in`/`mut`) arguments
//!   passed as pointers to the caller's place;
//! - the ADR-0004 aggregates: structs, tuples, enums, `Option`/`Result`, and
//!   fixed `[T; N]` arrays, laid out by [`tuo_runtime::abi`];
//! - the ADR-0006 Stage B strings and effects: `Str` as a two-word fat
//!   pointer over read-only static data, the `std::str` byte operations
//!   (with their deterministic `IndexOutOfBounds` traps), byte-wise `Str`
//!   equality via the C library's `memcmp`, and the `std::rt` host effects
//!   as direct calls to the [`tuo_runtime::effect`] symbols.
//!
//! Heap-owning types (`String`, the growable `Array[T]`,
//! `Box`/`Shared`/`Weak`) are **not** lowered yet (they await the allocator
//! ADR): a program that reaches one is refused with
//! [`CodegenError::unsupported`](tuo_codegen::CodegenError), and the caller
//! can fall back to the interpreter (the reference).
//!
//! # Optimization
//!
//! Correctness before optimization. LLVM's optimizer is its own evolving
//! subsystem, so this backend uses the **standard LLVM pass pipelines** rather
//! than a bespoke pass stack: [`OptLevel::None`] runs no passes (the default,
//! matching the Cranelift backend's unoptimized output) and [`OptLevel::Release`]
//! runs LLVM's stock `default<O2>` pipeline. Neither changes the program's
//! observable meaning — the differential suite pins interpreter == Cranelift ==
//! LLVM on both.
//!
//! # Output
//!
//! [`LlvmBackend::compile`] returns a relocatable object exporting the entry
//! (and every function it transitively calls). It also synthesizes a C `main`
//! shim that calls the entry and returns its value as the process exit status,
//! so the linked executable's exit code is the program's result — the same
//! value the interpreter observes running the same entry. Linking (with the
//! runtime's trap object) is the caller's job, per the interface.

mod abi;
mod lower;

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use inkwell::module::{Linkage, Module};
use inkwell::passes::PassBuilderOptions;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine, TargetTriple,
};
use inkwell::values::{BasicValue, FunctionValue};

use tuo_codegen::{CodegenBackend, CodegenError, EntryAbi, ObjectArtifact, TargetSpec};
use tuo_mir::Program;
use tuo_types::{IntKind, TypeckResult};

use crate::abi::{entry_returns_int, int_type, is_signed};
use crate::lower::lower_program;

/// The optimization the LLVM backend applies before emitting an object.
///
/// This is a tuonelang-owned enum — no LLVM type leaks through it — mapped to a
/// standard LLVM pipeline internally. It is the seam `tuo build --release`
/// drives.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum OptLevel {
    /// No optimization passes (LLVM `-O0`-equivalent): emit code directly from
    /// lowering. The default, and what a debug `tuo build`/`tuo run` uses —
    /// correctness first, and byte-for-byte closest to what the lowering emits.
    #[default]
    None,
    /// LLVM's standard `default<O2>` optimization pipeline: the release build.
    /// Uses LLVM's own evolving optimizer wholesale rather than a custom pass
    /// stack, and must not change the program's observable meaning.
    Release,
}

impl OptLevel {
    /// The LLVM code-generation optimization level for the target machine.
    fn machine_level(self) -> OptimizationLevel {
        match self {
            Self::None => OptimizationLevel::None,
            Self::Release => OptimizationLevel::Default,
        }
    }

    /// The LLVM pass-pipeline string to run over the module, or `None` to run no
    /// passes at all. `default<O2>` is LLVM's stock optimization pipeline.
    fn pass_pipeline(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Release => Some("default<O2>"),
        }
    }
}

/// The tuonelang LLVM native backend.
///
/// It holds only the [`OptLevel`] to apply — a fresh LLVM context and module are
/// built per [`compile`](CodegenBackend::compile) call — so one instance may
/// compile many programs.
///
/// Construct it with [`LlvmBackend::new`] for an unoptimized build or
/// [`LlvmBackend::release`] for the optimized release build.
#[derive(Clone, Copy, Debug, Default)]
pub struct LlvmBackend {
    opt: OptLevel,
}

impl LlvmBackend {
    /// A new backend at the default (unoptimized) optimization level.
    #[must_use]
    pub fn new() -> Self {
        Self {
            opt: OptLevel::None,
        }
    }

    /// A new backend that runs LLVM's standard release optimization pipeline.
    #[must_use]
    pub fn release() -> Self {
        Self {
            opt: OptLevel::Release,
        }
    }

    /// A new backend at the given optimization level.
    #[must_use]
    pub fn with_opt_level(opt: OptLevel) -> Self {
        Self { opt }
    }

    /// The optimization level this backend applies.
    #[must_use]
    pub fn opt_level(&self) -> OptLevel {
        self.opt
    }
}

impl CodegenBackend for LlvmBackend {
    fn name(&self) -> &'static str {
        "llvm"
    }

    fn compile(
        &self,
        program: &Program,
        types: &TypeckResult,
        entry: &str,
        abi: EntryAbi,
        target: &TargetSpec,
    ) -> Result<ObjectArtifact, CodegenError> {
        // Locate the entry; a missing entry is a distinct, non-"unsupported"
        // error so the caller can tell "not implemented" from "no such function".
        let Some(entry_fn) = program.functions.iter().find(|f| f.name == entry) else {
            return Err(CodegenError::no_such_entry(entry));
        };

        // The v0 entry ABI: a nullary function returning an integer, whose value
        // becomes the process exit status.
        let entry_kind = match abi {
            EntryAbi::IntReturn => {
                match (entry_fn.params.is_empty(), entry_returns_int(&entry_fn.ret)) {
                    (true, Some(kind)) => kind,
                    _ => {
                        return Err(CodegenError::unsupported(format!(
                            "the LLVM backend can only build a nullary entry returning an \
                             integer; `{entry}` does not match that shape"
                        )));
                    }
                }
            }
        };

        let context = Context::create();
        let module = context.create_module("tuo_program");
        let machine = target_machine(target, self.opt)?;
        module.set_triple(&machine.get_triple());
        module.set_data_layout(&machine.get_target_data().get_data_layout());

        // Lower every function, then synthesize the C-compatible `main` shim.
        let ids = lower_program(&context, &module, program, types)?;
        let entry_value = ids[&entry_fn.symbol];
        emit_main_shim(&context, &module, entry_value, entry_kind)?;

        // The module must verify before we optimize or emit — a verification
        // failure is a backend bug (we produced malformed IR), not a user error.
        module
            .verify()
            .map_err(|error| CodegenError::backend(format!("LLVM module verification: {error}")))?;

        // Run the standard optimization pipeline for the selected level.
        if let Some(pipeline) = self.opt.pass_pipeline() {
            module
                .run_passes(pipeline, &machine, PassBuilderOptions::create())
                .map_err(|error| {
                    CodegenError::backend(format!("running the `{pipeline}` pipeline: {error}"))
                })?;
        }

        // Emit a relocatable object in the target's native format.
        let buffer = machine
            .write_to_memory_buffer(&module, FileType::Object)
            .map_err(|error| CodegenError::backend(format!("object emission failed: {error}")))?;

        Ok(ObjectArtifact {
            bytes: buffer.as_slice().to_vec(),
            entry_symbol: "main".to_owned(),
            target: target.clone(),
        })
    }
}

/// Build an LLVM target machine for `target` at optimization level `opt`.
///
/// v0 supports the host target only; a different triple is refused as
/// unsupported rather than silently mis-targeted. PIC is used so the produced
/// object links into a position-independent executable on the supported hosts,
/// matching the Cranelift backend.
fn target_machine(target: &TargetSpec, opt: OptLevel) -> Result<TargetMachine, CodegenError> {
    let host = TargetSpec::host();
    if target.triple != host.triple {
        return Err(CodegenError::unsupported(format!(
            "the LLVM backend targets the host ({}) only in v0; requested `{}`",
            host.triple, target.triple
        )));
    }

    // Initialize only the native target's codegen; that is the one host target
    // v0 supports, and it keeps the linked LLVM footprint minimal.
    Target::initialize_native(&InitializationConfig::default()).map_err(|error| {
        CodegenError::backend(format!("initializing the native target: {error}"))
    })?;

    let triple = TargetTriple::create(&target.triple);
    let llvm_target = Target::from_triple(&triple)
        .map_err(|error| CodegenError::backend(format!("no LLVM target for the host: {error}")))?;

    let cpu = TargetMachine::get_host_cpu_name();
    let features = TargetMachine::get_host_cpu_features();
    llvm_target
        .create_target_machine(
            &triple,
            cpu.to_str().unwrap_or(""),
            features.to_str().unwrap_or(""),
            opt.machine_level(),
            RelocMode::PIC,
            CodeModel::Default,
        )
        .ok_or_else(|| CodegenError::backend("could not create the LLVM target machine"))
}

/// Emit a native `main` that calls the entry function and returns its value as
/// the process exit status.
///
/// `main`'s standard signature is `() -> i32`; the entry's integer return is
/// truncated/extended to `i32`, exactly as a C `main` return becomes the exit
/// code — and exactly the value the interpreter observes running the same
/// entry. This mirrors the Cranelift backend's `emit_main_shim` so a binary
/// built by either backend has an identical entry contract.
fn emit_main_shim<'ctx>(
    context: &'ctx Context,
    module: &Module<'ctx>,
    entry: FunctionValue<'ctx>,
    entry_kind: IntKind,
) -> Result<(), CodegenError> {
    let i32_type = context.i32_type();
    let main_type = i32_type.fn_type(&[], false);
    let main_fn = module.add_function("main", main_type, Some(Linkage::External));

    let block = context.append_basic_block(main_fn, "entry");
    let builder = context.create_builder();
    builder.position_at_end(block);

    let call = builder
        .build_call(entry, &[], "entry_call")
        .map_err(|error| CodegenError::backend(format!("calling the entry from main: {error}")))?;
    let value = call.try_as_basic_value().unwrap_basic().into_int_value();

    // Reduce/extend the entry's integer return to i32 for the exit code, exactly
    // as the Cranelift shim does: a wider source truncates, a narrower signed
    // source sign-extends, a narrower unsigned source zero-extends. The
    // observable exit code is the low byte regardless (a C `main` return).
    let from_ty = int_type(context, entry_kind);
    let ret = if from_ty.get_bit_width() == 32 {
        value
    } else if from_ty.get_bit_width() > 32 {
        builder
            .build_int_truncate(value, i32_type, "exit_trunc")
            .map_err(|error| CodegenError::backend(format!("truncating the exit code: {error}")))?
    } else if is_signed(entry_kind) {
        builder
            .build_int_s_extend(value, i32_type, "exit_sext")
            .map_err(|error| {
                CodegenError::backend(format!("sign-extending the exit code: {error}"))
            })?
    } else {
        builder
            .build_int_z_extend(value, i32_type, "exit_zext")
            .map_err(|error| {
                CodegenError::backend(format!("zero-extending the exit code: {error}"))
            })?
    };

    builder
        .build_return(Some(&ret as &dyn BasicValue))
        .map_err(|error| CodegenError::backend(format!("returning from main: {error}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{LlvmBackend, OptLevel};
    use tuo_codegen::{CodegenBackend as _, EntryAbi, TargetSpec};
    use tuo_mir::Program;
    use tuo_types::TypeckResult;

    #[test]
    fn the_backend_names_itself_llvm() {
        assert_eq!(LlvmBackend::new().name(), "llvm");
        assert_eq!(LlvmBackend::release().name(), "llvm");
    }

    #[test]
    fn the_constructors_select_the_expected_optimization_level() {
        assert_eq!(LlvmBackend::new().opt_level(), OptLevel::None);
        assert_eq!(LlvmBackend::release().opt_level(), OptLevel::Release);
        assert_eq!(
            LlvmBackend::with_opt_level(OptLevel::Release).opt_level(),
            OptLevel::Release
        );
        // The default backend is unoptimized (correctness-first).
        assert_eq!(LlvmBackend::default().opt_level(), OptLevel::None);
    }

    #[test]
    fn a_missing_entry_is_reported_as_no_such_entry_not_unsupported() {
        // An empty program has no `main`; the backend must distinguish "no such
        // function" from "feature not implemented" so the CLI reports it right.
        let program = Program::default();
        let types = TypeckResult::default();
        let error = LlvmBackend::new()
            .compile(
                &program,
                &types,
                "main",
                EntryAbi::IntReturn,
                &TargetSpec::host(),
            )
            .expect_err("an empty program has no entry to compile");
        assert!(
            !error.is_unsupported(),
            "a missing entry is a distinct error, not `unsupported`"
        );
    }
}
