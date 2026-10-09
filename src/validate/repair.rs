//! Lenient validation: collect every problem, repair, validate again.
//!
//! A repair replaces an offending node with its kind's error form (or makes a
//! narrower fix: an error resolution, a wildcard pattern, a corrected binder
//! kind). Repairs only ever remove structure, so each round strictly shrinks
//! what can be wrong; consequences of a repair (children left dead, binders
//! left unbound) are fixed silently. Problems are reported once: those found
//! by the first scan and the first walk, in source order.

use alloc::{collections::BTreeSet, vec::Vec};

use super::{
    Ctx, Index, Repair, Sink, preflight, scan,
    tree::{self, Cascades},
};
use crate::{
    error::HirError,
    expr::{Expr, Stmt},
    id::{ExprId, ItemId, List, NodeRef, PatId},
    item::{FieldDef, Item, ItemKind, Param, Shape, Variant},
    name::{Path, PathRoot, Res},
    origin::{ExpnId, Origin},
    pat::Pat,
    store::Store,
    ty::{Effects, Ty},
    walk::{Step, expand},
};

/// Rounds after which every node but the root is turned into an error node,
/// which is valid by construction. Real inputs settle in at most three rounds
/// (problems, their dead children, then their unbound references).
const MAX_ROUNDS: usize = 16;

/// Validates leniently, repairing `store` in place. Returns the index of the
/// repaired store and the problems found, in source order.
///
/// Fails only where no repair exists: capacity and the root.
pub(crate) fn validate_lenient(
    store: &mut Store,
    root: ItemId,
    ctx: Ctx,
) -> Result<(Index, Vec<HirError>), HirError> {
    preflight(store, root)?;
    let mut reported: Vec<(HirError, Origin, usize)> = Vec::new();
    let mut cascades = Cascades::default();
    let mut scanned = false;
    let mut walked = false;
    for _ in 0..MAX_ROUNDS {
        let mut sink = Sink::lenient();
        scan::scan(store, root, ctx, &mut sink)?;
        if !sink.problems.is_empty() {
            if !scanned {
                collect(store, &sink, &mut reported);
            }
            scanned = true;
            apply(store, root, &sink, &mut cascades);
            continue;
        }
        scanned = true;
        let mut sink = Sink::lenient();
        cascades.quiet = walked;
        let index = tree::walk(store, root, ctx, &mut sink, &cascades)?;
        if sink.problems.is_empty() {
            return Ok((index, finish(reported)));
        }
        collect(store, &sink, &mut reported);
        walked = true;
        apply(store, root, &sink, &mut cascades);
    }
    fallback(store, root);
    let mut sink = Sink::strict();
    let index = tree::walk(store, root, ctx, &mut sink, &Cascades::default())?;
    Ok((index, finish(reported)))
}

/// Keeps the problems to report, with their origins for sorting.
fn collect(store: &Store, sink: &Sink, out: &mut Vec<(HirError, Origin, usize)>) {
    for (err, _, show) in &sink.problems {
        if *show {
            let origin = err.node().map_or(Origin::default(), |n| store.origin(n));
            let at = out.len();
            out.push((*err, origin, at));
        }
    }
}

/// Sorts problems into source order (span, then discovery order).
fn finish(mut reported: Vec<(HirError, Origin, usize)>) -> Vec<HirError> {
    reported.sort_by_key(|(_, o, at)| (o.span.start(), o.span.end(), *at));
    reported.into_iter().map(|(e, _, _)| e).collect()
}

fn apply(store: &mut Store, root: ItemId, sink: &Sink, cascades: &mut Cascades) {
    let mut drop_attrs: BTreeSet<usize> = BTreeSet::new();
    for (_, repair, _) in &sink.problems {
        match *repair {
            Repair::ErrNode(node) => {
                note_cascades(store, node, cascades);
                errify(store, root, node);
            }
            Repair::ResErr { path, ns } => {
                if let Some(p) = store.paths.nodes.get_mut(path.index()) {
                    p.res = Res::Err;
                    p.unresolved = 0;
                    if let Some(ns) = ns {
                        p.ns = ns;
                    }
                }
            }
            Repair::WildPat(pat) => {
                note_cascades(store, NodeRef::Pat(pat), cascades);
                if let Some(p) = store.pats.nodes.get_mut(pat.index()) {
                    *p = Pat::Wild;
                }
            }
            Repair::SetKind(b, kind) => {
                if let Some(binder) = store.binders.nodes.get_mut(b.index()) {
                    binder.kind = kind;
                }
            }
            Repair::BinderMarkRoot(b) => {
                if let Some(binder) = store.binders.nodes.get_mut(b.index()) {
                    binder.name.mark = ExpnId::ROOT;
                }
            }
            Repair::OriginRoot(node) => {
                if let Some(o) = origin_mut(store, node) {
                    o.expn = ExpnId::ROOT;
                }
            }
            Repair::BinderOriginRoot(b) => {
                if let Some(o) = store.binders.origins.get_mut(b.index()) {
                    o.expn = ExpnId::ROOT;
                }
            }
            Repair::ExpansionRoot(i) => {
                if let Some(e) = store.expansions.get_mut(i) {
                    e.parent = ExpnId::ROOT;
                    e.def_site = ExpnId::ROOT;
                }
            }
            Repair::DropAttr(i) => {
                let _ = drop_attrs.insert(i);
            }
        }
    }
    for i in drop_attrs.into_iter().rev() {
        if i < store.attrs.len() {
            let _ = store.attrs.remove(i);
        }
    }
}

fn origin_mut(store: &mut Store, node: NodeRef) -> Option<&mut Origin> {
    let i = node.index();
    match node {
        NodeRef::Item(_) => store.items.origins.get_mut(i),
        NodeRef::Expr(_) => store.exprs.origins.get_mut(i),
        NodeRef::Stmt(_) => store.stmts.origins.get_mut(i),
        NodeRef::Pat(_) => store.pats.origins.get_mut(i),
        NodeRef::Ty(_) => store.tys.origins.get_mut(i),
        NodeRef::Path(_) => store.paths.origins.get_mut(i),
        NodeRef::Field(_) => store.fields.origins.get_mut(i),
        NodeRef::Variant(_) => store.variants.origins.get_mut(i),
        NodeRef::Param(_) => store.params.origins.get_mut(i),
    }
}

/// Records what repairing `node` leaves behind: its descendants become dead
/// and the binders bound inside lose their site. Each node is visited once
/// across all repairs of a round, so nested repairs stay linear.
fn note_cascades(store: &Store, node: NodeRef, cascades: &mut Cascades) {
    let mut stack = Vec::new();
    let mut steps = Vec::new();
    steps.clear();
    expand(store, node, &mut steps);
    for s in &steps {
        if let Step::Enter(child) = s {
            stack.push(*child);
        }
    }
    note_binders(store, node, cascades);
    while let Some(n) = stack.pop() {
        if !cascades.dead.insert(n) {
            continue;
        }
        note_binders(store, n, cascades);
        steps.clear();
        expand(store, n, &mut steps);
        for s in &steps {
            if let Step::Enter(child) = s {
                stack.push(*child);
            }
        }
    }
}

/// Notes the binders a node binds directly.
fn note_binders(store: &Store, node: NodeRef, cascades: &mut Cascades) {
    let mut add = |b| {
        let _ = cascades.lost.insert(b);
    };
    match node {
        NodeRef::Pat(p) => {
            if let Some(Pat::Bind { binder, .. } | Pat::Ident { binder, .. }) = store.pat(p) {
                add(*binder);
            }
        }
        NodeRef::Expr(e) => match store.expr(e) {
            Some(Expr::Closure(c)) => {
                for cap in store.list(c.captures) {
                    add(cap.binder);
                }
                if let Some(me) = c.self_binder {
                    add(me);
                }
            }
            Some(Expr::Loop { label: Some(l), .. }) => add(*l),
            Some(Expr::Block(b)) => {
                if let Some(l) = b.label {
                    add(l);
                }
            }
            _ => {}
        },
        NodeRef::Stmt(s) => {
            if let Some(Stmt::Static { binder, .. } | Stmt::Global { binder, .. }) = store.stmt(s) {
                add(*binder);
            }
        }
        NodeRef::Item(i) => {
            let generics = match store.item(i).map(|i| &i.kind) {
                Some(ItemKind::Fn(f)) => Some(f.generics),
                Some(ItemKind::Record(r)) => Some(r.generics),
                Some(ItemKind::Sum(s)) => Some(s.generics),
                Some(ItemKind::Class(c)) => Some(c.generics),
                Some(ItemKind::Interface(x)) => Some(x.generics),
                Some(ItemKind::Impl(x)) => Some(x.generics),
                Some(ItemKind::Alias { generics, .. }) => Some(*generics),
                _ => None,
            };
            if let Some(g) = generics {
                for gp in store.list(g.params) {
                    add(gp.binder);
                }
            }
        }
        _ => {}
    }
}

/// Replaces `node` with its kind's error form. The root stays a module.
pub(crate) fn errify(store: &mut Store, root: ItemId, node: NodeRef) {
    let i = node.index();
    let expansions = store.expansions.len();
    match node {
        NodeRef::Item(id) => {
            if let Some(item) = store.items.nodes.get_mut(i) {
                let kind = if id == root {
                    ItemKind::Module {
                        items: List::EMPTY,
                        body: None,
                        effects: Effects::NONE,
                    }
                } else {
                    ItemKind::Err
                };
                // Keep the name (diagnostics, resolution) unless its mark is bad.
                let name = item
                    .name
                    .filter(|n| (n.mark.as_u32() as usize) <= expansions);
                *item = Item {
                    name,
                    name_span: item.name_span,
                    vis: item.vis,
                    kind,
                };
            }
        }
        NodeRef::Expr(_) => {
            if let Some(e) = store.exprs.nodes.get_mut(i) {
                *e = Expr::Err;
            }
        }
        NodeRef::Stmt(_) => {
            if let Some(s) = store.stmts.nodes.get_mut(i) {
                *s = Stmt::Err;
            }
        }
        NodeRef::Pat(_) => {
            if let Some(p) = store.pats.nodes.get_mut(i) {
                *p = Pat::Err;
            }
        }
        NodeRef::Ty(_) => {
            if let Some(t) = store.tys.nodes.get_mut(i) {
                *t = Ty::Err;
            }
        }
        NodeRef::Path(_) => {
            if let Some(p) = store.paths.nodes.get_mut(i) {
                *p = Path {
                    segments: List::EMPTY,
                    ns: p.ns,
                    root: PathRoot::Relative,
                    qself: None,
                    res: Res::Err,
                    unresolved: 0,
                };
            }
        }
        NodeRef::Field(_) => {
            if let Some(f) = store.fields.nodes.get_mut(i) {
                *f = FieldDef {
                    name: f.name,
                    vis: f.vis,
                    ty: None,
                    default: None,
                };
            }
        }
        NodeRef::Variant(_) => {
            if let Some(v) = store.variants.nodes.get_mut(i) {
                *v = Variant {
                    name: v.name,
                    shape: Shape::Unit,
                    fields: List::EMPTY,
                    discriminant: None,
                };
            }
        }
        NodeRef::Param(_) => {
            let origin = store.params.origin(i);
            let fresh = PatId::from_raw_index(u32::try_from(store.pats.len()).unwrap_or(0));
            store.pats.nodes.push(Pat::Err);
            store.pats.origins.push(origin);
            if let Some(p) = store.params.nodes.get_mut(i) {
                *p = Param {
                    pat: fresh,
                    ty: None,
                    default: None,
                    kind: p.kind,
                    by_ref: p.by_ref,
                };
            }
        }
    }
}

/// Whether an unreachable node is harmless: an error form, or a field,
/// variant, or parameter reduced by a repair. Such nodes may stay in the
/// arenas (dead) without breaking any consumer.
pub(crate) fn is_dead_ok(store: &Store, node: NodeRef) -> bool {
    match node {
        NodeRef::Item(i) => matches!(store.item(i).map(|i| &i.kind), Some(ItemKind::Err)),
        NodeRef::Expr(e) => matches!(store.expr(e), Some(Expr::Err)),
        NodeRef::Stmt(s) => matches!(store.stmt(s), Some(Stmt::Err)),
        NodeRef::Pat(p) => matches!(store.pat(p), Some(Pat::Err)),
        NodeRef::Ty(t) => matches!(store.ty(t), Some(Ty::Err)),
        NodeRef::Path(p) => store
            .path(p)
            .is_some_and(|p| p.segments.is_empty() && p.res == Res::Err),
        NodeRef::Field(f) => store
            .field(f)
            .is_some_and(|f| f.ty.is_none() && f.default.is_none()),
        NodeRef::Variant(v) => store.variant(v).is_some_and(|v| {
            v.shape == Shape::Unit && v.fields.is_empty() && v.discriminant.is_none()
        }),
        NodeRef::Param(p) => store.param(p).is_some_and(|p| {
            p.ty.is_none() && p.default.is_none() && matches!(store.pat(p.pat), Some(Pat::Err))
        }),
    }
}

/// The last resort: an empty root module and every other node an error form.
/// Valid by construction (dead error nodes are allowed; binders may be
/// unbound; expansions were already repaired by the scan rounds).
fn fallback(store: &mut Store, root: ItemId) {
    errify(store, root, NodeRef::Item(root));
    let counts = [
        store.items.len(),
        store.exprs.len(),
        store.stmts.len(),
        store.pats.len(),
        store.tys.len(),
        store.paths.len(),
        store.fields.len(),
        store.variants.len(),
        store.params.len(),
    ];
    for (slot, n) in counts.iter().enumerate() {
        for i in 0..*n {
            let i = u32::try_from(i).unwrap_or(0);
            let node = match slot {
                0 => NodeRef::Item(ItemId::from_raw_index(i)),
                1 => NodeRef::Expr(ExprId::from_raw_index(i)),
                2 => NodeRef::Stmt(crate::id::StmtId::from_raw_index(i)),
                3 => NodeRef::Pat(PatId::from_raw_index(i)),
                4 => NodeRef::Ty(crate::id::TyId::from_raw_index(i)),
                5 => NodeRef::Path(crate::id::PathId::from_raw_index(i)),
                6 => NodeRef::Field(crate::id::FieldId::from_raw_index(i)),
                7 => NodeRef::Variant(crate::id::VariantId::from_raw_index(i)),
                _ => NodeRef::Param(crate::id::ParamId::from_raw_index(i)),
            };
            if node != NodeRef::Item(root) && !is_dead_ok(store, node) {
                errify(store, root, node);
            }
        }
    }
    for b in &mut store.binders.nodes {
        b.name.mark = ExpnId::ROOT;
    }
    for o in &mut store.binders.origins {
        o.expn = ExpnId::ROOT;
    }
    store.attrs.clear();
}
