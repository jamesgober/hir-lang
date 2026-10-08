//! The validated HIR and its read API.

use alloc::{collections::BTreeSet, vec::Vec};

use span_lang::Span;

use crate::{
    error::{HirError, Site},
    expr::{Expr, Stmt},
    id::{
        BinderId, ExprId, FieldId, IdKind, ItemId, List, NodeRef, ParamId, PatId, PathId, StmtId,
        TextRef, TyId, VariantId,
    },
    item::{Attr, FieldDef, Item, ItemKind, Param, Variant, Vis},
    name::{Binder, Ns, Path, Res},
    origin::{Expansion, ExpnId, Origin},
    pat::Pat,
    store::{Pooled, Store},
    ty::Ty,
    validate::{Index, check_local, res_allowed},
    walk::{Control, Event, children_store, walk_store},
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
    res: Res::Err,
    global: false,
};
static EMPTY_FIELD: FieldDef = FieldDef {
    name: None,
    vis: Vis::Private,
    ty: None,
    default: None,
};

/// A validated HIR: one compilation unit rooted at a module.
///
/// The only ways to obtain one are [`Builder::finish`](crate::Builder::finish)
/// (and, from 0.5, the decoder), both of which run the validator, so every
/// `Hir` satisfies the contract in the spec, §13: every id resolves, the nodes
/// form one tree, binders are bound once and referenced only in scope, jumps
/// have targets, effects are placed where allowed, and every op carries
/// exactly its policy. The only mutation, [`resolve`](Self::resolve), checks
/// each change and keeps that contract.
///
/// Storage is flat (arenas of `Copy` nodes, out-of-line lists), so `Clone`,
/// `PartialEq`, `Debug`, and `Drop` never recurse, at any depth.
///
/// Accessors take ids issued for this `Hir`. A foreign id never panics: the
/// node accessors return an error node (`Expr::Err`, `Pat::Err`, ...), and the
/// accessors for records without an error form return `None`.
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
    index: Index,
}

impl Hir {
    pub(crate) fn from_parts(store: Store, root: ItemId, index: Index) -> Self {
        Self { store, root, index }
    }

    pub(crate) fn store(&self) -> &Store {
        &self.store
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

    /// Returns the number of entries in an arena.
    ///
    /// Ids of that kind are exactly `0..count`, so side tables (types per
    /// expression, slots per binder) are vectors of this length.
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
    #[must_use]
    pub fn item(&self, id: ItemId) -> &Item {
        self.store.item(id).unwrap_or(&ERR_ITEM)
    }

    /// Returns an expression (`Expr::Err` for a foreign id).
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Expr, ExprId};
    ///
    /// let mut b = Builder::new();
    /// let root = b.module(None, &[]);
    /// let hir = b.finish(root)?;
    /// assert_eq!(hir.expr(ExprId::from_index(99).unwrap()), &Expr::Err);
    /// # Ok::<(), hir_lang::HirError>(())
    /// ```
    #[must_use]
    pub fn expr(&self, id: ExprId) -> &Expr {
        self.store.expr(id).unwrap_or(&ERR_EXPR)
    }

    /// Returns a statement (`Stmt::Err` for a foreign id).
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Stmt, StmtId};
    ///
    /// let mut b = Builder::new();
    /// let root = b.module(None, &[]);
    /// let hir = b.finish(root)?;
    /// assert_eq!(hir.stmt(StmtId::from_index(0).unwrap()), &Stmt::Err);
    /// # Ok::<(), hir_lang::HirError>(())
    /// ```
    #[must_use]
    pub fn stmt(&self, id: StmtId) -> &Stmt {
        self.store.stmt(id).unwrap_or(&ERR_STMT)
    }

    /// Returns a pattern (`Pat::Err` for a foreign id).
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Pat, PatId};
    ///
    /// let mut b = Builder::new();
    /// let root = b.module(None, &[]);
    /// let hir = b.finish(root)?;
    /// assert_eq!(hir.pat(PatId::from_index(3).unwrap()), &Pat::Err);
    /// # Ok::<(), hir_lang::HirError>(())
    /// ```
    #[must_use]
    pub fn pat(&self, id: PatId) -> &Pat {
        self.store.pat(id).unwrap_or(&ERR_PAT)
    }

    /// Returns a type term (`Ty::Err` for a foreign id).
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Ty, TyId};
    ///
    /// let mut b = Builder::new();
    /// let root = b.module(None, &[]);
    /// let hir = b.finish(root)?;
    /// assert_eq!(hir.ty(TyId::from_index(0).unwrap()), &Ty::Err);
    /// # Ok::<(), hir_lang::HirError>(())
    /// ```
    #[must_use]
    pub fn ty(&self, id: TyId) -> &Ty {
        self.store.ty(id).unwrap_or(&ERR_TY)
    }

    /// Returns a path (an empty path resolved to `Res::Err` for a foreign id).
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
    #[must_use]
    pub fn path(&self, id: PathId) -> &Path {
        self.store.path(id).unwrap_or(&ERR_PATH)
    }

    /// Returns a field definition (an unnamed, untyped field for a foreign id).
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, FieldId};
    ///
    /// let mut b = Builder::new();
    /// let root = b.module(None, &[]);
    /// let hir = b.finish(root)?;
    /// assert!(hir.field(FieldId::from_index(0).unwrap()).name.is_none());
    /// # Ok::<(), hir_lang::HirError>(())
    /// ```
    #[must_use]
    pub fn field(&self, id: FieldId) -> &FieldDef {
        self.store.field(id).unwrap_or(&EMPTY_FIELD)
    }

    /// Returns a variant, or `None` for a foreign id.
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

    /// Returns a parameter, or `None` for a foreign id.
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
    /// assert_eq!(hir.param(p).map(|p| p.kind), Some(ParamKind::Normal));
    /// # Ok::<(), hir_lang::HirError>(())
    /// ```
    #[must_use]
    pub fn param(&self, id: ParamId) -> Option<&Param> {
        self.store.param(id)
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

    /// Runs the validator again over this `Hir`.
    ///
    /// A `Hir` is valid by construction and [`resolve`](Self::resolve) keeps it
    /// valid, so this always succeeds; it exists so that tests and debug builds
    /// of consumers can confirm that, at linear cost.
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
        crate::validate::validate(&self.store, self.root).map(|_| ())
    }

    // ------------------------------------------------------- resolution

    /// Sets the resolution of a path, after checking it in O(1).
    ///
    /// This is how resolve-lang writes its results: the `Hir` stays valid after
    /// every call.
    ///
    /// # Errors
    ///
    /// - [`HirError::Dangling`] if `path`, or the binder, item, or variant `res`
    ///   names, is not in this `Hir`.
    /// - [`HirError::Resolution`] if the path's namespace cannot name `res`
    ///   (spec §3.3).
    /// - [`HirError::OutOfScope`] if `res` is a binder not in scope at the path.
    /// - [`HirError::NotCapturable`] if `res` is a binder outside a frame the
    ///   path may not reach across (a nested item, a constant context, or a
    ///   closure without implicit captures).
    ///
    /// On error the path is left unchanged.
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
        let site = Site::Node(NodeRef::Path(path));
        let Some(current) = self.store.path(path) else {
            return Err(HirError::Dangling {
                site,
                kind: IdKind::Path,
                index: path.index(),
            });
        };
        let dangling = match res {
            Res::Local(b) if self.store.binder(b).is_none() => Some((IdKind::Binder, b.index())),
            Res::Item(i) if self.store.item(i).is_none() => Some((IdKind::Item, i.index())),
            Res::Variant(v) if self.store.variant(v).is_none() => {
                Some((IdKind::Variant, v.index()))
            }
            _ => None,
        };
        if let Some((kind, index)) = dangling {
            return Err(HirError::Dangling { site, kind, index });
        }
        if !res_allowed(&self.store, current.ns, res) {
            return Err(HirError::Resolution { path, res });
        }
        if let Res::Local(binder) = res {
            check_local(&self.store, &self.index, path, binder)?;
        }
        if let Some(slot) = self.store.paths.nodes.get_mut(path.index()) {
            slot.res = res;
        }
        Ok(())
    }

    /// Returns `true` if `path` could resolve to `binder`: the binder's kind
    /// fits the path's namespace, it is in scope at the path, and no frame in
    /// between forbids the reference. resolve-lang uses this to filter
    /// candidates before committing with [`resolve`](Self::resolve).
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
        res_allowed(&self.store, p.ns, Res::Local(binder))
            && check_local(&self.store, &self.index, path, binder).is_ok()
    }

    /// Returns the variables a closure captures implicitly: value binders
    /// defined outside the closure that its body (or its parameters' defaults)
    /// refer to through resolved paths, in order of first use. Explicit
    /// captures are not included (they are in the closure's `captures`).
    ///
    /// Returns an empty vector if `closure` is not a closure of this `Hir`.
    /// Linear in the size of the closure.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{BinderKind, Builder, CaptureMode, Closure, Effects, Expr, List, Name, OpKind};
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
    ///     ret: None,
    ///     body,
    ///     effects: Effects::NONE,
    ///     implicit: Some(CaptureMode::Infer),
    ///     captures: List::EMPTY,
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
        let Some(Expr::Closure(c)) = self.store.expr(closure) else {
            return Vec::new();
        };
        let mut inside: BTreeSet<BinderId> = self
            .store
            .list(c.captures)
            .iter()
            .map(|cap| cap.binder)
            .collect();
        let mut seen: BTreeSet<BinderId> = BTreeSet::new();
        let mut out = Vec::new();
        let starts = self
            .store
            .list(c.params)
            .iter()
            .map(|p| NodeRef::Param(*p))
            .chain(c.ret.map(NodeRef::Ty))
            .chain(core::iter::once(NodeRef::Expr(c.body)));
        for start in starts {
            walk_store(&self.store, start, |event| {
                if let Event::Enter(node) = event {
                    match node {
                        NodeRef::Pat(p) => {
                            if let Some(Pat::Bind { binder, .. }) = self.store.pat(p) {
                                let _ = inside.insert(*binder);
                            }
                        }
                        NodeRef::Expr(e) => {
                            if let Some(Expr::Closure(inner)) = self.store.expr(e) {
                                inside.extend(
                                    self.store.list(inner.captures).iter().map(|c| c.binder),
                                );
                            }
                        }
                        NodeRef::Path(p) => {
                            if let Some(Res::Local(b)) = self.store.path(p).map(|p| p.res) {
                                let value = self.store.binder(b).is_some_and(|b| b.kind.is_value());
                                if value && !inside.contains(&b) && seen.insert(b) {
                                    out.push(b);
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Control::Continue
            });
        }
        out
    }

    // -------------------------------------------------------- traversal

    /// Walks the subtree of `start` in canonical order (spec §16), calling `f`
    /// on entering and leaving each node. Returning [`Control::Skip`] from an
    /// `Enter` skips that node's children; [`Control::Stop`] ends the walk.
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
