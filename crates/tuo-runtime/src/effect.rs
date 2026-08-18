//! The host-effect boundary (ADR-0006 Stage B).
//!
//! Every host effect a compiled tuonelang program performs — the `std::rt`
//! builtins, lowered to MIR `Statement::Effect` — passes through
//! three C-ABI runtime symbols, so the effect boundary is a single, inspectable
//! seam and no backend embeds a syscall directly. See
//! [`specification/abi.md`](../../../specification/abi.md).
//!
//! ```text
//! long long tuo_rt_write(long long fd, const unsigned char *ptr, unsigned long long len);
//! long long tuo_rt_read_byte(long long fd);
//! void      tuo_rt_exit(long long code);   // never returns
//! ```
//!
//! The **operative implementation is C** ([`effect_runtime_c_source`]) — the
//! build driver links it into every built binary alongside the trap shim, so a
//! generated executable needs no Rust runtime, and because this crate forbids
//! `unsafe`, a real `write(2)`/`read(2)` wrapper cannot live here anyway. What
//! *does* live here in Rust is the boundary's pure, testable **policy**: the
//! exit-status truncation ([`exit_status_of`]) and the result vocabulary the
//! C source must honor ([`READ_EOF`], [`READ_ERROR`], [`WRITE_ERROR`]). The
//! Rust policy and the C source are pinned to agree by this module's tests, so
//! the observable contract is fixed without invoking a C compiler.
//!
//! The reference interpreter **never** performs an effect (the spec-purity
//! gate `R0007` makes one unreachable under the spec runner); these symbols
//! are the *only* place an effect's meaning touches the host, and only in a
//! natively built program.

/// The name of the C-ABI symbol generated code calls for `std::rt::write`.
pub const WRITE_SYMBOL: &str = "tuo_rt_write";

/// The name of the C-ABI symbol generated code calls for `std::rt::read_byte`.
pub const READ_BYTE_SYMBOL: &str = "tuo_rt_read_byte";

/// The name of the C-ABI symbol generated code calls for `std::rt::exit`.
/// Unlike the other two it **never returns**.
pub const EXIT_SYMBOL: &str = "tuo_rt_exit";

/// The value [`WRITE_SYMBOL`] returns on a host write error (after retrying
/// `EINTR`). A successful write returns the total byte count instead.
pub const WRITE_ERROR: i64 = -1;

/// The value [`READ_BYTE_SYMBOL`] returns at end of input.
pub const READ_EOF: i64 = -1;

/// The value [`READ_BYTE_SYMBOL`] returns on a host read error (after
/// retrying `EINTR`). Distinct from [`READ_EOF`] so a program can tell the
/// two apart, exactly as `specification/abi.md` specifies ("another negative
/// value on host error").
pub const READ_ERROR: i64 = -2;

/// The process exit status `tuo_rt_exit(code)` terminates with: the low byte
/// of `code`, exactly the truncation a normal `main` return undergoes on the
/// supported hosts. This is the pure policy the C source implements
/// (`_exit(code & 0xff)`), factored out so it is testable without a process.
#[must_use]
pub const fn exit_status_of(code: i64) -> i32 {
    (code & 0xff) as i32
}

/// The C source of the runtime's effect boundary.
///
/// The build driver writes this to a `.c` file, compiles it, and links it into
/// the final executable so [`WRITE_SYMBOL`], [`READ_BYTE_SYMBOL`], and
/// [`EXIT_SYMBOL`] resolve. It is freestanding C over `<unistd.h>` and
/// `<errno.h>` — POSIX `write(2)`/`read(2)`/`_exit(2)`, which every supported
/// host provides — and drags no Rust runtime into the target binary.
///
/// Behavior, per [`specification/abi.md`](../../../specification/abi.md):
///
/// - `tuo_rt_write(fd, ptr, len)` writes the `len` bytes at `ptr` to file
///   descriptor `fd`, looping over partial writes and retrying `EINTR`;
///   it returns the total bytes written, or [`WRITE_ERROR`] on any other
///   host error. It never traps.
/// - `tuo_rt_read_byte(fd)` reads one byte from `fd`, retrying `EINTR`;
///   it returns the byte (`0..=255`), [`READ_EOF`] at end of input, or
///   [`READ_ERROR`] on any other host error. It never traps.
/// - `tuo_rt_exit(code)` terminates the process with `code & 0xff` as the
///   exit status ([`exit_status_of`]) via `_exit` — a **noreturn** function
///   (declared `_Noreturn`), a *normal* exit path (no stderr message, no
///   cleanup; none is pending by construction), not a trap.
#[must_use]
pub fn effect_runtime_c_source() -> String {
    format!(
        "#include <errno.h>\n\
         #include <unistd.h>\n\
         \n\
         long long {WRITE_SYMBOL}(long long fd, const unsigned char *ptr,\n\
         \x20                     unsigned long long len) {{\n\
         \x20   unsigned long long written = 0;\n\
         \x20   while (written < len) {{\n\
         \x20       ssize_t n = write((int)fd, ptr + written, (size_t)(len - written));\n\
         \x20       if (n < 0) {{\n\
         \x20           if (errno == EINTR) continue;\n\
         \x20           return {WRITE_ERROR};\n\
         \x20       }}\n\
         \x20       written += (unsigned long long)n;\n\
         \x20   }}\n\
         \x20   return (long long)written;\n\
         }}\n\
         \n\
         long long {READ_BYTE_SYMBOL}(long long fd) {{\n\
         \x20   unsigned char byte;\n\
         \x20   for (;;) {{\n\
         \x20       ssize_t n = read((int)fd, &byte, 1);\n\
         \x20       if (n == 1) return (long long)byte;\n\
         \x20       if (n == 0) return {READ_EOF};\n\
         \x20       if (errno == EINTR) continue;\n\
         \x20       return {READ_ERROR};\n\
         \x20   }}\n\
         }}\n\
         \n\
         _Noreturn void {EXIT_SYMBOL}(long long code) {{\n\
         \x20   _exit((int)(code & 0xff));\n\
         }}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::{
        EXIT_SYMBOL, READ_BYTE_SYMBOL, READ_EOF, READ_ERROR, WRITE_ERROR, WRITE_SYMBOL,
        effect_runtime_c_source, exit_status_of,
    };

    #[test]
    fn exit_status_is_the_low_byte_of_the_code() {
        assert_eq!(exit_status_of(0), 0);
        assert_eq!(exit_status_of(7), 7);
        assert_eq!(exit_status_of(255), 255);
        assert_eq!(exit_status_of(256), 0);
        assert_eq!(exit_status_of(257), 1);
        // Negative codes truncate to their low byte too (two's complement).
        assert_eq!(exit_status_of(-1), 255);
    }

    #[test]
    fn the_error_vocabulary_is_distinct_and_negative() {
        // EOF and error must be distinguishable, and both outside 0..=255 so
        // no real byte value collides with them.
        assert_ne!(READ_EOF, READ_ERROR);
        for value in [READ_EOF, READ_ERROR, WRITE_ERROR] {
            assert!(value < 0, "{value} must sit outside the byte range");
        }
    }

    #[test]
    fn the_c_source_defines_all_three_effect_symbols_and_matches_the_policy() {
        let source = effect_runtime_c_source();
        // The three C-ABI signatures, exactly as `specification/abi.md` fixes
        // them (i64 fd/code, {ptr, len} for the Str's bytes).
        assert!(source.contains(&format!(
            "long long {WRITE_SYMBOL}(long long fd, const unsigned char *ptr"
        )));
        assert!(source.contains(&format!("long long {READ_BYTE_SYMBOL}(long long fd)")));
        assert!(source.contains(&format!("_Noreturn void {EXIT_SYMBOL}(long long code)")));
        // The write loop retries EINTR and reports the policy's error values.
        assert!(source.contains("if (errno == EINTR) continue;"));
        assert!(source.contains(&format!("return {WRITE_ERROR};")));
        assert!(source.contains(&format!("if (n == 0) return {READ_EOF};")));
        assert!(source.contains(&format!("return {READ_ERROR};")));
        // Exit truncates exactly as `exit_status_of` does, via `_exit`.
        assert!(source.contains("_exit((int)(code & 0xff));"));
        assert_eq!(exit_status_of(0x1_07), 7);
    }
}
