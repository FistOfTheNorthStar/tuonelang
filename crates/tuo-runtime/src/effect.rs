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

/// The name of the C-ABI symbol generated code calls for `std::rt::par_map`
/// (ADR-0007): `void tuo_rt_par_map(long long f, const long long *tasks,
/// long long n, long long workers, long long *out_hdr)` — apply the task
/// function (a tuonelang `fn(take Int) -> Int` code pointer, whose native
/// ABI is exactly C's `long long (*)(long long)`) to every task on
/// `workers` POSIX threads (round-robin: task `i` on thread `i % workers`;
/// `workers < 1` behaves as 1, and never more threads than tasks), join
/// them all, and write a fresh `Array[Int]` header of the results in task
/// order into `out_hdr`. Structured fork-join: nothing outlives the call.
pub const PAR_MAP_SYMBOL: &str = "tuo_rt_par_map";

/// The name of the C-ABI symbol generated code calls for
/// `std::rt::now_nanos` (ADR-0013): `long long tuo_rt_now_nanos(void)` — the
/// monotonic clock (`CLOCK_MONOTONIC`) in nanoseconds since an arbitrary
/// process-local epoch. Only differences are meaningful. On the (practically
/// unreachable) host failure it returns `0` rather than trapping.
pub const NOW_NANOS_SYMBOL: &str = "tuo_rt_now_nanos";

/// The name of the C-ABI symbol generated code calls for
/// `std::rt::arg_count` (ADR-0013): `long long tuo_rt_arg_count(void)` — the
/// number of process arguments including the program name. The runtime
/// captures `argc`/`argv` before `main` runs via the platform's initializer
/// mechanism (`crt_externs.h` on macOS, a constructor receiving `argc`/`argv`
/// on glibc).
pub const ARG_COUNT_SYMBOL: &str = "tuo_rt_arg_count";

/// The name of the C-ABI symbol generated code calls for `std::rt::arg_byte`
/// (ADR-0013): `long long tuo_rt_arg_byte(long long i, long long j)` — byte
/// `j` (`0..=255`) of process argument `i`, or [`ARG_MISSING`] when `i` is
/// out of range or `j` is past that argument's end.
pub const ARG_BYTE_SYMBOL: &str = "tuo_rt_arg_byte";

/// The name of the C-ABI symbol generated code calls for `std::rt::open`
/// (ADR-0013): `long long tuo_rt_open(const unsigned char *ptr, unsigned
/// long long len, long long mode)` — open the file whose path is the `len`
/// bytes at `ptr`; a file descriptor (`>= 0`) on success,
/// [`FILE_NOT_FOUND`] when the path does not exist, [`FILE_ERROR`] on any
/// other host error (an unknown `mode` or an over-long path included).
/// Modes: `0` read, `1` write (create + truncate), `2` append (create).
pub const OPEN_SYMBOL: &str = "tuo_rt_open";

/// The name of the C-ABI symbol generated code calls for `std::rt::close`
/// (ADR-0013): `long long tuo_rt_close(long long fd)` — `0` on success,
/// [`FILE_ERROR`] on host error.
pub const CLOSE_SYMBOL: &str = "tuo_rt_close";

/// The name of the C-ABI symbol generated code calls for
/// `std::rt::remove_file` (ADR-0013): `long long tuo_rt_remove_file(const
/// unsigned char *ptr, unsigned long long len)` — `0` on success,
/// [`FILE_NOT_FOUND`] when the path does not exist, [`FILE_ERROR`] on any
/// other host error.
pub const REMOVE_FILE_SYMBOL: &str = "tuo_rt_remove_file";

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

/// The value [`ARG_BYTE_SYMBOL`] returns for an out-of-range argument index
/// or a byte index past the argument's end (ADR-0013) — the same
/// "no more bytes" convention as [`READ_EOF`].
pub const ARG_MISSING: i64 = -1;

/// The value [`OPEN_SYMBOL`] and [`REMOVE_FILE_SYMBOL`] return when the
/// path does not exist (ADR-0013). Distinct from [`FILE_ERROR`] so a
/// program can classify a missing file without `errno`.
pub const FILE_NOT_FOUND: i64 = -2;

/// The value the ADR-0013 file symbols return on any other host error
/// (an unknown open mode and an over-long path included).
pub const FILE_ERROR: i64 = -1;

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
         }}\n\
         \n\
         /* ADR-0007: structured fork-join over POSIX threads. Task i runs on\n\
         \x20  thread i % workers (the round-robin partition the scheduling\n\
         \x20  model predicts); every thread writes only its own disjoint\n\
         \x20  result slots and is joined before the function returns. */\n\
         #include <pthread.h>\n\
         #include <stdint.h>\n\
         \n\
         extern void *tuo_rt_alloc(unsigned long long size, unsigned long long align);\n\
         \n\
         typedef long long (*tuo_rt_task_fn)(long long);\n\
         \n\
         typedef struct {{\n\
         \x20   tuo_rt_task_fn task;\n\
         \x20   const long long *tasks;\n\
         \x20   long long *results;\n\
         \x20   long long count;\n\
         \x20   long long workers;\n\
         \x20   long long worker;\n\
         }} tuo_rt_par_ctx;\n\
         \n\
         static void *tuo_rt_par_worker(void *arg) {{\n\
         \x20   tuo_rt_par_ctx *ctx = (tuo_rt_par_ctx *)arg;\n\
         \x20   for (long long i = ctx->worker; i < ctx->count; i += ctx->workers) {{\n\
         \x20       ctx->results[i] = ctx->task(ctx->tasks[i]);\n\
         \x20   }}\n\
         \x20   return 0;\n\
         }}\n\
         \n\
         void {PAR_MAP_SYMBOL}(long long f, const long long *tasks, long long n,\n\
         \x20                  long long workers, long long *out_hdr) {{\n\
         \x20   if (n <= 0) {{\n\
         \x20       out_hdr[0] = {sentinel};\n\
         \x20       out_hdr[1] = 0;\n\
         \x20       out_hdr[2] = 0;\n\
         \x20       return;\n\
         \x20   }}\n\
         \x20   if (workers < 1) workers = 1;\n\
         \x20   if (workers > n) workers = n;\n\
         \x20   long long *results =\n\
         \x20       (long long *)tuo_rt_alloc((unsigned long long)n * 8, 8);\n\
         \x20   tuo_rt_par_ctx ctx[64];\n\
         \x20   pthread_t threads[64];\n\
         \x20   char started[64];\n\
         \x20   if (workers > 64) workers = 64;\n\
         \x20   for (long long w = 0; w < workers; w++) {{\n\
         \x20       ctx[w].task = (tuo_rt_task_fn)f;\n\
         \x20       ctx[w].tasks = tasks;\n\
         \x20       ctx[w].results = results;\n\
         \x20       ctx[w].count = n;\n\
         \x20       ctx[w].workers = workers;\n\
         \x20       ctx[w].worker = w;\n\
         \x20   }}\n\
         \x20   for (long long w = 1; w < workers; w++) {{\n\
         \x20       started[w] = pthread_create(&threads[w], 0, tuo_rt_par_worker, &ctx[w]) == 0;\n\
         \x20       if (!started[w]) {{\n\
         \x20           /* Could not start a thread: run that partition inline\n\
         \x20              instead — the result is identical, only less\n\
         \x20              parallel. */\n\
         \x20           tuo_rt_par_worker(&ctx[w]);\n\
         \x20       }}\n\
         \x20   }}\n\
         \x20   tuo_rt_par_worker(&ctx[0]);\n\
         \x20   for (long long w = 1; w < workers; w++) {{\n\
         \x20       if (started[w]) pthread_join(threads[w], 0);\n\
         \x20   }}\n\
         \x20   out_hdr[0] = (long long)results;\n\
         \x20   out_hdr[1] = n;\n\
         \x20   out_hdr[2] = n;\n\
         }}\n\
         \n\
         /* ADR-0013: the OS effect boundary — clock, argv, and files. */\n\
         #include <fcntl.h>\n\
         #include <string.h>\n\
         #include <time.h>\n\
         \n\
         long long {NOW_NANOS_SYMBOL}(void) {{\n\
         \x20   struct timespec ts;\n\
         \x20   if (clock_gettime(CLOCK_MONOTONIC, &ts) != 0) return 0;\n\
         \x20   return (long long)ts.tv_sec * 1000000000LL + (long long)ts.tv_nsec;\n\
         }}\n\
         \n\
         /* The process arguments, captured before `main` runs: macOS exposes\n\
         \x20  them via crt_externs.h; glibc passes them to an ELF constructor. */\n\
         static int tuo_rt_argc = 0;\n\
         static char **tuo_rt_argv = 0;\n\
         \n\
         #if defined(__APPLE__)\n\
         #include <crt_externs.h>\n\
         __attribute__((constructor)) static void tuo_rt_capture_args(void) {{\n\
         \x20   tuo_rt_argc = *_NSGetArgc();\n\
         \x20   tuo_rt_argv = *_NSGetArgv();\n\
         }}\n\
         #else\n\
         __attribute__((constructor)) static void tuo_rt_capture_args(int argc,\n\
         \x20                                                         char **argv) {{\n\
         \x20   tuo_rt_argc = argc;\n\
         \x20   tuo_rt_argv = argv;\n\
         }}\n\
         #endif\n\
         \n\
         long long {ARG_COUNT_SYMBOL}(void) {{\n\
         \x20   return (long long)tuo_rt_argc;\n\
         }}\n\
         \n\
         long long {ARG_BYTE_SYMBOL}(long long i, long long j) {{\n\
         \x20   if (i < 0 || i >= (long long)tuo_rt_argc || j < 0) return {ARG_MISSING};\n\
         \x20   const char *arg = tuo_rt_argv[i];\n\
         \x20   unsigned long long len = strlen(arg);\n\
         \x20   if ((unsigned long long)j >= len) return {ARG_MISSING};\n\
         \x20   return (long long)(unsigned char)arg[j];\n\
         }}\n\
         \n\
         /* A `Str` path is not NUL-terminated; copy it into a bounded buffer.\n\
         \x20  An over-long path is a host error ({FILE_ERROR}), never a trap. */\n\
         static int tuo_rt_path_copy(char *dst, unsigned long long cap,\n\
         \x20                        const unsigned char *ptr, unsigned long long len) {{\n\
         \x20   if (len >= cap) return 0;\n\
         \x20   memcpy(dst, ptr, len);\n\
         \x20   dst[len] = 0;\n\
         \x20   return 1;\n\
         }}\n\
         \n\
         long long {OPEN_SYMBOL}(const unsigned char *ptr, unsigned long long len,\n\
         \x20                    long long mode) {{\n\
         \x20   char path[4096];\n\
         \x20   int flags;\n\
         \x20   if (!tuo_rt_path_copy(path, sizeof(path), ptr, len)) return {FILE_ERROR};\n\
         \x20   if (mode == 0) flags = O_RDONLY;\n\
         \x20   else if (mode == 1) flags = O_WRONLY | O_CREAT | O_TRUNC;\n\
         \x20   else if (mode == 2) flags = O_WRONLY | O_CREAT | O_APPEND;\n\
         \x20   else return {FILE_ERROR};\n\
         \x20   for (;;) {{\n\
         \x20       int fd = open(path, flags, 0644);\n\
         \x20       if (fd >= 0) return (long long)fd;\n\
         \x20       if (errno == EINTR) continue;\n\
         \x20       return errno == ENOENT ? {FILE_NOT_FOUND} : {FILE_ERROR};\n\
         \x20   }}\n\
         }}\n\
         \n\
         long long {CLOSE_SYMBOL}(long long fd) {{\n\
         \x20   return close((int)fd) == 0 ? 0 : {FILE_ERROR};\n\
         }}\n\
         \n\
         long long {REMOVE_FILE_SYMBOL}(const unsigned char *ptr,\n\
         \x20                           unsigned long long len) {{\n\
         \x20   char path[4096];\n\
         \x20   if (!tuo_rt_path_copy(path, sizeof(path), ptr, len)) return {FILE_ERROR};\n\
         \x20   if (unlink(path) == 0) return 0;\n\
         \x20   return errno == ENOENT ? {FILE_NOT_FOUND} : {FILE_ERROR};\n\
         }}\n",
        sentinel = crate::alloc::ZERO_SIZE_SENTINEL,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        ARG_BYTE_SYMBOL, ARG_COUNT_SYMBOL, ARG_MISSING, CLOSE_SYMBOL, EXIT_SYMBOL, FILE_ERROR,
        FILE_NOT_FOUND, NOW_NANOS_SYMBOL, OPEN_SYMBOL, READ_BYTE_SYMBOL, READ_EOF, READ_ERROR,
        REMOVE_FILE_SYMBOL, WRITE_ERROR, WRITE_SYMBOL, effect_runtime_c_source, exit_status_of,
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

    #[test]
    fn the_c_source_defines_the_os_boundary_symbols_and_matches_the_policy() {
        let source = effect_runtime_c_source();
        // The six ADR-0013 C-ABI signatures, exactly as `specification/abi.md`
        // fixes them.
        assert!(source.contains(&format!("long long {NOW_NANOS_SYMBOL}(void)")));
        assert!(source.contains(&format!("long long {ARG_COUNT_SYMBOL}(void)")));
        assert!(source.contains(&format!(
            "long long {ARG_BYTE_SYMBOL}(long long i, long long j)"
        )));
        assert!(source.contains(&format!(
            "long long {OPEN_SYMBOL}(const unsigned char *ptr, unsigned long long len"
        )));
        assert!(source.contains(&format!("long long {CLOSE_SYMBOL}(long long fd)")));
        assert!(source.contains(&format!(
            "long long {REMOVE_FILE_SYMBOL}(const unsigned char *ptr"
        )));
        // The monotonic clock, argv capture before `main`, and the error
        // vocabulary the policy consts fix.
        assert!(source.contains("clock_gettime(CLOCK_MONOTONIC, &ts)"));
        assert!(source.contains("__attribute__((constructor))"));
        assert!(source.contains(&format!(
            "if ((unsigned long long)j >= len) return {ARG_MISSING};"
        )));
        assert!(source.contains(&format!(
            "return errno == ENOENT ? {FILE_NOT_FOUND} : {FILE_ERROR};"
        )));
        // Not-found and generic errors stay distinguishable, and every code
        // sits outside the byte range so no real value collides with one.
        assert_ne!(FILE_NOT_FOUND, FILE_ERROR);
        for value in [ARG_MISSING, FILE_NOT_FOUND, FILE_ERROR] {
            assert!(value < 0, "{value} must sit outside the byte range");
        }
    }
}
