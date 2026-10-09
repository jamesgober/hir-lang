//! Subtree copies with fresh binders (for template instantiation, inlining,
//! loop unrolling).
//!
//! The copy is a new subtree in the same builder: every node of the subtree
//! is duplicated, every binder *bound inside* the subtree is replaced by a
//! fresh binder (so the copy and the original never share a binding site),
//! and references to binders bound *outside* keep pointing at them. Origins
//! and attributes are copied. Iterative: any depth is safe.

use alloc::{collections::BTreeMap, vec::Vec};

use crate::{
    expr::{Arg, Arm, Capture, Closure, Expr, FieldInit, MapEntry, Stmt},
    id::{
        BinderId, ExprId, FieldId, ItemId, List, MAX_LEN, NodeRef, ParamId, PatId, PathId, StmtId,
        TyId, VariantId,
    },
    intrinsic::{Asm, AsmOperand},
    item::{
        Attr, FieldDef, GenericParam, Generics, Item, ItemKind, MixinAction, MixinRule, Param,
        Variant, WherePred,
    },
    name::{Path, QSelf, Res, Segment},
    origin::Origin,
    pat::{FieldPat, Pat, SliceRest},
    store::{Arena, Pooled, Store},
    ty::{Bound, GenericArg, Ty},
    walk::{Step, expand},
};

/// Copies the subtree of `root`; returns the copy's root, or `None` if an
/// arena or pool would overflow or `root` does not exist.
pub(crate) fn copy_subtree(
    store: &mut Store,
    attrs: &mut Vec<(NodeRef, List<Attr>)>,
    root: NodeRef,
) -> Option<NodeRef> {
    // Preorder, each node once (an invalid shared subtree is copied once and
    // shared again; the validator reports it either way).
    let mut order: Vec<NodeRef> = Vec::new();
    let mut seen: BTreeMap<NodeRef, ()> = BTreeMap::new();
    let mut stack = alloc::vec![root];
    let mut steps = Vec::new();
    while let Some(n) = stack.pop() {
        if store.arena_len(n) <= n.index() || seen.insert(n, ()).is_some() {
            continue;
        }
        order.push(n);
        steps.clear();
        expand(store, n, &mut steps);
        let kids: Vec<NodeRef> = steps
            .iter()
            .filter_map(|s| match s {
                Step::Enter(c) => Some(*c),
                _ => None,
            })
            .collect();
        stack.extend(kids.into_iter().rev());
    }
    let mut m = Mapper {
        nodes: BTreeMap::new(),
        binders: BTreeMap::new(),
    };
    // Fresh binders for everything bound inside.
    for &n in &order {
        for b in bound_by(store, n) {
            if m.binders.contains_key(&b) {
                continue;
            }
            let record = *store.binder(b)?;
            let origin = store.binders.origin(b.index());
            let fresh = push(&mut store.binders, record, origin)?;
            let _ = m.binders.insert(b, BinderId::from_raw_index(fresh));
        }
    }
    // Children before parents.
    for &n in order.iter().rev() {
        let copy = m.copy_node(store, n)?;
        let _ = m.nodes.insert(n, copy);
        let attached: Vec<List<Attr>> = attrs
            .iter()
            .filter(|(t, _)| *t == n)
            .map(|(_, l)| *l)
            .collect();
        for list in attached {
            attrs.push((copy, list));
        }
    }
    m.nodes.get(&root).copied()
}

/// The binders a node binds directly.
fn bound_by(store: &Store, n: NodeRef) -> Vec<BinderId> {
    let mut out = Vec::new();
    match n {
        NodeRef::Pat(p) => {
            if let Some(Pat::Bind { binder, .. } | Pat::Ident { binder, .. }) = store.pat(p) {
                out.push(*binder);
            }
        }
        NodeRef::Expr(e) => match store.expr(e) {
            Some(Expr::Closure(c)) => {
                out.extend(store.list(c.captures).iter().map(|c| c.binder));
                out.extend(c.self_binder);
            }
            Some(Expr::Loop { label, .. }) => out.extend(*label),
            Some(Expr::Block(b)) => out.extend(b.label),
            _ => {}
        },
        NodeRef::Stmt(s) => {
            if let Some(Stmt::Static { binder, .. } | Stmt::Global { binder, .. }) = store.stmt(s) {
                out.push(*binder);
            }
        }
        NodeRef::Item(i) => {
            if let Some(g) = store.item(i).and_then(|i| generics_of(&i.kind)) {
                out.extend(store.list(g.params).iter().map(|g| g.binder));
            }
        }
        _ => {}
    }
    out
}

fn generics_of(kind: &ItemKind) -> Option<Generics> {
    match kind {
        ItemKind::Fn(f) => Some(f.generics),
        ItemKind::Record(r) => Some(r.generics),
        ItemKind::Sum(s) => Some(s.generics),
        ItemKind::Class(c) => Some(c.generics),
        ItemKind::Interface(i) => Some(i.generics),
        ItemKind::Impl(i) => Some(i.generics),
        ItemKind::Alias { generics, .. } => Some(*generics),
        _ => None,
    }
}

fn push<T>(arena: &mut Arena<T>, node: T, origin: Origin) -> Option<u32> {
    let len = arena.nodes.len();
    if len >= MAX_LEN {
        return None;
    }
    arena.nodes.push(node);
    arena.origins.push(origin);
    u32::try_from(len).ok()
}

struct Mapper {
    nodes: BTreeMap<NodeRef, NodeRef>,
    binders: BTreeMap<BinderId, BinderId>,
}

macro_rules! map_id {
    ($name:ident, $ty:ident, $node:ident) => {
        fn $name(&self, id: $ty) -> $ty {
            match self.nodes.get(&NodeRef::$node(id)) {
                Some(NodeRef::$node(new)) => *new,
                _ => id,
            }
        }
    };
}

impl Mapper {
    map_id!(e, ExprId, Expr);
    map_id!(s, StmtId, Stmt);
    map_id!(p, PatId, Pat);
    map_id!(t, TyId, Ty);
    map_id!(i, ItemId, Item);
    map_id!(pa, PathId, Path);
    map_id!(f, FieldId, Field);
    map_id!(v, VariantId, Variant);
    map_id!(pm, ParamId, Param);

    fn b(&self, id: BinderId) -> BinderId {
        self.binders.get(&id).copied().unwrap_or(id)
    }

    /// Copies a list whose elements hold ids but no inner lists.
    fn list<T: Pooled>(
        &self,
        store: &mut Store,
        list: List<T>,
        f: impl Fn(&Self, T) -> T,
    ) -> Option<List<T>> {
        let mapped: Vec<T> = store.list(list).iter().map(|x| f(self, *x)).collect();
        store.push_list(&mapped)
    }

    fn generics(&self, store: &mut Store, g: Generics) -> Option<Generics> {
        let mut params = Vec::new();
        let source: Vec<_> = store.list(g.params).to_vec();
        for gp in source {
            params.push(GenericParam {
                binder: self.b(gp.binder),
                bounds: self.bounds(store, gp.bounds)?,
                ty: gp.ty.map(|t| self.t(t)),
                default: match gp.default {
                    Some(d) => Some(self.generic_arg(store, d)?),
                    None => None,
                },
            });
        }
        let params = store.push_list(&params)?;
        let mut preds = Vec::new();
        let source: Vec<_> = store.list(g.preds).to_vec();
        for wp in source {
            preds.push(WherePred {
                subject: self.bound(wp.subject),
                bounds: self.bounds(store, wp.bounds)?,
            });
        }
        let preds = store.push_list(&preds)?;
        Some(Generics { params, preds })
    }

    fn bound(&self, b: Bound) -> Bound {
        match b {
            Bound::Ty(t) => Bound::Ty(self.t(t)),
            Bound::Region(p) => Bound::Region(self.pa(p)),
        }
    }

    fn bounds(&self, store: &mut Store, list: List<Bound>) -> Option<List<Bound>> {
        self.list(store, list, |m, b| m.bound(b))
    }

    fn generic_arg(&self, store: &mut Store, a: GenericArg) -> Option<GenericArg> {
        Some(match a {
            GenericArg::Ty(t) => GenericArg::Ty(self.t(t)),
            GenericArg::Const(e) => GenericArg::Const(self.e(e)),
            GenericArg::Region(p) => GenericArg::Region(self.pa(p)),
            GenericArg::Binding { name, ty } => GenericArg::Binding {
                name,
                ty: self.t(ty),
            },
            GenericArg::Constraint { name, bounds } => GenericArg::Constraint {
                name,
                bounds: self.bounds(store, bounds)?,
            },
        })
    }

    fn generic_args(&self, store: &mut Store, list: List<GenericArg>) -> Option<List<GenericArg>> {
        let mut out = Vec::new();
        let source: Vec<_> = store.list(list).to_vec();
        for a in source {
            out.push(self.generic_arg(store, a)?);
        }
        store.push_list(&out)
    }

    fn args(&self, store: &mut Store, list: List<Arg>) -> Option<List<Arg>> {
        self.list(store, list, |m, a| Arg {
            value: m.e(a.value),
            ..a
        })
    }

    fn arms(&self, store: &mut Store, list: List<Arm>) -> Option<List<Arm>> {
        self.list(store, list, |m, a| Arm {
            pat: m.p(a.pat),
            guard: a.guard.map(|g| m.e(g)),
            body: m.e(a.body),
        })
    }

    fn tys(&self, store: &mut Store, list: List<TyId>) -> Option<List<TyId>> {
        self.list(store, list, |m, t| m.t(t))
    }

    fn copy_node(&self, store: &mut Store, n: NodeRef) -> Option<NodeRef> {
        let origin = store.origin(n);
        Some(match n {
            NodeRef::Expr(id) => {
                let expr = *store.expr(id)?;
                let new = self.expr(store, expr)?;
                NodeRef::Expr(ExprId::from_raw_index(push(&mut store.exprs, new, origin)?))
            }
            NodeRef::Stmt(id) => {
                let new = self.stmt(*store.stmt(id)?);
                NodeRef::Stmt(StmtId::from_raw_index(push(&mut store.stmts, new, origin)?))
            }
            NodeRef::Pat(id) => {
                let pat = *store.pat(id)?;
                let new = self.pat(store, pat)?;
                NodeRef::Pat(PatId::from_raw_index(push(&mut store.pats, new, origin)?))
            }
            NodeRef::Ty(id) => {
                let ty = *store.ty(id)?;
                let new = self.ty(store, ty)?;
                NodeRef::Ty(TyId::from_raw_index(push(&mut store.tys, new, origin)?))
            }
            NodeRef::Path(id) => {
                let path = *store.path(id)?;
                let mut segments = Vec::new();
                let source: Vec<_> = store.list(path.segments).to_vec();
                for seg in source {
                    segments.push(Segment {
                        args: self.generic_args(store, seg.args)?,
                        ..seg
                    });
                }
                let res = match path.res {
                    Res::Local(b) => Res::Local(self.b(b)),
                    other => other,
                };
                let new = Path {
                    segments: store.push_list(&segments)?,
                    qself: path.qself.map(|q| QSelf {
                        ty: self.t(q.ty),
                        trait_len: q.trait_len,
                    }),
                    res,
                    ..path
                };
                NodeRef::Path(PathId::from_raw_index(push(&mut store.paths, new, origin)?))
            }
            NodeRef::Field(id) => {
                let f = *store.field(id)?;
                let new = FieldDef {
                    ty: f.ty.map(|t| self.t(t)),
                    default: f.default.map(|e| self.e(e)),
                    ..f
                };
                NodeRef::Field(FieldId::from_raw_index(push(
                    &mut store.fields,
                    new,
                    origin,
                )?))
            }
            NodeRef::Variant(id) => {
                let v = *store.variant(id)?;
                let new = Variant {
                    fields: self.list(store, v.fields, |m, f| m.f(f))?,
                    discriminant: v.discriminant.map(|e| self.e(e)),
                    ..v
                };
                NodeRef::Variant(VariantId::from_raw_index(push(
                    &mut store.variants,
                    new,
                    origin,
                )?))
            }
            NodeRef::Param(id) => {
                let p = *store.param(id)?;
                let new = Param {
                    pat: self.p(p.pat),
                    ty: p.ty.map(|t| self.t(t)),
                    default: p.default.map(|e| self.e(e)),
                    ..p
                };
                NodeRef::Param(ParamId::from_raw_index(push(
                    &mut store.params,
                    new,
                    origin,
                )?))
            }
            NodeRef::Item(id) => {
                let item = *store.item(id)?;
                let kind = self.item_kind(store, item.kind)?;
                let new = Item { kind, ..item };
                NodeRef::Item(ItemId::from_raw_index(push(&mut store.items, new, origin)?))
            }
        })
    }

    fn stmt(&self, stmt: Stmt) -> Stmt {
        match stmt {
            Stmt::Let {
                pat,
                ty,
                init,
                else_,
            } => Stmt::Let {
                pat: self.p(pat),
                ty: ty.map(|t| self.t(t)),
                init: init.map(|e| self.e(e)),
                else_: else_.map(|e| self.e(e)),
            },
            Stmt::Expr(e) => Stmt::Expr(self.e(e)),
            Stmt::Item(i) => Stmt::Item(self.i(i)),
            Stmt::Defer(e) => Stmt::Defer(self.e(e)),
            Stmt::Static { binder, ty, init } => Stmt::Static {
                binder: self.b(binder),
                ty: ty.map(|t| self.t(t)),
                init: init.map(|e| self.e(e)),
            },
            Stmt::Global { binder, path } => Stmt::Global {
                binder: self.b(binder),
                path: self.pa(path),
            },
            Stmt::Err => Stmt::Err,
        }
    }

    fn pat(&self, store: &mut Store, pat: Pat) -> Option<Pat> {
        let pats = |m: &Self, store: &mut Store, l: List<PatId>| m.list(store, l, |m, p| m.p(p));
        Some(match pat {
            Pat::Bind { binder, mode, sub } => Pat::Bind {
                binder: self.b(binder),
                mode,
                sub: sub.map(|p| self.p(p)),
            },
            Pat::Ident { binder, path } => Pat::Ident {
                binder: self.b(binder),
                path: self.pa(path),
            },
            Pat::Range { lo, hi, inclusive } => Pat::Range {
                lo: lo.map(|p| self.p(p)),
                hi: hi.map(|p| self.p(p)),
                inclusive,
            },
            Pat::Tuple { elems, rest } => Pat::Tuple {
                elems: pats(self, store, elems)?,
                rest,
            },
            Pat::Ctor { path, elems, rest } => Pat::Ctor {
                path: self.pa(path),
                elems: pats(self, store, elems)?,
                rest,
            },
            Pat::Record { path, fields, rest } => Pat::Record {
                path: path.map(|p| self.pa(p)),
                fields: self.list(store, fields, |m, f| FieldPat {
                    pat: m.p(f.pat),
                    ..f
                })?,
                rest,
            },
            Pat::Path(p) => Pat::Path(self.pa(p)),
            Pat::Slice { prefix, rest } => Pat::Slice {
                prefix: pats(self, store, prefix)?,
                rest: match rest {
                    Some(r) => Some(SliceRest {
                        bind: r.bind.map(|p| self.p(p)),
                        suffix: pats(self, store, r.suffix)?,
                    }),
                    None => None,
                },
            },
            Pat::Or(alts) => Pat::Or(pats(self, store, alts)?),
            Pat::Ref { mutable, inner } => Pat::Ref {
                mutable,
                inner: self.p(inner),
            },
            Pat::TypeTest { ty, pat } => Pat::TypeTest {
                ty: self.t(ty),
                pat: pat.map(|p| self.p(p)),
            },
            other @ (Pat::Wild | Pat::Lit(_) | Pat::Err) => other,
        })
    }

    fn ty(&self, store: &mut Store, ty: Ty) -> Option<Ty> {
        Some(match ty {
            Ty::Path(p) => Ty::Path(self.pa(p)),
            Ty::Tuple(ts) => Ty::Tuple(self.tys(store, ts)?),
            Ty::Array { elem, len } => Ty::Array {
                elem: self.t(elem),
                len: self.e(len),
            },
            Ty::Slice(t) => Ty::Slice(self.t(t)),
            Ty::Ref {
                mutable,
                region,
                inner,
            } => Ty::Ref {
                mutable,
                region: region.map(|p| self.pa(p)),
                inner: self.t(inner),
            },
            Ty::Ptr { mutable, inner } => Ty::Ptr {
                mutable,
                inner: self.t(inner),
            },
            Ty::Fn {
                params,
                ret,
                effects,
                throws,
                abi,
            } => Ty::Fn {
                params: self.tys(store, params)?,
                ret: self.t(ret),
                effects,
                throws: throws.map(|t| self.t(t)),
                abi,
            },
            Ty::Nullable(t) => Ty::Nullable(self.t(t)),
            Ty::Object(bs) => Ty::Object(self.bounds(store, bs)?),
            Ty::Impl(bs) => Ty::Impl(self.bounds(store, bs)?),
            other @ (Ty::Infer | Ty::Prim(_) | Ty::Any | Ty::Never | Ty::SelfTy | Ty::Err) => other,
        })
    }

    fn item_kind(&self, store: &mut Store, kind: ItemKind) -> Option<ItemKind> {
        let items = |m: &Self, store: &mut Store, l: List<ItemId>| m.list(store, l, |m, i| m.i(i));
        Some(match kind {
            ItemKind::Fn(f) => ItemKind::Fn(crate::item::FnDef {
                generics: self.generics(store, f.generics)?,
                params: self.list(store, f.params, |m, p| m.pm(p))?,
                ret: f.ret.map(|t| self.t(t)),
                throws: f.throws.map(|t| self.t(t)),
                body: f.body.map(|e| self.e(e)),
                ..f
            }),
            ItemKind::Record(r) => ItemKind::Record(crate::item::RecordDef {
                generics: self.generics(store, r.generics)?,
                fields: self.list(store, r.fields, |m, f| m.f(f))?,
                ..r
            }),
            ItemKind::Sum(s) => ItemKind::Sum(crate::item::SumDef {
                generics: self.generics(store, s.generics)?,
                variants: self.list(store, s.variants, |m, v| m.v(v))?,
            }),
            ItemKind::Class(c) => ItemKind::Class(crate::item::ClassDef {
                generics: self.generics(store, c.generics)?,
                bases: self.tys(store, c.bases)?,
                interfaces: self.tys(store, c.interfaces)?,
                fields: self.list(store, c.fields, |m, f| m.f(f))?,
                items: items(self, store, c.items)?,
                ..c
            }),
            ItemKind::Interface(i) => ItemKind::Interface(crate::item::InterfaceDef {
                generics: self.generics(store, i.generics)?,
                supers: self.tys(store, i.supers)?,
                items: items(self, store, i.items)?,
            }),
            ItemKind::Impl(i) => ItemKind::Impl(crate::item::ImplDef {
                generics: self.generics(store, i.generics)?,
                interface: i.interface.map(|t| self.t(t)),
                self_ty: self.t(i.self_ty),
                items: items(self, store, i.items)?,
            }),
            ItemKind::Alias { generics, ty } => ItemKind::Alias {
                generics: self.generics(store, generics)?,
                ty: self.t(ty),
            },
            ItemKind::AssocType { bounds, default } => ItemKind::AssocType {
                bounds: self.bounds(store, bounds)?,
                default: default.map(|t| self.t(t)),
            },
            ItemKind::Const { ty, value } => ItemKind::Const {
                ty: ty.map(|t| self.t(t)),
                value: value.map(|e| self.e(e)),
            },
            ItemKind::Global { ty, mutable, init } => ItemKind::Global {
                ty: ty.map(|t| self.t(t)),
                mutable,
                init: init.map(|e| self.e(e)),
            },
            ItemKind::Module {
                items: list,
                body,
                effects,
            } => ItemKind::Module {
                items: items(self, store, list)?,
                body: body.map(|e| self.e(e)),
                effects,
            },
            ItemKind::Import { path, glob } => ItemKind::Import {
                path: self.pa(path),
                glob,
            },
            ItemKind::MixinUse(m) => {
                let mixins = self.tys(store, m.mixins)?;
                let mut rules = Vec::new();
                let source: Vec<_> = store.list(m.rules).to_vec();
                for r in source {
                    let action = match r.action {
                        MixinAction::Insteadof(l) => MixinAction::Insteadof(self.tys(store, l)?),
                        other => other,
                    };
                    rules.push(MixinRule {
                        from: r.from.map(|t| self.t(t)),
                        action,
                        ..r
                    });
                }
                ItemKind::MixinUse(crate::item::MixinUseDef {
                    mixins,
                    rules: store.push_list(&rules)?,
                })
            }
            ItemKind::Err => ItemKind::Err,
        })
    }

    fn expr(&self, store: &mut Store, expr: Expr) -> Option<Expr> {
        let exprs = |m: &Self, store: &mut Store, l: List<ExprId>| m.list(store, l, |m, e| m.e(e));
        Some(match expr {
            Expr::Path(p) => Expr::Path(self.pa(p)),
            Expr::Tuple(xs) => Expr::Tuple(exprs(self, store, xs)?),
            Expr::Array(xs) => Expr::Array(exprs(self, store, xs)?),
            Expr::Repeat { elem, count } => Expr::Repeat {
                elem: self.e(elem),
                count: self.e(count),
            },
            Expr::Record { path, fields, base } => Expr::Record {
                path: path.map(|p| self.pa(p)),
                fields: self.list(store, fields, |m, f| FieldInit {
                    value: m.e(f.value),
                    ..f
                })?,
                base: base.map(|e| self.e(e)),
            },
            Expr::Map(entries) => Expr::Map(self.list(store, entries, |m, e| MapEntry {
                key: e.key.map(|k| m.e(k)),
                value: m.e(e.value),
            })?),
            Expr::Call { callee, args } => Expr::Call {
                callee: self.e(callee),
                args: self.args(store, args)?,
            },
            Expr::MethodCall {
                receiver,
                method,
                generic_args,
                args,
            } => Expr::MethodCall {
                receiver: self.e(receiver),
                method,
                generic_args: self.generic_args(store, generic_args)?,
                args: self.args(store, args)?,
            },
            Expr::DynMethodCall {
                receiver,
                name,
                args,
            } => Expr::DynMethodCall {
                receiver: self.e(receiver),
                name: self.e(name),
                args: self.args(store, args)?,
            },
            Expr::Field { base, member, span } => Expr::Field {
                base: self.e(base),
                member,
                span,
            },
            Expr::DynField { base, name } => Expr::DynField {
                base: self.e(base),
                name: self.e(name),
            },
            Expr::VarVar(x) => Expr::VarVar(self.e(x)),
            Expr::Append(x) => Expr::Append(self.e(x)),
            Expr::Index { base, index } => Expr::Index {
                base: self.e(base),
                index: self.e(index),
            },
            Expr::Op { op, args } => Expr::Op {
                op,
                args: exprs(self, store, args)?,
            },
            Expr::Cast { expr, ty, policy } => Expr::Cast {
                expr: self.e(expr),
                ty: self.t(ty),
                policy,
            },
            Expr::Assign { target, op, value } => Expr::Assign {
                target: self.e(target),
                op,
                value: self.e(value),
            },
            Expr::RefAssign { target, source } => Expr::RefAssign {
                target: self.e(target),
                source: self.e(source),
            },
            Expr::Deref(x) => Expr::Deref(self.e(x)),
            Expr::Borrow { kind, expr } => Expr::Borrow {
                kind,
                expr: self.e(expr),
            },
            Expr::Block(b) => Expr::Block(crate::expr::Block {
                stmts: self.list(store, b.stmts, |m, s| m.s(s))?,
                tail: b.tail.map(|e| self.e(e)),
                label: b.label.map(|l| self.b(l)),
                is_unsafe: b.is_unsafe,
            }),
            Expr::If { cond, then, else_ } => Expr::If {
                cond: self.e(cond),
                then: self.e(then),
                else_: else_.map(|e| self.e(e)),
            },
            Expr::Match { scrutinee, arms } => Expr::Match {
                scrutinee: self.e(scrutinee),
                arms: self.arms(store, arms)?,
            },
            Expr::Loop { label, body, step } => Expr::Loop {
                label: label.map(|l| self.b(l)),
                body: self.e(body),
                step: step.map(|e| self.e(e)),
            },
            Expr::Break { label, value } => Expr::Break {
                label: label.map(|l| self.b(l)),
                value: value.map(|e| self.e(e)),
            },
            Expr::Continue { label } => Expr::Continue {
                label: label.map(|l| self.b(l)),
            },
            Expr::Return(v) => Expr::Return(v.map(|e| self.e(e))),
            Expr::Closure(c) => Expr::Closure(Closure {
                params: self.list(store, c.params, |m, p| m.pm(p))?,
                ret: c.ret.map(|t| self.t(t)),
                body: self.e(c.body),
                captures: self.list(store, c.captures, |m, cap| Capture {
                    outer: m.pa(cap.outer),
                    binder: m.b(cap.binder),
                    mode: cap.mode,
                })?,
                self_binder: c.self_binder.map(|b| self.b(b)),
                ..c
            }),
            Expr::Throw(x) => Expr::Throw(self.e(x)),
            Expr::Try {
                body,
                catches,
                finally,
            } => Expr::Try {
                body: self.e(body),
                catches: self.arms(store, catches)?,
                finally: finally.map(|e| self.e(e)),
            },
            Expr::Await(x) => Expr::Await(self.e(x)),
            Expr::Yield(v) => Expr::Yield(v.map(|e| self.e(e))),
            Expr::Spawn(x) => Expr::Spawn(self.e(x)),
            Expr::Asm(a) => Expr::Asm(Asm {
                operands: self.list(store, a.operands, |m, op| AsmOperand {
                    expr: m.e(op.expr),
                    ..op
                })?,
                ..a
            }),
            Expr::Intrinsic {
                kind,
                generic_args,
                args,
            } => Expr::Intrinsic {
                kind,
                generic_args: self.generic_args(store, generic_args)?,
                args: exprs(self, store, args)?,
            },
            other @ (Expr::Lit(_) | Expr::Err) => other,
        })
    }
}
