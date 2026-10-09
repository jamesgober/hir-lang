//! The validated HIR and its read API.

use alloc::{collections::BTreeSet, vec::Vec};
use core::fmt;

use span_lang::Span;

use crate::{
    def::{Def, DefId, UnitId},
    error::{HirError, Site},
    expr::{Expr, Stmt},
    id::{
        BinderId, ExprId, FieldId, IdKind, ItemId, List, NodeRef, ParamId, PatId, PathId, StmtId,
        TextRef, TyId, VariantId,
    },
    item::{Attr, FieldDef, Item, ItemKind, Param, ParamKind, Variant, Vis},
    name::{Binder, Ns, Path, PathRoot, Res},
    origin::{Expansion, ExpnId, Name, Origin},
    pat::Pat,
    store::{Pooled, Store},
    ty::Ty,
    validate::{Ctx, Index, check_local, check_res, lookup_local},
    walk::{Control, Event, Frame, children_store, walk_store},
};

static ERR_ITEM: Item = Item {
    name: None,
    name_span: Span::empty(0),
    vis: Vis::Private,
    kind: ItemKind::Err,
};
static ERR_EXPR: Expr = Expr::Err;
static ERR_STMT: Stmt = Stmt::Err;
static ERR_PAT: Pat = Pat::Err;
static ERR_TY: Ty = Ty::Err;
static ERR_PATH: Path = Path {
    segments: List::EMPTY,
    ns: Ns::Value,
    root: PathRoot::Relative,
    qself: None,
    res: Res::Err,
    unresolved: 0,
};
static EMPTY_FIELD: FieldDef = FieldDef {
    name: None,
    vis: Vis::Private,
    ty: None,
    default: None,
};
static ERR_PARAM: Param = Param {
    pat: PatId::DANGLING,
    ty: None,
    default: None,
    kind: ParamKind::Normal,
    by_ref: false,
};

/// The builder's tag, carried by a `Hir` to recognize its own `DefId`s.
/// Excluded from equality and debug output: two builds of the same input are
/// equal and print the same.
#[derive(Clone, Copy)]
pub(crate) struct Tag(pub(crate) u32);

impl PartialEq for Tag {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for Tag {}

impl fmt::Debug for Tag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Tag")
    }
}

/// A validated HIR: one compilation unit rooted at a module.
///
/// The only ways to obtain one are [`Builder::finish`](crate::Builder::finish),
/// [`Builder::finish_lenient`](crate::Builder::finish_lenient) (and, from 0.5,
/// the decoder), all of which run the validator, so every `Hir` satisfies the
/// contract in the spec, §13: every id resolves, the live nodes form one tree,
/// binders are bound once and referenced only in scope, jumps have targets,
/// effects are placed where allowed, and every op carries exactly its policy.
/// The only mutations, [`resolve`](Self::resolve) and
/// [`resolve_partial`](Self::resolve_partial), check each change and keep
/// that contract.
///
/// Storage is flat (arenas of `Copy` nodes, out-of-line lists), so `Clone`,
/// `PartialEq`, `Debug`, and `Drop` never recurse, at any depth.
///
/// Accessors take ids issued for this `Hir`. The total accessors (`item`,
/// `expr`, ...) never panic in release builds: a foreign id reads as the
/// kind's error form (and trips a debug assertion in debug builds). The
/// `get_*` accessors return `None` for a foreign id instead.
///
/// # Examples
///
/// ```
/// use hir_lang::{Builder, Expr, Lit, IntLit};
///
/// let mut b = Builder::new();
/// let answer = b.int(42);
/// let body = b.block(&[], Some(answer));
/// let main = b.func(hir_lang::Name::new(intern_lang::Interner::new().intern("main")), &[], body);
/// let root = b.module(None, &[main]);
/// let hir = b.finish(root)?;
///
/// assert_eq!(hir.root(), root);
/// assert_eq!(hir.expr(answer), &Expr::Lit(Lit::Int(IntLit::new(42))));
/// # Ok::<(), hir_lang::HirError>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hir {
    store: Store,
    root: ItemId,
    unit: UnitId,
    tag: Tag,
    index: Index,
}

macro_rules! total {
    ($(#[$meta:meta])* $name:ident, $get:ident, $id:ty, $out:ty, $err:expr) => {
        $(#[$meta])*
        #[must_use]
        pub fn $name(&self, id: $id) -> &$out {
            let node = self.store.$name(id);
            debug_assert!(node.is_some(), "foreign id passed to Hir::{}", stringify!($name));
            node.unwrap_or($err)
        }

        #[doc = concat!("Like [`", stringify!($name), "`](Self::", stringify!($name), "), but `None` for an id this `Hir` did not issue.")]
        ///
        /// # Examples
        ///
        /// ```
        /// use hir_lang::Builder;
        ///
        /// let mut b = Builder::new();
        /// let root = b.module(None, &[]);
        /// let hir = b.finish(root)?;
        #[doc = concat!("assert!(hir.", stringify!($get), "(hir_lang::", stringify!($id), "::from_index(1000).unwrap()).is_none());")]
        /// # Ok::<(), hir_lang::HirError>(())
        /// ```
        #[must_use]
        pub fn $get(&self, id: $id) -> Option<&$out> {
            self.store.$name(id)
        }
    };
}

impl Hir {
    pub(crate) fn from_parts(
        store: Store,
        root: ItemId,
        unit: UnitId,
        tag: u32,
        index: Index,
    ) -> Self {
        Self {
            store,
            root,
            unit,
            tag: Tag(tag),
            index,
        }
    }

    pub(crate) fn store(&self) -> &Store {
        &self.store
    }

    pub(crate) fn ctx(&self) -> Ctx {
        Ctx {
            unit: self.unit,
            tag: self.tag.0,
        }
    }

    /// Returns the root module.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::Builder;
    ///
    /// let mut b = Builder::new();
    /// let root = b.module(None, &[]);
    /// assert_eq!(b.finish(root)?.root(), root);
    /// # Ok::<(), hir_lang::HirError>(())
    /// ```
    #[must_use]
    pub fn root(&self) -> ItemId {
        self.root
    }

    /// Returns the compilation unit this `Hir` is.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, UnitId};
    ///
    /// let mut b = Builder::for_unit(UnitId::new(4));
    /// let root = b.module(None, &[]);
    /// assert_eq!(b.finish(root)?.unit(), UnitId::new(4));
    /// # Ok::<(), hir_lang::HirError>(())
    /// ```
    #[must_use]
    pub fn unit(&self) -> UnitId {
        self.unit
    }

    /// Returns the `DefId` naming a definition of this unit, for resolutions
    /// here or in other units' `Hir`s.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Def, UnitId};
    ///
    /// let mut b = Builder::for_unit(UnitId::new(1));
    /// let root = b.module(None, &[]);
    /// let hir = b.finish(root)?;
    /// assert_eq!(hir.def(Def::Item(root)).unit(), UnitId::new(1));
    /// # Ok::<(), hir_lang::HirError>(())
    /// ```
    #[must_use]
    pub fn def(&self, def: Def) -> DefId {
        DefId::tagged(self.unit, def, self.tag.0)
    }

    /// Returns the number of entries in an arena.
    ///
    /// Ids of that kind are exactly `0..count`, so side tables (types per
    /// expression, slots per binder) are vectors of this length. Arenas may
    /// hold dead error nodes left by lenient repairs; they are not in the tree.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, IdKind};
    ///
    /// let mut b = Builder::new();
    /// let root = b.module(None, &[]);
    /// let hir = b.finish(root)?;
    /// assert_eq!(hir.count(IdKind::Item), 1);
    /// assert_eq!(hir.count(IdKind::Expr), 0);
    /// # Ok::<(), hir_lang::HirError>(())
    /// ```
    #[must_use]
    pub fn count(&self, kind: IdKind) -> usize {
        let s = &self.store;
        match kind {
            IdKind::Item => s.items.len(),
            IdKind::Expr => s.exprs.len(),
            IdKind::Stmt => s.stmts.len(),
            IdKind::Pat => s.pats.len(),
            IdKind::Ty => s.tys.len(),
            IdKind::Path => s.paths.len(),
            IdKind::Field => s.fields.len(),
            IdKind::Variant => s.variants.len(),
            IdKind::Param => s.params.len(),
            IdKind::Binder => s.binders.len(),
            IdKind::Expansion => s.expansions.len(),
        }
    }

    total!(
        /// Returns an item (`ItemKind::Err` for a foreign id).
        ///
        /// # Examples
        ///
        /// ```
        /// use hir_lang::{Builder, ItemKind};
        ///
        /// let mut b = Builder::new();
        /// let root = b.module(None, &[]);
        /// let hir = b.finish(root)?;
        /// assert!(matches!(hir.item(root).kind, ItemKind::Module { .. }));
        /// # Ok::<(), hir_lang::HirError>(())
        /// ```
        item, get_item, ItemId, Item, &ERR_ITEM
    );
    total!(
        /// Returns an expression (`Expr::Err` for a foreign id).
        ///
        /// # Examples
        ///
        /// ```
        /// use hir_lang::{Builder, Expr};
        ///
        /// let mut b = Builder::new();
        /// let e = b.expr(Expr::Err);
        /// let body = b.block(&[], Some(e));
        /// let f = b.func(hir_lang::Name::new(intern_lang::Interner::new().intern("f")), &[], body);
        /// let root = b.module(None, &[f]);
        /// let hir = b.finish(root)?;
        /// assert_eq!(hir.expr(e), &Expr::Err);
        /// # Ok::<(), hir_lang::HirError>(())
        /// ```
        expr, get_expr, ExprId, Expr, &ERR_EXPR
    );
    total!(
        /// Returns a statement (`Stmt::Err` for a foreign id).
        ///
        /// # Examples
        ///
        /// ```
        /// use hir_lang::{Builder, Stmt};
        ///
        /// let mut b = Builder::new();
        /// let s = b.stmt(Stmt::Err);
        /// let body = b.block(&[s], None);
        /// let f = b.func(hir_lang::Name::new(intern_lang::Interner::new().intern("f")), &[], body);
        /// let root = b.module(None, &[f]);
        /// assert_eq!(b.finish(root)?.stmt(s), &Stmt::Err);
        /// # Ok::<(), hir_lang::HirError>(())
        /// ```
        stmt, get_stmt, StmtId, Stmt, &ERR_STMT
    );
    total!(
        /// Returns a pattern (`Pat::Err` for a foreign id).
        ///
        /// # Examples
        ///
        /// ```
        /// use hir_lang::{Builder, Pat};
        ///
        /// let mut b = Builder::new();
        /// let root = b.module(None, &[]);
        /// let hir = b.finish(root)?;
        /// assert!(hir.get_pat(hir_lang::PatId::from_index(3).unwrap()).is_none());
        /// # Ok::<(), hir_lang::HirError>(())
        /// ```
        pat, get_pat, PatId, Pat, &ERR_PAT
    );
    total!(
        /// Returns a type term (`Ty::Err` for a foreign id).
        ///
        /// # Examples
        ///
        /// ```
        /// use hir_lang::{Builder, Ty};
        ///
        /// let mut b = Builder::new();
        /// let root = b.module(None, &[]);
        /// let hir = b.finish(root)?;
        /// assert!(hir.get_ty(hir_lang::TyId::from_index(0).unwrap()).is_none());
        /// # Ok::<(), hir_lang::HirError>(())
        /// ```
        ty, get_ty, TyId, Ty, &ERR_TY
    );
    total!(
        /// Returns a path (the error path, with no segments and `Res::Err`,
        /// for a foreign id).
        ///
        /// # Examples
        ///
        /// ```
        /// use hir_lang::{Builder, Name, Res};
        /// use intern_lang::Interner;
        ///
        /// let mut names = Interner::new();
        /// let mut b = Builder::new();
        /// let callee = b.name_expr(Name::new(names.intern("f")));
        /// let call = b.call(callee, &[]);
        /// let body = b.block(&[], Some(call));
        /// let main = b.func(Name::new(names.intern("main")), &[], body);
        /// let root = b.module(None, &[main]);
        /// let hir = b.finish(root)?;
        /// let hir_lang::Expr::Path(p) = *hir.expr(callee) else { unreachable!() };
        /// assert_eq!(hir.path(p).res, Res::Unresolved);
        /// # Ok::<(), hir_lang::HirError>(())
        /// ```
        path, get_path, PathId, Path, &ERR_PATH
    );
    total!(
        /// Returns a field definition (an unnamed, untyped field for a foreign
        /// id).
        ///
        /// # Examples
        ///
        /// ```
        /// use hir_lang::Builder;
        ///
        /// let mut b = Builder::new();
        /// let root = b.module(None, &[]);
        /// let hir = b.finish(root)?;
        /// assert!(hir.get_field(hir_lang::FieldId::from_index(0).unwrap()).is_none());
        /// # Ok::<(), hir_lang::HirError>(())
        /// ```
        field, get_field, FieldId, FieldDef, &EMPTY_FIELD
    );
    total!(
        /// Returns a parameter (a `Normal` parameter whose pattern is a foreign
        /// id, which reads as `Pat::Err`, for a foreign id).
        ///
        /// # Examples
        ///
        /// ```
        /// use hir_lang::{Builder, Name, ParamKind};
        /// use intern_lang::Interner;
        ///
        /// let mut names = Interner::new();
        /// let mut b = Builder::new();
        /// let (p, _) = b.local_param(Name::new(names.intern("n")));
        /// let body = b.block(&[], None);
        /// let f = b.func(Name::new(names.intern("f")), &[p], body);
        /// let root = b.module(None, &[f]);
        /// let hir = b.finish(root)?;
        /// assert_eq!(hir.param(p).kind, ParamKind::Normal);
        /// # Ok::<(), hir_lang::HirError>(())
        /// ```
        param, get_param, ParamId, Param, &ERR_PARAM
    );

    /// Returns a variant, or `None` for a foreign id (a variant has no error
    /// form; its name is a symbol of the caller's interner).
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, VariantId};
    ///
    /// let mut b = Builder::new();
    /// let root = b.module(None, &[]);
    /// let hir = b.finish(root)?;
    /// assert!(hir.variant(VariantId::from_index(0).unwrap()).is_none());
    /// # Ok::<(), hir_lang::HirError>(())
    /// ```
    #[must_use]
    pub fn variant(&self, id: VariantId) -> Option<&Variant> {
        self.store.variant(id)
    }

    /// Returns the sum item that declares a variant.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Ident, Item, ItemKind, List, Name, Shape, Span, SumDef, Variant};
    /// use intern_lang::Interner;
    ///
    /// let mut names = Interner::new();
    /// let mut b = Builder::new();
    /// let none = b.variant(Variant {
    ///     name: Ident::new(names.intern("None"), Span::empty(0)),
    ///     shape: Shape::Unit,
    ///     fields: List::EMPTY,
    ///     discriminant: None,
    /// });
    /// let variants = b.list(&[none]);
    /// let option = b.item(Item::new(
    ///     Some(Name::new(names.intern("Option"))),
    ///     ItemKind::Sum(SumDef { variants, ..SumDef::default() }),
    /// ));
    /// let root = b.module(None, &[option]);
    /// let hir = b.finish(root)?;
    /// assert_eq!(hir.variant_owner(none), Some(option));
    /// # Ok::<(), hir_lang::HirError>(())
    /// ```
    #[must_use]
    pub fn variant_owner(&self, id: VariantId) -> Option<ItemId> {
        let owner = *self.index.variant_owner.get(id.index())?;
        (owner != u32::MAX).then(|| ItemId::from_raw_index(owner))
    }

    /// Returns a binder, or `None` for a foreign id.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{BinderKind, Builder, Name};
    /// use intern_lang::Interner;
    ///
    /// let mut names = Interner::new();
    /// let mut b = Builder::new();
    /// let (p, n) = b.local_param(Name::new(names.intern("n")));
    /// let body = b.block(&[], None);
    /// let f = b.func(Name::new(names.intern("f")), &[p], body);
    /// let root = b.module(None, &[f]);
    /// let hir = b.finish(root)?;
    /// assert_eq!(hir.binder(n).map(|b| b.kind), Some(BinderKind::Param));
    /// # Ok::<(), hir_lang::HirError>(())
    /// ```
    #[must_use]
    pub fn binder(&self, id: BinderId) -> Option<&Binder> {
        self.store.binder(id)
    }

    /// Returns an expansion record, or `None` for [`ExpnId::ROOT`] and foreign ids.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, ExpnId};
    ///
    /// let mut b = Builder::new();
    /// let root = b.module(None, &[]);
    /// let hir = b.finish(root)?;
    /// assert!(hir.expansion(ExpnId::ROOT).is_none());
    /// # Ok::<(), hir_lang::HirError>(())
    /// ```
    #[must_use]
    pub fn expansion(&self, id: ExpnId) -> Option<&Expansion> {
        let i = (id.as_u32() as usize).checked_sub(1)?;
        self.store.expansions.get(i)
    }

    /// Returns a node's origin (the default origin for a foreign id).
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, NodeRef, Span};
    ///
    /// let mut b = Builder::new();
    /// b.set_span(Span::new(0, 12));
    /// let root = b.module(None, &[]);
    /// let hir = b.finish(root)?;
    /// assert_eq!(hir.origin(NodeRef::Item(root)).span, Span::new(0, 12));
    /// # Ok::<(), hir_lang::HirError>(())
    /// ```
    #[must_use]
    pub fn origin(&self, node: NodeRef) -> Origin {
        self.store.origin(node)
    }

    /// Returns a binder's origin: where its name was written.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Name, Span};
    /// use intern_lang::Interner;
    ///
    /// let mut names = Interner::new();
    /// let mut b = Builder::new();
    /// b.set_span(Span::new(3, 4));
    /// let (p, n) = b.local_param(Name::new(names.intern("n")));
    /// let body = b.block(&[], None);
    /// let f = b.func(Name::new(names.intern("f")), &[p], body);
    /// let root = b.module(None, &[f]);
    /// let hir = b.finish(root)?;
    /// assert_eq!(hir.binder_origin(n).span, Span::new(3, 4));
    /// # Ok::<(), hir_lang::HirError>(())
    /// ```
    #[must_use]
    pub fn binder_origin(&self, id: BinderId) -> Origin {
        self.store.binders.origin(id.index())
    }

    /// Returns the elements of a list.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Expr, OpKind};
    ///
    /// let mut b = Builder::new();
    /// let (x, y) = (b.int(1), b.int(2));
    /// let sum = b.op(OpKind::Add, &[x, y]);
    /// let body = b.block(&[], Some(sum));
    /// let f = b.func(hir_lang::Name::new(intern_lang::Interner::new().intern("f")), &[], body);
    /// let root = b.module(None, &[f]);
    /// let hir = b.finish(root)?;
    /// let Expr::Op { args, .. } = *hir.expr(sum) else { unreachable!() };
    /// assert_eq!(hir.list(args), &[x, y]);
    /// # Ok::<(), hir_lang::HirError>(())
    /// ```
    #[must_use]
    pub fn list<T: Pooled>(&self, list: List<T>) -> &[T] {
        self.store.list(list)
    }

    /// Returns the bytes of a text reference (empty for a foreign one).
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Expr, Lit};
    ///
    /// let mut b = Builder::new();
    /// let s = b.str_lit("hi");
    /// let body = b.block(&[], Some(s));
    /// let f = b.func(hir_lang::Name::new(intern_lang::Interner::new().intern("f")), &[], body);
    /// let root = b.module(None, &[f]);
    /// let hir = b.finish(root)?;
    /// let Expr::Lit(Lit::Str(t)) = *hir.expr(s) else { unreachable!() };
    /// assert_eq!(hir.text(t), b"hi");
    /// # Ok::<(), hir_lang::HirError>(())
    /// ```
    #[must_use]
    pub fn text(&self, text: TextRef) -> &[u8] {
        text.range()
            .and_then(|r| self.store.text.get(r))
            .unwrap_or(&[])
    }

    /// Returns a text reference as a string, or `""` if it is not UTF-8 (string
    /// literals always are; the validator checks them).
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Expr, Lit};
    ///
    /// let mut b = Builder::new();
    /// let s = b.str_lit("héllo");
    /// let body = b.block(&[], Some(s));
    /// let f = b.func(hir_lang::Name::new(intern_lang::Interner::new().intern("f")), &[], body);
    /// let root = b.module(None, &[f]);
    /// let hir = b.finish(root)?;
    /// let Expr::Lit(Lit::Str(t)) = *hir.expr(s) else { unreachable!() };
    /// assert_eq!(hir.str(t), "héllo");
    /// # Ok::<(), hir_lang::HirError>(())
    /// ```
    #[must_use]
    pub fn str(&self, text: TextRef) -> &str {
        core::str::from_utf8(self.text(text)).unwrap_or("")
    }

    /// Returns the attributes attached to a node, in attach order (O(log n)).
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, NodeRef};
    ///
    /// let mut b = Builder::new();
    /// let root = b.module(None, &[]);
    /// let hir = b.finish(root)?;
    /// assert!(hir.attrs(NodeRef::Item(root)).is_empty());
    /// # Ok::<(), hir_lang::HirError>(())
    /// ```
    #[must_use]
    pub fn attrs(&self, node: NodeRef) -> &[Attr] {
        self.store.attrs(node)
    }

    /// Runs the validator again (strictly) over this `Hir`.
    ///
    /// A `Hir` is valid by construction and `resolve` keeps it valid, so this
    /// always succeeds; it exists so that tests and debug builds of consumers
    /// can confirm that, at linear cost.
    ///
    /// # Errors
    ///
    /// Any [`HirError`] the validator reports (none, for a `Hir` obtained from
    /// this crate).
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::Builder;
    ///
    /// let mut b = Builder::new();
    /// let root = b.module(None, &[]);
    /// let hir = b.finish(root)?;
    /// assert_eq!(hir.validate(), Ok(()));
    /// # Ok::<(), hir_lang::HirError>(())
    /// ```
    pub fn validate(&self) -> Result<(), HirError> {
        crate::validate::validate(&self.store, self.root, self.ctx()).map(|_| ())
    }

    // ------------------------------------------------------- resolution

    /// Resolves a whole path, after an O(1) check; see
    /// [`resolve_partial`](Self::resolve_partial) (this is it with no
    /// unresolved segments).
    ///
    /// # Errors
    ///
    /// As [`resolve_partial`](Self::resolve_partial).
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{BinderKind, Builder, Expr, HirError, Name, Res};
    /// use intern_lang::Interner;
    ///
    /// let mut names = Interner::new();
    /// let mut b = Builder::new();
    /// let (p, n) = b.local_param(Name::new(names.intern("n")));
    /// let use_n = b.name_expr(Name::new(names.intern("n")));
    /// let body = b.block(&[], Some(use_n));
    /// let f = b.func(Name::new(names.intern("f")), &[p], body);
    /// let root = b.module(None, &[f]);
    /// let mut hir = b.finish(root)?;
    ///
    /// let Expr::Path(path) = *hir.expr(use_n) else { unreachable!() };
    /// hir.resolve(path, Res::Local(n))?;
    /// assert_eq!(hir.path(path).res, Res::Local(n));
    ///
    /// // A parameter is not a type.
    /// assert!(matches!(hir.resolve(path, Res::Prim(hir_lang::Prim::I32)), Err(HirError::Resolution { .. })));
    /// # Ok::<(), hir_lang::HirError>(())
    /// ```
    pub fn resolve(&mut self, path: PathId, res: Res) -> Result<(), HirError> {
        self.resolve_partial(path, res, 0)
    }

    /// Sets a path's resolution for its first `segments - unresolved`
    /// segments; the remaining `unresolved` segments are left to
    /// type-directed resolution (`Vec::new`, `T::Item`, `<T as Tr>::Out`).
    /// Checked in O(1); on error nothing changes, so the `Hir` stays valid.
    ///
    /// # Errors
    ///
    /// - [`HirError::Dangling`] if `path`, or the binder or (this unit's)
    ///   definition in `res`, does not exist.
    /// - [`HirError::ForeignDef`] if `res` names this unit through a `DefId`
    ///   minted by a different `Hir` or builder.
    /// - [`HirError::Malformed`] (`PathShape`) if `unresolved` does not fit the
    ///   path (more than its segments; an empty resolved prefix without a type
    ///   root or qualified self; a prefix past a qualified self's trait).
    /// - [`HirError::Resolution`] if the namespace cannot name `res` (with
    ///   unresolved segments: if `res` has no associated items).
    /// - [`HirError::OutOfScope`] / [`HirError::NotCapturable`] for a binder
    ///   not in scope at the path, or behind a frame it may not cross.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Def, Expr, Item, ItemKind, Name, Ns, Path, RecordDef, Res, Segment};
    /// use intern_lang::Interner;
    ///
    /// // `Point::new` in an expression: resolve-lang binds `Point`, typeck `new`.
    /// let mut names = Interner::new();
    /// let mut b = Builder::new();
    /// let point = b.item(Item::new(Some(Name::new(names.intern("Point"))), ItemKind::Record(RecordDef::default())));
    /// let segs = [
    ///     Segment::new(Name::new(names.intern("Point")), b.origin()),
    ///     Segment::new(Name::new(names.intern("new")), b.origin()),
    /// ];
    /// let segments = b.list(&segs);
    /// let path = b.path(Path::new(segments, Ns::Value));
    /// let e = b.expr(Expr::Path(path));
    /// let body = b.block(&[], Some(e));
    /// let main = b.func(Name::new(names.intern("main")), &[], body);
    /// let root = b.module(None, &[point, main]);
    /// let mut hir = b.finish(root)?;
    /// let point_def = hir.def(Def::Item(point));
    /// hir.resolve_partial(path, Res::Def(point_def), 1)?;
    /// assert_eq!(hir.path(path).unresolved, 1);
    /// # Ok::<(), hir_lang::HirError>(())
    /// ```
    pub fn resolve_partial(
        &mut self,
        path: PathId,
        res: Res,
        unresolved: u32,
    ) -> Result<(), HirError> {
        let Some(current) = self.store.path(path) else {
            return Err(HirError::Dangling {
                site: Site::Node(NodeRef::Path(path)),
                kind: IdKind::Path,
                index: path.index(),
            });
        };
        if current.segments.is_empty() && res != Res::Err {
            // The error path stays the error path.
            return Err(HirError::Resolution { path, res });
        }
        check_res(&self.store, self.ctx(), path, current, res, unresolved)?;
        if let Res::Local(binder) = res {
            check_local(&self.store, &self.index, path, binder)?;
        }
        if let Some(slot) = self.store.paths.nodes.get_mut(path.index()) {
            slot.res = res;
            slot.unresolved = if res == Res::Err { 0 } else { unresolved };
        }
        Ok(())
    }

    /// Returns `true` if `path` could resolve (fully) to `binder`: the
    /// binder's kind fits the path's namespace, it is in scope at the path,
    /// and no frame in between forbids the reference.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Expr, Name};
    /// use intern_lang::Interner;
    ///
    /// let mut names = Interner::new();
    /// let mut b = Builder::new();
    /// let (p, n) = b.local_param(Name::new(names.intern("n")));
    /// let use_n = b.name_expr(Name::new(names.intern("n")));
    /// let body = b.block(&[], Some(use_n));
    /// let f = b.func(Name::new(names.intern("f")), &[p], body);
    /// let root = b.module(None, &[f]);
    /// let hir = b.finish(root)?;
    /// let Expr::Path(path) = *hir.expr(use_n) else { unreachable!() };
    /// assert!(hir.can_reference(path, n));
    /// # Ok::<(), hir_lang::HirError>(())
    /// ```
    #[must_use]
    pub fn can_reference(&self, path: PathId, binder: BinderId) -> bool {
        let Some(p) = self.store.path(path) else {
            return false;
        };
        !p.segments.is_empty()
            && check_res(&self.store, self.ctx(), path, p, Res::Local(binder), 0).is_ok()
            && check_local(&self.store, &self.index, path, binder).is_ok()
    }

    /// Returns the lexically innermost binder named `name` in the path's
    /// namespace that is in scope at `path` (spec §3.2), in O(log n + d) where
    /// d is the nesting depth of same-named binders.
    ///
    /// This is the scope rule, applied once by the validator and reused here,
    /// so a resolver never re-derives it. Names match exactly (symbol and
    /// hygiene mark); a template's names never match user names.
    /// Visibility across frames is not filtered: the result may be behind a
    /// frame the path may not cross, which [`resolve`](Self::resolve) then
    /// reports as [`HirError::NotCapturable`] (Rust's "can't capture dynamic
    /// environment"). Returns `None` when nothing by that name is in scope, or
    /// for a foreign path.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{BinderKind, Builder, Expr, Name};
    /// use intern_lang::Interner;
    ///
    /// // { let x = 1; { let x = 2; x } }  — the inner `x` wins.
    /// let mut names = Interner::new();
    /// let x = Name::new(names.intern("x"));
    /// let mut b = Builder::new();
    /// let outer = b.new_binder(x, BinderKind::Local);
    /// let inner = b.new_binder(x, BinderKind::Local);
    /// let (po, pi) = (b.bind(outer), b.bind(inner));
    /// let (one, two) = (b.int(1), b.int(2));
    /// let (so, si) = (b.let_stmt(po, Some(one)), b.let_stmt(pi, Some(two)));
    /// let use_x = b.name_expr(x);
    /// let inner_block = b.block(&[si], Some(use_x));
    /// let body = b.block(&[so], Some(inner_block));
    /// let f = b.func(Name::new(names.intern("f")), &[], body);
    /// let root = b.module(None, &[f]);
    /// let hir = b.finish(root)?;
    /// let Expr::Path(p) = *hir.expr(use_x) else { unreachable!() };
    /// assert_eq!(hir.lookup_local(p, x), Some(inner));
    /// # Ok::<(), hir_lang::HirError>(())
    /// ```
    #[must_use]
    pub fn lookup_local(&self, path: PathId, name: Name) -> Option<BinderId> {
        let ns = self.store.path(path)?.ns;
        let ns = if ns == Ns::Pattern { Ns::Value } else { ns };
        let pos = *self.index.path_pos.get(path.index())?;
        if pos == u32::MAX {
            return None;
        }
        lookup_local(&self.index, name, ns, pos)
    }

    /// Like [`lookup_local`](Self::lookup_local), but in the namespace `ns`
    /// instead of the path's own: the innermost binder named `name` of a kind
    /// that lives in `ns` and is in scope at `path`.
    ///
    /// A resolver needs this for a path whose prefix lives in another
    /// namespace than the whole path, such as the type parameter `T` in the
    /// value path `T::new`. `Ns::Pattern` searches value binders (patterns
    /// never bind into a namespace of their own); `Ns::Import` finds nothing
    /// (no binder is imported). Frames are not filtered, as for
    /// `lookup_local`.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{BinderKind, Builder, Expr, FnDef, GenericParam, Generics, Item, ItemKind, Name, Ns};
    /// use intern_lang::Interner;
    ///
    /// // fn make<T>() { T::new }  — `T` is a type parameter seen from a value path.
    /// let mut names = Interner::new();
    /// let t = Name::new(names.intern("T"));
    /// let mut b = Builder::new();
    /// let tp = b.new_binder(t, BinderKind::TypeParam);
    /// let params = b.list(&[GenericParam::new(tp)]);
    /// let segs = [
    ///     hir_lang::Segment::new(t, b.origin()),
    ///     hir_lang::Segment::new(Name::new(names.intern("new")), b.origin()),
    /// ];
    /// let segments = b.list(&segs);
    /// let path = b.path(hir_lang::Path::new(segments, Ns::Value));
    /// let use_new = b.expr(Expr::Path(path));
    /// let body = b.block(&[], Some(use_new));
    /// let make = b.item(Item::new(
    ///     Some(Name::new(names.intern("make"))),
    ///     ItemKind::Fn(FnDef { generics: Generics { params, ..Generics::default() }, body: Some(body), ..FnDef::default() }),
    /// ));
    /// let root = b.module(None, &[make]);
    /// let hir = b.finish(root)?;
    /// assert_eq!(hir.lookup_local(path, t), None);            // no value named `T`
    /// assert_eq!(hir.lookup_local_in(path, t, Ns::Type), Some(tp));
    /// # Ok::<(), hir_lang::HirError>(())
    /// ```
    #[must_use]
    pub fn lookup_local_in(&self, path: PathId, name: Name, ns: Ns) -> Option<BinderId> {
        let ns = match ns {
            Ns::Pattern => Ns::Value,
            Ns::Import => return None,
            other => other,
        };
        let pos = *self.index.path_pos.get(path.index())?;
        if pos == u32::MAX {
            return None;
        }
        lookup_local(&self.index, name, ns, pos)
    }

    /// Returns the variables a closure captures implicitly: value binders
    /// defined outside the closure that its body (or its parameters' defaults)
    /// refer to through resolved paths, in order of first use. Explicit
    /// captures are not included (they are in the closure's `captures`).
    ///
    /// Returns an empty vector if `closure` is not a closure of this `Hir`.
    /// Linear in the size of the closure; for every closure at once, use
    /// [`all_implicit_captures`](Self::all_implicit_captures).
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, CaptureMode, Closure, Expr, Name, OpKind};
    /// use intern_lang::Interner;
    ///
    /// let mut names = Interner::new();
    /// let mut b = Builder::new();
    /// // fn f(scale) { |x| x * scale }
    /// let (ps, scale) = b.local_param(Name::new(names.intern("scale")));
    /// let (px, x) = b.local_param(Name::new(names.intern("x")));
    /// let (ux, us) = (b.use_binder(x), b.use_binder(scale));
    /// let body = b.op(OpKind::Mul, &[ux, us]);
    /// let params = b.list(&[px]);
    /// let closure = b.expr(Expr::Closure(Closure {
    ///     params,
    ///     implicit: Some(CaptureMode::Infer),
    ///     ..Closure::new(body)
    /// }));
    /// let fbody = b.block(&[], Some(closure));
    /// let f = b.func(Name::new(names.intern("f")), &[ps], fbody);
    /// let root = b.module(None, &[f]);
    /// let hir = b.finish(root)?;
    /// assert_eq!(hir.implicit_captures(closure), vec![scale]);
    /// # Ok::<(), hir_lang::HirError>(())
    /// ```
    #[must_use]
    pub fn implicit_captures(&self, closure: ExprId) -> Vec<BinderId> {
        if !matches!(self.store.expr(closure), Some(Expr::Closure(_))) {
            return Vec::new();
        }
        // Binders bound inside the closure arrive as `Bind` events before any
        // use (scope); a use of a value binder not bound inside is a capture.
        // The closure's explicit capture sources come before its frame opens
        // and are evaluated outside it, so they are skipped.
        let mut inside: BTreeSet<BinderId> = BTreeSet::new();
        let mut seen: BTreeSet<BinderId> = BTreeSet::new();
        let mut out = Vec::new();
        let mut in_frame = false;
        walk_store(&self.store, NodeRef::Expr(closure), |event| {
            match event {
                Event::FrameOpen(_) => in_frame = true,
                Event::Bind(b) if in_frame => {
                    let _ = inside.insert(b);
                }
                Event::Enter(NodeRef::Path(p)) if in_frame => {
                    if let Some(Res::Local(b)) = self.store.path(p).map(|p| p.res) {
                        let value = self.store.binder(b).is_some_and(|b| b.kind.is_value());
                        if value && !inside.contains(&b) && seen.insert(b) {
                            out.push(b);
                        }
                    }
                }
                _ => {}
            }
            Control::Continue
        });
        out
    }

    /// Returns the implicit capture set of every closure under the root, in
    /// one walk: `(closure, captures)` pairs in closure preorder, each set in
    /// first-use order. Linear in the size of the HIR plus the output.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, CaptureMode, Closure, Expr, Name};
    /// use intern_lang::Interner;
    ///
    /// let mut names = Interner::new();
    /// let mut b = Builder::new();
    /// let (p, v) = b.local_param(Name::new(names.intern("v")));
    /// let use_v = b.use_binder(v);
    /// let inner = b.expr(Expr::Closure(Closure { implicit: Some(CaptureMode::Infer), ..Closure::new(use_v) }));
    /// let outer = b.expr(Expr::Closure(Closure { implicit: Some(CaptureMode::Infer), ..Closure::new(inner) }));
    /// let body = b.block(&[], Some(outer));
    /// let f = b.func(Name::new(names.intern("f")), &[p], body);
    /// let root = b.module(None, &[f]);
    /// let hir = b.finish(root)?;
    /// assert_eq!(hir.all_implicit_captures(), vec![(outer, vec![v]), (inner, vec![v])]);
    /// # Ok::<(), hir_lang::HirError>(())
    /// ```
    #[must_use]
    pub fn all_implicit_captures(&self) -> Vec<(ExprId, Vec<BinderId>)> {
        self.captures_from(NodeRef::Item(self.root))
    }

    /// One walk from `start`: each reference to an outer value binder is added
    /// to the open closures it escapes, innermost outward, stopping at the
    /// first that already has it (every closure outside that one has it too).
    fn captures_from(&self, start: NodeRef) -> Vec<(ExprId, Vec<BinderId>)> {
        let mut out: Vec<(ExprId, Vec<BinderId>)> = Vec::new();
        let mut sets: Vec<BTreeSet<BinderId>> = Vec::new();
        // Open closures: (index into `out`, frame depth of the closure frame).
        let mut open: Vec<(usize, u32)> = Vec::new();
        let mut frames: Vec<bool> = Vec::new();
        walk_store(&self.store, start, |event| {
            match event {
                Event::FrameOpen(frame) => {
                    let is_closure = matches!(frame, Frame::Closure(_));
                    frames.push(is_closure);
                    if let Frame::Closure(c) = frame {
                        open.push((out.len(), u32::try_from(frames.len()).unwrap_or(u32::MAX)));
                        out.push((c, Vec::new()));
                        sets.push(BTreeSet::new());
                    }
                }
                Event::FrameClose => {
                    let was_closure = frames.pop() == Some(true);
                    open.truncate(open.len() - usize::from(was_closure && !open.is_empty()));
                }
                Event::Enter(NodeRef::Path(p)) => {
                    let Some(Res::Local(b)) = self.store.path(p).map(|p| p.res) else {
                        return Control::Continue;
                    };
                    if !self.store.binder(b).is_some_and(|b| b.kind.is_value()) {
                        return Control::Continue;
                    }
                    let depth = self.index.binder_depth.get(b.index()).copied().unwrap_or(0);
                    for &(slot, closure_depth) in open.iter().rev() {
                        if closure_depth <= depth {
                            break;
                        }
                        let Some(set) = sets.get_mut(slot) else { break };
                        if !set.insert(b) {
                            break;
                        }
                        if let Some((_, list)) = out.get_mut(slot) {
                            list.push(b);
                        }
                    }
                }
                _ => {}
            }
            Control::Continue
        });
        out
    }

    // -------------------------------------------------------- traversal

    /// Walks the subtree of `start` in canonical order (spec §16), calling `f`
    /// on entering and leaving each node and at each scope, binding, and frame
    /// boundary ([`Event`]). Returning [`Control::Skip`] from an `Enter` skips
    /// that node's children; [`Control::Stop`] ends the walk.
    ///
    /// Uses an explicit stack: any nesting depth is safe.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Control, Event, NodeRef};
    ///
    /// let mut b = Builder::new();
    /// let one = b.int(1);
    /// let body = b.block(&[], Some(one));
    /// let f = b.func(hir_lang::Name::new(intern_lang::Interner::new().intern("f")), &[], body);
    /// let root = b.module(None, &[f]);
    /// let hir = b.finish(root)?;
    ///
    /// let mut depth = 0usize;
    /// let mut max = 0usize;
    /// hir.walk_from(NodeRef::Item(root), |event| {
    ///     match event {
    ///         Event::Enter(_) => { depth += 1; max = max.max(depth); }
    ///         Event::Leave(_) => depth -= 1,
    ///         _ => {}
    ///     }
    ///     Control::Continue
    /// });
    /// assert_eq!(max, 4); // module > fn > block > literal
    /// # Ok::<(), hir_lang::HirError>(())
    /// ```
    pub fn walk_from<F>(&self, start: NodeRef, f: F)
    where
        F: FnMut(Event) -> Control,
    {
        walk_store(&self.store, start, f);
    }

    /// Appends the direct children of `node` to `out`, in canonical order.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, NodeRef, OpKind};
    ///
    /// let mut b = Builder::new();
    /// let (x, y) = (b.int(1), b.int(2));
    /// let sum = b.op(OpKind::Add, &[x, y]);
    /// let body = b.block(&[], Some(sum));
    /// let f = b.func(hir_lang::Name::new(intern_lang::Interner::new().intern("f")), &[], body);
    /// let root = b.module(None, &[f]);
    /// let hir = b.finish(root)?;
    /// let mut kids = Vec::new();
    /// hir.children_into(NodeRef::Expr(sum), &mut kids);
    /// assert_eq!(kids, [NodeRef::Expr(x), NodeRef::Expr(y)]);
    /// # Ok::<(), hir_lang::HirError>(())
    /// ```
    pub fn children_into(&self, node: NodeRef, out: &mut Vec<NodeRef>) {
        children_store(&self.store, node, out);
    }
}
