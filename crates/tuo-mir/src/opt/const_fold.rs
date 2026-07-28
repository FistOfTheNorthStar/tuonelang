//! Constant folding: evaluate a constant-operand computation at compile time.
//!
//! Where every operand of an rvalue is a [`Const`], the pass replaces the
//! rvalue with the constant it evaluates to — but **only when that evaluation
//! cannot trap**. A tuonelang integer operation traps deterministically on
//! overflow and on division/remainder by zero or `MIN`-over-`-1` (§24); the
//! interpreter aborts there, so folding such an operation into a value would
//! erase an observable trap and change the program's meaning. This pass
//! therefore leaves any trapping operation exactly as it found it, and the
//! backend lowers it to the same trap the interpreter would raise.
//!
//! The fold arithmetic mirrors the reference interpreter
//! ([`tuo_mir_interp`]) instruction for instruction — same two's-complement
//! ranges, same wrapping/saturating cast rules, same float rounding — so a
//! folded constant is bit-identical to the value the interpreter would have
//! computed at run time. Folding never introduces a value the interpreter
//! could not have produced.
//!
//! Only scalar constants are folded (`Bool`, `Int`, `Char`, `Float`). `Unit`
//! and `Str` have no arithmetic, and aggregates are left alone.

use tuo_types::{FloatKind, IntKind, Ty, TypeckResult};

use super::Pass;
use crate::mir::{BinOp, CastKind, Const, Function, Operand, Rvalue, Statement, UnOp};

/// The constant-folding pass. See the module docs for the soundness argument.
pub(super) struct ConstFold;

impl Pass for ConstFold {
    fn name(&self) -> &'static str {
        "const-fold"
    }

    fn purpose(&self) -> &'static str {
        "Replace a constant-operand arithmetic, comparison, unary, or cast \
         rvalue with the single constant it evaluates to, matching the \
         reference interpreter — but never folding an operation that would \
         trap, since that would erase an observable deterministic abort."
    }

    fn preconditions(&self) -> &'static [&'static str] {
        &[
            "verified MIR",
            "operand types are consistent (the verifier guarantees a binary \
             op's operands share a kind), so folding may trust the operand \
             kinds it reads",
        ]
    }

    fn run(&self, function: &mut Function, _types: &TypeckResult) -> bool {
        let mut changed = false;
        for block in &mut function.blocks {
            for statement in &mut block.statements {
                if let Statement::Assign { rvalue, .. } = statement
                    && let Some(folded) = fold_rvalue(rvalue)
                {
                    *rvalue = Rvalue::Use(Operand::Const(folded));
                    changed = true;
                }
            }
        }
        changed
    }
}

/// Fold an rvalue to a single constant, or `None` if it is not a
/// constant-operand computation or the computation would trap.
fn fold_rvalue(rvalue: &Rvalue) -> Option<Const> {
    match rvalue {
        // `Use(Const)` is already a constant — folding it to itself is not a
        // change, so report nothing.
        Rvalue::Use(_) => None,
        Rvalue::Unary { op, operand } => fold_unary(*op, as_const(operand)?),
        Rvalue::Binary { op, lhs, rhs } => fold_binary(*op, as_const(lhs)?, as_const(rhs)?),
        Rvalue::Cast { kind, operand, to } => fold_cast(*kind, as_const(operand)?, to),
        Rvalue::Aggregate { .. } | Rvalue::Discriminant(_) | Rvalue::Len(_) => None,
    }
}

/// The constant an operand reads, or `None` for a place read.
fn as_const(operand: &Operand) -> Option<&Const> {
    match operand {
        Operand::Const(constant) => Some(constant),
        Operand::Copy(_) | Operand::Move(_) => None,
    }
}

/// Fold a unary op over a constant. Integer negation traps on the minimum
/// value, so it is not folded there (`None`).
fn fold_unary(op: UnOp, operand: &Const) -> Option<Const> {
    match (op, operand) {
        (UnOp::Not, Const::Bool(value)) => Some(Const::Bool(!value)),
        (UnOp::Neg, Const::Int(value, kind)) => {
            let (min, _max) = int_bounds(*kind);
            // Negating the minimum overflows and traps — leave it for the
            // backend/interpreter to trap, do not fold.
            (*value != min).then(|| Const::Int(-value, *kind))
        }
        (UnOp::Neg, Const::Float(value, kind)) => Some(Const::Float(-value, *kind)),
        _ => None,
    }
}

/// Fold a binary op over two constants. Returns `None` when the operation
/// would trap (so the trap is preserved) or the operands are not a foldable
/// scalar pair.
fn fold_binary(op: BinOp, lhs: &Const, rhs: &Const) -> Option<Const> {
    match (lhs, rhs) {
        (Const::Int(a, ka), Const::Int(b, kb)) => {
            debug_assert_eq!(ka, kb, "verifier guarantees matching integer kinds");
            fold_int_binary(op, *a, *b, *ka)
        }
        (Const::Float(a, ka), Const::Float(b, _kb)) => Some(fold_float_binary(op, *a, *b, *ka)),
        (Const::Bool(a), Const::Bool(b)) => match op {
            BinOp::Eq => Some(Const::Bool(a == b)),
            BinOp::Ne => Some(Const::Bool(a != b)),
            _ => None,
        },
        (Const::Char(a), Const::Char(b)) => Some(Const::Bool(match op {
            BinOp::Eq => a == b,
            BinOp::Ne => a != b,
            BinOp::Lt => a < b,
            BinOp::Le => a <= b,
            BinOp::Gt => a > b,
            BinOp::Ge => a >= b,
            _ => return None,
        })),
        // `Str` comparisons are constant-evaluable but v0 lowers no `Str`
        // arithmetic to a scalar backend; leave them for a later pass.
        _ => None,
    }
}

/// Fold integer arithmetic/comparison, mirroring the interpreter's checked
/// semantics. Any operation whose interpreter counterpart would trap folds to
/// `None` (leaving the operation in place so the trap still happens).
fn fold_int_binary(op: BinOp, a: i128, b: i128, kind: IntKind) -> Option<Const> {
    let (min, max) = int_bounds(kind);
    let in_range = |value: i128| (min..=max).contains(&value);
    match op {
        // Arithmetic: fold only if the exact result is in range (no overflow
        // trap). `i128` cannot overflow for any tuonelang integer width, so
        // the range check is the whole overflow test.
        BinOp::Add => in_range(a + b).then_some(Const::Int(a + b, kind)),
        BinOp::Sub => in_range(a - b).then_some(Const::Int(a - b, kind)),
        BinOp::Mul => in_range(a * b).then_some(Const::Int(a * b, kind)),
        BinOp::Div => {
            // Trap on zero divisor and on `MIN / -1`; otherwise truncate
            // toward zero (Rust's `/` on i128 truncates toward zero).
            if b == 0 || (a == min && b == -1) {
                None
            } else {
                Some(Const::Int(a / b, kind))
            }
        }
        BinOp::Rem => {
            // Trap on zero divisor and on `MIN % -1` (its division overflows).
            if b == 0 || (a == min && b == -1) {
                None
            } else {
                Some(Const::Int(a % b, kind))
            }
        }
        BinOp::Eq => Some(Const::Bool(a == b)),
        BinOp::Ne => Some(Const::Bool(a != b)),
        BinOp::Lt => Some(Const::Bool(a < b)),
        BinOp::Le => Some(Const::Bool(a <= b)),
        BinOp::Gt => Some(Const::Bool(a > b)),
        BinOp::Ge => Some(Const::Bool(a >= b)),
    }
}

/// Fold float arithmetic/comparison (IEEE 754, never traps).
fn fold_float_binary(op: BinOp, a: f64, b: f64, kind: FloatKind) -> Const {
    match op {
        BinOp::Add => Const::Float(normalize_float(a + b, kind), kind),
        BinOp::Sub => Const::Float(normalize_float(a - b, kind), kind),
        BinOp::Mul => Const::Float(normalize_float(a * b, kind), kind),
        BinOp::Div => Const::Float(normalize_float(a / b, kind), kind),
        BinOp::Rem => Const::Float(normalize_float(a % b, kind), kind),
        BinOp::Eq => Const::Bool(a == b),
        BinOp::Ne => Const::Bool(a != b),
        BinOp::Lt => Const::Bool(a < b),
        BinOp::Le => Const::Bool(a <= b),
        BinOp::Gt => Const::Bool(a > b),
        BinOp::Ge => Const::Bool(a >= b),
    }
}

/// Fold a numeric cast (casts never trap; §MIR `CastKind`).
fn fold_cast(kind: CastKind, operand: &Const, to: &Ty) -> Option<Const> {
    match (kind, operand) {
        (CastKind::IntToInt, Const::Int(value, _)) => {
            let target = int_kind(to)?;
            Some(Const::Int(wrap_int(*value, target), target))
        }
        (CastKind::IntToFloat, Const::Int(value, _)) => {
            let target = float_kind(to)?;
            #[expect(
                clippy::cast_precision_loss,
                reason = "int→float rounds to nearest even by definition of the cast"
            )]
            Some(Const::Float(normalize_float(*value as f64, target), target))
        }
        (CastKind::FloatToInt, Const::Float(value, _)) => {
            let target = int_kind(to)?;
            Some(Const::Int(saturating_float_to_int(*value, target), target))
        }
        (CastKind::FloatToFloat, Const::Float(value, _)) => {
            let target = float_kind(to)?;
            Some(Const::Float(normalize_float(*value, target), target))
        }
        _ => None,
    }
}

// --- shared numeric helpers, kept in lockstep with the interpreter ---------

/// The inclusive `[min, max]` range of an integer kind. `Isize`/`Usize` are
/// modelled as 64-bit, matching the interpreter's target-independent choice.
fn int_bounds(kind: IntKind) -> (i128, i128) {
    match kind {
        IntKind::I8 => (i128::from(i8::MIN), i128::from(i8::MAX)),
        IntKind::I16 => (i128::from(i16::MIN), i128::from(i16::MAX)),
        IntKind::I32 => (i128::from(i32::MIN), i128::from(i32::MAX)),
        IntKind::I64 | IntKind::Isize => (i128::from(i64::MIN), i128::from(i64::MAX)),
        IntKind::U8 => (0, i128::from(u8::MAX)),
        IntKind::U16 => (0, i128::from(u16::MAX)),
        IntKind::U32 => (0, i128::from(u32::MAX)),
        IntKind::U64 | IntKind::Usize => (0, i128::from(u64::MAX)),
    }
}

/// Wrap a value into an integer kind's width (two's-complement truncation),
/// matching `CastKind::IntToInt`.
fn wrap_int(value: i128, kind: IntKind) -> i128 {
    let (min, max) = int_bounds(kind);
    let modulus = (max - min) + 1;
    let mut wrapped = value.rem_euclid(modulus);
    if wrapped > max {
        wrapped -= modulus;
    }
    wrapped
}

/// Truncate a float toward zero, then saturate into an integer kind's range;
/// NaN maps to 0. Matches `CastKind::FloatToInt`.
fn saturating_float_to_int(value: f64, kind: IntKind) -> i128 {
    let (min, max) = int_bounds(kind);
    if value.is_nan() {
        return 0;
    }
    let truncated = value.trunc();
    if truncated <= min as f64 {
        min
    } else if truncated >= max as f64 {
        max
    } else {
        // In-range after truncation: exactly representable as i128.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "guarded to be within [min, max] by the branches above"
        )]
        let as_int = truncated as i128;
        as_int
    }
}

/// Round a float to the precision of its kind (an `F32` value is stored at
/// `f64` precision and rounds to nearest even when materialized).
fn normalize_float(value: f64, kind: FloatKind) -> f64 {
    match kind {
        FloatKind::F64 => value,
        FloatKind::F32 => f64::from(value as f32),
    }
}

/// The integer kind of a scalar target type, or `None` if it is not one.
fn int_kind(ty: &Ty) -> Option<IntKind> {
    match ty {
        Ty::Int(kind) => Some(*kind),
        _ => None,
    }
}

/// The float kind of a scalar target type, or `None` if it is not one.
fn float_kind(ty: &Ty) -> Option<FloatKind> {
    match ty {
        Ty::Float(kind) => Some(*kind),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use tuo_types::{FloatKind, IntKind, Ty};

    use super::{fold_binary, fold_cast, fold_unary, saturating_float_to_int, wrap_int};
    use crate::mir::{BinOp, CastKind, Const, UnOp};

    fn int(value: i128, kind: IntKind) -> Const {
        Const::Int(value, kind)
    }

    #[test]
    fn folds_non_trapping_addition() {
        let folded = fold_binary(BinOp::Add, &int(2, IntKind::I64), &int(3, IntKind::I64));
        assert!(matches!(folded, Some(Const::Int(5, IntKind::I64))));
    }

    #[test]
    fn refuses_to_fold_overflowing_addition() {
        // I8::MAX + 1 overflows — must not fold (the trap must survive).
        let folded = fold_binary(BinOp::Add, &int(127, IntKind::I8), &int(1, IntKind::I8));
        assert!(folded.is_none(), "overflowing add must not be folded");
    }

    #[test]
    fn refuses_to_fold_division_by_zero() {
        let folded = fold_binary(BinOp::Div, &int(1, IntKind::I64), &int(0, IntKind::I64));
        assert!(folded.is_none(), "1/0 must not be folded");
    }

    #[test]
    fn refuses_to_fold_min_over_minus_one() {
        let min = i128::from(i32::MIN);
        let folded = fold_binary(BinOp::Div, &int(min, IntKind::I32), &int(-1, IntKind::I32));
        assert!(folded.is_none(), "MIN / -1 must not be folded");
    }

    #[test]
    fn division_truncates_toward_zero() {
        let folded = fold_binary(BinOp::Div, &int(-7, IntKind::I64), &int(2, IntKind::I64));
        assert!(matches!(folded, Some(Const::Int(-3, IntKind::I64))));
    }

    #[test]
    fn folds_comparisons_to_bool() {
        let folded = fold_binary(BinOp::Lt, &int(2, IntKind::I64), &int(3, IntKind::I64));
        assert!(matches!(folded, Some(Const::Bool(true))));
    }

    #[test]
    fn refuses_to_fold_negating_the_minimum() {
        let min = i128::from(i8::MIN);
        assert!(fold_unary(UnOp::Neg, &int(min, IntKind::I8)).is_none());
        // A non-minimum negation folds.
        assert!(matches!(
            fold_unary(UnOp::Neg, &int(5, IntKind::I8)),
            Some(Const::Int(-5, IntKind::I8))
        ));
    }

    #[test]
    fn folds_bool_not() {
        assert!(matches!(
            fold_unary(UnOp::Not, &Const::Bool(true)),
            Some(Const::Bool(false))
        ));
    }

    #[test]
    fn int_to_int_cast_wraps() {
        let folded = fold_cast(
            CastKind::IntToInt,
            &int(300, IntKind::I32),
            &Ty::Int(IntKind::I8),
        );
        // 300 wraps into I8 as 44.
        assert!(matches!(folded, Some(Const::Int(44, IntKind::I8))));
    }

    #[test]
    fn wrap_and_saturate_match_the_interpreter() {
        assert_eq!(wrap_int(300, IntKind::I8), 44);
        assert_eq!(wrap_int(-1, IntKind::U8), 255);
        assert_eq!(saturating_float_to_int(3.9, IntKind::I32), 3);
        assert_eq!(saturating_float_to_int(1e30, IntKind::I8), 127);
        assert_eq!(saturating_float_to_int(f64::NAN, IntKind::I32), 0);
    }

    #[test]
    fn f32_folding_rounds_to_f32_precision() {
        // 0.1 + 0.2 at f32 precision differs from the f64 result; the fold
        // must round to f32 exactly as the interpreter does.
        let folded = super::fold_float_binary(BinOp::Add, 0.1, 0.2, FloatKind::F32);
        if let Const::Float(value, FloatKind::F32) = folded {
            assert_eq!(value, f64::from(0.1_f32 + 0.2_f32));
        } else {
            panic!("expected a folded F32 constant");
        }
    }
}
