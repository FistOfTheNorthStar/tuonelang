//! End-to-end tests for the native effect boundary (ADR-0006 Stage B).
//!
//! Effectful programs cannot go through the interpreter suites — the reference
//! interpreter never performs an effect, and the spec-purity gate (`R0007`)
//! only shields *specs*, so `tuo run` of an effectful `main` must simply work.
//! These tests therefore build and run real binaries through the actual `tuo`
//! binary with **both** backends (Cranelift by default, LLVM via `--release`)
//! and assert the observable process behavior: bytes written to the right
//! stream, the write's return value surfacing as the exit status, `exit`
//! terminating mid-`main` without running the rest, and `read_byte` echoing a
//! piped stdin byte (and reporting `-1`, exit byte 255, at EOF).
//!
//! The runtime seam under test is `tuo_runtime::effect::effect_runtime_c_source`
//! (`tuo_rt_write`/`tuo_rt_read_byte`/`tuo_rt_exit`), which the CLI links into
//! every built binary alongside the trap shim.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// A unique scratch directory per test (so tests do not collide when run in
/// parallel), rooted under Cargo's per-crate temp directory.
fn workspace(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("effects_native")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch workspace is creatable");
    dir
}

/// Write `source` as a `.tuo` program and `tuo run` it with the chosen
/// backend, returning the completed process output.
fn run_program(dir: &Path, name: &str, source: &str, release: bool) -> Output {
    let path = dir.join(name);
    fs::write(&path, source).expect("program is writable");
    let mut command = Command::new(env!("CARGO_BIN_EXE_tuo"));
    command.arg("run");
    if release {
        command.arg("--release");
    }
    command.arg(&path).output().expect("the tuo binary runs")
}

/// Like [`run_program`], but with `stdin_bytes` piped to the program's stdin.
fn run_program_with_stdin(
    dir: &Path,
    name: &str,
    source: &str,
    release: bool,
    stdin_bytes: &[u8],
) -> Output {
    let path = dir.join(name);
    fs::write(&path, source).expect("program is writable");
    let mut command = Command::new(env!("CARGO_BIN_EXE_tuo"));
    command.arg("run");
    if release {
        command.arg("--release");
    }
    let mut child = command
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the tuo binary runs");
    child
        .stdin
        .as_mut()
        .expect("stdin is piped")
        .write_all(stdin_bytes)
        .expect("stdin bytes are writable");
    child.wait_with_output().expect("the program completes")
}

/// Compile `source` to an executable with `tuo build` and return its path,
/// so a test can run it with real command-line arguments and a chosen
/// working directory (which `tuo run` does not forward).
fn build_program(dir: &Path, name: &str, source: &str, release: bool) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, source).expect("program is writable");
    let exe = dir.join(format!("{name}.exe"));
    let mut command = Command::new(env!("CARGO_BIN_EXE_tuo"));
    command.arg("build");
    if release {
        command.arg("--release");
    }
    let output = command
        .arg("-o")
        .arg(&exe)
        .arg(&path)
        .output()
        .expect("the tuo binary runs");
    assert!(
        output.status.success(),
        "build succeeds; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    exe
}

/// The backend a `release` flag selects, for failure messages.
fn backend_name(release: bool) -> &'static str {
    if release { "llvm" } else { "cranelift" }
}

/// `std::rt::write` to fd 1: the bytes land on stdout exactly, and the
/// write's return value (the byte count, 13) is `main`'s result and therefore
/// the process exit status — on both backends.
#[test]
fn write_to_stdout_returns_the_byte_count_and_lands_on_stdout() {
    let dir = workspace("write_stdout");
    let source = "fn main() -> Int {\n    std::rt::write(1, \"hello, world\\n\")\n}\n";
    for release in [false, true] {
        let output = run_program(&dir, "hello.tuo", source, release);
        let which = backend_name(release);
        assert_eq!(
            output.status.code(),
            Some(13),
            "{which}: main returns the write's return value (13 bytes written); stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "hello, world\n",
            "{which}: the written bytes must land on stdout, exactly"
        );
    }
}

/// `std::rt::exit(7)` mid-`main` terminates with status 7 without executing
/// the rest of the function: the later write never happens and the would-be
/// return value 99 is never observed — on both backends.
#[test]
fn exit_mid_main_terminates_without_running_the_rest() {
    let dir = workspace("exit_mid");
    let source = "fn main() -> Int {\n    std::rt::write(1, \"before\");\n    \
                  std::rt::exit(7);\n    std::rt::write(1, \"after\");\n    99\n}\n";
    for release in [false, true] {
        let output = run_program(&dir, "exit_mid.tuo", source, release);
        let which = backend_name(release);
        assert_eq!(
            output.status.code(),
            Some(7),
            "{which}: exit(7) is the process status, not the 99 main would return; stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "before",
            "{which}: output before the exit is flushed; nothing after it runs"
        );
    }
}

/// `std::rt::write` to fd 2 lands on stderr, not stdout — on both backends.
/// (stderr also carries toolchain noise such as linker warnings, so the
/// assertion is containment there and exact emptiness on stdout.)
#[test]
fn write_to_fd_2_lands_on_stderr_not_stdout() {
    let dir = workspace("write_stderr");
    let source = "fn main() -> Int {\n    std::rt::write(2, \"to-stderr-marker\")\n}\n";
    for release in [false, true] {
        let output = run_program(&dir, "err.tuo", source, release);
        let which = backend_name(release);
        assert_eq!(
            output.status.code(),
            Some(16),
            "{which}: 16 bytes written to fd 2"
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("to-stderr-marker"),
            "{which}: the bytes must land on stderr; got:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "",
            "{which}: nothing may land on stdout"
        );
    }
}

/// `std::rt::read_byte(0)` reads the piped stdin byte and `main` returns it:
/// the exit status is the byte's value — on both backends. At end of input it
/// returns `-1`, whose exit byte is 255.
#[test]
fn read_byte_echoes_piped_stdin_and_reports_eof() {
    let dir = workspace("read_byte");
    let source = "fn main() -> Int {\n    std::rt::read_byte(0)\n}\n";
    for release in [false, true] {
        let which = backend_name(release);
        let output = run_program_with_stdin(&dir, "echo.tuo", source, release, b"A");
        assert_eq!(
            output.status.code(),
            Some(65),
            "{which}: the piped byte 'A' (65) echoes as the exit status; stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let eof = run_program_with_stdin(&dir, "echo.tuo", source, release, b"");
        assert_eq!(
            eof.status.code(),
            Some(255),
            "{which}: EOF is -1, whose exit byte is 255"
        );
    }
}

/// `std::rt::now_nanos()` (ADR-0013) is positive and monotonic: two reads in
/// sequence never go backwards — on both backends. (The clock's actual value
/// is non-deterministic, so monotonicity and positivity are the only honest
/// native assertions.)
#[test]
fn now_nanos_is_positive_and_monotonic() {
    let dir = workspace("now_nanos");
    let source = "fn main() -> Int {\n    let a = std::rt::now_nanos();\n    \
                  let b = std::rt::now_nanos();\n    \
                  if b >= a {\n        if a > 0 { 0 } else { 2 }\n    } else { 1 }\n}\n";
    for release in [false, true] {
        let output = run_program(&dir, "clock.tuo", source, release);
        let which = backend_name(release);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{which}: the monotonic clock is positive and never goes backwards; stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// `std::rt::arg_count()`/`arg_byte(i, j)` (ADR-0013) see the real command
/// line: the count includes the program name, `arg_byte` reads an argument's
/// bytes, and both out-of-range indexes report `-1` — on both backends. Runs
/// the built executable directly, since `tuo run` forwards no arguments.
#[test]
fn arg_count_and_arg_byte_read_the_real_command_line() {
    let dir = workspace("argv");
    let count_src = "fn main() -> Int {\n    std::rt::arg_count()\n}\n";
    let byte_src = "fn main() -> Int {\n    \
                    if std::rt::arg_byte(9, 0) != 0 - 1 { return 1; }\n    \
                    if std::rt::arg_byte(1, 9) != 0 - 1 { return 2; }\n    \
                    std::rt::arg_byte(1, 2)\n}\n";
    for release in [false, true] {
        let which = backend_name(release);
        let count_exe = build_program(&dir, &format!("argc_{which}.tuo"), count_src, release);
        let bare = Command::new(&count_exe).output().expect("the exe runs");
        assert_eq!(
            bare.status.code(),
            Some(1),
            "{which}: with no arguments, argc is 1 (the program name)"
        );
        let with_args = Command::new(&count_exe)
            .args(["alpha", "beta", "gamma"])
            .output()
            .expect("the exe runs");
        assert_eq!(
            with_args.status.code(),
            Some(4),
            "{which}: three arguments plus the program name is 4"
        );
        let byte_exe = build_program(&dir, &format!("argb_{which}.tuo"), byte_src, release);
        let output = Command::new(&byte_exe)
            .arg("xyz")
            .output()
            .expect("the exe runs");
        assert_eq!(
            output.status.code(),
            Some(122),
            "{which}: byte 2 of argument 1 (\"xyz\") is 'z' (122); out-of-range reads are -1"
        );
    }
}

/// The ADR-0013 file primitives round-trip for real: `open` write mode
/// creates and truncates, `write` puts bytes in, `open` append mode extends,
/// `open` read mode plus `read_byte` gets the exact bytes back with `-1` at
/// EOF, `close` succeeds, `remove_file` deletes (a second open reports `-2`
/// not-found) — on both backends, in a scratch working directory.
#[test]
fn file_open_write_read_append_remove_roundtrip() {
    let dir = workspace("file_roundtrip");
    let source = "fn main() -> Int {\n    \
        let path = \"roundtrip.tmp\";\n    \
        let fd = std::rt::open(path, 1);\n    \
        if fd < 0 { return 10; }\n    \
        if std::rt::write(fd, \"hi\") != 2 { return 11; }\n    \
        if std::rt::close(fd) != 0 { return 12; }\n    \
        let afd = std::rt::open(path, 2);\n    \
        if afd < 0 { return 13; }\n    \
        if std::rt::write(afd, \"!\") != 1 { return 14; }\n    \
        if std::rt::close(afd) != 0 { return 15; }\n    \
        let rfd = std::rt::open(path, 0);\n    \
        if rfd < 0 { return 16; }\n    \
        if std::rt::read_byte(rfd) != 104 { return 17; }\n    \
        if std::rt::read_byte(rfd) != 105 { return 18; }\n    \
        if std::rt::read_byte(rfd) != 33 { return 19; }\n    \
        if std::rt::read_byte(rfd) != 0 - 1 { return 20; }\n    \
        if std::rt::close(rfd) != 0 { return 21; }\n    \
        if std::rt::remove_file(path) != 0 { return 22; }\n    \
        if std::rt::open(path, 0) != 0 - 2 { return 23; }\n    \
        if std::rt::remove_file(path) != 0 - 2 { return 24; }\n    \
        0\n}\n";
    for release in [false, true] {
        let which = backend_name(release);
        let exe = build_program(&dir, &format!("files_{which}.tuo"), source, release);
        let output = Command::new(&exe)
            .current_dir(&dir)
            .output()
            .expect("the exe runs");
        assert_eq!(
            output.status.code(),
            Some(0),
            "{which}: the full write/append/read/remove roundtrip succeeds \
             (a nonzero status names the failing step); stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// The ADR-0014 socket primitives round-trip for real, in one process over
/// loopback: `listen(0)` takes an ephemeral port `bound_port` reveals,
/// `connect` completes against the listening socket's backlog *before*
/// `accept` runs (TCP completes the handshake in the kernel), and the two
/// connected descriptors move bytes both ways through the ordinary
/// `write`/`read_byte` seam, with EOF after the peer closes. A `connect` to
/// the closed port and a non-numeric host are host errors, not traps — on
/// both backends.
#[test]
fn socket_listen_connect_accept_roundtrip() {
    let dir = workspace("socket_roundtrip");
    let source = "fn main() -> Int {\n    \
        let listener = std::rt::listen(0);\n    \
        if listener < 0 { return 10; }\n    \
        let port = std::rt::bound_port(listener);\n    \
        if port <= 0 { return 11; }\n    \
        let client = std::rt::connect(\"127.0.0.1\", port);\n    \
        if client < 0 { return 12; }\n    \
        let server = std::rt::accept(listener);\n    \
        if server < 0 { return 13; }\n    \
        if std::rt::write(client, \"hi\") != 2 { return 14; }\n    \
        if std::rt::read_byte(server) != 104 { return 15; }\n    \
        if std::rt::read_byte(server) != 105 { return 16; }\n    \
        if std::rt::write(server, \"!\") != 1 { return 17; }\n    \
        if std::rt::read_byte(client) != 33 { return 18; }\n    \
        if std::rt::close(client) != 0 { return 19; }\n    \
        if std::rt::read_byte(server) != 0 - 1 { return 20; }\n    \
        if std::rt::close(server) != 0 { return 21; }\n    \
        if std::rt::close(listener) != 0 { return 22; }\n    \
        if std::rt::connect(\"127.0.0.1\", port) >= 0 { return 23; }\n    \
        if std::rt::connect(\"localhost\", port) >= 0 { return 24; }\n    \
        0\n}\n";
    for release in [false, true] {
        let which = backend_name(release);
        let output = run_program(&dir, &format!("sockets_{which}.tuo"), source, release);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{which}: the full listen/connect/accept/write/read/close \
             roundtrip succeeds (a nonzero status names the failing step); \
             stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// The ADR-0017 bounded-wait primitives really bound their wait, and report
/// a timeout **distinguishably** from an error or end of input.
///
/// Only the robust directions are asserted, never a precise duration: an
/// `accept_timeout` on a listener nobody connects to must report the `-2`
/// timeout (not hang, not error); a `read_byte_timeout` on a connection whose
/// peer sends nothing must do the same, then read the byte once it is there;
/// a negative `ms` must be a host error rather than an unbounded wait; and
/// `connect_timeout` must still succeed on a live listener. The test's own
/// completion is the proof the waits are bounded — an unbounded one would
/// hang the suite.
#[test]
fn bounded_waits_time_out_without_blocking_forever() {
    let dir = workspace("bounded_waits");
    let source = "fn main() -> Int {\n    \
        let listener = std::rt::listen(0);\n    \
        if listener < 0 { return 10; }\n    \
        let port = std::rt::bound_port(listener);\n    \
        if port <= 0 { return 11; }\n    \
        if std::rt::accept_timeout(listener, 50) != 0 - 3 { return 12; }\n    \
        if std::rt::accept_timeout(listener, 0 - 1) != 0 - 1 { return 13; }\n    \
        let client = std::rt::connect_timeout(\"127.0.0.1\", port, 1000);\n    \
        if client < 0 { return 14; }\n    \
        let server = std::rt::accept_timeout(listener, 1000);\n    \
        if server < 0 { return 15; }\n    \
        if std::rt::read_byte_timeout(server, 50) != 0 - 3 { return 16; }\n    \
        if std::rt::write(client, \"z\") != 1 { return 17; }\n    \
        if std::rt::read_byte_timeout(server, 1000) != 122 { return 18; }\n    \
        if std::rt::read_byte_timeout(server, 0 - 1) != 0 - 2 { return 19; }\n    \
        if std::rt::close(client) != 0 { return 20; }\n    \
        if std::rt::read_byte_timeout(server, 1000) != 0 - 1 { return 21; }\n    \
        if std::rt::close(server) != 0 { return 22; }\n    \
        if std::rt::close(listener) != 0 { return 23; }\n    \
        if std::rt::connect_timeout(\"127.0.0.1\", port, 1000) >= 0 { return 24; }\n    \
        if std::rt::connect_timeout(\"nope\", port, 1000) != 0 - 1 { return 25; }\n    \
        0\n}\n";
    for release in [false, true] {
        let which = backend_name(release);
        let output = run_program(&dir, &format!("timeouts_{which}.tuo"), source, release);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{which}: the bounded waits time out, distinguish a timeout from \
             an error and from EOF, and still complete a live roundtrip (a \
             nonzero status names the failing step); stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// The ADR-0017 IPv6 surface: `listen6` really binds `[::1]`, `connect`
/// infers the family from the address it is given (so the *same* spelling
/// reaches both stacks), `bound_port` reads the port back from **either**
/// family, and `peer_family` reports which one a descriptor got.
///
/// `bound_port` reads through `sockaddr_storage` and switches on
/// `ss_family` rather than assuming `sockaddr_in`. On the POSIX layouts this
/// project targets `sin_port` and `sin6_port` both sit at offset 2, so the
/// v4-only spelling happened to read the right bytes for a v6 socket — but
/// only by coincidence of layout, not by any guarantee, and it also passed a
/// too-small `sizeof(sockaddr_in)` to `getsockname` for a v6 address. The
/// explicit form is correct by construction; this test pins the behavior on
/// both families either way.
///
/// A host with IPv6 loopback disabled is a real configuration, so a failure
/// to *create* the v6 listener is tolerated; everything after it is not.
#[test]
fn ipv6_listen_connect_and_family_reporting() {
    let dir = workspace("ipv6_sockets");
    let source = "fn main() -> Int {\n    \
        let v4 = std::rt::listen(0);\n    \
        if v4 < 0 { return 10; }\n    \
        let v4port = std::rt::bound_port(v4);\n    \
        if v4port <= 0 { return 11; }\n    \
        if std::rt::peer_family(v4) != 4 { return 12; }\n    \
        let c4 = std::rt::connect(\"127.0.0.1\", v4port);\n    \
        if c4 < 0 { return 13; }\n    \
        if std::rt::peer_family(c4) != 4 { return 14; }\n    \
        let _ = std::rt::close(c4);\n    \
        let _ = std::rt::close(v4);\n    \
        let v6 = std::rt::listen6(0);\n    \
        if v6 < 0 { return 0; }\n    \
        let v6port = std::rt::bound_port(v6);\n    \
        if v6port <= 0 { return 15; }\n    \
        if std::rt::peer_family(v6) != 6 { return 16; }\n    \
        let c6 = std::rt::connect(\"::1\", v6port);\n    \
        if c6 < 0 { return 17; }\n    \
        if std::rt::peer_family(c6) != 6 { return 18; }\n    \
        let s6 = std::rt::accept_timeout(v6, 1000);\n    \
        if s6 < 0 { return 19; }\n    \
        if std::rt::write(c6, \"6\") != 1 { return 20; }\n    \
        if std::rt::read_byte_timeout(s6, 1000) != 54 { return 21; }\n    \
        if std::rt::close(c6) != 0 { return 22; }\n    \
        if std::rt::close(s6) != 0 { return 23; }\n    \
        if std::rt::close(v6) != 0 { return 24; }\n    \
        if std::rt::connect(\"::2\", v6port) >= 0 { return 25; }\n    \
        if std::rt::connect(\"not-an-address\", v6port) >= 0 { return 26; }\n    \
        0\n}\n";
    for release in [false, true] {
        let which = backend_name(release);
        let output = run_program(&dir, &format!("ipv6_{which}.tuo"), source, release);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{which}: the v4 path still works, and where IPv6 loopback exists \
             listen6/connect(\"::1\")/bound_port/peer_family all agree (a \
             nonzero status names the failing step); stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// The ADR-0017 UDP surface: a real datagram round-trip in one process,
/// including the **reply** path that motivated `udp_peer_port`.
///
/// A server socket and a client socket are bound to ephemeral loopback
/// ports; the client sends a datagram, the server receives it (getting the
/// *length* back, because a datagram is a message), indexes the payload with
/// `udp_byte_at`, and replies to the port `udp_peer_port` reports. The
/// timeout paths are asserted too: a receive on a silent socket reports the
/// `-3` timeout rather than hanging, and an out-of-range index is a host
/// error rather than a trap.
#[test]
fn udp_datagram_roundtrip_with_reply_to_sender() {
    let dir = workspace("udp_sockets");
    let source = "fn main() -> Int {\n    \
        let server = std::rt::udp_bind(0);\n    \
        if server < 0 { return 10; }\n    \
        let sport = std::rt::bound_port(server);\n    \
        if sport <= 0 { return 11; }\n    \
        let client = std::rt::udp_bind(0);\n    \
        if client < 0 { return 12; }\n    \
        let cport = std::rt::bound_port(client);\n    \
        if cport <= 0 { return 13; }\n    \
        if std::rt::udp_recv(server, 50) != 0 - 3 { return 14; }\n    \
        if std::rt::udp_byte_at(server, 0) >= 0 { return 15; }\n    \
        if std::rt::udp_send(client, \"127.0.0.1\", sport, \"ping\") != 4 { return 16; }\n    \
        if std::rt::udp_recv(server, 1000) != 4 { return 17; }\n    \
        if std::rt::udp_byte_at(server, 0) != 112 { return 18; }\n    \
        if std::rt::udp_byte_at(server, 1) != 105 { return 19; }\n    \
        if std::rt::udp_byte_at(server, 2) != 110 { return 20; }\n    \
        if std::rt::udp_byte_at(server, 3) != 103 { return 21; }\n    \
        if std::rt::udp_byte_at(server, 4) >= 0 { return 22; }\n    \
        if std::rt::udp_byte_at(server, 0 - 1) >= 0 { return 23; }\n    \
        if std::rt::udp_peer_port(server) != cport { return 24; }\n    \
        if std::rt::udp_send(server, \"127.0.0.1\", cport, \"ok\") != 2 { return 25; }\n    \
        if std::rt::udp_recv(client, 1000) != 2 { return 26; }\n    \
        if std::rt::udp_byte_at(client, 0) != 111 { return 27; }\n    \
        if std::rt::udp_byte_at(client, 1) != 107 { return 28; }\n    \
        if std::rt::udp_peer_port(client) != sport { return 29; }\n    \
        if std::rt::close(client) != 0 { return 30; }\n    \
        if std::rt::close(server) != 0 { return 31; }\n    \
        0\n}\n";
    for release in [false, true] {
        let which = backend_name(release);
        let output = run_program(&dir, &format!("udp_{which}.tuo"), source, release);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{which}: the datagram roundtrip, the reply to udp_peer_port, and \
             the timeout/out-of-range paths all behave (a nonzero status \
             names the failing step); stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// The ADR-0015 channel and mutex primitives obey their documented policy,
/// single-threaded: FIFO order, negative payloads refused (so the closed
/// signal stays unambiguous), sends to a closed channel refused, `-1` after
/// close-and-drain, idempotent close, invalid handles as `-1`, and the
/// error-checking mutex reporting a relock and a non-holder unlock as `-1`
/// instead of undefined behavior — on both backends.
#[test]
fn channel_and_mutex_policy_roundtrip() {
    let dir = workspace("sync_policy");
    let source = "fn main() -> Int {\n    \
        let ch = std::rt::chan_new();\n    \
        if ch < 0 { return 10; }\n    \
        if std::rt::chan_send(ch, 7) != 0 { return 11; }\n    \
        if std::rt::chan_send(ch, 0 - 3) != 0 - 1 { return 12; }\n    \
        if std::rt::chan_send(ch, 9) != 0 { return 13; }\n    \
        if std::rt::chan_recv(ch) != 7 { return 14; }\n    \
        if std::rt::chan_recv(ch) != 9 { return 15; }\n    \
        if std::rt::chan_close(ch) != 0 { return 16; }\n    \
        if std::rt::chan_send(ch, 1) != 0 - 1 { return 17; }\n    \
        if std::rt::chan_recv(ch) != 0 - 1 { return 18; }\n    \
        if std::rt::chan_close(ch) != 0 { return 19; }\n    \
        if std::rt::chan_recv(9999) != 0 - 1 { return 20; }\n    \
        let m = std::rt::mutex_new();\n    \
        if m < 0 { return 21; }\n    \
        if std::rt::mutex_lock(m) != 0 { return 22; }\n    \
        if std::rt::mutex_lock(m) != 0 - 1 { return 23; }\n    \
        if std::rt::mutex_unlock(m) != 0 { return 24; }\n    \
        if std::rt::mutex_unlock(m) != 0 - 1 { return 25; }\n    \
        if std::rt::mutex_lock(9999) != 0 - 1 { return 26; }\n    \
        0\n}\n";
    for release in [false, true] {
        let which = backend_name(release);
        let output = run_program(&dir, &format!("sync_{which}.tuo"), source, release);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{which}: the full channel/mutex policy roundtrip succeeds \
             (a nonzero status names the failing step); stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// Channels really synchronize across OS threads (ADR-0015 ∘ ADR-0007): a
/// pre-filled, closed channel is drained by three `par_map` workers racing
/// `chan_recv` — dynamic stealing, where which worker gets which value is
/// scheduler-dependent — and the drained total still equals the sum of what
/// was sent (1..=10 → 55), the invariant stealing must preserve — on both
/// backends.
#[test]
fn channels_distribute_work_across_par_map_threads() {
    let dir = workspace("chan_threads");
    let source = "fn drain(take ch: Int) -> Int {\n    \
        var acc = 0;\n    \
        var done = false;\n    \
        while !done {\n        \
        let v = std::rt::chan_recv(ch);\n        \
        if v < 0 { done = true; } else { acc = acc + v; }\n    \
        }\n    \
        acc\n}\n\
        fn main() -> Int {\n    \
        let ch = std::rt::chan_new();\n    \
        if ch < 0 { return 1; }\n    \
        var tasks = std::array::empty();\n    \
        std::array::push(tasks, ch);\n    \
        std::array::push(tasks, ch);\n    \
        std::array::push(tasks, ch);\n    \
        var i = 1;\n    \
        while i <= 10 {\n        \
        if std::rt::chan_send(ch, i) != 0 { return 2; }\n        \
        i = i + 1;\n    \
        }\n    \
        if std::rt::chan_close(ch) != 0 { return 3; }\n    \
        let sums = std::rt::par_map(drain, tasks, 3);\n    \
        var total = 0;\n    \
        var j = 0;\n    \
        while j < std::array::len(sums) {\n        \
        total = total + std::array::get(sums, j);\n        \
        j = j + 1;\n    \
        }\n    \
        total\n}\n";
    for release in [false, true] {
        let which = backend_name(release);
        let output = run_program(&dir, &format!("chan_par_{which}.tuo"), source, release);
        assert_eq!(
            output.status.code(),
            Some(55),
            "{which}: three workers drain the queue to exactly the sum sent \
             (55), however the values are stolen; stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
