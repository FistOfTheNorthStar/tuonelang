//! Front-end orchestration: parse, resolve, type-check, and
//! ownership-check one program snapshot — the pipeline behind `tuo check`.
//!
//! Specs are first-class here: they are parsed, resolved (attachment and
//! dependency discovery), type-checked, and ownership-checked like any
//! other item, per ADR-0002. They are **not** executed — spec execution
//! arrives with the MIR interpreter.

use tuo_ast::Ast;
use tuo_diagnostics::{Diagnostic, Severity};
use tuo_ownership::OwnershipResult;
use tuo_resolve::Resolution;
use tuo_source::{SourceId, SourceMap};
use tuo_types::TypeckResult;

/// Everything the front end produced for one program snapshot.
#[derive(Debug)]
pub struct CheckResult {
    /// Name resolution: symbols, references, spec attachments and
    /// dependencies.
    pub resolution: Resolution,
    /// Type checking: symbol types and type diagnostics.
    pub types: TypeckResult,
    /// Ownership checking: the `O0001`–`O0009` diagnostics of the v0
    /// ownership model (ADR-0003).
    pub ownership: OwnershipResult,
    /// Every diagnostic the pipeline produced, in stage order (parse,
    /// then resolution, then types, then ownership).
    pub diagnostics: Vec<Diagnostic>,
}

impl CheckResult {
    /// Does any diagnostic reject the program?
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }
}

/// Run the front end — parse, resolve, type-check, ownership-check — over
/// `sources` (one program snapshot drawn from `map`).
#[must_use]
pub fn check_sources(map: &SourceMap, sources: &[SourceId]) -> CheckResult {
    let parses: Vec<tuo_parser::ParseResult> = sources
        .iter()
        .map(|&id| tuo_parser::parse(map.source(id)))
        .collect();
    let mut diagnostics: Vec<Diagnostic> = parses
        .iter()
        .flat_map(tuo_parser::ParseResult::all_diagnostics)
        .collect();
    let asts: Vec<Ast<'_>> = parses
        .iter()
        .zip(sources)
        .map(|(parse, &id)| Ast::new(&parse.tree, map.source(id).text()))
        .collect();
    let resolution = tuo_resolve::resolve(&asts);
    let types = tuo_types::check(&asts, &resolution);
    let ownership = tuo_ownership::check(&asts, &resolution, &types);
    diagnostics.extend_from_slice(resolution.diagnostics());
    diagnostics.extend_from_slice(types.diagnostics());
    diagnostics.extend_from_slice(ownership.diagnostics());
    // The runnable-core advisory (`T0022`): warnings, never errors, so the
    // accepted language is unchanged — see `crate::native_core`. Emitted
    // here rather than inside a stage so the stage crates' own "zero
    // diagnostics" contracts stay exactly as strict as they were.
    diagnostics.extend(crate::native_core::advisories(&asts));
    CheckResult {
        resolution,
        types,
        ownership,
        diagnostics,
    }
}
