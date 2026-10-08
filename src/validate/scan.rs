//! Pass 1: every node on its own, in arena order.
//!
//! Checks every id, list, and text range against its arena or pool, every
//! origin and mark against the expansion table, and every rule that needs only
//! the node itself (literals, op arity and policy, shapes, duplicate member
//! names, parameter order). After this pass every id in the store is in range,
//! so the tree walk may index freely.

use alloc::vec::Vec;

use intern_lang::Symbol;

use crate::{
    error::{HirError, Malformed, Site},
    expr::{ArgKind, CaptureMode, Expr, Stmt},
    id::{
        BinderId, ExprId, FieldId, IdKind, ItemId, List, NodeRef, ParamId, PatId, PathId, StmtId,
        TextRef, TyId, VariantId,
    },
    item::{Generics, ItemKind, ParamKind, Shape},
    lit::Lit,
    name::Res,
    ops::Policy,
    origin::{ExpnId, Name, Origin},
    pat::Pat,
    store::{Pooled, Store},
    ty::Ty,
};

type R = Result<(), HirError>;

pub(super) fn scan(store: &Store, root: ItemId) -> R {
    let mut scan = Scan {
        s: store,
        names: Vec::new(),
    };
    scan.run(root)
}

struct Scan<'a> {
    s: &'a Store,
    /// Scratch for duplicate-name detection.
    names: Vec<Symbol>,
}

fn malformed(site: Site, problem: Malformed) -> HirError {
    HirError::Malformed { site, problem }
}

impl<'a> Scan<'a> {
    fn run(&mut self, root: ItemId) -> R {
        self.expansions()?;
        if root.index() >= self.s.items.len() {
            return Err(HirError::Dangling {
                site: Site::Root,
                kind: IdKind::Item,
                index: root.index(),
            });
        }
        for (i, binder) in self.s.binders.nodes.iter().enumerate() {
            let site = Site::Binder(BinderId::from_raw_index(raw(i)));
            self.name(binder.name, site)?;
            self.origin(self.s.binders.origin(i), site)?;
        }
        for i in 0..self.s.items.len() {
            self.item(ItemId::from_raw_index(raw(i)))?;
        }
        for i in 0..self.s.exprs.len() {
            self.expr(ExprId::from_raw_index(raw(i)))?;
        }
        for i in 0..self.s.stmts.len() {
            self.stmt(StmtId::from_raw_index(raw(i)))?;
        }
        for i in 0..self.s.pats.len() {
            self.pat(PatId::from_raw_index(raw(i)))?;
        }
        for i in 0..self.s.tys.len() {
            self.ty(TyId::from_raw_index(raw(i)))?;
        }
        for i in 0..self.s.paths.len() {
            self.path(PathId::from_raw_index(raw(i)))?;
        }
        for i in 0..self.s.fields.len() {
            self.field(FieldId::from_raw_index(raw(i)))?;
        }
        for i in 0..self.s.variants.len() {
            self.variant(VariantId::from_raw_index(raw(i)))?;
        }
        for i in 0..self.s.params.len() {
            self.param(ParamId::from_raw_index(raw(i)))?;
        }
        self.attrs()
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

    fn text(&self, text: TextRef, site: Site) -> Result<&'a [u8], HirError> {
        text.range()
            .and_then(|r| self.s.text.get(r))
            .ok_or(HirError::TextOutOfBounds { site })
    }

    /// Fails with `DuplicateName` if two of the collected names are equal.
    fn no_dups(&mut self, node: NodeRef) -> R {
        self.names.sort_unstable();
        for pair in self.names.windows(2) {
            if let [a, b] = pair {
                if a == b {
                    return Err(HirError::DuplicateName { node, name: *a });
                }
            }
        }
        Ok(())
    }

    // ---------------------------------------------------------- tables

    fn expansions(&self) -> R {
        for (i, e) in self.s.expansions.iter().enumerate() {
            // Record `i` is expansion id `i + 1`; references must be strictly earlier.
            let id = (i as u64) + 1;
            let expn = ExpnId::from_u32(u32::try_from(id).unwrap_or(u32::MAX));
            if u64::from(e.parent.as_u32()) >= id || u64::from(e.def_site.as_u32()) >= id {
                return Err(HirError::ExpansionOrder { expn });
            }
        }
        Ok(())
    }

    fn attrs(&self) -> R {
        let mut prev: Option<NodeRef> = None;
        for (target, list) in &self.s.attrs {
            let site = Site::Attrs(*target);
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
            prev = Some(*target);
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
        }
        Ok(())
    }

    // -------------------------------------------------------- literals

    fn lit(&self, lit: Lit, site: Site) -> R {
        match lit {
            Lit::Null | Lit::Bool(_) | Lit::Char(_) => Ok(()),
            Lit::Int(int) => match int.suffix {
                Some(p) if !p.is_int() => Err(malformed(site, Malformed::LiteralSuffix)),
                Some(p) if !int.fits(p) => Err(malformed(site, Malformed::LiteralOutOfRange)),
                _ => Ok(()),
            },
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
            self.tys(gp.bounds, site)?;
            self.ot(gp.ty, site)?;
            self.ot(gp.default, site)?;
        }
        for wp in self.list(g.preds, site)? {
            self.t(wp.ty, site)?;
            self.tys(wp.bounds, site)?;
        }
        Ok(())
    }

    /// Checks parameter ids and the kind order shared by functions and closures.
    fn params(&self, list: List<ParamId>, site: Site, closure: bool) -> R {
        let ids = self.list(list, site)?;
        let mut last: Option<ParamKind> = None;
        for id in ids {
            self.id(id.index(), self.s.params.len(), IdKind::Param, site)?;
            let Some(param) = self.s.param(*id) else {
                continue;
            };
            let psite = Site::Node(NodeRef::Param(*id));
            if param.kind == ParamKind::Receiver && closure {
                return Err(malformed(psite, Malformed::ReceiverPlacement));
            }
            let single = matches!(
                param.kind,
                ParamKind::Receiver | ParamKind::Rest | ParamKind::RestNamed
            );
            match last {
                // Kinds are non-decreasing; the single-use kinds strictly increase.
                Some(prev) if prev > param.kind || (single && prev == param.kind) => {
                    return Err(malformed(psite, Malformed::ParamOrder));
                }
                _ => {}
            }
            if single && param.default.is_some() {
                return Err(malformed(psite, Malformed::ParamDefault));
            }
            last = Some(param.kind);
        }
        Ok(())
    }

    /// Checks field ids and that the fields fit `shape`; named fields are unique.
    fn fields(&mut self, list: List<FieldId>, shape: Shape, node: NodeRef) -> R {
        let site = Site::Node(node);
        self.names.clear();
        for id in self.list(list, site)? {
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
                self.names.push(name.sym);
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
        self.origin(self.s.items.origin(id.index()), site)?;
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
            ItemKind::Impl(_) | ItemKind::Import { glob: true, .. } => Some(false),
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
                self.fields(r.fields, r.shape, node)
            }
            ItemKind::Sum(d) => {
                self.generics(&d.generics, site)?;
                self.names.clear();
                for v in self.list(d.variants, site)? {
                    self.id(v.index(), self.s.variants.len(), IdKind::Variant, site)?;
                    if let Some(variant) = self.s.variant(*v) {
                        self.names.push(variant.name.sym);
                    }
                }
                self.no_dups(node)
            }
            ItemKind::Class(c) => {
                self.generics(&c.generics, site)?;
                self.ot(c.base, site)?;
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
                self.tys(*bounds, site)?;
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
            ItemKind::Module { items } => self.items(*items, site),
            ItemKind::Import { path, .. } => self.pa(*path, site),
            ItemKind::Err => Ok(()),
        }
    }

    fn field(&self, id: FieldId) -> R {
        let site = Site::Node(NodeRef::Field(id));
        let Some(field) = self.s.field(id) else {
            return Ok(());
        };
        self.origin(self.s.fields.origin(id.index()), site)?;
        self.ot(field.ty, site)?;
        self.oe(field.default, site)
    }

    fn variant(&mut self, id: VariantId) -> R {
        let node = NodeRef::Variant(id);
        let site = Site::Node(node);
        let Some(variant) = self.s.variant(id) else {
            return Ok(());
        };
        self.origin(self.s.variants.origin(id.index()), site)?;
        self.oe(variant.discriminant, site)?;
        self.fields(variant.fields, variant.shape, node)
    }

    fn param(&self, id: ParamId) -> R {
        let site = Site::Node(NodeRef::Param(id));
        let Some(param) = self.s.param(id) else {
            return Ok(());
        };
        self.origin(self.s.params.origin(id.index()), site)?;
        self.p(param.pat, site)?;
        self.ot(param.ty, site)?;
        self.oe(param.default, site)
    }

    // ------------------------------------------------------ expressions

    fn args(&mut self, list: List<crate::expr::Arg>, node: NodeRef) -> R {
        let site = Site::Node(node);
        self.names.clear();
        for arg in self.list(list, site)? {
            self.e(arg.value, site)?;
            if let ArgKind::Named(name) = arg.kind {
                self.names.push(name.sym);
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
        self.origin(self.s.exprs.origin(id.index()), site)?;
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
                for fi in self.list(fields, site)? {
                    self.e(fi.value, site)?;
                    self.names.push(fi.name.sym);
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
                self.tys(generic_args, site)?;
                self.args(args, node)
            }
            Expr::Field { base, .. } => self.e(base, site),
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
                let promotes = matches!(policy.overflow, Some(crate::ops::Overflow::Promote));
                if promotes && !matches!(self.s.ty(ty), Some(Ty::Any)) {
                    return Err(malformed(site, Malformed::PromoteOnStaticResult));
                }
                Ok(())
            }
            Expr::Assign { target, op, value } => {
                self.e(target, site)?;
                self.e(value, site)?;
                if let Some(op) = op {
                    if op.kind.arity() != 2 {
                        return Err(malformed(site, Malformed::CompoundAssignOp));
                    }
                    self.policy(id, op)?;
                }
                match self.s.expr(target) {
                    Some(
                        Expr::Path(_)
                        | Expr::Field { .. }
                        | Expr::Index { .. }
                        | Expr::Deref(_)
                        | Expr::Err,
                    ) => Ok(()),
                    _ => Err(malformed(site, Malformed::AssignTarget)),
                }
            }
            Expr::Deref(x)
            | Expr::Borrow { expr: x, .. }
            | Expr::Throw(x)
            | Expr::Await(x)
            | Expr::Spawn(x) => self.e(x, site),
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
            Expr::Return(x) | Expr::Yield(x) => self.oe(x, site),
            Expr::Closure(c) => {
                self.params(c.params, site, true)?;
                self.ot(c.ret, site)?;
                self.e(c.body, site)?;
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
        self.origin(self.s.stmts.origin(id.index()), site)?;
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
        self.origin(self.s.pats.origin(id.index()), site)?;
        match *pat {
            Pat::Wild | Pat::Err => Ok(()),
            Pat::Bind { binder, sub, .. } => {
                self.b(binder, site)?;
                self.op_(sub, site)
            }
            Pat::Lit(lit) => self.lit(lit, site),
            Pat::Range { lo, hi, .. } => {
                if lo.is_none() && hi.is_none() {
                    return Err(malformed(site, Malformed::RangeWithoutBounds));
                }
                let class = |lit: Lit| match lit {
                    Lit::Int(_) => Some(0u8),
                    Lit::Char(_) => Some(1),
                    Lit::Float(_) => Some(2),
                    _ => None,
                };
                let classes = [lo.map(class), hi.map(class)];
                if classes.iter().any(|c| matches!(c, Some(None))) {
                    return Err(malformed(site, Malformed::RangeBoundKinds));
                }
                if let [Some(a), Some(b)] = classes {
                    if a != b {
                        return Err(malformed(site, Malformed::RangeBoundKinds));
                    }
                }
                if let Some(lo) = lo {
                    self.lit(lo, site)?;
                }
                if let Some(hi) = hi {
                    self.lit(hi, site)?;
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
                for fp in self.list(fields, site)? {
                    self.p(fp.pat, site)?;
                    self.names.push(fp.name.sym);
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
        self.origin(self.s.tys.origin(id.index()), site)?;
        match *ty {
            Ty::Infer | Ty::Prim(_) | Ty::Any | Ty::Never | Ty::SelfTy | Ty::Err => Ok(()),
            Ty::Path(p) => self.pa(p, site),
            Ty::Tuple(ts) | Ty::Object(ts) => self.tys(ts, site),
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
            Ty::Const(e) => self.e(e, site),
        }
    }

    fn path(&self, id: PathId) -> R {
        let site = Site::Node(NodeRef::Path(id));
        let Some(path) = self.s.path(id) else {
            return Ok(());
        };
        self.origin(self.s.paths.origin(id.index()), site)?;
        let segments = self.list(path.segments, site)?;
        if segments.is_empty() {
            return Err(malformed(site, Malformed::EmptyPath));
        }
        for seg in segments {
            self.name(seg.name, site)?;
            self.origin(seg.origin, site)?;
            self.tys(seg.args, site)?;
        }
        match path.res {
            Res::Local(b) => self.b(b, site),
            Res::Item(i) => self.i(i, site),
            Res::Variant(v) => self.id(v.index(), self.s.variants.len(), IdKind::Variant, site),
            Res::Unresolved | Res::Prim(_) | Res::Err => Ok(()),
        }
    }
}

/// Converts an arena position to a `u32` index. Arena lengths never exceed
/// `MAX_LEN` (the builder refuses to grow past it), so the fallback is never hit.
#[inline]
fn raw(i: usize) -> u32 {
    u32::try_from(i).unwrap_or(u32::MAX - 1)
}
