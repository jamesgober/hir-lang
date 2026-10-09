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

use alloc::{collections::BTreeSet, vec::Vec};

use crate::{
    expr::{Arg, ArgKind, Arm, Capture, Expr, Stmt},
    id::{BinderId, ExprId, ItemId, List, NodeRef, ParamId, PatId, StmtId, TyId},
    intrinsic::AsmOperand,
    item::{GenericParam, Generics, ItemKind, MixinRule},
    name::{BinderKind, Ns},
    origin::Ident,
    pat::Pat,
    store::Store,
    ty::{Bound, GenericArg, Ty},
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
    /// A parameter default; the owner (function or closure) decides when it
    /// is evaluated.
    Default(ParamId),
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
    Arg(Arg),
    FieldPat(Ident),
    SliceRest,
    SegmentArgs(u32),
    Binding(Ident),
    Constraint(Ident),
    AsmOperand(AsmOperand),
    MixinRule(MixinRule),
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
    /// The binders of a construct's root pattern become visible.
    Bind(BindSite, PatId),
    /// A declaration's binder (`static`, `global`, a `let_place` alias)
    /// becomes visible; the kind its site requires.
    BindDecl(BinderId, BinderKind),
    /// The explicit captures' binders become visible.
    Captures(List<Capture>),
    /// A closure's self binder becomes visible.
    SelfBinder(BinderId),
    /// A loop or block label becomes visible.
    Label(BinderId),
    Loop {
        label: Option<BinderId>,
        is_loop: bool,
    },
    PopLoop,
    /// The innermost loop's `step` begins and ends.
    StepBegin,
    StepEnd,
    Try,
    PopTry,
    /// A `defer` body: no jump may leave it, no `return` inside.
    Defer,
    PopDefer,
    /// The next node is a path in this namespace.
    ExpectNs(Ns),
    Open(Group),
    Close,
}

/// Pushes children and marks of `node`, in canonical order, onto `out`.
///
/// The node's own `Enter`/`Leave` are not included. Ids that do not resolve in
/// `store` contribute nothing (the validator checks ranges before it walks).
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
                if let Some(q) = path.qself {
                    e.open(Group::Tag("qself"));
                    e.ty_node(q.ty);
                    e.close();
                }
                for (i, seg) in store.list(path.segments).iter().enumerate() {
                    if !seg.args.is_empty() {
                        let i = u32::try_from(i).unwrap_or(u32::MAX);
                        e.open(Group::SegmentArgs(i));
                        e.generic_args(seg.args);
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
                    e.mark(Mark::Frame(FrameOf::Default(id)));
                    e.tagged_expr("default", d);
                    e.mark(Mark::PopFrame);
                }
                e.mark(Mark::Bind(BindSite::Param, param.pat));
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

    fn path(&mut self, id: crate::id::PathId, ns: Ns) {
        self.mark(Mark::ExpectNs(ns));
        self.node(NodeRef::Path(id));
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

    fn bound(&mut self, b: Bound) {
        match b {
            Bound::Ty(t) => self.ty_node(t),
            Bound::Region(p) => self.path(p, Ns::Region),
        }
    }

    fn bounds(&mut self, list: List<Bound>) {
        for b in self.s.list(list) {
            self.bound(*b);
        }
    }

    fn generic_arg(&mut self, arg: GenericArg) {
        match arg {
            GenericArg::Ty(t) => self.ty_node(t),
            GenericArg::Const(e) => {
                self.open(Group::Tag("const"));
                self.mark(Mark::Frame(FrameOf::Const { integer: true }));
                self.expr_node(e);
                self.mark(Mark::PopFrame);
                self.close();
            }
            GenericArg::Region(p) => self.path(p, Ns::Region),
            GenericArg::Binding { name, ty } => {
                self.open(Group::Binding(name));
                self.ty_node(ty);
                self.close();
            }
            GenericArg::Constraint { name, bounds } => {
                self.open(Group::Constraint(name));
                self.bounds(bounds);
                self.close();
            }
        }
    }

    fn generic_args(&mut self, list: List<GenericArg>) {
        for a in self.s.list(list) {
            self.generic_arg(*a);
        }
    }

    fn generics(&mut self, g: &Generics) {
        self.mark(Mark::Generics(g.params));
        for gp in self.s.list(g.params) {
            self.open(Group::Generic(gp.binder));
            self.bounds(gp.bounds);
            if let Some(t) = gp.ty {
                self.open(Group::Tag("type"));
                self.ty_node(t);
                self.close();
            }
            if let Some(d) = gp.default {
                self.open(Group::Tag("default"));
                self.generic_arg(d);
                self.close();
            }
            self.close();
        }
        for wp in self.s.list(g.preds) {
            self.open(Group::Where);
            self.bound(wp.subject);
            self.bounds(wp.bounds);
            self.close();
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
                if !c.bases.is_empty() {
                    self.open(Group::Tag("bases"));
                    self.tys(c.bases);
                    self.close();
                }
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
                self.bounds(*bounds);
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
            ItemKind::Module { items, body, .. } => {
                self.items(*items);
                self.opt_expr(*body);
            }
            ItemKind::Import { path, .. } => self.path(*path, Ns::Import),
            ItemKind::MixinUse(m) => {
                self.tys(m.mixins);
                for rule in self.s.list(m.rules) {
                    self.out
                        .push(Step::Mark(Mark::Open(Group::MixinRule(*rule))));
                    if let Some(from) = rule.from {
                        self.out.push(Step::Enter(NodeRef::Ty(from)));
                    }
                    if let crate::item::MixinAction::Insteadof(others) = rule.action {
                        for t in self.s.list(others) {
                            self.out.push(Step::Enter(NodeRef::Ty(*t)));
                        }
                    }
                    self.out.push(Step::Mark(Mark::Close));
                }
            }
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
            self.out
                .push(Step::Mark(Mark::Bind(BindSite::Arm, arm.pat)));
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

    fn args(&mut self, args: List<Arg>) {
        for arg in self.s.list(args) {
            self.out.push(Step::Mark(Mark::Open(Group::Arg(*arg))));
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
            Expr::Path(p) => self.path(p, Ns::Value),
            Expr::Tuple(xs) | Expr::Array(xs) => self.exprs(xs),
            Expr::Repeat { elem, count } => {
                self.expr_node(elem);
                self.expr_node(count);
            }
            Expr::Record { path, fields, base } => {
                if let Some(p) = path {
                    self.path(p, Ns::Type);
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
                self.generic_args(generic_args);
                self.args(args);
            }
            Expr::DynMethodCall {
                receiver,
                name,
                args,
            } => {
                self.expr_node(receiver);
                self.tagged_expr("name", name);
                self.args(args);
            }
            Expr::Field { base, .. } => self.expr_node(base),
            Expr::DynField { base, name } => {
                self.expr_node(base);
                self.tagged_expr("name", name);
            }
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
            Expr::RefAssign { target, source } => {
                self.expr_node(target);
                self.expr_node(source);
            }
            Expr::Deref(x)
            | Expr::Borrow { expr: x, .. }
            | Expr::Throw(x)
            | Expr::Await(x)
            | Expr::Spawn(x)
            | Expr::VarVar(x)
            | Expr::Append(x) => self.expr_node(x),
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
                    self.open(Group::Tag("step"));
                    self.mark(Mark::StepBegin);
                    self.expr_node(s);
                    self.mark(Mark::StepEnd);
                    self.close();
                }
                self.mark(Mark::PopLoop);
                self.mark(Mark::PopScope);
            }
            Expr::Break { value, .. } | Expr::Return(value) => self.opt_expr(value),
            Expr::Yield { key, value } => {
                if let Some(k) = key {
                    self.tagged_expr("key", k);
                }
                self.opt_expr(value);
            }
            Expr::YieldFrom(x) => self.expr_node(x),
            Expr::LetPlace {
                binder,
                place,
                body,
            } => {
                self.expr_node(place);
                self.mark(Mark::Scope);
                self.mark(Mark::BindDecl(binder, BinderKind::Place));
                self.expr_node(body);
                self.mark(Mark::PopScope);
            }
            Expr::Closure(c) => {
                for cap in self.s.list(c.captures) {
                    self.out.push(Step::Mark(Mark::Open(Group::Capture(*cap))));
                    self.out.push(Step::Mark(Mark::ExpectNs(Ns::Value)));
                    self.out.push(Step::Enter(NodeRef::Path(cap.outer)));
                    self.out.push(Step::Mark(Mark::Close));
                }
                self.mark(Mark::Frame(FrameOf::Closure(id)));
                self.mark(Mark::Scope);
                if let Some(me) = c.self_binder {
                    self.mark(Mark::SelfBinder(me));
                }
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
                    self.tagged_expr("finally", f);
                }
            }
            Expr::Asm(asm) => {
                for op in self.s.list(asm.operands) {
                    self.out
                        .push(Step::Mark(Mark::Open(Group::AsmOperand(*op))));
                    self.out.push(Step::Enter(NodeRef::Expr(op.expr)));
                    self.out.push(Step::Mark(Mark::Close));
                }
            }
            Expr::Intrinsic {
                generic_args, args, ..
            } => {
                self.generic_args(generic_args);
                self.exprs(args);
            }
        }
    }

    fn stmt(&mut self, id: StmtId) {
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
                self.mark(Mark::Bind(BindSite::Let, pat));
            }
            Stmt::Expr(e) => self.expr_node(e),
            Stmt::Item(i) => self.node(NodeRef::Item(i)),
            Stmt::Defer(e) => {
                self.mark(Mark::Defer);
                self.expr_node(e);
                self.mark(Mark::PopDefer);
            }
            Stmt::Static { binder, ty, init } => {
                self.opt_ty(ty);
                self.opt_expr(init);
                self.mark(Mark::BindDecl(binder, BinderKind::Local));
            }
            Stmt::Global { binder, path } => {
                self.path(path, Ns::Value);
                self.mark(Mark::BindDecl(binder, BinderKind::Local));
            }
            Stmt::Err => {}
        }
    }

    fn pats(&mut self, list: List<PatId>) {
        for p in self.s.list(list) {
            self.out.push(Step::Enter(NodeRef::Pat(*p)));
        }
    }

    fn pat(&mut self, id: PatId) {
        let Some(pat) = self.s.pat(id) else { return };
        match *pat {
            Pat::Wild | Pat::Lit(_) | Pat::Err => {}
            Pat::Bind { sub, .. } => {
                if let Some(s) = sub {
                    self.node(NodeRef::Pat(s));
                }
            }
            Pat::Ident { path, .. } => self.path(path, Ns::Pattern),
            Pat::Range { lo, hi, .. } => {
                if let Some(lo) = lo {
                    self.open(Group::Tag("lo"));
                    self.node(NodeRef::Pat(lo));
                    self.close();
                }
                if let Some(hi) = hi {
                    self.open(Group::Tag("hi"));
                    self.node(NodeRef::Pat(hi));
                    self.close();
                }
            }
            Pat::Tuple { elems, .. } | Pat::Or(elems) => self.pats(elems),
            Pat::Ctor { path, elems, .. } => {
                self.path(path, Ns::Pattern);
                self.pats(elems);
            }
            Pat::Record { path, fields, .. } => {
                if let Some(p) = path {
                    self.path(p, Ns::Type);
                }
                for fp in self.s.list(fields) {
                    self.out
                        .push(Step::Mark(Mark::Open(Group::FieldPat(fp.name))));
                    self.out.push(Step::Enter(NodeRef::Pat(fp.pat)));
                    self.out.push(Step::Mark(Mark::Close));
                }
            }
            Pat::Path(p) => self.path(p, Ns::Pattern),
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
            Ty::Path(p) => self.path(p, Ns::Type),
            Ty::Tuple(ts) | Ty::Union(ts) | Ty::Intersection(ts) => self.tys(ts),
            Ty::Object(bs) | Ty::Impl(bs) => self.bounds(bs),
            Ty::Array { elem, len } => {
                self.ty_node(elem);
                self.mark(Mark::Frame(FrameOf::Const { integer: true }));
                self.expr_node(len);
                self.mark(Mark::PopFrame);
            }
            Ty::Slice(t) | Ty::Ptr { inner: t, .. } | Ty::Nullable(t) => self.ty_node(t),
            Ty::Ref { region, inner, .. } => {
                if let Some(r) = region {
                    self.path(r, Ns::Region);
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
        }
    }
}

/// The kind of a frame, as delivered by [`Event::FrameOpen`].
///
/// # Examples
///
/// ```
/// use hir_lang::{Frame, ItemId};
///
/// let f = Frame::Item(ItemId::from_index(0).unwrap());
/// assert!(matches!(f, Frame::Item(_)));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Frame {
    /// An item (its generics, parameters, body).
    Item(ItemId),
    /// A closure.
    Closure(ExprId),
    /// A parameter's default expression.
    Default(ParamId),
    /// A constant context (array length, const argument, discriminant, field
    /// default).
    Const,
}

/// An event of [`Hir::walk_from`](crate::Hir::walk_from).
///
/// Besides entering and leaving nodes, the walk reports the scoping structure
/// the validator uses, so a resolver never re-derives the scope rules: a
/// `Bind(b)` arrives exactly where `b` becomes visible, and `b` stays visible
/// until the `ScopeClose` matching the innermost `ScopeOpen` around it.
/// Frames bracket items, closures, parameter defaults, and constant contexts.
///
/// # Examples
///
/// ```
/// use hir_lang::{Event, ItemId, NodeRef};
///
/// let root = NodeRef::Item(ItemId::from_index(0).unwrap());
/// assert_eq!(Event::Enter(root).node(), Some(root));
/// assert_eq!(Event::ScopeOpen.node(), None);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Event {
    /// The walk reached a node; its children follow.
    Enter(NodeRef),
    /// All of the node's children have been walked.
    Leave(NodeRef),
    /// A scope begins.
    ScopeOpen,
    /// The innermost open scope ends; binders bound in it are no longer
    /// visible.
    ScopeClose,
    /// A binder becomes visible here.
    Bind(BinderId),
    /// A frame begins.
    FrameOpen(Frame),
    /// The innermost frame ends.
    FrameClose,
}

impl Event {
    /// Returns the node an `Enter` or `Leave` is about.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Event, ExprId, NodeRef};
    ///
    /// let n = NodeRef::Expr(ExprId::from_index(2).unwrap());
    /// assert_eq!(Event::Leave(n).node(), Some(n));
    /// ```
    #[must_use]
    pub const fn node(self) -> Option<NodeRef> {
        match self {
            Self::Enter(n) | Self::Leave(n) => Some(n),
            _ => None,
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

/// Appends the binders a root pattern binds, in first-occurrence order, each
/// once (or-pattern alternatives repeat them).
pub(crate) fn pattern_binders(store: &Store, root: PatId, out: &mut Vec<BinderId>) {
    let mut seen: BTreeSet<BinderId> = BTreeSet::new();
    let mut add = |b: BinderId, out: &mut Vec<BinderId>| {
        if seen.insert(b) {
            out.push(b);
        }
    };
    let mut stack = alloc::vec![root];
    while let Some(p) = stack.pop() {
        let Some(pat) = store.pat(p) else { continue };
        match *pat {
            Pat::Bind { binder, sub, .. } => {
                add(binder, out);
                stack.extend(sub);
            }
            Pat::Ident { binder, .. } => add(binder, out),
            Pat::Tuple { elems, .. } | Pat::Ctor { elems, .. } | Pat::Or(elems) => {
                stack.extend(store.list(elems).iter().rev());
            }
            Pat::Record { fields, .. } => {
                stack.extend(store.list(fields).iter().rev().map(|f| f.pat));
            }
            Pat::Slice { prefix, rest } => {
                if let Some(rest) = rest {
                    stack.extend(store.list(rest.suffix).iter().rev());
                    stack.extend(rest.bind);
                }
                stack.extend(store.list(prefix).iter().rev());
            }
            Pat::Ref { inner, .. } => stack.push(inner),
            Pat::TypeTest { pat, .. } => stack.extend(pat),
            Pat::Wild | Pat::Lit(_) | Pat::Range { .. } | Pat::Path(_) | Pat::Err => {}
        }
    }
}

/// Walks the subtree of `start` in canonical order with an explicit stack.
pub(crate) fn walk_store<F>(store: &Store, start: NodeRef, mut f: F)
where
    F: FnMut(Event) -> Control,
{
    let mut stack: Vec<Step> = Vec::new();
    let mut scratch: Vec<Step> = Vec::new();
    let mut binders: Vec<BinderId> = Vec::new();
    stack.push(Step::Enter(start));
    while let Some(step) = stack.pop() {
        let event = match step {
            Step::Enter(node) => {
                match f(Event::Enter(node)) {
                    Control::Stop => return,
                    Control::Skip => stack.push(Step::Leave(node)),
                    Control::Continue => {
                        stack.push(Step::Leave(node));
                        scratch.clear();
                        expand(store, node, &mut scratch);
                        stack.extend(scratch.drain(..).rev());
                    }
                }
                continue;
            }
            Step::Leave(node) => Event::Leave(node),
            Step::Mark(mark) => {
                binders.clear();
                let event = match mark {
                    Mark::Scope => Some(Event::ScopeOpen),
                    Mark::PopScope => Some(Event::ScopeClose),
                    Mark::PopFrame => Some(Event::FrameClose),
                    Mark::Frame(of) => Some(Event::FrameOpen(match of {
                        FrameOf::Item(i) => Frame::Item(i),
                        FrameOf::Closure(c) => Frame::Closure(c),
                        FrameOf::Default(p) => Frame::Default(p),
                        FrameOf::Const { .. } => Frame::Const,
                    })),
                    Mark::Generics(list) => {
                        binders.extend(store.list(list).iter().map(|g| g.binder));
                        None
                    }
                    Mark::Bind(_, pat) => {
                        pattern_binders(store, pat, &mut binders);
                        None
                    }
                    Mark::Captures(list) => {
                        binders.extend(store.list(list).iter().map(|c| c.binder));
                        None
                    }
                    Mark::BindDecl(b, _) | Mark::SelfBinder(b) | Mark::Label(b) => {
                        binders.push(b);
                        None
                    }
                    _ => None,
                };
                for b in &binders {
                    if f(Event::Bind(*b)) == Control::Stop {
                        return;
                    }
                }
                match event {
                    Some(e) => e,
                    None => continue,
                }
            }
        };
        if f(event) == Control::Stop {
            return;
        }
    }
}

/// Calls `f` on each binder `node` binds directly (its own binding site, not
/// its descendants'): a binding pattern, a closure's captures and self
/// binder, a label, a `static`/`global`/`let_place` binder, generic
/// parameters.
pub(crate) fn direct_binders(store: &Store, node: NodeRef, mut f: impl FnMut(BinderId)) {
    match node {
        NodeRef::Pat(p) => {
            if let Some(Pat::Bind { binder, .. } | Pat::Ident { binder, .. }) = store.pat(p) {
                f(*binder);
            }
        }
        NodeRef::Expr(e) => match store.expr(e) {
            Some(Expr::Closure(c)) => {
                for cap in store.list(c.captures) {
                    f(cap.binder);
                }
                if let Some(me) = c.self_binder {
                    f(me);
                }
            }
            Some(Expr::Loop { label: Some(l), .. }) => f(*l),
            Some(Expr::Block(b)) => {
                if let Some(l) = b.label {
                    f(l);
                }
            }
            Some(Expr::LetPlace { binder, .. }) => f(*binder),
            _ => {}
        },
        NodeRef::Stmt(s) => {
            if let Some(Stmt::Static { binder, .. } | Stmt::Global { binder, .. }) = store.stmt(s) {
                f(*binder);
            }
        }
        NodeRef::Item(i) => {
            let generics = match store.item(i).map(|i| &i.kind) {
                Some(ItemKind::Fn(x)) => Some(x.generics),
                Some(ItemKind::Record(x)) => Some(x.generics),
                Some(ItemKind::Sum(x)) => Some(x.generics),
                Some(ItemKind::Class(x)) => Some(x.generics),
                Some(ItemKind::Interface(x)) => Some(x.generics),
                Some(ItemKind::Impl(x)) => Some(x.generics),
                Some(ItemKind::Alias { generics, .. }) => Some(*generics),
                _ => None,
            };
            if let Some(g) = generics {
                for gp in store.list(g.params) {
                    f(gp.binder);
                }
            }
        }
        _ => {}
    }
}

/// Calls `f` on every node of the subtree of `start` in canonical preorder.
/// The node-only fast path behind the Tier-1 [`walk`](crate::walk): no
/// leave, scope, binder, or frame events are produced.
pub(crate) fn walk_nodes<F: FnMut(NodeRef)>(store: &Store, start: NodeRef, mut f: F) {
    let mut stack: Vec<NodeRef> = Vec::new();
    let mut scratch: Vec<Step> = Vec::new();
    stack.push(start);
    while let Some(node) = stack.pop() {
        f(node);
        scratch.clear();
        expand(store, node, &mut scratch);
        stack.extend(scratch.iter().rev().filter_map(|step| match step {
            Step::Enter(child) => Some(*child),
            _ => None,
        }));
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

/// Returns `true` for the arg kinds that may carry `place`.
pub(crate) const fn place_arg_ok(kind: ArgKind) -> bool {
    matches!(kind, ArgKind::Positional | ArgKind::Named(_))
}
