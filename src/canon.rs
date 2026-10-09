//! The canonical form of union and intersection types (spec §5.1).
//!
//! A member's **key** is the preorder token stream of its type subtree: node
//! tags, primitive kinds, path roots, segment names (symbol and mark) and
//! list lengths. Resolutions, namespaces and origins are not part of it, so
//! resolving a path never changes a key. Members are ordered by key
//! (lexicographically); equal keys are repeats.
//!
//! A member has **no key** when its subtree contains an error form, a
//! constant expression (an array length, a const generic argument), a
//! dangling id, or more than [`KEY_BUDGET`] nodes. Such members are exempt
//! from ordering: an erroneous member cannot be compared meaningfully, an
//! expression would need expression equality, and the budget keeps every
//! check linear (each union looks at most `KEY_BUDGET` nodes per member, no
//! matter how deeply unions nest).

use alloc::vec::Vec;
use core::cmp::Ordering;

use crate::{
    error::{Capacity, Malformed},
    id::{IdKind, List, NodeRef, PathId, TyId},
    name::{Path, PathRoot, Res},
    store::Store,
    ty::{Bound, GenericArg, Ty},
    walk::{Step, expand},
};

/// The most type and path nodes a member's key may cover.
pub(crate) const KEY_BUDGET: usize = 64;

/// Work items of the key serializer.
pub(crate) enum Work {
    Ty(TyId),
    Path(PathId),
    Arg(GenericArg),
    Bound(Bound),
}

/// Writes the key of `ty` into `out` (cleared first). Returns `false` if the
/// type has no key.
#[cfg(test)]
pub(crate) fn key(store: &Store, ty: TyId, out: &mut Vec<u32>) -> bool {
    out.clear();
    append_key(store, ty, out, &mut Vec::new())
}

/// Appends the key of `ty` to `out`, using `stack` as scratch. Returns
/// `false` if the type has no key (`out` then holds a partial key).
pub(crate) fn append_key(
    store: &Store,
    ty: TyId,
    out: &mut Vec<u32>,
    stack: &mut Vec<Work>,
) -> bool {
    stack.clear();
    stack.push(Work::Ty(ty));
    let mut nodes = 0usize;
    while let Some(work) = stack.pop() {
        match work {
            Work::Ty(id) => {
                nodes += 1;
                if nodes > KEY_BUDGET {
                    return false;
                }
                let Some(t) = store.ty(id) else { return false };
                match *t {
                    Ty::Infer => out.push(1),
                    Ty::Prim(p) => out.extend([2, p as u32]),
                    Ty::Path(p) => {
                        out.push(3);
                        stack.push(Work::Path(p));
                    }
                    Ty::Tuple(ts) => {
                        out.push(4);
                        if !tys(store, ts, out, stack) {
                            return false;
                        }
                    }
                    Ty::Slice(t) => {
                        out.push(5);
                        stack.push(Work::Ty(t));
                    }
                    Ty::Ref {
                        mutable,
                        region,
                        inner,
                    } => {
                        out.extend([6, u32::from(mutable), u32::from(region.is_some())]);
                        stack.push(Work::Ty(inner));
                        if let Some(r) = region {
                            stack.push(Work::Path(r));
                        }
                    }
                    Ty::Ptr { mutable, inner } => {
                        out.extend([7, u32::from(mutable)]);
                        stack.push(Work::Ty(inner));
                    }
                    Ty::Fn {
                        params,
                        ret,
                        effects,
                        throws,
                        abi,
                    } => {
                        out.extend([
                            8,
                            u32::from(effects.bits()),
                            abi.map_or(0, |s| s.as_u32().saturating_add(1)),
                            u32::from(throws.is_some()),
                        ]);
                        if let Some(t) = throws {
                            stack.push(Work::Ty(t));
                        }
                        stack.push(Work::Ty(ret));
                        if !tys(store, params, out, stack) {
                            return false;
                        }
                    }
                    Ty::Nullable(t) => {
                        out.push(9);
                        stack.push(Work::Ty(t));
                    }
                    Ty::Any => out.push(10),
                    Ty::Object(bs) | Ty::Impl(bs) => {
                        out.push(if matches!(t, Ty::Object(_)) { 11 } else { 12 });
                        let list = store.list(bs);
                        out.push(len(list.len()));
                        stack.extend(list.iter().rev().map(|b| Work::Bound(*b)));
                    }
                    Ty::Never => out.push(13),
                    Ty::SelfTy => out.push(14),
                    Ty::Union(ts) | Ty::Intersection(ts) => {
                        out.push(if matches!(t, Ty::Union(_)) { 15 } else { 16 });
                        if !tys(store, ts, out, stack) {
                            return false;
                        }
                    }
                    Ty::Array { .. } | Ty::Err => return false,
                }
            }
            Work::Path(id) => {
                nodes += 1;
                if nodes > KEY_BUDGET {
                    return false;
                }
                let Some(path) = store.path(id) else {
                    return false;
                };
                if !path_key(store, path, out, stack) {
                    return false;
                }
            }
            Work::Arg(arg) => match arg {
                GenericArg::Ty(t) => {
                    out.push(0);
                    stack.push(Work::Ty(t));
                }
                GenericArg::Const(_) => return false,
                GenericArg::Region(p) => {
                    out.push(1);
                    stack.push(Work::Path(p));
                }
                GenericArg::Binding { name, ty } => {
                    out.extend([2, name.sym.as_u32()]);
                    stack.push(Work::Ty(ty));
                }
                GenericArg::Constraint { name, bounds } => {
                    out.extend([3, name.sym.as_u32()]);
                    let list = store.list(bounds);
                    out.push(len(list.len()));
                    stack.extend(list.iter().rev().map(|b| Work::Bound(*b)));
                }
            },
            Work::Bound(b) => match b {
                Bound::Ty(t) => {
                    out.push(0);
                    stack.push(Work::Ty(t));
                }
                Bound::Region(p) => {
                    out.push(1);
                    stack.push(Work::Path(p));
                }
            },
        }
    }
    true
}

/// Pushes a type list's length and its members (in order) as work.
fn tys(store: &Store, ts: List<TyId>, out: &mut Vec<u32>, stack: &mut Vec<Work>) -> bool {
    let list = store.list(ts);
    if list.len() != ts.len() {
        return false;
    }
    out.push(len(list.len()));
    stack.extend(list.iter().rev().map(|t| Work::Ty(*t)));
    true
}

/// Serializes a path: root, qualified self, segments with their names and
/// generic arguments. The resolution and namespace are left out.
fn path_key(store: &Store, path: &Path, out: &mut Vec<u32>, stack: &mut Vec<Work>) -> bool {
    if path.segments.is_empty() && path.res == Res::Err {
        return false;
    }
    let root = match path.root {
        PathRoot::Relative => 0,
        PathRoot::Global => 1,
        PathRoot::SelfModule => 2,
        PathRoot::SelfType => 3,
        PathRoot::ParentType => 4,
        PathRoot::StaticType => 5,
        PathRoot::Super(n) => 6 + u32::from(n),
    };
    out.push(root);
    let segments = store.list(path.segments);
    if segments.len() != path.segments.len() {
        return false;
    }
    match path.qself {
        Some(q) => out.extend([1, q.trait_len]),
        None => out.push(0),
    }
    out.push(len(segments.len()));
    // Names are emitted now; the segments' generic arguments are serialized
    // after all names (the qualified self first). Still unambiguous: every
    // list carries its length. Work is a stack, so push in reverse.
    for seg in segments {
        out.extend([seg.name.sym.as_u32(), seg.name.mark.as_u32()]);
        let args = store.list(seg.args);
        if args.len() != seg.args.len() {
            return false;
        }
        out.push(len(args.len()));
    }
    for seg in segments.iter().rev() {
        stack.extend(store.list(seg.args).iter().rev().map(|a| Work::Arg(*a)));
    }
    if let Some(q) = path.qself {
        stack.push(Work::Ty(q.ty));
    }
    true
}

fn len(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Checks a union (`union = true`) or intersection member list: at least two
/// members, no nested compound of the same kind (and no `Nullable` in a
/// union), keyed members in strictly increasing key order.
pub(crate) fn check(store: &Store, members: &[TyId], union: bool) -> Result<(), Malformed> {
    if members.len() < 2 {
        return Err(Malformed::TypeArity);
    }
    for m in members {
        let nested = match store.ty(*m) {
            Some(Ty::Union(_)) => union,
            Some(Ty::Nullable(_)) => union,
            Some(Ty::Intersection(_)) => !union,
            _ => false,
        };
        if nested {
            return Err(Malformed::TypeNesting);
        }
    }
    let mut prev: Vec<u32> = Vec::new();
    let mut have_prev = false;
    let mut cur: Vec<u32> = Vec::new();
    let mut stack = Vec::new();
    for m in members {
        cur.clear();
        if !append_key(store, *m, &mut cur, &mut stack) {
            continue;
        }
        if have_prev && prev.as_slice().cmp(cur.as_slice()) != Ordering::Less {
            return Err(Malformed::TypeOrder);
        }
        core::mem::swap(&mut prev, &mut cur);
        have_prev = true;
    }
    Ok(())
}

/// Brings every union and intersection of `store` into canonical form:
/// nested same-kind members are flattened, `Nullable` members of a union are
/// hoisted (`?A | B` becomes `Nullable(A | B)`), keyed members are sorted and
/// repeats removed (keyed members first, then the unkeyed ones in their
/// given order), a single remaining member replaces the compound, and an
/// empty union becomes `never`. Members that are dropped or flattened away
/// become dead error nodes.
///
/// Types are processed children first (an explicit-stack post-order over
/// types and paths), so every member is already canonical when its parent is
/// sorted. Normalization is a pure **tree** rewrite: a union whose member list
/// is out of range, or any of whose touched nodes has more than one parent
/// (or none, or is part of a cycle), is left exactly as it is, so a lowering
/// bug there is still reported by the validator rather than papered over.
/// Costs nothing when the store has no union or intersection.
/// Fails if a pool or the type arena would overflow.
pub(crate) fn normalize(store: &mut Store) -> Result<(), Capacity> {
    if !store
        .tys
        .nodes
        .iter()
        .any(|t| matches!(t, Ty::Union(_) | Ty::Intersection(_)))
    {
        return Ok(());
    }
    let parents = Parents::count(store);
    let n_tys = store.tys.len();
    let n_paths = store.paths.len();
    // 0 = new, 1 = on the stack, 2 = done.
    let mut ty_state = alloc::vec![0u8; n_tys];
    let mut path_state = alloc::vec![0u8; n_paths];
    let mut stack: Vec<(NodeRef, bool)> = Vec::new();
    let mut steps: Vec<Step> = Vec::new();
    let mut scratch = Scratch::default();
    for start in 0..n_tys {
        if ty_state.get(start).copied() != Some(0) {
            continue;
        }
        stack.push((NodeRef::Ty(TyId::from_raw_index(len(start))), false));
        while let Some((node, expanded)) = stack.pop() {
            let state = match node {
                NodeRef::Ty(t) => ty_state.get_mut(t.index()),
                NodeRef::Path(p) => path_state.get_mut(p.index()),
                _ => None,
            };
            let Some(state) = state else { continue };
            if expanded {
                *state = 2;
                if let NodeRef::Ty(t) = node {
                    process(store, t, &ty_state, &parents, &mut scratch)?;
                }
                continue;
            }
            if *state != 0 {
                continue;
            }
            *state = 1;
            stack.push((node, true));
            steps.clear();
            expand(store, node, &mut steps);
            for step in steps.iter().rev() {
                if let Step::Enter(child @ (NodeRef::Ty(_) | NodeRef::Path(_))) = *step {
                    stack.push((child, false));
                }
            }
        }
    }
    Ok(())
}

/// How many structural parents each type and path node has (over every node
/// of every arena, live or not).
struct Parents {
    tys: Vec<u32>,
    paths: Vec<u32>,
}

impl Parents {
    fn count(store: &Store) -> Self {
        let mut parents = Self {
            tys: alloc::vec![0; store.tys.len()],
            paths: alloc::vec![0; store.paths.len()],
        };
        let mut steps = Vec::new();
        let mut visit = |node: NodeRef, parents: &mut Self| {
            steps.clear();
            expand(store, node, &mut steps);
            for step in &steps {
                let slot = match step {
                    Step::Enter(NodeRef::Ty(t)) => parents.tys.get_mut(t.index()),
                    Step::Enter(NodeRef::Path(p)) => parents.paths.get_mut(p.index()),
                    _ => None,
                };
                if let Some(n) = slot {
                    *n = n.saturating_add(1);
                }
            }
        };
        macro_rules! arena {
            ($arena:ident, $id:ty, $kind:ident) => {
                for i in 0..store.$arena.len() {
                    visit(NodeRef::$kind(<$id>::from_raw_index(len(i))), &mut parents);
                }
            };
        }
        arena!(items, crate::id::ItemId, Item);
        arena!(exprs, crate::id::ExprId, Expr);
        arena!(stmts, crate::id::StmtId, Stmt);
        arena!(pats, crate::id::PatId, Pat);
        arena!(tys, TyId, Ty);
        arena!(paths, PathId, Path);
        arena!(fields, crate::id::FieldId, Field);
        arena!(variants, crate::id::VariantId, Variant);
        arena!(params, crate::id::ParamId, Param);
        parents
    }

    /// Exactly one parent. Nodes created during normalization lie past the
    /// counts and have one parent by construction.
    fn one(&self, node: NodeRef) -> bool {
        match node {
            NodeRef::Ty(t) => self.tys.get(t.index()).is_none_or(|n| *n == 1),
            NodeRef::Path(p) => self.paths.get(p.index()).is_none_or(|n| *n == 1),
            _ => false,
        }
    }
}

#[derive(Default)]
struct Scratch {
    members: Vec<TyId>,
    doomed: Vec<TyId>,
    /// All keyed members' tokens, back to back.
    tokens: Vec<u32>,
    /// `(start, end, member index)` of each keyed member's key in `tokens`.
    keys: Vec<(usize, usize, usize)>,
    unkeyed: Vec<TyId>,
    stack: Vec<Work>,
}

/// The members of a same-kind compound being flattened, if its list is in
/// range and every member has exactly one parent.
fn flat_members(store: &Store, list: List<TyId>, parents: &Parents) -> Option<Vec<TyId>> {
    let members = store.list(list);
    if members.len() != list.len() || !members.iter().all(|m| parents.one(NodeRef::Ty(*m))) {
        return None;
    }
    Some(members.to_vec())
}

/// Normalizes one union or intersection whose members are all done. Plans
/// every change first and leaves the node untouched if any touched node is
/// not tree-shaped.
fn process(
    store: &mut Store,
    id: TyId,
    state: &[u8],
    parents: &Parents,
    sc: &mut Scratch,
) -> Result<(), Capacity> {
    let (list, union) = match store.ty(id) {
        Some(Ty::Union(l)) => (*l, true),
        Some(Ty::Intersection(l)) => (*l, false),
        _ => return Ok(()),
    };
    // Nodes created by earlier processing lie past `state` and are canonical.
    let done = |t: TyId| state.get(t.index()).is_none_or(|s| *s == 2);
    let one = |t: TyId| parents.one(NodeRef::Ty(t));
    let originals = store.list(list);
    if originals.len() != list.len() || !one(id) {
        return Ok(());
    }
    let originals = originals.to_vec();
    sc.members.clear();
    sc.doomed.clear();
    let mut nullable = false;
    for m in originals {
        if !one(m) || m == id {
            return Ok(());
        }
        match store.ty(m).copied() {
            Some(Ty::Union(inner)) if union && done(m) => {
                let Some(inner) = flat_members(store, inner, parents) else {
                    return Ok(());
                };
                sc.members.extend(inner);
                sc.doomed.push(m);
            }
            Some(Ty::Intersection(inner)) if !union && done(m) => {
                let Some(inner) = flat_members(store, inner, parents) else {
                    return Ok(());
                };
                sc.members.extend(inner);
                sc.doomed.push(m);
            }
            Some(Ty::Nullable(x)) if union && done(m) => {
                nullable = true;
                sc.doomed.push(m);
                // `??T` is `?T`: unwrap every directly nested `Nullable`.
                let mut x = x;
                let mut links = 0usize;
                loop {
                    if !one(x) || x == id {
                        return Ok(());
                    }
                    match store.ty(x).copied() {
                        Some(Ty::Nullable(y)) if done(x) && links < KEY_BUDGET => {
                            sc.doomed.push(x);
                            x = y;
                            links += 1;
                        }
                        Some(Ty::Union(inner)) if done(x) => {
                            let Some(inner) = flat_members(store, inner, parents) else {
                                return Ok(());
                            };
                            sc.members.extend(inner);
                            sc.doomed.push(x);
                            break;
                        }
                        _ => {
                            sc.members.push(x);
                            break;
                        }
                    }
                }
            }
            _ => sc.members.push(m),
        }
    }
    // The plan is tree-shaped: apply it.
    for d in &sc.doomed {
        kill(store, NodeRef::Ty(*d));
    }
    // Sort keyed members (stable), drop repeats, keep unkeyed ones after.
    sc.keys.clear();
    sc.unkeyed.clear();
    sc.tokens.clear();
    for (i, m) in sc.members.iter().enumerate() {
        let start = sc.tokens.len();
        if append_key(store, *m, &mut sc.tokens, &mut sc.stack) {
            sc.keys.push((start, sc.tokens.len(), i));
        } else {
            sc.tokens.truncate(start);
            sc.unkeyed.push(*m);
        }
    }
    let tokens = &sc.tokens;
    let slice = |(a, b, _): (usize, usize, usize)| tokens.get(a..b).unwrap_or(&[]);
    sc.keys
        .sort_by(|x, y| slice(*x).cmp(slice(*y)).then(x.2.cmp(&y.2)));
    let mut kept: Vec<TyId> = Vec::with_capacity(sc.members.len());
    let mut dropped: Vec<TyId> = Vec::new();
    let mut last: Option<&[u32]> = None;
    for k in &sc.keys {
        let Some(m) = sc.members.get(k.2).copied() else {
            continue;
        };
        let this = slice(*k);
        if last == Some(this) {
            dropped.push(m);
            continue;
        }
        last = Some(this);
        kept.push(m);
    }
    kept.extend_from_slice(&sc.unkeyed);
    for d in dropped {
        kill_subtree(store, NodeRef::Ty(d), parents);
    }
    // Rebuild the node.
    let body = match kept.as_slice() {
        [] => Ty::Never,
        [m] if nullable => {
            set(store, id, Ty::Nullable(*m));
            return Ok(());
        }
        [m] => match store.ty(*m).copied() {
            Some(single) => {
                // The compound takes its only member's place.
                kill(store, NodeRef::Ty(*m));
                single
            }
            None => return Ok(()),
        },
        _ => {
            let Some(list) = store.push_list(&kept) else {
                return Err(Capacity::Pool);
            };
            if union {
                Ty::Union(list)
            } else {
                Ty::Intersection(list)
            }
        }
    };
    if nullable && matches!(body, Ty::Union(_)) {
        // `id` becomes `Nullable(new)`, where `new` holds the union.
        let origin = store.tys.origin(id.index());
        if store.tys.len() >= crate::id::MAX_LEN {
            return Err(Capacity::Arena(IdKind::Ty));
        }
        let new = TyId::from_raw_index(len(store.tys.len()));
        store.tys.nodes.push(body);
        store.tys.origins.push(origin);
        set(store, id, Ty::Nullable(new));
    } else {
        set(store, id, body);
    }
    Ok(())
}

fn set(store: &mut Store, id: TyId, ty: Ty) {
    if let Some(slot) = store.tys.nodes.get_mut(id.index()) {
        *slot = ty;
    }
}

/// Turns one type or path node into a dead error node (its children stay as
/// they are).
fn kill(store: &mut Store, node: NodeRef) {
    match node {
        NodeRef::Ty(t) => set(store, t, Ty::Err),
        NodeRef::Path(p) => {
            if let Some(slot) = store.paths.nodes.get_mut(p.index()) {
                *slot = Path {
                    segments: List::EMPTY,
                    ns: slot.ns,
                    root: PathRoot::Relative,
                    qself: None,
                    res: Res::Err,
                    unresolved: 0,
                };
            }
        }
        _ => {}
    }
}

/// Turns a dropped (single-parent) member and the single-parent nodes under
/// it into dead error nodes. A keyed member contains only types and paths, at
/// most `KEY_BUDGET` of them; a node with another parent is left alone.
fn kill_subtree(store: &mut Store, root: NodeRef, parents: &Parents) {
    let mut stack = alloc::vec![root];
    let mut steps = Vec::new();
    let mut seen = 0usize;
    while let Some(node) = stack.pop() {
        seen += 1;
        if seen > KEY_BUDGET || !parents.one(node) {
            continue;
        }
        steps.clear();
        expand(store, node, &mut steps);
        for step in &steps {
            if let Step::Enter(child @ (NodeRef::Ty(_) | NodeRef::Path(_))) = *step {
                stack.push(child);
            }
        }
        kill(store, node);
    }
}

#[cfg(test)]
mod tests {
    use intern_lang::Interner;

    use crate::{Builder, HirError, Malformed, Name, NodeRef, Ns, Prim, Site, Ty, TyId};

    /// `fn main() { let _: <ty> }` over a module, returning the builder, the
    /// root and the type.
    fn with_ty(b: &mut Builder, names: &mut Interner, ty: TyId) -> crate::ItemId {
        let pat = b.pat(crate::Pat::Wild);
        let stmt = b.stmt(crate::Stmt::Let {
            pat,
            ty: Some(ty),
            init: None,
            else_: None,
        });
        let body = b.block(&[stmt], None);
        let f = b.func(Name::new(names.intern("main")), &[], body);
        b.module(None, &[f])
    }

    fn named(b: &mut Builder, names: &mut Interner, s: &str) -> TyId {
        let p = b.name_path(Name::new(names.intern(s)), Ns::Type);
        b.ty(Ty::Path(p))
    }

    fn problem(result: Result<crate::Hir, HirError>) -> Option<Malformed> {
        match result {
            Err(HirError::Malformed { problem, .. }) => Some(problem),
            _ => None,
        }
    }

    #[test]
    fn test_keys_order_prims_before_paths_and_ignore_resolution() {
        let mut names = Interner::new();
        let mut b = Builder::new();
        let int = b.ty(Ty::Prim(Prim::I64));
        let s = named(&mut b, &mut names, "S");
        let (mut ki, mut ks) = (alloc::vec::Vec::new(), alloc::vec::Vec::new());
        assert!(super::key(b.store_for_tests(), int, &mut ki));
        assert!(super::key(b.store_for_tests(), s, &mut ks));
        assert!(ki < ks);
    }

    #[test]
    fn test_validator_rejects_non_canonical_unions() {
        // one member
        let mut names = Interner::new();
        let mut b = Builder::new();
        let int = b.ty(Ty::Prim(Prim::I64));
        let l = b.list(&[int]);
        let u = b.ty(Ty::Union(l));
        let root = with_ty(&mut b, &mut names, u);
        assert_eq!(
            problem(b.finish_unnormalized(root)),
            Some(Malformed::TypeArity)
        );

        // out of order: path before prim
        let mut b = Builder::new();
        let s = named(&mut b, &mut names, "S");
        let int = b.ty(Ty::Prim(Prim::I64));
        let l = b.list(&[s, int]);
        let u = b.ty(Ty::Union(l));
        let root = with_ty(&mut b, &mut names, u);
        assert_eq!(
            problem(b.finish_unnormalized(root)),
            Some(Malformed::TypeOrder)
        );

        // a repeat
        let mut b = Builder::new();
        let a = b.ty(Ty::Prim(Prim::I64));
        let c = b.ty(Ty::Prim(Prim::I64));
        let l = b.list(&[a, c]);
        let u = b.ty(Ty::Union(l));
        let root = with_ty(&mut b, &mut names, u);
        assert_eq!(
            problem(b.finish_unnormalized(root)),
            Some(Malformed::TypeOrder)
        );

        // a nested union, and a nullable member
        for nullable in [false, true] {
            let mut b = Builder::new();
            // `bool` sorts before `i64`, so the inner union is canonical.
            let a = b.ty(Ty::Prim(Prim::Bool));
            let c = b.ty(Ty::Prim(Prim::I64));
            let inner = if nullable {
                b.ty(Ty::Nullable(c))
            } else {
                let l = b.list(&[a, c]);
                b.ty(Ty::Union(l))
            };
            let d = b.ty(Ty::Prim(Prim::Str));
            let l = b.list(&[inner, d]);
            let u = b.ty(Ty::Union(l));
            let root = with_ty(&mut b, &mut names, u);
            assert_eq!(
                problem(b.finish_unnormalized(root)),
                Some(Malformed::TypeNesting)
            );
        }
    }

    #[test]
    fn test_unkeyed_members_are_exempt_from_ordering() {
        // [i64; n] | str | i64: the array (a constant expression) has no key.
        let mut names = Interner::new();
        let mut b = Builder::new();
        let elem = b.ty(Ty::Prim(Prim::I64));
        let len = b.int(3);
        let arr = b.ty(Ty::Array { elem, len });
        let s = b.ty(Ty::Prim(Prim::Str));
        let i = b.ty(Ty::Prim(Prim::I64));
        let l = b.list(&[arr, i, s]);
        let u = b.ty(Ty::Union(l));
        let root = with_ty(&mut b, &mut names, u);
        assert!(b.finish_unnormalized(root).is_ok());
    }

    #[test]
    fn test_lenient_repairs_a_non_canonical_union_in_one_round() {
        let mut names = Interner::new();
        let mut b = Builder::new();
        let s = named(&mut b, &mut names, "S");
        let int = b.ty(Ty::Prim(Prim::I64));
        let l = b.list(&[s, int]);
        let u = b.ty(Ty::Union(l));
        let root = with_ty(&mut b, &mut names, u);
        let result = b.finish_lenient_unnormalized(root);
        assert!(result.is_ok());
        if let Ok((hir, problems)) = result {
            assert_eq!(
                problems,
                [HirError::Malformed {
                    site: Site::Node(NodeRef::Ty(u)),
                    problem: Malformed::TypeOrder
                }]
            );
            assert_eq!(hir.ty(u), &Ty::Err);
            assert_eq!(hir.validate(), Ok(()));
        }
    }

    #[test]
    fn test_normalize_never_hides_a_lowering_bug() {
        let mut names = Interner::new();
        // A member list out of range stays out of range (not `never`).
        let mut b = Builder::new();
        let u = b.ty(Ty::Union(crate::List::from_raw(40, 3)));
        let root = with_ty(&mut b, &mut names, u);
        assert!(matches!(
            b.finish(root),
            Err(HirError::ListOutOfBounds { .. })
        ));

        // The same member twice is left as it is (not deduplicated away), so
        // the scan reports the repeat (and the walk would report the sharing).
        let mut b = Builder::new();
        let i = b.ty(Ty::Prim(Prim::I64));
        let l = b.list(&[i, i]);
        let u = b.ty(Ty::Union(l));
        let root = with_ty(&mut b, &mut names, u);
        assert_eq!(problem(b.finish(root)), Some(Malformed::TypeOrder));

        // An inner union with a second parent is not flattened away.
        let mut b = Builder::new();
        let (x, y, z) = (
            b.ty(Ty::Prim(Prim::Bool)),
            b.ty(Ty::Prim(Prim::I64)),
            b.ty(Ty::Prim(Prim::Str)),
        );
        let inner_l = b.list(&[x, y]);
        let inner = b.ty(Ty::Union(inner_l));
        let outer_l = b.list(&[inner, z]);
        let outer = b.ty(Ty::Union(outer_l));
        let other = b.ty(Ty::Slice(inner));
        let tl = b.list(&[outer, other]);
        let tuple = b.ty(Ty::Tuple(tl));
        let root = with_ty(&mut b, &mut names, tuple);
        // Left nested, so the scan reports the nesting before the walk would
        // report the second parent.
        assert_eq!(problem(b.finish(root)), Some(Malformed::TypeNesting));
    }

    #[test]
    fn test_normalize_is_total_on_a_union_containing_itself() {
        let mut names = Interner::new();
        let mut b = Builder::new();
        // ty 0 = Union([ty 0, ty 1]): a cycle only garbage can contain.
        let int = b.ty(Ty::Prim(Prim::I64));
        let me = TyId::from_raw_index(1);
        let l = b.list(&[me, int]);
        let u = b.ty(Ty::Union(l));
        assert_eq!(u, me);
        let root = with_ty(&mut b, &mut names, u);
        assert!(b.finish(root).is_err());
    }
}
