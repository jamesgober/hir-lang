//! Canonical-order traversal.
//!
//! [`expand`] is the single definition of a node's children and their order
//! (spec §16). It also emits *marks* at the points where scopes open and close,
//! binders become visible, frames begin and end, and printer groups start, so
//! the validator, the public walker, and the printer all derive from one source
//! and can never disagree about order.
//!
//! Every traversal is driven by an explicit stack: native stack use is constant
//! whatever the nesting depth.

use alloc::vec::Vec;

use crate::{
    ArgKind,
    expr::{Arm, Capture, Expr, Stmt},
    id::{BinderId, ExprId, ItemId, List, NodeRef, TyId},
    item::{GenericParam, Generics, ItemKind},
    origin::Ident,
    pat::Pat,
    store::Store,
    ty::Ty,
};

/// One step of a canonical traversal.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Step {
    Enter(NodeRef),
    Leave(NodeRef),
    Mark(Mark),
}

/// Which binding construct a [`Mark::Bind`] closes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BindSite {
    Let,
    Param,
    Arm,
}

/// What pushed a frame.
#[derive(Clone, Copy, Debug)]
pub(crate) enum FrameOf {
    Item(ItemId),
    Closure(ExprId),
    /// A constant context; `integer` when its value is an integer by
    /// definition (array length, const generic argument, discriminant).
    Const {
        integer: bool,
    },
}

/// A printer group: a bracket around children that are not nodes themselves
/// (an arm, a named argument, a capture), so the printed form is unambiguous.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Group {
    Tag(&'static str),
    Arm,
    Catch,
    Generic(BinderId),
    Where,
    Capture(Capture),
    FieldInit(Ident),
    Entry,
    Arg(ArgKind),
    FieldPat(Ident),
    SliceRest,
    SegmentArgs(u32),
}

/// Points of interest between child visits.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Mark {
    Frame(FrameOf),
    PopFrame,
    Scope,
    PopScope,
    /// The generic parameters' binders become visible.
    Generics(List<GenericParam>),
    /// The binders of the last completed root pattern become visible.
    Bind(BindSite),
    /// The explicit captures' binders become visible.
    Captures(List<Capture>),
    /// A loop or block label becomes visible.
    Label(BinderId),
    Loop {
        label: Option<BinderId>,
        is_loop: bool,
    },
    PopLoop,
    Try,
    PopTry,
    Defer,
    PopDefer,
    Open(Group),
    Close,
}

/// Pushes children and marks of `node`, in canonical order, onto `out`.
///
/// The node's own `Enter`/`Leave` are not included. Ids that do not resolve in
/// `store` contribute nothing (only the validator ever sees such a store, and it
/// checks ranges before it traverses).
pub(crate) fn expand(store: &Store, node: NodeRef, out: &mut Vec<Step>) {
    let mut e = Expander { s: store, out };
    match node {
        NodeRef::Item(id) => e.item(id),
        NodeRef::Expr(id) => e.expr(id),
        NodeRef::Stmt(id) => e.stmt(id),
        NodeRef::Pat(id) => e.pat(id),
        NodeRef::Ty(id) => e.ty(id),
        NodeRef::Path(id) => {
            if let Some(path) = store.path(id) {
                for (i, seg) in store.list(path.segments).iter().enumerate() {
                    if !seg.args.is_empty() {
                        let i = u32::try_from(i).unwrap_or(u32::MAX);
                        e.open(Group::SegmentArgs(i));
                        e.tys(seg.args);
                        e.close();
                    }
                }
            }
        }
        NodeRef::Field(id) => {
            if let Some(field) = store.field(id) {
                e.opt_ty(field.ty);
                if let Some(default) = field.default {
                    e.mark(Mark::Frame(FrameOf::Const { integer: false }));
                    e.tagged_expr("default", default);
                    e.mark(Mark::PopFrame);
                }
            }
        }
        NodeRef::Variant(id) => {
            if let Some(variant) = store.variant(id) {
                for f in store.list(variant.fields) {
                    e.node(NodeRef::Field(*f));
                }
                if let Some(d) = variant.discriminant {
                    e.mark(Mark::Frame(FrameOf::Const { integer: true }));
                    e.tagged_expr("discriminant", d);
                    e.mark(Mark::PopFrame);
                }
            }
        }
        NodeRef::Param(id) => {
            if let Some(param) = store.param(id) {
                e.node(NodeRef::Pat(param.pat));
                e.opt_ty(param.ty);
                if let Some(d) = param.default {
                    e.tagged_expr("default", d);
                }
                e.mark(Mark::Bind(BindSite::Param));
            }
        }
    }
}

struct Expander<'a> {
    s: &'a Store,
    out: &'a mut Vec<Step>,
}

impl Expander<'_> {
    #[inline]
    fn node(&mut self, node: NodeRef) {
        self.out.push(Step::Enter(node));
    }

    #[inline]
    fn mark(&mut self, mark: Mark) {
        self.out.push(Step::Mark(mark));
    }

    #[inline]
    fn open(&mut self, group: Group) {
        self.mark(Mark::Open(group));
    }

    #[inline]
    fn close(&mut self) {
        self.mark(Mark::Close);
    }

    fn expr_node(&mut self, id: ExprId) {
        self.node(NodeRef::Expr(id));
    }

    fn opt_expr(&mut self, id: Option<ExprId>) {
        if let Some(id) = id {
            self.expr_node(id);
        }
    }

    fn tagged_expr(&mut self, tag: &'static str, id: ExprId) {
        self.open(Group::Tag(tag));
        self.expr_node(id);
        self.close();
    }

    fn ty_node(&mut self, id: TyId) {
        self.node(NodeRef::Ty(id));
    }

    fn opt_ty(&mut self, id: Option<TyId>) {
        if let Some(id) = id {
            self.ty_node(id);
        }
    }

    fn tagged_ty(&mut self, tag: &'static str, id: Option<TyId>) {
        if let Some(id) = id {
            self.open(Group::Tag(tag));
            self.ty_node(id);
            self.close();
        }
    }

    fn tys(&mut self, list: List<TyId>) {
        for t in self.s.list(list) {
            self.out.push(Step::Enter(NodeRef::Ty(*t)));
        }
    }

    fn exprs(&mut self, list: List<ExprId>) {
        for x in self.s.list(list) {
            self.out.push(Step::Enter(NodeRef::Expr(*x)));
        }
    }

    fn items(&mut self, list: List<ItemId>) {
        for i in self.s.list(list) {
            self.out.push(Step::Enter(NodeRef::Item(*i)));
        }
    }

    fn generics(&mut self, g: &Generics) {
        self.mark(Mark::Generics(g.params));
        for gp in self.s.list(g.params) {
            self.out
                .push(Step::Mark(Mark::Open(Group::Generic(gp.binder))));
            for t in self.s.list(gp.bounds) {
                self.out.push(Step::Enter(NodeRef::Ty(*t)));
            }
            if let Some(t) = gp.ty {
                self.out.push(Step::Mark(Mark::Open(Group::Tag("type"))));
                self.out.push(Step::Enter(NodeRef::Ty(t)));
                self.out.push(Step::Mark(Mark::Close));
            }
            if let Some(t) = gp.default {
                self.out.push(Step::Mark(Mark::Open(Group::Tag("default"))));
                self.out.push(Step::Enter(NodeRef::Ty(t)));
                self.out.push(Step::Mark(Mark::Close));
            }
            self.out.push(Step::Mark(Mark::Close));
        }
        for wp in self.s.list(g.preds) {
            self.out.push(Step::Mark(Mark::Open(Group::Where)));
            self.out.push(Step::Enter(NodeRef::Ty(wp.ty)));
            for t in self.s.list(wp.bounds) {
                self.out.push(Step::Enter(NodeRef::Ty(*t)));
            }
            self.out.push(Step::Mark(Mark::Close));
        }
    }

    fn item(&mut self, id: ItemId) {
        let Some(item) = self.s.item(id) else { return };
        self.mark(Mark::Frame(FrameOf::Item(id)));
        self.mark(Mark::Scope);
        match &item.kind {
            ItemKind::Fn(f) => {
                self.generics(&f.generics);
                for p in self.s.list(f.params) {
                    self.out.push(Step::Enter(NodeRef::Param(*p)));
                }
                self.tagged_ty("ret", f.ret);
                self.tagged_ty("throws", f.throws);
                self.opt_expr(f.body);
            }
            ItemKind::Record(r) => {
                self.generics(&r.generics);
                for f in self.s.list(r.fields) {
                    self.out.push(Step::Enter(NodeRef::Field(*f)));
                }
            }
            ItemKind::Sum(d) => {
                self.generics(&d.generics);
                for v in self.s.list(d.variants) {
                    self.out.push(Step::Enter(NodeRef::Variant(*v)));
                }
            }
            ItemKind::Class(c) => {
                self.generics(&c.generics);
                self.tagged_ty("base", c.base);
                if !c.interfaces.is_empty() {
                    self.open(Group::Tag("implements"));
                    self.tys(c.interfaces);
                    self.close();
                }
                for f in self.s.list(c.fields) {
                    self.out.push(Step::Enter(NodeRef::Field(*f)));
                }
                self.items(c.items);
            }
            ItemKind::Interface(i) => {
                self.generics(&i.generics);
                self.tys(i.supers);
                self.items(i.items);
            }
            ItemKind::Impl(i) => {
                self.generics(&i.generics);
                self.tagged_ty("interface", i.interface);
                self.ty_node(i.self_ty);
                self.items(i.items);
            }
            ItemKind::Alias { generics, ty } => {
                self.generics(generics);
                self.ty_node(*ty);
            }
            ItemKind::AssocType { bounds, default } => {
                self.tys(*bounds);
                self.tagged_ty("default", *default);
            }
            ItemKind::Const { ty, value } => {
                self.opt_ty(*ty);
                self.opt_expr(*value);
            }
            ItemKind::Global { ty, init, .. } => {
                self.opt_ty(*ty);
                self.opt_expr(*init);
            }
            ItemKind::Module { items } => self.items(*items),
            ItemKind::Import { path, .. } => self.node(NodeRef::Path(*path)),
            ItemKind::Err => {}
        }
        self.mark(Mark::PopScope);
        self.mark(Mark::PopFrame);
    }

    fn arms(&mut self, arms: List<Arm>, group: Group) {
        for arm in self.s.list(arms) {
            self.out.push(Step::Mark(Mark::Open(group)));
            self.out.push(Step::Mark(Mark::Scope));
            self.out.push(Step::Enter(NodeRef::Pat(arm.pat)));
            self.out.push(Step::Mark(Mark::Bind(BindSite::Arm)));
            if let Some(g) = arm.guard {
                self.out.push(Step::Mark(Mark::Open(Group::Tag("guard"))));
                self.out.push(Step::Enter(NodeRef::Expr(g)));
                self.out.push(Step::Mark(Mark::Close));
            }
            self.out.push(Step::Enter(NodeRef::Expr(arm.body)));
            self.out.push(Step::Mark(Mark::PopScope));
            self.out.push(Step::Mark(Mark::Close));
        }
    }

    fn args(&mut self, args: List<crate::expr::Arg>) {
        for arg in self.s.list(args) {
            self.out.push(Step::Mark(Mark::Open(Group::Arg(arg.kind))));
            self.out.push(Step::Enter(NodeRef::Expr(arg.value)));
            self.out.push(Step::Mark(Mark::Close));
        }
    }

    fn label_scope(&mut self, label: Option<BinderId>, is_loop: bool) {
        self.mark(Mark::Scope);
        if let Some(l) = label {
            self.mark(Mark::Label(l));
        }
        self.mark(Mark::Loop { label, is_loop });
    }

    fn expr(&mut self, id: ExprId) {
        let Some(expr) = self.s.expr(id) else { return };
        match *expr {
            Expr::Lit(_) | Expr::Continue { .. } | Expr::Err => {}
            Expr::Path(p) => self.node(NodeRef::Path(p)),
            Expr::Tuple(xs) | Expr::Array(xs) => self.exprs(xs),
            Expr::Repeat { elem, count } => {
                self.expr_node(elem);
                self.expr_node(count);
            }
            Expr::Record { path, fields, base } => {
                if let Some(p) = path {
                    self.node(NodeRef::Path(p));
                }
                for fi in self.s.list(fields) {
                    self.out
                        .push(Step::Mark(Mark::Open(Group::FieldInit(fi.name))));
                    self.out.push(Step::Enter(NodeRef::Expr(fi.value)));
                    self.out.push(Step::Mark(Mark::Close));
                }
                if let Some(b) = base {
                    self.tagged_expr("base", b);
                }
            }
            Expr::Map(entries) => {
                for entry in self.s.list(entries) {
                    self.out.push(Step::Mark(Mark::Open(Group::Entry)));
                    if let Some(k) = entry.key {
                        self.out.push(Step::Enter(NodeRef::Expr(k)));
                    }
                    self.out.push(Step::Enter(NodeRef::Expr(entry.value)));
                    self.out.push(Step::Mark(Mark::Close));
                }
            }
            Expr::Call { callee, args } => {
                self.expr_node(callee);
                self.args(args);
            }
            Expr::MethodCall {
                receiver,
                generic_args,
                args,
                ..
            } => {
                self.expr_node(receiver);
                self.tys(generic_args);
                self.args(args);
            }
            Expr::Field { base, .. } => self.expr_node(base),
            Expr::Index { base, index } => {
                self.expr_node(base);
                self.expr_node(index);
            }
            Expr::Op { args, .. } => self.exprs(args),
            Expr::Cast { expr, ty, .. } => {
                self.expr_node(expr);
                self.ty_node(ty);
            }
            Expr::Assign { target, value, .. } => {
                self.expr_node(target);
                self.expr_node(value);
            }
            Expr::Deref(x)
            | Expr::Borrow { expr: x, .. }
            | Expr::Throw(x)
            | Expr::Await(x)
            | Expr::Spawn(x) => self.expr_node(x),
            Expr::Block(block) => {
                self.label_scope(block.label, false);
                for st in self.s.list(block.stmts) {
                    self.out.push(Step::Enter(NodeRef::Stmt(*st)));
                }
                self.opt_expr(block.tail);
                self.mark(Mark::PopLoop);
                self.mark(Mark::PopScope);
            }
            Expr::If { cond, then, else_ } => {
                self.expr_node(cond);
                self.expr_node(then);
                self.opt_expr(else_);
            }
            Expr::Match { scrutinee, arms } => {
                self.expr_node(scrutinee);
                self.arms(arms, Group::Arm);
            }
            Expr::Loop { label, body, step } => {
                self.label_scope(label, true);
                self.expr_node(body);
                if let Some(s) = step {
                    self.tagged_expr("step", s);
                }
                self.mark(Mark::PopLoop);
                self.mark(Mark::PopScope);
            }
            Expr::Break { value, .. } | Expr::Return(value) | Expr::Yield(value) => {
                self.opt_expr(value);
            }
            Expr::Closure(c) => {
                for cap in self.s.list(c.captures) {
                    self.out.push(Step::Mark(Mark::Open(Group::Capture(*cap))));
                    self.out.push(Step::Enter(NodeRef::Path(cap.outer)));
                    self.out.push(Step::Mark(Mark::Close));
                }
                self.mark(Mark::Frame(FrameOf::Closure(id)));
                self.mark(Mark::Scope);
                self.mark(Mark::Captures(c.captures));
                for p in self.s.list(c.params) {
                    self.out.push(Step::Enter(NodeRef::Param(*p)));
                }
                self.tagged_ty("ret", c.ret);
                self.expr_node(c.body);
                self.mark(Mark::PopScope);
                self.mark(Mark::PopFrame);
            }
            Expr::Try {
                body,
                catches,
                finally,
            } => {
                self.mark(Mark::Try);
                self.expr_node(body);
                self.mark(Mark::PopTry);
                self.arms(catches, Group::Catch);
                if let Some(f) = finally {
                    self.open(Group::Tag("finally"));
                    self.mark(Mark::Defer);
                    self.expr_node(f);
                    self.mark(Mark::PopDefer);
                    self.close();
                }
            }
        }
    }

    fn stmt(&mut self, id: crate::id::StmtId) {
        let Some(stmt) = self.s.stmt(id) else { return };
        match *stmt {
            Stmt::Let {
                pat,
                ty,
                init,
                else_,
            } => {
                self.node(NodeRef::Pat(pat));
                self.opt_ty(ty);
                self.opt_expr(init);
                if let Some(e) = else_ {
                    self.tagged_expr("else", e);
                }
                self.mark(Mark::Bind(BindSite::Let));
            }
            Stmt::Expr(e) => self.expr_node(e),
            Stmt::Item(i) => self.node(NodeRef::Item(i)),
            Stmt::Defer(e) => {
                self.mark(Mark::Defer);
                self.expr_node(e);
                self.mark(Mark::PopDefer);
            }
            Stmt::Err => {}
        }
    }

    fn pats(&mut self, list: List<crate::id::PatId>) {
        for p in self.s.list(list) {
            self.out.push(Step::Enter(NodeRef::Pat(*p)));
        }
    }

    fn pat(&mut self, id: crate::id::PatId) {
        let Some(pat) = self.s.pat(id) else { return };
        match *pat {
            Pat::Wild | Pat::Lit(_) | Pat::Range { .. } | Pat::Err => {}
            Pat::Bind { sub, .. } => {
                if let Some(s) = sub {
                    self.node(NodeRef::Pat(s));
                }
            }
            Pat::Tuple { elems, .. } | Pat::Or(elems) => self.pats(elems),
            Pat::Ctor { path, elems, .. } => {
                self.node(NodeRef::Path(path));
                self.pats(elems);
            }
            Pat::Record { path, fields, .. } => {
                if let Some(p) = path {
                    self.node(NodeRef::Path(p));
                }
                for fp in self.s.list(fields) {
                    self.out
                        .push(Step::Mark(Mark::Open(Group::FieldPat(fp.name))));
                    self.out.push(Step::Enter(NodeRef::Pat(fp.pat)));
                    self.out.push(Step::Mark(Mark::Close));
                }
            }
            Pat::Path(p) => self.node(NodeRef::Path(p)),
            Pat::Slice { prefix, rest } => {
                self.pats(prefix);
                if let Some(rest) = rest {
                    self.open(Group::SliceRest);
                    if let Some(b) = rest.bind {
                        self.open(Group::Tag("as"));
                        self.node(NodeRef::Pat(b));
                        self.close();
                    }
                    self.pats(rest.suffix);
                    self.close();
                }
            }
            Pat::Ref { inner, .. } => self.node(NodeRef::Pat(inner)),
            Pat::TypeTest { ty, pat } => {
                self.ty_node(ty);
                if let Some(p) = pat {
                    self.node(NodeRef::Pat(p));
                }
            }
        }
    }

    fn ty(&mut self, id: TyId) {
        let Some(ty) = self.s.ty(id) else { return };
        match *ty {
            Ty::Infer | Ty::Prim(_) | Ty::Any | Ty::Never | Ty::SelfTy | Ty::Err => {}
            Ty::Path(p) => self.node(NodeRef::Path(p)),
            Ty::Tuple(ts) | Ty::Object(ts) => self.tys(ts),
            Ty::Array { elem, len } => {
                self.ty_node(elem);
                self.mark(Mark::Frame(FrameOf::Const { integer: true }));
                self.expr_node(len);
                self.mark(Mark::PopFrame);
            }
            Ty::Slice(t) | Ty::Ptr { inner: t, .. } | Ty::Nullable(t) => self.ty_node(t),
            Ty::Ref { region, inner, .. } => {
                if let Some(r) = region {
                    self.node(NodeRef::Path(r));
                }
                self.ty_node(inner);
            }
            Ty::Fn {
                params,
                ret,
                throws,
                ..
            } => {
                self.tys(params);
                self.tagged_ty("ret", Some(ret));
                self.tagged_ty("throws", throws);
            }
            Ty::Const(e) => {
                self.mark(Mark::Frame(FrameOf::Const { integer: true }));
                self.expr_node(e);
                self.mark(Mark::PopFrame);
            }
        }
    }
}

/// An event of [`Hir::walk_from`](crate::Hir::walk_from).
///
/// # Examples
///
/// ```
/// use hir_lang::{Event, ItemId, NodeRef};
///
/// let root = NodeRef::Item(ItemId::from_index(0).unwrap());
/// assert_eq!(Event::Enter(root).node(), root);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Event {
    /// The walk reached a node; its children follow.
    Enter(NodeRef),
    /// All of the node's children have been walked.
    Leave(NodeRef),
}

impl Event {
    /// Returns the node the event is about.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Event, ExprId, NodeRef};
    ///
    /// let n = NodeRef::Expr(ExprId::from_index(2).unwrap());
    /// assert_eq!(Event::Leave(n).node(), n);
    /// ```
    #[must_use]
    pub const fn node(self) -> NodeRef {
        match self {
            Self::Enter(n) | Self::Leave(n) => n,
        }
    }
}

/// What a walk callback asks for next (only meaningful on [`Event::Enter`]).
///
/// # Examples
///
/// ```
/// use hir_lang::Control;
///
/// assert_eq!(Control::default(), Control::Continue);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Control {
    /// Walk the node's children.
    #[default]
    Continue,
    /// Skip the node's children (its `Leave` event is still delivered).
    Skip,
    /// End the walk now.
    Stop,
}

/// Walks the subtree of `start` in canonical order with an explicit stack.
pub(crate) fn walk_store<F>(store: &Store, start: NodeRef, mut f: F)
where
    F: FnMut(Event) -> Control,
{
    let mut stack: Vec<Step> = Vec::new();
    let mut scratch: Vec<Step> = Vec::new();
    stack.push(Step::Enter(start));
    while let Some(step) = stack.pop() {
        match step {
            Step::Enter(node) => match f(Event::Enter(node)) {
                Control::Stop => return,
                Control::Skip => stack.push(Step::Leave(node)),
                Control::Continue => {
                    stack.push(Step::Leave(node));
                    scratch.clear();
                    expand(store, node, &mut scratch);
                    stack.extend(scratch.drain(..).rev());
                }
            },
            Step::Leave(node) => {
                if f(Event::Leave(node)) == Control::Stop {
                    return;
                }
            }
            Step::Mark(_) => {}
        }
    }
}

/// Pushes the direct children of `node` onto `out`, in canonical order.
pub(crate) fn children_store(store: &Store, node: NodeRef, out: &mut Vec<NodeRef>) {
    let mut steps = Vec::new();
    expand(store, node, &mut steps);
    out.extend(steps.into_iter().filter_map(|s| match s {
        Step::Enter(n) => Some(n),
        _ => None,
    }));
}
