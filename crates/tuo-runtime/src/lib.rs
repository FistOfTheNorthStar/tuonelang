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
//!   allocator is one seam (the LLVM release backend inlines a copy of its
//!   body built from the same constants).
//! - **The effect boundary** ([`effect`], ADR-0006 Stage B) — the three C-ABI
//!   symbols ([`effect::WRITE_SYMBOL`], [`effect::READ_BYTE_SYMBOL`],
//!   [`effect::EXIT_SYMBOL`]) through which every host effect (the `std::rt`
//!   builtins, MIR `Statement::Effect`) flows, so the effect boundary is one
//!   inspectable seam.
//!
//! # How it reaches a generated binary
//!
//! The runtime's C-ABI symbols must be present in the final executable, but the
//! runtime is a Rust library, not something a generated object links to
//! directly. So this crate exposes its trap, its allocator, and its effect
//! boundary as portable **C source** ([`trap_runtime_c_source`],
//! [`alloc::alloc_runtime_c_source`], [`effect::effect_runtime_c_source`]) that
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
pub mod effect;
pub mod map;

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
    /// A byte-valued argument was outside `0..=255`
    /// (`std::string::push_byte`, ADR-0009). Appended together with the
    /// interpreter's `TrapKind::InvalidByte` — the taxonomy is append-only.
    InvalidByte = 4,
    /// A shift amount was negative or >= the operand's width in bits
    /// (`BinOp::Shl`/`Shr`, ADR-0019). Appended to the taxonomy, which is
    /// append-only.
    InvalidShift = 5,
    /// A thread ran out of native stack — deep recursion, including the
    /// out-of-line drop glue of a long `Box` chain (ADR-0028). Raised by the
    /// runtime's guard-page handler rather than by generated code, so it
    /// covers every function, glue included. The interpreter's analogue is its
    /// recursion limit (`TrapKind::RecursionLimit`): a *resource* limit whose
    /// depth differs between the engines, so it is never compared
    /// differentially. Appended to the taxonomy, which is append-only.
    StackOverflow = 6,
}

impl TrapCode {
    /// Every trap code, in discriminant order. The C generator and its
    /// mirroring test both iterate this, so a newly appended code cannot be
    /// left out of the emitted runtime (which would make a native trap print
    /// `unknown` while the interpreter named it).
    pub const ALL: &'static [Self] = &[
        Self::IntegerOverflow,
        Self::DivisionByZero,
        Self::IndexOutOfBounds,
        Self::Unreachable,
        Self::InvalidByte,
        Self::InvalidShift,
        Self::StackOverflow,
    ];

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
            4 => Some(Self::InvalidByte),
            5 => Some(Self::InvalidShift),
            6 => Some(Self::StackOverflow),
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
            Self::InvalidByte => "tuo: trap: byte value out of range",
            Self::InvalidShift => "tuo: trap: shift amount out of range",
            Self::StackOverflow => "tuo: trap: stack overflow",
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
/// It is plain POSIX C — no Rust runtime is dragged into the target binary.
///
/// It also owns the **stack-overflow trap** ([`TrapCode::StackOverflow`]). A
/// constructor installs a `SIGSEGV`/`SIGBUS` handler that runs on an
/// alternate signal stack, and records the main thread's stack bounds; each
/// `par_map` worker records its own through [`THREAD_GUARD_SYMBOL`]. A fault
/// whose address lies in the guard region just below the faulting thread's
/// stack is reported as the trap (message, then `abort()`, the same status as
/// every trap). Any other fault restores the default action and re-faults, so
/// a genuine runtime bug is never disguised as a stack overflow.
#[must_use]
pub fn trap_runtime_c_source() -> String {
    // Keep the messages here byte-identical to `TrapCode::message`, and the
    // codes identical to `TrapCode`'s discriminants; the native test below
    // guards the Rust side, and this string is its C mirror.
    let mut source = String::new();
    source.push_str("#define _GNU_SOURCE\n");
    source.push_str("#include <stdio.h>\n");
    source.push_str("#include <stdlib.h>\n\n");
    source.push_str("void tuo_rt_trap(int code) {\n");
    source.push_str("    const char *message;\n");
    source.push_str("    switch (code) {\n");
    for code in TrapCode::ALL {
        source.push_str(&format!(
            "        case {}: message = \"{}\"; break;\n",
            code.as_i32(),
            code.message()
        ));
    }
    source.push_str("        default: message = \"tuo: trap: unknown\"; break;\n");
    source.push_str("    }\n");
    source.push_str("    fprintf(stderr, \"%s\\n\", message);\n");
    source.push_str("    abort();\n");
    source.push_str("}\n");
    source.push_str(&stack_guard_c_source());
    source
}

/// The C symbol a newly started runtime thread calls to arm the stack-overflow
/// trap for itself (an alternate signal stack, and its own stack bounds); it
/// returns a token the thread passes to [`THREAD_UNGUARD_SYMBOL`] on exit.
pub const THREAD_GUARD_SYMBOL: &str = "tuo_rt_thread_guard";

/// The C symbol a runtime thread calls before exiting to release what
/// [`THREAD_GUARD_SYMBOL`] armed.
pub const THREAD_UNGUARD_SYMBOL: &str = "tuo_rt_thread_unguard";

/// The guard-page handler behind [`TrapCode::StackOverflow`] (see
/// [`trap_runtime_c_source`]).
fn stack_guard_c_source() -> String {
    let code = TrapCode::StackOverflow.as_i32();
    let message = TrapCode::StackOverflow.message();
    format!(
        "\n\
         /* The stack-overflow trap (TrapCode {code}). */\n\
         #include <pthread.h>\n\
         #include <signal.h>\n\
         #include <stdint.h>\n\
         #include <string.h>\n\
         #include <unistd.h>\n\
         \n\
         #define TUO_RT_ALTSTACK_SIZE 65536\n\
         /* How far below the stack's lowest address a fault still counts as\n\
            an overflow: a frame larger than the guard page can skip past it. */\n\
         #define TUO_RT_GUARD_REACH (1u << 20)\n\
         \n\
         static _Thread_local uintptr_t tuo_rt_stack_low;\n\
         \n\
         static void tuo_rt_record_stack(void) {{\n\
         #if defined(__APPLE__)\n\
         \x20   pthread_t self = pthread_self();\n\
         \x20   tuo_rt_stack_low = (uintptr_t)pthread_get_stackaddr_np(self)\n\
         \x20       - pthread_get_stacksize_np(self);\n\
         #else\n\
         \x20   pthread_attr_t attr;\n\
         \x20   void *addr;\n\
         \x20   size_t size;\n\
         \x20   if (pthread_getattr_np(pthread_self(), &attr) == 0) {{\n\
         \x20       if (pthread_attr_getstack(&attr, &addr, &size) == 0)\n\
         \x20           tuo_rt_stack_low = (uintptr_t)addr;\n\
         \x20       pthread_attr_destroy(&attr);\n\
         \x20   }}\n\
         #endif\n\
         }}\n\
         \n\
         static void tuo_rt_on_fault(int sig, siginfo_t *info, void *context) {{\n\
         \x20   (void)context;\n\
         \x20   uintptr_t fault = (uintptr_t)info->si_addr;\n\
         \x20   uintptr_t low = tuo_rt_stack_low;\n\
         \x20   if (low != 0 && fault < low + 4096 && fault + TUO_RT_GUARD_REACH >= low) {{\n\
         \x20       static const char text[] = \"{message}\\n\";\n\
         \x20       ssize_t ignored = write(2, text, sizeof text - 1);\n\
         \x20       (void)ignored;\n\
         \x20       abort();\n\
         \x20   }}\n\
         \x20   /* Not an overflow: crash exactly as without this handler. */\n\
         \x20   signal(sig, SIG_DFL);\n\
         }}\n\
         \n\
         /* Arm the calling thread: its alternate signal stack, and its own\n\
            stack bounds. Returns 0 if the alternate stack was refused. */\n\
         static int tuo_rt_arm(void *altstack) {{\n\
         \x20   stack_t alt;\n\
         \x20   alt.ss_sp = altstack;\n\
         \x20   alt.ss_size = TUO_RT_ALTSTACK_SIZE;\n\
         \x20   alt.ss_flags = 0;\n\
         \x20   int armed = altstack != 0 && sigaltstack(&alt, 0) == 0;\n\
         \x20   tuo_rt_record_stack();\n\
         \x20   return armed;\n\
         }}\n\
         \n\
         void *{THREAD_GUARD_SYMBOL}(void) {{\n\
         \x20   void *altstack = malloc(TUO_RT_ALTSTACK_SIZE);\n\
         \x20   if (!tuo_rt_arm(altstack)) {{\n\
         \x20       free(altstack);\n\
         \x20       return 0;\n\
         \x20   }}\n\
         \x20   return altstack;\n\
         }}\n\
         \n\
         void {THREAD_UNGUARD_SYMBOL}(void *token) {{\n\
         \x20   if (token == 0) return;\n\
         \x20   stack_t off;\n\
         \x20   memset(&off, 0, sizeof off);\n\
         \x20   off.ss_flags = SS_DISABLE;\n\
         \x20   sigaltstack(&off, 0);\n\
         \x20   free(token);\n\
         }}\n\
         \n\
         __attribute__((constructor)) static void tuo_rt_install_stack_guard(void) {{\n\
         \x20   struct sigaction action;\n\
         \x20   memset(&action, 0, sizeof action);\n\
         \x20   action.sa_sigaction = tuo_rt_on_fault;\n\
         \x20   action.sa_flags = SA_SIGINFO | SA_ONSTACK;\n\
         \x20   sigemptyset(&action.sa_mask);\n\
         \x20   sigaction(SIGSEGV, &action, 0);\n\
         \x20   sigaction(SIGBUS, &action, 0);\n\
         \x20   /* The main thread's alternate stack lives as long as the process,\n\
         \x20      so it is static rather than an allocation never freed. */\n\
         \x20   static char main_altstack[TUO_RT_ALTSTACK_SIZE];\n\
         \x20   (void)tuo_rt_arm(main_altstack);\n\
         }}\n"
    )
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
            TrapCode::InvalidByte,
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
        for code in TrapCode::ALL {
            let code = *code;
            let arm = format!("case {}: message = \"{}\";", code.as_i32(), code.message());
            assert!(
                source.contains(&arm),
                "C runtime is missing the arm for {code:?}: expected `{arm}`"
            );
        }
        assert!(source.contains("void tuo_rt_trap(int code)"));
        assert!(source.contains("abort();"));
    }

    #[test]
    fn all_lists_every_trap_code_in_discriminant_order() {
        // `ALL` is what the C generator iterates, so a code missing from it
        // would silently print `unknown` natively while the interpreter named
        // the trap. Walking the discriminants via `from_i32` proves the list
        // is complete without a second hand-maintained copy.
        for (index, code) in TrapCode::ALL.iter().enumerate() {
            let expected = i32::try_from(index).expect("the taxonomy is small");
            assert_eq!(
                code.as_i32(),
                expected,
                "TrapCode::ALL must be in discriminant order with no gaps"
            );
        }
        let next = i32::try_from(TrapCode::ALL.len()).expect("the taxonomy is small");
        assert!(
            TrapCode::from_i32(next).is_none(),
            "TrapCode::{next} exists but is missing from TrapCode::ALL"
        );
    }
}
