//! Match exhaustiveness (`T0007`): do a match's unguarded arms cover every
//! value of the scrutinee's type?
//!
//! Each arm's pattern is reduced to a [`Pat`] — a wildcard, a constructor
//! with its field sub-patterns, a literal of an unbounded type, or a set of
//! alternatives — and the classic *usefulness* recursion (Maranget,
//! "Warnings for pattern matching", 2007) asks whether a wildcard would
//! still match a value no arm matches. Unlike the shallow check this
//! replaced, a constructor counts as covered only when its fields are
//! covered too: `Some { value: 0 } => …, None => …` misses every other
//! `Some`, and a struct pattern binding each field covers the whole struct.
//!
//! The recursion enumerates a column's constructors only when the arms use
//! every one of them; otherwise the rows that match anything (the default
//! matrix) decide. So it only descends where patterns do, and terminates on
//! recursive types.

use tuo_ast::Pattern;
use tuo_diagnostics::{Diagnostic, StructuredValue};
use tuo_lexer::TokenKind;
use tuo_resolve::{SymbolId, SymbolKind};
use tuo_source::Span;

use super::{Checker, code, substitute, substitution};
use crate::ty::Ty;

/// A pattern reduced to what coverage needs.
#[derive(Clone, Debug)]
enum Pat {
    /// Matches every value: `_`, a fresh binding, or a pattern the checker
    /// could not type (already reported, so it must not cascade).
    Wild,
    /// A constructor and its field sub-patterns, in declaration order.
    Ctor(Ctor, Vec<Pat>),
    /// A literal of a type with unboundedly many values (integers, floats,
    /// characters, strings): it never completes a column on its own.
    Lit,
    /// Alternatives (`A | B`).
    Or(Vec<Pat>),
}

/// One constructor of a type with finitely many.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ctor {
    /// A struct (its only constructor) or an enum, `Option`, or `Result`
    /// variant.
    Symbol(SymbolId),
    /// `true` or `false`.
    Bool(bool),
    /// `()`.
    Unit,
}

type Row = Vec<Pat>;

impl Checker<'_> {
    /// Report `T0007` unless `patterns` (the unguarded arms, in order)
    /// cover every value of `scrutinee`.
    pub(super) fn check_exhaustiveness(
        &mut self,
        scrutinee: &Ty,
        patterns: &[Pattern<'_>],
        span: Span,
    ) {
        let applied = self.icx.apply(scrutinee);
        if matches!(applied, Ty::Error | Ty::Var(_) | Ty::Never) {
            return;
        }
        let rows: Vec<Row> = patterns
            .iter()
            .map(|pattern| vec![self.reduce(pattern, &applied)])
            .collect();
        let rows = expand_or(&rows);
        let Some(ctors) = self.ctors_of(&applied) else {
            if self.useful(&rows, &[Pat::Wild], std::slice::from_ref(&applied)) {
                self.non_exhaustive(&applied, span);
            }
            return;
        };
        // A constructor is missing when some value built with it is not
        // matched; `partial` ones are written in an arm, but not for every
        // value of their fields.
        let mut missing = Vec::new();
        let mut partial = Vec::new();
        for ctor in ctors {
            let fields = self.ctor_field_tys(ctor, &applied);
            let specialized = specialize(&rows, ctor, fields.len());
            if self.useful(&specialized, &vec![Pat::Wild; fields.len()], &fields) {
                let name = self.ctor_name(ctor);
                if rows
                    .iter()
                    .any(|row| matches!(&row[0], Pat::Ctor(c, _) if *c == ctor))
                {
                    partial.push(name.clone());
                }
                missing.push(name);
            }
        }
        if missing.is_empty() {
            return;
        }
        if matches!(applied, Ty::Struct(..)) {
            // A struct has one constructor, so "missing `Point`" would only
            // repeat the type; say which values are left instead.
            self.non_exhaustive(&applied, span);
            return;
        }
        let rendered = self.render(&applied);
        let mut diagnostic = Diagnostic::error(
            code(7),
            format!(
                "non-exhaustive match on `{rendered}`: missing {}",
                missing
                    .iter()
                    .map(|name| format!("`{name}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            span,
        )
        .with_help("cover every variant, or add a `_` arm");
        if !partial.is_empty() {
            diagnostic = diagnostic.with_note(format!(
                "{} {} matched only for some of {} field values",
                partial
                    .iter()
                    .map(|name| format!("`{name}`"))
                    .collect::<Vec<_>>()
                    .join(", "),
                if partial.len() == 1 { "is" } else { "are" },
                if partial.len() == 1 { "its" } else { "their" },
            ));
        }
        for name in missing {
            diagnostic = diagnostic.with_expected(StructuredValue::Name(name));
        }
        self.push(diagnostic);
    }

    /// `T0007` for a `let` whose pattern can fail to match (`let Some {
    /// value } = o;`, `let P { x: 0, y } = p;`): a binding has no other arm
    /// to fall to. Before this check such a `let` was accepted and lowering
    /// dropped the function.
    pub(super) fn check_irrefutable(&mut self, ty: &Ty, pattern: &Pattern<'_>, span: Span) {
        let applied = self.icx.apply(ty);
        if matches!(applied, Ty::Error | Ty::Var(_) | Ty::Never) {
            return;
        }
        let rows = expand_or(&[vec![self.reduce(pattern, &applied)]]);
        if !self.useful(&rows, &[Pat::Wild], std::slice::from_ref(&applied)) {
            return;
        }
        let rendered = self.render(&applied);
        self.push(
            Diagnostic::error(
                code(7),
                format!("`let` pattern does not match every `{rendered}`"),
                span,
            )
            .with_primary_label("some values of this type are not matched")
            .with_help(
                "a `let` has nowhere to go when its pattern fails; use a `match` to handle \
                 the other values",
            ),
        );
    }

    /// The `T0007` error for a type whose leftover values have no short
    /// name (an integer, a string, a partly matched struct).
    fn non_exhaustive(&mut self, ty: &Ty, span: Span) {
        let rendered = self.render(ty);
        self.push(
            Diagnostic::error(
                code(7),
                format!("non-exhaustive match on `{rendered}`"),
                span,
            )
            .with_primary_label("not all values are covered")
            .with_help("add a `_` arm to cover the remaining values"),
        );
    }

    /// Whether `vector` matches a value that no row of `rows` matches, the
    /// columns having types `tys`.
    fn useful(&self, rows: &[Row], vector: &[Pat], tys: &[Ty]) -> bool {
        let (Some((head, rest)), Some((ty, rest_tys))) = (vector.split_first(), tys.split_first())
        else {
            return rows.is_empty();
        };
        if rows.is_empty() {
            return true;
        }
        let rows = expand_or(rows);
        let continue_with = |front: Vec<Pat>, front_tys: Vec<Ty>, matrix: &[Row]| {
            let mut next = front;
            next.extend_from_slice(rest);
            let mut next_tys = front_tys;
            next_tys.extend_from_slice(rest_tys);
            self.useful(matrix, &next, &next_tys)
        };
        match head {
            Pat::Or(alternatives) => alternatives.iter().any(|alternative| {
                continue_with(vec![alternative.clone()], vec![ty.clone()], &rows)
            }),
            Pat::Ctor(ctor, args) => {
                let fields = self.ctor_field_tys(*ctor, ty);
                let specialized = specialize(&rows, *ctor, args.len());
                continue_with(args.clone(), fields, &specialized)
            }
            Pat::Wild | Pat::Lit => {
                let ctors = self.ctors_of(ty);
                let complete = ctors.as_ref().is_some_and(|ctors| {
                    ctors.iter().all(|ctor| {
                        rows.iter()
                            .any(|row| matches!(&row[0], Pat::Ctor(c, _) if c == ctor))
                    })
                });
                if complete {
                    ctors.into_iter().flatten().any(|ctor| {
                        let fields = self.ctor_field_tys(ctor, ty);
                        let specialized = specialize(&rows, ctor, fields.len());
                        continue_with(vec![Pat::Wild; fields.len()], fields, &specialized)
                    })
                } else {
                    // Some constructor (or, for an unbounded type, some
                    // value) appears in no row's head: only the rows that
                    // match anything there can match it.
                    let default: Vec<Row> = rows
                        .iter()
                        .filter(|row| matches!(row[0], Pat::Wild))
                        .map(|row| row[1..].to_vec())
                        .collect();
                    self.useful(&default, rest, rest_tys)
                }
            }
        }
    }

    /// Reduce an arm's pattern, matched against a value of type `ty`.
    fn reduce(&self, pattern: &Pattern<'_>, ty: &Ty) -> Pat {
        match pattern {
            Pattern::Wildcard(_) => Pat::Wild,
            Pattern::Binding(binding) => {
                // Resolution decides whether a lone name is a fresh binding
                // or a payload-less variant (`None`, a bare `Red`).
                if self.declared_at(binding.name_ref()).is_some() {
                    return Pat::Wild;
                }
                match self.symbol_at(binding.name_ref()) {
                    Some(symbol) if self.resolution.symbol(symbol).kind == SymbolKind::Variant => {
                        let arity = self.ctor_field_tys(Ctor::Symbol(symbol), ty).len();
                        Pat::Ctor(Ctor::Symbol(symbol), vec![Pat::Wild; arity])
                    }
                    _ => Pat::Wild,
                }
            }
            Pattern::Literal(literal) => match literal.token_kind() {
                Some(TokenKind::BoolLiteral) => {
                    Pat::Ctor(Ctor::Bool(literal.text() == "true"), Vec::new())
                }
                Some(TokenKind::OpenParen) => Pat::Ctor(Ctor::Unit, Vec::new()),
                _ => Pat::Lit,
            },
            Pattern::Path(path) => {
                let Some(symbol) = self.symbol_at(path.segment_names().last()) else {
                    return Pat::Wild;
                };
                if !matches!(
                    self.resolution.symbol(symbol).kind,
                    SymbolKind::Struct | SymbolKind::Variant
                ) {
                    return Pat::Wild;
                }
                let ctor = Ctor::Symbol(symbol);
                let fields = self.ctor_fields(symbol, ty);
                let mut args = vec![Pat::Wild; fields.len()];
                for field in path.fields() {
                    let Some(name) = field.name_ref() else {
                        continue;
                    };
                    let Some(index) = fields
                        .iter()
                        .position(|(field_name, _)| field_name == name.text)
                    else {
                        continue;
                    };
                    // Shorthand (`{ value }`) binds the field: a wildcard.
                    if let Some(sub) = field.pattern() {
                        args[index] = self.reduce(&sub, &fields[index].1);
                    }
                }
                Pat::Ctor(ctor, args)
            }
            Pattern::Or(or) => Pat::Or(
                or.alternatives()
                    .map(|alternative| self.reduce(&alternative, ty))
                    .collect(),
            ),
            Pattern::Group(group) => group
                .inner()
                .map_or(Pat::Wild, |inner| self.reduce(&inner, ty)),
        }
    }

    /// Every constructor of `ty`, or `None` when its values are unbounded
    /// (or not known well enough to enumerate).
    fn ctors_of(&self, ty: &Ty) -> Option<Vec<Ctor>> {
        let prelude = |names: &[&str]| -> Option<Vec<Ctor>> {
            names
                .iter()
                .map(|name| self.prelude(name).map(Ctor::Symbol))
                .collect()
        };
        match self.icx.apply(ty) {
            Ty::Bool => Some(vec![Ctor::Bool(true), Ctor::Bool(false)]),
            Ty::Unit => Some(vec![Ctor::Unit]),
            Ty::Option(_) => prelude(&["Some", "None"]),
            Ty::Result(..) => prelude(&["Ok", "Err"]),
            Ty::Enum(symbol, _) => Some(
                self.resolution
                    .variants_of(symbol)
                    .iter()
                    .copied()
                    .map(Ctor::Symbol)
                    .collect(),
            ),
            Ty::Struct(symbol, _) => Some(vec![Ctor::Symbol(symbol)]),
            _ => None,
        }
    }

    /// The field types of `ctor` building a value of type `ty`.
    fn ctor_field_tys(&self, ctor: Ctor, ty: &Ty) -> Vec<Ty> {
        match ctor {
            Ctor::Symbol(symbol) => self
                .ctor_fields(symbol, ty)
                .into_iter()
                .map(|(_, ty)| ty)
                .collect(),
            Ctor::Bool(_) | Ctor::Unit => Vec::new(),
        }
    }

    /// The fields (name and type, in declaration order) of the struct or
    /// variant `symbol` building a value of type `ty`, instantiated at
    /// `ty`'s type arguments.
    fn ctor_fields(&self, symbol: SymbolId, ty: &Ty) -> Vec<(String, Ty)> {
        let ty = self.icx.apply(ty);
        let prelude = |name: &str| self.prelude(name) == Some(symbol);
        match &ty {
            Ty::Option(value) if prelude("Some") => {
                return vec![("value".to_owned(), (**value).clone())];
            }
            Ty::Result(ok, _) if prelude("Ok") => {
                return vec![("value".to_owned(), (**ok).clone())];
            }
            Ty::Result(_, err) if prelude("Err") => {
                return vec![("error".to_owned(), (**err).clone())];
            }
            _ => {}
        }
        if prelude("None") {
            return Vec::new();
        }
        let args: &[Ty] = match &ty {
            Ty::Struct(_, args) | Ty::Enum(_, args) => args,
            _ => &[],
        };
        let (params, fields) = if let Some(def) = self.structs.get(&symbol) {
            (&def.type_params, def.fields.clone())
        } else if let Some(def) = self
            .variant_owner
            .get(&symbol)
            .and_then(|owner| self.enums.get(owner))
        {
            let fields = def
                .variants
                .iter()
                .find(|(variant, _)| *variant == symbol)
                .map(|(_, fields)| fields.clone())
                .unwrap_or_default();
            (&def.type_params, fields)
        } else {
            return Vec::new();
        };
        let map = substitution(params, args);
        fields
            .into_iter()
            .map(|(name, ty)| (name, substitute(&ty, &map)))
            .collect()
    }

    fn ctor_name(&self, ctor: Ctor) -> String {
        match ctor {
            Ctor::Symbol(symbol) => self.resolution.symbol(symbol).name.clone(),
            Ctor::Bool(value) => value.to_string(),
            Ctor::Unit => "()".to_owned(),
        }
    }
}

/// `rows` with every leading or-pattern split into one row per alternative.
fn expand_or(rows: &[Row]) -> Vec<Row> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        match row.first() {
            Some(Pat::Or(alternatives)) => {
                let split: Vec<Row> = alternatives
                    .iter()
                    .map(|alternative| {
                        let mut next = vec![alternative.clone()];
                        next.extend_from_slice(&row[1..]);
                        next
                    })
                    .collect();
                out.extend(expand_or(&split));
            }
            _ => out.push(row.clone()),
        }
    }
    out
}

/// The rows that can match a value built with `ctor`, its `arity` field
/// sub-patterns replacing the head.
fn specialize(rows: &[Row], ctor: Ctor, arity: usize) -> Vec<Row> {
    rows.iter()
        .filter_map(|row| {
            let mut next = match &row[0] {
                // A mismatched arity is a pattern already reported as a type
                // error; it must not cascade (nor index past a row).
                Pat::Ctor(c, args) if *c == ctor && args.len() == arity => args.clone(),
                Pat::Ctor(c, _) if *c == ctor => vec![Pat::Wild; arity],
                Pat::Wild => vec![Pat::Wild; arity],
                Pat::Ctor(..) | Pat::Lit | Pat::Or(_) => return None,
            };
            next.extend_from_slice(&row[1..]);
            Some(next)
        })
        .collect()
}
