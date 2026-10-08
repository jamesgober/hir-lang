//! Primitive types and literals.

use crate::id::TextRef;

/// A primitive type: the value domains of `specs/OPS.md` plus `isize`/`usize`
/// (target-width integers) and `str`.
///
/// # Examples
///
/// ```
/// use hir_lang::Prim;
///
/// assert!(Prim::I32.is_int() && Prim::I32.is_signed());
/// assert!(Prim::F64.is_float());
/// assert_eq!(Prim::U8.name(), "u8");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Prim {
    /// `bool`.
    Bool,
    /// `i8`.
    I8,
    /// `i16`.
    I16,
    /// `i32`.
    I32,
    /// `i64`.
    I64,
    /// The target-width signed integer (`i32` or `i64` per target).
    Isize,
    /// `u8`.
    U8,
    /// `u16`.
    U16,
    /// `u32`.
    U32,
    /// `u64`.
    U64,
    /// The target-width unsigned integer.
    Usize,
    /// IEEE 754 binary32.
    F32,
    /// IEEE 754 binary64.
    F64,
    /// A Unicode scalar value.
    Char,
    /// The language's string type (a runtime-library type).
    Str,
}

impl Prim {
    /// Returns `true` for the integer types, `isize`/`usize` included.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::Prim;
    ///
    /// assert!(Prim::Usize.is_int());
    /// assert!(!Prim::Bool.is_int());
    /// ```
    #[must_use]
    pub const fn is_int(self) -> bool {
        matches!(
            self,
            Self::I8
                | Self::I16
                | Self::I32
                | Self::I64
                | Self::Isize
                | Self::U8
                | Self::U16
                | Self::U32
                | Self::U64
                | Self::Usize
        )
    }

    /// Returns `true` for the signed integer types.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::Prim;
    ///
    /// assert!(Prim::I8.is_signed());
    /// assert!(!Prim::U8.is_signed());
    /// ```
    #[must_use]
    pub const fn is_signed(self) -> bool {
        matches!(
            self,
            Self::I8 | Self::I16 | Self::I32 | Self::I64 | Self::Isize
        )
    }

    /// Returns `true` for `f32` and `f64`.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::Prim;
    ///
    /// assert!(Prim::F32.is_float());
    /// assert!(!Prim::I64.is_float());
    /// ```
    #[must_use]
    pub const fn is_float(self) -> bool {
        matches!(self, Self::F32 | Self::F64)
    }

    /// Returns the bit width of a numeric type (`isize`/`usize` count as 64),
    /// or `0` for `bool`, `char`, and `str`.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::Prim;
    ///
    /// assert_eq!(Prim::I16.bits(), 16);
    /// assert_eq!(Prim::Char.bits(), 0);
    /// ```
    #[must_use]
    pub const fn bits(self) -> u32 {
        match self {
            Self::I8 | Self::U8 => 8,
            Self::I16 | Self::U16 => 16,
            Self::I32 | Self::U32 | Self::F32 => 32,
            Self::I64 | Self::U64 | Self::Isize | Self::Usize | Self::F64 => 64,
            Self::Bool | Self::Char | Self::Str => 0,
        }
    }

    /// Returns the type's spelling (`"i32"`, `"usize"`, `"str"`).
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::Prim;
    ///
    /// assert_eq!(Prim::Isize.name(), "isize");
    /// ```
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::I8 => "i8",
            Self::I16 => "i16",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::Isize => "isize",
            Self::U8 => "u8",
            Self::U16 => "u16",
            Self::U32 => "u32",
            Self::U64 => "u64",
            Self::Usize => "usize",
            Self::F32 => "f32",
            Self::F64 => "f64",
            Self::Char => "char",
            Self::Str => "str",
        }
    }
}

/// An integer literal: magnitude, sign, and optional type suffix.
///
/// Magnitude plus sign covers every `i64` and `u64` value. With a suffix the
/// value must fit that type (checked by the validator).
///
/// # Examples
///
/// ```
/// use hir_lang::{IntLit, Prim};
///
/// let minus_one = IntLit::signed(-1);
/// assert!(minus_one.negative && minus_one.value == 1);
/// let byte = IntLit::new(255).with_suffix(Prim::U8);
/// assert!(byte.fits(Prim::U8));
/// assert!(!IntLit::new(256).fits(Prim::U8));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct IntLit {
    /// The magnitude.
    pub value: u64,
    /// `true` for a negative literal (`-0` is allowed and equals `0`).
    pub negative: bool,
    /// The type suffix, if the literal had one (`1u8`); otherwise inferred.
    pub suffix: Option<Prim>,
}

impl IntLit {
    /// A non-negative literal without a suffix.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::IntLit;
    ///
    /// assert_eq!(IntLit::new(7).value, 7);
    /// ```
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self {
            value,
            negative: false,
            suffix: None,
        }
    }

    /// A literal from a signed value, without a suffix.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::IntLit;
    ///
    /// let min = IntLit::signed(i64::MIN);
    /// assert_eq!(min.value, 1 << 63);
    /// assert!(min.negative);
    /// ```
    #[must_use]
    pub const fn signed(value: i64) -> Self {
        Self {
            value: value.unsigned_abs(),
            negative: value < 0,
            suffix: None,
        }
    }

    /// Returns the literal with a type suffix.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{IntLit, Prim};
    ///
    /// assert_eq!(IntLit::new(1).with_suffix(Prim::I8).suffix, Some(Prim::I8));
    /// ```
    #[must_use]
    pub const fn with_suffix(mut self, suffix: Prim) -> Self {
        self.suffix = Some(suffix);
        self
    }

    /// Returns `true` if the value is representable in the integer type `ty`
    /// (`isize`/`usize` are treated as 64-bit). Non-integer types never fit.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{IntLit, Prim};
    ///
    /// assert!(IntLit::signed(-128).fits(Prim::I8));
    /// assert!(!IntLit::signed(-129).fits(Prim::I8));
    /// assert!(!IntLit::signed(-1).fits(Prim::U64));
    /// assert!(IntLit::signed(0).fits(Prim::U64));
    /// ```
    #[must_use]
    pub const fn fits(self, ty: Prim) -> bool {
        if !ty.is_int() {
            return false;
        }
        let bits = ty.bits();
        if ty.is_signed() {
            // Positive limit 2^(bits-1) - 1, negative limit 2^(bits-1).
            let limit = 1u64 << (bits - 1);
            if self.negative {
                self.value <= limit
            } else {
                self.value < limit
            }
        } else if self.negative {
            self.value == 0
        } else if bits == 64 {
            true
        } else {
            self.value < (1u64 << bits)
        }
    }
}

/// A floating-point literal, stored as IEEE binary64 bits so that literals
/// compare and hash exactly.
///
/// An `f32`-suffixed literal must hold a value exactly representable in `f32`
/// (the lowering rounds once; no tier rounds twice).
///
/// # Examples
///
/// ```
/// use hir_lang::{FloatLit, Prim};
///
/// let half = FloatLit::new(0.5);
/// assert_eq!(half.value(), 0.5);
/// assert!(FloatLit::new(0.5).with_suffix(Prim::F32).is_exact());
/// assert!(!FloatLit::new(0.1).with_suffix(Prim::F32).is_exact());
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FloatLit {
    /// The value's binary64 bit pattern.
    pub bits: u64,
    /// The type suffix (`f32` or `f64`), if any.
    pub suffix: Option<Prim>,
}

impl FloatLit {
    /// A literal without a suffix.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::FloatLit;
    ///
    /// assert_eq!(FloatLit::new(2.5).value(), 2.5);
    /// ```
    #[must_use]
    pub fn new(value: f64) -> Self {
        Self {
            bits: value.to_bits(),
            suffix: None,
        }
    }

    /// Returns the literal with a type suffix.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{FloatLit, Prim};
    ///
    /// assert_eq!(FloatLit::new(1.0).with_suffix(Prim::F64).suffix, Some(Prim::F64));
    /// ```
    #[must_use]
    pub const fn with_suffix(mut self, suffix: Prim) -> Self {
        self.suffix = Some(suffix);
        self
    }

    /// Returns the value.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::FloatLit;
    ///
    /// assert!(FloatLit::new(f64::NAN).value().is_nan());
    /// ```
    #[must_use]
    pub fn value(self) -> f64 {
        f64::from_bits(self.bits)
    }

    /// Returns `true` unless the literal has an `f32` suffix and a value that
    /// `f32` cannot hold exactly. NaN and infinities are exact.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{FloatLit, Prim};
    ///
    /// assert!(FloatLit::new(f64::INFINITY).with_suffix(Prim::F32).is_exact());
    /// assert!(FloatLit::new(0.1).is_exact());
    /// ```
    #[must_use]
    pub fn is_exact(self) -> bool {
        if self.suffix != Some(Prim::F32) {
            return true;
        }
        let v = self.value();
        // Rounding to f32 and back reproduces the value only when it is
        // representable; NaN never compares equal, so it is handled first.
        v.is_nan() || f64::from(v as f32) == v
    }
}

/// A literal value.
///
/// String, byte-string, and big-integer payloads live in the `Hir`'s text pool.
///
/// # Examples
///
/// ```
/// use hir_lang::{IntLit, Lit};
///
/// let lit = Lit::Int(IntLit::new(42));
/// assert!(matches!(lit, Lit::Int(IntLit { value: 42, .. })));
/// assert_eq!(Lit::Bool(true), Lit::Bool(true));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lit {
    /// The null/nil value of languages with nullability.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// An integer up to 64 bits.
    Int(IntLit),
    /// A floating-point number.
    Float(FloatLit),
    /// A Unicode scalar value.
    Char(char),
    /// A string; the payload must be valid UTF-8.
    Str(TextRef),
    /// A byte string.
    Bytes(TextRef),
    /// An integer beyond 64 bits, as decimal digits with an optional leading `-`
    /// (a runtime-library number in languages with bigints).
    BigInt(TextRef),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_int_fits_boundaries_each_width() {
        for (ty, max, min_mag) in [
            (Prim::I8, 127u64, 128u64),
            (Prim::I16, 32767, 32768),
            (Prim::I32, i32::MAX as u64, 1 << 31),
            (Prim::I64, i64::MAX as u64, 1 << 63),
        ] {
            assert!(IntLit::new(max).fits(ty));
            assert!(!IntLit::new(max + 1).fits(ty));
            let neg = IntLit {
                value: min_mag,
                negative: true,
                suffix: None,
            };
            assert!(neg.fits(ty));
            let too_neg = IntLit {
                value: min_mag + 1,
                negative: true,
                suffix: None,
            };
            assert!(!too_neg.fits(ty));
        }
        assert!(IntLit::new(u64::MAX).fits(Prim::U64));
        assert!(IntLit::new(u64::MAX).fits(Prim::Usize));
        assert!(IntLit::new(65535).fits(Prim::U16));
        assert!(!IntLit::new(65536).fits(Prim::U16));
        assert!(!IntLit::new(0).fits(Prim::F32));
    }

    #[test]
    fn test_negative_zero_fits_unsigned() {
        let neg_zero = IntLit {
            value: 0,
            negative: true,
            suffix: None,
        };
        assert!(neg_zero.fits(Prim::U8));
    }

    #[test]
    fn test_float_exactness() {
        assert!(FloatLit::new(1.5).with_suffix(Prim::F32).is_exact());
        assert!(!FloatLit::new(1e300).with_suffix(Prim::F32).is_exact());
        assert!(FloatLit::new(f64::NAN).with_suffix(Prim::F32).is_exact());
    }
}
