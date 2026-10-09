//! The flat storage shared by [`Builder`](crate::Builder) and [`Hir`](crate::Hir).
//!
//! Every node kind has an arena (nodes plus a parallel origin array) and every
//! list element type has a pool. Nothing here owns a per-node allocation, so
//! clone, compare, debug-print, and drop are flat loops at any nesting depth.

use alloc::vec::Vec;

use crate::{
    expr::{Arg, Arm, Capture, Expr, FieldInit, MapEntry, Stmt},
    id::{
        BinderId, ExprId, FieldId, ItemId, List, NodeRef, ParamId, PatId, PathId, StmtId, TyId,
        VariantId,
    },
    intrinsic::AsmOperand,
    item::{Attr, AttrArg, FieldDef, GenericParam, Item, MixinRule, Param, Variant, WherePred},
    name::{Binder, Path, Segment},
    origin::{Expansion, Origin},
    pat::{FieldPat, Pat},
    ty::{Bound, GenericArg, Ty},
};

/// Nodes of one kind plus their origins, index-aligned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Arena<T> {
    pub(crate) nodes: Vec<T>,
    pub(crate) origins: Vec<Origin>,
}

impl<T> Default for Arena<T> {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            origins: Vec::new(),
        }
    }
}

impl<T> Arena<T> {
    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.nodes.len()
    }

    #[inline]
    pub(crate) fn get(&self, index: usize) -> Option<&T> {
        self.nodes.get(index)
    }

    #[inline]
    pub(crate) fn origin(&self, index: usize) -> Origin {
        self.origins.get(index).copied().unwrap_or_default()
    }
}

/// All arenas, pools, the text pool, the expansion table, and the attribute
/// table of one HIR.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Store {
    pub(crate) items: Arena<Item>,
    pub(crate) exprs: Arena<Expr>,
    pub(crate) stmts: Arena<Stmt>,
    pub(crate) pats: Arena<Pat>,
    pub(crate) tys: Arena<Ty>,
    pub(crate) paths: Arena<Path>,
    pub(crate) fields: Arena<FieldDef>,
    pub(crate) variants: Arena<Variant>,
    pub(crate) params: Arena<Param>,
    pub(crate) binders: Arena<Binder>,
    pub(crate) expansions: Vec<Expansion>,
    pub(crate) text: Vec<u8>,
    /// Sorted by target, one entry per target.
    pub(crate) attrs: Vec<(NodeRef, List<Attr>)>,
    pub(crate) pool_expr: Vec<ExprId>,
    pub(crate) pool_pat: Vec<PatId>,
    pub(crate) pool_ty: Vec<TyId>,
    pub(crate) pool_stmt: Vec<StmtId>,
    pub(crate) pool_item: Vec<ItemId>,
    pub(crate) pool_field: Vec<FieldId>,
    pub(crate) pool_variant: Vec<VariantId>,
    pub(crate) pool_param: Vec<ParamId>,
    pub(crate) pool_arg: Vec<Arg>,
    pub(crate) pool_field_init: Vec<FieldInit>,
    pub(crate) pool_map_entry: Vec<MapEntry>,
    pub(crate) pool_arm: Vec<Arm>,
    pub(crate) pool_capture: Vec<Capture>,
    pub(crate) pool_generic: Vec<GenericParam>,
    pub(crate) pool_where: Vec<WherePred>,
    pub(crate) pool_segment: Vec<Segment>,
    pub(crate) pool_field_pat: Vec<FieldPat>,
    pub(crate) pool_attr: Vec<Attr>,
    pub(crate) pool_attr_arg: Vec<AttrArg>,
    pub(crate) pool_generic_arg: Vec<GenericArg>,
    pub(crate) pool_bound: Vec<Bound>,
    pub(crate) pool_asm_operand: Vec<AsmOperand>,
    pub(crate) pool_mixin_rule: Vec<MixinRule>,
}

impl Store {
    /// Returns the elements of `list`, or `None` if its range is outside the pool.
    #[inline]
    pub(crate) fn try_list<T: Pooled>(&self, list: List<T>) -> Option<&[T]> {
        T::pool(self).get(list.range()?)
    }

    /// Returns the elements of `list`, or nothing if its range is outside the
    /// pool. On a validated store every list is in range.
    #[inline]
    pub(crate) fn list<T: Pooled>(&self, list: List<T>) -> &[T] {
        self.try_list(list).unwrap_or(&[])
    }

    /// Copies `elems` into their pool; `None` if the pool would outgrow
    /// `u32` indexes.
    pub(crate) fn push_list<T: Pooled>(&mut self, elems: &[T]) -> Option<List<T>> {
        if elems.is_empty() {
            return Some(List::EMPTY);
        }
        let pool = T::pool_mut(self);
        let start = pool.len();
        let end = start.checked_add(elems.len())?;
        if end > crate::id::MAX_LEN {
            return None;
        }
        let list = List::from_raw(u32::try_from(start).ok()?, u32::try_from(elems.len()).ok()?);
        pool.extend_from_slice(elems);
        Some(list)
    }

    #[inline]
    pub(crate) fn item(&self, id: ItemId) -> Option<&Item> {
        self.items.get(id.index())
    }

    #[inline]
    pub(crate) fn expr(&self, id: ExprId) -> Option<&Expr> {
        self.exprs.get(id.index())
    }

    #[inline]
    pub(crate) fn stmt(&self, id: StmtId) -> Option<&Stmt> {
        self.stmts.get(id.index())
    }

    #[inline]
    pub(crate) fn pat(&self, id: PatId) -> Option<&Pat> {
        self.pats.get(id.index())
    }

    #[inline]
    pub(crate) fn ty(&self, id: TyId) -> Option<&Ty> {
        self.tys.get(id.index())
    }

    #[inline]
    pub(crate) fn path(&self, id: PathId) -> Option<&Path> {
        self.paths.get(id.index())
    }

    #[inline]
    pub(crate) fn field(&self, id: FieldId) -> Option<&FieldDef> {
        self.fields.get(id.index())
    }

    #[inline]
    pub(crate) fn variant(&self, id: VariantId) -> Option<&Variant> {
        self.variants.get(id.index())
    }

    #[inline]
    pub(crate) fn param(&self, id: ParamId) -> Option<&Param> {
        self.params.get(id.index())
    }

    #[inline]
    pub(crate) fn binder(&self, id: BinderId) -> Option<&Binder> {
        self.binders.get(id.index())
    }

    /// Returns the number of nodes in the arena `node` lives in.
    pub(crate) fn arena_len(&self, node: NodeRef) -> usize {
        match node {
            NodeRef::Item(_) => self.items.len(),
            NodeRef::Expr(_) => self.exprs.len(),
            NodeRef::Stmt(_) => self.stmts.len(),
            NodeRef::Pat(_) => self.pats.len(),
            NodeRef::Ty(_) => self.tys.len(),
            NodeRef::Path(_) => self.paths.len(),
            NodeRef::Field(_) => self.fields.len(),
            NodeRef::Variant(_) => self.variants.len(),
            NodeRef::Param(_) => self.params.len(),
        }
    }

    /// Returns the origin of `node` (the default origin for a foreign id).
    pub(crate) fn origin(&self, node: NodeRef) -> Origin {
        let i = node.index();
        match node {
            NodeRef::Item(_) => self.items.origin(i),
            NodeRef::Expr(_) => self.exprs.origin(i),
            NodeRef::Stmt(_) => self.stmts.origin(i),
            NodeRef::Pat(_) => self.pats.origin(i),
            NodeRef::Ty(_) => self.tys.origin(i),
            NodeRef::Path(_) => self.paths.origin(i),
            NodeRef::Field(_) => self.fields.origin(i),
            NodeRef::Variant(_) => self.variants.origin(i),
            NodeRef::Param(_) => self.params.origin(i),
        }
    }

    /// Returns the attributes attached to `node` (binary search; the table is
    /// sorted with one entry per target).
    pub(crate) fn attrs(&self, node: NodeRef) -> &[Attr] {
        match self.attrs.binary_search_by(|(target, _)| target.cmp(&node)) {
            Ok(at) => self
                .attrs
                .get(at)
                .map_or(&[][..], |(_, list)| self.list(*list)),
            Err(_) => &[],
        }
    }
}

mod sealed {
    /// Ties each list element type to its pool. Not implementable outside the
    /// crate.
    pub trait Sealed: Sized {
        fn pool(store: &super::Store) -> &[Self];
        fn pool_mut(store: &mut super::Store) -> &mut alloc::vec::Vec<Self>;
    }
}

/// An element type that lives in one of the `Hir`'s pools and can therefore
/// appear in a [`List`].
///
/// Implemented for every id type that appears in lists and for the list record
/// types ([`Arg`], [`Arm`], [`Segment`], [`GenericArg`], [`Bound`], ...). Sealed: the set of pools is part
/// of the HIR definition.
///
/// # Examples
///
/// ```
/// use hir_lang::{Builder, ExprId, List, Pooled};
///
/// fn count<T: Pooled>(list: List<T>) -> usize {
///     list.len()
/// }
///
/// let mut b = Builder::new();
/// let one = b.int(1);
/// let list: List<ExprId> = b.list(&[one]);
/// assert_eq!(count(list), 1);
/// ```
pub trait Pooled: sealed::Sealed + Copy {}

macro_rules! pooled {
    ($($ty:ty => $field:ident),* $(,)?) => {
        $(
            impl sealed::Sealed for $ty {
                #[inline]
                fn pool(store: &Store) -> &[Self] {
                    &store.$field
                }
                #[inline]
                fn pool_mut(store: &mut Store) -> &mut Vec<Self> {
                    &mut store.$field
                }
            }
            impl Pooled for $ty {}
        )*
    };
}

pooled! {
    ExprId => pool_expr,
    PatId => pool_pat,
    TyId => pool_ty,
    StmtId => pool_stmt,
    ItemId => pool_item,
    FieldId => pool_field,
    VariantId => pool_variant,
    ParamId => pool_param,
    Arg => pool_arg,
    FieldInit => pool_field_init,
    MapEntry => pool_map_entry,
    Arm => pool_arm,
    Capture => pool_capture,
    GenericParam => pool_generic,
    WherePred => pool_where,
    Segment => pool_segment,
    FieldPat => pool_field_pat,
    Attr => pool_attr,
    AttrArg => pool_attr_arg,
    GenericArg => pool_generic_arg,
    Bound => pool_bound,
    AsmOperand => pool_asm_operand,
    MixinRule => pool_mixin_rule,
}
