//! Type terms and effect sets.

use core::fmt;

use intern_lang::Symbol;

use crate::{
    id::{ExprId, List, PathId, TyId},
    lit::Prim,
    origin::Ident,
};

/// The effects a function, closure, or function type may perform.
///
/// Effect forms (`throw`, `await`, `yield`) are valid only inside a frame whose
/// effects allow them; see the spec, §8.9. New bits may be added in minor
/// versions.
///
/// # Examples
///
/// ```
/// use hir_lang::Effects;
///
/// let fx = Effects::THROWS.union(Effects::ASYNC);
/// assert!(fx.contains(Effects::ASYNC));
/// assert!(!fx.contains(Effects::YIELD));
/// assert!(Effects::NONE.is_empty());
/// ```
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Effects(u8);

impl Effects {
    /// No effects.
    pub const NONE: Self = Self(0);
    /// May throw (exceptions, or the language's error model).
    pub const THROWS: Self = Self(1);
    /// Asynchronous: may `await`; calling it yields an awaitable.
    pub const ASYNC: Self = Self(1 << 1);
    /// A generator: may `yield`.
    pub const YIELD: Self = Self(1 << 2);
    /// May perform unchecked operations (`unsafe`).
    pub const UNSAFE: Self = Self(1 << 3);

    /// Returns the union of two effect sets.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::Effects;
    ///
    /// assert!(Effects::NONE.union(Effects::UNSAFE).contains(Effects::UNSAFE));
    /// ```
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Returns `true` if every effect in `other` is in `self`.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::Effects;
    ///
    /// assert!(Effects::THROWS.contains(Effects::NONE));
    /// ```
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Returns `true` for the empty set.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::Effects;
    ///
    /// assert!(!Effects::YIELD.is_empty());
    /// ```
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Debug for Effects {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Effects(")?;
        let mut first = true;
        for (bit, name) in [
            (Self::THROWS, "throws"),
            (Self::ASYNC, "async"),
            (Self::YIELD, "yield"),
            (Self::UNSAFE, "unsafe"),
        ] {
            if self.contains(bit) {
                if !first {
                    f.write_str(" ")?;
                }
                f.write_str(name)?;
                first = false;
            }
        }
        f.write_str(")")
    }
}

/// A type term. Every annotation slot in HIR is optional, so a dynamic language
/// never creates one; see the spec, §5.
///
/// # Examples
///
/// ```
/// use hir_lang::{Builder, Prim, Ty};
///
/// let mut b = Builder::new();
/// let int = b.ty(Ty::Prim(Prim::I32));
/// let pair = b.list(&[int, int]);
/// let tuple = b.ty(Ty::Tuple(pair));
/// assert_ne!(int, tuple);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ty {
    /// An explicit hole (`_`) to be inferred.
    Infer,
    /// A primitive type.
    Prim(Prim),
    /// A named type; generic arguments live in the path's segments.
    Path(PathId),
    /// A tuple; the empty tuple is the unit type.
    Tuple(List<TyId>),
    /// A fixed-length array; `len` is an integer constant context.
    Array {
        /// Element type.
        elem: TyId,
        /// Length expression (no locals visible).
        len: ExprId,
    },
    /// A slice.
    Slice(TyId),
    /// A borrow; `region` is an optional `Region`-namespace path.
    Ref {
        /// `&mut` rather than `&`.
        mutable: bool,
        /// An explicit region (an override; Iron infers regions).
        region: Option<PathId>,
        /// The referent type.
        inner: TyId,
    },
    /// A raw pointer.
    Ptr {
        /// `*mut` rather than `*const`.
        mutable: bool,
        /// The pointee type.
        inner: TyId,
    },
    /// A function type.
    Fn {
        /// Parameter types.
        params: List<TyId>,
        /// Return type.
        ret: TyId,
        /// Declared effects.
        effects: Effects,
        /// The thrown type, if the language types its exceptions.
        throws: Option<TyId>,
        /// A foreign calling convention (`extern "C" fn`).
        abi: Option<Symbol>,
    },
    /// `T?`: the type or null.
    Nullable(TyId),
    /// The gradual dynamic type.
    Any,
    /// An interface (trait) object: `dyn A + B + 'r`.
    Object(List<Bound>),
    /// An opaque type known only by its bounds: `impl A + B`.
    Impl(List<Bound>),
    /// The type of expressions that never complete.
    Never,
    /// `Self` inside an interface, impl, or class.
    SelfTy,
    /// A type that failed to lower.
    Err,
}

/// A term in bound position: an interface type or a region.
///
/// Used by generic-parameter bounds, `where` predicates, associated-type
/// bounds, `dyn` and `impl` types, and associated-type constraints.
///
/// # Examples
///
/// ```
/// use hir_lang::{Bound, Builder, Ty};
///
/// let mut b = Builder::new();
/// let any = b.ty(Ty::Any);
/// assert_eq!(Bound::Ty(any), Bound::Ty(any));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Bound {
    /// An interface (or, as a `where` subject, any type).
    Ty(TyId),
    /// A region (`'a`, a `Region`-namespace path).
    Region(PathId),
}

/// A generic argument in a path segment or method call.
///
/// # Examples
///
/// ```
/// use hir_lang::{Builder, GenericArg, Prim, Ty};
///
/// let mut b = Builder::new();
/// let int = b.ty(Ty::Prim(Prim::I32));
/// assert!(matches!(GenericArg::Ty(int), GenericArg::Ty(_)));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GenericArg {
    /// A type: `Vec<i32>`.
    Ty(TyId),
    /// A constant (an integer constant context): `Array<3>`.
    Const(ExprId),
    /// A region: `Ref<'a>`.
    Region(PathId),
    /// An associated-type binding: `Iterator<Item = u8>`.
    Binding {
        /// The associated type.
        name: Ident,
        /// Its value.
        ty: TyId,
    },
    /// An associated-type constraint: `Iterator<Item: Show>`.
    Constraint {
        /// The associated type.
        name: Ident,
        /// Its bounds.
        bounds: List<Bound>,
    },
}
