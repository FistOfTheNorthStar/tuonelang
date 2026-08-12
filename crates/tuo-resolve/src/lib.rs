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
//!   and non-function spec targets (`R0006`). (`R0007` — a spec reaching an
//!   effectful function — belongs to the same spec-semantics code group but
//!   is computed by the type-checking stage, which owns the effect
//!   discipline; see `specification/static-semantics.md` §2.3, §3.6.)
//!
//! Alongside the prelude, resolution installs the **builtin functions**
//! of ADR-0006 and ADR-0009 ([`Builtin`]) as real symbols in the
//! always-present `std::rt` / `std::str` / `std::string` / `std::array`
//! modules, so `std::rt::write(1, "x")` and `std::string::concat("a", "b")`
//! resolve in every program with no stdlib loaded ([`Resolution::builtin`]).
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
//! This crate resolves directly over the typed AST views. The HIR layer
//! (`tuo-hir`) sits *above* resolution and consumes its output — lowering
//! combines the AST with this crate's stable IDs so HIR nodes carry
//! resolved names.

mod builtin;
mod ids;
mod resolution;
mod resolver;
mod symbol;

pub use builtin::{Builtin, BuiltinParamMode};
pub use ids::{ModuleId, SymbolId};
pub use resolution::{ModuleInfo, Resolution};
pub use resolver::{is_builtin_type, resolve};
pub use symbol::{Reference, SpecTarget, Symbol, SymbolKind};
