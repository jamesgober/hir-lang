//! The validator: total, linear, and the only door into a [`Hir`](crate::Hir).
//!
//! Pass 1 ([`scan`]) checks each node on its own. Pass 2 walks the tree once in
//! canonical order with an explicit stack, checking the tree property, binders,
//! namespaces, control flow, and effects, and computing the scope index. Pass 3
//! reports unreachable nodes, unbound binders, and out-of-scope resolutions.

mod scan;

use alloc::vec::Vec;

use crate::{
    error::{Capacity, EffectProblem, HirError, JumpProblem, Malformed, Site},
    expr::Expr,
    id::{BinderId, ExprId, ItemId, MAX_LEN, NodeRef, PatId, PathId},
    item::{ItemKind, ParamKind},
    name::{BinderKind, Ns, Res},
    pat::Pat,
    store::Store,
    ty::{Effects, Ty},
    walk::{BindSite, FrameOf, Mark, Step, expand},
};

/// Marks an unset position, scope bound, or owner.
const UNSET: u32 = u32::MAX;

/// What the validator computed for O(1) checks after construction: each path's
/// position and frame floors, each binder's scope and frame depth, and each
/// variant's owning sum. A pure function of the arenas.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Index {
    /// Preorder position of each path.
    pub(crate) path_pos: Vec<u32>,
    /// `[value floor, type floor]` at each path (spec §3.2).
    pub(crate) path_floor: Vec<[u32; 2]>,
    /// `[start, end)` preorder range in which each binder is visible.
    pub(crate) binder_scope: Vec<[u32; 2]>,
    /// The frame depth at which each binder was bound.
    pub(crate) binder_depth: Vec<u32>,
    /// The sum item owning each variant (index), or `UNSET`.
    pub(crate) variant_owner: Vec<u32>,
}

/// Validates `store` with `root` as the root item.
pub(crate) fn validate(store: &Store, root: ItemId) -> Result<Index, HirError> {
    scan::scan(store, root)?;
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
    if !matches!(
        store.item(root).map(|i| &i.kind),
        Some(ItemKind::Module { .. })
    ) {
        return Err(HirError::RootNotModule);
    }
    let mut walker = Walker::new(store);
    walker.walk(root)?;
    walker.finish()
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
    let [value_floor, type_floor] = index
        .path_floor
        .get(path.index())
        .copied()
        .unwrap_or([UNSET, UNSET]);
    let value = store.binder(binder).is_some_and(|b| b.kind.is_value());
    let floor = if value { value_floor } else { type_floor };
    if depth < floor {
        return Err(HirError::NotCapturable { path, binder });
    }
    Ok(())
}

/// Returns `true` if `res` is something a path in `ns` may name (ids are
/// assumed in range; spec §3.3).
pub(crate) fn res_allowed(store: &Store, ns: Ns, res: Res) -> bool {
    match res {
        Res::Unresolved | Res::Err => true,
        Res::Local(b) => {
            let Some(binder) = store.binder(b) else {
                return false;
            };
            match ns {
                Ns::Value => matches!(
                    binder.kind,
                    BinderKind::Local
                        | BinderKind::Param
                        | BinderKind::Capture
                        | BinderKind::ConstParam
                ),
                Ns::Type => binder.kind == BinderKind::TypeParam,
                Ns::Region => binder.kind == BinderKind::Region,
                Ns::Pattern | Ns::Import => false,
            }
        }
        Res::Item(i) => {
            let Some(item) = store.item(i) else {
                return false;
            };
            let k = &item.kind;
            match ns {
                Ns::Value => matches!(
                    k,
                    ItemKind::Fn(_)
                        | ItemKind::Const { .. }
                        | ItemKind::Global { .. }
                        | ItemKind::Record(_)
                        | ItemKind::Class(_)
                        | ItemKind::Err
                ),
                Ns::Type => matches!(
                    k,
                    ItemKind::Record(_)
                        | ItemKind::Sum(_)
                        | ItemKind::Class(_)
                        | ItemKind::Interface(_)
                        | ItemKind::Alias { .. }
                        | ItemKind::AssocType { .. }
                        | ItemKind::Err
                ),
                Ns::Pattern => matches!(
                    k,
                    ItemKind::Const { .. }
                        | ItemKind::Record(_)
                        | ItemKind::Class(_)
                        | ItemKind::Err
                ),
                Ns::Region => false,
                Ns::Import => !matches!(k, ItemKind::Impl(_) | ItemKind::Import { .. }),
            }
        }
        Res::Variant(v) => {
            store.variant(v).is_some()
                && matches!(ns, Ns::Value | Ns::Type | Ns::Pattern | Ns::Import)
        }
        Res::Prim(_) => ns == Ns::Type,
    }
}

/// How a frame treats references to binders outside it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FrameKind {
    /// An item: opaque to every outer binder.
    Item,
    /// A member of an impl, interface, or class: opaque to values, transparent
    /// to the container's type-level binders.
    Member,
    /// A closure; transparent to values only with implicit captures.
    Closure { implicit: bool },
    /// A constant context: opaque to values, transparent to type-level binders.
    Const,
}

struct Frame {
    effects: Effects,
    /// An integer constant context: `promote` is rejected directly inside.
    integer_const: bool,
    /// `return` is allowed (a function with a body, or a closure).
    fn_like: bool,
    /// Loop-stack length when the frame began: older loops are out of reach.
    loop_base: u32,
    /// Open `try` bodies in this frame.
    tries: u32,
    /// Open `defer`/`finally` bodies in this frame.
    defers: u32,
}

struct LoopEntry {
    label: Option<BinderId>,
    is_loop: bool,
    /// Index + 1 of the nearest real loop at or below this entry; 0 if none.
    nearest_loop: u32,
}

/// Where an item sits, for placement rules.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Container {
    Root,
    Module,
    Block,
    Interface,
    Impl,
    Class,
    Other,
}

struct Walker<'a> {
    s: &'a Store,
    seen: [Vec<bool>; 9],
    pos: u32,
    ancestors: Vec<NodeRef>,
    frames: Vec<Frame>,
    /// Value floor and type floor for each frame-stack length (index = len - 1).
    floors: Vec<[u32; 2]>,
    /// Each open scope's start in `open`.
    scopes: Vec<u32>,
    /// Binders visible right now, in activation order.
    open: Vec<BinderId>,
    /// Completed root patterns waiting for their construct's `Bind` mark.
    groups: Vec<u32>,
    pending: Vec<BinderId>,
    /// Per open pattern: its start in `scratch` and in `or_bounds`.
    pat_frames: Vec<(u32, u32)>,
    scratch: Vec<BinderId>,
    or_bounds: Vec<u32>,
    sort_a: Vec<BinderId>,
    sort_b: Vec<BinderId>,
    loops: Vec<LoopEntry>,
    /// Loop-stack lengths below which a jump may not reach (frames and defers).
    jump_floors: Vec<u32>,
    label_index: Vec<u32>,
    bound: Vec<bool>,
    index: Index,
    stack: Vec<Step>,
    buf: Vec<Step>,
}

fn kind_slot(node: NodeRef) -> usize {
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

fn len_u32(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(UNSET)
}

impl<'a> Walker<'a> {
    fn new(s: &'a Store) -> Self {
        let binders = s.binders.len();
        Self {
            s,
            seen: [
                alloc::vec![false; s.items.len()],
                alloc::vec![false; s.exprs.len()],
                alloc::vec![false; s.stmts.len()],
                alloc::vec![false; s.pats.len()],
                alloc::vec![false; s.tys.len()],
                alloc::vec![false; s.paths.len()],
                alloc::vec![false; s.fields.len()],
                alloc::vec![false; s.variants.len()],
                alloc::vec![false; s.params.len()],
            ],
            pos: 0,
            ancestors: Vec::new(),
            frames: Vec::new(),
            floors: Vec::new(),
            scopes: Vec::new(),
            open: Vec::new(),
            groups: Vec::new(),
            pending: Vec::new(),
            pat_frames: Vec::new(),
            scratch: Vec::new(),
            or_bounds: Vec::new(),
            sort_a: Vec::new(),
            sort_b: Vec::new(),
            loops: Vec::new(),
            jump_floors: Vec::new(),
            label_index: alloc::vec![UNSET; binders],
            bound: alloc::vec![false; binders],
            index: Index {
                path_pos: alloc::vec![UNSET; s.paths.len()],
                path_floor: alloc::vec![[UNSET, UNSET]; s.paths.len()],
                binder_scope: alloc::vec![[UNSET, UNSET]; binders],
                binder_depth: alloc::vec![0; binders],
                variant_owner: alloc::vec![UNSET; s.variants.len()],
            },
            stack: Vec::new(),
            buf: Vec::new(),
        }
    }

    fn walk(&mut self, root: ItemId) -> Result<(), HirError> {
        self.stack.push(Step::Enter(NodeRef::Item(root)));
        while let Some(step) = self.stack.pop() {
            match step {
                Step::Enter(node) => self.enter(node)?,
                Step::Leave(node) => self.leave(node)?,
                Step::Mark(mark) => self.mark(mark)?,
            }
        }
        Ok(())
    }

    fn finish(self) -> Result<Index, HirError> {
        // Unreachable nodes, in arena order.
        for (slot, seen) in self.seen.iter().enumerate() {
            if let Some(i) = seen.iter().position(|s| !s) {
                let i = u32::try_from(i).unwrap_or(UNSET);
                let node = match slot {
                    0 => NodeRef::Item(crate::id::ItemId::from_raw_index(i)),
                    1 => NodeRef::Expr(crate::id::ExprId::from_raw_index(i)),
                    2 => NodeRef::Stmt(crate::id::StmtId::from_raw_index(i)),
                    3 => NodeRef::Pat(crate::id::PatId::from_raw_index(i)),
                    4 => NodeRef::Ty(crate::id::TyId::from_raw_index(i)),
                    5 => NodeRef::Path(crate::id::PathId::from_raw_index(i)),
                    6 => NodeRef::Field(crate::id::FieldId::from_raw_index(i)),
                    7 => NodeRef::Variant(crate::id::VariantId::from_raw_index(i)),
                    _ => NodeRef::Param(crate::id::ParamId::from_raw_index(i)),
                };
                return Err(HirError::Unreachable { node });
            }
        }
        if let Some(i) = self.bound.iter().position(|b| !b) {
            return Err(HirError::BinderNotBound {
                binder: BinderId::from_raw_index(len_u32(i)),
            });
        }
        for (i, path) in self.s.paths.nodes.iter().enumerate() {
            if let Res::Local(binder) = path.res {
                let path = PathId::from_raw_index(len_u32(i));
                check_local(self.s, &self.index, path, binder)?;
            }
        }
        Ok(self.index)
    }

    // ------------------------------------------------------------ steps

    fn enter(&mut self, node: NodeRef) -> Result<(), HirError> {
        let slot = kind_slot(node);
        match self.seen[slot].get_mut(node.index()) {
            Some(seen) if !*seen => *seen = true,
            _ => return Err(HirError::SharedNode { node }),
        }
        let pos = self.pos;
        self.pos = self.pos.saturating_add(1);
        let parent = self.ancestors.last().copied();
        self.ancestors.push(node);
        match node {
            NodeRef::Item(id) => self.enter_item(id, parent)?,
            NodeRef::Expr(id) => self.enter_expr(id)?,
            NodeRef::Pat(_) => self
                .pat_frames
                .push((len_u32(self.scratch.len()), len_u32(self.or_bounds.len()))),
            NodeRef::Path(id) => self.enter_path(id, parent, pos)?,
            _ => {}
        }
        self.stack.push(Step::Leave(node));
        self.buf.clear();
        expand(self.s, node, &mut self.buf);
        self.stack.extend(self.buf.drain(..).rev());
        Ok(())
    }

    fn leave(&mut self, node: NodeRef) -> Result<(), HirError> {
        let _ = self.ancestors.pop();
        if let NodeRef::Pat(id) = node {
            self.leave_pat(id)?;
        }
        Ok(())
    }

    fn mark(&mut self, mark: Mark) -> Result<(), HirError> {
        match mark {
            Mark::Frame(of) => self.push_frame(of),
            Mark::PopFrame => {
                let _ = self.frames.pop();
                let _ = self.floors.pop();
                let _ = self.jump_floors.pop();
            }
            Mark::Scope => self.scopes.push(len_u32(self.open.len())),
            Mark::PopScope => {
                let start = self.scopes.pop().unwrap_or(0) as usize;
                let end = self.pos;
                for b in self.open.drain(start.min(self.open.len())..) {
                    if let Some(scope) = self.index.binder_scope.get_mut(b.index()) {
                        scope[1] = end;
                    }
                }
            }
            Mark::Generics(list) => {
                let owner = self.ancestors.last().copied();
                for gp in self.s.list(list) {
                    let kind = self.binder_kind(gp.binder);
                    let ok = if gp.ty.is_some() {
                        kind == Some(BinderKind::ConstParam)
                    } else {
                        matches!(kind, Some(BinderKind::TypeParam | BinderKind::Region))
                    };
                    if !ok {
                        return Err(HirError::BinderKind {
                            binder: gp.binder,
                            expected: if gp.ty.is_some() {
                                BinderKind::ConstParam
                            } else {
                                BinderKind::TypeParam
                            },
                        });
                    }
                    self.bind(gp.binder, owner)?;
                }
            }
            Mark::Bind(site) => {
                let expected = if site == BindSite::Param {
                    BinderKind::Param
                } else {
                    BinderKind::Local
                };
                let start = self.groups.pop().unwrap_or(len_u32(self.pending.len())) as usize;
                let start = start.min(self.pending.len());
                let owner = self.ancestors.last().copied();
                let group: Vec<BinderId> = self.pending.drain(start..).collect();
                for b in group {
                    self.expect_kind(b, expected)?;
                    self.bind(b, owner)?;
                }
            }
            Mark::Captures(list) => {
                let owner = self.ancestors.last().copied();
                for cap in self.s.list(list) {
                    self.expect_kind(cap.binder, BinderKind::Capture)?;
                    self.bind(cap.binder, owner)?;
                }
            }
            Mark::Label(b) => {
                self.expect_kind(b, BinderKind::Label)?;
                let owner = self.ancestors.last().copied();
                self.bind(b, owner)?;
            }
            Mark::Loop { label, is_loop } => {
                let at = len_u32(self.loops.len());
                let nearest_loop = if is_loop {
                    at.saturating_add(1)
                } else {
                    self.loops.last().map_or(0, |l| l.nearest_loop)
                };
                self.loops.push(LoopEntry {
                    label,
                    is_loop,
                    nearest_loop,
                });
                if let Some(slot) = label.and_then(|l| self.label_index.get_mut(l.index())) {
                    *slot = at;
                }
            }
            Mark::PopLoop => {
                if let Some(entry) = self.loops.pop() {
                    if let Some(slot) = entry
                        .label
                        .and_then(|l| self.label_index.get_mut(l.index()))
                    {
                        *slot = UNSET;
                    }
                }
            }
            Mark::Try => {
                if let Some(f) = self.frames.last_mut() {
                    f.tries += 1;
                }
            }
            Mark::PopTry => {
                if let Some(f) = self.frames.last_mut() {
                    f.tries = f.tries.saturating_sub(1);
                }
            }
            Mark::Defer => {
                if let Some(f) = self.frames.last_mut() {
                    f.defers += 1;
                }
                self.jump_floors.push(len_u32(self.loops.len()));
            }
            Mark::PopDefer => {
                if let Some(f) = self.frames.last_mut() {
                    f.defers = f.defers.saturating_sub(1);
                }
                let _ = self.jump_floors.pop();
            }
            Mark::Open(_) | Mark::Close => {}
        }
        Ok(())
    }

    // ---------------------------------------------------------- binders

    fn binder_kind(&self, b: BinderId) -> Option<BinderKind> {
        self.s.binder(b).map(|b| b.kind)
    }

    fn expect_kind(&self, binder: BinderId, expected: BinderKind) -> Result<(), HirError> {
        if self.binder_kind(binder) == Some(expected) {
            Ok(())
        } else {
            Err(HirError::BinderKind { binder, expected })
        }
    }

    /// Records `b`'s binding site and makes it visible from the current position.
    fn bind(&mut self, b: BinderId, owner: Option<NodeRef>) -> Result<(), HirError> {
        match self.bound.get_mut(b.index()) {
            Some(bound) if !*bound => *bound = true,
            _ => {
                return Err(HirError::BinderBoundTwice {
                    binder: b,
                    node: owner.unwrap_or(NodeRef::Item(ItemId::from_raw_index(0))),
                });
            }
        }
        if let Some(scope) = self.index.binder_scope.get_mut(b.index()) {
            *scope = [self.pos, UNSET];
        }
        if let Some(depth) = self.index.binder_depth.get_mut(b.index()) {
            *depth = len_u32(self.frames.len());
        }
        self.open.push(b);
        Ok(())
    }

    // ----------------------------------------------------------- frames

    fn container(&self, parent: Option<NodeRef>) -> Container {
        match parent {
            None => Container::Root,
            Some(NodeRef::Stmt(_)) => Container::Block,
            Some(NodeRef::Item(p)) => match self.s.item(p).map(|i| &i.kind) {
                Some(ItemKind::Module { .. }) => Container::Module,
                Some(ItemKind::Interface(_)) => Container::Interface,
                Some(ItemKind::Impl(_)) => Container::Impl,
                Some(ItemKind::Class(_)) => Container::Class,
                _ => Container::Other,
            },
            Some(_) => Container::Other,
        }
    }

    fn push_frame(&mut self, of: FrameOf) {
        let (kind, effects, fn_like) = match of {
            FrameOf::Item(id) => {
                // The item is the innermost ancestor; its parent decides membership.
                let parent = self
                    .ancestors
                    .len()
                    .checked_sub(2)
                    .and_then(|i| self.ancestors.get(i))
                    .copied();
                let member = matches!(
                    self.container(parent),
                    Container::Interface | Container::Impl | Container::Class
                );
                let kind = if member {
                    FrameKind::Member
                } else {
                    FrameKind::Item
                };
                match self.s.item(id).map(|i| &i.kind) {
                    Some(ItemKind::Fn(f)) => (kind, f.effects, f.body.is_some()),
                    _ => (kind, Effects::NONE, false),
                }
            }
            FrameOf::Closure(id) => match self.s.expr(id) {
                Some(Expr::Closure(c)) => (
                    FrameKind::Closure {
                        implicit: c.implicit.is_some(),
                    },
                    c.effects,
                    true,
                ),
                _ => (FrameKind::Closure { implicit: false }, Effects::NONE, true),
            },
            FrameOf::Const { .. } => (FrameKind::Const, Effects::NONE, false),
        };
        let integer_const = matches!(of, FrameOf::Const { integer: true });
        let n = len_u32(self.frames.len());
        let [prev_value, prev_type] = self.floors.last().copied().unwrap_or([0, 0]);
        let value_floor = if kind == (FrameKind::Closure { implicit: true }) {
            prev_value
        } else {
            n.saturating_add(1)
        };
        let type_floor = match kind {
            FrameKind::Closure { .. } | FrameKind::Const | FrameKind::Member => prev_type,
            FrameKind::Item => n.saturating_add(1),
        };
        self.floors.push([value_floor, type_floor]);
        self.frames.push(Frame {
            effects,
            integer_const,
            fn_like,
            loop_base: len_u32(self.loops.len()),
            tries: 0,
            defers: 0,
        });
        self.jump_floors.push(len_u32(self.loops.len()));
    }

    // ------------------------------------------------------------ items

    fn enter_item(&mut self, id: ItemId, parent: Option<NodeRef>) -> Result<(), HirError> {
        let Some(item) = self.s.item(id) else {
            return Ok(());
        };
        let site = Site::Node(NodeRef::Item(id));
        let container = self.container(parent);
        let kind = &item.kind;
        let placed = match container {
            Container::Root => matches!(kind, ItemKind::Module { .. }),
            Container::Module | Container::Block => !matches!(kind, ItemKind::AssocType { .. }),
            Container::Interface => matches!(
                kind,
                ItemKind::Fn(_)
                    | ItemKind::Const { .. }
                    | ItemKind::AssocType { .. }
                    | ItemKind::Err
            ),
            Container::Impl => matches!(
                kind,
                ItemKind::Fn(_) | ItemKind::Const { .. } | ItemKind::Alias { .. } | ItemKind::Err
            ),
            Container::Class => matches!(
                kind,
                ItemKind::Fn(_)
                    | ItemKind::Const { .. }
                    | ItemKind::Global { .. }
                    | ItemKind::Alias { .. }
                    | ItemKind::Err
            ),
            Container::Other => false,
        };
        if !placed {
            return Err(HirError::Malformed {
                site,
                problem: Malformed::ItemPlacement,
            });
        }
        let member = matches!(
            container,
            Container::Interface | Container::Impl | Container::Class
        );
        match kind {
            ItemKind::Fn(f) => {
                if f.body.is_none()
                    && f.abi.is_none()
                    && !matches!(container, Container::Interface | Container::Class)
                {
                    return Err(HirError::Malformed {
                        site,
                        problem: Malformed::MissingBody,
                    });
                }
                if !member {
                    for p in self.s.list(f.params) {
                        if self
                            .s
                            .param(*p)
                            .is_some_and(|p| p.kind == ParamKind::Receiver)
                        {
                            return Err(HirError::Malformed {
                                site: Site::Node(NodeRef::Param(*p)),
                                problem: Malformed::ReceiverPlacement,
                            });
                        }
                    }
                }
            }
            ItemKind::Const { value: None, .. } if container != Container::Interface => {
                return Err(HirError::Malformed {
                    site,
                    problem: Malformed::MissingConstValue,
                });
            }
            ItemKind::Sum(d) => {
                let owner = len_u32(id.index());
                for v in self.s.list(d.variants) {
                    if let Some(slot) = self.index.variant_owner.get_mut(v.index()) {
                        *slot = owner;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    // ------------------------------------------------------ expressions

    fn enter_expr(&mut self, id: ExprId) -> Result<(), HirError> {
        let Some(expr) = self.s.expr(id) else {
            return Ok(());
        };
        let effect = |problem| HirError::Effect { expr: id, problem };
        match *expr {
            Expr::Break { label, .. } => self.jump(id, label, false),
            Expr::Continue { label } => self.jump(id, label, true),
            Expr::Return(_) => match self.frames.last() {
                Some(f) if f.fn_like && f.defers == 0 => Ok(()),
                Some(f) if f.fn_like => Err(HirError::Jump {
                    expr: id,
                    problem: JumpProblem::OutOfDefer,
                }),
                _ => Err(effect(EffectProblem::ReturnOutsideFunction)),
            },
            Expr::Await(_) => {
                self.require(Effects::ASYNC, effect(EffectProblem::AwaitOutsideAsync))
            }
            Expr::Yield(_) => {
                self.require(Effects::YIELD, effect(EffectProblem::YieldOutsideGenerator))
            }
            Expr::Throw(_) => match self.frames.last() {
                Some(f) if f.effects.contains(Effects::THROWS) || f.tries > 0 => Ok(()),
                _ => Err(effect(EffectProblem::ThrowNotAllowed)),
            },
            Expr::Op { op, .. } | Expr::Assign { op: Some(op), .. }
                if op.promotes() && self.frames.last().is_some_and(|f| f.integer_const) =>
            {
                Err(HirError::Malformed {
                    site: Site::Node(NodeRef::Expr(id)),
                    problem: Malformed::PromoteOnStaticResult,
                })
            }
            _ => Ok(()),
        }
    }

    fn require(&self, effect: Effects, err: HirError) -> Result<(), HirError> {
        match self.frames.last() {
            Some(f) if f.effects.contains(effect) => Ok(()),
            _ => Err(err),
        }
    }

    fn jump(
        &self,
        expr: ExprId,
        label: Option<BinderId>,
        is_continue: bool,
    ) -> Result<(), HirError> {
        let err = |problem| Err(HirError::Jump { expr, problem });
        let base = self.frames.last().map_or(0, |f| f.loop_base);
        let floor = self.jump_floors.last().copied().unwrap_or(0);
        match label {
            Some(b) => {
                let at = self.label_index.get(b.index()).copied().unwrap_or(UNSET);
                if at == UNSET || at < base {
                    return err(JumpProblem::LabelNotInScope);
                }
                if at < floor {
                    return err(JumpProblem::OutOfDefer);
                }
                let target_is_loop = self.loops.get(at as usize).is_some_and(|l| l.is_loop);
                if is_continue && !target_is_loop {
                    return err(JumpProblem::ContinueToBlock);
                }
                Ok(())
            }
            None => {
                let nearest = self.loops.last().map_or(0, |l| l.nearest_loop);
                let outside = if is_continue {
                    JumpProblem::ContinueOutsideLoop
                } else {
                    JumpProblem::BreakOutsideLoop
                };
                if nearest == 0 || nearest - 1 < base {
                    return err(outside);
                }
                if nearest - 1 < floor {
                    return err(JumpProblem::OutOfDefer);
                }
                Ok(())
            }
        }
    }

    // ------------------------------------------------------------ paths

    fn enter_path(
        &mut self,
        id: PathId,
        parent: Option<NodeRef>,
        pos: u32,
    ) -> Result<(), HirError> {
        let Some(path) = self.s.path(id) else {
            return Ok(());
        };
        let expected = match parent {
            Some(NodeRef::Expr(e)) => match self.s.expr(e) {
                Some(Expr::Record { .. }) => Ns::Type,
                _ => Ns::Value,
            },
            Some(NodeRef::Ty(t)) => match self.s.ty(t) {
                Some(Ty::Ref { .. }) => Ns::Region,
                _ => Ns::Type,
            },
            Some(NodeRef::Pat(p)) => match self.s.pat(p) {
                Some(Pat::Record { .. }) => Ns::Type,
                _ => Ns::Pattern,
            },
            _ => Ns::Import,
        };
        if path.ns != expected {
            return Err(HirError::PathNamespace { path: id, expected });
        }
        if !res_allowed(self.s, path.ns, path.res) {
            return Err(HirError::Resolution {
                path: id,
                res: path.res,
            });
        }
        if let Some(slot) = self.index.path_pos.get_mut(id.index()) {
            *slot = pos;
        }
        let floors = self.floors.last().copied().unwrap_or([0, 0]);
        if let Some(slot) = self.index.path_floor.get_mut(id.index()) {
            *slot = floors;
        }
        Ok(())
    }

    // --------------------------------------------------------- patterns

    fn leave_pat(&mut self, id: PatId) -> Result<(), HirError> {
        let (start, bounds_start) = self.pat_frames.pop().unwrap_or((0, 0));
        let (start, bounds_start) = (start as usize, bounds_start as usize);
        match self.s.pat(id) {
            Some(Pat::Bind { binder, .. }) => self.scratch.push(*binder),
            Some(Pat::Or(alts)) => self.close_or(id, *alts, bounds_start)?,
            _ => {}
        }
        match self.ancestors.last().copied() {
            Some(NodeRef::Pat(parent)) => {
                if matches!(self.s.pat(parent), Some(Pat::Or(_))) {
                    self.or_bounds.push(len_u32(start));
                }
            }
            _ => {
                // The root of a binding construct: its binders form one group.
                let start = start.min(self.scratch.len());
                self.sort_a.clear();
                self.sort_a.extend_from_slice(&self.scratch[start..]);
                if let Some(dup) = first_dup(&mut self.sort_a) {
                    return Err(HirError::DuplicateBinding {
                        binder: dup,
                        pat: id,
                    });
                }
                self.groups.push(len_u32(self.pending.len()));
                self.pending.extend(self.scratch.drain(start..));
            }
        }
        Ok(())
    }

    /// Checks that every alternative binds the same set, then keeps one copy.
    fn close_or(
        &mut self,
        id: PatId,
        alts: crate::id::List<PatId>,
        bounds_start: usize,
    ) -> Result<(), HirError> {
        let bounds_start = bounds_start.min(self.or_bounds.len());
        let bounds: Vec<usize> = self.or_bounds[bounds_start..]
            .iter()
            .map(|b| *b as usize)
            .collect();
        self.or_bounds.truncate(bounds_start);
        let end = self.scratch.len();
        let Some(&first_start) = bounds.first() else {
            return Ok(());
        };
        let alt_ids = self.s.list(alts);
        let range = |i: usize| {
            let lo = bounds.get(i).copied().unwrap_or(end).min(end);
            let hi = bounds.get(i + 1).copied().unwrap_or(end).min(end);
            lo..hi.max(lo)
        };
        let first = range(0);
        self.sort_a.clear();
        self.sort_a.extend_from_slice(&self.scratch[first.clone()]);
        if let Some(dup) = first_dup(&mut self.sort_a) {
            return Err(HirError::DuplicateBinding {
                binder: dup,
                pat: alt_ids.first().copied().unwrap_or(id),
            });
        }
        for i in 1..bounds.len() {
            self.sort_b.clear();
            self.sort_b.extend_from_slice(&self.scratch[range(i)]);
            if let Some(dup) = first_dup(&mut self.sort_b) {
                return Err(HirError::DuplicateBinding {
                    binder: dup,
                    pat: alt_ids.get(i).copied().unwrap_or(id),
                });
            }
            if self.sort_a != self.sort_b {
                return Err(HirError::OrPatternBinders { pat: id });
            }
        }
        self.scratch.truncate(first_start.min(end) + first.len());
        Ok(())
    }
}

/// Sorts `v` and returns the first repeated element, if any.
fn first_dup(v: &mut [BinderId]) -> Option<BinderId> {
    v.sort_unstable();
    v.windows(2).find_map(|w| match w {
        [a, b] if a == b => Some(*a),
        _ => None,
    })
}
