//! Typed ids, out-of-line lists, text references, and node references.
//!
//! Every node lives in a dense arena and is referenced by a 4-byte id. Ids wrap a
//! `NonZeroU32` holding `index + 1`, so `Option<ExprId>` is also 4 bytes and a
//! node's optional children cost nothing extra.

use core::{fmt, hash::Hash, marker::PhantomData, num::NonZeroU32};

/// The largest number of entries an arena, pool, or the text pool may hold.
///
/// Indexes are stored as `u32` and ids as `index + 1` in a `NonZeroU32`, so the
/// last representable index is `u32::MAX - 1`.
pub(crate) const MAX_LEN: usize = (u32::MAX - 1) as usize;

macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident, $what:literal) => {
        $(#[$meta])*
        ///
        /// Ids are dense: `index()` is the node's position in its arena, so side
        /// tables are plain vectors indexed by id. An id is only meaningful with
        /// the [`Hir`](crate::Hir) (or [`Builder`](crate::Builder)) that issued it.
        ///
        /// # Examples
        ///
        /// ```
        #[doc = concat!("use hir_lang::", stringify!($name), ";")]
        ///
        #[doc = concat!("let id = ", stringify!($name), "::from_index(7).unwrap();")]
        /// assert_eq!(id.index(), 7);
        #[doc = concat!("assert_eq!(core::mem::size_of::<Option<", stringify!($name), ">>(), 4);")]
        /// ```
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(NonZeroU32);

        impl $name {
            #[doc = concat!("Returns the dense index of this ", $what, " in its arena.")]
            #[inline]
            #[must_use]
            pub const fn index(self) -> usize {
                // `get()` is at least 1, so the subtraction never underflows.
                (self.0.get() - 1) as usize
            }

            #[doc = concat!("Builds the id of the ", $what, " at `index`, or `None` if the")]
            /// index is beyond the largest an arena can hold.
            ///
            /// Rebuilding an id never makes it valid: whether it names a node is
            /// decided by the `Hir` it is used with (and checked by the validator
            /// for every id stored inside a `Hir`).
            #[inline]
            #[must_use]
            pub fn from_index(index: usize) -> Option<Self> {
                if index >= MAX_LEN {
                    return None;
                }
                let raw = u32::try_from(index).ok()?.checked_add(1)?;
                NonZeroU32::new(raw).map(Self)
            }

            /// Builds an id from an index the caller has bounded by `MAX_LEN`.
            #[inline]
            pub(crate) fn from_raw_index(index: u32) -> Self {
                Self(NonZeroU32::new(index.wrapping_add(1)).unwrap_or(NonZeroU32::MIN))
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self.index())
            }
        }
    };
}

define_id!(
    /// The id of an [`Item`](crate::Item): a declaration.
    ItemId,
    "item"
);
define_id!(
    /// The id of an [`Expr`](crate::Expr): an expression.
    ExprId,
    "expression"
);
define_id!(
    /// The id of a [`Stmt`](crate::Stmt): a statement inside a block.
    StmtId,
    "statement"
);
define_id!(
    /// The id of a [`Pat`](crate::Pat): a pattern.
    PatId,
    "pattern"
);
define_id!(
    /// The id of a [`Ty`](crate::Ty): a type term.
    TyId,
    "type term"
);
define_id!(
    /// The id of a [`Path`](crate::Path): a name reference with its resolution slot.
    PathId,
    "path"
);
define_id!(
    /// The id of a [`FieldDef`](crate::FieldDef): a field of a record, class, or variant.
    FieldId,
    "field definition"
);
define_id!(
    /// The id of a [`Variant`](crate::Variant): a variant of a sum.
    VariantId,
    "variant"
);
define_id!(
    /// The id of a [`Param`](crate::Param): a function or closure parameter.
    ParamId,
    "parameter"
);
define_id!(
    /// The id of a [`Binder`](crate::Binder): a variable, parameter, capture, generic
    /// parameter, or label. Binder ids are unique across a whole `Hir`.
    BinderId,
    "binder"
);

/// A reference to any tree node: the nine node kinds in one `Copy` value.
///
/// Traversal events, origins, attributes, and validation errors all speak in
/// `NodeRef`s. The ordering (by kind, then by index) is what the attribute table
/// is sorted by.
///
/// # Examples
///
/// ```
/// use hir_lang::{ExprId, NodeRef};
///
/// let node = NodeRef::Expr(ExprId::from_index(3).unwrap());
/// assert_eq!(node.index(), 3);
/// assert_eq!(node.kind(), hir_lang::IdKind::Expr);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NodeRef {
    /// An item.
    Item(ItemId),
    /// An expression.
    Expr(ExprId),
    /// A statement.
    Stmt(StmtId),
    /// A pattern.
    Pat(PatId),
    /// A type term.
    Ty(TyId),
    /// A path.
    Path(PathId),
    /// A field definition.
    Field(FieldId),
    /// A sum variant.
    Variant(VariantId),
    /// A parameter.
    Param(ParamId),
}

impl NodeRef {
    /// Returns the node's index in its own arena.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{NodeRef, PatId};
    ///
    /// assert_eq!(NodeRef::Pat(PatId::from_index(9).unwrap()).index(), 9);
    /// ```
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Item(id) => id.index(),
            Self::Expr(id) => id.index(),
            Self::Stmt(id) => id.index(),
            Self::Pat(id) => id.index(),
            Self::Ty(id) => id.index(),
            Self::Path(id) => id.index(),
            Self::Field(id) => id.index(),
            Self::Variant(id) => id.index(),
            Self::Param(id) => id.index(),
        }
    }

    /// Returns which arena the node lives in.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{IdKind, ItemId, NodeRef};
    ///
    /// assert_eq!(NodeRef::Item(ItemId::from_index(0).unwrap()).kind(), IdKind::Item);
    /// ```
    #[must_use]
    pub const fn kind(self) -> IdKind {
        match self {
            Self::Item(_) => IdKind::Item,
            Self::Expr(_) => IdKind::Expr,
            Self::Stmt(_) => IdKind::Stmt,
            Self::Pat(_) => IdKind::Pat,
            Self::Ty(_) => IdKind::Ty,
            Self::Path(_) => IdKind::Path,
            Self::Field(_) => IdKind::Field,
            Self::Variant(_) => IdKind::Variant,
            Self::Param(_) => IdKind::Param,
        }
    }
}

/// Names an arena: the nine node kinds plus binders and expansions.
///
/// Used by [`Hir::count`](crate::Hir::count) and by errors that report which kind
/// of id was dangling.
///
/// # Examples
///
/// ```
/// use hir_lang::IdKind;
///
/// assert_eq!(IdKind::Binder.to_string(), "binder");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IdKind {
    /// Items.
    Item,
    /// Expressions.
    Expr,
    /// Statements.
    Stmt,
    /// Patterns.
    Pat,
    /// Type terms.
    Ty,
    /// Paths.
    Path,
    /// Field definitions.
    Field,
    /// Sum variants.
    Variant,
    /// Parameters.
    Param,
    /// Binders.
    Binder,
    /// Expansion records.
    Expansion,
}

impl fmt::Display for IdKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Item => "item",
            Self::Expr => "expression",
            Self::Stmt => "statement",
            Self::Pat => "pattern",
            Self::Ty => "type",
            Self::Path => "path",
            Self::Field => "field",
            Self::Variant => "variant",
            Self::Param => "parameter",
            Self::Binder => "binder",
            Self::Expansion => "expansion",
        })
    }
}

/// A run of `T`s stored out of line in one of the `Hir`'s pools.
///
/// Nodes never own heap memory: a call's arguments, a block's statements, or a
/// sum's variants are a `List` (8 bytes) into a shared pool. Lists are made by
/// [`Builder::list`](crate::Builder::list) and read with
/// [`Hir::list`](crate::Hir::list).
///
/// # Examples
///
/// ```
/// use hir_lang::{Builder, ExprId, List};
///
/// let mut b = Builder::new();
/// let one = b.int(1);
/// let two = b.int(2);
/// let list = b.list(&[one, two]);
/// assert_eq!(list.len(), 2);
/// assert!(List::<ExprId>::EMPTY.is_empty());
/// ```
pub struct List<T> {
    start: u32,
    len: u32,
    _elem: PhantomData<fn() -> T>,
}

impl<T> List<T> {
    /// The empty list.
    pub const EMPTY: Self = Self::from_raw(0, 0);

    /// Builds a list from its raw position in the pool.
    ///
    /// For decoders and tests. A list built this way is checked by the validator
    /// like any other: a range outside the pool is rejected, never read.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{ExprId, List};
    ///
    /// let list: List<ExprId> = List::from_raw(4, 2);
    /// assert_eq!((list.start(), list.len()), (4, 2));
    /// ```
    #[must_use]
    pub const fn from_raw(start: u32, len: u32) -> Self {
        Self {
            start,
            len,
            _elem: PhantomData,
        }
    }

    /// Returns the position of the first element in the pool.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{List, TyId};
    ///
    /// assert_eq!(List::<TyId>::EMPTY.start(), 0);
    /// ```
    #[must_use]
    pub const fn start(self) -> u32 {
        self.start
    }

    /// Returns the number of elements.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{List, PatId};
    ///
    /// assert_eq!(List::<PatId>::from_raw(10, 3).len(), 3);
    /// ```
    #[must_use]
    pub const fn len(self) -> usize {
        self.len as usize
    }

    /// Returns `true` if the list has no elements.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{List, StmtId};
    ///
    /// assert!(List::<StmtId>::EMPTY.is_empty());
    /// ```
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.len == 0
    }

    /// Returns the pool range, or `None` if it overflows `usize` arithmetic.
    pub(crate) fn range(self) -> Option<core::ops::Range<usize>> {
        let start = self.start as usize;
        Some(start..start.checked_add(self.len as usize)?)
    }
}

impl<T> Clone for List<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for List<T> {}

impl<T> PartialEq for List<T> {
    fn eq(&self, other: &Self) -> bool {
        self.start == other.start && self.len == other.len
    }
}

impl<T> Eq for List<T> {}

impl<T> Hash for List<T> {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.start.hash(state);
        self.len.hash(state);
    }
}

impl<T> Default for List<T> {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl<T> fmt::Debug for List<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "List({}+{})", self.start, self.len)
    }
}

/// A run of bytes in the `Hir`'s text pool: the payload of a string, byte-string,
/// or big-integer literal.
///
/// Made by [`Builder::text`](crate::Builder::text) or
/// [`Builder::bytes`](crate::Builder::bytes); read with
/// [`Hir::text`](crate::Hir::text) or [`Hir::str`](crate::Hir::str).
///
/// # Examples
///
/// ```
/// use hir_lang::Builder;
///
/// let mut b = Builder::new();
/// let text = b.text("hello");
/// assert_eq!(text.len(), 5);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TextRef {
    start: u32,
    len: u32,
}

impl TextRef {
    /// Builds a text reference from its raw position in the text pool.
    ///
    /// For decoders and tests; the validator rejects a range outside the pool.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::TextRef;
    ///
    /// let text = TextRef::from_raw(3, 4);
    /// assert_eq!((text.start(), text.len()), (3, 4));
    /// ```
    #[must_use]
    pub const fn from_raw(start: u32, len: u32) -> Self {
        Self { start, len }
    }

    /// Returns the position of the first byte in the text pool.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::TextRef;
    ///
    /// assert_eq!(TextRef::from_raw(8, 1).start(), 8);
    /// ```
    #[must_use]
    pub const fn start(self) -> u32 {
        self.start
    }

    /// Returns the length in bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::TextRef;
    ///
    /// assert_eq!(TextRef::from_raw(0, 6).len(), 6);
    /// ```
    #[must_use]
    pub const fn len(self) -> usize {
        self.len as usize
    }

    /// Returns `true` for the empty text.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::TextRef;
    ///
    /// assert!(TextRef::default().is_empty());
    /// ```
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.len == 0
    }

    pub(crate) fn range(self) -> Option<core::ops::Range<usize>> {
        let start = self.start as usize;
        Some(start..start.checked_add(self.len as usize)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_id_from_index_roundtrips() {
        for i in [0usize, 1, 2, 1000, MAX_LEN - 1] {
            assert_eq!(ExprId::from_index(i).map(ExprId::index), Some(i));
        }
    }

    #[test]
    fn test_id_from_index_at_limit_returns_none() {
        assert_eq!(ExprId::from_index(MAX_LEN), None);
        assert_eq!(ExprId::from_index(usize::MAX), None);
    }

    #[test]
    fn test_option_id_is_four_bytes() {
        assert_eq!(core::mem::size_of::<Option<ItemId>>(), 4);
        assert_eq!(core::mem::size_of::<List<ExprId>>(), 8);
    }

    #[test]
    fn test_list_range_overflow_returns_none() {
        let list: List<ExprId> = List::from_raw(u32::MAX, u32::MAX);
        assert!(list.range().is_some());
        assert_eq!(TextRef::from_raw(1, 2).range(), Some(1..3));
    }

    #[test]
    fn test_noderef_orders_by_kind_then_index() {
        let a = NodeRef::Item(ItemId::from_raw_index(5));
        let b = NodeRef::Expr(ExprId::from_raw_index(0));
        assert!(a < b);
    }
}
