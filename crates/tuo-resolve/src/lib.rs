//! Name resolution for tuonelang.
//!
//! [`resolve`] takes one program snapshot — every parsed file, viewed
//! through the typed AST ([`tuo_ast`]) — and produces a [`Resolution`]:
//!
//! - a **symbol table** of every declared entity (modules, functions,
//!   structs, enums and their variants, interfaces, constants, specs,
//!   type parameters, parameters, locals), each with a stable semantic
//!   [`SymbolId`] rather than an AST pointer;
//! - a **reference map** from every resolved name occurrence (exact source
//!   span) to its symbol — the substrate for rename, go-to-definition, and
//!   find-all-references ([`Resolution::rename_spans`]);
//! - **spec attachments**: an identifier-named `spec fibonacci { … }`
//!   resolves to the *same* function symbol that calls to `fibonacci`
//!   resolve to ([`Resolution::spec_targets`]);
//! - structured diagnostics in the reserved `Rxxxx` namespace: duplicate
//!   definitions (`R0001`), undefined names (`R0002`), unresolved imports
//!   (`R0003`), ambiguous names (`R0004`), visibility violations (`R0005`),
//!   and non-function spec targets (`R0006`).
//!
//! Module-level declarations are collected before any use is resolved, so
//! **forward references** across items and files always work. Lexical
//! scoping is sequential inside bodies: a `let` shadows from its binding
//! onward, and an initializer still sees the outer name.
//!
//! **No type information lives here.** Names whose meaning depends on types
//! — field accesses, method calls, `Self`, associated path tails — are
//! deliberately skipped, never guessed at; the type checker owns them.
//!
//! This crate currently resolves directly over the typed AST views. When
//! the HIR layer (`tuo-hir`) is implemented, resolution will move to
//! consuming it; the stable-ID surface here is designed to survive that
//! move unchanged.

mod ids;
mod resolution;
mod resolver;
mod symbol;

pub use ids::{ModuleId, SymbolId};
pub use resolution::{ModuleInfo, Resolution};
pub use resolver::resolve;
pub use symbol::{Reference, SpecTarget, Symbol, SymbolKind};
