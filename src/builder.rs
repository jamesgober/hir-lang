//! Bottom-up HIR construction with origin capture.

use alloc::vec::Vec;

use span_lang::Span;

use crate::{
    def::{Def, DefId, UnitId, next_tag},
    error::{Capacity, HirError},
    expr::{Arg, Block, Expr, Stmt},
    hir::Hir,
    id::{
        BinderId, ExprId, FieldId, IdKind, ItemId, List, MAX_LEN, NodeRef, ParamId, PatId, PathId,
        StmtId, TextRef, TyId, VariantId,
    },
    item::{Attr, FieldDef, FnDef, Item, ItemKind, Param, Variant},
    lit::{IntLit, Lit},
    name::{Binder, BinderKind, Ns, Path, Res, Segment},
    ops::{Op, OpKind},
    origin::{Expansion, ExpnId, Name, Origin},
    pat::{BindMode, Pat},
    store::{Arena, Pooled, Store},
    ty::Effects,
    ty::Ty,
    validate::{Ctx, validate, validate_lenient},
};

/// Builds a [`Hir`] bottom-up: children first, then the parent that lists them.
///
/// The builder keeps a *current origin* that every node and binder it creates
/// captures, so lowering sets the span as it walks the source tree and never
/// passes an origin per node. It never panics: an arena that would outgrow
/// `u32` indexes is recorded and reported by [`finish`](Self::finish), which
/// also runs the validator: the only way to get a `Hir` is a valid one.
///
/// # Examples
///
/// `fn add(x, y) { x + y }` in a module:
///
/// ```
/// use hir_lang::{Builder, Name, OpKind, Span};
/// use intern_lang::Interner;
///
/// let mut names = Interner::new();
/// let mut b = Builder::new();
///
/// b.set_span(Span::new(7, 8));
/// let x = b.local_param(Name::new(names.intern("x")));
/// b.set_span(Span::new(10, 11));
/// let y = b.local_param(Name::new(names.intern("y")));
/// b.set_span(Span::new(15, 20));
/// let (px, py) = (b.use_binder(x.1), b.use_binder(y.1));
/// let sum = b.op(OpKind::Add, &[px, py]);
/// let body = b.block(&[], Some(sum));
/// let add = b.func(Name::new(names.intern("add")), &[x.0, y.0], body);
/// let root = b.module(None, &[add]);
///
/// let hir = b.finish(root)?;
/// assert_eq!(hir.origin(hir_lang::NodeRef::Expr(sum)).span, Span::new(15, 20));
/// # Ok::<(), hir_lang::HirError>(())
/// ```
#[derive(Debug)]
pub struct Builder {
    store: Store,
    origin: Origin,
    overflow: Option<Capacity>,
    attrs: Vec<(NodeRef, List<Attr>)>,
    unit: UnitId,
    tag: u32,
}

impl Default for Builder {
    fn default() -> Self {
        Self::for_unit(UnitId::default())
    }
}

impl Builder {
    /// An empty builder for unit 0; the current origin is `0..0` in source.
    /// Single-unit tools use this; a host with several units uses
    /// [`for_unit`](Self::for_unit).
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Origin};
    ///
    /// assert_eq!(Builder::new().origin(), Origin::default());
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty builder for the compilation unit `unit` (host-assigned).
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, UnitId};
    ///
    /// let b = Builder::for_unit(UnitId::new(3));
    /// assert_eq!(b.unit(), UnitId::new(3));
    /// ```
    #[must_use]
    pub fn for_unit(unit: UnitId) -> Self {
        Self {
            store: Store::default(),
            origin: Origin::default(),
            overflow: None,
            attrs: Vec::new(),
            unit,
            tag: next_tag(),
        }
    }

    /// Returns the unit being built.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, UnitId};
    ///
    /// assert_eq!(Builder::new().unit(), UnitId::new(0));
    /// ```
    #[must_use]
    pub fn unit(&self) -> UnitId {
        self.unit
    }

    /// Returns the `DefId` naming a definition of this unit, to store in a
    /// path's resolution (`Res::Def`). It carries this builder's tag, so it
    /// cannot be mistaken for an id of another `Hir`.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Def, Name, Ns, Res};
    /// use intern_lang::Interner;
    ///
    /// let mut names = Interner::new();
    /// let mut b = Builder::new();
    /// let helper_name = Name::new(names.intern("helper"));
    /// let hbody = b.block(&[], None);
    /// let helper = b.func(helper_name, &[], hbody);
    /// let def = b.def(Def::Item(helper));
    /// let callee_path = b.resolved_path(helper_name, Ns::Value, Res::Def(def));
    /// let callee = b.expr(hir_lang::Expr::Path(callee_path));
    /// let call = b.call(callee, &[]);
    /// let body = b.block(&[], Some(call));
    /// let main = b.func(Name::new(names.intern("main")), &[], body);
    /// let root = b.module(None, &[helper, main]);
    /// assert!(b.finish(root).is_ok());
    /// ```
    #[must_use]
    pub fn def(&self, def: Def) -> DefId {
        DefId::tagged(self.unit, def, self.tag)
    }

    // ---------------------------------------------------------- origins

    /// Returns the current origin.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Span};
    ///
    /// let mut b = Builder::new();
    /// b.set_span(Span::new(1, 2));
    /// assert_eq!(b.origin().span, Span::new(1, 2));
    /// ```
    #[must_use]
    pub fn origin(&self) -> Origin {
        self.origin
    }

    /// Sets the origin that subsequent nodes capture.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, ExpnId, Origin, Span};
    ///
    /// let mut b = Builder::new();
    /// b.set_origin(Origin::expanded(Span::new(0, 4), ExpnId::ROOT));
    /// assert_eq!(b.origin().span.len(), 4);
    /// ```
    pub fn set_origin(&mut self, origin: Origin) {
        self.origin = origin;
    }

    /// Sets the span of the current origin, keeping its expansion.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Span};
    ///
    /// let mut b = Builder::new();
    /// b.set_span(Span::new(5, 9));
    /// assert_eq!(b.origin().span, Span::new(5, 9));
    /// ```
    pub fn set_span(&mut self, span: Span) {
        self.origin.span = span;
    }

    /// Sets the expansion of the current origin, keeping its span.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, ExpnId};
    ///
    /// let mut b = Builder::new();
    /// b.set_expansion(ExpnId::ROOT);
    /// assert!(b.origin().expn.is_root());
    /// ```
    pub fn set_expansion(&mut self, expn: ExpnId) {
        self.origin.expn = expn;
    }

    /// Records an expansion and returns its id (also its hygiene mark).
    ///
    /// `parent` and `def_site` must name earlier expansions (or the root); the
    /// validator rejects anything else.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, ExpnId, ExpnKind, Expansion, Name, Span};
    /// use intern_lang::Interner;
    ///
    /// let mut names = Interner::new();
    /// let mut b = Builder::new();
    /// let e = b.expansion(Expansion {
    ///     kind: ExpnKind::Template,
    ///     name: names.intern("for_in"),
    ///     call_site: Span::new(0, 40),
    ///     parent: ExpnId::ROOT,
    ///     def_site: ExpnId::ROOT,
    /// });
    /// // Names the template writes carry its mark.
    /// let it = Name::marked(names.intern("it"), e);
    /// assert_eq!(it.mark, e);
    /// ```
    pub fn expansion(&mut self, expansion: Expansion) -> ExpnId {
        if self.store.expansions.len() >= MAX_LEN {
            self.overflow = Some(Capacity::Arena(IdKind::Expansion));
            return ExpnId::ROOT;
        }
        self.store.expansions.push(expansion);
        ExpnId::from_u32(u32::try_from(self.store.expansions.len()).unwrap_or(u32::MAX))
    }

    // ------------------------------------------------------------ nodes

    fn push<T>(
        arena: &mut Arena<T>,
        node: T,
        origin: Origin,
        overflow: &mut Option<Capacity>,
        kind: IdKind,
    ) -> u32 {
        let len = arena.nodes.len();
        if len >= MAX_LEN {
            *overflow = Some(Capacity::Arena(kind));
            return 0;
        }
        arena.nodes.push(node);
        arena.origins.push(origin);
        u32::try_from(len).unwrap_or(0)
    }

    /// Creates a binder at the current origin.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Binder, BinderKind, Builder, Name};
    /// use intern_lang::Interner;
    ///
    /// let mut names = Interner::new();
    /// let mut b = Builder::new();
    /// let x = b.binder(Binder::new(Name::new(names.intern("x")), BinderKind::Local));
    /// assert_eq!(x.index(), 0);
    /// ```
    pub fn binder(&mut self, binder: Binder) -> BinderId {
        let i = Self::push(
            &mut self.store.binders,
            binder,
            self.origin,
            &mut self.overflow,
            IdKind::Binder,
        );
        BinderId::from_raw_index(i)
    }

    /// Creates an item at the current origin.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Item, ItemKind, List};
    ///
    /// let mut b = Builder::new();
    /// let m = b.item(Item::new(None, ItemKind::Module { items: List::EMPTY, body: None, effects: hir_lang::Effects::NONE }));
    /// assert!(b.finish(m).is_ok());
    /// ```
    pub fn item(&mut self, item: Item) -> ItemId {
        let i = Self::push(
            &mut self.store.items,
            item,
            self.origin,
            &mut self.overflow,
            IdKind::Item,
        );
        ItemId::from_raw_index(i)
    }

    /// Creates an expression at the current origin.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Expr};
    ///
    /// let mut b = Builder::new();
    /// let e = b.expr(Expr::Err);
    /// assert_eq!(e.index(), 0);
    /// ```
    pub fn expr(&mut self, expr: Expr) -> ExprId {
        let i = Self::push(
            &mut self.store.exprs,
            expr,
            self.origin,
            &mut self.overflow,
            IdKind::Expr,
        );
        ExprId::from_raw_index(i)
    }

    /// Creates a statement at the current origin.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Stmt};
    ///
    /// let mut b = Builder::new();
    /// let e = b.int(1);
    /// assert_eq!(b.stmt(Stmt::Expr(e)).index(), 0);
    /// ```
    pub fn stmt(&mut self, stmt: Stmt) -> StmtId {
        let i = Self::push(
            &mut self.store.stmts,
            stmt,
            self.origin,
            &mut self.overflow,
            IdKind::Stmt,
        );
        StmtId::from_raw_index(i)
    }

    /// Creates a pattern at the current origin.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Pat};
    ///
    /// let mut b = Builder::new();
    /// assert_eq!(b.pat(Pat::Wild).index(), 0);
    /// ```
    pub fn pat(&mut self, pat: Pat) -> PatId {
        let i = Self::push(
            &mut self.store.pats,
            pat,
            self.origin,
            &mut self.overflow,
            IdKind::Pat,
        );
        PatId::from_raw_index(i)
    }

    /// Creates a type term at the current origin.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Prim, Ty};
    ///
    /// let mut b = Builder::new();
    /// assert_eq!(b.ty(Ty::Prim(Prim::Bool)).index(), 0);
    /// ```
    pub fn ty(&mut self, ty: Ty) -> TyId {
        let i = Self::push(
            &mut self.store.tys,
            ty,
            self.origin,
            &mut self.overflow,
            IdKind::Ty,
        );
        TyId::from_raw_index(i)
    }

    /// Creates a path at the current origin.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Name, Ns, Path, Res, Segment};
    /// use intern_lang::Interner;
    ///
    /// let mut names = Interner::new();
    /// let mut b = Builder::new();
    /// let seg = Segment::new(Name::new(names.intern("Vec")), b.origin());
    /// let segments = b.list(&[seg]);
    /// let path = b.path(Path { res: Res::Unresolved, ..Path::new(segments, Ns::Type) });
    /// assert_eq!(path.index(), 0);
    /// ```
    pub fn path(&mut self, path: Path) -> PathId {
        let i = Self::push(
            &mut self.store.paths,
            path,
            self.origin,
            &mut self.overflow,
            IdKind::Path,
        );
        PathId::from_raw_index(i)
    }

    /// Creates a field definition at the current origin.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, FieldDef, Ident, Span};
    /// use intern_lang::Interner;
    ///
    /// let mut names = Interner::new();
    /// let mut b = Builder::new();
    /// let f = b.field(FieldDef::named(Ident::new(names.intern("x"), Span::empty(0))));
    /// assert_eq!(f.index(), 0);
    /// ```
    pub fn field(&mut self, field: FieldDef) -> FieldId {
        let i = Self::push(
            &mut self.store.fields,
            field,
            self.origin,
            &mut self.overflow,
            IdKind::Field,
        );
        FieldId::from_raw_index(i)
    }

    /// Creates a sum variant at the current origin.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Ident, List, Shape, Span, Variant};
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
    /// assert_eq!(none.index(), 0);
    /// ```
    pub fn variant(&mut self, variant: Variant) -> VariantId {
        let i = Self::push(
            &mut self.store.variants,
            variant,
            self.origin,
            &mut self.overflow,
            IdKind::Variant,
        );
        VariantId::from_raw_index(i)
    }

    /// Creates a parameter at the current origin.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Param, Pat};
    ///
    /// let mut b = Builder::new();
    /// let pat = b.pat(Pat::Wild);
    /// assert_eq!(b.param(Param::new(pat)).index(), 0);
    /// ```
    pub fn param(&mut self, param: Param) -> ParamId {
        let i = Self::push(
            &mut self.store.params,
            param,
            self.origin,
            &mut self.overflow,
            IdKind::Param,
        );
        ParamId::from_raw_index(i)
    }

    // ------------------------------------------------------- lists, text

    /// Copies `elems` into their pool and returns the list.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::Builder;
    ///
    /// let mut b = Builder::new();
    /// let (one, two) = (b.int(1), b.int(2));
    /// let list = b.list(&[one, two]);
    /// assert_eq!(list.len(), 2);
    /// assert!(b.list::<hir_lang::ExprId>(&[]).is_empty());
    /// ```
    pub fn list<T: Pooled>(&mut self, elems: &[T]) -> List<T> {
        self.store.push_list(elems).unwrap_or_else(|| {
            self.overflow = Some(Capacity::Pool);
            List::EMPTY
        })
    }

    /// Stores string-literal text and returns its reference.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Expr, Lit};
    ///
    /// let mut b = Builder::new();
    /// let greeting = b.text("hello");
    /// let lit = b.expr(Expr::Lit(Lit::Str(greeting)));
    /// let _ = lit;
    /// assert_eq!(greeting.len(), 5);
    /// ```
    pub fn text(&mut self, text: &str) -> TextRef {
        self.bytes(text.as_bytes())
    }

    /// Stores raw bytes (byte strings, big-integer digits) and returns their
    /// reference.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::Builder;
    ///
    /// let mut b = Builder::new();
    /// assert_eq!(b.bytes(&[0xff, 0x00]).len(), 2);
    /// ```
    pub fn bytes(&mut self, bytes: &[u8]) -> TextRef {
        let start = self.store.text.len();
        let fits = start
            .checked_add(bytes.len())
            .is_some_and(|end| end <= MAX_LEN);
        let (Ok(start32), Ok(len32), true) =
            (u32::try_from(start), u32::try_from(bytes.len()), fits)
        else {
            self.overflow = Some(Capacity::Text);
            return TextRef::default();
        };
        self.store.text.extend_from_slice(bytes);
        TextRef::from_raw(start32, len32)
    }

    /// Attaches attributes to a node. Several calls for one node accumulate in
    /// call order.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Attr, Builder, Ident, List, NodeRef, Span};
    /// use intern_lang::Interner;
    ///
    /// let mut names = Interner::new();
    /// let mut b = Builder::new();
    /// let root = b.module(None, &[]);
    /// let inline = Attr { name: Ident::new(names.intern("doc"), Span::empty(0)), args: List::EMPTY };
    /// b.attach(NodeRef::Item(root), &[inline]);
    /// let hir = b.finish(root)?;
    /// assert_eq!(hir.attrs(NodeRef::Item(root)).len(), 1);
    /// # Ok::<(), hir_lang::HirError>(())
    /// ```
    pub fn attach(&mut self, node: NodeRef, attrs: &[Attr]) {
        let list = self.list(attrs);
        if !list.is_empty() {
            self.attrs.push((node, list));
        }
    }

    // ----------------------------------------------------- conveniences

    /// Creates a literal expression.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Lit};
    ///
    /// let mut b = Builder::new();
    /// let t = b.lit(Lit::Bool(true));
    /// assert_eq!(t.index(), 0);
    /// ```
    pub fn lit(&mut self, lit: Lit) -> ExprId {
        self.expr(Expr::Lit(lit))
    }

    /// Creates an unsuffixed integer literal expression.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::Builder;
    ///
    /// let mut b = Builder::new();
    /// let minus_seven = b.int(-7);
    /// assert_eq!(minus_seven.index(), 0);
    /// ```
    pub fn int(&mut self, value: i64) -> ExprId {
        self.lit(Lit::Int(IntLit::signed(value)))
    }

    /// Creates a string literal expression.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::Builder;
    ///
    /// let mut b = Builder::new();
    /// let s = b.str_lit("users");
    /// assert_eq!(s.index(), 0);
    /// ```
    pub fn str_lit(&mut self, text: &str) -> ExprId {
        let text = self.text(text);
        self.lit(Lit::Str(text))
    }

    /// Creates an intrinsic op at its OPS default policy.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, OpKind};
    ///
    /// let mut b = Builder::new();
    /// let (a, c) = (b.int(6), b.int(7));
    /// let product = b.op(OpKind::Mul, &[a, c]);
    /// assert_eq!(product.index(), 2);
    /// ```
    pub fn op(&mut self, kind: OpKind, args: &[ExprId]) -> ExprId {
        self.op_with(Op::new(kind), args)
    }

    /// Creates an intrinsic op with an explicit policy.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Op, OpKind, Overflow};
    ///
    /// let mut b = Builder::new();
    /// let (a, c) = (b.int(1), b.int(2));
    /// let wrapping = b.op_with(Op::new(OpKind::Add).with_overflow(Overflow::Wrap), &[a, c]);
    /// assert_eq!(wrapping.index(), 2);
    /// ```
    pub fn op_with(&mut self, op: Op, args: &[ExprId]) -> ExprId {
        let args = self.list(args);
        self.expr(Expr::Op { op, args })
    }

    /// Creates a call with positional arguments.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Name};
    /// use intern_lang::Interner;
    ///
    /// let mut names = Interner::new();
    /// let mut b = Builder::new();
    /// let print = b.name_expr(Name::new(names.intern("print")));
    /// let arg = b.str_lit("hi");
    /// let call = b.call(print, &[arg]);
    /// assert_eq!(call.index(), 2);
    /// ```
    pub fn call(&mut self, callee: ExprId, args: &[ExprId]) -> ExprId {
        let args: Vec<Arg> = args.iter().map(|a| Arg::positional(*a)).collect();
        let args = self.list(&args);
        self.expr(Expr::Call { callee, args })
    }

    /// Creates a binder of the given kind with an immutable name.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{BinderKind, Builder, Name};
    /// use intern_lang::Interner;
    ///
    /// let mut names = Interner::new();
    /// let mut b = Builder::new();
    /// let t = b.new_binder(Name::new(names.intern("T")), BinderKind::TypeParam);
    /// assert_eq!(t.index(), 0);
    /// ```
    pub fn new_binder(&mut self, name: Name, kind: BinderKind) -> BinderId {
        self.binder(Binder::new(name, kind))
    }

    /// Creates a by-value binding pattern for `binder`.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{BinderKind, Builder, Name};
    /// use intern_lang::Interner;
    ///
    /// let mut names = Interner::new();
    /// let mut b = Builder::new();
    /// let x = b.new_binder(Name::new(names.intern("x")), BinderKind::Local);
    /// let pat = b.bind(x);
    /// assert_eq!(pat.index(), 0);
    /// ```
    pub fn bind(&mut self, binder: BinderId) -> PatId {
        self.pat(Pat::Bind {
            binder,
            mode: BindMode::Value,
            sub: None,
        })
    }

    /// Creates a `Normal` parameter binding a fresh `Param` binder named `name`,
    /// returning the parameter and the binder.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Name};
    /// use intern_lang::Interner;
    ///
    /// let mut names = Interner::new();
    /// let mut b = Builder::new();
    /// let (param, binder) = b.local_param(Name::new(names.intern("n")));
    /// assert_eq!((param.index(), binder.index()), (0, 0));
    /// ```
    pub fn local_param(&mut self, name: Name) -> (ParamId, BinderId) {
        let binder = self.new_binder(name, BinderKind::Param);
        let pat = self.bind(binder);
        (self.param(Param::new(pat)), binder)
    }

    /// Creates a single-segment path named `name`, unresolved.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Name, Ns};
    /// use intern_lang::Interner;
    ///
    /// let mut names = Interner::new();
    /// let mut b = Builder::new();
    /// let p = b.name_path(Name::new(names.intern("Option")), Ns::Type);
    /// assert_eq!(p.index(), 0);
    /// ```
    pub fn name_path(&mut self, name: Name, ns: Ns) -> PathId {
        self.resolved_path(name, ns, Res::Unresolved)
    }

    /// Creates a single-segment path named `name` with a resolution.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Name, Ns, Prim, Res};
    /// use intern_lang::Interner;
    ///
    /// let mut names = Interner::new();
    /// let mut b = Builder::new();
    /// let i32_path = b.resolved_path(Name::new(names.intern("i32")), Ns::Type, Res::Prim(Prim::I32));
    /// assert_eq!(i32_path.index(), 0);
    /// ```
    pub fn resolved_path(&mut self, name: Name, ns: Ns, res: Res) -> PathId {
        let segment = Segment::new(name, self.origin);
        let segments = self.list(&[segment]);
        self.path(Path {
            res,
            ..Path::new(segments, ns)
        })
    }

    /// Creates an expression naming `name`, unresolved (`Value` namespace).
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Name};
    /// use intern_lang::Interner;
    ///
    /// let mut names = Interner::new();
    /// let mut b = Builder::new();
    /// let e = b.name_expr(Name::new(names.intern("println")));
    /// assert_eq!(e.index(), 0);
    /// ```
    pub fn name_expr(&mut self, name: Name) -> ExprId {
        let path = self.name_path(name, Ns::Value);
        self.expr(Expr::Path(path))
    }

    /// Creates an expression referring to `binder`, already resolved — what a
    /// lowering template emits for its own temporaries (hygiene by construction).
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{BinderKind, Builder, Name};
    /// use intern_lang::Interner;
    ///
    /// let mut names = Interner::new();
    /// let mut b = Builder::new();
    /// let x = b.new_binder(Name::new(names.intern("x")), BinderKind::Local);
    /// let use_x = b.use_binder(x);
    /// assert_eq!(use_x.index(), 0);
    /// ```
    pub fn use_binder(&mut self, binder: BinderId) -> ExprId {
        let name = self.store.binder(binder).map(|b| b.name);
        let path = match name {
            Some(name) => self.resolved_path(name, Ns::Value, Res::Local(binder)),
            // A foreign binder id: build an empty path, which the validator
            // reports, rather than failing here.
            None => self.path(Path {
                res: Res::Local(binder),
                ..Path::new(List::EMPTY, Ns::Value)
            }),
        };
        self.expr(Expr::Path(path))
    }

    /// Creates a block expression.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::Builder;
    ///
    /// let mut b = Builder::new();
    /// let one = b.int(1);
    /// let block = b.block(&[], Some(one));
    /// assert_eq!(block.index(), 1);
    /// ```
    pub fn block(&mut self, stmts: &[StmtId], tail: Option<ExprId>) -> ExprId {
        let stmts = self.list(stmts);
        self.expr(Expr::Block(Block {
            stmts,
            tail,
            label: None,
            is_unsafe: false,
        }))
    }

    /// Creates `let pat = init;`.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{BinderKind, Builder, Name};
    /// use intern_lang::Interner;
    ///
    /// let mut names = Interner::new();
    /// let mut b = Builder::new();
    /// let x = b.new_binder(Name::new(names.intern("x")), BinderKind::Local);
    /// let pat = b.bind(x);
    /// let one = b.int(1);
    /// let stmt = b.let_stmt(pat, Some(one));
    /// assert_eq!(stmt.index(), 0);
    /// ```
    pub fn let_stmt(&mut self, pat: PatId, init: Option<ExprId>) -> StmtId {
        self.stmt(Stmt::Let {
            pat,
            ty: None,
            init,
            else_: None,
        })
    }

    /// Creates an expression statement.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::Builder;
    ///
    /// let mut b = Builder::new();
    /// let e = b.int(0);
    /// assert_eq!(b.expr_stmt(e).index(), 0);
    /// ```
    pub fn expr_stmt(&mut self, expr: ExprId) -> StmtId {
        self.stmt(Stmt::Expr(expr))
    }

    /// Creates a private function item with the given parameters and body, no
    /// generics, no annotations, and no effects.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Name};
    /// use intern_lang::Interner;
    ///
    /// let mut names = Interner::new();
    /// let mut b = Builder::new();
    /// let body = b.block(&[], None);
    /// let main = b.func(Name::new(names.intern("main")), &[], body);
    /// let root = b.module(None, &[main]);
    /// assert!(b.finish(root).is_ok());
    /// ```
    pub fn func(&mut self, name: Name, params: &[ParamId], body: ExprId) -> ItemId {
        let params = self.list(params);
        self.item(Item::new(
            Some(name),
            ItemKind::Fn(FnDef {
                params,
                body: Some(body),
                ..FnDef::default()
            }),
        ))
    }

    /// Creates a private module item.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::Builder;
    ///
    /// let mut b = Builder::new();
    /// let root = b.module(None, &[]);
    /// assert!(b.finish(root).is_ok());
    /// ```
    pub fn module(&mut self, name: Option<Name>, items: &[ItemId]) -> ItemId {
        let items = self.list(items);
        self.item(Item::new(
            name,
            ItemKind::Module {
                items,
                body: None,
                effects: Effects::NONE,
            },
        ))
    }

    /// Copies the subtree under `node` and returns the copy's root.
    ///
    /// Every binder *bound inside* the subtree is replaced by a fresh binder
    /// in the copy (references inside follow it); references to binders bound
    /// outside keep pointing at them. Origins and attributes are copied. The
    /// copy is unattached: place it as a child like any new node. For template
    /// instantiation, inlining, and unrolling. Iterative: any depth is safe.
    ///
    /// If `node` does not exist, or an arena would overflow, the overflow is
    /// recorded (reported by `finish`) and `node` itself is returned.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{BinderKind, Builder, Expr, Name, NodeRef, Stmt};
    /// use intern_lang::Interner;
    ///
    /// // { let t = 1; t } used twice: the copy gets its own `t`.
    /// let mut names = Interner::new();
    /// let mut b = Builder::new();
    /// let t = b.new_binder(Name::new(names.intern("t")), BinderKind::Local);
    /// let pat = b.bind(t);
    /// let one = b.int(1);
    /// let decl = b.let_stmt(pat, Some(one));
    /// let use_t = b.use_binder(t);
    /// let original = b.block(&[decl], Some(use_t));
    /// let NodeRef::Expr(copy) = b.copy_subtree(NodeRef::Expr(original)) else { unreachable!() };
    /// let pair = b.list(&[original, copy]);
    /// let both = b.expr(Expr::Tuple(pair));
    /// let body = b.block(&[], Some(both));
    /// let f = b.func(Name::new(names.intern("f")), &[], body);
    /// let root = b.module(None, &[f]);
    /// let hir = b.finish(root)?;
    /// assert_eq!(hir.count(hir_lang::IdKind::Binder), 2);
    /// # Ok::<(), hir_lang::HirError>(())
    /// ```
    pub fn copy_subtree(&mut self, node: NodeRef) -> NodeRef {
        match crate::copy::copy_subtree(&mut self.store, &mut self.attrs, node) {
            Some(copy) => copy,
            None => {
                self.overflow = Some(Capacity::Pool);
                node
            }
        }
    }

    // ----------------------------------------------------------- finish

    /// Validates everything built so far with `root` (a module) as the root
    /// and returns the `Hir`.
    ///
    /// # Errors
    ///
    /// [`HirError::CapacityExceeded`] if an arena or pool overflowed while
    /// building; otherwise the first validation error (spec §13): a dangling
    /// id, a shared or unreachable node, a misused binder, an out-of-scope
    /// resolution, a jump without a target, a misplaced effect, a policy
    /// mismatch, or a malformed node.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, HirError, NodeRef};
    ///
    /// let mut b = Builder::new();
    /// let orphan = b.int(1); // never attached to the tree
    /// let root = b.module(None, &[]);
    /// assert_eq!(b.finish(root), Err(HirError::Unreachable { node: NodeRef::Expr(orphan) }));
    /// ```
    pub fn finish(mut self, root: ItemId) -> Result<Hir, HirError> {
        if let Some(what) = self.overflow {
            return Err(HirError::CapacityExceeded { what });
        }
        self.normalize_attrs();
        let ctx = Ctx {
            unit: self.unit,
            tag: self.tag,
        };
        let index = validate(&self.store, root, ctx)?;
        Ok(Hir::from_parts(
            self.store, root, self.unit, self.tag, index,
        ))
    }

    /// Validates leniently: every problem is collected, each offending node
    /// is replaced by its kind's error form (or a narrower fix: an error
    /// resolution, a wildcard binding), and the repaired, valid `Hir` is
    /// returned with every problem in source order.
    ///
    /// This is the mode for user code: a `break` outside a loop, an `await`
    /// outside an async function, a duplicate field or named argument, an
    /// out-of-range literal, or an out-of-scope reference becomes a
    /// diagnostic, and the rest of the unit is still analyzed. Consequences
    /// of a repair (children of a replaced node, references to binders it
    /// bound) are fixed silently, not reported again. Use [`finish`](Self::finish)
    /// in tools that must reject any malformed HIR.
    ///
    /// # Errors
    ///
    /// Only where no repair exists: [`HirError::CapacityExceeded`], a root
    /// that does not exist ([`HirError::Dangling`]), or one that is not a
    /// module ([`HirError::RootNotModule`]).
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Expr, HirError, IntLit, JumpProblem, Lit, Prim};
    ///
    /// let mut b = Builder::new();
    /// let stray = b.expr(Expr::Break { label: None, value: None });
    /// let big = b.lit(Lit::Int(IntLit::new(300).with_suffix(Prim::U8)));
    /// let s1 = b.expr_stmt(stray);
    /// let s2 = b.expr_stmt(big);
    /// let body = b.block(&[s1, s2], None);
    /// let f = b.func(hir_lang::Name::new(intern_lang::Interner::new().intern("f")), &[], body);
    /// let root = b.module(None, &[f]);
    ///
    /// let (hir, problems) = b.finish_lenient(root)?;
    /// assert_eq!(problems.len(), 2);
    /// assert!(problems.contains(&HirError::Jump { expr: stray, problem: JumpProblem::BreakOutsideLoop }));
    /// assert_eq!(hir.expr(stray), &Expr::Err);
    /// assert_eq!(hir.validate(), Ok(()));
    /// # Ok::<(), HirError>(())
    /// ```
    pub fn finish_lenient(mut self, root: ItemId) -> Result<(Hir, Vec<HirError>), HirError> {
        if let Some(what) = self.overflow {
            return Err(HirError::CapacityExceeded { what });
        }
        self.normalize_attrs();
        let ctx = Ctx {
            unit: self.unit,
            tag: self.tag,
        };
        let (index, problems) = validate_lenient(&mut self.store, root, ctx)?;
        Ok((
            Hir::from_parts(self.store, root, self.unit, self.tag, index),
            problems,
        ))
    }

    /// Sorts the attribute table by target, merging several lists for one target
    /// into one contiguous list (in attach order).
    fn normalize_attrs(&mut self) {
        let mut entries = core::mem::take(&mut self.attrs);
        entries.sort_by_key(|(node, _)| *node);
        let mut table: Vec<(NodeRef, List<Attr>)> = Vec::with_capacity(entries.len());
        let mut i = 0;
        while i < entries.len() {
            let target = entries[i].0;
            let mut j = i + 1;
            while j < entries.len() && entries[j].0 == target {
                j += 1;
            }
            let list = if j - i == 1 {
                entries[i].1
            } else {
                let merged: Vec<Attr> = entries[i..j]
                    .iter()
                    .flat_map(|(_, l)| self.store.list(*l).iter().copied())
                    .collect();
                self.list(&merged)
            };
            table.push((target, list));
            i = j;
        }
        self.store.attrs = table;
    }

    /// Exposes the raw store to the crate's tests (to plant invalid states the
    /// public API cannot express).
    #[cfg(test)]
    pub(crate) fn store_mut(&mut self) -> &mut Store {
        &mut self.store
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_finish_with_overflow_flag_returns_capacity_error() {
        let mut b = Builder::new();
        let root = b.module(None, &[]);
        b.overflow = Some(Capacity::Pool);
        assert_eq!(
            b.finish(root),
            Err(HirError::CapacityExceeded {
                what: Capacity::Pool
            })
        );
    }

    #[test]
    fn test_normalize_attrs_merges_targets_in_attach_order() {
        let mut names = intern_lang::Interner::new();
        let mut b = Builder::new();
        let root = b.module(None, &[]);
        let a = Attr {
            name: crate::Ident::new(names.intern("a"), Span::empty(0)),
            args: List::EMPTY,
        };
        let c = Attr {
            name: crate::Ident::new(names.intern("c"), Span::empty(0)),
            args: List::EMPTY,
        };
        b.attach(NodeRef::Item(root), &[a]);
        b.attach(NodeRef::Item(root), &[c]);
        let hir = b.finish(root);
        let hir = hir.as_ref().map(|h| h.attrs(NodeRef::Item(root)).to_vec());
        assert_eq!(hir, Ok(alloc::vec![a, c]));
    }

    #[test]
    fn test_store_mut_reaches_planted_state() {
        let mut b = Builder::new();
        let root = b.module(None, &[]);
        b.store_mut().text.push(b'x');
        assert!(b.finish(root).is_ok());
    }
}
