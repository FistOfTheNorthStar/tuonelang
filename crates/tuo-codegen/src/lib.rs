//! Backend-agnostic code-generation interface for tuonelang.
//!
//! This crate defines the tuonelang-owned seam every native backend implements,
//! so the MIR consumers above it (the CLI's `build`/`run`, later the package
//! driver) stay uniform and backend-independent. It depends on [`tuo_mir`] but
//! on **no** concrete backend: Cranelift and LLVM sit *below* this interface and
//! remain interchangeable behind it.
//!
//! # The contract
//!
//! A [`CodegenBackend`] consumes **verified** MIR — a [`tuo_mir::Program`] the
//! caller has already run through [`tuo_mir::verify`] — plus the
//! [`tuo_types::TypeckResult`] it was checked against, and lowers one named
//! entry function to a relocatable [`ObjectArtifact`]. The caller links that
//! object into an executable (with the tuonelang runtime) and runs it. The
//! interface deliberately stops at "emit an object": linking and process
//! management are the caller's concern, not the backend's, so a backend never
//! shells out.
//!
//! # What must not leak
//!
//! The whole point of this crate is a firewall. No Cranelift/LLVM type, and no
//! detail of either backend's IR, appears in any type this module exposes — so
//! it cannot leak into MIR, type checking, the CLI protocol, or the runtime's
//! public surface. A backend translates *from* MIR *to* its own IR entirely
//! behind [`CodegenBackend::compile`]; everything that crosses the boundary is
//! a plain tuonelang-owned value ([`TargetSpec`], [`ObjectArtifact`],
//! [`CodegenError`]).
//!
//! # Correctness before optimization
//!
//! The reference semantics of a tuonelang program is what [`tuo_mir_interp`]
//! computes for its MIR. A backend must agree with the interpreter instruction
//! for instruction; where they diverge, the backend is wrong. This interface
//! says nothing about optimization — a backend is free to emit naive,
//! unoptimized code, and correctness is required first.

use std::fmt;

use tuo_mir::Program;
use tuo_types::TypeckResult;

/// The native target a backend generates code for.
///
/// v0 supports exactly one target: the host the compiler runs on
/// ([`TargetSpec::host`]). The type exists so the interface is already
/// target-parameterized when cross-compilation lands, without any backend
/// signature changing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetSpec {
    /// The target triple, in the conventional `arch-vendor-os` form (e.g.
    /// `aarch64-apple-darwin`). A backend maps this to its own target
    /// description internally; the string is the tuonelang-owned identity.
    pub triple: String,
}

impl TargetSpec {
    /// The target describing the current host.
    ///
    /// This is the only target v0 backends are required to support. The triple
    /// is the one the compiler itself was built for.
    #[must_use]
    pub fn host() -> Self {
        Self {
            triple: current_host_triple().to_owned(),
        }
    }
}

/// The host triple this compiler was built for, as a compile-time constant.
///
/// It comes from the `TARGET` Cargo sets when building this crate, so it always
/// matches the machine the `tuo` binary runs on — the one target every v0
/// backend must handle.
#[must_use]
pub const fn current_host_triple() -> &'static str {
    env!("TUO_CODEGEN_HOST_TRIPLE")
}

/// A relocatable object produced by a backend.
///
/// It is exactly the bytes an object file would contain on disk (the platform's
/// native format — Mach-O, ELF, COFF). The caller writes it out and links it
/// into an executable together with the tuonelang runtime. The artifact records
/// the symbol name of the emitted entry so the caller (or the runtime's C
/// `main`) can reference it.
#[derive(Clone, Debug)]
pub struct ObjectArtifact {
    /// The raw object-file bytes, in the target's native container format.
    pub bytes: Vec<u8>,
    /// The exported symbol name of the compiled entry function. The runtime's
    /// entry shim calls this symbol; the caller passes it to the linker.
    pub entry_symbol: String,
    /// The target this object was generated for.
    pub target: TargetSpec,
}

/// Why a backend could not lower a program.
///
/// A backend never panics on verified MIR: input it cannot handle yet is
/// reported as a structured error, exactly as MIR lowering itself records the
/// bodies it declines. The distinction between the variants is *whose* problem
/// it is — [`Unsupported`](CodegenError::kind) marks a program feature the
/// backend has not implemented (the caller may still run it on the
/// interpreter), while the others mark a genuine failure.
#[derive(Clone, Debug)]
pub struct CodegenError {
    /// The category of failure.
    pub kind: CodegenErrorKind,
    /// A human-readable explanation.
    pub message: String,
}

/// The category of a [`CodegenError`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CodegenErrorKind {
    /// The program uses a language feature this backend does not lower yet.
    /// The MIR is well-formed; the backend simply has no rule for it. The
    /// caller can fall back to the interpreter, which is the reference
    /// semantics.
    Unsupported,
    /// The requested entry function is not present in the program.
    NoSuchEntry,
    /// The backend's own machinery failed (target lookup, object emission).
    /// This is a backend or environment fault, not a user error.
    Backend,
}

impl CodegenError {
    /// A feature the backend does not lower yet.
    #[must_use]
    pub fn unsupported(message: impl Into<String>) -> Self {
        Self {
            kind: CodegenErrorKind::Unsupported,
            message: message.into(),
        }
    }

    /// The requested entry function does not exist in the program.
    #[must_use]
    pub fn no_such_entry(name: &str) -> Self {
        Self {
            kind: CodegenErrorKind::NoSuchEntry,
            message: format!("the program has no function named `{name}` to compile"),
        }
    }

    /// An internal backend or environment failure.
    #[must_use]
    pub fn backend(message: impl Into<String>) -> Self {
        Self {
            kind: CodegenErrorKind::Backend,
            message: message.into(),
        }
    }

    /// Whether this error means "not implemented yet" rather than a real fault.
    #[must_use]
    pub fn is_unsupported(&self) -> bool {
        self.kind == CodegenErrorKind::Unsupported
    }
}

impl fmt::Display for CodegenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for CodegenError {}

/// How the compiled entry reports its result to the runtime.
///
/// v0's observable output is a single integer the entry returns, which the
/// runtime shim turns into the process exit status. This mirrors what the
/// interpreter observes when it runs the same entry: the returned value is the
/// program's result. The enum names the ABI shape so the runtime shim and the
/// backend agree on how the entry hands its value back.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EntryAbi {
    /// The entry takes no arguments and returns a machine-word integer, which
    /// the runtime interprets as the exit status (truncated to the platform's
    /// exit-code width, exactly as a C `main` return is). This is the only
    /// shape v0 supports; a non-integer or parameterized entry is
    /// [`CodegenError::unsupported`].
    IntReturn,
}

/// A tuonelang native code-generation backend.
///
/// One implementation exists per backend (Cranelift today, LLVM later). The
/// interface is intentionally small: hand it verified MIR and an entry name,
/// get back an object file. Everything backend-specific — instruction
/// selection, register allocation, the backend's own IR — lives entirely
/// inside [`compile`](CodegenBackend::compile) and never crosses this boundary.
pub trait CodegenBackend {
    /// A stable, human-readable name for this backend (e.g. `"cranelift"`),
    /// for diagnostics and `--help` text.
    fn name(&self) -> &'static str;

    /// Lower the `entry` function of a **verified** `program` to a relocatable
    /// object for `target`, using `types` for the type information the front
    /// end computed.
    ///
    /// `program` must already have passed [`tuo_mir::verify`] — a backend
    /// assumes verified MIR, exactly as the interpreter does, and does not
    /// re-check it. `entry` names the function whose compiled symbol the
    /// runtime will call; `abi` fixes how that entry returns its result.
    ///
    /// The emitted object exports the entry (and every function it calls,
    /// transitively). The caller links it with the runtime into an executable.
    ///
    /// # Errors
    ///
    /// Returns [`CodegenError::no_such_entry`] if `entry` is absent,
    /// [`CodegenError::unsupported`] if the reachable program uses a feature
    /// this backend does not lower yet (the caller may fall back to the
    /// interpreter), and [`CodegenError::backend`] on an internal failure.
    fn compile(
        &self,
        program: &Program,
        types: &TypeckResult,
        entry: &str,
        abi: EntryAbi,
        target: &TargetSpec,
    ) -> Result<ObjectArtifact, CodegenError>;
}
