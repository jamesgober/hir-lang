//! Pass 2 (the tree walk) and pass 3 (reachability and scope checks).

use alloc::{collections::BTreeSet, vec::Vec};

use super::{Ctx, Index, Repair, Sink, UNSET, build_locals, check_local, check_res};
use crate::{
    error::{EffectProblem, HirError, JumpProblem, Malformed, Site},
    expr::{DefaultEval, Expr},
    id::{BinderId, ExprId, ItemId, NodeRef, PatId, PathId},
    item::{ItemKind, ParamKind},
    name::{BinderKind, Ns, Res},
    pat::{BindMode, Pat},
    store::Store,
    ty::Effects,
    walk::{BindSite, FrameOf, Mark, Step, expand},
};

/// What earlier repairs made dead or unbound, so their consequences are fixed
/// silently instead of reported again.
#[derive(Default)]
pub(crate) struct Cascades {
    /// Nodes that became unreachable because an ancestor was repaired.
    pub(crate) dead: BTreeSet<NodeRef>,
    /// Binders whose binding site was removed by a repair.
    pub(crate) lost: BTreeSet<BinderId>,
    /// Report nothing (consequences of an earlier round's repairs).
    pub(crate) quiet: bool,
}

type R = Result<(), HirError>;

/// Walks the tree from `root`, then checks reachability and scopes. Returns
/// the index (meaningful when no problem was found).
pub(crate) fn walk(
    store: &Store,
    root: ItemId,
    ctx: Ctx,
    sink: &mut Sink,
    cascades: &Cascades,
) -> Result<Index, HirError> {
    let mut walker = Walker::new(store, ctx, sink, cascades);
    walker.walk(root)?;
    walker.finish()
}

/// How a frame treats references to binders outside it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FrameKind {
    Item,
    Member,
    Closure { implicit: bool },
    Const,
    Default(DefaultEval),
}

struct FrameInfo {
    effects: Effects,
    fn_like: bool,
    loop_base: u32,
    tries: u32,
    /// Open `defer` bodies: no jump may leave, no `return`.
    defers: u32,
    /// Open `defer` or `finally` bodies: no `yield`.
    cleanups: u32,
    integer_const: bool,
}

struct LoopEntry {
    label: Option<BinderId>,
    is_loop: bool,
    /// Index + 1 of the nearest real loop at or below this entry; 0 if none.
    nearest_loop: u32,
    /// The loop's `step` is being walked.
    in_step: bool,
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

/// One binder collected from a pattern: the binder, its mode, its pattern.
#[derive(Clone, Copy)]
struct Bound {
    binder: BinderId,
    mode: BindMode,
    pat: PatId,
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

struct Walker<'a, 's> {
    s: &'a Store,
    ctx: Ctx,
    sink: &'s mut Sink,
    cascades: &'a Cascades,
    seen: [Vec<bool>; 9],
    pos: u32,
    ancestors: Vec<NodeRef>,
    frames: Vec<FrameInfo>,
    /// `[value floor, type floor, value ceiling]` for each frame-stack length.
    floors: Vec<[u32; 3]>,
    scopes: Vec<u32>,
    open: Vec<BinderId>,
    groups: Vec<u32>,
    pending: Vec<Bound>,
    pat_frames: Vec<(u32, u32)>,
    scratch: Vec<Bound>,
    or_bounds: Vec<u32>,
    /// Generation stamps per binder for duplicate and or-pattern checks.
    stamp_a: Vec<u32>,
    stamp_b: Vec<u32>,
    mode_a: Vec<BindMode>,
    generation: u32,
    loops: Vec<LoopEntry>,
    jump_floors: Vec<u32>,
    label_index: Vec<u32>,
    bound: Vec<bool>,
    expect_ns: Option<Ns>,
    index: Index,
    stack: Vec<Step>,
    buf: Vec<Step>,
}

impl<'a, 's> Walker<'a, 's> {
    fn new(s: &'a Store, ctx: Ctx, sink: &'s mut Sink, cascades: &'a Cascades) -> Self {
        let binders = s.binders.len();
        Self {
            s,
            ctx,
            sink,
            cascades,
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
            stamp_a: alloc::vec![0; binders],
            stamp_b: alloc::vec![0; binders],
            mode_a: alloc::vec![BindMode::Value; binders],
            generation: 0,
            loops: Vec::new(),
            jump_floors: Vec::new(),
            label_index: alloc::vec![UNSET; binders],
            bound: alloc::vec![false; binders],
            expect_ns: None,
            index: Index {
                path_pos: alloc::vec![UNSET; s.paths.len()],
                path_floor: alloc::vec![[UNSET, UNSET, 0]; s.paths.len()],
                binder_scope: alloc::vec![[UNSET, UNSET]; binders],
                binder_depth: alloc::vec![0; binders],
                variant_owner: alloc::vec![UNSET; s.variants.len()],
                locals: Vec::new(),
            },
            stack: Vec::new(),
            buf: Vec::new(),
        }
    }

    /// Reports a problem found by the walk (never a cascade of a pass-1 repair
    /// unless the whole round is quiet).
    fn report(&mut self, err: HirError, repair: Repair) -> R {
        let show = !self.cascades.quiet;
        self.sink.report(err, repair, show)
    }

    fn walk(&mut self, root: ItemId) -> R {
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

    fn finish(mut self) -> Result<Index, HirError> {
        // Unreachable nodes, in arena order. Error-form nodes may be dead.
        for slot in 0..9 {
            let len = self.seen.get(slot).map_or(0, Vec::len);
            for i in 0..len {
                if self
                    .seen
                    .get(slot)
                    .and_then(|v| v.get(i))
                    .copied()
                    .unwrap_or(true)
                {
                    continue;
                }
                let i = len_u32(i);
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
                if super::repair::is_dead_ok(self.s, node) {
                    continue;
                }
                let show = !self.cascades.quiet && !self.cascades.dead.contains(&node);
                self.sink
                    .report(HirError::Unreachable { node }, Repair::ErrNode(node), show)?;
            }
        }
        for (i, path) in self.s.paths.nodes.iter().enumerate() {
            let Res::Local(binder) = path.res else {
                continue;
            };
            if !self
                .seen
                .get(5)
                .and_then(|v| v.get(i))
                .copied()
                .unwrap_or(false)
            {
                continue;
            }
            let id = PathId::from_raw_index(len_u32(i));
            if let Err(e) = check_local(self.s, &self.index, id, binder) {
                let show = !self.cascades.quiet && !self.cascades.lost.contains(&binder);
                self.sink
                    .report(e, Repair::ResErr { path: id, ns: None }, show)?;
            }
        }
        build_locals(self.s, &mut self.index);
        Ok(self.index)
    }

    // ------------------------------------------------------------ steps

    fn enter(&mut self, node: NodeRef) -> R {
        let slot = kind_slot(node);
        let parent = self.ancestors.last().copied();
        match self
            .seen
            .get_mut(slot)
            .and_then(|v| v.get_mut(node.index()))
        {
            Some(seen) if !*seen => *seen = true,
            _ => {
                // Shared (or dangling, after a repair): leave this subtree.
                let repair = Repair::ErrNode(parent.unwrap_or(node));
                return self.report(HirError::SharedNode { node }, repair);
            }
        }
        let pos = self.pos;
        self.pos = self.pos.saturating_add(1);
        self.ancestors.push(node);
        match node {
            NodeRef::Item(id) => self.enter_item(id, parent)?,
            NodeRef::Expr(id) => self.enter_expr(id, parent)?,
            NodeRef::Pat(_) => self
                .pat_frames
                .push((len_u32(self.scratch.len()), len_u32(self.or_bounds.len()))),
            NodeRef::Path(id) => self.enter_path(id, pos)?,
            _ => {}
        }
        self.stack.push(Step::Leave(node));
        self.buf.clear();
        expand(self.s, node, &mut self.buf);
        self.stack.extend(self.buf.drain(..).rev());
        Ok(())
    }

    fn leave(&mut self, node: NodeRef) -> R {
        let _ = self.ancestors.pop();
        if let NodeRef::Pat(id) = node {
            self.leave_pat(id)?;
        }
        Ok(())
    }

    fn mark(&mut self, mark: Mark) -> R {
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
                        let expected = if gp.ty.is_some() {
                            BinderKind::ConstParam
                        } else {
                            BinderKind::TypeParam
                        };
                        self.report(
                            HirError::BinderKind {
                                binder: gp.binder,
                                expected,
                            },
                            Repair::SetKind(gp.binder, expected),
                        )?;
                    }
                    self.bind(gp.binder, owner, owner.map(Repair::ErrNode))?;
                }
            }
            Mark::Bind(site, _) => {
                let expected = if site == BindSite::Param {
                    BinderKind::Param
                } else {
                    BinderKind::Local
                };
                let start = self.groups.pop().unwrap_or(len_u32(self.pending.len())) as usize;
                let start = start.min(self.pending.len());
                let owner = self.ancestors.last().copied();
                for i in start..self.pending.len() {
                    let Some(entry) = self.pending.get(i).copied() else {
                        continue;
                    };
                    self.expect_kind(entry.binder, expected)?;
                    self.bind(entry.binder, owner, Some(Repair::WildPat(entry.pat)))?;
                }
                self.pending.truncate(start);
            }
            Mark::BindDecl(b) => {
                let owner = self.ancestors.last().copied();
                self.expect_kind(b, BinderKind::Local)?;
                self.bind(b, owner, owner.map(Repair::ErrNode))?;
            }
            Mark::Captures(list) => {
                let owner = self.ancestors.last().copied();
                for cap in self.s.list(list) {
                    self.expect_kind(cap.binder, BinderKind::Capture)?;
                    self.bind(cap.binder, owner, owner.map(Repair::ErrNode))?;
                }
            }
            Mark::SelfBinder(b) => {
                let owner = self.ancestors.last().copied();
                self.expect_kind(b, BinderKind::Capture)?;
                self.bind(b, owner, owner.map(Repair::ErrNode))?;
            }
            Mark::Label(b) => {
                let owner = self.ancestors.last().copied();
                self.expect_kind(b, BinderKind::Label)?;
                self.bind(b, owner, owner.map(Repair::ErrNode))?;
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
                    in_step: false,
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
            Mark::StepBegin => {
                if let Some(l) = self.loops.last_mut() {
                    l.in_step = true;
                }
            }
            Mark::StepEnd => {
                if let Some(l) = self.loops.last_mut() {
                    l.in_step = false;
                }
            }
            Mark::Try => self.frame_mut(|f| f.tries += 1),
            Mark::PopTry => self.frame_mut(|f| f.tries = f.tries.saturating_sub(1)),
            Mark::Defer => {
                self.frame_mut(|f| {
                    f.defers += 1;
                    f.cleanups += 1;
                });
                self.jump_floors.push(len_u32(self.loops.len()));
            }
            Mark::PopDefer => {
                self.frame_mut(|f| {
                    f.defers = f.defers.saturating_sub(1);
                    f.cleanups = f.cleanups.saturating_sub(1);
                });
                let _ = self.jump_floors.pop();
            }
            Mark::Finally => self.frame_mut(|f| f.cleanups += 1),
            Mark::PopFinally => self.frame_mut(|f| f.cleanups = f.cleanups.saturating_sub(1)),
            Mark::ExpectNs(ns) => self.expect_ns = Some(ns),
            Mark::Open(_) | Mark::Close => {}
        }
        Ok(())
    }

    fn frame_mut(&mut self, f: impl FnOnce(&mut FrameInfo)) {
        if let Some(frame) = self.frames.last_mut() {
            f(frame);
        }
    }

    // ---------------------------------------------------------- binders

    fn binder_kind(&self, b: BinderId) -> Option<BinderKind> {
        self.s.binder(b).map(|b| b.kind)
    }

    fn expect_kind(&mut self, binder: BinderId, expected: BinderKind) -> R {
        if self.binder_kind(binder) == Some(expected) {
            Ok(())
        } else {
            self.report(
                HirError::BinderKind { binder, expected },
                Repair::SetKind(binder, expected),
            )
        }
    }

    /// Records `b`'s binding site and makes it visible from the current
    /// position. A second binding site is a problem; its repair removes it.
    fn bind(&mut self, b: BinderId, owner: Option<NodeRef>, twice: Option<Repair>) -> R {
        match self.bound.get_mut(b.index()) {
            Some(bound) if !*bound => *bound = true,
            _ => {
                let node = owner.unwrap_or(NodeRef::Item(ItemId::from_raw_index(0)));
                let repair = twice.unwrap_or(Repair::ErrNode(node));
                return self.report(HirError::BinderBoundTwice { binder: b, node }, repair);
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

    /// The node `depth` levels above the innermost ancestor.
    fn ancestor(&self, depth: usize) -> Option<NodeRef> {
        self.ancestors
            .len()
            .checked_sub(depth + 1)
            .and_then(|i| self.ancestors.get(i))
            .copied()
    }

    fn push_frame(&mut self, of: FrameOf) {
        let (kind, effects, fn_like) = match of {
            FrameOf::Item(id) => {
                let member = matches!(
                    self.container(self.ancestor(1)),
                    Container::Interface | Container::Impl | Container::Class
                );
                let kind = if member {
                    FrameKind::Member
                } else {
                    FrameKind::Item
                };
                match self.s.item(id).map(|i| &i.kind) {
                    Some(ItemKind::Fn(f)) => (kind, f.effects, f.body.is_some()),
                    Some(ItemKind::Module { body, effects, .. }) => {
                        (kind, *effects, body.is_some())
                    }
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
            FrameOf::Default(_) => {
                // The parameter is the innermost ancestor; its owner is above.
                let eval = match self.ancestor(1) {
                    Some(NodeRef::Item(i)) => match self.s.item(i).map(|i| &i.kind) {
                        Some(ItemKind::Fn(f)) => f.defaults,
                        _ => DefaultEval::PerCall,
                    },
                    Some(NodeRef::Expr(e)) => match self.s.expr(e) {
                        Some(Expr::Closure(c)) => c.defaults,
                        _ => DefaultEval::PerCall,
                    },
                    _ => DefaultEval::PerCall,
                };
                (FrameKind::Default(eval), Effects::NONE, false)
            }
            FrameOf::Const { .. } => (FrameKind::Const, Effects::NONE, false),
        };
        let integer_const = matches!(of, FrameOf::Const { integer: true });
        let n = len_u32(self.frames.len());
        let [prev_value, prev_type, prev_ceiling] =
            self.floors.last().copied().unwrap_or([0, 0, UNSET]);
        let value_floor = match kind {
            FrameKind::Closure { implicit: true } | FrameKind::Default(_) => prev_value,
            _ => n.saturating_add(1),
        };
        let type_floor = match kind {
            FrameKind::Closure { .. }
            | FrameKind::Const
            | FrameKind::Member
            | FrameKind::Default(_) => prev_type,
            FrameKind::Item => n.saturating_add(1),
        };
        let ceiling = match kind {
            // Evaluated once at definition: the owner's parameters (bound at
            // depth `n`) are not visible.
            FrameKind::Default(DefaultEval::Once) => n,
            FrameKind::Item | FrameKind::Member => UNSET,
            _ => prev_ceiling,
        };
        self.floors.push([value_floor, type_floor, ceiling]);
        self.frames.push(FrameInfo {
            effects,
            fn_like,
            loop_base: len_u32(self.loops.len()),
            tries: 0,
            defers: 0,
            cleanups: 0,
            integer_const,
        });
        self.jump_floors.push(len_u32(self.loops.len()));
    }

    // ------------------------------------------------------------ items

    fn enter_item(&mut self, id: ItemId, parent: Option<NodeRef>) -> R {
        let Some(item) = self.s.item(id) else {
            return Ok(());
        };
        let node = NodeRef::Item(id);
        let site = Site::Node(node);
        let container = self.container(parent);
        let kind = &item.kind;
        let placed = match container {
            Container::Root => matches!(kind, ItemKind::Module { .. }),
            Container::Module | Container::Block => {
                !matches!(kind, ItemKind::AssocType { .. } | ItemKind::MixinUse(_))
            }
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
                    | ItemKind::MixinUse(_)
                    | ItemKind::Err
            ),
            Container::Other => false,
        };
        let malformed = |problem| HirError::Malformed { site, problem };
        if !placed {
            self.report(malformed(Malformed::ItemPlacement), Repair::ErrNode(node))?;
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
                    self.report(malformed(Malformed::MissingBody), Repair::ErrNode(node))?;
                }
                if !member {
                    let receiver = self.s.list(f.params).iter().find(|p| {
                        self.s
                            .param(**p)
                            .is_some_and(|p| p.kind == ParamKind::Receiver)
                    });
                    if let Some(p) = receiver {
                        let err = HirError::Malformed {
                            site: Site::Node(NodeRef::Param(*p)),
                            problem: Malformed::ReceiverPlacement,
                        };
                        self.report(err, Repair::ErrNode(node))?;
                    }
                }
            }
            ItemKind::Const { value: None, .. } if container != Container::Interface => {
                self.report(
                    malformed(Malformed::MissingConstValue),
                    Repair::ErrNode(node),
                )?;
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

    fn enter_expr(&mut self, id: ExprId, parent: Option<NodeRef>) -> R {
        let Some(expr) = self.s.expr(id) else {
            return Ok(());
        };
        let problem = match *expr {
            Expr::Break { label, .. } => self.jump(label, false).err().map(|p| HirError::Jump {
                expr: id,
                problem: p,
            }),
            Expr::Continue { label } => self.jump(label, true).err().map(|p| HirError::Jump {
                expr: id,
                problem: p,
            }),
            Expr::Return(_) => match self.frames.last() {
                Some(f) if f.fn_like && f.defers == 0 => None,
                Some(f) if f.fn_like => Some(HirError::Jump {
                    expr: id,
                    problem: JumpProblem::OutOfDefer,
                }),
                _ => Some(effect(id, EffectProblem::ReturnOutsideFunction)),
            },
            Expr::Await(_) => match self.frames.last() {
                Some(f) if f.effects.contains(Effects::ASYNC) => None,
                _ => Some(effect(id, EffectProblem::AwaitOutsideAsync)),
            },
            Expr::Yield(_) => match self.frames.last() {
                Some(f) if !f.effects.contains(Effects::YIELD) => {
                    Some(effect(id, EffectProblem::YieldOutsideGenerator))
                }
                Some(f) if f.cleanups > 0 => Some(effect(id, EffectProblem::YieldInCleanup)),
                Some(_) => None,
                None => Some(effect(id, EffectProblem::YieldOutsideGenerator)),
            },
            Expr::Throw(_) => match self.frames.last() {
                Some(f) if f.effects.contains(Effects::THROWS) || f.tries > 0 => None,
                _ => Some(effect(id, EffectProblem::ThrowNotAllowed)),
            },
            Expr::Op { op, .. } | Expr::Assign { op: Some(op), .. }
                if op.promotes() && self.frames.last().is_some_and(|f| f.integer_const) =>
            {
                Some(malformed_expr(id, Malformed::PromoteOnStaticResult))
            }
            Expr::Append(_) if !self.append_ok(id, parent) => {
                Some(malformed_expr(id, Malformed::AppendContext))
            }
            _ => None,
        };
        match problem {
            Some(err) => self.report(err, Repair::ErrNode(NodeRef::Expr(id))),
            None => Ok(()),
        }
    }

    /// An append place is only a write target or a place argument.
    fn append_ok(&self, id: ExprId, parent: Option<NodeRef>) -> bool {
        let Some(NodeRef::Expr(p)) = parent else {
            return false;
        };
        match self.s.expr(p) {
            Some(Expr::Assign { target, .. } | Expr::RefAssign { target, .. }) => *target == id,
            Some(
                Expr::Call { args, .. }
                | Expr::MethodCall { args, .. }
                | Expr::DynMethodCall { args, .. },
            ) => self.s.list(*args).iter().any(|a| a.place && a.value == id),
            _ => false,
        }
    }

    fn jump(&self, label: Option<BinderId>, is_continue: bool) -> Result<(), JumpProblem> {
        let base = self.frames.last().map_or(0, |f| f.loop_base);
        let floor = self.jump_floors.last().copied().unwrap_or(0);
        let target = match label {
            Some(b) => {
                let at = self.label_index.get(b.index()).copied().unwrap_or(UNSET);
                if at == UNSET || at < base {
                    return Err(JumpProblem::LabelNotInScope);
                }
                if at < floor {
                    return Err(JumpProblem::OutOfDefer);
                }
                let entry = self.loops.get(at as usize);
                if is_continue && !entry.is_some_and(|l| l.is_loop) {
                    return Err(JumpProblem::ContinueToBlock);
                }
                entry
            }
            None => {
                let nearest = self.loops.last().map_or(0, |l| l.nearest_loop);
                if nearest == 0 || nearest - 1 < base {
                    return Err(if is_continue {
                        JumpProblem::ContinueOutsideLoop
                    } else {
                        JumpProblem::BreakOutsideLoop
                    });
                }
                if nearest - 1 < floor {
                    return Err(JumpProblem::OutOfDefer);
                }
                self.loops.get(nearest as usize - 1)
            }
        };
        if is_continue && target.is_some_and(|l| l.in_step) {
            return Err(JumpProblem::ContinueInStep);
        }
        Ok(())
    }

    // ------------------------------------------------------------ paths

    fn enter_path(&mut self, id: PathId, pos: u32) -> R {
        let Some(path) = self.s.path(id) else {
            return Ok(());
        };
        let expected = self.expect_ns.take().unwrap_or(path.ns);
        if let Some(slot) = self.index.path_pos.get_mut(id.index()) {
            *slot = pos;
        }
        let floors = self.floors.last().copied().unwrap_or([0, 0, UNSET]);
        if let Some(slot) = self.index.path_floor.get_mut(id.index()) {
            *slot = floors;
        }
        if path.ns != expected {
            return self.report(
                HirError::PathNamespace { path: id, expected },
                Repair::ResErr {
                    path: id,
                    ns: Some(expected),
                },
            );
        }
        if let Err(e) = check_res(self.s, self.ctx, id, path, path.res, path.unresolved) {
            return self.report(e, Repair::ResErr { path: id, ns: None });
        }
        Ok(())
    }

    // --------------------------------------------------------- patterns

    fn leave_pat(&mut self, id: PatId) -> R {
        let (start, bounds_start) = self.pat_frames.pop().unwrap_or((0, 0));
        let (start, bounds_start) = (start as usize, bounds_start as usize);
        match self.s.pat(id) {
            Some(Pat::Bind { binder, mode, .. }) => self.scratch.push(Bound {
                binder: *binder,
                mode: *mode,
                pat: id,
            }),
            Some(Pat::Ident { binder, .. }) => self.scratch.push(Bound {
                binder: *binder,
                mode: BindMode::Value,
                pat: id,
            }),
            Some(Pat::Or(_)) => self.close_or(id, bounds_start)?,
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
                let end = self.scratch.len();
                if let Some(dup) = self.first_dup(start, end) {
                    self.report(
                        HirError::DuplicateBinding {
                            binder: dup.binder,
                            pat: id,
                        },
                        Repair::WildPat(dup.pat),
                    )?;
                }
                self.groups.push(len_u32(self.pending.len()));
                self.pending.extend(self.scratch.drain(start..));
            }
        }
        Ok(())
    }

    fn next_generation(&mut self) -> u32 {
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            // Wrapped: clear the stamps so old generations cannot match.
            self.stamp_a.iter_mut().for_each(|s| *s = 0);
            self.stamp_b.iter_mut().for_each(|s| *s = 0);
            self.generation = 1;
        }
        self.generation
    }

    /// The first entry of `scratch[start..end]`, in order, whose binder an
    /// earlier entry in the range already binds. Linear.
    fn first_dup(&mut self, start: usize, end: usize) -> Option<Bound> {
        let g = self.next_generation();
        for i in start..end {
            let entry = self.scratch.get(i).copied()?;
            let slot = self.stamp_a.get_mut(entry.binder.index())?;
            if *slot == g {
                return Some(entry);
            }
            *slot = g;
        }
        None
    }

    /// Checks that every alternative binds the same set with the same modes,
    /// then keeps one copy (the first alternative's).
    fn close_or(&mut self, id: PatId, bounds_start: usize) -> R {
        let bounds_start = bounds_start.min(self.or_bounds.len());
        let bounds: Vec<usize> = self
            .or_bounds
            .get(bounds_start..)
            .unwrap_or(&[])
            .iter()
            .map(|b| *b as usize)
            .collect();
        self.or_bounds.truncate(bounds_start);
        let end = self.scratch.len();
        let Some(&first_start) = bounds.first() else {
            return Ok(());
        };
        let range = |i: usize| {
            let lo = bounds.get(i).copied().unwrap_or(end).min(end);
            let hi = bounds.get(i + 1).copied().unwrap_or(end).min(end);
            lo..hi.max(lo)
        };
        let first = range(0);
        if let Some(dup) = self.first_dup(first.start, first.end) {
            self.report(
                HirError::DuplicateBinding {
                    binder: dup.binder,
                    pat: id,
                },
                Repair::WildPat(dup.pat),
            )?;
        }
        // Stamp the first alternative's set (generation `g0`) and modes.
        let g0 = self.next_generation();
        for i in first.clone() {
            if let Some(e) = self.scratch.get(i).copied() {
                if let Some(s) = self.stamp_a.get_mut(e.binder.index()) {
                    *s = g0;
                }
                if let Some(m) = self.mode_a.get_mut(e.binder.index()) {
                    *m = e.mode;
                }
            }
        }
        let first_count = first.len();
        let mut problem = None;
        for alt in 1..bounds.len() {
            let gi = self.next_generation();
            let mut count = 0usize;
            for i in range(alt) {
                let Some(e) = self.scratch.get(i).copied() else {
                    continue;
                };
                let b = e.binder.index();
                if self.stamp_b.get(b) == Some(&gi) {
                    first_problem(
                        &mut problem,
                        (
                            HirError::DuplicateBinding {
                                binder: e.binder,
                                pat: id,
                            },
                            Repair::WildPat(e.pat),
                        ),
                    );
                    continue;
                }
                if let Some(s) = self.stamp_b.get_mut(b) {
                    *s = gi;
                }
                count += 1;
                if self.stamp_a.get(b) != Some(&g0) {
                    first_problem(
                        &mut problem,
                        (
                            HirError::OrPatternBinders { pat: id },
                            Repair::ErrNode(NodeRef::Pat(id)),
                        ),
                    );
                } else if self.mode_a.get(b) != Some(&e.mode) {
                    first_problem(
                        &mut problem,
                        (
                            HirError::Malformed {
                                site: Site::Node(NodeRef::Pat(id)),
                                problem: Malformed::OrPatternModes,
                            },
                            Repair::ErrNode(NodeRef::Pat(id)),
                        ),
                    );
                }
            }
            if count != first_count {
                first_problem(
                    &mut problem,
                    (
                        HirError::OrPatternBinders { pat: id },
                        Repair::ErrNode(NodeRef::Pat(id)),
                    ),
                );
            }
        }
        // `next_generation` may have cleared stamps on wrap; generations are
        // only compared within this call, after the clear.
        self.scratch.truncate(first_start.min(end) + first_count);
        if let Some((err, repair)) = problem {
            self.report(err, repair)?;
        }
        Ok(())
    }
}

fn effect(expr: ExprId, problem: EffectProblem) -> HirError {
    HirError::Effect { expr, problem }
}

fn malformed_expr(expr: ExprId, problem: Malformed) -> HirError {
    HirError::Malformed {
        site: Site::Node(NodeRef::Expr(expr)),
        problem,
    }
}

/// Keeps the first problem found.
fn first_problem(slot: &mut Option<(HirError, Repair)>, problem: (HirError, Repair)) {
    if slot.is_none() {
        *slot = Some(problem);
    }
}
