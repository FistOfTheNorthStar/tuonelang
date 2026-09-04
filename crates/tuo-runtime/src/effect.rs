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
/// `std::rt::random_byte` (ADR-0019 Stage B): `long long
/// tuo_rt_random_byte(void)` — one cryptographically-secure random byte in
/// `0..=255`, or [`RANDOM_ERROR`] when the platform CSPRNG is unavailable.
///
/// A **byte at a time** rather than a buffer, because the effect seam moves
/// scalars: the same shape as `read_byte`, needing no new ABI concept. A
/// caller wanting sixteen bytes calls it sixteen times, which is what
/// `std::crypto::nonce` does.
///
/// The source is the platform CSPRNG (`getentropy`), never a seeded PRNG:
/// a nonce or key generated from a predictable stream silently voids the
/// security property it exists to provide, so a failure is reported as an
/// error rather than papered over with a fallback.
pub const RANDOM_BYTE_SYMBOL: &str = "tuo_rt_random_byte";

/// The value [`RANDOM_BYTE_SYMBOL`] returns when the platform CSPRNG cannot
/// be read. Negative, so it can never be mistaken for a byte value, and
/// distinct from the byte range by construction.
pub const RANDOM_ERROR: i64 = -1;

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

/// The name of the C-ABI symbol generated code calls for `std::rt::listen`
/// (ADR-0014): `long long tuo_rt_listen(long long port)` — an IPv4 TCP
/// socket bound to `127.0.0.1:port` (`0` = ephemeral) and listening
/// (backlog 16, `SO_REUSEADDR`); the listening descriptor (`>= 0`) or
/// [`NET_ERROR`] on host error.
pub const LISTEN_SYMBOL: &str = "tuo_rt_listen";

/// The name of the C-ABI symbol generated code calls for
/// `std::rt::bound_port` (ADR-0014): `long long tuo_rt_bound_port(long long
/// fd)` — the local port `fd` is bound to (`getsockname`), or [`NET_ERROR`]
/// on host error.
pub const BOUND_PORT_SYMBOL: &str = "tuo_rt_bound_port";

/// The name of the C-ABI symbol generated code calls for `std::rt::accept`
/// (ADR-0014): `long long tuo_rt_accept(long long fd)` — accept one pending
/// connection (`EINTR` retried); the connected descriptor (`>= 0`) or
/// [`NET_ERROR`] on host error.
pub const ACCEPT_SYMBOL: &str = "tuo_rt_accept";

/// The name of the C-ABI symbol generated code calls for `std::rt::connect`
/// (ADR-0014): `long long tuo_rt_connect(const unsigned char *ptr, unsigned
/// long long len, long long port)` — open a TCP connection to the numeric
/// IPv4 address in the `{ptr, len}` bytes at `port`; the connected
/// descriptor (`>= 0`) or [`NET_ERROR`] on host error.
pub const CONNECT_SYMBOL: &str = "tuo_rt_connect";

/// The name of the C-ABI symbol generated code calls for
/// `std::rt::accept_timeout` (ADR-0017): `long long
/// tuo_rt_accept_timeout(long long fd, long long ms)` — accept one pending
/// connection, waiting at most `ms` milliseconds; the connected descriptor
/// (`>= 0`), [`NET_TIMEOUT`], or [`NET_ERROR`].
pub const ACCEPT_TIMEOUT_SYMBOL: &str = "tuo_rt_accept_timeout";

/// The name of the C-ABI symbol generated code calls for `std::rt::listen6`
/// (ADR-0017): `long long tuo_rt_listen6(long long port)` — an IPv6 TCP
/// socket bound to `[::1]:port` and listening (`IPV6_V6ONLY`, loopback
/// only); the listening descriptor (`>= 0`) or [`NET_ERROR`].
pub const LISTEN6_SYMBOL: &str = "tuo_rt_listen6";

/// The name of the C-ABI symbol generated code calls for
/// `std::rt::peer_family` (ADR-0017): `long long tuo_rt_peer_family(long
/// long fd)` — the address family of a descriptor as [`FAMILY_IPV4`] or
/// [`FAMILY_IPV6`], or [`NET_ERROR`] on host error.
pub const PEER_FAMILY_SYMBOL: &str = "tuo_rt_peer_family";

/// The name of the C-ABI symbol generated code calls for `std::rt::udp_bind`
/// (ADR-0017): `long long tuo_rt_udp_bind(long long port)` — an IPv4 UDP
/// socket bound to `127.0.0.1:port`; the descriptor (`>= 0`) or
/// [`NET_ERROR`].
pub const UDP_BIND_SYMBOL: &str = "tuo_rt_udp_bind";

/// The name of the C-ABI symbol generated code calls for `std::rt::udp_send`
/// (ADR-0017): `long long tuo_rt_udp_send(long long fd, const unsigned char
/// *hptr, unsigned long long hlen, long long port, const unsigned char
/// *bptr, unsigned long long blen)` — send one datagram; the byte count
/// (`>= 0`) or [`NET_ERROR`].
pub const UDP_SEND_SYMBOL: &str = "tuo_rt_udp_send";

/// The name of the C-ABI symbol generated code calls for `std::rt::udp_recv`
/// (ADR-0017): `long long tuo_rt_udp_recv(long long fd, long long ms)` —
/// receive one datagram into the descriptor's staging buffer; its length
/// (`>= 0`), [`NET_TIMEOUT`], or [`NET_ERROR`]. The bytes are then drained
/// with [`READ_BYTE_SYMBOL`].
pub const UDP_RECV_SYMBOL: &str = "tuo_rt_udp_recv";

/// The name of the C-ABI symbol generated code calls for
/// `std::rt::udp_byte_at` (ADR-0017): `long long tuo_rt_udp_byte_at(long
/// long fd, long long i)` — byte `i` of the datagram most recently staged on
/// `fd` (`0..=255`), or [`NET_ERROR`] when `i` is out of range or nothing is
/// staged.
pub const UDP_BYTE_AT_SYMBOL: &str = "tuo_rt_udp_byte_at";

/// The name of the C-ABI symbol generated code calls for
/// `std::rt::udp_peer_port` (ADR-0017): `long long
/// tuo_rt_udp_peer_port(long long fd)` — the source port of the most recent
/// [`UDP_RECV_SYMBOL`] on `fd`, or [`NET_ERROR`] if there was none.
pub const UDP_PEER_PORT_SYMBOL: &str = "tuo_rt_udp_peer_port";

/// The bound on one staged datagram (ADR-0017). A larger datagram is
/// truncated to this many bytes while [`UDP_RECV_SYMBOL`] still reports the
/// true length, matching `recvfrom`'s own semantics.
pub const UDP_DATAGRAM_CAP: i64 = 2048;

/// The name of the C-ABI symbol generated code calls for
/// `std::rt::connect_timeout` (ADR-0017): `long long
/// tuo_rt_connect_timeout(const unsigned char *ptr, unsigned long long len,
/// long long port, long long ms)` — as [`CONNECT_SYMBOL`], abandoning the
/// handshake after `ms` milliseconds; [`NET_TIMEOUT`] on timeout.
pub const CONNECT_TIMEOUT_SYMBOL: &str = "tuo_rt_connect_timeout";

/// The name of the C-ABI symbol generated code calls for
/// `std::rt::read_byte_timeout` (ADR-0017): `long long
/// tuo_rt_read_byte_timeout(long long fd, long long ms)` — as
/// [`READ_BYTE_SYMBOL`], waiting at most `ms` milliseconds for readability;
/// the byte, [`READ_EOF`], [`NET_TIMEOUT`], or [`READ_ERROR`].
pub const READ_BYTE_TIMEOUT_SYMBOL: &str = "tuo_rt_read_byte_timeout";

/// The name of the C-ABI symbol generated code calls for
/// `std::rt::chan_new` (ADR-0015): `long long tuo_rt_chan_new(void)` — a
/// process-lived channel handle (`>= 0`) or [`SYNC_ERROR`] on registry
/// exhaustion.
pub const CHAN_NEW_SYMBOL: &str = "tuo_rt_chan_new";

/// The name of the C-ABI symbol generated code calls for
/// `std::rt::chan_send` (ADR-0015): `long long tuo_rt_chan_send(long long
/// ch, long long v)` — `0` on success, [`SYNC_ERROR`] on an invalid handle,
/// a closed channel, or a negative `v`.
pub const CHAN_SEND_SYMBOL: &str = "tuo_rt_chan_send";

/// The name of the C-ABI symbol generated code calls for
/// `std::rt::chan_recv` (ADR-0015): `long long tuo_rt_chan_recv(long long
/// ch)` — blocks; the oldest value, or [`SYNC_ERROR`] once closed and
/// drained (or on an invalid handle).
pub const CHAN_RECV_SYMBOL: &str = "tuo_rt_chan_recv";

/// The name of the C-ABI symbol generated code calls for
/// `std::rt::chan_close` (ADR-0015): `long long tuo_rt_chan_close(long long
/// ch)` — `0` on success (idempotent), [`SYNC_ERROR`] on an invalid handle.
pub const CHAN_CLOSE_SYMBOL: &str = "tuo_rt_chan_close";

/// The name of the C-ABI symbol generated code calls for
/// `std::rt::mutex_new` (ADR-0015): `long long tuo_rt_mutex_new(void)` — a
/// process-lived mutex handle (`>= 0`) or [`SYNC_ERROR`] on registry
/// exhaustion.
pub const MUTEX_NEW_SYMBOL: &str = "tuo_rt_mutex_new";

/// The name of the C-ABI symbol generated code calls for
/// `std::rt::mutex_lock` (ADR-0015): `long long tuo_rt_mutex_lock(long long
/// m)` — blocks; `0` on success, [`SYNC_ERROR`] on an invalid handle or a
/// host error (an error-checked relock included).
pub const MUTEX_LOCK_SYMBOL: &str = "tuo_rt_mutex_lock";

/// The name of the C-ABI symbol generated code calls for
/// `std::rt::mutex_unlock` (ADR-0015): `long long tuo_rt_mutex_unlock(long
/// long m)` — `0` on success, [`SYNC_ERROR`] on an invalid handle or when
/// the calling thread does not hold it.
pub const MUTEX_UNLOCK_SYMBOL: &str = "tuo_rt_mutex_unlock";

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

/// The value the ADR-0014 socket symbols return on any host error (an
/// out-of-range port, a non-numeric or over-long host, and every refused
/// system call included). There is deliberately no finer taxonomy: a socket
/// failure is environmental, and v0 programs branch only on "descriptor or
/// not".
pub const NET_ERROR: i64 = -1;

/// The value the ADR-0017 bounded-wait symbols return when their deadline
/// passed with nothing to report.
///
/// This is deliberately **distinct from every other sentinel the seam
/// already uses**: a timeout is not a failure, and a program must be able to
/// tell "the peer is slow" from "the peer is gone". `-1` is taken by
/// [`NET_ERROR`] and [`READ_EOF`], and `-2` is taken by [`READ_ERROR`] — so
/// the timeout is `-3`, the first value that stays unambiguous on
/// `read_byte_timeout`, where all four outcomes (byte, EOF, host error,
/// timeout) can occur on one call. A negative `ms` is a host error, never an
/// unbounded wait — a bounded primitive must not silently become a blocking
/// one.
pub const NET_TIMEOUT: i64 = -3;

/// The value [`PEER_FAMILY_SYMBOL`] returns for an IPv4 descriptor
/// (ADR-0017). Spelled as the familiar version number rather than the host's
/// `AF_INET`, whose numeric value is not portable.
pub const FAMILY_IPV4: i64 = 4;

/// The value [`PEER_FAMILY_SYMBOL`] returns for an IPv6 descriptor
/// (ADR-0017). See [`FAMILY_IPV4`].
pub const FAMILY_IPV6: i64 = 6;

/// The value the ADR-0015 channel and mutex symbols return on any error —
/// an invalid handle, an exhausted registry, a send of a negative value or
/// to a closed channel, an error-checked relock, an unlock by a non-holder
/// — and the value `chan_recv` returns once the channel is closed and
/// drained. Channel payloads are non-negative by contract (`chan_send`
/// refuses a negative `v`), so this closed/error signal is unambiguous.
pub const SYNC_ERROR: i64 = -1;

/// The bound on how many channels (and, separately, mutexes) one process
/// may create (ADR-0015). Handles are process-lived — there is deliberately
/// no free — so the registries are fixed arrays; creation past the bound is
/// a [`SYNC_ERROR`], never a trap.
pub const SYNC_REGISTRY_CAP: i64 = 256;

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
         /* `getentropy` is declared in <sys/random.h> on macOS, the BSDs, and\n\
         \x20  glibc >= 2.25; older glibc has no declaration at all. Reading\n\
         \x20  /dev/urandom is the portable fallback that needs no header and\n\
         \x20  no version test, and it is the same kernel CSPRNG. */\n\
         #if defined(__APPLE__) || defined(__FreeBSD__) || defined(__OpenBSD__)\n\
         #include <sys/random.h>\n\
         #define TUO_RT_HAVE_GETENTROPY 1\n\
         #elif defined(__GLIBC__) && (__GLIBC__ > 2 || (__GLIBC__ == 2 && __GLIBC_MINOR__ >= 25))\n\
         #include <sys/random.h>\n\
         #define TUO_RT_HAVE_GETENTROPY 1\n\
         #endif\n\
         \n\
         /* ADR-0019 Stage B: one byte from the platform CSPRNG. This is\n\
         \x20  deliberately NOT a seeded PRNG — a nonce drawn from a\n\
         \x20  predictable stream voids the property it exists to provide, so\n\
         \x20  a failure is reported rather than substituted. */\n\
         long long {RANDOM_BYTE_SYMBOL}(void) {{\n\
         \x20   unsigned char byte;\n\
         #if defined(TUO_RT_HAVE_GETENTROPY)\n\
         \x20   if (getentropy(&byte, 1) != 0) return {RANDOM_ERROR};\n\
         \x20   return (long long)byte;\n\
         #else\n\
         \x20   /* The same kernel source, read directly. */\n\
         \x20   int fd = open(\"/dev/urandom\", O_RDONLY);\n\
         \x20   if (fd < 0) return {RANDOM_ERROR};\n\
         \x20   ssize_t n;\n\
         \x20   do {{ n = read(fd, &byte, 1); }} while (n < 0 && errno == EINTR);\n\
         \x20   close(fd);\n\
         \x20   if (n != 1) return {RANDOM_ERROR};\n\
         \x20   return (long long)byte;\n\
         #endif\n\
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
         }}\n\
         \n\
         /* ADR-0014: socket effects — descriptor producers on the same seam\n\
         \x20  (`tuo_rt_write`/`tuo_rt_read_byte`/`tuo_rt_close` move and\n\
         \x20  release the bytes). IPv4 TCP, loopback listen, numeric hosts. */\n\
         #include <sys/socket.h>\n\
         #include <netinet/in.h>\n\
         #include <arpa/inet.h>\n\
         #include <poll.h>\n\
         \n\
         /* ADR-0017: parse a numeric host into either family. Tries v4 then\n\
         \x20  v6, so every string that parsed before parses identically and\n\
         \x20  `::1` now works too. Returns the socket family, or -1. */\n\
         static int tuo_rt_addr_parse(const char *host, long long port,\n\
         \x20                        struct sockaddr_storage *out,\n\
         \x20                        socklen_t *outlen) {{\n\
         \x20   struct sockaddr_in *v4 = (struct sockaddr_in *)out;\n\
         \x20   struct sockaddr_in6 *v6 = (struct sockaddr_in6 *)out;\n\
         \x20   memset(out, 0, sizeof(*out));\n\
         \x20   if (inet_pton(AF_INET, host, &v4->sin_addr) == 1) {{\n\
         \x20       v4->sin_family = AF_INET;\n\
         \x20       v4->sin_port = htons((unsigned short)port);\n\
         \x20       *outlen = sizeof(*v4);\n\
         \x20       return AF_INET;\n\
         \x20   }}\n\
         \x20   memset(out, 0, sizeof(*out));\n\
         \x20   if (inet_pton(AF_INET6, host, &v6->sin6_addr) == 1) {{\n\
         \x20       v6->sin6_family = AF_INET6;\n\
         \x20       v6->sin6_port = htons((unsigned short)port);\n\
         \x20       *outlen = sizeof(*v6);\n\
         \x20       return AF_INET6;\n\
         \x20   }}\n\
         \x20   return -1;\n\
         }}\n\
         \n\
         long long {LISTEN_SYMBOL}(long long port) {{\n\
         \x20   struct sockaddr_in addr;\n\
         \x20   int one = 1;\n\
         \x20   int fd;\n\
         \x20   if (port < 0 || port > 65535) return {NET_ERROR};\n\
         \x20   fd = socket(AF_INET, SOCK_STREAM, 0);\n\
         \x20   if (fd < 0) return {NET_ERROR};\n\
         \x20   setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &one, sizeof(one));\n\
         \x20   memset(&addr, 0, sizeof(addr));\n\
         \x20   addr.sin_family = AF_INET;\n\
         \x20   addr.sin_port = htons((unsigned short)port);\n\
         \x20   addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);\n\
         \x20   if (bind(fd, (struct sockaddr *)&addr, sizeof(addr)) != 0 ||\n\
         \x20       listen(fd, 16) != 0) {{\n\
         \x20       close(fd);\n\
         \x20       return {NET_ERROR};\n\
         \x20   }}\n\
         \x20   return (long long)fd;\n\
         }}\n\
         \n\
         /* ADR-0017 widened this to read the port from either family via\n\
         \x20  sockaddr_storage + ss_family. On the targeted POSIX layouts\n\
         \x20  sin_port and sin6_port share offset 2, so the v4-only spelling\n\
         \x20  read the right bytes by coincidence -- but it also passed a\n\
         \x20  too-small sizeof(sockaddr_in) for a v6 address. This form is\n\
         \x20  correct by construction rather than by layout accident. */\n\
         long long {BOUND_PORT_SYMBOL}(long long fd) {{\n\
         \x20   struct sockaddr_storage addr;\n\
         \x20   socklen_t alen = sizeof(addr);\n\
         \x20   if (getsockname((int)fd, (struct sockaddr *)&addr, &alen) != 0)\n\
         \x20       return {NET_ERROR};\n\
         \x20   if (addr.ss_family == AF_INET)\n\
         \x20       return (long long)ntohs(((struct sockaddr_in *)&addr)->sin_port);\n\
         \x20   if (addr.ss_family == AF_INET6)\n\
         \x20       return (long long)ntohs(((struct sockaddr_in6 *)&addr)->sin6_port);\n\
         \x20   return {NET_ERROR};\n\
         }}\n\
         \n\
         long long {LISTEN6_SYMBOL}(long long port) {{\n\
         \x20   struct sockaddr_in6 addr;\n\
         \x20   int one = 1;\n\
         \x20   int fd;\n\
         \x20   if (port < 0 || port > 65535) return {NET_ERROR};\n\
         \x20   fd = socket(AF_INET6, SOCK_STREAM, 0);\n\
         \x20   if (fd < 0) return {NET_ERROR};\n\
         \x20   setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &one, sizeof(one));\n\
         \x20   /* v6-only: a dual-stack socket would also accept mapped v4,\n\
         \x20      making `peer_family` ambiguous on an accepted connection. */\n\
         \x20   setsockopt(fd, IPPROTO_IPV6, IPV6_V6ONLY, &one, sizeof(one));\n\
         \x20   memset(&addr, 0, sizeof(addr));\n\
         \x20   addr.sin6_family = AF_INET6;\n\
         \x20   addr.sin6_port = htons((unsigned short)port);\n\
         \x20   addr.sin6_addr = in6addr_loopback;\n\
         \x20   if (bind(fd, (struct sockaddr *)&addr, sizeof(addr)) != 0 ||\n\
         \x20       listen(fd, 16) != 0) {{\n\
         \x20       close(fd);\n\
         \x20       return {NET_ERROR};\n\
         \x20   }}\n\
         \x20   return (long long)fd;\n\
         }}\n\
         \n\
         long long {PEER_FAMILY_SYMBOL}(long long fd) {{\n\
         \x20   struct sockaddr_storage addr;\n\
         \x20   socklen_t alen = sizeof(addr);\n\
         \x20   if (getsockname((int)fd, (struct sockaddr *)&addr, &alen) != 0)\n\
         \x20       return {NET_ERROR};\n\
         \x20   if (addr.ss_family == AF_INET) return {FAMILY_IPV4};\n\
         \x20   if (addr.ss_family == AF_INET6) return {FAMILY_IPV6};\n\
         \x20   return {NET_ERROR};\n\
         }}\n\
         \n\
         long long {ACCEPT_SYMBOL}(long long fd) {{\n\
         \x20   for (;;) {{\n\
         \x20       int conn = accept((int)fd, 0, 0);\n\
         \x20       if (conn >= 0) return (long long)conn;\n\
         \x20       if (errno == EINTR) continue;\n\
         \x20       return {NET_ERROR};\n\
         \x20   }}\n\
         }}\n\
         \n\
         long long {CONNECT_SYMBOL}(const unsigned char *ptr, unsigned long long len,\n\
         \x20                       long long port) {{\n\
         \x20   char host[64];\n\
         \x20   struct sockaddr_storage addr;\n\
         \x20   socklen_t alen;\n\
         \x20   int family, fd;\n\
         \x20   if (port < 0 || port > 65535) return {NET_ERROR};\n\
         \x20   if (!tuo_rt_path_copy(host, sizeof(host), ptr, len)) return {NET_ERROR};\n\
         \x20   family = tuo_rt_addr_parse(host, port, &addr, &alen);\n\
         \x20   if (family < 0) return {NET_ERROR};\n\
         \x20   fd = socket(family, SOCK_STREAM, 0);\n\
         \x20   if (fd < 0) return {NET_ERROR};\n\
         \x20   for (;;) {{\n\
         \x20       if (connect(fd, (struct sockaddr *)&addr, alen) == 0)\n\
         \x20           return (long long)fd;\n\
         \x20       /* An EINTR'd connect may complete asynchronously: a retry\n\
         \x20          reporting EISCONN is success, not failure. */\n\
         \x20       if (errno == EISCONN) return (long long)fd;\n\
         \x20       if (errno == EINTR) continue;\n\
         \x20       close(fd);\n\
         \x20       return {NET_ERROR};\n\
         \x20   }}\n\
         }}\n\
         \n\
         /* ADR-0017: bounded waits. Each computes a monotonic deadline once\n\
         \x20  and re-derives the remaining time on every EINTR retry, so a\n\
         \x20  signal storm can never extend the wait past `ms`. A negative\n\
         \x20  `ms` is a host error, never an unbounded wait. */\n\
         static int tuo_rt_poll_until(int fd, short events, long long ms) {{\n\
         \x20   struct timespec start;\n\
         \x20   struct pollfd pfd;\n\
         \x20   if (ms < 0) return -1;\n\
         \x20   if (clock_gettime(CLOCK_MONOTONIC, &start) != 0) return -1;\n\
         \x20   pfd.fd = fd;\n\
         \x20   pfd.events = events;\n\
         \x20   for (;;) {{\n\
         \x20       struct timespec now;\n\
         \x20       long long elapsed, remaining;\n\
         \x20       int n;\n\
         \x20       pfd.revents = 0;\n\
         \x20       if (clock_gettime(CLOCK_MONOTONIC, &now) != 0) return -1;\n\
         \x20       elapsed = (long long)(now.tv_sec - start.tv_sec) * 1000\n\
         \x20               + (long long)(now.tv_nsec - start.tv_nsec) / 1000000;\n\
         \x20       remaining = ms - elapsed;\n\
         \x20       if (remaining < 0) remaining = 0;\n\
         \x20       n = poll(&pfd, 1, (int)remaining);\n\
         \x20       if (n > 0) return 1;   /* ready */\n\
         \x20       if (n == 0) return 0;  /* timed out */\n\
         \x20       if (errno == EINTR) {{\n\
         \x20           if (remaining == 0) return 0;\n\
         \x20           continue;\n\
         \x20       }}\n\
         \x20       return -1;\n\
         \x20   }}\n\
         }}\n\
         \n\
         long long {ACCEPT_TIMEOUT_SYMBOL}(long long fd, long long ms) {{\n\
         \x20   int ready = tuo_rt_poll_until((int)fd, POLLIN, ms);\n\
         \x20   if (ready < 0) return {NET_ERROR};\n\
         \x20   if (ready == 0) return {NET_TIMEOUT};\n\
         \x20   for (;;) {{\n\
         \x20       int conn = accept((int)fd, 0, 0);\n\
         \x20       if (conn >= 0) return (long long)conn;\n\
         \x20       if (errno == EINTR) continue;\n\
         \x20       return {NET_ERROR};\n\
         \x20   }}\n\
         }}\n\
         \n\
         long long {READ_BYTE_TIMEOUT_SYMBOL}(long long fd, long long ms) {{\n\
         \x20   unsigned char byte;\n\
         \x20   int ready = tuo_rt_poll_until((int)fd, POLLIN, ms);\n\
         \x20   if (ready < 0) return {READ_ERROR};\n\
         \x20   if (ready == 0) return {NET_TIMEOUT};\n\
         \x20   for (;;) {{\n\
         \x20       ssize_t n = read((int)fd, &byte, 1);\n\
         \x20       if (n == 1) return (long long)byte;\n\
         \x20       if (n == 0) return {READ_EOF};\n\
         \x20       if (errno == EINTR) continue;\n\
         \x20       return {READ_ERROR};\n\
         \x20   }}\n\
         }}\n\
         \n\
         long long {CONNECT_TIMEOUT_SYMBOL}(const unsigned char *ptr,\n\
         \x20                              unsigned long long len,\n\
         \x20                              long long port, long long ms) {{\n\
         \x20   char host[64];\n\
         \x20   struct sockaddr_storage addr;\n\
         \x20   socklen_t alen;\n\
         \x20   int family, fd, flags, ready, err = 0;\n\
         \x20   socklen_t errlen = sizeof(err);\n\
         \x20   if (ms < 0) return {NET_ERROR};\n\
         \x20   if (port < 0 || port > 65535) return {NET_ERROR};\n\
         \x20   if (!tuo_rt_path_copy(host, sizeof(host), ptr, len)) return {NET_ERROR};\n\
         \x20   family = tuo_rt_addr_parse(host, port, &addr, &alen);\n\
         \x20   if (family < 0) return {NET_ERROR};\n\
         \x20   fd = socket(family, SOCK_STREAM, 0);\n\
         \x20   if (fd < 0) return {NET_ERROR};\n\
         \x20   /* Non-blocking for the bounded handshake, restored on success. */\n\
         \x20   flags = fcntl(fd, F_GETFL, 0);\n\
         \x20   if (flags < 0 || fcntl(fd, F_SETFL, flags | O_NONBLOCK) < 0) {{\n\
         \x20       close(fd);\n\
         \x20       return {NET_ERROR};\n\
         \x20   }}\n\
         \x20   for (;;) {{\n\
         \x20       if (connect(fd, (struct sockaddr *)&addr, alen) == 0) break;\n\
         \x20       if (errno == EISCONN) break;\n\
         \x20       if (errno == EINTR) continue;\n\
         \x20       if (errno == EINPROGRESS || errno == EALREADY) {{\n\
         \x20           ready = tuo_rt_poll_until(fd, POLLOUT, ms);\n\
         \x20           if (ready < 0) {{ close(fd); return {NET_ERROR}; }}\n\
         \x20           if (ready == 0) {{ close(fd); return {NET_TIMEOUT}; }}\n\
         \x20           if (getsockopt(fd, SOL_SOCKET, SO_ERROR, &err, &errlen) != 0\n\
         \x20               || err != 0) {{\n\
         \x20               close(fd);\n\
         \x20               return {NET_ERROR};\n\
         \x20           }}\n\
         \x20           break;\n\
         \x20       }}\n\
         \x20       close(fd);\n\
         \x20       return {NET_ERROR};\n\
         \x20   }}\n\
         \x20   if (fcntl(fd, F_SETFL, flags) < 0) {{ close(fd); return {NET_ERROR}; }}\n\
         \x20   return (long long)fd;\n\
         }}\n\
         \n\
         /* ADR-0017: UDP. A datagram is a message, not a stream, so a\n\
         \x20  receive reports the boundary (its length) and stages the\n\
         \x20  payload; tuo_rt_udp_byte_at indexes it. The staging table is a\n\
         \x20  small fixed array keyed by descriptor -- process-lived like the\n\
         \x20  ADR-0015 handle registries, so it introduces no new lifetime\n\
         \x20  concept -- and a datagram larger than the cap is truncated\n\
         \x20  while the true length is still reported, exactly as recvfrom\n\
         \x20  itself behaves. */\n\
         #define TUO_RT_UDP_SLOTS 16\n\
         \n\
         struct tuo_rt_udp_slot {{\n\
         \x20   int fd;\n\
         \x20   int used;\n\
         \x20   long long len;\n\
         \x20   long long peer_port;\n\
         \x20   unsigned char bytes[{UDP_DATAGRAM_CAP}];\n\
         }};\n\
         \n\
         static struct tuo_rt_udp_slot tuo_rt_udp_slots[TUO_RT_UDP_SLOTS];\n\
         \n\
         static struct tuo_rt_udp_slot *tuo_rt_udp_slot_for(int fd, int create) {{\n\
         \x20   int i;\n\
         \x20   for (i = 0; i < TUO_RT_UDP_SLOTS; i++)\n\
         \x20       if (tuo_rt_udp_slots[i].used && tuo_rt_udp_slots[i].fd == fd)\n\
         \x20           return &tuo_rt_udp_slots[i];\n\
         \x20   if (!create) return 0;\n\
         \x20   for (i = 0; i < TUO_RT_UDP_SLOTS; i++)\n\
         \x20       if (!tuo_rt_udp_slots[i].used) {{\n\
         \x20           tuo_rt_udp_slots[i].used = 1;\n\
         \x20           tuo_rt_udp_slots[i].fd = fd;\n\
         \x20           tuo_rt_udp_slots[i].len = -1;\n\
         \x20           tuo_rt_udp_slots[i].peer_port = -1;\n\
         \x20           return &tuo_rt_udp_slots[i];\n\
         \x20       }}\n\
         \x20   return 0;\n\
         }}\n\
         \n\
         long long {UDP_BIND_SYMBOL}(long long port) {{\n\
         \x20   struct sockaddr_in addr;\n\
         \x20   int one = 1;\n\
         \x20   int fd;\n\
         \x20   if (port < 0 || port > 65535) return {NET_ERROR};\n\
         \x20   fd = socket(AF_INET, SOCK_DGRAM, 0);\n\
         \x20   if (fd < 0) return {NET_ERROR};\n\
         \x20   setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &one, sizeof(one));\n\
         \x20   memset(&addr, 0, sizeof(addr));\n\
         \x20   addr.sin_family = AF_INET;\n\
         \x20   addr.sin_port = htons((unsigned short)port);\n\
         \x20   addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);\n\
         \x20   if (bind(fd, (struct sockaddr *)&addr, sizeof(addr)) != 0) {{\n\
         \x20       close(fd);\n\
         \x20       return {NET_ERROR};\n\
         \x20   }}\n\
         \x20   return (long long)fd;\n\
         }}\n\
         \n\
         long long {UDP_SEND_SYMBOL}(long long fd, const unsigned char *hptr,\n\
         \x20                       unsigned long long hlen, long long port,\n\
         \x20                       const unsigned char *bptr,\n\
         \x20                       unsigned long long blen) {{\n\
         \x20   char host[64];\n\
         \x20   struct sockaddr_storage addr;\n\
         \x20   socklen_t alen;\n\
         \x20   if (port < 0 || port > 65535) return {NET_ERROR};\n\
         \x20   if (!tuo_rt_path_copy(host, sizeof(host), hptr, hlen)) return {NET_ERROR};\n\
         \x20   if (tuo_rt_addr_parse(host, port, &addr, &alen) < 0) return {NET_ERROR};\n\
         \x20   for (;;) {{\n\
         \x20       ssize_t n = sendto((int)fd, bptr, (size_t)blen, 0,\n\
         \x20                          (struct sockaddr *)&addr, alen);\n\
         \x20       if (n >= 0) return (long long)n;\n\
         \x20       if (errno == EINTR) continue;\n\
         \x20       return {NET_ERROR};\n\
         \x20   }}\n\
         }}\n\
         \n\
         long long {UDP_RECV_SYMBOL}(long long fd, long long ms) {{\n\
         \x20   struct tuo_rt_udp_slot *slot;\n\
         \x20   struct sockaddr_storage from;\n\
         \x20   socklen_t flen = sizeof(from);\n\
         \x20   int ready = tuo_rt_poll_until((int)fd, POLLIN, ms);\n\
         \x20   if (ready < 0) return {NET_ERROR};\n\
         \x20   if (ready == 0) return {NET_TIMEOUT};\n\
         \x20   slot = tuo_rt_udp_slot_for((int)fd, 1);\n\
         \x20   if (!slot) return {NET_ERROR};\n\
         \x20   for (;;) {{\n\
         \x20       ssize_t n = recvfrom((int)fd, slot->bytes, sizeof(slot->bytes),\n\
         \x20                            0, (struct sockaddr *)&from, &flen);\n\
         \x20       if (n >= 0) {{\n\
         \x20           slot->len = (long long)n;\n\
         \x20           if (from.ss_family == AF_INET)\n\
         \x20               slot->peer_port =\n\
         \x20                   (long long)ntohs(((struct sockaddr_in *)&from)->sin_port);\n\
         \x20           else if (from.ss_family == AF_INET6)\n\
         \x20               slot->peer_port =\n\
         \x20                   (long long)ntohs(((struct sockaddr_in6 *)&from)->sin6_port);\n\
         \x20           else slot->peer_port = -1;\n\
         \x20           return slot->len;\n\
         \x20       }}\n\
         \x20       if (errno == EINTR) continue;\n\
         \x20       return {NET_ERROR};\n\
         \x20   }}\n\
         }}\n\
         \n\
         long long {UDP_BYTE_AT_SYMBOL}(long long fd, long long i) {{\n\
         \x20   struct tuo_rt_udp_slot *slot = tuo_rt_udp_slot_for((int)fd, 0);\n\
         \x20   long long staged;\n\
         \x20   if (!slot || slot->len < 0) return {NET_ERROR};\n\
         \x20   /* A truncated datagram reports its true length, so only the\n\
         \x20      bytes actually captured are readable. */\n\
         \x20   staged = slot->len < {UDP_DATAGRAM_CAP} ? slot->len : {UDP_DATAGRAM_CAP};\n\
         \x20   if (i < 0 || i >= staged) return {NET_ERROR};\n\
         \x20   return (long long)slot->bytes[i];\n\
         }}\n\
         \n\
         long long {UDP_PEER_PORT_SYMBOL}(long long fd) {{\n\
         \x20   struct tuo_rt_udp_slot *slot = tuo_rt_udp_slot_for((int)fd, 0);\n\
         \x20   if (!slot || slot->len < 0) return {NET_ERROR};\n\
         \x20   return slot->peer_port;\n\
         }}\n\
         \n\
         /* ADR-0015: channels and mutexes — communication over the effect\n\
         \x20  seam. Bounded static registries of process-lived handles; each\n\
         \x20  channel is a mutex+condvar over a heap FIFO whose nodes flow\n\
         \x20  through the ADR-0009 allocation seam; each mutex is an\n\
         \x20  error-checking pthread mutex (a misuse is a {SYNC_ERROR},\n\
         \x20  never undefined behavior). */\n\
         extern void tuo_rt_dealloc(void *ptr, unsigned long long size,\n\
         \x20                       unsigned long long align);\n\
         \n\
         typedef struct tuo_rt_chan_node {{\n\
         \x20   long long value;\n\
         \x20   struct tuo_rt_chan_node *next;\n\
         }} tuo_rt_chan_node;\n\
         \n\
         typedef struct {{\n\
         \x20   pthread_mutex_t lock;\n\
         \x20   pthread_cond_t ready;\n\
         \x20   tuo_rt_chan_node *head;\n\
         \x20   tuo_rt_chan_node *tail;\n\
         \x20   int closed;\n\
         }} tuo_rt_chan;\n\
         \n\
         static tuo_rt_chan tuo_rt_chans[{SYNC_REGISTRY_CAP}];\n\
         static long long tuo_rt_chan_count = 0;\n\
         static pthread_mutex_t tuo_rt_mutexes[{SYNC_REGISTRY_CAP}];\n\
         static long long tuo_rt_mutex_count = 0;\n\
         static pthread_mutex_t tuo_rt_sync_registry = PTHREAD_MUTEX_INITIALIZER;\n\
         \n\
         long long {CHAN_NEW_SYMBOL}(void) {{\n\
         \x20   long long id;\n\
         \x20   pthread_mutex_lock(&tuo_rt_sync_registry);\n\
         \x20   if (tuo_rt_chan_count >= {SYNC_REGISTRY_CAP}) {{\n\
         \x20       pthread_mutex_unlock(&tuo_rt_sync_registry);\n\
         \x20       return {SYNC_ERROR};\n\
         \x20   }}\n\
         \x20   id = tuo_rt_chan_count;\n\
         \x20   pthread_mutex_init(&tuo_rt_chans[id].lock, 0);\n\
         \x20   pthread_cond_init(&tuo_rt_chans[id].ready, 0);\n\
         \x20   tuo_rt_chans[id].head = 0;\n\
         \x20   tuo_rt_chans[id].tail = 0;\n\
         \x20   tuo_rt_chans[id].closed = 0;\n\
         \x20   tuo_rt_chan_count = id + 1;\n\
         \x20   pthread_mutex_unlock(&tuo_rt_sync_registry);\n\
         \x20   return id;\n\
         }}\n\
         \n\
         /* Handle validation happens under the registry lock, so a handle is\n\
         \x20  visible only after its slot is fully initialized. */\n\
         static tuo_rt_chan *tuo_rt_chan_get(long long ch) {{\n\
         \x20   tuo_rt_chan *c = 0;\n\
         \x20   pthread_mutex_lock(&tuo_rt_sync_registry);\n\
         \x20   if (ch >= 0 && ch < tuo_rt_chan_count) c = &tuo_rt_chans[ch];\n\
         \x20   pthread_mutex_unlock(&tuo_rt_sync_registry);\n\
         \x20   return c;\n\
         }}\n\
         \n\
         long long {CHAN_SEND_SYMBOL}(long long ch, long long v) {{\n\
         \x20   tuo_rt_chan *c = tuo_rt_chan_get(ch);\n\
         \x20   tuo_rt_chan_node *node;\n\
         \x20   if (!c || v < 0) return {SYNC_ERROR};\n\
         \x20   node = (tuo_rt_chan_node *)tuo_rt_alloc(sizeof(tuo_rt_chan_node), 8);\n\
         \x20   node->value = v;\n\
         \x20   node->next = 0;\n\
         \x20   pthread_mutex_lock(&c->lock);\n\
         \x20   if (c->closed) {{\n\
         \x20       pthread_mutex_unlock(&c->lock);\n\
         \x20       tuo_rt_dealloc(node, sizeof(tuo_rt_chan_node), 8);\n\
         \x20       return {SYNC_ERROR};\n\
         \x20   }}\n\
         \x20   if (c->tail) c->tail->next = node; else c->head = node;\n\
         \x20   c->tail = node;\n\
         \x20   pthread_cond_signal(&c->ready);\n\
         \x20   pthread_mutex_unlock(&c->lock);\n\
         \x20   return 0;\n\
         }}\n\
         \n\
         long long {CHAN_RECV_SYMBOL}(long long ch) {{\n\
         \x20   tuo_rt_chan *c = tuo_rt_chan_get(ch);\n\
         \x20   tuo_rt_chan_node *node;\n\
         \x20   long long v;\n\
         \x20   if (!c) return {SYNC_ERROR};\n\
         \x20   pthread_mutex_lock(&c->lock);\n\
         \x20   while (!c->head && !c->closed)\n\
         \x20       pthread_cond_wait(&c->ready, &c->lock);\n\
         \x20   if (!c->head) {{\n\
         \x20       pthread_mutex_unlock(&c->lock);\n\
         \x20       return {SYNC_ERROR};\n\
         \x20   }}\n\
         \x20   node = c->head;\n\
         \x20   c->head = node->next;\n\
         \x20   if (!c->head) c->tail = 0;\n\
         \x20   pthread_mutex_unlock(&c->lock);\n\
         \x20   v = node->value;\n\
         \x20   tuo_rt_dealloc(node, sizeof(tuo_rt_chan_node), 8);\n\
         \x20   return v;\n\
         }}\n\
         \n\
         long long {CHAN_CLOSE_SYMBOL}(long long ch) {{\n\
         \x20   tuo_rt_chan *c = tuo_rt_chan_get(ch);\n\
         \x20   if (!c) return {SYNC_ERROR};\n\
         \x20   pthread_mutex_lock(&c->lock);\n\
         \x20   c->closed = 1;\n\
         \x20   pthread_cond_broadcast(&c->ready);\n\
         \x20   pthread_mutex_unlock(&c->lock);\n\
         \x20   return 0;\n\
         }}\n\
         \n\
         long long {MUTEX_NEW_SYMBOL}(void) {{\n\
         \x20   long long id;\n\
         \x20   pthread_mutexattr_t attr;\n\
         \x20   pthread_mutex_lock(&tuo_rt_sync_registry);\n\
         \x20   if (tuo_rt_mutex_count >= {SYNC_REGISTRY_CAP}) {{\n\
         \x20       pthread_mutex_unlock(&tuo_rt_sync_registry);\n\
         \x20       return {SYNC_ERROR};\n\
         \x20   }}\n\
         \x20   id = tuo_rt_mutex_count;\n\
         \x20   pthread_mutexattr_init(&attr);\n\
         \x20   pthread_mutexattr_settype(&attr, PTHREAD_MUTEX_ERRORCHECK);\n\
         \x20   pthread_mutex_init(&tuo_rt_mutexes[id], &attr);\n\
         \x20   pthread_mutexattr_destroy(&attr);\n\
         \x20   tuo_rt_mutex_count = id + 1;\n\
         \x20   pthread_mutex_unlock(&tuo_rt_sync_registry);\n\
         \x20   return id;\n\
         }}\n\
         \n\
         static pthread_mutex_t *tuo_rt_mutex_get(long long m) {{\n\
         \x20   pthread_mutex_t *mu = 0;\n\
         \x20   pthread_mutex_lock(&tuo_rt_sync_registry);\n\
         \x20   if (m >= 0 && m < tuo_rt_mutex_count) mu = &tuo_rt_mutexes[m];\n\
         \x20   pthread_mutex_unlock(&tuo_rt_sync_registry);\n\
         \x20   return mu;\n\
         }}\n\
         \n\
         long long {MUTEX_LOCK_SYMBOL}(long long m) {{\n\
         \x20   pthread_mutex_t *mu = tuo_rt_mutex_get(m);\n\
         \x20   if (!mu) return {SYNC_ERROR};\n\
         \x20   return pthread_mutex_lock(mu) == 0 ? 0 : {SYNC_ERROR};\n\
         }}\n\
         \n\
         long long {MUTEX_UNLOCK_SYMBOL}(long long m) {{\n\
         \x20   pthread_mutex_t *mu = tuo_rt_mutex_get(m);\n\
         \x20   if (!mu) return {SYNC_ERROR};\n\
         \x20   return pthread_mutex_unlock(mu) == 0 ? 0 : {SYNC_ERROR};\n\
         }}\n",
        sentinel = crate::alloc::ZERO_SIZE_SENTINEL,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        ACCEPT_SYMBOL, ARG_BYTE_SYMBOL, ARG_COUNT_SYMBOL, ARG_MISSING, BOUND_PORT_SYMBOL,
        CHAN_CLOSE_SYMBOL, CHAN_NEW_SYMBOL, CHAN_RECV_SYMBOL, CHAN_SEND_SYMBOL, CLOSE_SYMBOL,
        CONNECT_SYMBOL, EXIT_SYMBOL, FILE_ERROR, FILE_NOT_FOUND, LISTEN_SYMBOL, MUTEX_LOCK_SYMBOL,
        MUTEX_NEW_SYMBOL, MUTEX_UNLOCK_SYMBOL, NET_ERROR, NOW_NANOS_SYMBOL, OPEN_SYMBOL,
        READ_BYTE_SYMBOL, READ_EOF, READ_ERROR, REMOVE_FILE_SYMBOL, SYNC_ERROR, SYNC_REGISTRY_CAP,
        WRITE_ERROR, WRITE_SYMBOL, effect_runtime_c_source, exit_status_of,
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

    #[test]
    fn the_c_source_defines_the_socket_symbols_and_matches_the_policy() {
        let source = effect_runtime_c_source();
        // The four ADR-0014 C-ABI signatures, exactly as
        // `specification/abi.md` fixes them.
        assert!(source.contains(&format!("long long {LISTEN_SYMBOL}(long long port)")));
        assert!(source.contains(&format!("long long {BOUND_PORT_SYMBOL}(long long fd)")));
        assert!(source.contains(&format!("long long {ACCEPT_SYMBOL}(long long fd)")));
        assert!(source.contains(&format!(
            "long long {CONNECT_SYMBOL}(const unsigned char *ptr, unsigned long long len"
        )));
        // Loopback-only listening, SO_REUSEADDR, the ephemeral-port query,
        // numeric-host parsing, and the EINTR/EISCONN connect policy.
        assert!(source.contains("htonl(INADDR_LOOPBACK)"));
        assert!(source.contains("SO_REUSEADDR"));
        assert!(source.contains("getsockname((int)fd"));
        assert!(source.contains("inet_pton(AF_INET, host"));
        assert!(source.contains("if (errno == EISCONN) return (long long)fd;"));
        // The socket error value sits outside the byte and descriptor range,
        // and shares the file vocabulary's generic-error code.
        for value in [NET_ERROR, FILE_ERROR] {
            assert!(value < 0, "{value} must sit outside the byte range");
        }
        assert_eq!(NET_ERROR, FILE_ERROR);
    }

    #[test]
    fn the_c_source_defines_the_sync_symbols_and_matches_the_policy() {
        let source = effect_runtime_c_source();
        // The seven ADR-0015 C-ABI signatures, exactly as
        // `specification/abi.md` fixes them.
        assert!(source.contains(&format!("long long {CHAN_NEW_SYMBOL}(void)")));
        assert!(source.contains(&format!(
            "long long {CHAN_SEND_SYMBOL}(long long ch, long long v)"
        )));
        assert!(source.contains(&format!("long long {CHAN_RECV_SYMBOL}(long long ch)")));
        assert!(source.contains(&format!("long long {CHAN_CLOSE_SYMBOL}(long long ch)")));
        assert!(source.contains(&format!("long long {MUTEX_NEW_SYMBOL}(void)")));
        assert!(source.contains(&format!("long long {MUTEX_LOCK_SYMBOL}(long long m)")));
        assert!(source.contains(&format!("long long {MUTEX_UNLOCK_SYMBOL}(long long m)")));
        // The load-bearing policies: negative payloads refused (so the
        // closed signal stays unambiguous), the blocking condvar wait, the
        // close broadcast, error-checking mutexes, FIFO nodes through the
        // ADR-0009 allocation seam, and the bounded registries.
        assert!(source.contains("if (!c || v < 0) return"));
        assert!(source.contains("pthread_cond_wait(&c->ready, &c->lock);"));
        assert!(source.contains("pthread_cond_broadcast(&c->ready);"));
        assert!(source.contains("PTHREAD_MUTEX_ERRORCHECK"));
        assert!(source.contains("tuo_rt_alloc(sizeof(tuo_rt_chan_node), 8)"));
        assert!(source.contains("tuo_rt_dealloc(node, sizeof(tuo_rt_chan_node), 8);"));
        assert!(source.contains(&format!("[{SYNC_REGISTRY_CAP}]")));
        // The sync error value sits outside the byte and handle range, and
        // shares the seam's generic-error code.
        for value in [SYNC_ERROR, NET_ERROR] {
            assert!(value < 0, "{value} must sit outside the byte range");
        }
        assert_eq!(SYNC_ERROR, NET_ERROR);
    }
}
