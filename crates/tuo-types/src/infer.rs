//! Unification for local type inference.
//!
//! Inference in tuonelang is deliberately **local**: function signatures
//! are always explicit (Constitution §8), so unification only ever solves
//! for types *within* one body — `let` bindings without annotations,
//! numeric literals, and generic instantiations. Unsolved integer/float
//! literal variables default to `I64`/`F64` (§11); any other unsolved
//! variable is an error ("type annotation needed"), never a guess.

use tuo_source::Span;

use crate::ty::{FnTy, InferVar, Ty};

/// What a fresh variable is allowed to become.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum VarClass {
    /// Any type.
    General,
    /// Some integer type; defaults to `I64`.
    Integer,
    /// Some floating-point type; defaults to `F64`.
    Float,
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
        match self.shallow(ty) {
            Ty::Var(var) => Ty::Var(var),
            Ty::Tuple(items) => Ty::Tuple(items.iter().map(|item| self.apply(item)).collect()),
            Ty::Array(item) => Ty::Array(Box::new(self.apply(&item))),
            Ty::FixedArray(item, n) => Ty::FixedArray(Box::new(self.apply(&item)), n),
            Ty::Range(item) => Ty::Range(Box::new(self.apply(&item))),
            Ty::Fn(fn_ty) => Ty::Fn(Box::new(FnTy {
                params: fn_ty.params.iter().map(|param| self.apply(param)).collect(),
                ret: self.apply(&fn_ty.ret),
            })),
            Ty::Option(item) => Ty::Option(Box::new(self.apply(&item))),
            Ty::Result(ok, err) => {
                Ty::Result(Box::new(self.apply(&ok)), Box::new(self.apply(&err)))
            }
            Ty::Struct(symbol, args) => {
                Ty::Struct(symbol, args.iter().map(|arg| self.apply(arg)).collect())
            }
            Ty::Enum(symbol, args) => {
                Ty::Enum(symbol, args.iter().map(|arg| self.apply(arg)).collect())
            }
            Ty::Wrapper(kind, item) => Ty::Wrapper(kind, Box::new(self.apply(&item))),
            other => other,
        }
    }

    /// Does `var` occur in `ty`? (Prevents building infinite types.)
    fn occurs(&self, var: InferVar, ty: &Ty) -> bool {
        match self.shallow(ty) {
            Ty::Var(other) => other == var,
            Ty::Tuple(items) => items.iter().any(|item| self.occurs(var, item)),
            Ty::Array(item)
            | Ty::FixedArray(item, _)
            | Ty::Range(item)
            | Ty::Option(item)
            | Ty::Wrapper(_, item) => self.occurs(var, &item),
            Ty::Fn(fn_ty) => {
                fn_ty.params.iter().any(|param| self.occurs(var, param))
                    || self.occurs(var, &fn_ty.ret)
            }
            Ty::Result(ok, err) => self.occurs(var, &ok) || self.occurs(var, &err),
            Ty::Struct(_, args) | Ty::Enum(_, args) => args.iter().any(|arg| self.occurs(var, arg)),
            _ => false,
        }
    }

    /// May a variable of `class` be solved to `ty`?
    fn class_admits(class: VarClass, ty: &Ty) -> bool {
        match class {
            VarClass::General => true,
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
        let class = self.vars[var.0 as usize].class;
        if !Self::class_admits(class, &ty) {
            return Err(Mismatch {
                expected: match class {
                    VarClass::Integer => Ty::int(),
                    VarClass::Float => Ty::float(),
                    VarClass::General => Ty::Var(var),
                },
                actual: self.apply(&ty),
            });
        }
        self.vars[var.0 as usize].value = Some(ty);
        Ok(())
    }

    /// Merge two unsolved variables, keeping the stricter class.
    fn merge_vars(&mut self, a: InferVar, b: InferVar) -> Result<(), Mismatch> {
        if a == b {
            return Ok(());
        }
        let class_a = self.vars[a.0 as usize].class;
        let class_b = self.vars[b.0 as usize].class;
        let merged = match (class_a, class_b) {
            (VarClass::General, other) | (other, VarClass::General) => Some(other),
            (left, right) if left == right => Some(left),
            _ => None,
        };
        let Some(merged) = merged else {
            return Err(Mismatch {
                expected: Ty::int(),
                actual: Ty::float(),
            });
        };
        self.vars[b.0 as usize].class = merged;
        self.vars[a.0 as usize].value = Some(Ty::Var(b));
        Ok(())
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
            // an annotation.
            (Ty::Var(var), ty) | (ty, Ty::Var(var)) => self.bind(var, ty),
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
            // variables and no length inference in v0; differing lengths
            // fall through to the mismatch arm.
            (Ty::FixedArray(a, n), Ty::FixedArray(b, m)) if n == m => self.unify(&a, &b),
            (Ty::Option(a), Ty::Option(b)) => self.unify(&a, &b),
            (Ty::Result(a_ok, a_err), Ty::Result(b_ok, b_err)) => {
                self.unify(&a_ok, &b_ok)?;
                self.unify(&a_err, &b_err)
            }
            (Ty::Wrapper(ka, a), Ty::Wrapper(kb, b)) if ka == kb => self.unify(&a, &b),
            (Ty::Fn(a), Ty::Fn(b)) if a.params.len() == b.params.len() => {
                for (left, right) in a.params.iter().zip(&b.params) {
                    self.unify(left, right)?;
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

    /// The literal class of an unsolved variable, if `ty` is one — a
    /// `{integer}`/`{float}` is known *not* to be a function, struct, etc.
    /// even before it defaults.
    pub(crate) fn literal_class(&self, ty: &Ty) -> Option<VarClass> {
        match self.shallow(ty) {
            Ty::Var(var) => {
                let class = self.vars[var.0 as usize].class;
                (class != VarClass::General).then_some(class)
            }
            _ => None,
        }
    }

    /// Solve every remaining literal variable to its Constitution default
    /// (`I64` / `F64`, §11) and return the origin spans of variables that
    /// stay genuinely unknown — each needs a type annotation.
    pub(crate) fn finalize(&mut self) -> Vec<Span> {
        let mut needs_annotation = Vec::new();
        for index in 0..self.vars.len() {
            let var = InferVar(u32::try_from(index).expect("var index fits in u32"));
            let Ty::Var(root) = self.shallow(&Ty::Var(var)) else {
                continue;
            };
            match self.vars[root.0 as usize].class {
                VarClass::Integer => self.vars[root.0 as usize].value = Some(Ty::int()),
                VarClass::Float => self.vars[root.0 as usize].value = Some(Ty::float()),
                VarClass::General => {
                    self.vars[root.0 as usize].value = Some(Ty::Error);
                    needs_annotation.push(self.vars[index].origin);
                }
            }
        }
        needs_annotation
    }
}
