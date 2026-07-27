//! Runtime support for compiled tuonelang programs, and the definition of the
//! tuonelang **runtime ABI**.
//!
//! This crate is the minimal native runtime that generated tuonelang binaries
//! link against *and* the single normative home of the ABI those binaries
//! obey — how values are laid out in memory, how a program traps, starts, and
//! exits, how it acquires and releases memory, and the internal calling
//! conventions. It is deliberately independent of the compiler front-end and of
//! any backend: it holds only what a *running* program needs and the contract a
//! backend must satisfy, never anything Cranelift- or LLVM-specific. The prose
//! specification is [`specification/abi.md`](../../../specification/abi.md);
//! this crate is its implementation, and the two are kept in step.
//!
//! # What it provides
//!
//! - **The ABI layouts** ([`abi`]) — the backend-independent size, alignment,
//!   field offsets, and enum discriminant numbering of every tuonelang type,
//!   computed from `tuo-types` alone. A backend consults these; it never
//!   defines its own. The ABI carries an explicit [`abi::ABI_VERSION`], bumped
//!   on any layout-affecting change (versioned before it is frozen).
//! - **The trap** ([`TRAP_SYMBOL`], [`TrapCode`]) — the native counterpart of
//!   the interpreter's structured abort. At a language trap (integer overflow,
//!   division by zero, an out-of-bounds index, or a proved-unreachable point —
//!   Constitution §24), generated code calls [`TRAP_SYMBOL`] with a
//!   [`TrapCode`]; the runtime prints a stable one-line message to stderr and
//!   aborts with [`TRAP_EXIT_STATUS`], **without unwinding** (§24: no
//!   destructors run). This mirrors the interpreter, whose traps likewise abort
//!   a run deterministically.
//! - **The allocation boundary** ([`alloc`]) — the two C-ABI symbols
//!   ([`alloc::ALLOC_SYMBOL`], [`alloc::DEALLOC_SYMBOL`]) through which every
//!   heap allocation (`Box`, `Shared`, `String`, `Array`) flows, so the
//!   allocator is one swappable seam.
//!
//! # How it reaches a generated binary
//!
//! The runtime's C-ABI symbols must be present in the final executable, but the
//! runtime is a Rust library, not something a generated object links to
//! directly. So this crate exposes its trap and its allocator as portable **C
//! source** ([`trap_runtime_c_source`], [`alloc::alloc_runtime_c_source`]) that
//! the build driver compiles and links alongside the backend's object. Keeping
//! them as C (a) needs no Rust-runtime machinery in the target binary and (b)
//! makes the ABI contract between backend and runtime explicit and
//! inspectable.
//!
//! The same contracts are pinned natively — [`report_trap`] for the trap and
//! [`alloc::alloc_decision`] for the allocator's policy — so this crate's own
//! tests fix the exit status, message shape, and allocation policy without
//! invoking a C compiler.

pub mod abi;
pub mod alloc;

use std::process::abort;

/// The name of the C-ABI symbol generated code calls to trap.
///
/// A backend emits, at each trap site, a call to this symbol passing the
/// [`TrapCode`] as its single integer argument. The symbol never returns.
pub const TRAP_SYMBOL: &str = "tuo_rt_trap";

/// The process exit status a trapped tuonelang program terminates with.
///
/// It is a fixed, non-zero code so a trap is always distinguishable from a
/// normal `0` (or any small integer a program deliberately returns). `134` is
/// the conventional "aborted" status (`128 + SIGABRT`) on Unix, which matches
/// the `abort()` this runtime performs.
pub const TRAP_EXIT_STATUS: i32 = 134;

/// The stable cause of a runtime trap, shared with [`tuo_mir`]'s trap taxonomy
/// and the arithmetic traps the interpreter defines.
///
/// The discriminants are the integer contract between a backend (which passes
/// one to [`TRAP_SYMBOL`]) and the runtime (which maps it to a message). They
/// are stable: a backend and the runtime must always agree on them, so they are
/// written explicitly and only ever appended to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(i32)]
pub enum TrapCode {
    /// Integer arithmetic overflowed (add/sub/mul/neg/div-overflow, §24).
    IntegerOverflow = 0,
    /// Integer division or remainder by zero.
    DivisionByZero = 1,
    /// An array index was out of bounds.
    IndexOutOfBounds = 2,
    /// Control reached a point the front end proved unreachable.
    Unreachable = 3,
}

impl TrapCode {
    /// The stable integer a backend passes to [`TRAP_SYMBOL`].
    #[must_use]
    pub const fn as_i32(self) -> i32 {
        self as i32
    }

    /// Reconstruct a code from its integer, or `None` if it is unknown.
    #[must_use]
    pub const fn from_i32(value: i32) -> Option<Self> {
        match value {
            0 => Some(Self::IntegerOverflow),
            1 => Some(Self::DivisionByZero),
            2 => Some(Self::IndexOutOfBounds),
            3 => Some(Self::Unreachable),
            _ => None,
        }
    }

    /// The stable one-line message printed to stderr when this trap fires.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::IntegerOverflow => "tuo: trap: integer overflow",
            Self::DivisionByZero => "tuo: trap: division by zero",
            Self::IndexOutOfBounds => "tuo: trap: index out of bounds",
            Self::Unreachable => "tuo: trap: reached unreachable code",
        }
    }
}

/// The native trap behavior, in Rust: print the stable message and abort.
///
/// This is the reference for what [`trap_runtime_c_source`] compiles to. It
/// never returns. An unrecognized `code` still aborts (a backend must never
/// pass one, but the runtime fails safe rather than continuing).
#[expect(
    clippy::print_stderr,
    reason = "this is the runtime's trap reporter: a trap message is written to stderr by design"
)]
pub fn report_trap(code: i32) -> ! {
    let message = TrapCode::from_i32(code).map_or("tuo: trap: unknown", TrapCode::message);
    // A trap goes to stderr, not stdout: stdout is the program's own output.
    eprintln!("{message}");
    abort();
}

/// The C source of the runtime's trap implementation.
///
/// The build driver writes this to a `.c` file, compiles it, and links it into
/// the final executable so [`TRAP_SYMBOL`] resolves. It implements exactly the
/// behavior of [`report_trap`]: a stable stderr message keyed by [`TrapCode`],
/// then `abort()` (whose status is [`TRAP_EXIT_STATUS`] on the supported hosts).
///
/// It is intentionally freestanding C — only `<stdio.h>` and `<stdlib.h>` — so
/// it compiles anywhere the platform `cc` runs and drags no Rust runtime into
/// the target binary.
#[must_use]
pub fn trap_runtime_c_source() -> String {
    // Keep the messages here byte-identical to `TrapCode::message`, and the
    // codes identical to `TrapCode`'s discriminants; the native test below
    // guards the Rust side, and this string is its C mirror.
    let mut source = String::new();
    source.push_str("#include <stdio.h>\n");
    source.push_str("#include <stdlib.h>\n\n");
    source.push_str("void tuo_rt_trap(int code) {\n");
    source.push_str("    const char *message;\n");
    source.push_str("    switch (code) {\n");
    for (code, message) in [
        (
            TrapCode::IntegerOverflow,
            TrapCode::IntegerOverflow.message(),
        ),
        (TrapCode::DivisionByZero, TrapCode::DivisionByZero.message()),
        (
            TrapCode::IndexOutOfBounds,
            TrapCode::IndexOutOfBounds.message(),
        ),
        (TrapCode::Unreachable, TrapCode::Unreachable.message()),
    ] {
        source.push_str(&format!(
            "        case {}: message = \"{}\"; break;\n",
            code.as_i32(),
            message
        ));
    }
    source.push_str("        default: message = \"tuo: trap: unknown\"; break;\n");
    source.push_str("    }\n");
    source.push_str("    fprintf(stderr, \"%s\\n\", message);\n");
    source.push_str("    abort();\n");
    source.push_str("}\n");
    source
}

#[cfg(test)]
mod tests {
    use super::{TrapCode, trap_runtime_c_source};

    #[test]
    fn trap_codes_round_trip_through_their_integers() {
        for code in [
            TrapCode::IntegerOverflow,
            TrapCode::DivisionByZero,
            TrapCode::IndexOutOfBounds,
            TrapCode::Unreachable,
        ] {
            assert_eq!(TrapCode::from_i32(code.as_i32()), Some(code));
        }
        assert_eq!(TrapCode::from_i32(99), None);
    }

    #[test]
    fn the_c_source_mirrors_every_trap_message_and_code() {
        // The C runtime and the Rust reference must agree message-for-message
        // and code-for-code, or a native trap would print differently from the
        // interpreter's. Assert each pair appears in the emitted C.
        let source = trap_runtime_c_source();
        for code in [
            TrapCode::IntegerOverflow,
            TrapCode::DivisionByZero,
            TrapCode::IndexOutOfBounds,
            TrapCode::Unreachable,
        ] {
            let arm = format!("case {}: message = \"{}\";", code.as_i32(), code.message());
            assert!(
                source.contains(&arm),
                "C runtime is missing the arm for {code:?}: expected `{arm}`"
            );
        }
        assert!(source.contains("void tuo_rt_trap(int code)"));
        assert!(source.contains("abort();"));
    }
}
