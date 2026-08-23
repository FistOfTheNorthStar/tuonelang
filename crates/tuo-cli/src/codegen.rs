//! The `tuo build` and `tuo run` commands: native compilation via the backend.
//!
//! Both drive the same pipeline: run the front end, lower the accepted program
//! to **verified** MIR, hand it to a native backend behind the tuonelang-owned
//! [`CodegenBackend`](tuo_codegen::CodegenBackend) interface, then link the
//! emitted object together with the runtime's trap shim (compiled from
//! [`tuo_runtime::trap_runtime_c_source`]), effect shim (compiled from
//! [`tuo_runtime::effect::effect_runtime_c_source`], ADR-0006 Stage B), and
//! allocator shim (compiled from [`tuo_runtime::alloc::alloc_runtime_c_source`],
//! ADR-0009 Stage B) into a native executable using the platform `cc`.
//!
//! Which backend runs is a `--release` choice, and the *only* thing it changes
//! is speed: the default debug build uses the [Cranelift
//! backend](tuo_codegen_cranelift) (fast, unoptimized), and `--release` uses the
//! optimizing [LLVM backend](tuo_codegen_llvm). Both agree with the reference
//! interpreter — and each other — on every program's observable result, pinned
//! by the three-way differential suite. The CLI selects between them through the
//! shared interface and never sees a Cranelift or LLVM type.
//!
//! - `tuo build` writes the executable to disk (next to the first input, or to
//!   `-o <path>`) and stops.
//! - `tuo run` builds to a temporary executable, runs it, and propagates its
//!   exit status — which, by the v0 entry ABI, is the integer the program's
//!   entry returns. That is the same value the reference interpreter observes
//!   running the same entry, so a native run and an interpreted run agree.
//!
//! The chosen entry is the function named `main`. The backend requires it to be
//! nullary and to return an integer (its value becomes the process exit code);
//! anything else is refused with a clear message rather than mis-compiled.
//!
//! Human mode reports progress and errors on stderr; a machine format drives the
//! [`crate::protocol`] event stream on stdout (`started` → `progress` per stage
//! → `finished` with an artifact/exit summary).

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use serde_json::json;
use tuo_codegen::{CodegenBackend, CodegenError, EntryAbi, ObjectArtifact, TargetSpec};
use tuo_codegen_cranelift::CraneliftBackend;
use tuo_codegen_llvm::LlvmBackend;
use tuo_compiler::ast::Ast;
use tuo_compiler::source::{SourceId, SourceMap};
use tuo_compiler::{diagnostics, hir, mir, parser, types};

use crate::output::OutputMode;
use crate::protocol::{self, Emitter, Event, ProtocolCommand, Status};

/// The entry function a built program starts at.
const ENTRY: &str = "main";

/// `tuo build [-o out] [--release] <files>`: compile to a native executable.
pub(crate) fn build(
    output: Option<PathBuf>,
    release: bool,
    files: &[PathBuf],
    mode: OutputMode,
) -> ExitCode {
    drive(files, output, Backend::select(release), Mode::Build, mode)
}

/// `tuo run [--release] <files>`: compile to a temporary executable and run it.
pub(crate) fn run(release: bool, files: &[PathBuf], mode: OutputMode) -> ExitCode {
    drive(files, None, Backend::select(release), Mode::Run, mode)
}

/// Which native backend a build uses. The debug build favors fast compilation
/// (Cranelift, unoptimized); the release build favors fast *output* (LLVM's
/// standard optimization pipeline). Both implement the same
/// [`CodegenBackend`](tuo_codegen::CodegenBackend) and must produce a program
/// that agrees with the reference interpreter.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Backend {
    /// The default: the Cranelift backend, emitting unoptimized code quickly.
    Cranelift,
    /// `--release`: the LLVM backend, running LLVM's standard optimizer.
    Llvm,
}

impl Backend {
    /// The backend `--release` selects: LLVM when set, Cranelift otherwise.
    fn select(release: bool) -> Self {
        if release { Self::Llvm } else { Self::Cranelift }
    }

    /// Compile `program` with this backend behind the shared interface.
    fn compile(
        self,
        program: &mir::Program,
        types: &types::TypeckResult,
        target: &TargetSpec,
    ) -> Result<ObjectArtifact, CodegenError> {
        // Each backend is a distinct concrete type; both are driven only through
        // the `CodegenBackend` trait, so no Cranelift or LLVM type is named here.
        let backend: &dyn CodegenBackend = match self {
            Self::Cranelift => &CraneliftBackend::new(),
            Self::Llvm => &LlvmBackend::release(),
        };
        backend.compile(program, types, ENTRY, EntryAbi::IntReturn, target)
    }

    /// The stable backend name for the progress/summary reporting.
    fn name(self) -> &'static str {
        match self {
            Self::Cranelift => "cranelift",
            Self::Llvm => "llvm",
        }
    }
}

/// Which command is being driven.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Build,
    Run,
}

impl Mode {
    fn command(self) -> ProtocolCommand {
        match self {
            Self::Build => ProtocolCommand::Build,
            Self::Run => ProtocolCommand::Run,
        }
    }
}

/// The shared body of `build` and `run`.
fn drive(
    files: &[PathBuf],
    output: Option<PathBuf>,
    backend: Backend,
    kind: Mode,
    mode: OutputMode,
) -> ExitCode {
    let command = kind.command();
    let (map, sources) = match load(files, command, mode) {
        Ok(loaded) => loaded,
        Err(code) => return code,
    };
    drive_loaded(&map, &sources, files, output, backend, kind, mode)
}

/// Compile and finish an already-loaded program snapshot. Shared by `tuo build`
/// (which loads files) and the package-aware `tuo build` (which loads a
/// resolved package graph). `names` labels the inputs in machine output.
fn drive_loaded(
    map: &SourceMap,
    sources: &[SourceId],
    names: &[PathBuf],
    output: Option<PathBuf>,
    backend: Backend,
    kind: Mode,
    mode: OutputMode,
) -> ExitCode {
    let command = kind.command();
    match compile_and_finish(map, sources, names, output, backend, kind, mode) {
        Ok(code) => code,
        Err(outcome) => report_failure(map, names, command, mode, outcome),
    }
}

/// The package-aware `tuo build`: compile an already-resolved package graph to
/// a native executable. `release` selects the LLVM backend as usual.
pub(crate) fn build_loaded(
    map: &SourceMap,
    sources: &[SourceId],
    names: &[PathBuf],
    output: Option<PathBuf>,
    release: bool,
    mode: OutputMode,
) -> ExitCode {
    drive_loaded(
        map,
        sources,
        names,
        output,
        Backend::select(release),
        Mode::Build,
        mode,
    )
}

/// A failure carrying enough to report through either output mode.
enum Failure {
    /// The front end rejected the program; carries its diagnostics.
    FrontEnd(Vec<diagnostics::Diagnostic>),
    /// A backend/link/run step failed; carries a category and message.
    Step {
        stage: &'static str,
        message: String,
        unsupported: bool,
    },
}

/// Run the whole pipeline; on success return the process exit code to use.
fn compile_and_finish(
    map: &SourceMap,
    sources: &[SourceId],
    files: &[PathBuf],
    output: Option<PathBuf>,
    backend: Backend,
    kind: Mode,
    mode: OutputMode,
) -> Result<ExitCode, Failure> {
    let command = kind.command();
    let mut emitter = mode.emitter(command);
    emit(&mut emitter, mode, &Event::started(&display_all(files)));

    // Front end → verified MIR. The check result owns the real `TypeckResult`
    // handed to the backend, so it stays alive for the whole compile.
    emit_progress(&mut emitter, mode, "checking", "running the front end");
    let check = tuo_compiler::check_sources(map, sources);
    if check.has_errors() {
        return Err(Failure::FrontEnd(check.diagnostics));
    }
    // Re-parse to re-derive the ASTs MIR lowering needs (the same pattern the
    // `debug mir` tool uses); the trees live for the backend call below.
    let parses: Vec<_> = sources
        .iter()
        .map(|&id| parser::parse(map.source(id)))
        .collect();
    let asts: Vec<Ast<'_>> = parses
        .iter()
        .zip(sources)
        .map(|(parse, &id)| Ast::new(&parse.tree, map.source(id).text()))
        .collect();
    let lowered_hir = hir::lower(&asts, &check.resolution);
    let mut program = mir::lower(&lowered_hir, &check.resolution, &check.types);
    // A backend must never consume unverified MIR; refuse if lowering somehow
    // produced malformed MIR (a compiler bug, not a user error).
    if !mir::verify(&program, &check.types).is_empty() {
        return Err(Failure::Step {
            stage: "codegen",
            message: "internal: lowered MIR failed verification".to_owned(),
            unsupported: false,
        });
    }

    // Optimize the verified MIR before handing it to the backend. Every pass
    // is a meaning-preserving rewrite (the interpreter's result on the
    // *unoptimized* MIR is the reference the differential suites pin against),
    // and the driver re-verifies after each pass, so `program` remains
    // verified MIR a backend may consume. Both backends run over the optimized
    // MIR; the release backend layers LLVM's own optimizer on top.
    emit_progress(
        &mut emitter,
        mode,
        "optimizing",
        "running MIR optimization passes",
    );
    let _opt = mir::optimize(&mut program, &check.types);

    // Backend → object. The backend receives the real, verified MIR and the
    // real type-check result — no leaked placeholders.
    emit_progress(
        &mut emitter,
        mode,
        "codegen",
        &format!("generating native code ({} backend)", backend.name()),
    );
    let target = TargetSpec::host();
    let artifact = backend
        .compile(&program, &check.types, &target)
        .map_err(codegen_failure)?;

    // Link object + runtime → executable.
    emit_progress(&mut emitter, mode, "linking", "linking the executable");
    let exe_path = executable_path(files, output.as_deref(), kind);
    link(&artifact, &exe_path).map_err(|message| Failure::Step {
        stage: "linking",
        message,
        unsupported: false,
    })?;

    match kind {
        Mode::Build => {
            let summary = json!({
                "artifact": exe_path.display().to_string(),
                "backend": backend.name(),
            });
            emit_finished(&mut emitter, mode, Status::Ok, summary);
            report_build_human(mode, &exe_path);
            Ok(ExitCode::SUCCESS)
        }
        Mode::Run => {
            emit_progress(&mut emitter, mode, "running", "running the program");
            let status = Command::new(&exe_path)
                .status()
                .map_err(|error| Failure::Step {
                    stage: "running",
                    message: format!("could not run the built executable: {error}"),
                    unsupported: false,
                })?;
            let code = status.code().unwrap_or_else(|| signal_exit_code(&status));
            // The tooling operation *succeeded*: the program was built, linked,
            // and ran to completion. The program's own result is data in the
            // summary (`exit_status`), not a tooling error — a program that
            // deliberately returns a non-zero code is not a `run` failure. A
            // trap is the exception: it is an abnormal termination, so the run
            // is reported as an error.
            let trapped = code == tuo_runtime::TRAP_EXIT_STATUS;
            let summary = json!({
                "exit_status": code,
                "trapped": trapped,
                "backend": backend.name(),
            });
            let overall = if trapped { Status::Error } else { Status::Ok };
            emit_finished(&mut emitter, mode, overall, summary);
            // Clean up the temporary executable; ignore removal errors.
            let _ = std::fs::remove_file(&exe_path);
            Ok(exit_code_from(code))
        }
    }
}

/// The outcome of a self-contained native compile-link-run (used by the corpus
/// validator's [`NativeExecutor`] seam). Unlike the `build`/`run` commands, this
/// helper emits no protocol events and prints nothing; it just reports what
/// happened so a caller can record it.
pub(crate) enum NativeRunResult {
    /// The program compiled, linked, ran, and terminated with this exit status.
    Ran { exit_status: i32 },
    /// A compile, link, or run step failed.
    Failed { reason: String },
}

/// Compile the already-loaded program to native code with the default
/// (Cranelift) backend, link it with the runtime, run the produced binary in a
/// temporary location, and report the outcome. Never panics and never prints:
/// every failure is a [`NativeRunResult::Failed`].
///
/// This is the machinery the corpus pipeline cannot perform on its own (it needs
/// a concrete backend and `cc`), exposed as a reusable helper so the CLI can
/// inject it as the corpus validator's native-execution seam.
pub(crate) fn native_run(map: &SourceMap, sources: &[SourceId]) -> NativeRunResult {
    // Front end → verified, optimized MIR (the same path `build`/`run` take,
    // without the protocol emission).
    let check = tuo_compiler::check_sources(map, sources);
    if check.has_errors() {
        return NativeRunResult::Failed {
            reason: "the program does not pass the front end".to_owned(),
        };
    }
    let parses: Vec<_> = sources
        .iter()
        .map(|&id| parser::parse(map.source(id)))
        .collect();
    let asts: Vec<Ast<'_>> = parses
        .iter()
        .zip(sources)
        .map(|(parse, &id)| Ast::new(&parse.tree, map.source(id).text()))
        .collect();
    let lowered_hir = hir::lower(&asts, &check.resolution);
    let mut program = mir::lower(&lowered_hir, &check.resolution, &check.types);
    if !mir::verify(&program, &check.types).is_empty() {
        return NativeRunResult::Failed {
            reason: "lowered MIR failed verification".to_owned(),
        };
    }
    let _opt = mir::optimize(&mut program, &check.types);

    let target = TargetSpec::host();
    let artifact = match Backend::Cranelift.compile(&program, &check.types, &target) {
        Ok(artifact) => artifact,
        Err(error) => {
            return NativeRunResult::Failed {
                reason: format!("codegen: {}", error.message),
            };
        }
    };

    // Link + run in a unique temp path so concurrent validations don't collide.
    let mut exe_path = std::env::temp_dir();
    exe_path.push(format!("tuo-corpus-{}", std::process::id()));
    if let Err(message) = link(&artifact, &exe_path) {
        return NativeRunResult::Failed {
            reason: format!("linking: {message}"),
        };
    }
    let status = Command::new(&exe_path).status();
    let _ = std::fs::remove_file(&exe_path);
    match status {
        Ok(status) => {
            let code = status.code().unwrap_or_else(|| signal_exit_code(&status));
            NativeRunResult::Ran { exit_status: code }
        }
        Err(error) => NativeRunResult::Failed {
            reason: format!("running the built executable: {error}"),
        },
    }
}

/// Map a [`CodegenError`] to a reportable failure, preserving "unsupported".
fn codegen_failure(error: CodegenError) -> Failure {
    let unsupported = error.is_unsupported();
    Failure::Step {
        stage: "codegen",
        message: error.message,
        unsupported,
    }
}

/// Link the backend's object together with the runtime trap shim, the runtime
/// effect shim, the runtime allocator shim, and the runtime map shim into an
/// executable at `exe_path`, using the platform `cc`. The effect shim
/// (ADR-0006 Stage B: `tuo_rt_write`/`tuo_rt_read_byte`/`tuo_rt_exit`), the
/// allocator shim (ADR-0009 Stage B: `tuo_rt_alloc`/`tuo_rt_dealloc`), and the
/// map shim (ADR-0011 Stage B: the `tuo_rt_map_*` table) are linked
/// unconditionally, like `-lm` — harmless for a program that never allocates or
/// performs an effect, required the moment one builds a `String`/`Array`/`Map`
/// or writes to a descriptor.
fn link(artifact: &ObjectArtifact, exe_path: &Path) -> Result<(), String> {
    let dir = exe_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let stem = exe_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("tuo_program");

    // Write the object next to the target executable.
    let object_path = dir.join(format!("{stem}.tuo.o"));
    std::fs::write(&object_path, &artifact.bytes)
        .map_err(|error| format!("writing the object file: {error}"))?;

    // Write the runtime trap and effect shims as C, so the linker resolves
    // the runtime symbols without dragging in a Rust runtime.
    let runtime_c = dir.join(format!("{stem}.tuo_rt.c"));
    std::fs::write(&runtime_c, tuo_runtime::trap_runtime_c_source())
        .map_err(|error| format!("writing the runtime shim: {error}"))?;
    let effect_c = dir.join(format!("{stem}.tuo_rt_effect.c"));
    std::fs::write(&effect_c, tuo_runtime::effect::effect_runtime_c_source())
        .map_err(|error| format!("writing the effect runtime shim: {error}"))?;
    let alloc_c = dir.join(format!("{stem}.tuo_rt_alloc.c"));
    std::fs::write(&alloc_c, tuo_runtime::alloc::alloc_runtime_c_source())
        .map_err(|error| format!("writing the allocator runtime shim: {error}"))?;
    let map_c = dir.join(format!("{stem}.tuo_rt_map.c"));
    std::fs::write(&map_c, tuo_runtime::map::map_runtime_c_source())
        .map_err(|error| format!("writing the map runtime shim: {error}"))?;

    // `cc object runtime.c effect.c alloc.c -lm -o exe` — the platform driver
    // picks the linker and the correct startup files, so the produced binary
    // has a real `main` entry point and a working C ABI on every supported
    // host. `-lm` resolves the C math library's `fmod`/`fmodf`, which the
    // Cranelift backend calls for float remainder (Cranelift has no `frem`
    // instruction) — harmless on macOS (libm is part of libSystem), required on
    // Linux.
    let status = Command::new("cc")
        .arg(&object_path)
        .arg(&runtime_c)
        .arg(&effect_c)
        .arg(&alloc_c)
        .arg(&map_c)
        .arg("-lm")
        .arg("-pthread")
        .arg("-o")
        .arg(exe_path)
        .status()
        .map_err(|error| format!("could not launch the linker (cc): {error}"))?;

    // Clean up the intermediates regardless of link outcome.
    let _ = std::fs::remove_file(&object_path);
    let _ = std::fs::remove_file(&runtime_c);
    let _ = std::fs::remove_file(&effect_c);
    let _ = std::fs::remove_file(&alloc_c);
    let _ = std::fs::remove_file(&map_c);

    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "the linker (cc) failed with status {}",
            status.code().unwrap_or(-1)
        ))
    }
}

/// Where to write the built executable.
///
/// `build` honors `-o`, else names it after the first input's stem (dropping
/// the `.tuo` extension) in the current directory. `run` always builds to a
/// unique temporary path.
fn executable_path(files: &[PathBuf], output: Option<&Path>, kind: Mode) -> PathBuf {
    if let (Mode::Build, Some(path)) = (kind, output) {
        return path.to_path_buf();
    }
    let stem = files
        .first()
        .and_then(|p| p.file_stem())
        .and_then(|s| s.to_str())
        .unwrap_or("a")
        .to_owned();
    match kind {
        Mode::Build => PathBuf::from(stem),
        Mode::Run => {
            // A per-process temp path in the system temp dir; the pid keeps
            // concurrent runs from colliding.
            let mut path = std::env::temp_dir();
            path.push(format!("tuo-run-{}-{stem}", std::process::id()));
            path
        }
    }
}

/// Convert an exit code integer to an [`ExitCode`], clamping to a byte (the
/// portable exit-status width).
fn exit_code_from(code: i32) -> ExitCode {
    ExitCode::from((code & 0xff) as u8)
}

/// A synthetic exit code for a process killed by a signal (no natural
/// `status.code()`), following the `128 + signal` Unix convention where
/// available; falls back to a generic failure otherwise.
fn signal_exit_code(status: &std::process::ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    let _ = status;
    1
}

// ---- loading & reporting ----

/// Load `files` into one program snapshot, reporting a read error through
/// `mode`.
#[expect(
    clippy::print_stderr,
    reason = "this is the CLI presentation layer: in human mode read errors go to stderr"
)]
fn load(
    files: &[PathBuf],
    command: ProtocolCommand,
    mode: OutputMode,
) -> Result<(SourceMap, Vec<SourceId>), ExitCode> {
    let mut map = SourceMap::new();
    let mut sources = Vec::new();
    for path in files {
        let display = path.display().to_string();
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => {
                if mode.is_machine() {
                    protocol::io_error(command, mode, &display, &error.to_string());
                } else {
                    eprintln!("error: cannot read {display}: {error}");
                }
                return Err(ExitCode::FAILURE);
            }
        };
        let file = map.intern_file(&display);
        match map.add_source(file, text.as_str()) {
            Ok(id) => sources.push(id),
            Err(error) => {
                if mode.is_machine() {
                    protocol::io_error(command, mode, &display, &error.to_string());
                } else {
                    eprintln!("error: {display}: {error}");
                }
                return Err(ExitCode::FAILURE);
            }
        }
    }
    Ok((map, sources))
}

/// Report a failure through the active output mode and return a failure exit.
fn report_failure(
    map: &SourceMap,
    files: &[PathBuf],
    command: ProtocolCommand,
    mode: OutputMode,
    failure: Failure,
) -> ExitCode {
    if mode.is_machine() {
        report_failure_machine(map, files, command, mode, &failure);
    } else {
        report_failure_human(map, &failure);
    }
    ExitCode::FAILURE
}

/// Machine mode: emit a well-formed, terminated error stream.
fn report_failure_machine(
    map: &SourceMap,
    files: &[PathBuf],
    command: ProtocolCommand,
    mode: OutputMode,
    failure: &Failure,
) {
    let Some(mut emitter) = mode.emitter(command) else {
        return;
    };
    let write = (|| -> std::io::Result<()> {
        emitter.emit(&Event::started(&display_all(files)))?;
        let summary = match failure {
            Failure::FrontEnd(problems) => {
                for problem in problems {
                    emitter.emit(&Event::diagnostic(problem, map))?;
                }
                json!({ "reason": "front-end errors" })
            }
            Failure::Step {
                stage,
                message,
                unsupported,
            } => json!({ "stage": stage, "message": message, "unsupported": unsupported }),
        };
        emitter.emit(&Event::finished(Status::Error, summary))?;
        emitter.finish()
    })();
    if write.is_err() {
        mode.log("protocol: stdout write failed");
    }
}

/// Human mode: render the failure to stderr.
#[expect(
    clippy::print_stderr,
    reason = "this is the CLI presentation layer: diagnostics and errors go to stderr"
)]
fn report_failure_human(map: &SourceMap, failure: &Failure) {
    match failure {
        Failure::FrontEnd(problems) => {
            if !problems.is_empty() {
                eprint!("{}", diagnostics::render::render_all(problems, map));
            }
            eprintln!("error: cannot build: the program has front-end errors");
        }
        Failure::Step {
            stage,
            message,
            unsupported,
        } => {
            if *unsupported {
                eprintln!(
                    "error: cannot build ({stage}): {message}\n\
                     note: this program is outside the native backend's current subset; \
                     `tuo spec`/`tuo verify` can still execute it on the reference interpreter"
                );
            } else {
                eprintln!("error: {stage}: {message}");
            }
        }
    }
}

/// Human mode: announce a successful build.
#[expect(
    clippy::print_stderr,
    reason = "this is the CLI presentation layer: status goes to stderr"
)]
fn report_build_human(mode: OutputMode, exe_path: &Path) {
    if !mode.is_machine() {
        eprintln!("built {}", exe_path.display());
    }
}

// ---- protocol emission helpers ----

/// Emit an event through the optional emitter, logging a write failure.
fn emit(emitter: &mut Option<Emitter<std::io::Stdout>>, mode: OutputMode, event: &Event) {
    if let Some(emitter) = emitter {
        if emitter.emit(event).is_err() {
            mode.log("protocol: stdout write failed");
        }
    }
}

/// Emit a `progress` event (machine mode only).
fn emit_progress(
    emitter: &mut Option<Emitter<std::io::Stdout>>,
    mode: OutputMode,
    stage: &str,
    message: &str,
) {
    emit(emitter, mode, &Event::progress(stage, message));
}

/// Emit the terminal `finished` event and flush the stream.
fn emit_finished(
    emitter: &mut Option<Emitter<std::io::Stdout>>,
    mode: OutputMode,
    status: Status,
    summary: serde_json::Value,
) {
    if let Some(emitter) = emitter {
        let write = (|| -> std::io::Result<()> {
            emitter.emit(&Event::finished(status, summary))?;
            emitter.finish()
        })();
        if write.is_err() {
            mode.log("protocol: stdout write failed");
        }
    }
}

/// The display strings of every input path.
fn display_all(files: &[PathBuf]) -> Vec<String> {
    files.iter().map(|p| p.display().to_string()).collect()
}
