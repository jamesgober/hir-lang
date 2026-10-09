//! Pass 1: every node on its own, in arena order.
//!
//! Checks every id, list, and text range against its arena or pool, every
//! origin and mark against the expansion table, and every rule that needs only
//! the node itself (literals, op arity and policy, shapes, duplicate member
//! names, parameter order). A node gets at most one problem (the first found);
//! its repair is to become an error node. After this pass (and its repairs)
//! every id in the store is in range, so the tree walk may index freely.

use alloc::vec::Vec;

use intern_lang::Symbol;

use super::{Ctx, Repair, Sink};
use crate::{
    def::Def,
    error::{HirError, Malformed, Site},
    expr::{Arg, CaptureMode, Expr, Stmt},
    id::{
        BinderId, ExprId, FieldId, IdKind, ItemId, List, NodeRef, ParamId, PatId, PathId, StmtId,
        TextRef, TyId, VariantId,
    },
    intrinsic::{AsmDir, template_ok},
    item::{Generics, ItemKind, MixinAction, ParamKind, Shape},
    lit::Lit,
    name::{PathRoot, Res},
    ops::{OpKind, Overflow, Policy},
    origin::{ExpnId, Name, Origin},
    pat::Pat,
    store::{Pooled, Store},
    ty::{Bound, GenericArg, Ty},
    walk::place_arg_ok,
};

type R = Result<(), HirError>;

pub(super) fn scan(store: &Store, root: ItemId, ctx: Ctx, sink: &mut Sink) -> R {
    // The root is checked by `preflight`; every node is scanned in arena order.
    let _ = root;
    let mut scan = Scan {
        s: store,
        ctx,
        names: Vec::new(),
    };
    scan.run(sink)
}

struct Scan<'a> {
    s: &'a Store,
    ctx: Ctx,
    /// Scratch for duplicate-name detection: (name, position).
    names: Vec<(Symbol, u32)>,
}

fn malformed(site: Site, problem: Malformed) -> HirError {
    HirError::Malformed { site, problem }
}

/// Converts an arena position to a `u32` index (arena lengths never exceed
/// `MAX_LEN`, so the fallback is never hit).
#[inline]
fn raw(i: usize) -> u32 {
    u32::try_from(i).unwrap_or(u32::MAX - 1)
}

/// Returns `true` for the expressions that denote places.
pub(crate) fn is_place(expr: Option<&Expr>) -> bool {
    matches!(
        expr,
        Some(
            Expr::Path(_)
                | Expr::Field { .. }
                | Expr::DynField { .. }
                | Expr::Index { .. }
                | Expr::Deref(_)
                | Expr::VarVar(_)
                | Expr::Append(_)
                | Expr::Err
        )
    )
}

impl<'a> Scan<'a> {
    fn run(&mut self, sink: &mut Sink) -> R {
        for (i, e) in self.s.expansions.iter().enumerate() {
            // Record `i` is expansion id `i + 1`; references must be strictly earlier.
            let id = (i as u64) + 1;
            if u64::from(e.parent.as_u32()) >= id || u64::from(e.def_site.as_u32()) >= id {
                let expn = ExpnId::from_u32(u32::try_from(id).unwrap_or(u32::MAX));
                sink.report(
                    HirError::ExpansionOrder { expn },
                    Repair::ExpansionRoot(i),
                    true,
                )?;
            }
        }
        for (i, binder) in self.s.binders.nodes.iter().enumerate() {
            let b = BinderId::from_raw_index(raw(i));
            let site = Site::Binder(b);
            if let Err(e) = self.name(binder.name, site) {
                sink.report(e, Repair::BinderMarkRoot(b), true)?;
            }
            if let Err(e) = self.origin(self.s.binders.origin(i), site) {
                sink.report(e, Repair::BinderOriginRoot(b), true)?;
            }
        }
        macro_rules! arena {
            ($len:expr, $id:ident, $node:ident, $check:ident) => {
                for i in 0..$len {
                    let id = $id::from_raw_index(raw(i));
                    let node = NodeRef::$node(id);
                    if let Err(e) = self.origin(self.s.origin(node), Site::Node(node)) {
                        sink.report(e, Repair::OriginRoot(node), true)?;
                    }
                    if let Err(e) = self.$check(id) {
                        sink.report(e, Repair::ErrNode(node), true)?;
                    }
                }
            };
        }
        arena!(self.s.items.len(), ItemId, Item, item);
        arena!(self.s.exprs.len(), ExprId, Expr, expr);
        arena!(self.s.stmts.len(), StmtId, Stmt, stmt);
        arena!(self.s.pats.len(), PatId, Pat, pat);
        arena!(self.s.tys.len(), TyId, Ty, ty);
        arena!(self.s.paths.len(), PathId, Path, path);
        arena!(self.s.fields.len(), FieldId, Field, field);
        arena!(self.s.variants.len(), VariantId, Variant, variant);
        arena!(self.s.params.len(), ParamId, Param, param);
        self.attrs(sink)
    }

    // ---------------------------------------------------------------- ids

    fn id(&self, index: usize, len: usize, kind: IdKind, site: Site) -> R {
        if index < len {
            Ok(())
        } else {
            Err(HirError::Dangling { site, kind, index })
        }
    }

    fn e(&self, id: ExprId, site: Site) -> R {
        self.id(id.index(), self.s.exprs.len(), IdKind::Expr, site)
    }

    fn oe(&self, id: Option<ExprId>, site: Site) -> R {
        id.map_or(Ok(()), |id| self.e(id, site))
    }

    fn p(&self, id: PatId, site: Site) -> R {
        self.id(id.index(), self.s.pats.len(), IdKind::Pat, site)
    }

    fn op_(&self, id: Option<PatId>, site: Site) -> R {
        id.map_or(Ok(()), |id| self.p(id, site))
    }

    fn t(&self, id: TyId, site: Site) -> R {
        self.id(id.index(), self.s.tys.len(), IdKind::Ty, site)
    }

    fn ot(&self, id: Option<TyId>, site: Site) -> R {
        id.map_or(Ok(()), |id| self.t(id, site))
    }

    fn i(&self, id: ItemId, site: Site) -> R {
        self.id(id.index(), self.s.items.len(), IdKind::Item, site)
    }

    fn pa(&self, id: PathId, site: Site) -> R {
        self.id(id.index(), self.s.paths.len(), IdKind::Path, site)
    }

    fn opa(&self, id: Option<PathId>, site: Site) -> R {
        id.map_or(Ok(()), |id| self.pa(id, site))
    }

    fn b(&self, id: BinderId, site: Site) -> R {
        self.id(id.index(), self.s.binders.len(), IdKind::Binder, site)
    }

    fn ob(&self, id: Option<BinderId>, site: Site) -> R {
        id.map_or(Ok(()), |id| self.b(id, site))
    }

    fn expn(&self, expn: ExpnId, site: Site) -> R {
        let index = expn.as_u32() as usize;
        if index <= self.s.expansions.len() {
            Ok(())
        } else {
            Err(HirError::Dangling {
                site,
                kind: IdKind::Expansion,
                index,
            })
        }
    }

    fn origin(&self, origin: Origin, site: Site) -> R {
        self.expn(origin.expn, site)
    }

    fn name(&self, name: Name, site: Site) -> R {
        self.expn(name.mark, site)
    }

    fn list<T: Pooled>(&self, list: List<T>, site: Site) -> Result<&'a [T], HirError> {
        self.s
            .try_list(list)
            .ok_or(HirError::ListOutOfBounds { site })
    }

    fn exprs(&self, list: List<ExprId>, site: Site) -> R {
        for x in self.list(list, site)? {
            self.e(*x, site)?;
        }
        Ok(())
    }

    fn pats(&self, list: List<PatId>, site: Site) -> R {
        for x in self.list(list, site)? {
            self.p(*x, site)?;
        }
        Ok(())
    }

    fn tys(&self, list: List<TyId>, site: Site) -> R {
        for x in self.list(list, site)? {
            self.t(*x, site)?;
        }
        Ok(())
    }

    fn items(&self, list: List<ItemId>, site: Site) -> R {
        for x in self.list(list, site)? {
            self.i(*x, site)?;
        }
        Ok(())
    }

    fn bound(&self, b: Bound, site: Site) -> R {
        match b {
            Bound::Ty(t) => self.t(t, site),
            Bound::Region(p) => self.pa(p, site),
        }
    }

    fn bounds(&self, list: List<Bound>, site: Site) -> R {
        for b in self.list(list, site)? {
            self.bound(*b, site)?;
        }
        Ok(())
    }

    fn generic_arg(&self, a: GenericArg, site: Site) -> R {
        match a {
            GenericArg::Ty(t) | GenericArg::Binding { ty: t, .. } => self.t(t, site),
            GenericArg::Const(e) => self.e(e, site),
            GenericArg::Region(p) => self.pa(p, site),
            GenericArg::Constraint { bounds, .. } => self.bounds(bounds, site),
        }
    }

    fn generic_args(&self, list: List<GenericArg>, site: Site) -> R {
        for a in self.list(list, site)? {
            self.generic_arg(*a, site)?;
        }
        Ok(())
    }

    fn text(&self, text: TextRef, site: Site) -> Result<&'a [u8], HirError> {
        text.range()
            .and_then(|r| self.s.text.get(r))
            .ok_or(HirError::TextOutOfBounds { site })
    }

    /// Fails with `DuplicateName` at the first member, in list order, whose
    /// name an earlier member already has.
    fn no_dups(&mut self, node: NodeRef) -> R {
        self.names.sort_unstable();
        let mut first: Option<(Symbol, u32)> = None;
        for pair in self.names.windows(2) {
            if let [(a, _), (b, at)] = pair {
                if a == b && first.is_none_or(|(_, f)| *at < f) {
                    first = Some((*b, *at));
                }
            }
        }
        match first {
            Some((name, index)) => Err(HirError::DuplicateName { node, name, index }),
            None => Ok(()),
        }
    }

    fn push_name(&mut self, sym: Symbol, at: usize) {
        self.names.push((sym, raw(at)));
    }

    // ---------------------------------------------------------- tables

    fn attrs(&self, sink: &mut Sink) -> R {
        let mut prev: Option<NodeRef> = None;
        for (i, (target, list)) in self.s.attrs.iter().enumerate() {
            let site = Site::Attrs(*target);
            let check = || -> R {
                if target.index() >= self.s.arena_len(*target) {
                    return Err(HirError::Dangling {
                        site,
                        kind: target.kind(),
                        index: target.index(),
                    });
                }
                if prev.is_some_and(|p| p >= *target) {
                    return Err(malformed(site, Malformed::AttrOrder));
                }
                for attr in self.list(*list, site)? {
                    for arg in self.list(attr.args, site)? {
                        match arg.value {
                            None if arg.key.is_none() => {
                                return Err(malformed(site, Malformed::EmptyAttrArg));
                            }
                            Some(crate::item::AttrValue::Lit(lit)) => self.lit(lit, site)?,
                            _ => {}
                        }
                    }
                }
                Ok(())
            };
            match check() {
                Ok(()) => prev = Some(*target),
                Err(e) => sink.report(e, Repair::DropAttr(i), true)?,
            }
        }
        Ok(())
    }

    // -------------------------------------------------------- literals

    fn lit(&self, lit: Lit, site: Site) -> R {
        match lit {
            Lit::Null | Lit::Bool(_) | Lit::Char(_) => Ok(()),
            Lit::Int(int) => {
                if int.negative && int.value == 0 {
                    return Err(malformed(site, Malformed::NegativeZero));
                }
                match int.suffix {
                    Some(p) if !p.is_int() => Err(malformed(site, Malformed::LiteralSuffix)),
                    Some(p) if !int.fits(p) => Err(malformed(site, Malformed::LiteralOutOfRange)),
                    _ => Ok(()),
                }
            }
            Lit::Float(f) => match f.suffix {
                Some(p) if !p.is_float() => Err(malformed(site, Malformed::LiteralSuffix)),
                _ if !f.is_exact() => Err(malformed(site, Malformed::InexactF32)),
                _ => Ok(()),
            },
            Lit::Str(t) => {
                let bytes = self.text(t, site)?;
                if core::str::from_utf8(bytes).is_ok() {
                    Ok(())
                } else {
                    Err(malformed(site, Malformed::InvalidUtf8))
                }
            }
            Lit::Bytes(t) => self.text(t, site).map(|_| ()),
            Lit::BigInt(t) => {
                let bytes = self.text(t, site)?;
                let digits = bytes.strip_prefix(b"-").unwrap_or(bytes);
                if !digits.is_empty() && digits.iter().all(u8::is_ascii_digit) {
                    Ok(())
                } else {
                    Err(malformed(site, Malformed::InvalidBigInt))
                }
            }
        }
    }

    // ------------------------------------------------------------ items

    fn generics(&self, g: &Generics, site: Site) -> R {
        for gp in self.list(g.params, site)? {
            self.b(gp.binder, site)?;
            self.bounds(gp.bounds, site)?;
            self.ot(gp.ty, site)?;
            if let Some(d) = gp.default {
                self.generic_arg(d, site)?;
            }
        }
        for wp in self.list(g.preds, site)? {
            self.bound(wp.subject, site)?;
            self.bounds(wp.bounds, site)?;
        }
        Ok(())
    }

    /// Checks parameter ids and the kind order shared by functions and
    /// closures; problems are reported at the owner, whose repair removes
    /// the parameter list's problem with it.
    fn params(&self, list: List<ParamId>, site: Site, closure: bool) -> R {
        let ids = self.list(list, site)?;
        let mut last: Option<ParamKind> = None;
        for id in ids {
            self.id(id.index(), self.s.params.len(), IdKind::Param, site)?;
            let Some(param) = self.s.param(*id) else {
                continue;
            };
            if param.kind == ParamKind::Receiver && closure {
                return Err(malformed(site, Malformed::ReceiverPlacement));
            }
            let single = matches!(
                param.kind,
                ParamKind::Receiver | ParamKind::Rest | ParamKind::RestNamed
            );
            match last {
                // Kinds are non-decreasing; the single-use kinds strictly increase.
                Some(prev) if prev > param.kind || (single && prev == param.kind) => {
                    return Err(malformed(site, Malformed::ParamOrder));
                }
                _ => {}
            }
            if single && param.default.is_some() {
                return Err(malformed(site, Malformed::ParamDefault));
            }
            last = Some(param.kind);
        }
        Ok(())
    }

    /// Checks field ids and that the fields fit `shape`; named fields are unique.
    fn fields(&mut self, list: List<FieldId>, shape: Shape, node: NodeRef) -> R {
        let site = Site::Node(node);
        self.names.clear();
        for (at, id) in self.list(list, site)?.iter().enumerate() {
            self.id(id.index(), self.s.fields.len(), IdKind::Field, site)?;
            let named = self.s.field(*id).and_then(|f| f.name);
            let fits = match shape {
                Shape::Named => named.is_some(),
                Shape::Tuple => named.is_none(),
                Shape::Unit => false,
            };
            if !fits {
                return Err(malformed(site, Malformed::ShapeFields));
            }
            if let Some(name) = named {
                self.push_name(name.sym, at);
            }
        }
        self.no_dups(node)
    }

    fn item(&mut self, id: ItemId) -> R {
        let node = NodeRef::Item(id);
        let site = Site::Node(node);
        let Some(item) = self.s.item(id) else {
            return Ok(());
        };
        if let Some(name) = item.name {
            self.name(name, site)?;
        }
        let needs_name = match item.kind {
            ItemKind::Fn(_)
            | ItemKind::Record(_)
            | ItemKind::Sum(_)
            | ItemKind::Class(_)
            | ItemKind::Interface(_)
            | ItemKind::Alias { .. }
            | ItemKind::AssocType { .. }
            | ItemKind::Const { .. }
            | ItemKind::Global { .. } => Some(true),
            ItemKind::Impl(_) | ItemKind::MixinUse(_) | ItemKind::Import { glob: true, .. } => {
                Some(false)
            }
            ItemKind::Module { .. } | ItemKind::Import { .. } | ItemKind::Err => None,
        };
        match (needs_name, item.name.is_some()) {
            (Some(true), false) => return Err(malformed(site, Malformed::MissingName)),
            (Some(false), true) => return Err(malformed(site, Malformed::UnexpectedName)),
            _ => {}
        }
        match &item.kind {
            ItemKind::Fn(f) => {
                self.generics(&f.generics, site)?;
                self.params(f.params, site, false)?;
                self.ot(f.ret, site)?;
                self.ot(f.throws, site)?;
                self.oe(f.body, site)
            }
            ItemKind::Record(r) => {
                self.generics(&r.generics, site)?;
                if r.is_union && r.shape != Shape::Named {
                    return Err(malformed(site, Malformed::ShapeFields));
                }
                self.fields(r.fields, r.shape, node)
            }
            ItemKind::Sum(d) => {
                self.generics(&d.generics, site)?;
                self.names.clear();
                for (at, v) in self.list(d.variants, site)?.iter().enumerate() {
                    self.id(v.index(), self.s.variants.len(), IdKind::Variant, site)?;
                    if let Some(variant) = self.s.variant(*v) {
                        self.push_name(variant.name.sym, at);
                    }
                }
                self.no_dups(node)
            }
            ItemKind::Class(c) => {
                self.generics(&c.generics, site)?;
                self.tys(c.bases, site)?;
                self.tys(c.interfaces, site)?;
                self.items(c.items, site)?;
                self.fields(c.fields, Shape::Named, node)
            }
            ItemKind::Interface(i) => {
                self.generics(&i.generics, site)?;
                self.tys(i.supers, site)?;
                self.items(i.items, site)
            }
            ItemKind::Impl(i) => {
                self.generics(&i.generics, site)?;
                self.ot(i.interface, site)?;
                self.t(i.self_ty, site)?;
                self.items(i.items, site)
            }
            ItemKind::Alias { generics, ty } => {
                self.generics(generics, site)?;
                self.t(*ty, site)
            }
            ItemKind::AssocType { bounds, default } => {
                self.bounds(*bounds, site)?;
                self.ot(*default, site)
            }
            ItemKind::Const { ty, value } => {
                self.ot(*ty, site)?;
                self.oe(*value, site)
            }
            ItemKind::Global { ty, init, .. } => {
                self.ot(*ty, site)?;
                self.oe(*init, site)
            }
            ItemKind::Module { items, body, .. } => {
                self.items(*items, site)?;
                self.oe(*body, site)
            }
            ItemKind::Import { path, .. } => self.pa(*path, site),
            ItemKind::MixinUse(m) => {
                self.tys(m.mixins, site)?;
                for rule in self.list(m.rules, site)? {
                    self.ot(rule.from, site)?;
                    if let MixinAction::Insteadof(others) = rule.action {
                        self.tys(others, site)?;
                    }
                }
                Ok(())
            }
            ItemKind::Err => Ok(()),
        }
    }

    fn field(&self, id: FieldId) -> R {
        let site = Site::Node(NodeRef::Field(id));
        let Some(field) = self.s.field(id) else {
            return Ok(());
        };
        self.ot(field.ty, site)?;
        self.oe(field.default, site)
    }

    fn variant(&mut self, id: VariantId) -> R {
        let node = NodeRef::Variant(id);
        let site = Site::Node(node);
        let Some(variant) = self.s.variant(id) else {
            return Ok(());
        };
        self.oe(variant.discriminant, site)?;
        self.fields(variant.fields, variant.shape, node)
    }

    fn param(&self, id: ParamId) -> R {
        let site = Site::Node(NodeRef::Param(id));
        let Some(param) = self.s.param(id) else {
            return Ok(());
        };
        self.p(param.pat, site)?;
        self.ot(param.ty, site)?;
        self.oe(param.default, site)
    }

    // ------------------------------------------------------ expressions

    fn args(&mut self, list: List<Arg>, node: NodeRef) -> R {
        let site = Site::Node(node);
        self.names.clear();
        for (at, arg) in self.list(list, site)?.iter().enumerate() {
            self.e(arg.value, site)?;
            if arg.place && (!place_arg_ok(arg.kind) || !is_place(self.s.expr(arg.value))) {
                return Err(malformed(site, Malformed::PlaceArg));
            }
            if let crate::expr::ArgKind::Named(name) = arg.kind {
                self.push_name(name.sym, at);
            }
        }
        self.no_dups(node)
    }

    fn policy(&self, expr: ExprId, op: crate::ops::Op) -> R {
        if op.policy_matches() {
            Ok(())
        } else {
            Err(HirError::Policy { expr })
        }
    }

    fn expr(&mut self, id: ExprId) -> R {
        let node = NodeRef::Expr(id);
        let site = Site::Node(node);
        let Some(expr) = self.s.expr(id) else {
            return Ok(());
        };
        match *expr {
            Expr::Lit(lit) => self.lit(lit, site),
            Expr::Path(p) => self.pa(p, site),
            Expr::Tuple(xs) | Expr::Array(xs) => self.exprs(xs, site),
            Expr::Repeat { elem, count } => {
                self.e(elem, site)?;
                self.e(count, site)
            }
            Expr::Record { path, fields, base } => {
                self.opa(path, site)?;
                self.names.clear();
                for (at, fi) in self.list(fields, site)?.iter().enumerate() {
                    self.e(fi.value, site)?;
                    self.push_name(fi.name.sym, at);
                }
                self.no_dups(node)?;
                self.oe(base, site)
            }
            Expr::Map(entries) => {
                for entry in self.list(entries, site)? {
                    self.oe(entry.key, site)?;
                    self.e(entry.value, site)?;
                }
                Ok(())
            }
            Expr::Call { callee, args } => {
                self.e(callee, site)?;
                self.args(args, node)
            }
            Expr::MethodCall {
                receiver,
                generic_args,
                args,
                ..
            } => {
                self.e(receiver, site)?;
                self.generic_args(generic_args, site)?;
                self.args(args, node)
            }
            Expr::DynMethodCall {
                receiver,
                name,
                args,
            } => {
                self.e(receiver, site)?;
                self.e(name, site)?;
                self.args(args, node)
            }
            Expr::Field { base, .. } | Expr::Deref(base) => self.e(base, site),
            Expr::DynField { base, name } => {
                self.e(base, site)?;
                self.e(name, site)
            }
            Expr::Index { base, index } => {
                self.e(base, site)?;
                self.e(index, site)
            }
            Expr::Op { op, args } => {
                self.exprs(args, site)?;
                if args.len() != op.kind.arity() {
                    return Err(HirError::Arity {
                        expr: id,
                        expected: op.kind.arity(),
                        found: args.len(),
                    });
                }
                if !op.kind.target_ok() {
                    return Err(malformed(site, Malformed::ConversionTarget));
                }
                self.policy(id, op)?;
                // `int_cast<T>` has the static result type `T`.
                if op.promotes() && op.kind.target().is_some() {
                    return Err(malformed(site, Malformed::PromoteOnStaticResult));
                }
                Ok(())
            }
            Expr::Cast { expr, ty, policy } => {
                self.e(expr, site)?;
                self.t(ty, site)?;
                if !policy.same_fields(Policy::CAST) {
                    return Err(HirError::Policy { expr: id });
                }
                // A cast's result has its target type; only `Any` is dynamic.
                let promotes = matches!(policy.overflow, Some(Overflow::Promote));
                if promotes && !matches!(self.s.ty(ty), Some(Ty::Any)) {
                    return Err(malformed(site, Malformed::PromoteOnStaticResult));
                }
                Ok(())
            }
            Expr::Assign { target, op, value } => {
                self.e(target, site)?;
                self.e(value, site)?;
                if let Some(op) = op {
                    if !compound_op(op.kind) {
                        return Err(malformed(site, Malformed::CompoundAssignOp));
                    }
                    self.policy(id, op)?;
                }
                if is_place(self.s.expr(target)) {
                    Ok(())
                } else {
                    Err(malformed(site, Malformed::AssignTarget))
                }
            }
            Expr::LetPlace {
                binder,
                place,
                body,
            } => {
                self.b(binder, site)?;
                self.e(place, site)?;
                self.e(body, site)?;
                let place = self.s.expr(place);
                if is_place(place) && !matches!(place, Some(Expr::Append(_))) {
                    Ok(())
                } else {
                    Err(malformed(site, Malformed::AssignTarget))
                }
            }
            Expr::RefAssign { target, source } => {
                self.e(target, site)?;
                self.e(source, site)?;
                let source_ok = is_place(self.s.expr(source))
                    && !matches!(self.s.expr(source), Some(Expr::Append(_)));
                if is_place(self.s.expr(target)) && source_ok {
                    Ok(())
                } else {
                    Err(malformed(site, Malformed::AssignTarget))
                }
            }
            Expr::Borrow { expr: x, .. }
            | Expr::Throw(x)
            | Expr::Await(x)
            | Expr::Spawn(x)
            | Expr::YieldFrom(x)
            | Expr::VarVar(x)
            | Expr::Append(x) => self.e(x, site),
            Expr::Block(block) => {
                for st in self.list(block.stmts, site)? {
                    self.id(st.index(), self.s.stmts.len(), IdKind::Stmt, site)?;
                }
                self.oe(block.tail, site)?;
                self.ob(block.label, site)
            }
            Expr::If { cond, then, else_ } => {
                self.e(cond, site)?;
                self.e(then, site)?;
                self.oe(else_, site)
            }
            Expr::Match { scrutinee, arms } => {
                self.e(scrutinee, site)?;
                self.arms(arms, site)
            }
            Expr::Loop { label, body, step } => {
                self.ob(label, site)?;
                self.e(body, site)?;
                self.oe(step, site)
            }
            Expr::Break { label, value } => {
                self.ob(label, site)?;
                self.oe(value, site)
            }
            Expr::Continue { label } => self.ob(label, site),
            Expr::Return(x) => self.oe(x, site),
            Expr::Yield { key, value } => {
                self.oe(key, site)?;
                self.oe(value, site)?;
                if key.is_some() && value.is_none() {
                    return Err(malformed(site, Malformed::YieldKey));
                }
                Ok(())
            }
            Expr::Closure(c) => {
                self.params(c.params, site, true)?;
                self.ot(c.ret, site)?;
                self.e(c.body, site)?;
                self.ob(c.self_binder, site)?;
                for cap in self.list(c.captures, site)? {
                    self.pa(cap.outer, site)?;
                    self.b(cap.binder, site)?;
                    if cap.mode == CaptureMode::Infer {
                        return Err(malformed(site, Malformed::InferCapture));
                    }
                }
                Ok(())
            }
            Expr::Try {
                body,
                catches,
                finally,
            } => {
                self.e(body, site)?;
                self.arms(catches, site)?;
                self.oe(finally, site)
            }
            Expr::Asm(asm) => {
                let operands = self.list(asm.operands, site)?;
                let template = self.text(asm.template, site)?;
                if !template_ok(template, operands.len()) {
                    return Err(malformed(site, Malformed::AsmTemplate));
                }
                for op in operands {
                    self.e(op.expr, site)?;
                    let constraint = self.text(op.constraint, site)?;
                    let target = self.s.expr(op.expr);
                    let ok = core::str::from_utf8(constraint).is_ok()
                        && match op.dir {
                            AsmDir::Out | AsmDir::InOut => is_place(target),
                            AsmDir::Sym => matches!(target, Some(Expr::Path(_))),
                            AsmDir::In | AsmDir::Const => true,
                        };
                    if !ok {
                        return Err(malformed(site, Malformed::AsmOperand));
                    }
                }
                Ok(())
            }
            Expr::Intrinsic {
                kind,
                generic_args,
                args,
            } => {
                self.generic_args(generic_args, site)?;
                self.exprs(args, site)?;
                if kind.arity().is_some_and(|n| n != args.len()) {
                    return Err(malformed(site, Malformed::IntrinsicArity));
                }
                if !kind.orderings_ok() {
                    return Err(malformed(site, Malformed::MemOrder));
                }
                Ok(())
            }
            Expr::Err => Ok(()),
        }
    }

    fn arms(&self, arms: List<crate::expr::Arm>, site: Site) -> R {
        for arm in self.list(arms, site)? {
            self.p(arm.pat, site)?;
            self.oe(arm.guard, site)?;
            self.e(arm.body, site)?;
        }
        Ok(())
    }

    fn stmt(&self, id: StmtId) -> R {
        let site = Site::Node(NodeRef::Stmt(id));
        let Some(stmt) = self.s.stmt(id) else {
            return Ok(());
        };
        match *stmt {
            Stmt::Let {
                pat,
                ty,
                init,
                else_,
            } => {
                self.p(pat, site)?;
                self.ot(ty, site)?;
                self.oe(init, site)?;
                self.oe(else_, site)?;
                if else_.is_some() && init.is_none() {
                    return Err(malformed(site, Malformed::LetElseWithoutInit));
                }
                Ok(())
            }
            Stmt::Expr(e) | Stmt::Defer(e) => self.e(e, site),
            Stmt::Item(i) => self.i(i, site),
            Stmt::Static { binder, ty, init } => {
                self.b(binder, site)?;
                self.ot(ty, site)?;
                self.oe(init, site)
            }
            Stmt::Global { binder, path } => {
                self.b(binder, site)?;
                self.pa(path, site)
            }
            Stmt::Err => Ok(()),
        }
    }

    // --------------------------------------------------------- patterns

    fn pat(&mut self, id: PatId) -> R {
        let node = NodeRef::Pat(id);
        let site = Site::Node(node);
        let Some(pat) = self.s.pat(id) else {
            return Ok(());
        };
        match *pat {
            Pat::Wild | Pat::Err => Ok(()),
            Pat::Bind { binder, sub, .. } => {
                self.b(binder, site)?;
                self.op_(sub, site)
            }
            Pat::Ident { binder, path } => {
                self.b(binder, site)?;
                self.pa(path, site)
            }
            Pat::Lit(lit) => self.lit(lit, site),
            Pat::Range { lo, hi, .. } => {
                self.op_(lo, site)?;
                self.op_(hi, site)?;
                if lo.is_none() && hi.is_none() {
                    return Err(malformed(site, Malformed::RangeWithoutBounds));
                }
                // Bounds are literal or constant-path patterns; literals of one class.
                let class = |p: Option<PatId>| -> Result<Option<u8>, HirError> {
                    match p.and_then(|p| self.s.pat(p)) {
                        None | Some(Pat::Path(_)) => Ok(None),
                        Some(Pat::Lit(Lit::Int(_))) => Ok(Some(0)),
                        Some(Pat::Lit(Lit::Char(_))) => Ok(Some(1)),
                        Some(Pat::Lit(Lit::Float(_))) => Ok(Some(2)),
                        Some(Pat::Lit(_)) => Err(malformed(site, Malformed::RangeBoundKinds)),
                        Some(_) => Err(malformed(site, Malformed::RangeBound)),
                    }
                };
                if let (Some(a), Some(b)) = (class(lo)?, class(hi)?) {
                    if a != b {
                        return Err(malformed(site, Malformed::RangeBoundKinds));
                    }
                }
                Ok(())
            }
            Pat::Tuple { elems, rest } => self.rest_elems(elems, rest, site),
            Pat::Ctor { path, elems, rest } => {
                self.pa(path, site)?;
                self.rest_elems(elems, rest, site)
            }
            Pat::Record { path, fields, .. } => {
                self.opa(path, site)?;
                self.names.clear();
                for (at, fp) in self.list(fields, site)?.iter().enumerate() {
                    self.p(fp.pat, site)?;
                    self.push_name(fp.name.sym, at);
                }
                self.no_dups(node)
            }
            Pat::Path(p) => self.pa(p, site),
            Pat::Slice { prefix, rest } => {
                self.pats(prefix, site)?;
                if let Some(rest) = rest {
                    self.op_(rest.bind, site)?;
                    self.pats(rest.suffix, site)?;
                }
                Ok(())
            }
            Pat::Or(alts) => {
                if alts.is_empty() {
                    return Err(malformed(site, Malformed::EmptyOrPattern));
                }
                self.pats(alts, site)
            }
            Pat::Ref { inner, .. } => self.p(inner, site),
            Pat::TypeTest { ty, pat } => {
                self.t(ty, site)?;
                self.op_(pat, site)
            }
        }
    }

    fn rest_elems(&self, elems: List<PatId>, rest: Option<u32>, site: Site) -> R {
        self.pats(elems, site)?;
        match rest {
            Some(at) if at as usize > elems.len() => {
                Err(malformed(site, Malformed::RestOutOfRange))
            }
            _ => Ok(()),
        }
    }

    // ------------------------------------------------------------ types

    fn ty(&self, id: TyId) -> R {
        let site = Site::Node(NodeRef::Ty(id));
        let Some(ty) = self.s.ty(id) else {
            return Ok(());
        };
        match *ty {
            Ty::Infer | Ty::Prim(_) | Ty::Any | Ty::Never | Ty::SelfTy | Ty::Err => Ok(()),
            Ty::Path(p) => self.pa(p, site),
            Ty::Tuple(ts) => self.tys(ts, site),
            Ty::Union(ts) | Ty::Intersection(ts) => {
                self.tys(ts, site)?;
                let members = self.list(ts, site)?;
                crate::canon::check(self.s, members, matches!(ty, Ty::Union(_)))
                    .map_err(|problem| malformed(site, problem))
            }
            Ty::Object(bs) | Ty::Impl(bs) => self.bounds(bs, site),
            Ty::Array { elem, len } => {
                self.t(elem, site)?;
                self.e(len, site)
            }
            Ty::Slice(t) | Ty::Ptr { inner: t, .. } | Ty::Nullable(t) => self.t(t, site),
            Ty::Ref { region, inner, .. } => {
                self.opa(region, site)?;
                self.t(inner, site)
            }
            Ty::Fn {
                params,
                ret,
                throws,
                ..
            } => {
                self.tys(params, site)?;
                self.t(ret, site)?;
                self.ot(throws, site)
            }
        }
    }

    fn path(&self, id: PathId) -> R {
        let site = Site::Node(NodeRef::Path(id));
        let Some(path) = self.s.path(id) else {
            return Ok(());
        };
        let segments = self.list(path.segments, site)?;
        if segments.is_empty() && path.res != Res::Err {
            return Err(malformed(site, Malformed::EmptyPath));
        }
        for seg in segments {
            self.name(seg.name, site)?;
            self.origin(seg.origin, site)?;
            self.generic_args(seg.args, site)?;
        }
        if let Some(q) = path.qself {
            self.t(q.ty, site)?;
            if path.root != PathRoot::Relative || q.trait_len as usize >= segments.len() {
                return Err(malformed(site, Malformed::PathShape));
            }
        }
        if path.root == PathRoot::Super(0) {
            return Err(malformed(site, Malformed::PathShape));
        }
        match path.res {
            Res::Local(b) => self.b(b, site),
            Res::Def(def) => match def.def() {
                // Only this unit's definitions can be checked here; the tag
                // and kind are checked with the namespace in the tree walk.
                Def::Item(i) if def.unit() == self.unit() => self.i(i, site),
                Def::Variant(v) if def.unit() == self.unit() => {
                    self.id(v.index(), self.s.variants.len(), IdKind::Variant, site)
                }
                _ => Ok(()),
            },
            Res::Unresolved | Res::Prim(_) | Res::Extern(_) | Res::Err => Ok(()),
        }
    }

    fn unit(&self) -> crate::def::UnitId {
        self.ctx.unit
    }
}

/// The ops a compound assignment may use: arithmetic, bitwise, and shifts.
const fn compound_op(kind: OpKind) -> bool {
    matches!(
        kind,
        OpKind::Add
            | OpKind::Sub
            | OpKind::Mul
            | OpKind::Div
            | OpKind::FloorDiv
            | OpKind::Rem
            | OpKind::FloorMod
            | OpKind::And
            | OpKind::Or
            | OpKind::Xor
            | OpKind::Shl
            | OpKind::Shr
            | OpKind::Pow
    )
}
