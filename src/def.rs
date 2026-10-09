//! Compilation units and definition ids that cross them.
//!
//! A [`Hir`](crate::Hir) is one compilation unit (a file or module). Items in
//! other units are named by [`DefId`]s carrying the unit, so a local `ItemId`
//! can never be read against the wrong `Hir` by accident: resolutions name
//! definitions only through `DefId`, and a `DefId` that claims this unit but
//! was minted by a different `Hir` or builder is rejected.

use core::{
    cmp::Ordering,
    fmt,
    hash::{Hash, Hasher},
};

use crate::id::{ItemId, VariantId};

/// A compilation unit's identity, assigned by the host (the LexerSketch
/// engine numbers the files of a session).
///
/// The id must be unique among the units whose definitions refer to each
/// other, and stable across rebuilds of a unit, so that references from other
/// units stay valid when one unit is lowered again.
///
/// # Examples
///
/// ```
/// use hir_lang::UnitId;
///
/// assert_eq!(UnitId::new(7).as_u32(), 7);
/// assert_eq!(UnitId::default(), UnitId::new(0));
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnitId(u32);

impl UnitId {
    /// A unit id.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::UnitId;
    ///
    /// assert_ne!(UnitId::new(1), UnitId::new(2));
    /// ```
    #[must_use]
    pub const fn new(id: u32) -> Self {
        Self(id)
    }

    /// Returns the raw id.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::UnitId;
    ///
    /// assert_eq!(UnitId::new(3).as_u32(), 3);
    /// ```
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self.0
    }
}

/// The definition inside a unit that a [`DefId`] names: an item or a variant.
///
/// # Examples
///
/// ```
/// use hir_lang::{Def, ItemId};
///
/// let d = Def::Item(ItemId::from_index(0).unwrap());
/// assert!(matches!(d, Def::Item(_)));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Def {
    /// An item.
    Item(ItemId),
    /// A sum variant.
    Variant(VariantId),
}

/// A definition in some compilation unit.
///
/// Mint one for this unit with [`Builder::def`](crate::Builder::def) or
/// [`Hir::def`](crate::Hir::def); those carry a private tag of the issuing
/// builder or `Hir`, and a `DefId` claiming the same unit but carrying another
/// tag is rejected (it was minted for a different `Hir` of the same unit).
/// [`DefId::foreign`] makes an untagged id for a definition in another unit,
/// or for decoders. Equality, ordering, and hashing ignore the tag.
///
/// # Examples
///
/// ```
/// use hir_lang::{Def, DefId, ItemId, UnitId};
///
/// let other = DefId::foreign(UnitId::new(2), Def::Item(ItemId::from_index(4).unwrap()));
/// assert_eq!(other.unit(), UnitId::new(2));
/// ```
#[derive(Clone, Copy)]
pub struct DefId {
    unit: UnitId,
    def: Def,
    /// The issuing builder's or `Hir`'s tag; 0 when untagged.
    tag: u32,
}

impl DefId {
    /// An untagged id for `def` in `unit`: a definition in another unit (or one
    /// rebuilt by a decoder).
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Def, DefId, UnitId, VariantId};
    ///
    /// let v = DefId::foreign(UnitId::new(1), Def::Variant(VariantId::from_index(0).unwrap()));
    /// assert!(matches!(v.def(), Def::Variant(_)));
    /// ```
    #[must_use]
    pub const fn foreign(unit: UnitId, def: Def) -> Self {
        Self { unit, def, tag: 0 }
    }

    pub(crate) const fn tagged(unit: UnitId, def: Def, tag: u32) -> Self {
        Self { unit, def, tag }
    }

    /// Returns the unit the definition lives in.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Def, DefId, ItemId, UnitId};
    ///
    /// let d = DefId::foreign(UnitId::new(9), Def::Item(ItemId::from_index(0).unwrap()));
    /// assert_eq!(d.unit().as_u32(), 9);
    /// ```
    #[must_use]
    pub const fn unit(self) -> UnitId {
        self.unit
    }

    /// Returns the definition within its unit.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Def, DefId, ItemId, UnitId};
    ///
    /// let i = ItemId::from_index(1).unwrap();
    /// assert_eq!(DefId::foreign(UnitId::new(0), Def::Item(i)).def(), Def::Item(i));
    /// ```
    #[must_use]
    pub const fn def(self) -> Def {
        self.def
    }

    pub(crate) const fn tag(self) -> u32 {
        self.tag
    }
}

impl PartialEq for DefId {
    fn eq(&self, other: &Self) -> bool {
        self.unit == other.unit && self.def == other.def
    }
}

impl Eq for DefId {}

impl Hash for DefId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.unit.hash(state);
        self.def.hash(state);
    }
}

impl PartialOrd for DefId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for DefId {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.unit, self.def).cmp(&(other.unit, other.def))
    }
}

impl fmt::Debug for DefId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DefId({}, {:?})", self.unit.0, self.def)
    }
}

/// A process-unique tag for each builder, so `DefId`s minted for one `Hir`
/// cannot silently name items of another `Hir` of the same unit.
///
/// The counter is the crate's only global state: a single atomic, incremented
/// once per `Builder::new`, never reset. On targets without 32-bit atomics the
/// tag is always 0 and this check is disabled (documented fallback).
pub(crate) fn next_tag() -> u32 {
    #[cfg(target_has_atomic = "32")]
    {
        use core::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(1);
        // Skip 0 (the untagged value) if the counter ever wraps.
        loop {
            let tag = NEXT.fetch_add(1, Ordering::Relaxed);
            if tag != 0 {
                return tag;
            }
        }
    }
    #[cfg(not(target_has_atomic = "32"))]
    {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defid_equality_ignores_tag() {
        let i = Def::Item(ItemId::from_raw_index(3));
        let a = DefId::tagged(UnitId::new(1), i, 5);
        let b = DefId::tagged(UnitId::new(1), i, 9);
        assert_eq!(a, b);
        assert_eq!(a.cmp(&b), Ordering::Equal);
        assert_ne!(a, DefId::foreign(UnitId::new(2), i));
    }

    #[test]
    fn test_next_tag_is_never_zero_and_increases() {
        let a = next_tag();
        let b = next_tag();
        assert_ne!(a, 0);
        assert_ne!(a, b);
    }
}
