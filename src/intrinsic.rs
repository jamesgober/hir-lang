//! Inline assembly and the non-arithmetic intrinsics (atomics, volatile,
//! fences, and named backend intrinsics).
//!
//! These are not `specs/OPS.md` operations: they touch memory or the machine
//! directly. [`Intrinsic`] is `#[non_exhaustive]` and has a [`Intrinsic::Named`]
//! escape hatch, so new intrinsics are additive.

use intern_lang::Symbol;

use crate::id::{ExprId, List, TextRef};

/// A memory ordering (C++20 / LLVM semantics).
///
/// # Examples
///
/// ```
/// use hir_lang::MemOrder;
///
/// assert!(MemOrder::SeqCst > MemOrder::Relaxed);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MemOrder {
    /// No ordering beyond atomicity.
    Relaxed,
    /// Acquire (loads, fences, read-modify-writes).
    Acquire,
    /// Release (stores, fences, read-modify-writes).
    Release,
    /// Acquire and release (read-modify-writes, fences).
    AcqRel,
    /// Sequentially consistent.
    SeqCst,
}

/// The operation of an atomic read-modify-write.
///
/// # Examples
///
/// ```
/// use hir_lang::RmwOp;
///
/// assert_ne!(RmwOp::Add, RmwOp::Xchg);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RmwOp {
    /// Swap.
    Xchg,
    /// Wrapping add.
    Add,
    /// Wrapping subtract.
    Sub,
    /// Bitwise and.
    And,
    /// Bitwise or.
    Or,
    /// Bitwise xor.
    Xor,
    /// Bitwise nand.
    Nand,
    /// Minimum (signedness from the operand type).
    Min,
    /// Maximum.
    Max,
}

/// A non-arithmetic intrinsic; operands are `Expr::Intrinsic`'s `args`.
///
/// Pointer operands are addresses (`*T`); the validator checks arity and the
/// legality of each memory ordering.
///
/// # Examples
///
/// ```
/// use hir_lang::{Intrinsic, MemOrder};
///
/// assert_eq!(Intrinsic::AtomicLoad(MemOrder::Acquire).arity(), Some(1));
/// assert_eq!(Intrinsic::Fence(MemOrder::SeqCst).arity(), Some(0));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Intrinsic {
    /// `load(ptr)`; ordering not `Release` or `AcqRel`.
    AtomicLoad(MemOrder),
    /// `store(ptr, value)`; ordering not `Acquire` or `AcqRel`.
    AtomicStore(MemOrder),
    /// `rmw(ptr, value)` returning the old value.
    AtomicRmw(RmwOp, MemOrder),
    /// `cmpxchg(ptr, expected, new)` returning `(old, succeeded)`; the failure
    /// ordering is not `Release` or `AcqRel` and not stronger than success.
    AtomicCmpXchg {
        /// Ordering on success.
        success: MemOrder,
        /// Ordering on failure.
        failure: MemOrder,
    },
    /// A fence; ordering not `Relaxed`.
    Fence(MemOrder),
    /// `volatile_load(ptr)`.
    VolatileLoad,
    /// `volatile_store(ptr, value)`.
    VolatileStore,
    /// A backend- or host-defined intrinsic by name, any arity.
    Named(Symbol),
}

impl Intrinsic {
    /// Returns the number of operands, or `None` for a named intrinsic.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Intrinsic, MemOrder};
    ///
    /// let cas = Intrinsic::AtomicCmpXchg { success: MemOrder::SeqCst, failure: MemOrder::Relaxed };
    /// assert_eq!(cas.arity(), Some(3));
    /// ```
    #[must_use]
    pub const fn arity(self) -> Option<usize> {
        match self {
            Self::Fence(_) => Some(0),
            Self::AtomicLoad(_) | Self::VolatileLoad => Some(1),
            Self::AtomicStore(_) | Self::AtomicRmw(..) | Self::VolatileStore => Some(2),
            Self::AtomicCmpXchg { .. } => Some(3),
            Self::Named(_) => None,
        }
    }

    /// Returns `true` if the memory orderings are legal for this intrinsic.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Intrinsic, MemOrder};
    ///
    /// assert!(!Intrinsic::AtomicLoad(MemOrder::Release).orderings_ok());
    /// assert!(Intrinsic::AtomicStore(MemOrder::Release).orderings_ok());
    /// ```
    #[must_use]
    pub const fn orderings_ok(self) -> bool {
        use MemOrder::{AcqRel, Acquire, Relaxed, Release};
        match self {
            Self::AtomicLoad(o) => !matches!(o, Release | AcqRel),
            Self::AtomicStore(o) => !matches!(o, Acquire | AcqRel),
            Self::AtomicCmpXchg { success, failure } => {
                !matches!(failure, Release | AcqRel) && strength(failure) <= strength(success)
            }
            Self::Fence(o) => !matches!(o, Relaxed),
            _ => true,
        }
    }
}

/// The acquire strength of an ordering, for comparing cmpxchg orderings.
const fn strength(o: MemOrder) -> u8 {
    match o {
        MemOrder::Relaxed | MemOrder::Release => 0,
        MemOrder::Acquire | MemOrder::AcqRel => 1,
        MemOrder::SeqCst => 2,
    }
}

/// How an inline-assembly operand is used.
///
/// # Examples
///
/// ```
/// use hir_lang::AsmDir;
///
/// assert_ne!(AsmDir::In, AsmDir::Out);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AsmDir {
    /// An input value.
    In,
    /// An output; the expression is a place.
    Out,
    /// Read and written; the expression is a place.
    InOut,
    /// An assemble-time constant.
    Const,
    /// A symbol address; the expression is a path.
    Sym,
}

/// One inline-assembly operand: `in(reg) x`, `out("rax") y`.
///
/// # Examples
///
/// ```
/// use hir_lang::{AsmDir, AsmOperand, Builder};
///
/// let mut b = Builder::new();
/// let x = b.int(1);
/// let reg = b.text("reg");
/// let op = AsmOperand { dir: AsmDir::In, constraint: reg, expr: x };
/// assert_eq!(op.dir, AsmDir::In);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AsmOperand {
    /// The direction.
    pub dir: AsmDir,
    /// The register class or explicit register (UTF-8 text), checked by the
    /// target's assembler.
    pub constraint: TextRef,
    /// The operand expression.
    pub expr: ExprId,
}

/// Inline-assembly options, a bit set.
///
/// # Examples
///
/// ```
/// use hir_lang::AsmOptions;
///
/// let o = AsmOptions::NOSTACK.union(AsmOptions::NOMEM);
/// assert!(o.contains(AsmOptions::NOMEM));
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct AsmOptions(u8);

impl AsmOptions {
    /// No options.
    pub const NONE: Self = Self(0);
    /// No side effects; may be removed or merged.
    pub const PURE: Self = Self(1);
    /// Does not touch memory.
    pub const NOMEM: Self = Self(1 << 1);
    /// Only reads memory.
    pub const READONLY: Self = Self(1 << 2);
    /// Does not use the stack.
    pub const NOSTACK: Self = Self(1 << 3);
    /// Preserves the flags register.
    pub const PRESERVES_FLAGS: Self = Self(1 << 4);
    /// Never returns.
    pub const NORETURN: Self = Self(1 << 5);

    /// Returns the union of two option sets.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::AsmOptions;
    ///
    /// assert!(AsmOptions::NONE.union(AsmOptions::PURE).contains(AsmOptions::PURE));
    /// ```
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Returns `true` if every option in `other` is set.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::AsmOptions;
    ///
    /// assert!(!AsmOptions::NONE.contains(AsmOptions::NORETURN));
    /// ```
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// Structured inline assembly (Zero, Iron).
///
/// The template is UTF-8 text with `{N}` placeholders naming operands by
/// position (`{{` and `}}` are literal braces); the validator checks that every
/// placeholder names an operand and that output operands are places. The
/// instructions themselves are checked by the target's generated assembler.
///
/// # Examples
///
/// ```
/// use hir_lang::{Asm, AsmOptions, List};
///
/// let empty = Asm { template: hir_lang::TextRef::default(), operands: List::EMPTY, options: AsmOptions::NONE };
/// assert!(empty.operands.is_empty());
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Asm {
    /// The template text.
    pub template: TextRef,
    /// The operands, referenced as `{0}`, `{1}`, ...
    pub operands: List<AsmOperand>,
    /// Options.
    pub options: AsmOptions,
}

/// Checks an asm template: UTF-8 and every `{N}` below `operands`. Returns
/// `false` on an unbalanced brace, a non-numeric placeholder, or an index out
/// of range.
pub(crate) fn template_ok(text: &[u8], operands: usize) -> bool {
    let Ok(text) = core::str::from_utf8(text) else {
        return false;
    };
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes.get(i) {
            Some(b'{') if bytes.get(i + 1) == Some(&b'{') => i += 2,
            Some(b'}') if bytes.get(i + 1) == Some(&b'}') => i += 2,
            Some(b'{') => {
                let start = i + 1;
                let mut end = start;
                while bytes.get(end).is_some_and(u8::is_ascii_digit) {
                    end += 1;
                }
                if end == start || bytes.get(end) != Some(&b'}') {
                    return false;
                }
                let index = text.get(start..end).and_then(|d| d.parse::<usize>().ok());
                if index.is_none_or(|n| n >= operands) {
                    return false;
                }
                i = end + 1;
            }
            Some(b'}') => return false,
            _ => i += 1,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_template_placeholders() {
        assert!(template_ok(b"mov {0}, {1}", 2));
        assert!(!template_ok(b"mov {0}, {2}", 2));
        assert!(template_ok(b"{{literal}} {0}", 1));
        assert!(!template_ok(b"bad {x}", 1));
        assert!(!template_ok(b"open {0", 1));
        assert!(!template_ok(b"close }", 1));
        assert!(!template_ok(&[0xff], 0));
    }

    #[test]
    fn test_cmpxchg_ordering_rules() {
        use MemOrder::*;
        let ok = Intrinsic::AtomicCmpXchg {
            success: SeqCst,
            failure: Acquire,
        };
        assert!(ok.orderings_ok());
        let stronger = Intrinsic::AtomicCmpXchg {
            success: Relaxed,
            failure: Acquire,
        };
        assert!(!stronger.orderings_ok());
        assert!(!Intrinsic::Fence(Relaxed).orderings_ok());
    }
}
