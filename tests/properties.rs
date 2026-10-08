//! Property tests for the invariants in `dev/DIRECTIVES.md` §4.
//!
//! Random programs are generated as small recipe trees and lowered through the
//! builder. While lowering, a direct reference implementation of the scoping,
//! frame, and jump rules (a recursive environment walk, the textbook way)
//! computes what the validator must say. The validator's single-pass interval
//! algorithm is held to that reference.

use hir_lang::{
    Arm, Binder, BinderId, BinderKind, Builder, CaptureMode, Closure, Control, Effects, Expansion,
    ExpnId, ExpnKind, Expr, ExprId, FnDef, Hir, HirError, IdKind, Item, ItemId, ItemKind,
    JumpProblem, List, Lit, Name, NodeRef, Ns, Op, OpKind, Param, Pat, PatId, PathId, Policy, Res,
    Span, Stmt,
};
use intern_lang::{Interner, Symbol};
use proptest::{collection::vec, option, prelude::*};

// ------------------------------------------------------------------ recipes

#[derive(Clone, Debug)]
enum P {
    Wild,
    Bind(u32),
    Lit(i8),
    Tuple(Vec<P>),
    /// `p | p`: both alternatives bind the same binders.
    Or(Box<P>),
}

#[derive(Clone, Debug)]
enum S {
    Let(P, Option<E>),
    Expr(E),
    Fn(Vec<P>, E),
}

#[derive(Clone, Debug)]
enum E {
    Lit(i8),
    Var(u16),
    Add(Box<E>, Box<E>),
    Block(Vec<S>, Option<Box<E>>),
    If(Box<E>, Box<E>, Option<Box<E>>),
    Match(Box<E>, Vec<(P, Option<E>, E)>),
    Loop(bool, Box<E>),
    Break(u8),
    Continue(u8),
    Closure(bool, Vec<P>, Box<E>),
    Call(Box<E>, Vec<E>),
}

fn pat() -> impl Strategy<Value = P> {
    let leaf = prop_oneof![
        Just(P::Wild),
        Just(P::Bind(0)),
        Just(P::Bind(0)),
        any::<i8>().prop_map(P::Lit),
    ];
    leaf.prop_recursive(3, 8, 3, |inner| {
        prop_oneof![
            vec(inner.clone(), 0..3).prop_map(P::Tuple),
            inner.prop_map(|p| P::Or(Box::new(p))),
        ]
    })
}

fn expr() -> impl Strategy<Value = E> {
    let leaf = prop_oneof![
        3 => any::<i8>().prop_map(E::Lit),
        6 => any::<u16>().prop_map(E::Var),
        1 => (0u8..3).prop_map(E::Break),
        1 => (0u8..3).prop_map(E::Continue),
    ];
    leaf.prop_recursive(5, 64, 4, |inner| {
        let stmt = prop_oneof![
            (pat(), option::of(inner.clone())).prop_map(|(p, i)| S::Let(p, i)),
            (pat(), option::of(inner.clone())).prop_map(|(p, i)| S::Let(p, i)),
            inner.clone().prop_map(S::Expr),
            (vec(pat(), 0..2), inner.clone()).prop_map(|(ps, b)| S::Fn(ps, b)),
        ];
        prop_oneof![
            (inner.clone(), inner.clone()).prop_map(|(a, b)| E::Add(Box::new(a), Box::new(b))),
            (vec(stmt, 0..4), option::of(inner.clone()))
                .prop_map(|(s, t)| E::Block(s, t.map(Box::new))),
            (inner.clone(), inner.clone(), option::of(inner.clone())).prop_map(|(c, t, e)| E::If(
                Box::new(c),
                Box::new(t),
                e.map(Box::new)
            )),
            (
                inner.clone(),
                vec((pat(), option::of(inner.clone()), inner.clone()), 1..3)
            )
                .prop_map(|(s, arms)| E::Match(Box::new(s), arms)),
            (any::<bool>(), inner.clone()).prop_map(|(l, b)| E::Loop(l, Box::new(b))),
            (any::<bool>(), vec(pat(), 0..2), inner.clone()).prop_map(|(i, ps, b)| E::Closure(
                i,
                ps,
                Box::new(b)
            )),
            (inner.clone(), vec(inner, 0..2)).prop_map(|(c, a)| E::Call(Box::new(c), a)),
        ]
    })
}

/// Numbers every `Bind` in recipe order and records whether its site is a
/// parameter. Returns the binder count.
fn number(e: &mut E, kinds: &mut Vec<BinderKind>) {
    fn pat(p: &mut P, kind: BinderKind, kinds: &mut Vec<BinderKind>) {
        match p {
            P::Bind(id) => {
                *id = u32::try_from(kinds.len()).unwrap();
                kinds.push(kind);
            }
            P::Tuple(ps) => ps.iter_mut().for_each(|p| pat(p, kind, kinds)),
            P::Or(p) => pat(p, kind, kinds),
            P::Wild | P::Lit(_) => {}
        }
    }
    match e {
        E::Lit(_) | E::Var(_) | E::Break(_) | E::Continue(_) => {}
        E::Add(a, b) => {
            number(a, kinds);
            number(b, kinds);
        }
        E::Block(stmts, tail) => {
            for s in stmts {
                match s {
                    S::Let(p, init) => {
                        pat(p, BinderKind::Local, kinds);
                        if let Some(i) = init {
                            number(i, kinds);
                        }
                    }
                    S::Expr(e) => number(e, kinds),
                    S::Fn(ps, body) => {
                        ps.iter_mut().for_each(|p| pat(p, BinderKind::Param, kinds));
                        number(body, kinds);
                    }
                }
            }
            if let Some(t) = tail {
                number(t, kinds);
            }
        }
        E::If(c, t, e) => {
            number(c, kinds);
            number(t, kinds);
            if let Some(e) = e {
                number(e, kinds);
            }
        }
        E::Match(s, arms) => {
            number(s, kinds);
            for (p, g, b) in arms {
                pat(p, BinderKind::Local, kinds);
                if let Some(g) = g {
                    number(g, kinds);
                }
                number(b, kinds);
            }
        }
        E::Loop(_, b) => number(b, kinds),
        E::Closure(_, ps, b) => {
            ps.iter_mut().for_each(|p| pat(p, BinderKind::Param, kinds));
            number(b, kinds);
        }
        E::Call(c, args) => {
            number(c, kinds);
            args.iter_mut().for_each(|a| number(a, kinds));
        }
    }
}

// ------------------------------------------------- lowering + reference

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expect {
    OutOfScope,
    NotCapturable,
}

#[derive(Clone, Copy)]
struct Frame {
    /// A closure that allows implicit captures.
    implicit_closure: bool,
    loop_base: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mutation {
    None,
    Share,
    Orphan,
    Dangling,
    Unbound,
    Policy,
    Arity,
    KindMismatch,
    BreakOutside,
    UseBeforeLet,
}

struct Low {
    b: Builder,
    names: Interner,
    x: Symbol,
    binders: Vec<BinderId>,
    /// Reference state: visible binders with the frame depth they belong to.
    env: Vec<(usize, usize)>,
    frames: Vec<Frame>,
    /// Enclosing loops: (label binder, frame depth).
    loops: Vec<Option<BinderId>>,
    jump_errors: Vec<(ExprId, JumpProblem)>,
    scope_errors: Vec<(PathId, Expect)>,
    /// When set, references pick only binders the reference says are legal.
    valid_refs: bool,
    span: u32,
}

impl Low {
    fn new(kinds: &[BinderKind], valid_refs: bool) -> Self {
        let mut names = Interner::new();
        let x = names.intern("x");
        let mut b = Builder::new();
        let binders = kinds
            .iter()
            .map(|k| b.binder(Binder::new(Name::new(x), *k)))
            .collect();
        Self {
            b,
            names,
            x,
            binders,
            env: Vec::new(),
            frames: Vec::new(),
            loops: Vec::new(),
            jump_errors: Vec::new(),
            scope_errors: Vec::new(),
            valid_refs,
            span: 0,
        }
    }

    /// Gives every node created next a fresh one-byte span.
    fn tick(&mut self) {
        self.span += 1;
        self.b.set_span(Span::new(self.span, self.span + 1));
    }

    fn reference_check(&self, binder_index: usize) -> Option<Expect> {
        let Some(&(_, depth)) = self.env.iter().rev().find(|(b, _)| *b == binder_index) else {
            return Some(Expect::OutOfScope);
        };
        let crossed = &self.frames[depth..];
        if crossed.iter().all(|f| f.implicit_closure) {
            None
        } else {
            Some(Expect::NotCapturable)
        }
    }

    fn pat(&mut self, p: &P, bound: &mut Vec<usize>) -> PatId {
        self.tick();
        match p {
            P::Wild => self.b.pat(Pat::Wild),
            P::Bind(id) => {
                let i = *id as usize;
                if !bound.contains(&i) {
                    bound.push(i);
                }
                self.b.bind(self.binders[i])
            }
            P::Lit(v) => self
                .b
                .pat(Pat::Lit(Lit::Int(hir_lang::IntLit::signed((*v).into())))),
            P::Tuple(ps) => {
                let ids: Vec<PatId> = ps.iter().map(|p| self.pat(p, bound)).collect();
                let elems = self.b.list(&ids);
                self.b.pat(Pat::Tuple { elems, rest: None })
            }
            P::Or(inner) => {
                let a = self.pat(inner, bound);
                let b = self.pat(inner, bound);
                let alts = self.b.list(&[a, b]);
                self.b.pat(Pat::Or(alts))
            }
        }
    }

    fn activate(&mut self, bound: &[usize]) {
        let depth = self.frames.len();
        self.env.extend(bound.iter().map(|b| (*b, depth)));
    }

    fn var(&mut self, sel: u16) -> ExprId {
        self.tick();
        if self.binders.is_empty() {
            return self.b.int(sel.into());
        }
        let index = if self.valid_refs {
            let legal: Vec<usize> = (0..self.binders.len())
                .filter(|b| self.reference_check(*b).is_none())
                .collect();
            if legal.is_empty() {
                return self.b.int(sel.into());
            }
            legal[sel as usize % legal.len()]
        } else if sel % 2 == 0 && !self.env.is_empty() {
            // Half the references name a lexically visible binder (possibly
            // across a frame), so frame errors are well represented.
            self.env[(sel as usize / 2) % self.env.len()].0
        } else {
            sel as usize % self.binders.len()
        };
        let path = self.b.resolved_path(
            Name::new(self.x),
            Ns::Value,
            Res::Local(self.binders[index]),
        );
        if let Some(problem) = self.reference_check(index) {
            self.scope_errors.push((path, problem));
        }
        self.b.expr(Expr::Path(path))
    }

    fn jump(&mut self, sel: u8, is_continue: bool) -> ExprId {
        self.tick();
        // sel 0: unlabeled; sel k: the k-th labeled loop outward, if any.
        let labeled: Vec<(usize, BinderId)> = self
            .loops
            .iter()
            .enumerate()
            .rev()
            .filter_map(|(i, l)| l.map(|b| (i, b)))
            .collect();
        let target = (sel > 0)
            .then(|| labeled.get(sel as usize - 1).copied())
            .flatten();
        let base = self.frames.last().map_or(0, |f| f.loop_base);
        let (label, problem) = match target {
            Some((at, b)) => (Some(b), (at < base).then_some(JumpProblem::LabelNotInScope)),
            None => {
                let ok = self.loops.len() > base;
                let p = if is_continue {
                    JumpProblem::ContinueOutsideLoop
                } else {
                    JumpProblem::BreakOutsideLoop
                };
                (None, (!ok).then_some(p))
            }
        };
        if problem.is_some() && sel != 2 {
            // Most stray jumps become literals so that jump errors, which the
            // validator reports first, do not mask the scope errors.
            return self.b.int(0);
        }
        let e = if is_continue {
            self.b.expr(Expr::Continue { label })
        } else {
            self.b.expr(Expr::Break { label, value: None })
        };
        if let Some(p) = problem {
            self.jump_errors.push((e, p));
        }
        e
    }

    fn params(&mut self, ps: &[P]) -> Vec<hir_lang::ParamId> {
        let mut out = Vec::new();
        for p in ps {
            let mut bound = Vec::new();
            let pat = self.pat(p, &mut bound);
            self.tick();
            out.push(self.b.param(Param::new(pat)));
            self.activate(&bound);
        }
        out
    }

    fn expr(&mut self, e: &E) -> ExprId {
        match e {
            E::Lit(v) => {
                self.tick();
                self.b.int((*v).into())
            }
            E::Var(sel) => self.var(*sel),
            E::Add(a, b) => {
                let (a, b) = (self.expr(a), self.expr(b));
                self.tick();
                self.b.op(OpKind::Add, &[a, b])
            }
            E::Block(stmts, tail) => {
                let mark = self.env.len();
                let mut ids = Vec::new();
                for s in stmts {
                    ids.push(self.stmt(s));
                }
                let tail = tail.as_ref().map(|t| self.expr(t));
                self.env.truncate(mark);
                self.tick();
                self.b.block(&ids, tail)
            }
            E::If(c, t, e) => {
                let c = self.expr(c);
                let t = self.expr(t);
                let e = e.as_ref().map(|e| self.expr(e));
                self.tick();
                self.b.expr(Expr::If {
                    cond: c,
                    then: t,
                    else_: e,
                })
            }
            E::Match(s, arms) => {
                let scrutinee = self.expr(s);
                let mut lowered = Vec::new();
                for (p, g, body) in arms {
                    let mark = self.env.len();
                    let mut bound = Vec::new();
                    let pat = self.pat(p, &mut bound);
                    self.activate(&bound);
                    let guard = g.as_ref().map(|g| self.expr(g));
                    let body = self.expr(body);
                    self.env.truncate(mark);
                    lowered.push(Arm { pat, guard, body });
                }
                let arms = self.b.list(&lowered);
                self.tick();
                self.b.expr(Expr::Match { scrutinee, arms })
            }
            E::Loop(labeled, body) => {
                let label = labeled.then(|| {
                    self.tick();
                    self.b
                        .binder(Binder::new(Name::new(self.x), BinderKind::Label))
                });
                self.loops.push(label);
                let body = self.expr(body);
                let _ = self.loops.pop();
                self.tick();
                self.b.expr(Expr::Loop {
                    label,
                    body,
                    step: None,
                })
            }
            E::Break(sel) => self.jump(*sel, false),
            E::Continue(sel) => self.jump(*sel, true),
            E::Closure(implicit, ps, body) => {
                let mark = self.env.len();
                self.frames.push(Frame {
                    implicit_closure: *implicit,
                    loop_base: self.loops.len(),
                });
                let params = self.params(ps);
                let body = self.expr(body);
                let _ = self.frames.pop();
                self.env.truncate(mark);
                let params = self.b.list(&params);
                self.tick();
                self.b.expr(Expr::Closure(Closure {
                    params,
                    ret: None,
                    body,
                    effects: Effects::NONE,
                    implicit: implicit.then_some(CaptureMode::Infer),
                    captures: List::EMPTY,
                }))
            }
            E::Call(c, args) => {
                let c = self.expr(c);
                let args: Vec<ExprId> = args.iter().map(|a| self.expr(a)).collect();
                self.tick();
                self.b.call(c, &args)
            }
        }
    }

    fn stmt(&mut self, s: &S) -> hir_lang::StmtId {
        match s {
            S::Let(p, init) => {
                let mut bound = Vec::new();
                let pat = self.pat(p, &mut bound);
                let init = init.as_ref().map(|i| self.expr(i));
                self.activate(&bound);
                self.tick();
                self.b.let_stmt(pat, init)
            }
            S::Expr(e) => {
                let e = self.expr(e);
                self.tick();
                self.b.expr_stmt(e)
            }
            S::Fn(ps, body) => {
                let mark = self.env.len();
                self.frames.push(Frame {
                    implicit_closure: false,
                    loop_base: self.loops.len(),
                });
                let params = self.params(ps);
                let body = self.expr(body);
                let _ = self.frames.pop();
                self.env.truncate(mark);
                let body = self.b.block(&[], Some(body));
                let params = self.b.list(&params);
                self.tick();
                let f = self.b.item(Item::new(
                    Some(Name::new(self.x)),
                    ItemKind::Fn(FnDef {
                        params,
                        body: Some(body),
                        ..FnDef::default()
                    }),
                ));
                self.b.stmt(Stmt::Item(f))
            }
        }
    }

    /// Lowers `body` as `fn main() { body; <mutation> }` and finishes.
    fn finish(mut self, body: &E, mutation: Mutation) -> (Result<Hir, HirError>, Self2) {
        self.frames.push(Frame {
            implicit_closure: false,
            loop_base: 0,
        });
        // fn main(x, x): two parameters visible everywhere in the body, so
        // references from inside closures and nested functions often cross a
        // frame.
        let mut params = Vec::new();
        for _ in 0..2 {
            let binder = self
                .b
                .binder(Binder::new(Name::new(self.x), BinderKind::Param));
            self.binders.push(binder);
            let pat = self.b.bind(binder);
            params.push(self.b.param(Param::new(pat)));
            let index = self.binders.len() - 1;
            self.activate(&[index]);
        }
        let e = self.expr(body);
        let mut stmts = vec![self.b.expr_stmt(e)];
        self.mutate(mutation, &mut stmts);
        let _ = self.frames.pop();
        let block = self.b.block(&stmts, None);
        let main = self.b.func(Name::new(self.x), &params, block);
        let root = self.b.module(None, &[main]);
        let rest = Self2 {
            names: self.names,
            jump_errors: self.jump_errors,
            scope_errors: self.scope_errors,
            span: self.span,
        };
        (self.b.finish(root), rest)
    }

    fn mutate(&mut self, mutation: Mutation, stmts: &mut Vec<hir_lang::StmtId>) {
        match mutation {
            Mutation::None => {}
            Mutation::Share => {
                let one = self.b.int(1);
                let elems = self.b.list(&[one, one]);
                let t = self.b.expr(Expr::Tuple(elems));
                stmts.push(self.b.expr_stmt(t));
            }
            Mutation::Orphan => {
                let _ = self.b.int(1);
            }
            Mutation::Dangling => {
                let ghost = ExprId::from_index(1 << 30).unwrap();
                let neg = self.b.op(OpKind::Neg, &[ghost]);
                stmts.push(self.b.expr_stmt(neg));
            }
            Mutation::Unbound => {
                let _ = self
                    .b
                    .binder(Binder::new(Name::new(self.x), BinderKind::Local));
            }
            Mutation::Policy => {
                let (a, c) = (self.b.int(1), self.b.int(2));
                let bad = self.b.op_with(
                    Op {
                        kind: OpKind::Add,
                        policy: Policy::NONE,
                    },
                    &[a, c],
                );
                stmts.push(self.b.expr_stmt(bad));
            }
            Mutation::Arity => {
                let a = self.b.int(1);
                let bad = self.b.op(OpKind::Mul, &[a]);
                stmts.push(self.b.expr_stmt(bad));
            }
            Mutation::KindMismatch => {
                let p = self
                    .b
                    .binder(Binder::new(Name::new(self.x), BinderKind::Param));
                let pat = self.b.bind(p);
                let one = self.b.int(1);
                stmts.push(self.b.let_stmt(pat, Some(one)));
            }
            Mutation::BreakOutside => {
                let br = self.b.expr(Expr::Break {
                    label: None,
                    value: None,
                });
                stmts.push(self.b.expr_stmt(br));
            }
            Mutation::UseBeforeLet => {
                let late = self
                    .b
                    .binder(Binder::new(Name::new(self.x), BinderKind::Local));
                let early = self.b.use_binder(late);
                stmts.push(self.b.expr_stmt(early));
                let pat = self.b.bind(late);
                let one = self.b.int(1);
                stmts.push(self.b.let_stmt(pat, Some(one)));
            }
        }
    }
}

/// What lowering hands back besides the HIR.
struct Self2 {
    names: Interner,
    jump_errors: Vec<(ExprId, JumpProblem)>,
    scope_errors: Vec<(PathId, Expect)>,
    span: u32,
}

fn build(recipe: &E, valid_refs: bool, mutation: Mutation) -> (Result<Hir, HirError>, Self2) {
    let mut recipe = recipe.clone();
    let mut kinds = Vec::new();
    number(&mut recipe, &mut kinds);
    Low::new(&kinds, valid_refs).finish(&recipe, mutation)
}

fn total_nodes(hir: &Hir) -> usize {
    [
        IdKind::Item,
        IdKind::Expr,
        IdKind::Stmt,
        IdKind::Pat,
        IdKind::Ty,
        IdKind::Path,
        IdKind::Field,
        IdKind::Variant,
        IdKind::Param,
    ]
    .iter()
    .map(|k| hir.count(*k))
    .sum()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1024, ..ProptestConfig::default() })]

    /// Builder output always validates (references chosen among legal binders).
    #[test]
    fn prop_builder_output_always_validates(recipe in expr()) {
        let (result, rest) = build(&recipe, true, Mutation::None);
        // Jumps are generated freely; only scope is constrained in this mode.
        if rest.jump_errors.is_empty() {
            let hir = result.unwrap();
            prop_assert_eq!(hir.validate(), Ok(()));
        } else {
            let is_jump = matches!(result, Err(HirError::Jump { .. }));
            prop_assert!(is_jump);
        }
    }

    /// The validator agrees with the reference scope/frame/jump checker on
    /// every generated program, including the exact first error.
    #[test]
    fn prop_validator_matches_reference(recipe in expr()) {
        let (result, rest) = build(&recipe, false, Mutation::None);
        let expected: Result<(), HirError> = if let Some((expr, problem)) = rest.jump_errors.first() {
            Err(HirError::Jump { expr: *expr, problem: *problem })
        } else if let Some((path, kind)) = rest.scope_errors.iter().min_by_key(|(p, _)| p.index()) {
            let binder = match &result {
                Err(HirError::OutOfScope { binder, .. } | HirError::NotCapturable { binder, .. }) => *binder,
                _ => BinderId::from_index(0).unwrap(),
            };
            Err(match kind {
                Expect::OutOfScope => HirError::OutOfScope { path: *path, binder },
                Expect::NotCapturable => HirError::NotCapturable { path: *path, binder },
            })
        } else {
            Ok(())
        };
        prop_assert_eq!(result.map(|_| ()), expected);
    }

    /// Every node and binder carries the origin current when it was created.
    #[test]
    fn prop_origins_present_on_every_node(recipe in expr()) {
        let (result, rest) = build(&recipe, true, Mutation::None);
        if let Ok(hir) = result {
            let mut nodes = Vec::new();
            hir_lang::walk(&hir, |n| nodes.push(n));
            prop_assert_eq!(nodes.len(), total_nodes(&hir));
            for node in nodes {
                let o = hir.origin(node);
                prop_assert!(o.span.end().to_u32() <= rest.span + 1);
                prop_assert!(o.expn.is_root());
            }
            for i in 0..hir.count(IdKind::Binder) {
                let o = hir.binder_origin(BinderId::from_index(i).unwrap());
                prop_assert!(o.span.end().to_u32() <= rest.span + 1);
            }
        }
    }

    /// Construction is deterministic: the same recipe gives equal HIRs and
    /// byte-identical printed and debug forms.
    #[test]
    fn prop_construction_is_deterministic(recipe in expr()) {
        let (a, na) = build(&recipe, true, Mutation::None);
        let (b, nb) = build(&recipe, true, Mutation::None);
        prop_assert_eq!(&a, &b);
        if let (Ok(a), Ok(b)) = (a, b) {
            prop_assert_eq!(hir_lang::print(&a, &na.names), hir_lang::print(&b, &nb.names));
            prop_assert_eq!(format!("{a:?}"), format!("{b:?}"));
            prop_assert_eq!(a.clone(), b);
        }
    }

    /// The walker visits each node once, in the order `children_into` lists.
    #[test]
    fn prop_walk_order_matches_children(recipe in expr()) {
        let (result, _) = build(&recipe, true, Mutation::None);
        if let Ok(hir) = result {
            let mut walked = Vec::new();
            hir_lang::walk(&hir, |n| walked.push(n));
            // Rebuild preorder from `children_into` with an explicit stack.
            let mut rebuilt = Vec::new();
            let mut stack = vec![NodeRef::Item(hir.root())];
            let mut kids = Vec::new();
            while let Some(n) = stack.pop() {
                rebuilt.push(n);
                kids.clear();
                hir.children_into(n, &mut kids);
                stack.extend(kids.iter().rev());
            }
            prop_assert_eq!(walked, rebuilt);
        }
    }

    /// Each targeted corruption of a valid program is rejected with the
    /// matching error, never a panic.
    #[test]
    fn prop_mutated_programs_are_rejected(
        recipe in expr(),
        which in 1usize..10,
    ) {
        let mutation = [
            Mutation::None,
            Mutation::Share,
            Mutation::Orphan,
            Mutation::Dangling,
            Mutation::Unbound,
            Mutation::Policy,
            Mutation::Arity,
            Mutation::KindMismatch,
            Mutation::BreakOutside,
            Mutation::UseBeforeLet,
        ][which];
        let (result, rest) = build(&recipe, true, mutation);
        let err = result.unwrap_err();
        let ok = match mutation {
            Mutation::Share => matches!(err, HirError::SharedNode { .. }),
            Mutation::Orphan => matches!(err, HirError::Unreachable { .. }),
            Mutation::Dangling => matches!(err, HirError::Dangling { .. }),
            Mutation::Unbound => matches!(err, HirError::BinderNotBound { .. }),
            Mutation::Policy => matches!(err, HirError::Policy { .. }),
            Mutation::Arity => matches!(err, HirError::Arity { .. }),
            Mutation::KindMismatch => matches!(err, HirError::BinderKind { .. }),
            Mutation::BreakOutside => matches!(err, HirError::Jump { .. }),
            Mutation::UseBeforeLet => matches!(err, HirError::OutOfScope { .. }),
            Mutation::None => true,
        };
        // A generated program may already contain a stray jump, which is
        // reported first when the mutation is found in a later pass.
        let preexisting_jump = !rest.jump_errors.is_empty() && matches!(err, HirError::Jump { .. });
        prop_assert!(ok || preexisting_jump, "{mutation:?} gave {err:?}");
    }

    /// `resolve` keeps a HIR valid: any sequence of accepted resolutions
    /// leaves a HIR the validator still accepts, and rejected ones change
    /// nothing.
    #[test]
    fn prop_resolve_preserves_validity(
        recipe in expr(),
        picks in vec((any::<u16>(), any::<u16>(), 0u8..5), 0..24),
    ) {
        let (result, _) = build(&recipe, true, Mutation::None);
        if let Ok(mut hir) = result {
            let paths = hir.count(IdKind::Path);
            let binders = hir.count(IdKind::Binder);
            for (p, t, kind) in picks {
                if paths == 0 {
                    break;
                }
                let path = PathId::from_index(p as usize % paths).unwrap();
                let res = match kind {
                    0 if binders > 0 => Res::Local(BinderId::from_index(t as usize % binders).unwrap()),
                    1 => Res::Item(ItemId::from_index(t as usize % hir.count(IdKind::Item)).unwrap()),
                    2 => Res::Err,
                    3 => Res::Prim(hir_lang::Prim::I8),
                    _ => Res::Unresolved,
                };
                let before = hir.path(path).res;
                match hir.resolve(path, res) {
                    Ok(()) => prop_assert_eq!(hir.path(path).res, res),
                    Err(_) => prop_assert_eq!(hir.path(path).res, before),
                }
            }
            prop_assert_eq!(hir.validate(), Ok(()));
        }
    }
}

// ------------------------------------------------------- garbage arenas

/// One raw construction step with arbitrary small ids, valid or not.
#[derive(Clone, Debug)]
struct RawStep {
    kind: u8,
    a: u8,
    b: u8,
    c: u8,
}

fn raw_steps() -> impl Strategy<Value = Vec<RawStep>> {
    vec(
        (any::<u8>(), any::<u8>(), any::<u8>(), any::<u8>()).prop_map(|(kind, a, b, c)| RawStep {
            kind,
            a,
            b,
            c,
        }),
        0..48,
    )
}

fn apply_raw(b: &mut Builder, sym: Symbol, step: &RawStep) {
    let n = |x: u8| (x % 10) as usize;
    let e = |x: u8| ExprId::from_index(n(x)).unwrap();
    let p = |x: u8| PatId::from_index(n(x)).unwrap();
    let bi = |x: u8| BinderId::from_index(n(x)).unwrap();
    let it = |x: u8| ItemId::from_index(n(x)).unwrap();
    let pa = |x: u8| PathId::from_index(n(x)).unwrap();
    b.set_span(Span::new(u32::from(step.a), u32::from(step.b)));
    if step.c % 7 == 0 {
        b.set_expansion(ExpnId::from_u32(u32::from(step.c % 3)));
    }
    match step.kind % 32 {
        0 => {
            let _ = b.int(i64::from(step.a));
        }
        1 => {
            let l = b.list(&[e(step.a), e(step.b)]);
            let _ = b.expr(Expr::Tuple(l));
        }
        2 => {
            let s = hir_lang::StmtId::from_index(n(step.a)).unwrap();
            let l = b.list(&[s]);
            let _ = b.expr(Expr::Block(hir_lang::Block {
                stmts: l,
                tail: (step.c % 2 == 0).then(|| e(step.b)),
                label: (step.c % 5 == 0).then(|| bi(step.c)),
                is_unsafe: false,
            }));
        }
        3 => {
            let _ = b.expr(Expr::If {
                cond: e(step.a),
                then: e(step.b),
                else_: Some(e(step.c)),
            });
        }
        4 => {
            let arms = b.list(&[Arm {
                pat: p(step.b),
                guard: None,
                body: e(step.c),
            }]);
            let _ = b.expr(Expr::Match {
                scrutinee: e(step.a),
                arms,
            });
        }
        5 => {
            let _ = b.expr(Expr::Loop {
                label: (step.c % 2 == 0).then(|| bi(step.a)),
                body: e(step.b),
                step: None,
            });
        }
        6 => {
            let _ = b.expr(Expr::Break {
                label: (step.c % 2 == 0).then(|| bi(step.a)),
                value: None,
            });
        }
        7 => {
            let params = b.list(&[hir_lang::ParamId::from_index(n(step.a)).unwrap()]);
            let caps = if step.c % 3 == 0 {
                b.list(&[hir_lang::Capture {
                    outer: pa(step.c),
                    binder: bi(step.b),
                    mode: CaptureMode::ByValue,
                }])
            } else {
                List::EMPTY
            };
            let _ = b.expr(Expr::Closure(Closure {
                params,
                ret: None,
                body: e(step.b),
                effects: Effects::NONE,
                implicit: (step.c % 2 == 0).then_some(CaptureMode::Infer),
                captures: caps,
            }));
        }
        8 => {
            let _ = b.op(OpKind::Add, &[e(step.a), e(step.b)]);
        }
        9 => {
            let _ = b.expr(Expr::Assign {
                target: e(step.a),
                op: None,
                value: e(step.b),
            });
        }
        10 => {
            let _ = b.expr(Expr::Return(Some(e(step.a))));
        }
        11 => {
            let _ = b.expr(Expr::Path(pa(step.a)));
        }
        12 => {
            let _ = b.stmt(Stmt::Let {
                pat: p(step.a),
                ty: None,
                init: Some(e(step.b)),
                else_: None,
            });
        }
        13 => {
            let _ = b.stmt(Stmt::Expr(e(step.a)));
        }
        14 => {
            let _ = b.stmt(Stmt::Item(it(step.a)));
        }
        15 => {
            let _ = b.pat(Pat::Wild);
        }
        16 => {
            let _ = b.bind(bi(step.a));
        }
        17 => {
            let l = b.list(&[p(step.a), p(step.b)]);
            let _ = b.pat(Pat::Or(l));
        }
        18 => {
            let kinds = [
                BinderKind::Local,
                BinderKind::Param,
                BinderKind::Label,
                BinderKind::TypeParam,
                BinderKind::Capture,
            ];
            let _ = b.binder(Binder::new(Name::new(sym), kinds[n(step.a) % kinds.len()]));
        }
        19 => {
            let res = match step.b % 4 {
                0 => Res::Local(bi(step.c)),
                1 => Res::Item(it(step.c)),
                2 => Res::Unresolved,
                _ => Res::Err,
            };
            let ns = [Ns::Value, Ns::Type, Ns::Pattern, Ns::Import][n(step.a) % 4];
            let _ = b.resolved_path(Name::new(sym), ns, res);
        }
        20 => {
            let items = b.list(&[it(step.a)]);
            let _ = b.item(Item::new(None, ItemKind::Module { items }));
        }
        21 => {
            let params = if step.c % 2 == 0 {
                b.list(&[hir_lang::ParamId::from_index(n(step.a)).unwrap()])
            } else {
                List::EMPTY
            };
            let _ = b.item(Item::new(
                Some(Name::new(sym)),
                ItemKind::Fn(FnDef {
                    params,
                    body: Some(e(step.b)),
                    ..FnDef::default()
                }),
            ));
        }
        22 => {
            let _ = b.param(Param::new(p(step.a)));
        }
        23 => {
            let _ = b.ty(hir_lang::Ty::Array {
                elem: hir_lang::TyId::from_index(n(step.a)).unwrap(),
                len: e(step.b),
            });
        }
        24 => {
            let _ = b.ty(hir_lang::Ty::Prim(hir_lang::Prim::I32));
        }
        25 => {
            let _ = b.expansion(Expansion {
                kind: ExpnKind::Template,
                name: sym,
                call_site: Span::empty(0),
                parent: ExpnId::from_u32(u32::from(step.a % 3)),
                def_site: ExpnId::ROOT,
            });
        }
        26 => {
            let _ = b.expr(Expr::Tuple(List::from_raw(
                u32::from(step.a),
                u32::from(step.b % 4),
            )));
        }
        27 => {
            let _ = b.expr(Expr::Continue {
                label: (step.c % 2 == 0).then(|| bi(step.a)),
            });
        }
        28 => {
            let _ = b.stmt(Stmt::Defer(e(step.a)));
        }
        29 => {
            let _ = b.expr(Expr::Throw(e(step.a)));
        }
        30 => {
            let _ = b.pat(Pat::Tuple {
                elems: List::from_raw(u32::from(step.a % 4), u32::from(step.b % 3)),
                rest: Some(u32::from(step.c % 4)),
            });
        }
        _ => {
            let _ = b.expr(Expr::Lit(Lit::Str(hir_lang::TextRef::from_raw(
                u32::from(step.a),
                u32::from(step.b),
            ))));
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2048, ..ProptestConfig::default() })]

    /// The validator is total on arbitrary arenas: it never panics, and
    /// anything it accepts is a tree that every read operation handles.
    #[test]
    fn prop_validator_is_total_on_garbage(steps in raw_steps(), root in 0usize..10) {
        let mut names = Interner::new();
        let sym = names.intern("g");
        let mut b = Builder::new();
        for step in &steps {
            apply_raw(&mut b, sym, step);
        }
        let result = b.finish(ItemId::from_index(root).unwrap());
        if let Ok(hir) = result {
            prop_assert_eq!(hir.validate(), Ok(()));
            let mut nodes = Vec::new();
            hir_lang::walk(&hir, |n| nodes.push(n));
            prop_assert_eq!(nodes.len(), total_nodes(&hir));
            let _ = hir_lang::print(&hir, &names);
            for i in 0..hir.count(IdKind::Expr) {
                let _ = hir.implicit_captures(ExprId::from_index(i).unwrap());
            }
            hir.walk_from(NodeRef::Item(hir.root()), |_| Control::Continue);
        }
    }
}

#[test]
fn test_reference_generator_produces_both_outcomes() {
    // Guards the differential test against a generator that never produces
    // errors (or never produces valid programs).
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;
    let mut runner = TestRunner::deterministic();
    let (mut ok, mut out_of_scope, mut capture, mut jump) = (0, 0, 0, 0);
    for _ in 0..1000 {
        let recipe = expr().new_tree(&mut runner).unwrap().current();
        let (result, _) = build(&recipe, false, Mutation::None);
        match result {
            Ok(_) => ok += 1,
            Err(HirError::OutOfScope { .. }) => out_of_scope += 1,
            Err(HirError::NotCapturable { .. }) => capture += 1,
            Err(HirError::Jump { .. }) => jump += 1,
            Err(other) => panic!("unexpected {other:?}"),
        }
    }
    assert!(
        ok > 50 && out_of_scope > 50 && capture > 20 && jump > 50,
        "ok {ok}, out of scope {out_of_scope}, not capturable {capture}, jump {jump}"
    );
}
