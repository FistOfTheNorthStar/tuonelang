//! Unification for local type inference.
//!
//! Inference in tuonelang is deliberately **local**: function signatures
//! are always explicit (Constitution §8), so unification only ever solves
//! for types *within* one body — `let` bindings without annotations,
//! numeric literals, and generic instantiations. Unsolved integer/float
//! literal variables default to `I64`/`F64` (§11) and an unsolved array
//! literal to the fixed array `[elem; len]` (ADR-0040); any other unsolved
//! variable is an error ("type annotation needed"), never a guess.

use tuo_source::Span;

use crate::ty::{FnParam, FnTy, InferVar, Ty};

/// What a fresh variable is allowed to become.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum VarClass {
    /// Any type.
    General,
    /// Some integer type; defaults to `I64`.
    Integer,
    /// Some floating-point type; defaults to `F64`.
    Float,
    /// A list literal `[e1, …, en]` (ADR-0040): the fixed `[elem; len]` or
    /// the growable `Array[elem]`, whichever unification asks for; defaults
    /// to `[elem; len]`.
    ArrayLiteral {
        /// The element type every element was unified against.
        elem: Ty,
        /// The element count.
        len: u64,
    },
}

struct VarData {
    value: Option<Ty>,
    class: VarClass,
    origin: Span,
}

/// A failed unification: the two types that would not meet, fully
/// substituted for reporting.
pub(crate) struct Mismatch {
    pub expected: Ty,
    pub actual: Ty,
}

/// The unification context for one body.
#[derive(Default)]
pub(crate) struct InferCtx {
    vars: Vec<VarData>,
}

impl InferCtx {
    /// A fresh unsolved variable of `class`, remembering `origin` for
    /// "type annotation needed" reporting.
    pub(crate) fn fresh(&mut self, class: VarClass, origin: Span) -> Ty {
        let var = InferVar(u32::try_from(self.vars.len()).expect("var count fits in u32"));
        self.vars.push(VarData {
            value: None,
            class,
            origin,
        });
        Ty::Var(var)
    }

    fn value(&self, var: InferVar) -> Option<&Ty> {
        self.vars[var.0 as usize].value.as_ref()
    }

    /// Follow variable bindings one level.
    fn shallow(&self, ty: &Ty) -> Ty {
        let mut current = ty.clone();
        while let Ty::Var(var) = current {
            match self.value(var) {
                Some(bound) => current = bound.clone(),
                None => return Ty::Var(var),
            }
        }
        current
    }

    /// Substitute every solved variable in `ty`, deeply.
    pub(crate) fn apply(&self, ty: &Ty) -> Ty {
        self.substitute(ty, false)
    }

    /// [`Self::apply`], with every unsolved array literal shown as the
    /// `[elem; len]` it defaults to — the text a diagnostic prints. Never
    /// unify or record the result: it would fix the literal's kind.
    pub(crate) fn display(&self, ty: &Ty) -> Ty {
        self.substitute(ty, true)
    }

    /// [`Self::apply`], with an unsolved array literal at the top shown as
    /// `[elem; len]` and its element left live — what a site that inspects
    /// a type's shape mid-body sees, so it reports exactly what it reported
    /// before ADR-0040 and reads the element without choosing the kind.
    pub(crate) fn view(&self, ty: &Ty) -> Ty {
        match self.array_literal(ty) {
            Some((elem, len)) => Ty::FixedArray(Box::new(self.apply(&elem)), len),
            None => self.apply(ty),
        }
    }

    /// The element type and length of `ty` if it is an unsolved array
    /// literal.
    fn array_literal(&self, ty: &Ty) -> Option<(Ty, u64)> {
        match self.shallow(ty) {
            Ty::Var(var) => match &self.vars[var.0 as usize].class {
                VarClass::ArrayLiteral { elem, len } => Some((elem.clone(), *len)),
                _ => None,
            },
            _ => None,
        }
    }

    fn substitute(&self, ty: &Ty, expand: bool) -> Ty {
        let sub = |item: &Ty| self.substitute(item, expand);
        match self.shallow(ty) {
            Ty::Var(var) => match &self.vars[var.0 as usize].class {
                VarClass::ArrayLiteral { elem, len } if expand => {
                    Ty::FixedArray(Box::new(sub(elem)), *len)
                }
                _ => Ty::Var(var),
            },
            Ty::Tuple(items) => Ty::Tuple(items.iter().map(sub).collect()),
            Ty::Array(item) => Ty::Array(Box::new(sub(&item))),
            Ty::FixedArray(item, n) => Ty::FixedArray(Box::new(sub(&item)), n),
            Ty::Range(item) => Ty::Range(Box::new(sub(&item))),
            Ty::Fn(fn_ty) => Ty::Fn(Box::new(FnTy {
                params: fn_ty
                    .params
                    .iter()
                    .map(|param| FnParam {
                        mode: param.mode,
                        ty: sub(&param.ty),
                    })
                    .collect(),
                ret: sub(&fn_ty.ret),
            })),
            Ty::Option(item) => Ty::Option(Box::new(sub(&item))),
            Ty::Result(ok, err) => Ty::Result(Box::new(sub(&ok)), Box::new(sub(&err))),
            Ty::Map(key, value) => Ty::Map(Box::new(sub(&key)), Box::new(sub(&value))),
            Ty::Struct(symbol, args) => Ty::Struct(symbol, args.iter().map(sub).collect()),
            Ty::Enum(symbol, args) => Ty::Enum(symbol, args.iter().map(sub).collect()),
            Ty::Wrapper(kind, item) => Ty::Wrapper(kind, Box::new(sub(&item))),
            other => other,
        }
    }

    /// Does `var` occur in `ty`? (Prevents building infinite types.)
    fn occurs(&self, var: InferVar, ty: &Ty) -> bool {
        match self.shallow(ty) {
            Ty::Var(other) => {
                other == var
                    || matches!(
                        &self.vars[other.0 as usize].class,
                        VarClass::ArrayLiteral { elem, .. } if self.occurs(var, elem)
                    )
            }
            Ty::Tuple(items) => items.iter().any(|item| self.occurs(var, item)),
            Ty::Array(item)
            | Ty::FixedArray(item, _)
            | Ty::Range(item)
            | Ty::Option(item)
            | Ty::Wrapper(_, item) => self.occurs(var, &item),
            Ty::Fn(fn_ty) => {
                fn_ty.params.iter().any(|param| self.occurs(var, &param.ty))
                    || self.occurs(var, &fn_ty.ret)
            }
            Ty::Result(ok, err) | Ty::Map(ok, err) => {
                self.occurs(var, &ok) || self.occurs(var, &err)
            }
            Ty::Struct(_, args) | Ty::Enum(_, args) => args.iter().any(|arg| self.occurs(var, arg)),
            _ => false,
        }
    }

    /// May a variable of `class` be solved to `ty`? (An array literal is
    /// solved by [`Self::bind_array_literal`], never through here.)
    fn class_admits(class: &VarClass, ty: &Ty) -> bool {
        match class {
            VarClass::General | VarClass::ArrayLiteral { .. } => true,
            VarClass::Integer => matches!(ty, Ty::Int(_) | Ty::Error | Ty::Never),
            VarClass::Float => matches!(ty, Ty::Float(_) | Ty::Error | Ty::Never),
        }
    }

    fn bind(&mut self, var: InferVar, ty: Ty) -> Result<(), Mismatch> {
        if self.occurs(var, &ty) {
            return Err(Mismatch {
                expected: Ty::Var(var),
                actual: self.apply(&ty),
            });
        }
        let class = &self.vars[var.0 as usize].class;
        if !Self::class_admits(class, &ty) {
            return Err(Mismatch {
                expected: match class {
                    VarClass::Integer => Ty::int(),
                    VarClass::Float => Ty::float(),
                    VarClass::General | VarClass::ArrayLiteral { .. } => Ty::Var(var),
                },
                actual: self.apply(&ty),
            });
        }
        self.vars[var.0 as usize].value = Some(ty);
        Ok(())
    }

    /// Solve the unsolved `var` against the non-variable `ty`. Any variable
    /// but an array literal is [`Self::bind`]. For an array literal
    /// (ADR-0040), `[U; len]` fixes it as that fixed array and `Array[U]` as
    /// the growable one, each unifying the element with `U`; any other
    /// length or type is a mismatch. `Error`/`Never` leave it unsolved, so
    /// it keeps the `[elem; len]` default it had before ADR-0040.
    /// `literal_expected` says which side of the unification the literal
    /// was on, so the element and the mismatch keep their orientation.
    fn bind_array_literal(
        &mut self,
        var: InferVar,
        ty: Ty,
        literal_expected: bool,
    ) -> Result<(), Mismatch> {
        let Some((elem, len)) = self.array_literal(&Ty::Var(var)) else {
            return self.bind(var, ty);
        };
        let item = match &ty {
            Ty::Error | Ty::Never => return Ok(()),
            Ty::FixedArray(item, n) if *n == len && !self.occurs(var, &ty) => item,
            Ty::Array(item) if !self.occurs(var, &ty) => item,
            _ => {
                let literal = self.apply(&Ty::Var(var));
                let other = self.apply(&ty);
                return Err(if literal_expected {
                    Mismatch {
                        expected: literal,
                        actual: other,
                    }
                } else {
                    Mismatch {
                        expected: other,
                        actual: literal,
                    }
                });
            }
        };
        if literal_expected {
            self.unify(&elem, item)?;
        } else {
            self.unify(item, &elem)?;
        }
        self.vars[var.0 as usize].value = Some(ty);
        Ok(())
    }

    /// Merge two unsolved variables, keeping the stricter class. Two array
    /// literals merge only at equal lengths, unifying their elements — a
    /// length mismatch is a mismatch, never a reason to pick `Array[T]`.
    fn merge_vars(&mut self, a: InferVar, b: InferVar) -> Result<(), Mismatch> {
        if a == b {
            return Ok(());
        }
        let class_a = self.vars[a.0 as usize].class.clone();
        let class_b = self.vars[b.0 as usize].class.clone();
        let merged = match (class_a, class_b) {
            (
                VarClass::ArrayLiteral {
                    elem: elem_a,
                    len: len_a,
                },
                VarClass::ArrayLiteral {
                    elem: elem_b,
                    len: len_b,
                },
            ) => {
                if len_a != len_b || self.occurs(a, &elem_b) || self.occurs(b, &elem_a) {
                    return Err(Mismatch {
                        expected: self.apply(&Ty::Var(a)),
                        actual: self.apply(&Ty::Var(b)),
                    });
                }
                self.unify(&elem_a, &elem_b)?;
                VarClass::ArrayLiteral {
                    elem: elem_b,
                    len: len_b,
                }
            }
            (VarClass::General, VarClass::ArrayLiteral { elem, .. }) if self.occurs(a, &elem) => {
                return Err(Mismatch {
                    expected: Ty::Var(a),
                    actual: self.apply(&Ty::Var(b)),
                });
            }
            (VarClass::ArrayLiteral { elem, .. }, VarClass::General) if self.occurs(b, &elem) => {
                return Err(Mismatch {
                    expected: Ty::Var(b),
                    actual: self.apply(&Ty::Var(a)),
                });
            }
            (VarClass::General, other) | (other, VarClass::General) => other,
            (left, right) if left == right => left,
            // A number against an array literal reports as a number
            // against the `[elem; len]` it would have been.
            (number @ (VarClass::Integer | VarClass::Float), VarClass::ArrayLiteral { .. }) => {
                return Err(self.number_against_literal(&number, b));
            }
            (VarClass::ArrayLiteral { .. }, number @ (VarClass::Integer | VarClass::Float)) => {
                return Err(self.number_against_literal(&number, a));
            }
            _ => {
                return Err(Mismatch {
                    expected: Ty::int(),
                    actual: Ty::float(),
                });
            }
        };
        self.vars[b.0 as usize].class = merged;
        self.vars[a.0 as usize].value = Some(Ty::Var(b));
        Ok(())
    }

    fn number_against_literal(&self, number: &VarClass, literal: InferVar) -> Mismatch {
        Mismatch {
            expected: if *number == VarClass::Float {
                Ty::float()
            } else {
                Ty::int()
            },
            actual: self.apply(&Ty::Var(literal)),
        }
    }

    /// Unify `expected` with `actual`.
    ///
    /// `Never` unifies with everything (it can flow anywhere); `Error`
    /// unifies with everything (poison is reported once, at its source).
    /// Everything else is exact structural equality — there are **no
    /// implicit conversions**, numeric or otherwise.
    pub(crate) fn unify(&mut self, expected: &Ty, actual: &Ty) -> Result<(), Mismatch> {
        let left = self.shallow(expected);
        let right = self.shallow(actual);
        match (left, right) {
            (Ty::Var(a), Ty::Var(b)) => self.merge_vars(a, b),
            // Binding a variable to `Never`/`Error` is deliberate: the
            // variable *is* diverging/poisoned, and must not later demand
            // an annotation. (An array literal never demands one.)
            (Ty::Var(var), ty) => self.bind_array_literal(var, ty, true),
            (ty, Ty::Var(var)) => self.bind_array_literal(var, ty, false),
            (Ty::Error, _) | (_, Ty::Error) | (Ty::Never, _) | (_, Ty::Never) => Ok(()),
            (Ty::Unit, Ty::Unit)
            | (Ty::Bool, Ty::Bool)
            | (Ty::Char, Ty::Char)
            | (Ty::String, Ty::String)
            | (Ty::Str, Ty::Str) => Ok(()),
            (Ty::Int(a), Ty::Int(b)) if a == b => Ok(()),
            (Ty::Float(a), Ty::Float(b)) if a == b => Ok(()),
            (Ty::Param(a), Ty::Param(b)) if a == b => Ok(()),
            (Ty::Tuple(a), Ty::Tuple(b)) if a.len() == b.len() => {
                for (left, right) in a.iter().zip(&b) {
                    self.unify(left, right)?;
                }
                Ok(())
            }
            (Ty::Array(a), Ty::Array(b)) | (Ty::Range(a), Ty::Range(b)) => self.unify(&a, &b),
            // Fixed-array lengths are concrete `u64`s — there are no length
            // variables and no length inference in v0 (an array literal's
            // length is its element count); differing lengths fall through
            // to the mismatch arm.
            (Ty::FixedArray(a, n), Ty::FixedArray(b, m)) if n == m => self.unify(&a, &b),
            (Ty::Option(a), Ty::Option(b)) => self.unify(&a, &b),
            (Ty::Result(a_ok, a_err), Ty::Result(b_ok, b_err))
            | (Ty::Map(a_ok, a_err), Ty::Map(b_ok, b_err)) => {
                self.unify(&a_ok, &b_ok)?;
                self.unify(&a_err, &b_err)
            }
            (Ty::Wrapper(ka, a), Ty::Wrapper(kb, b)) if ka == kb => self.unify(&a, &b),
            (Ty::Fn(a), Ty::Fn(b))
                if a.params.len() == b.params.len()
                    && a.params
                        .iter()
                        .zip(&b.params)
                        .all(|(l, r)| l.mode == r.mode) =>
            {
                // Modes are part of the function type: `fn(take Int) -> Int`
                // and `fn(in Int) -> Int` are distinct. Equal modes are the
                // guard above; here we unify the pairwise param and return
                // types. A mode mismatch falls through to the `Mismatch` arm.
                for (left, right) in a.params.iter().zip(&b.params) {
                    self.unify(&left.ty, &right.ty)?;
                }
                self.unify(&a.ret, &b.ret)
            }
            (Ty::Struct(sa, aa), Ty::Struct(sb, ab)) | (Ty::Enum(sa, aa), Ty::Enum(sb, ab))
                if sa == sb && aa.len() == ab.len() =>
            {
                for (left, right) in aa.iter().zip(&ab) {
                    self.unify(left, right)?;
                }
                Ok(())
            }
            (left, right) => Err(Mismatch {
                expected: self.apply(&left),
                actual: self.apply(&right),
            }),
        }
    }

    /// The numeric literal class of an unsolved variable, if `ty` is one —
    /// a `{integer}`/`{float}` is known *not* to be a function, struct,
    /// etc. even before it defaults. (An unsolved array literal is seen
    /// through [`Self::view`] instead.)
    pub(crate) fn literal_class(&self, ty: &Ty) -> Option<VarClass> {
        match self.shallow(ty) {
            Ty::Var(var) => {
                let class = &self.vars[var.0 as usize].class;
                matches!(class, VarClass::Integer | VarClass::Float).then(|| class.clone())
            }
            _ => None,
        }
    }

    /// Solve every remaining literal variable to its default (`I64` /
    /// `F64`, Constitution §11; `[elem; len]` for an array literal,
    /// ADR-0040) and return the origin spans of variables that stay
    /// genuinely unknown — each needs a type annotation.
    pub(crate) fn finalize(&mut self) -> Vec<Span> {
        let mut needs_annotation = Vec::new();
        for index in 0..self.vars.len() {
            let var = InferVar(u32::try_from(index).expect("var index fits in u32"));
            let Ty::Var(root) = self.shallow(&Ty::Var(var)) else {
                continue;
            };
            match &self.vars[root.0 as usize].class {
                VarClass::Integer => self.vars[root.0 as usize].value = Some(Ty::int()),
                VarClass::Float => self.vars[root.0 as usize].value = Some(Ty::float()),
                VarClass::ArrayLiteral { elem, len } => {
                    let fixed = Ty::FixedArray(Box::new(elem.clone()), *len);
                    self.vars[root.0 as usize].value = Some(fixed);
                }
                VarClass::General => {
                    self.vars[root.0 as usize].value = Some(Ty::Error);
                    needs_annotation.push(self.vars[index].origin);
                }
            }
        }
        needs_annotation
    }
}

#[cfg(test)]
mod tests {
    use tuo_source::{SourceId, TextRange};

    use super::*;
    use crate::ty::IntKind;

    fn span() -> Span {
        Span::new(
            SourceId::from_raw(0),
            TextRange::new(0u32, 0u32).expect("empty range is forward"),
        )
    }

    /// A fresh `[e1, …, en]` literal whose `n` elements are integer literals.
    fn int_literal(icx: &mut InferCtx, len: u64) -> Ty {
        let elem = icx.fresh(VarClass::Integer, span());
        icx.fresh(VarClass::ArrayLiteral { elem, len }, span())
    }

    fn fixed(elem: Ty, len: u64) -> Ty {
        Ty::FixedArray(Box::new(elem), len)
    }

    fn growable(elem: Ty) -> Ty {
        Ty::Array(Box::new(elem))
    }

    fn mismatch(result: Result<(), Mismatch>, icx: &InferCtx) -> (Ty, Ty) {
        let mismatch = result.expect_err("unification must fail");
        (
            icx.display(&mismatch.expected),
            icx.display(&mismatch.actual),
        )
    }

    #[test]
    fn literal_against_fixed_array_of_its_length_is_that_array() {
        let mut icx = InferCtx::default();
        let literal = int_literal(&mut icx, 3);
        assert!(icx.unify(&fixed(Ty::Int(IntKind::U8), 3), &literal).is_ok());
        assert_eq!(icx.apply(&literal), fixed(Ty::Int(IntKind::U8), 3));
    }

    #[test]
    fn literal_against_fixed_array_of_another_length_is_a_mismatch() {
        let mut icx = InferCtx::default();
        let literal = int_literal(&mut icx, 3);
        let result = icx.unify(&fixed(Ty::int(), 4), &literal);
        let (expected, actual) = mismatch(result, &icx);
        assert_eq!(expected, fixed(Ty::int(), 4));
        assert_eq!(actual, fixed(Ty::Var(InferVar(0)), 3));
    }

    #[test]
    fn literal_against_growable_array_is_growable() {
        let mut icx = InferCtx::default();
        let literal = int_literal(&mut icx, 2);
        assert!(
            icx.unify(&literal, &growable(Ty::Int(IntKind::U32)))
                .is_ok()
        );
        assert_eq!(icx.apply(&literal), growable(Ty::Int(IntKind::U32)));
    }

    #[test]
    fn literal_element_still_unifies_with_the_context() {
        let mut icx = InferCtx::default();
        let literal = int_literal(&mut icx, 2);
        let (expected, actual) = mismatch(icx.unify(&growable(Ty::Bool), &literal), &icx);
        assert_eq!((expected, actual), (Ty::int(), Ty::Bool));
    }

    #[test]
    fn literal_against_a_non_array_is_a_mismatch_printing_the_asked_type() {
        let mut icx = InferCtx::default();
        let literal = int_literal(&mut icx, 2);
        let (expected, actual) = mismatch(icx.unify(&Ty::int(), &literal), &icx);
        assert_eq!(expected, Ty::int());
        assert_eq!(actual, fixed(Ty::Var(InferVar(0)), 2));
    }

    #[test]
    fn literal_against_error_or_never_stays_unsolved() {
        for ty in [Ty::Error, Ty::Never] {
            let mut icx = InferCtx::default();
            let literal = int_literal(&mut icx, 1);
            assert!(icx.unify(&literal, &ty).is_ok());
            assert!(matches!(icx.apply(&literal), Ty::Var(_)));
        }
    }

    #[test]
    fn general_variable_merged_with_a_literal_keeps_the_literal_class() {
        let mut icx = InferCtx::default();
        let literal = int_literal(&mut icx, 2);
        let general = icx.fresh(VarClass::General, span());
        assert!(icx.unify(&general, &literal).is_ok());
        assert!(icx.unify(&general, &growable(Ty::int())).is_ok());
        assert_eq!(icx.apply(&literal), growable(Ty::int()));
    }

    #[test]
    fn literals_of_equal_length_merge_and_resolve_together() {
        let mut icx = InferCtx::default();
        let left = int_literal(&mut icx, 2);
        let right = int_literal(&mut icx, 2);
        assert!(icx.unify(&left, &right).is_ok());
        assert!(icx.unify(&growable(Ty::Int(IntKind::I8)), &right).is_ok());
        assert_eq!(icx.apply(&left), growable(Ty::Int(IntKind::I8)));
    }

    #[test]
    fn literals_of_different_lengths_are_a_mismatch_even_for_a_growable_context() {
        let mut icx = InferCtx::default();
        let left = int_literal(&mut icx, 2);
        let right = int_literal(&mut icx, 3);
        let (expected, actual) = mismatch(icx.unify(&left, &right), &icx);
        assert!(matches!(expected, Ty::FixedArray(_, 2)));
        assert!(matches!(actual, Ty::FixedArray(_, 3)));
        assert!(icx.unify(&growable(Ty::int()), &left).is_ok());
        assert!(matches!(icx.apply(&right), Ty::Var(_)));
    }

    #[test]
    fn literal_against_a_numeric_literal_is_a_mismatch() {
        for (class, number) in [
            (VarClass::Integer, Ty::int()),
            (VarClass::Float, Ty::float()),
        ] {
            let mut icx = InferCtx::default();
            let literal = int_literal(&mut icx, 2);
            let scalar = icx.fresh(class, span());
            let (expected, actual) = mismatch(icx.unify(&literal, &scalar), &icx);
            assert_eq!(expected, number);
            assert!(matches!(actual, Ty::FixedArray(_, 2)));
        }
    }

    #[test]
    fn a_fixed_array_value_never_unifies_with_a_growable_one() {
        let mut icx = InferCtx::default();
        assert!(
            icx.unify(&growable(Ty::int()), &fixed(Ty::int(), 2))
                .is_err()
        );
    }

    #[test]
    fn view_shows_an_unsolved_literal_as_its_default_without_solving_it() {
        let mut icx = InferCtx::default();
        let literal = int_literal(&mut icx, 3);
        assert_eq!(icx.view(&literal), fixed(Ty::Var(InferVar(0)), 3));
        assert!(matches!(icx.apply(&literal), Ty::Var(_)));
        assert!(icx.unify(&literal, &growable(Ty::int())).is_ok());
        assert_eq!(icx.view(&literal), growable(Ty::int()));
    }

    #[test]
    fn finalize_defaults_an_unsolved_literal_to_the_fixed_array() {
        let mut icx = InferCtx::default();
        let literal = int_literal(&mut icx, 3);
        assert!(icx.finalize().is_empty());
        assert_eq!(icx.apply(&literal), fixed(Ty::int(), 3));
    }

    #[test]
    fn finalize_still_reports_an_unsolved_element_of_an_empty_literal() {
        let mut icx = InferCtx::default();
        let elem = icx.fresh(VarClass::General, span());
        let literal = icx.fresh(VarClass::ArrayLiteral { elem, len: 0 }, span());
        assert_eq!(icx.finalize(), vec![span()]);
        assert_eq!(icx.apply(&literal), fixed(Ty::Error, 0));
    }

    #[test]
    fn empty_literal_takes_its_element_from_a_growable_context() {
        let mut icx = InferCtx::default();
        let elem = icx.fresh(VarClass::General, span());
        let literal = icx.fresh(VarClass::ArrayLiteral { elem, len: 0 }, span());
        assert!(icx.unify(&growable(Ty::String), &literal).is_ok());
        assert!(icx.finalize().is_empty());
        assert_eq!(icx.apply(&literal), growable(Ty::String));
    }
}
