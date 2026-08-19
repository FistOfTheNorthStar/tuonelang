//! Cranelift backend for tuonelang.
//!
//! This crate lowers **verified** [`tuo_mir`] to native code with Cranelift,
//! implementing the tuonelang-owned [`CodegenBackend`](tuo_codegen::CodegenBackend)
//! interface. Cranelift is wrapped entirely behind that interface: no Cranelift
//! type appears in anything this crate exports, so it cannot leak into MIR, type
//! checking, the CLI protocol, or the runtime's public surface. Everything that
//! crosses the boundary is a plain tuonelang value.
//!
//! # What it lowers (v0)
//!
//! The reference semantics of a program is what [`tuo_mir_interp`] computes for
//! its MIR; this backend must agree with the interpreter instruction for
//! instruction. v0 lowers the **runnable core** the language already exercises
//! end to end:
//!
//! - integer, boolean, `Char`, and IEEE-754 float values (scalars held in
//!   machine registers);
//! - integer arithmetic with **trapping overflow** (Constitution §24), and
//!   trapping division/remainder by zero and `MIN / -1`;
//! - float arithmetic (IEEE 754, never trapping; `%` has C `fmod` semantics)
//!   and Rust-semantics float comparison (NaN: `==` false, `!=` true,
//!   orderings false);
//! - unary negation (trapping on integer `MIN`; sign-bit flip on floats) and
//!   boolean `not`;
//! - the full comparison suite;
//! - the four numeric cast directions (int↔int wrapping/extension,
//!   int→float round-to-nearest-even, float→int truncate-then-saturate with
//!   NaN → 0, float↔float IEEE conversion) — all total, never trapping;
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
//! can fall back to the interpreter (the reference). Correctness on this core
//! comes first; broadening the subset and optimizing come later.
//!
//! # Output
//!
//! [`CraneliftBackend::compile`] returns a relocatable object exporting the
//! entry (and every function it transitively calls). It also synthesizes a C
//! `main` shim that calls the entry and returns its value as the process exit
//! status, so the linked executable's exit code is the program's result — the
//! same value the interpreter observes running the same entry. Linking (with
//! the runtime's trap object) is the caller's job, per the interface.

mod abi;
mod lower;

use cranelift_codegen::settings::{self, Configurable as _};
use cranelift_codegen::{Context, isa};
use cranelift_module::{Linkage, Module};
use cranelift_object::object::macho::PLATFORM_MACOS;
use cranelift_object::object::write::MachOBuildVersion;
use cranelift_object::{ObjectBuilder, ObjectModule};
use target_lexicon::{OperatingSystem, Triple};

use tuo_codegen::{CodegenBackend, CodegenError, EntryAbi, ObjectArtifact, TargetSpec};
use tuo_mir::Program;
use tuo_types::TypeckResult;

use crate::abi::entry_returns_int;

/// The tuonelang Cranelift native backend.
///
/// It holds no state — a fresh Cranelift module is built per [`compile`] call —
/// so one instance may compile many programs.
///
/// [`compile`]: CodegenBackend::compile
#[derive(Clone, Copy, Debug, Default)]
pub struct CraneliftBackend;

impl CraneliftBackend {
    /// A new backend.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl CodegenBackend for CraneliftBackend {
    fn name(&self) -> &'static str {
        "cranelift"
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

        // The v0 entry ABI: a nullary function returning an integer, whose
        // value becomes the process exit status.
        let entry_kind = match abi {
            EntryAbi::IntReturn => {
                match (entry_fn.params.is_empty(), entry_returns_int(&entry_fn.ret)) {
                    (true, Some(kind)) => kind,
                    _ => {
                        return Err(CodegenError::unsupported(format!(
                            "the Cranelift backend can only build a nullary entry returning an \
                         integer; `{entry}` does not match that shape"
                        )));
                    }
                }
            }
        };

        let (mut module, triple) = build_module(target)?;
        let ids = lower::lower_program(&mut module, program, types)?;
        let entry_id = ids[&entry_fn.symbol];
        lower::emit_main_shim(&mut module, entry_id, entry_kind)?;

        let mut product = module.finish();
        // Stamp a Mach-O `LC_BUILD_VERSION` on Darwin/macOS targets. Without it
        // the linker warns "no platform load command found ... assuming: macOS"
        // on every build; `cranelift-object` does not set one by default.
        set_macho_platform(&mut product.object, &triple);
        let bytes = product
            .emit()
            .map_err(|error| CodegenError::backend(format!("object emission failed: {error}")))?;

        Ok(ObjectArtifact {
            bytes,
            entry_symbol: "main".to_owned(),
            target: target.clone(),
        })
    }
}

/// Build a fresh object module for `target`.
///
/// v0 supports the host target only; a different triple is refused as
/// unsupported rather than silently mis-targeted.
fn build_module(target: &TargetSpec) -> Result<(ObjectModule, Triple), CodegenError> {
    let host = TargetSpec::host();
    if target.triple != host.triple {
        return Err(CodegenError::unsupported(format!(
            "the Cranelift backend targets the host ({}) only in v0; requested `{}`",
            host.triple, target.triple
        )));
    }

    let triple: Triple = target
        .triple
        .parse()
        .map_err(|error| CodegenError::backend(format!("unrecognized target triple: {error}")))?;

    // Unoptimized, correct code first: opt_level stays at "none". PIC is
    // required for a position-independent executable on the supported hosts.
    let mut flag_builder = settings::builder();
    flag_builder
        .set("opt_level", "none")
        .map_err(|error| CodegenError::backend(format!("setting opt_level: {error}")))?;
    flag_builder
        .set("is_pic", "true")
        .map_err(|error| CodegenError::backend(format!("setting is_pic: {error}")))?;

    let isa_builder = isa::lookup(triple.clone())
        .map_err(|error| CodegenError::backend(format!("no backend for the host: {error}")))?;
    let isa = isa_builder
        .finish(settings::Flags::new(flag_builder))
        .map_err(|error| CodegenError::backend(format!("building the target ISA: {error}")))?;

    let builder = ObjectBuilder::new(
        isa,
        "tuo_program",
        cranelift_module::default_libcall_names(),
    )
    .map_err(|error| CodegenError::backend(format!("creating the object module: {error}")))?;
    Ok((ObjectModule::new(builder), triple))
}

/// Encode a `major.minor.patch` version as Mach-O's packed `X.Y.Z` `u32`
/// (`xxxx.yy.zz`), the form `LC_BUILD_VERSION` stores.
fn macho_version(major: u16, minor: u8, patch: u8) -> u32 {
    (u32::from(major) << 16) | (u32::from(minor) << 8) | u32::from(patch)
}

/// Stamp a Mach-O `LC_BUILD_VERSION` load command onto `object` when the target
/// is Darwin/macOS. `cranelift-object` emits none, so the system linker warns
/// ("no platform load command found ... assuming: macOS") on every build; this
/// records the platform explicitly. It is a no-op on non-Darwin targets (ELF /
/// COFF carry no such command).
fn set_macho_platform(object: &mut cranelift_object::object::write::Object<'_>, triple: &Triple) {
    let minos = match triple.operating_system {
        // Honor the triple's own deployment target when it carries one, so an
        // explicit `…-apple-darwin20` is not silently overridden.
        OperatingSystem::Darwin(Some(dt)) | OperatingSystem::MacOSX(Some(dt)) => {
            macho_version(dt.major, dt.minor, dt.patch)
        }
        // Versionless Darwin/macOS: a conservative floor the toolchain accepts.
        OperatingSystem::Darwin(None) | OperatingSystem::MacOSX(None) => macho_version(11, 0, 0),
        // Not a Mach-O platform — nothing to stamp.
        _ => return,
    };
    let mut build_version = MachOBuildVersion::default();
    build_version.platform = PLATFORM_MACOS;
    build_version.minos = minos;
    build_version.sdk = minos;
    object.set_macho_build_version(build_version);
}

/// A prepared, reusable Cranelift codegen context. Kept as a thin newtype so
/// the lowering module can borrow it without importing the Cranelift name.
pub(crate) struct CodegenCtx {
    ctx: Context,
}

impl CodegenCtx {
    pub(crate) fn new(module: &ObjectModule) -> Self {
        Self {
            ctx: module.make_context(),
        }
    }

    pub(crate) fn context_mut(&mut self) -> &mut Context {
        &mut self.ctx
    }

    pub(crate) fn clear(&mut self, module: &ObjectModule) {
        module.clear_context(&mut self.ctx);
    }
}

/// The linkage a compiled tuonelang function gets: exported, so the entry (and
/// the `main` shim) are visible to the linker and inter-function calls resolve.
pub(crate) const FUNCTION_LINKAGE: Linkage = Linkage::Export;
