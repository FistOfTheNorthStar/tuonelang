//! The type environment: derived `Copy`-ness and field-path types.
//!
//! Everything here is a read-only view over [`tuo_types::TypeckResult`],
//! implementing `specification/ownership.md` §2 (structural `Copy`) and the
//! field-path typing of §1/§9.

use std::collections::HashSet;

use tuo_resolve::SymbolId;
use tuo_types::{Ty, TypeckResult};

use crate::place::Place;

/// Read-only type queries the checker asks while walking a body.
pub(crate) struct TypeEnv<'a> {
    types: &'a TypeckResult,
}

impl<'a> TypeEnv<'a> {
    pub(crate) fn new(types: &'a TypeckResult) -> Self {
        Self { types }
    }

    /// Is `ty` `Copy` (`specification/ownership.md` §2)? Structural, never
    /// user-written: scalars, `()`, `Char`, `Str`, and ranges of `Copy` are
    /// `Copy`; aggregates are `Copy` iff every field of every variant is;
    /// `String`, `Array`, and the wrappers never are; a generic parameter is
    /// conservatively move-only. The poison type is treated as `Copy` so a
    /// type error never also produces ownership noise.
    pub(crate) fn is_copy(&self, ty: &Ty) -> bool {
        self.is_copy_guarded(ty, &mut HashSet::new())
    }

    fn is_copy_guarded(&self, ty: &Ty, visiting: &mut HashSet<SymbolId>) -> bool {
        match ty {
            Ty::Unit
            | Ty::Never
            | Ty::Bool
            | Ty::Char
            | Ty::Str
            | Ty::Int(_)
            | Ty::Float(_)
            | Ty::Fn(_)
            | Ty::Error => true,
            Ty::String | Ty::Array(_) | Ty::Wrapper(..) | Ty::Param(_) | Ty::Var(_) => false,
            Ty::Range(item) => self.is_copy_guarded(item, visiting),
            // A `[T; N]` owns no heap: like a tuple/struct of N identical
            // fields, it is `Copy` iff its element is — the length is
            // irrelevant. (Contrast: the growable `Array[T]` above is
            // *never* `Copy`; it owns a heap buffer.) ADR-0004 Stage 2.
            Ty::FixedArray(item, _) => self.is_copy_guarded(item, visiting),
            Ty::Tuple(items) => items
                .iter()
                .all(|item| self.is_copy_guarded(item, visiting)),
            Ty::Option(item) => self.is_copy_guarded(item, visiting),
            Ty::Result(ok, err) => {
                self.is_copy_guarded(ok, visiting) && self.is_copy_guarded(err, visiting)
            }
            Ty::Struct(symbol, args) => {
                if !visiting.insert(*symbol) {
                    return false;
                }
                let copy = self.types.struct_shape(*symbol).is_some_and(|shape| {
                    shape.fields.iter().all(|(_, field)| {
                        let field = instantiate(field, &shape.type_params, args);
                        self.is_copy_guarded(&field, visiting)
                    })
                });
                visiting.remove(symbol);
                copy
            }
            Ty::Enum(symbol, args) => {
                if !visiting.insert(*symbol) {
                    return false;
                }
                let copy = self.types.enum_shape(*symbol).is_some_and(|shape| {
                    shape.variants.iter().all(|(_, fields)| {
                        fields.iter().all(|(_, field)| {
                            let field = instantiate(field, &shape.type_params, args);
                            self.is_copy_guarded(&field, visiting)
                        })
                    })
                });
                visiting.remove(symbol);
                copy
            }
        }
    }

    /// The type of `place` given its root's type: walk the field path
    /// through struct fields and tuple elements. `None` when the path does
    /// not type (the front end has already diagnosed that).
    pub(crate) fn place_ty(&self, root_ty: &Ty, place: &Place) -> Option<Ty> {
        let mut ty = root_ty.clone();
        for field in &place.path {
            ty = match &ty {
                Ty::Struct(symbol, args) => {
                    let shape = self.types.struct_shape(*symbol)?;
                    let (_, declared) = shape.fields.iter().find(|(name, _)| name == field)?;
                    instantiate(declared, &shape.type_params, args)
                }
                Ty::Tuple(items) => {
                    let index: usize = field.parse().ok()?;
                    items.get(index)?.clone()
                }
                _ => return None,
            };
        }
        Some(ty)
    }
}

/// Substitute a declaration-context type: each [`Ty::Param`] listed in
/// `params` is replaced by the corresponding type argument.
fn instantiate(ty: &Ty, params: &[SymbolId], args: &[Ty]) -> Ty {
    match ty {
        Ty::Param(symbol) => params
            .iter()
            .position(|param| param == symbol)
            .and_then(|index| args.get(index))
            .cloned()
            .unwrap_or_else(|| ty.clone()),
        Ty::Tuple(items) => Ty::Tuple(
            items
                .iter()
                .map(|item| instantiate(item, params, args))
                .collect(),
        ),
        Ty::Array(item) => Ty::Array(Box::new(instantiate(item, params, args))),
        Ty::FixedArray(item, n) => Ty::FixedArray(Box::new(instantiate(item, params, args)), *n),
        Ty::Range(item) => Ty::Range(Box::new(instantiate(item, params, args))),
        Ty::Option(item) => Ty::Option(Box::new(instantiate(item, params, args))),
        Ty::Result(ok, err) => Ty::Result(
            Box::new(instantiate(ok, params, args)),
            Box::new(instantiate(err, params, args)),
        ),
        Ty::Wrapper(kind, item) => Ty::Wrapper(*kind, Box::new(instantiate(item, params, args))),
        Ty::Struct(symbol, inner) => Ty::Struct(
            *symbol,
            inner
                .iter()
                .map(|item| instantiate(item, params, args))
                .collect(),
        ),
        Ty::Enum(symbol, inner) => Ty::Enum(
            *symbol,
            inner
                .iter()
                .map(|item| instantiate(item, params, args))
                .collect(),
        ),
        Ty::Fn(fn_ty) => Ty::Fn(Box::new(tuo_types::FnTy {
            params: fn_ty
                .params
                .iter()
                .map(|item| instantiate(item, params, args))
                .collect(),
            ret: instantiate(&fn_ty.ret, params, args),
        })),
        _ => ty.clone(),
    }
}
