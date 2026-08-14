//! The tuonelang standard library — its source, as a catalog.
//!
//! The standard library is *written in tuonelang*: each module is a `.tuo`
//! source file under `src/std/`, embedded here at compile time. This crate is a
//! pure catalog — it holds no compiler machinery and depends on no stage crate
//! (it sits at the stdlib layer, below the compiler facade). A host that wants
//! to compile, check, or query the standard library loads these sources into
//! its own [`tuo_source::SourceMap`] and runs its own pipeline over them; the
//! catalog only says *what the modules are and where their text lives*.
//!
//! # What the library promises
//!
//! Every public function in the library carries an exact type signature, a doc
//! comment, at least one worked example, and — where it is executable in v0 — an
//! executable `spec`. There is deliberately **one** obvious API for each
//! fundamental task; the library never ships two competing spellings of the same
//! operation. These promises are enforced by the integration tests in
//! `tuo-cli` (which really compile every module and run every spec) rather than
//! asserted here.
//!
//! # Three honest tiers
//!
//! ADR-0006 landed a narrow effect boundary — the `std::rt` builtins
//! `write`/`read_byte`/`exit`, over already-open descriptors — and the spec
//! sandbox stays pure: a spec whose executed closure reaches an effect is a
//! front-end error (`R0007`). So the library is split, per function, into:
//!
//! * an **executable tier** — pure computation (ordering, `Option`/`Result`
//!   combinators, `Duration` arithmetic, error classification, the pure state
//!   models of a lock or latch), which runs and whose specs run;
//! * an **effect tier** — real tuonelang implementations over the `std::rt`
//!   effect primitives (`std::io::print`/`println`/`read_line`,
//!   `std::process::exit`). These compile and run natively, but `R0007` makes
//!   an effectful spec impossible, so each is marked `EFFECT:` in its doc and
//!   pinned by a named native CLI test (in `tuo-cli`'s `stdlib.rs`) instead of
//!   a spec (`read_line` builds an owned `String` from the bytes
//!   `std::rt::read_byte` yields — real since ADR-0009 landed the allocator);
//!   and
//! * a **contract tier** — the effectful entry points whose primitive does not
//!   exist yet (`std::fs::read` a file-open primitive; `std::time::now` a
//!   clock; `std::sync::lock` threads; `std::process::arg_count` argv), given
//!   as exact signatures with documented contracts and **no** executable spec,
//!   each marked `CONTRACT:` in its doc so no reader — human or machine — is
//!   misled into thinking it runs today. When the missing primitive lands, a
//!   contract gains an implementation with no change to its signature.
//!
//! This mirrors the toolchain's standing rule that the compiler never advertises
//! behavior it cannot perform.

/// One standard-library module: its dotted path and its tuonelang source.
#[derive(Debug, Clone, Copy)]
pub struct Module {
    /// The module's canonical dotted path, e.g. `"std::core"`.
    pub path: &'static str,
    /// A stable, file-like name for the module, e.g. `"std/core.tuo"`. Hosts use
    /// this when interning the source into a [`tuo_source::SourceMap`] so
    /// diagnostics carry a meaningful location.
    pub name: &'static str,
    /// The module's full tuonelang source text.
    pub source: &'static str,
}

/// `std::core` — ordering, arithmetic helpers, and `Option`/`Result`
/// combinators. Entirely executable.
pub const CORE: Module = Module {
    path: "std::core",
    name: "std/core.tuo",
    source: include_str!("std/core.tuo"),
};

/// `std::collections` — `Pair`, counted ranges, and pure `Array[Int]`
/// algorithms (`sum`/`max_of`/`contains`/`index_of`/`reversed`) over the
/// ADR-0009 allocator core. Entirely executable.
pub const COLLECTIONS: Module = Module {
    path: "std::collections",
    name: "std/collections.tuo",
    source: include_str!("std/collections.tuo"),
};

/// `std::io` — the I/O error vocabulary (executable), and the real
/// `print`/`println`/`read_line` I/O over the `std::rt` primitives (effect
/// tier — `read_line` builds an owned `String` from bytes read, ADR-0009).
pub const IO: Module = Module {
    path: "std::io",
    name: "std/io.tuo",
    source: include_str!("std/io.tuo"),
};

/// `std::fs` — the file-system error vocabulary and path helpers (executable),
/// and the disk-operation contract.
pub const FS: Module = Module {
    path: "std::fs",
    name: "std/fs.tuo",
    source: include_str!("std/fs.tuo"),
};

/// `std::time` — `Duration` arithmetic (executable) and the clock contract.
pub const TIME: Module = Module {
    path: "std::time",
    name: "std/time.tuo",
    source: include_str!("std/time.tuo"),
};

/// `std::process` — `ExitStatus` reasoning (executable), the real `exit` over
/// `std::rt::exit` (effect tier), and the `arg_count` contract.
pub const PROCESS: Module = Module {
    path: "std::process",
    name: "std/process.tuo",
    source: include_str!("std/process.tuo"),
};

/// `std::sync` — the pure state models of a latch and a lock (executable) and
/// the synchronization contract.
pub const SYNC: Module = Module {
    path: "std::sync",
    name: "std/sync.tuo",
    source: include_str!("std/sync.tuo"),
};

/// `std::test` — assertion-helper predicates for use inside specs. Entirely
/// executable.
pub const TEST: Module = Module {
    path: "std::test",
    name: "std/test.tuo",
    source: include_str!("std/test.tuo"),
};

/// Every standard-library module, in dependency-friendly order (`core` first).
///
/// The order is stable: hosts that load the whole library into one program can
/// rely on it. No module imports another today, so any order type-checks, but
/// `core` leads by convention.
pub const MODULES: &[Module] = &[CORE, COLLECTIONS, IO, FS, TIME, PROCESS, SYNC, TEST];

/// Look up a module by its dotted path (e.g. `"std::core"`).
#[must_use]
pub fn module(path: &str) -> Option<Module> {
    MODULES.iter().copied().find(|m| m.path == path)
}
