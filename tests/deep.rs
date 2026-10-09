//! Arbitrarily deep HIR builds, validates, walks, prints, clones, compares, and
//! drops without recursion. Each test builds a chain of about a million nodes
//! and runs on the default 2 MiB test-thread stack, which recursion over a
//! chain this deep would overflow.

use hir_lang::{
    BindMode, Binder, BinderKind, Builder, CaptureMode, Closure, Control, Effects, Expr, Hir,
    IdKind, List, Name, NodeRef, OpKind, Pat, Prim, Ty,
};
use intern_lang::Interner;

const DEEP: usize = 1_000_000;

fn wrap_in_fn(b: &mut Builder, names: &mut Interner, body: hir_lang::ExprId) -> hir_lang::ItemId {
    let block = b.block(&[], Some(body));
    let f = b.func(Name::new(names.intern("deep")), &[], block);
    b.module(None, &[f])
}

/// Exercises every whole-HIR operation that could recurse.
fn exercise(hir: &Hir, names: &Interner, expected_nodes: usize) {
    let mut count = 0usize;
    hir_lang::walk(hir, |_| count += 1);
    assert_eq!(count, expected_nodes);

    let mut max_depth = 0usize;
    let mut depth = 0usize;
    hir.walk_from(NodeRef::Item(hir.root()), |event| {
        match event {
            hir_lang::Event::Enter(_) => {
                depth += 1;
                max_depth = max_depth.max(depth);
            }
            hir_lang::Event::Leave(_) => depth -= 1,
            _ => {}
        }
        Control::Continue
    });
    assert!(max_depth > DEEP / 2);

    let text = hir_lang::print(hir, names);
    // Indentation is capped, so the text stays linear in the node count.
    assert!(text.len() < expected_nodes * 200);

    let copy = hir.clone();
    assert!(copy == *hir);
    let debug = format!("{copy:?}");
    assert!(!debug.is_empty());
    drop(copy);
}

#[test]
fn test_deep_op_chain_validates_and_traverses() {
    let mut names = Interner::new();
    let mut b = Builder::new();
    let mut e = b.int(1);
    for _ in 0..DEEP {
        e = b.op(OpKind::Neg, &[e]);
    }
    let root = wrap_in_fn(&mut b, &mut names, e);
    let hir = b.finish(root).unwrap();
    // module, fn, block, the literal, and the chain.
    exercise(&hir, &names, DEEP + 4);
}

#[test]
fn test_deep_block_nesting_validates_and_traverses() {
    let mut names = Interner::new();
    let mut b = Builder::new();
    let mut e = b.int(0);
    for _ in 0..DEEP {
        e = b.block(&[], Some(e));
    }
    let root = wrap_in_fn(&mut b, &mut names, e);
    let hir = b.finish(root).unwrap();
    exercise(&hir, &names, DEEP + 4);
}

#[test]
fn test_deep_nested_loops_with_break_to_outermost_label() {
    let mut names = Interner::new();
    let mut b = Builder::new();
    let outer = b.binder(Binder::new(
        Name::new(names.intern("outer")),
        BinderKind::Label,
    ));
    let mut e = b.expr(Expr::Break {
        label: Some(outer),
        value: None,
    });
    let depth = DEEP / 2;
    for _ in 0..depth {
        e = b.expr(Expr::Loop {
            label: None,
            body: e,
            step: None,
        });
    }
    let e = b.expr(Expr::Loop {
        label: Some(outer),
        body: e,
        step: None,
    });
    let root = wrap_in_fn(&mut b, &mut names, e);
    let hir = b.finish(root).unwrap();
    exercise(&hir, &names, depth + 5);
}

#[test]
fn test_deep_let_chain_resolves_every_reference() {
    // { let x0 = 0; let x1 = x0; ...; x_n }: a million binders in one block,
    // each referenced by the next, all checked for scope.
    let mut names = Interner::new();
    let x = names.intern("x");
    let mut b = Builder::new();
    let n = DEEP / 4;
    let mut stmts = Vec::with_capacity(n);
    let first = b.binder(Binder::new(Name::new(x), BinderKind::Local));
    let pat = b.bind(first);
    let zero = b.int(0);
    stmts.push(b.let_stmt(pat, Some(zero)));
    let mut prev = first;
    for _ in 1..n {
        let next = b.binder(Binder::new(Name::new(x), BinderKind::Local));
        let init = b.use_binder(prev);
        let pat = b.bind(next);
        stmts.push(b.let_stmt(pat, Some(init)));
        prev = next;
    }
    let tail = b.use_binder(prev);
    let body = b.block(&stmts, Some(tail));
    let f = b.func(Name::new(names.intern("chain")), &[], body);
    let root = b.module(None, &[f]);
    let hir = b.finish(root).unwrap();
    assert_eq!(hir.count(IdKind::Binder), n);
    let mut count = 0usize;
    hir_lang::walk(&hir, |_| count += 1);
    // module, fn, block, tail use + path; per let: stmt, pattern, init, and a
    // path for every initializer but the first (a literal).
    assert_eq!(count, 5 + n * 3 + (n - 1));
}

#[test]
fn test_deep_pattern_and_type_nesting() {
    let mut names = Interner::new();
    let mut b = Builder::new();
    let x = b.binder(Binder::new(Name::new(names.intern("x")), BinderKind::Local));
    let mut p = b.pat(Pat::Bind {
        binder: x,
        mode: BindMode::Value,
        sub: None,
    });
    let depth = DEEP / 2;
    for _ in 0..depth {
        p = b.pat(Pat::Ref {
            mutable: false,
            inner: p,
        });
    }
    let mut t = b.ty(Ty::Prim(Prim::I32));
    for _ in 0..depth {
        t = b.ty(Ty::Ref {
            mutable: false,
            region: None,
            inner: t,
        });
    }
    let init = b.int(1);
    let stmt = b.stmt(hir_lang::Stmt::Let {
        pat: p,
        ty: Some(t),
        init: Some(init),
        else_: None,
    });
    let use_x = b.use_binder(x);
    let body = b.block(&[stmt], Some(use_x));
    let f = b.func(Name::new(names.intern("f")), &[], body);
    let root = b.module(None, &[f]);
    let hir = b.finish(root).unwrap();
    exercise(&hir, &names, 2 * depth + 9);
}

#[test]
fn test_deep_or_pattern_nesting_checks_binder_sets() {
    // ((x | x) | x) | ... nested: every level re-checks the binder set.
    let mut names = Interner::new();
    let mut b = Builder::new();
    let x = b.binder(Binder::new(Name::new(names.intern("x")), BinderKind::Local));
    let mut p = b.bind(x);
    let depth = DEEP / 4;
    for _ in 0..depth {
        let alt = b.bind(x);
        let alts = b.list(&[p, alt]);
        p = b.pat(Pat::Or(alts));
    }
    let scrutinee = b.int(1);
    let body = b.use_binder(x);
    let arms = b.list(&[hir_lang::Arm {
        pat: p,
        guard: None,
        body,
    }]);
    let m = b.expr(Expr::Match { scrutinee, arms });
    let root = wrap_in_fn(&mut b, &mut names, m);
    assert!(b.finish(root).is_ok());
}

#[test]
fn test_deep_closure_nesting_captures_through_every_level() {
    // fn f(v) { || || || ... v } with implicit capture at every level.
    let mut names = Interner::new();
    let mut b = Builder::new();
    let (param, v) = b.local_param(Name::new(names.intern("v")));
    let mut e = b.use_binder(v);
    let depth = DEEP / 4;
    let mut innermost = None;
    for _ in 0..depth {
        e = b.expr(Expr::Closure(Closure {
            params: List::EMPTY,
            ret: None,
            body: e,
            effects: Effects::NONE,
            implicit: Some(CaptureMode::Infer),
            captures: List::EMPTY,
            self_binder: None,
            defaults: hir_lang::DefaultEval::PerCall,
        }));
        innermost.get_or_insert(e);
    }
    let body = b.block(&[], Some(e));
    let f = b.func(Name::new(names.intern("f")), &[param], body);
    let root = b.module(None, &[f]);
    let hir = b.finish(root).unwrap();
    assert_eq!(hir.implicit_captures(e), vec![v]);
    assert_eq!(hir.implicit_captures(innermost.unwrap()), vec![v]);
}

#[test]
fn test_deep_closure_nesting_without_implicit_captures_is_rejected() {
    let mut names = Interner::new();
    let mut b = Builder::new();
    let (param, v) = b.local_param(Name::new(names.intern("v")));
    let mut e = b.use_binder(v);
    for i in 0..DEEP / 4 {
        e = b.expr(Expr::Closure(Closure {
            params: List::EMPTY,
            ret: None,
            body: e,
            effects: Effects::NONE,
            // The outermost closure forbids implicit captures.
            implicit: (i + 1 < DEEP / 4).then_some(CaptureMode::Infer),
            captures: List::EMPTY,
            self_binder: None,
            defaults: hir_lang::DefaultEval::PerCall,
        }));
    }
    let body = b.block(&[], Some(e));
    let f = b.func(Name::new(names.intern("f")), &[param], body);
    let root = b.module(None, &[f]);
    let err = b.finish(root).unwrap_err();
    assert!(matches!(err, hir_lang::HirError::NotCapturable { binder, .. } if binder == v));
}
