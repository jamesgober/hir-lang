//! The validator: total, linear, and the only door into a [`Hir`](crate::Hir).
//!
//! Pass 1 ([`scan`]) checks each node on its own. Pass 2 ([`tree`]) walks the
//! tree once in canonical order with an explicit stack, checking the tree
//! property, binders, namespaces, control flow, and effects, and computing the
//! scope index; pass 3 (at the end of [`tree`]) reports unreachable nodes and
//! out-of-scope resolutions.
//!
//! In strict mode the first problem is returned. In lenient mode every problem
//! is collected with a [`Repair`]; [`repair`] applies them (turning offending
//! nodes into error nodes) and validates again until the store is clean.

mod repair;
mod scan;
mod tree;

use alloc::vec::Vec;

pub(crate) use repair::validate_lenient;

use crate::{
    def::{Def, DefId, UnitId},
    error::{Capacity, HirError, Malformed, Site},
    id::{BinderId, IdKind, ItemId, MAX_LEN, NodeRef, PatId, PathId},
    item::ItemKind,
    name::{BinderKind, Ns, Path, Res},
    origin::Name,
    store::Store,
};

/// Marks an unset position, scope bound, or owner.
pub(crate) const UNSET: u32 = u32::MAX;

/// The unit and tag of the `Hir` being validated, for `Res::Def` checks.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Ctx {
    pub(crate) unit: UnitId,
    pub(crate) tag: u32,
}

/// One binder's lexical extent, for `lookup_local`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LocalEntry {
    pub(crate) name: Name,
    pub(crate) ns: Ns,
    pub(crate) start: u32,
    pub(crate) end: u32,
    pub(crate) binder: BinderId,
    /// The innermost entry with the same name and namespace whose extent
    /// contains this one, or `UNSET`.
    pub(crate) parent: u32,
}

/// What the validator computed for O(1) checks after construction. A pure
/// function of the arenas.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Index {
    /// Preorder position of each path.
    pub(crate) path_pos: Vec<u32>,
    /// `[value floor, type floor, value ceiling]` at each path (spec §3.2).
    pub(crate) path_floor: Vec<[u32; 3]>,
    /// `[start, end)` preorder range in which each binder is visible.
    pub(crate) binder_scope: Vec<[u32; 2]>,
    /// The frame depth at which each binder was bound.
    pub(crate) binder_depth: Vec<u32>,
    /// The sum item owning each variant (index), or `UNSET`.
    pub(crate) variant_owner: Vec<u32>,
    /// Binder extents sorted by (name, namespace, start), for `lookup_local`.
    pub(crate) locals: Vec<LocalEntry>,
}

/// How a problem is fixed in lenient mode.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Repair {
    /// Replace the node with its kind's error form.
    ErrNode(NodeRef),
    /// Resolve the path to `Res::Err` (and set its namespace).
    ResErr { path: PathId, ns: Option<Ns> },
    /// Replace a binding pattern with a wildcard.
    WildPat(PatId),
    /// Change a binder's kind.
    SetKind(BinderId, BinderKind),
    /// Reset a binder name's mark to the root.
    BinderMarkRoot(BinderId),
    /// Reset a node's origin expansion to the root.
    OriginRoot(NodeRef),
    /// Reset a binder's origin expansion to the root.
    BinderOriginRoot(BinderId),
    /// Point an expansion record's parent and definition site at the root.
    ExpansionRoot(usize),
    /// Remove an entry of the attribute table.
    DropAttr(usize),
}

/// Where problems go: strict mode stops at the first, lenient collects all.
pub(crate) struct Sink {
    pub(crate) lenient: bool,
    pub(crate) problems: Vec<(HirError, Repair, bool)>,
}

impl Sink {
    pub(crate) fn strict() -> Self {
        Self {
            lenient: false,
            problems: Vec::new(),
        }
    }

    pub(crate) fn lenient() -> Self {
        Self {
            lenient: true,
            problems: Vec::new(),
        }
    }

    /// Records a problem (lenient) or fails (strict). `report` is `false` for
    /// cascades of earlier repairs, which are fixed but not shown.
    pub(crate) fn report(
        &mut self,
        err: HirError,
        repair: Repair,
        report: bool,
    ) -> Result<(), HirError> {
        if self.lenient {
            self.problems.push((err, repair, report));
            Ok(())
        } else {
            Err(err)
        }
    }
}

/// Validates `store` strictly with `root` as the root item.
pub(crate) fn validate(store: &Store, root: ItemId, ctx: Ctx) -> Result<Index, HirError> {
    preflight(store, root)?;
    let mut sink = Sink::strict();
    scan::scan(store, root, ctx, &mut sink)?;
    tree::walk(store, root, ctx, &mut sink, &tree::Cascades::default())
}

/// The fatal checks: capacity and the root. No mode can repair these.
pub(crate) fn preflight(store: &Store, root: ItemId) -> Result<(), HirError> {
    let total = [
        store.items.len(),
        store.exprs.len(),
        store.stmts.len(),
        store.pats.len(),
        store.tys.len(),
        store.paths.len(),
        store.fields.len(),
        store.variants.len(),
        store.params.len(),
    ]
    .iter()
    .try_fold(0usize, |acc, n| acc.checked_add(*n));
    if total.is_none_or(|t| t > MAX_LEN) {
        return Err(HirError::CapacityExceeded {
            what: Capacity::Total,
        });
    }
    match store.item(root).map(|i| &i.kind) {
        None => Err(HirError::Dangling {
            site: Site::Root,
            kind: IdKind::Item,
            index: root.index(),
        }),
        Some(ItemKind::Module { .. }) => Ok(()),
        Some(_) => Err(HirError::RootNotModule),
    }
}

/// Checks that `binder` may be referenced from `path` (scope and frames),
/// using a computed index. Shared by the validator and `Hir::resolve`.
pub(crate) fn check_local(
    store: &Store,
    index: &Index,
    path: PathId,
    binder: BinderId,
) -> Result<(), HirError> {
    let pos = index.path_pos.get(path.index()).copied().unwrap_or(UNSET);
    let [start, end] = index
        .binder_scope
        .get(binder.index())
        .copied()
        .unwrap_or([UNSET, UNSET]);
    if pos == UNSET || start == UNSET || pos < start || pos >= end {
        return Err(HirError::OutOfScope { path, binder });
    }
    let depth = index.binder_depth.get(binder.index()).copied().unwrap_or(0);
    let [value_floor, type_floor, value_ceiling] = index
        .path_floor
        .get(path.index())
        .copied()
        .unwrap_or([UNSET, UNSET, 0]);
    let value = store.binder(binder).is_some_and(|b| b.kind.is_value());
    let ok = if value {
        depth >= value_floor && depth < value_ceiling
    } else {
        depth >= type_floor
    };
    if ok {
        Ok(())
    } else {
        Err(HirError::NotCapturable { path, binder })
    }
}

/// The kind of a definition in this unit, for namespace checks.
enum DefKind<'a> {
    Item(&'a ItemKind),
    Variant,
}

/// Looks up a `DefId`: `Ok(None)` for another unit's definition (checked by
/// the host), the local definition's kind otherwise.
fn local_def(
    store: &Store,
    ctx: Ctx,
    path: PathId,
    def: DefId,
) -> Result<Option<DefKind<'_>>, HirError> {
    if def.unit() != ctx.unit {
        return Ok(None);
    }
    if def.tag() != 0 && ctx.tag != 0 && def.tag() != ctx.tag {
        return Err(HirError::ForeignDef { path });
    }
    let site = Site::Node(NodeRef::Path(path));
    match def.def() {
        Def::Item(i) => store
            .item(i)
            .map(|item| Some(DefKind::Item(&item.kind)))
            .ok_or(HirError::Dangling {
                site,
                kind: IdKind::Item,
                index: i.index(),
            }),
        Def::Variant(v) => {
            store
                .variant(v)
                .map(|_| Some(DefKind::Variant))
                .ok_or(HirError::Dangling {
                    site,
                    kind: IdKind::Variant,
                    index: v.index(),
                })
        }
    }
}

/// Checks a resolution (and its unresolved count) against a path's shape and
/// namespace, without the scope check of `Local` binders. Shared by the
/// validator and `Hir::resolve` (spec §3.3).
pub(crate) fn check_res(
    store: &Store,
    ctx: Ctx,
    id: PathId,
    path: &Path,
    res: Res,
    unresolved: u32,
) -> Result<(), HirError> {
    if res == Res::Err {
        return Ok(());
    }
    let shape = || HirError::Malformed {
        site: Site::Node(NodeRef::Path(id)),
        problem: Malformed::PathShape,
    };
    let segments = u32::try_from(path.segments.len()).unwrap_or(u32::MAX);
    if unresolved > segments {
        return Err(shape());
    }
    let prefix = segments - unresolved;
    if prefix == 0 {
        // Everything is type-directed: only from a type root or a qualified self.
        if !(path.root.is_type_root() || path.qself.is_some()) || res != Res::Unresolved {
            return Err(shape());
        }
        return Ok(());
    }
    if let Some(q) = path.qself {
        if res != Res::Unresolved && prefix > q.trait_len {
            return Err(shape());
        }
    }
    let bad = HirError::Resolution { path: id, res };
    let partial = unresolved > 0;
    let allowed = match res {
        Res::Unresolved | Res::Err => true,
        Res::Local(b) => {
            let Some(binder) = store.binder(b) else {
                return Err(HirError::Dangling {
                    site: Site::Node(NodeRef::Path(id)),
                    kind: IdKind::Binder,
                    index: b.index(),
                });
            };
            if partial {
                binder.kind == BinderKind::TypeParam && path.ns != Ns::Region
            } else {
                binder.kind.ns() == Some(path.ns)
            }
        }
        Res::Def(def) => match local_def(store, ctx, id, def)? {
            None => path.ns != Ns::Region,
            Some(DefKind::Variant) => !partial && path.ns != Ns::Region,
            Some(DefKind::Item(kind)) if partial => {
                path.ns != Ns::Region
                    && matches!(
                        kind,
                        ItemKind::Record(_)
                            | ItemKind::Sum(_)
                            | ItemKind::Class(_)
                            | ItemKind::Interface(_)
                            | ItemKind::Alias { .. }
                            | ItemKind::AssocType { .. }
                            | ItemKind::Module { .. }
                            | ItemKind::Err
                    )
            }
            Some(DefKind::Item(kind)) => item_in_ns(kind, path.ns),
        },
        Res::Prim(_) => path.ns == Ns::Type || (partial && path.ns != Ns::Region),
        Res::Extern(_) => path.ns != Ns::Region,
    };
    if allowed { Ok(()) } else { Err(bad) }
}

/// Whether a fully resolved path in `ns` may name an item of this kind.
fn item_in_ns(kind: &ItemKind, ns: Ns) -> bool {
    match ns {
        Ns::Value => matches!(
            kind,
            ItemKind::Fn(_)
                | ItemKind::Const { .. }
                | ItemKind::Global { .. }
                | ItemKind::Record(_)
                | ItemKind::Class(_)
                | ItemKind::Err
        ),
        Ns::Type => matches!(
            kind,
            ItemKind::Record(_)
                | ItemKind::Sum(_)
                | ItemKind::Class(_)
                | ItemKind::Interface(_)
                | ItemKind::Alias { .. }
                | ItemKind::AssocType { .. }
                | ItemKind::Err
        ),
        Ns::Pattern => matches!(
            kind,
            ItemKind::Const { .. } | ItemKind::Record(_) | ItemKind::Class(_) | ItemKind::Err
        ),
        Ns::Region => false,
        Ns::Import => !matches!(
            kind,
            ItemKind::Impl(_) | ItemKind::Import { .. } | ItemKind::MixinUse(_)
        ),
    }
}

/// Builds the sorted binder extents `lookup_local` searches.
pub(crate) fn build_locals(store: &Store, index: &mut Index) {
    let mut entries: Vec<LocalEntry> = Vec::new();
    for (i, binder) in store.binders.nodes.iter().enumerate() {
        let Some(ns) = binder.kind.ns() else { continue };
        let Some(&[start, end]) = index.binder_scope.get(i) else {
            continue;
        };
        if start == UNSET || end == UNSET {
            continue;
        }
        entries.push(LocalEntry {
            name: binder.name,
            ns,
            start,
            end,
            binder: BinderId::from_raw_index(u32::try_from(i).unwrap_or(UNSET)),
            parent: UNSET,
        });
    }
    entries.sort_by(|a, b| {
        (a.name, a.ns, a.start, core::cmp::Reverse(a.end), a.binder).cmp(&(
            b.name,
            b.ns,
            b.start,
            core::cmp::Reverse(b.end),
            b.binder,
        ))
    });
    // Extents of one name are nested or disjoint: a stack finds each parent.
    let mut stack: Vec<u32> = Vec::new();
    for i in 0..entries.len() {
        let Some(&cur) = entries.get(i) else { continue };
        while let Some(&top) = stack.last() {
            let keep = entries.get(top as usize).is_some_and(|t| {
                t.name == cur.name && t.ns == cur.ns && t.start <= cur.start && cur.end <= t.end
            });
            if keep {
                break;
            }
            let _ = stack.pop();
        }
        if let (Some(&top), Some(e)) = (stack.last(), entries.get_mut(i)) {
            e.parent = top;
        }
        stack.push(u32::try_from(i).unwrap_or(UNSET));
    }
    index.locals = entries;
}

/// The innermost binder named `name` in `ns` whose extent contains `pos`.
pub(crate) fn lookup_local(index: &Index, name: Name, ns: Ns, pos: u32) -> Option<BinderId> {
    let locals = &index.locals;
    // Last entry with key (name, ns) and start <= pos.
    let after = locals.partition_point(|e| (e.name, e.ns, e.start) <= (name, ns, pos));
    let mut at = after.checked_sub(1)?;
    loop {
        let e = locals.get(at)?;
        if e.name != name || e.ns != ns {
            return None;
        }
        if e.start <= pos && pos < e.end {
            return Some(e.binder);
        }
        if e.parent == UNSET {
            return None;
        }
        at = e.parent as usize;
    }
}
