//! Intrinsic operations (`specs/OPS.md`) and their per-instance policies.
//!
//! An [`Op`] names one OPS operation and carries exactly the policy fields that
//! operation consults. The policy is fixed when the op is lowered, so every tier
//! (T0 evaluator, VM, JIT, AOT) sees the same choice and produces the same result
//! or the same error.

use crate::lit::Prim;

/// What happens on integer overflow (`add sub mul neg abs`, `div MIN / -1`,
/// `int_cast`); `specs/OPS.md` section 2.
///
/// # Examples
///
/// ```
/// use hir_lang::Overflow;
///
/// assert_ne!(Overflow::Error, Overflow::Wrap);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Overflow {
    /// A runtime error with kind `ArithOverflow` (OPS default).
    Error,
    /// Two's-complement wrap-around; never an error.
    Wrap,
    /// A deterministic abort with the same kind code.
    Trap,
    /// The result is the `f64` nearest to the exact mathematical result
    /// (PHP's `PHP_INT_MAX + 1`). Valid only where the result is dynamically
    /// typed: the validator rejects it where HIR fixes a static result type
    /// (`int_cast`, a cast to anything but `Any`, an integer constant context),
    /// and typeck-lang rejects it where it infers one.
    Promote,
}

/// What happens on integer division or remainder by zero.
///
/// # Examples
///
/// ```
/// use hir_lang::DivZero;
///
/// assert_ne!(DivZero::Error, DivZero::Trap);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DivZero {
    /// A runtime error with kind `DivByZero` (OPS default).
    Error,
    /// A deterministic abort.
    Trap,
}

/// What happens when a shift amount is at least the bit width, or negative.
///
/// # Examples
///
/// ```
/// use hir_lang::Shift;
///
/// assert_ne!(Shift::Error, Shift::Mask);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Shift {
    /// A runtime error with kind `ShiftOutOfRange` (OPS default).
    Error,
    /// Use the amount modulo the bit width.
    Mask,
}

/// What happens when a float-to-int conversion meets NaN or an out-of-range value.
///
/// # Examples
///
/// ```
/// use hir_lang::FloatToInt;
///
/// assert_ne!(FloatToInt::Error, FloatToInt::Saturate);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FloatToInt {
    /// A runtime error with kind `InvalidConversion` (OPS default).
    Error,
    /// Clamp to the target's range; NaN becomes `0`.
    Saturate,
}

/// The policy fields of one op instance.
///
/// A field is `Some` exactly when the op consults it: the validator rejects a
/// missing field (a tier would have to guess) and an extra one (a sign of a
/// lowering bug).
///
/// # Examples
///
/// ```
/// use hir_lang::{Overflow, Policy};
///
/// let policy = Policy::NONE.with_overflow(Overflow::Wrap);
/// assert_eq!(policy.overflow, Some(Overflow::Wrap));
/// assert_eq!(policy.div_zero, None);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Policy {
    /// The overflow policy.
    pub overflow: Option<Overflow>,
    /// The division-by-zero policy.
    pub div_zero: Option<DivZero>,
    /// The shift-amount policy.
    pub shift: Option<Shift>,
    /// The float-to-int conversion policy.
    pub float_to_int: Option<FloatToInt>,
}

impl Policy {
    /// No policy fields: for ops that consult none.
    pub const NONE: Self = Self {
        overflow: None,
        div_zero: None,
        shift: None,
        float_to_int: None,
    };

    /// The policy a [`Cast`](crate::Expr::Cast) carries: `overflow` and
    /// `float_to_int`, at the OPS defaults (`error`).
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{FloatToInt, Overflow, Policy};
    ///
    /// let p = Policy::CAST;
    /// assert_eq!((p.overflow, p.float_to_int), (Some(Overflow::Error), Some(FloatToInt::Error)));
    /// ```
    pub const CAST: Self = Self {
        overflow: Some(Overflow::Error),
        div_zero: None,
        shift: None,
        float_to_int: Some(FloatToInt::Error),
    };

    /// Returns the policy with `overflow` set.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Overflow, Policy};
    ///
    /// assert_eq!(Policy::NONE.with_overflow(Overflow::Trap).overflow, Some(Overflow::Trap));
    /// ```
    #[must_use]
    pub const fn with_overflow(mut self, overflow: Overflow) -> Self {
        self.overflow = Some(overflow);
        self
    }

    /// Returns the policy with `div_zero` set.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{DivZero, Policy};
    ///
    /// assert_eq!(Policy::NONE.with_div_zero(DivZero::Trap).div_zero, Some(DivZero::Trap));
    /// ```
    #[must_use]
    pub const fn with_div_zero(mut self, div_zero: DivZero) -> Self {
        self.div_zero = Some(div_zero);
        self
    }

    /// Returns the policy with `shift` set.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Policy, Shift};
    ///
    /// assert_eq!(Policy::NONE.with_shift(Shift::Mask).shift, Some(Shift::Mask));
    /// ```
    #[must_use]
    pub const fn with_shift(mut self, shift: Shift) -> Self {
        self.shift = Some(shift);
        self
    }

    /// Returns the policy with `float_to_int` set.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{FloatToInt, Policy};
    ///
    /// let p = Policy::NONE.with_float_to_int(FloatToInt::Saturate);
    /// assert_eq!(p.float_to_int, Some(FloatToInt::Saturate));
    /// ```
    #[must_use]
    pub const fn with_float_to_int(mut self, float_to_int: FloatToInt) -> Self {
        self.float_to_int = Some(float_to_int);
        self
    }

    /// Returns `true` if the same fields are set in both policies (values aside).
    pub(crate) const fn same_fields(self, other: Self) -> bool {
        self.overflow.is_some() == other.overflow.is_some()
            && self.div_zero.is_some() == other.div_zero.is_some()
            && self.shift.is_some() == other.shift.is_some()
            && self.float_to_int.is_some() == other.float_to_int.is_some()
    }
}

/// An operation from `specs/OPS.md`. Conversions carry their target type.
///
/// Operand types are not part of the op: the same `add` is an integer add under
/// its overflow policy when typeck finds integer operands, an IEEE add for
/// floats, and an operator-overloading site for user types.
///
/// # Examples
///
/// ```
/// use hir_lang::{OpKind, Prim};
///
/// assert_eq!(OpKind::FloorDiv.name(), "floor_div");
/// assert_eq!(OpKind::Fma.arity(), 3);
/// assert_eq!(OpKind::IntCast(Prim::U8).name(), "int_cast");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OpKind {
    /// `add` (2).
    Add,
    /// `sub` (2).
    Sub,
    /// `mul` (2).
    Mul,
    /// `neg` (1).
    Neg,
    /// `abs` (1).
    Abs,
    /// `div`: truncating division (2).
    Div,
    /// `floor_div`: flooring division (2).
    FloorDiv,
    /// `rem`: remainder with the dividend's sign (2).
    Rem,
    /// `floor_mod`: remainder with the divisor's sign (2).
    FloorMod,
    /// `and` (2).
    And,
    /// `or` (2).
    Or,
    /// `xor` (2).
    Xor,
    /// `not`: bitwise or logical complement (1).
    Not,
    /// `shl` (2).
    Shl,
    /// `shr`: arithmetic for signed, logical for unsigned (2).
    Shr,
    /// `eq` (2).
    Eq,
    /// `ne` (2).
    Ne,
    /// `lt` (2).
    Lt,
    /// `le` (2).
    Le,
    /// `gt` (2).
    Gt,
    /// `ge` (2).
    Ge,
    /// `min` (2).
    Min,
    /// `max` (2).
    Max,
    /// `ieee_rem`: IEEE 754 remainder (2).
    IeeeRem,
    /// `sqrt` (1).
    Sqrt,
    /// `fma`: fused multiply-add (3).
    Fma,
    /// `floor` (1).
    Floor,
    /// `ceil` (1).
    Ceil,
    /// `trunc`: float truncation toward zero (1).
    Trunc,
    /// `round`: ties away from zero (1).
    Round,
    /// `round_even`: ties to even (1).
    RoundEven,
    /// `total_cmp`: IEEE totalOrder, yielding `i8` (2).
    TotalCmp,
    /// `int_cast<T>`: exact integer conversion (1).
    IntCast(Prim),
    /// `float_to_int<T>` (1).
    FloatToInt(Prim),
    /// `zext<T>`: zero extension (1).
    Zext(Prim),
    /// `sext<T>`: sign extension (1).
    Sext(Prim),
    /// `narrow<T>`: OPS's integer `trunc` conversion, renamed so it cannot be
    /// confused with the float [`OpKind::Trunc`] (1).
    Narrow(Prim),
    /// `int_to_float<T>` (1).
    IntToFloat(Prim),
    /// `bitcast<T>`: same-width reinterpretation (1).
    Bitcast(Prim),
    /// `bool_to_int<T>` (1).
    BoolToInt(Prim),
    /// `f32_to_f64` (1).
    F32ToF64,
    /// `f64_to_f32` (1).
    F64ToF32,
    /// `char_from_u32` (1).
    CharFromU32,
}

impl OpKind {
    /// Returns the operation's name as spelled in `specs/OPS.md` (`narrow` for
    /// the integer truncation conversion). Conversion targets are not included.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{OpKind, Prim};
    ///
    /// assert_eq!(OpKind::RoundEven.name(), "round_even");
    /// assert_eq!(OpKind::Narrow(Prim::U8).name(), "narrow");
    /// ```
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Sub => "sub",
            Self::Mul => "mul",
            Self::Neg => "neg",
            Self::Abs => "abs",
            Self::Div => "div",
            Self::FloorDiv => "floor_div",
            Self::Rem => "rem",
            Self::FloorMod => "floor_mod",
            Self::And => "and",
            Self::Or => "or",
            Self::Xor => "xor",
            Self::Not => "not",
            Self::Shl => "shl",
            Self::Shr => "shr",
            Self::Eq => "eq",
            Self::Ne => "ne",
            Self::Lt => "lt",
            Self::Le => "le",
            Self::Gt => "gt",
            Self::Ge => "ge",
            Self::Min => "min",
            Self::Max => "max",
            Self::IeeeRem => "ieee_rem",
            Self::Sqrt => "sqrt",
            Self::Fma => "fma",
            Self::Floor => "floor",
            Self::Ceil => "ceil",
            Self::Trunc => "trunc",
            Self::Round => "round",
            Self::RoundEven => "round_even",
            Self::TotalCmp => "total_cmp",
            Self::IntCast(_) => "int_cast",
            Self::FloatToInt(_) => "float_to_int",
            Self::Zext(_) => "zext",
            Self::Sext(_) => "sext",
            Self::Narrow(_) => "narrow",
            Self::IntToFloat(_) => "int_to_float",
            Self::Bitcast(_) => "bitcast",
            Self::BoolToInt(_) => "bool_to_int",
            Self::F32ToF64 => "f32_to_f64",
            Self::F64ToF32 => "f64_to_f32",
            Self::CharFromU32 => "char_from_u32",
        }
    }

    /// Returns the conversion target type, for the conversions that have one.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{OpKind, Prim};
    ///
    /// assert_eq!(OpKind::Zext(Prim::U64).target(), Some(Prim::U64));
    /// assert_eq!(OpKind::Add.target(), None);
    /// ```
    #[must_use]
    pub const fn target(self) -> Option<Prim> {
        match self {
            Self::IntCast(t)
            | Self::FloatToInt(t)
            | Self::Zext(t)
            | Self::Sext(t)
            | Self::Narrow(t)
            | Self::IntToFloat(t)
            | Self::Bitcast(t)
            | Self::BoolToInt(t) => Some(t),
            _ => None,
        }
    }

    /// Returns the number of operands.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::OpKind;
    ///
    /// assert_eq!(OpKind::Neg.arity(), 1);
    /// assert_eq!(OpKind::Shl.arity(), 2);
    /// ```
    #[must_use]
    pub const fn arity(self) -> usize {
        match self {
            Self::Neg
            | Self::Abs
            | Self::Not
            | Self::Sqrt
            | Self::Floor
            | Self::Ceil
            | Self::Trunc
            | Self::Round
            | Self::RoundEven
            | Self::IntCast(_)
            | Self::FloatToInt(_)
            | Self::Zext(_)
            | Self::Sext(_)
            | Self::Narrow(_)
            | Self::IntToFloat(_)
            | Self::Bitcast(_)
            | Self::BoolToInt(_)
            | Self::F32ToF64
            | Self::F64ToF32
            | Self::CharFromU32 => 1,
            Self::Fma => 3,
            _ => 2,
        }
    }

    /// Returns the policy this op uses at the OPS defaults: exactly the fields
    /// it consults, each set to `error`.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{DivZero, OpKind, Overflow};
    ///
    /// let p = OpKind::Div.default_policy();
    /// assert_eq!((p.overflow, p.div_zero), (Some(Overflow::Error), Some(DivZero::Error)));
    /// assert_eq!(OpKind::Eq.default_policy(), hir_lang::Policy::NONE);
    /// ```
    #[must_use]
    pub const fn default_policy(self) -> Policy {
        match self {
            Self::Add | Self::Sub | Self::Mul | Self::Neg | Self::Abs | Self::IntCast(_) => {
                Policy::NONE.with_overflow(Overflow::Error)
            }
            Self::Div | Self::FloorDiv => Policy::NONE
                .with_overflow(Overflow::Error)
                .with_div_zero(DivZero::Error),
            Self::Rem | Self::FloorMod => Policy::NONE.with_div_zero(DivZero::Error),
            Self::Shl | Self::Shr => Policy::NONE.with_shift(Shift::Error),
            Self::FloatToInt(_) => Policy::NONE.with_float_to_int(FloatToInt::Error),
            _ => Policy::NONE,
        }
    }

    /// Returns `true` if the conversion target (if any) has the right class:
    /// integer for the integer conversions, float for `int_to_float`, numeric
    /// for `bitcast`.
    pub(crate) const fn target_ok(self) -> bool {
        match self {
            Self::IntCast(t)
            | Self::FloatToInt(t)
            | Self::Zext(t)
            | Self::Sext(t)
            | Self::Narrow(t)
            | Self::BoolToInt(t) => t.is_int(),
            Self::IntToFloat(t) => t.is_float(),
            Self::Bitcast(t) => t.is_int() || t.is_float(),
            _ => true,
        }
    }
}

/// One op instance: the operation and its policy.
///
/// # Examples
///
/// ```
/// use hir_lang::{Op, OpKind, Overflow};
///
/// let wrapping_add = Op::new(OpKind::Add).with_overflow(Overflow::Wrap);
/// assert_eq!(wrapping_add.policy.overflow, Some(Overflow::Wrap));
/// assert!(wrapping_add.policy_matches());
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Op {
    /// The operation.
    pub kind: OpKind,
    /// Its policy; exactly the fields `kind` consults are set.
    pub policy: Policy,
}

impl Op {
    /// The op at the OPS default policy.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Op, OpKind};
    ///
    /// assert!(Op::new(OpKind::Shr).policy.shift.is_some());
    /// ```
    #[must_use]
    pub const fn new(kind: OpKind) -> Self {
        Self {
            kind,
            policy: kind.default_policy(),
        }
    }

    /// Sets the overflow policy (meaningful only for ops that consult it; the
    /// validator rejects it on others).
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Op, OpKind, Overflow};
    ///
    /// assert!(Op::new(OpKind::Mul).with_overflow(Overflow::Trap).policy_matches());
    /// assert!(!Op::new(OpKind::Eq).with_overflow(Overflow::Trap).policy_matches());
    /// ```
    #[must_use]
    pub const fn with_overflow(mut self, overflow: Overflow) -> Self {
        self.policy = self.policy.with_overflow(overflow);
        self
    }

    /// Sets the division-by-zero policy.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{DivZero, Op, OpKind};
    ///
    /// assert!(Op::new(OpKind::Rem).with_div_zero(DivZero::Trap).policy_matches());
    /// ```
    #[must_use]
    pub const fn with_div_zero(mut self, div_zero: DivZero) -> Self {
        self.policy = self.policy.with_div_zero(div_zero);
        self
    }

    /// Sets the shift policy.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Op, OpKind, Shift};
    ///
    /// assert!(Op::new(OpKind::Shl).with_shift(Shift::Mask).policy_matches());
    /// ```
    #[must_use]
    pub const fn with_shift(mut self, shift: Shift) -> Self {
        self.policy = self.policy.with_shift(shift);
        self
    }

    /// Sets the float-to-int policy.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{FloatToInt, Op, OpKind, Prim};
    ///
    /// let op = Op::new(OpKind::FloatToInt(Prim::I32)).with_float_to_int(FloatToInt::Saturate);
    /// assert!(op.policy_matches());
    /// ```
    #[must_use]
    pub const fn with_float_to_int(mut self, float_to_int: FloatToInt) -> Self {
        self.policy = self.policy.with_float_to_int(float_to_int);
        self
    }

    /// Returns `true` if the policy sets exactly the fields the op consults.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Op, OpKind, Policy};
    ///
    /// let missing = Op { kind: OpKind::Add, policy: Policy::NONE };
    /// assert!(!missing.policy_matches());
    /// ```
    #[must_use]
    pub const fn policy_matches(self) -> bool {
        self.policy.same_fields(self.kind.default_policy())
    }

    /// Returns `true` if the op uses the `promote` overflow policy, so its
    /// result may be an `f64` and must be dynamically typed.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Op, OpKind, Overflow};
    ///
    /// assert!(Op::new(OpKind::Add).with_overflow(Overflow::Promote).promotes());
    /// assert!(!Op::new(OpKind::Add).promotes());
    /// ```
    #[must_use]
    pub const fn promotes(self) -> bool {
        matches!(self.policy.overflow, Some(Overflow::Promote))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_policy_matches_ops_families() {
        assert_eq!(OpKind::Add.default_policy().overflow, Some(Overflow::Error));
        assert_eq!(OpKind::Rem.default_policy().overflow, None);
        assert_eq!(
            OpKind::FloorMod.default_policy().div_zero,
            Some(DivZero::Error)
        );
        assert_eq!(OpKind::Sqrt.default_policy(), Policy::NONE);
        assert_eq!(
            OpKind::IntCast(Prim::I8).default_policy().overflow,
            Some(Overflow::Error)
        );
        assert_eq!(OpKind::Zext(Prim::I8).default_policy(), Policy::NONE);
    }

    #[test]
    fn test_target_classes() {
        assert!(OpKind::IntCast(Prim::U16).target_ok());
        assert!(!OpKind::IntCast(Prim::F64).target_ok());
        assert!(OpKind::IntToFloat(Prim::F32).target_ok());
        assert!(!OpKind::IntToFloat(Prim::I32).target_ok());
        assert!(!OpKind::Bitcast(Prim::Char).target_ok());
        assert!(OpKind::Add.target_ok());
    }
}
