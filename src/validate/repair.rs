//! Lenient validation: collect every problem, repair, validate again.
//!
//! A repair replaces an offending node with its kind's error form (or makes a
//! narrower fix: an error resolution, a wildcard pattern, a corrected binder
//! kind, a root mark). Each round also removes the **consequences of its own
//! repairs** before the next pass: nodes that the repairs made unreachable
//! become error nodes, and resolutions naming a binder whose binding site
//! disappeared (or whose kind a repair changed) become `Res::Err`. Those
//! consequences are fixed without a report; every problem a pass finds is
//! reported.
//!
//! **At most two repair rounds** (spec §13.2): one for the node-local scan
//! and one for the tree walk. The argument:
//!
//! - *Scan, then rescan.* A scan problem is a property of one node, repaired
//!   by changing that node (error form, root mark, dropped attribute entry).
//!   Error forms pass every scan check, and every scan check that looks at a
//!   child accepts the child's error form (`Expr::Err` is a place, `Pat::Err`
//!   a range bound, an error path a path, and members with error forms are
//!   exempt from union ordering). So the rescan finds nothing.
//! - *Walk, then rewalk.* A walk problem is repaired at the node it names
//!   (or the parent of a shared node), and all its consequences are removed
//!   in the same round: what became unreachable is an error node, and every
//!   reference to a binder that lost its site or changed kind is `Res::Err`.
//!   A repair never changes what a surviving node may see: frames, loops,
//!   `try` bodies and effects come from ancestors, which survive, and binder
//!   scopes come from binding sites, which either survive unchanged or are
//!   gone (their references then repaired). Every problem a construct has
//!   is reported in the same pass (all repeats of a binding, every
//!   or-pattern alternative, every unreachable node, every out-of-scope
//!   path), and a second binding site is reported as such without touching
//!   the binder's kind. So the rewalk finds nothing, and since walk repairs
//!   only produce error forms, `Res::Err` and wildcards, nothing new for the
//!   scan either.
//!
//! The debug assertion on the round count is exercised by the lenient
//! property tests. Independently of the argument, the loop terminates:
//! every round strictly reduces the number of non-error nodes, non-`Err`
//! resolutions, non-root marks and attribute entries, except kind changes,
//! which happen at most once per binder; the hard limit below turns a
//! broken argument into an error instead of a hang.

use alloc::vec::Vec;

use super::{Ctx, Index, Repair, Sink, preflight, scan, tree};
use crate::{
    error::HirError,
    expr::{Expr, Stmt},
    id::{BinderId, ItemId, List, NodeRef, PatId},
    item::{FieldDef, Item, ItemKind, Param, Shape, Variant},
    name::{Path, PathRoot, Res},
    origin::{ExpnId, Origin},
    pat::Pat,
    store::Store,
    ty::{Effects, Ty},
    walk::{Step, direct_binders, expand},
};

/// The rounds the argument above allows; checked in debug builds.
const PROVEN_ROUNDS: usize = 2;

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
    // A bound on the rounds that does not depend on the argument: each round
    // removes at least one unit of the measure, which is at most this.
    let limit = store
        .items
        .len()
        .saturating_add(store.exprs.len())
        .saturating_add(store.stmts.len())
        .saturating_add(store.pats.len())
        .saturating_add(store.tys.len())
        .saturating_add(store.paths.len().saturating_mul(2))
        .saturating_add(store.fields.len())
        .saturating_add(store.variants.len())
        .saturating_add(store.params.len())
        .saturating_add(store.binders.len().saturating_mul(3))
        .saturating_add(store.expansions.len())
        .saturating_add(store.attrs.len())
        .saturating_add(2);
    let mut rounds = 0usize;
    loop {
        let mut sink = Sink::lenient();
        scan::scan(store, root, ctx, &mut sink)?;
        if sink.problems.is_empty() {
            sink = Sink::lenient();
            let index = tree::walk(store, root, ctx, &mut sink)?;
            if sink.problems.is_empty() {
                debug_assert!(
                    rounds <= PROVEN_ROUNDS,
                    "lenient repair took {rounds} rounds"
                );
                return Ok((index, finish(reported)));
            }
        }
        rounds += 1;
        if rounds > limit {
            // Only reachable if the termination argument is wrong.
            let first = sink.problems.first().map(|(e, _, _)| *e);
            return Err(first.unwrap_or(HirError::RootNotModule));
        }
        collect(store, &sink, &mut reported);
        round(store, root, &sink);
    }
}

/// One repair round: apply the repairs, then remove their consequences.
fn round(store: &mut Store, root: ItemId, sink: &Sink) {
    let before = Live::of(store, root);
    let changed_kind = apply(store, root, sink);
    let after = Live::of(store, root);
    // Nodes the repairs cut off become error nodes.
    for node in before.nodes_not_in(&after) {
        if !is_dead_ok(store, node) {
            errify(store, root, node);
        }
    }
    // References to binders that lost their site or changed kind.
    for (i, path) in store.paths.nodes.iter_mut().enumerate() {
        let Res::Local(b) = path.res else { continue };
        let live = after.path(i);
        let lost = before.binds(b) && !after.binds(b);
        let changed = changed_kind.get(b.index()).copied().unwrap_or(false);
        if live && (lost || changed) {
            path.res = Res::Err;
            path.unresolved = 0;
        }
    }
}

/// The nodes reachable from the root, and the binders bound by them.
struct Live {
    nodes: [Vec<bool>; 9],
    bound: Vec<bool>,
}

impl Live {
    fn of(store: &Store, root: ItemId) -> Self {
        let mut live = Self {
            nodes: [
                alloc::vec![false; store.items.len()],
                alloc::vec![false; store.exprs.len()],
                alloc::vec![false; store.stmts.len()],
                alloc::vec![false; store.pats.len()],
                alloc::vec![false; store.tys.len()],
                alloc::vec![false; store.paths.len()],
                alloc::vec![false; store.fields.len()],
                alloc::vec![false; store.variants.len()],
                alloc::vec![false; store.params.len()],
            ],
            bound: alloc::vec![false; store.binders.len()],
        };
        let mut stack = alloc::vec![NodeRef::Item(root)];
        let mut steps = Vec::new();
        while let Some(node) = stack.pop() {
            let Some(seen) = live
                .nodes
                .get_mut(slot(node))
                .and_then(|v| v.get_mut(node.index()))
            else {
                continue;
            };
            if *seen {
                continue;
            }
            *seen = true;
            direct_binders(store, node, |b| {
                if let Some(x) = live.bound.get_mut(b.index()) {
                    *x = true;
                }
            });
            steps.clear();
            expand(store, node, &mut steps);
            for step in &steps {
                if let Step::Enter(child) = step {
                    stack.push(*child);
                }
            }
        }
        live
    }

    fn contains(&self, node: NodeRef) -> bool {
        self.nodes
            .get(slot(node))
            .and_then(|v| v.get(node.index()))
            .copied()
            .unwrap_or(false)
    }

    fn path(&self, i: usize) -> bool {
        self.nodes
            .get(5)
            .and_then(|v| v.get(i))
            .copied()
            .unwrap_or(false)
    }

    fn binds(&self, b: BinderId) -> bool {
        self.bound.get(b.index()).copied().unwrap_or(false)
    }

    /// The nodes live in `self` but not in `other`, in arena order.
    fn nodes_not_in(&self, other: &Self) -> Vec<NodeRef> {
        let mut out = Vec::new();
        for (k, v) in self.nodes.iter().enumerate() {
            for (i, live) in v.iter().enumerate() {
                if !*live {
                    continue;
                }
                let node = node_at(k, i);
                if !other.contains(node) {
                    out.push(node);
                }
            }
        }
        out
    }
}

fn slot(node: NodeRef) -> usize {
    match node {
        NodeRef::Item(_) => 0,
        NodeRef::Expr(_) => 1,
        NodeRef::Stmt(_) => 2,
        NodeRef::Pat(_) => 3,
        NodeRef::Ty(_) => 4,
        NodeRef::Path(_) => 5,
        NodeRef::Field(_) => 6,
        NodeRef::Variant(_) => 7,
        NodeRef::Param(_) => 8,
    }
}

fn node_at(slot: usize, i: usize) -> NodeRef {
    let i = u32::try_from(i).unwrap_or(u32::MAX - 1);
    match slot {
        0 => NodeRef::Item(ItemId::from_raw_index(i)),
        1 => NodeRef::Expr(crate::id::ExprId::from_raw_index(i)),
        2 => NodeRef::Stmt(crate::id::StmtId::from_raw_index(i)),
        3 => NodeRef::Pat(PatId::from_raw_index(i)),
        4 => NodeRef::Ty(crate::id::TyId::from_raw_index(i)),
        5 => NodeRef::Path(crate::id::PathId::from_raw_index(i)),
        6 => NodeRef::Field(crate::id::FieldId::from_raw_index(i)),
        7 => NodeRef::Variant(crate::id::VariantId::from_raw_index(i)),
        _ => NodeRef::Param(crate::id::ParamId::from_raw_index(i)),
    }
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

/// Applies a round's repairs. Returns which binders changed kind.
fn apply(store: &mut Store, root: ItemId, sink: &Sink) -> Vec<bool> {
    let mut changed_kind = alloc::vec![false; store.binders.len()];
    let mut drop_attrs: Vec<usize> = Vec::new();
    for (_, repair, _) in &sink.problems {
        match *repair {
            Repair::ErrNode(node) => errify(store, root, node),
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
                if let Some(p) = store.pats.nodes.get_mut(pat.index()) {
                    *p = Pat::Wild;
                }
            }
            Repair::SetKind(b, kind) => {
                if let Some(binder) = store.binders.nodes.get_mut(b.index()) {
                    if binder.kind != kind {
                        binder.kind = kind;
                        if let Some(c) = changed_kind.get_mut(b.index()) {
                            *c = true;
                        }
                    }
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
            Repair::DropAttr(i) => drop_attrs.push(i),
        }
    }
    drop_attrs.sort_unstable();
    drop_attrs.dedup();
    for i in drop_attrs.into_iter().rev() {
        if i < store.attrs.len() {
            let _ = store.attrs.remove(i);
        }
    }
    changed_kind
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
