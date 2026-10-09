//! The 0.4 features: union and intersection types (P20), place-once
//! compound assignment and the value of assignments (P21), keyed yields,
//! `yield from`, and logical versus bitwise `not` (P22), `pow` and
//! `shift = saturate` (P23), the two-round lenient repair (P12), and
//! `lookup_local_in` (P13).

mod common;

use common::Kit;
use hir_lang::{
    Arm, BinderKind, CaptureMode, Closure, Effects, Expr, ExprId, FnDef, GenericParam, Generics,
    HirError, Item, ItemKind, Malformed, NodeRef, Ns, Op, OpKind, Overflow, Pat, Path, Prim, Res,
    Segment, Shift, Site, Stmt, Ty, TyId,
};

/// `{ let _: ty; }`
fn let_ty(k: &mut Kit, ty: TyId) -> ExprId {
    let pat = k.b.pat(Pat::Wild);
    let stmt = k.b.stmt(Stmt::Let {
        pat,
        ty: Some(ty),
        init: None,
        else_: None,
    });
    k.b.block(&[stmt], None)
}

fn named(k: &mut Kit, s: &str) -> TyId {
    let name = k.name(s);
    let p = k.b.name_path(name, Ns::Type);
    k.b.ty(Ty::Path(p))
}

fn malformed(result: Result<hir_lang::Hir, HirError>) -> Malformed {
    match result {
        Err(HirError::Malformed { problem, .. }) => problem,
        other => panic!("expected Malformed, got {other:?}"),
    }
}

// ------------------------------------------------- P20 unions, intersections

#[test]
fn test_union_is_sorted_and_deduplicated_on_finish() {
    // S | i64 | i64  becomes  i64 | S
    let mut k = Kit::new();
    let s = named(&mut k, "S");
    let i1 = k.b.ty(Ty::Prim(Prim::I64));
    let i2 = k.b.ty(Ty::Prim(Prim::I64));
    let l = k.b.list(&[s, i1, i2]);
    let u = k.b.ty(Ty::Union(l));
    let body = let_ty(&mut k, u);
    let hir = k.finish_body(body).unwrap();
    let Ty::Union(members) = *hir.ty(u) else {
        panic!("{:?}", hir.ty(u))
    };
    assert_eq!(hir.list(members), [i1, s]);
    assert_eq!(hir.ty(i2), &Ty::Err); // the repeat is a dead error node
    assert_eq!(hir.validate(), Ok(()));
}

#[test]
fn test_nested_unions_flatten_and_nullable_members_hoist() {
    // ?str | (bool | S)  becomes  ?(bool | str | S)
    let mut k = Kit::new();
    let st = k.b.ty(Ty::Prim(Prim::Str));
    let nullable = k.b.ty(Ty::Nullable(st));
    let bo = k.b.ty(Ty::Prim(Prim::Bool));
    let s = named(&mut k, "S");
    let inner_l = k.b.list(&[bo, s]);
    let inner = k.b.ty(Ty::Union(inner_l));
    let l = k.b.list(&[nullable, inner]);
    let u = k.b.ty(Ty::Union(l));
    let body = let_ty(&mut k, u);
    let (hir, names) = k.finish_body_keep(body);
    let hir = hir.unwrap();
    let Ty::Nullable(x) = *hir.ty(u) else {
        panic!("{:?}", hir.ty(u))
    };
    let Ty::Union(members) = *hir.ty(x) else {
        panic!("{:?}", hir.ty(x))
    };
    assert_eq!(hir.list(members), [bo, st, s]);
    assert!(hir_lang::print(&hir, &names).contains(
        "(nullable\n          (union\n            (prim bool)\n            (prim str)\n            (type (path S)))))"
    ));
}

#[test]
fn test_single_member_union_collapses_to_the_member() {
    let mut k = Kit::new();
    let a = k.b.ty(Ty::Prim(Prim::I64));
    let c = k.b.ty(Ty::Prim(Prim::I64));
    let l = k.b.list(&[a, c]);
    let u = k.b.ty(Ty::Union(l));
    let body = let_ty(&mut k, u);
    let hir = k.finish_body(body).unwrap();
    assert_eq!(hir.ty(u), &Ty::Prim(Prim::I64));
}

#[test]
fn test_intersections_flatten_and_may_contain_unions() {
    // A & (B & C) & (D | E)
    let mut k = Kit::new();
    let a = named(&mut k, "A");
    let b2 = named(&mut k, "B");
    let c = named(&mut k, "C");
    let d = named(&mut k, "D");
    let e = named(&mut k, "E");
    let bc_l = k.b.list(&[b2, c]);
    let bc = k.b.ty(Ty::Intersection(bc_l));
    let de_l = k.b.list(&[d, e]);
    let de = k.b.ty(Ty::Union(de_l));
    let l = k.b.list(&[a, bc, de]);
    let i = k.b.ty(Ty::Intersection(l));
    let body = let_ty(&mut k, i);
    let hir = k.finish_body(body).unwrap();
    let Ty::Intersection(members) = *hir.ty(i) else {
        panic!()
    };
    assert_eq!(hir.list(members).len(), 4);
    assert!(hir.list(members).contains(&de));
}

#[test]
fn test_resolving_a_union_member_keeps_it_canonical() {
    let mut k = Kit::new();
    let a = named(&mut k, "A");
    let b2 = named(&mut k, "B");
    let l = k.b.list(&[a, b2]);
    let u = k.b.ty(Ty::Union(l));
    let body = let_ty(&mut k, u);
    let mut hir = k.finish_body(body).unwrap();
    let Ty::Path(p) = *hir.ty(b2) else { panic!() };
    hir.resolve(p, Res::Prim(Prim::Bool)).unwrap();
    assert_eq!(hir.validate(), Ok(()));
}

// ------------------------------------------------- P21 places evaluated once

/// `fn main(a) { let_place p = a[f()] in p = concat(p, "x") }`, returning the
/// kit, the parameter, the place binder, and the let-place expression.
fn concat_assign(k: &mut Kit, place_kind: BinderKind) -> (hir_lang::ParamId, ExprId) {
    let a_name = k.name("a");
    let (pa, a) = k.b.local_param(a_name);
    let f = k.name("f");
    let f = k.b.name_expr(f);
    let call_f = k.b.call(f, &[]);
    let use_a = k.b.use_binder(a);
    let index = k.b.expr(Expr::Index {
        base: use_a,
        index: call_f,
    });
    let p = k.binder("p", place_kind);
    let target = k.b.use_binder(p);
    let read = k.b.use_binder(p);
    let concat = k.name("concat");
    let concat =
        k.b.resolved_path(concat, Ns::Value, Res::Extern(k.names.intern("mox.concat")));
    let concat = k.b.expr(Expr::Path(concat));
    let x = k.b.str_lit("x");
    let value = k.b.call(concat, &[read, x]);
    let assign = k.b.expr(Expr::Assign {
        target,
        op: None,
        value,
    });
    let lp = k.b.expr(Expr::LetPlace {
        binder: p,
        place: index,
        body: assign,
    });
    (pa, lp)
}

#[test]
fn test_let_place_evaluates_a_place_once_for_a_host_operator() {
    let mut k = Kit::new();
    let (pa, lp) = concat_assign(&mut k, BinderKind::Place);
    let body = k.b.block(&[], Some(lp));
    let f = k.func_fx("main", &[pa], Effects::NONE, body);
    let (hir, names) = k.finish_items_keep(&[f]);
    let hir = hir.unwrap();
    let text = hir_lang::print(&hir, &names);
    assert!(text.contains("(let-place p%1"), "{text}");
    assert!(
        text.contains("(assign\n          (use (path p → local p%1))"),
        "{text}"
    );
}

#[test]
fn test_let_place_binder_needs_the_place_kind() {
    let mut k = Kit::new();
    let (pa, lp) = concat_assign(&mut k, BinderKind::Local);
    let body = k.b.block(&[], Some(lp));
    let f = k.func_fx("main", &[pa], Effects::NONE, body);
    assert!(matches!(
        k.finish_items(&[f]),
        Err(HirError::BinderKind {
            expected: BinderKind::Place,
            ..
        })
    ));
}

#[test]
fn test_let_place_needs_a_readable_place() {
    for append in [false, true] {
        let mut k = Kit::new();
        let place = if append {
            let base = k.b.int(0);
            k.b.expr(Expr::Append(base))
        } else {
            k.b.int(1)
        };
        let p = k.binder("p", BinderKind::Place);
        let body = k.b.int(2);
        let lp = k.b.expr(Expr::LetPlace {
            binder: p,
            place,
            body,
        });
        assert_eq!(malformed(k.finish_body(lp)), Malformed::AssignTarget);
    }
}

#[test]
fn test_place_binder_never_crosses_a_closure() {
    let mut k = Kit::new();
    let base = k.b.int(0);
    let place = k.b.expr(Expr::Deref(base));
    let p = k.binder("p", BinderKind::Place);
    let inside = k.b.use_binder(p);
    let closure = k.b.expr(Expr::Closure(Closure {
        implicit: Some(CaptureMode::ByRef),
        ..Closure::new(inside)
    }));
    let lp = k.b.expr(Expr::LetPlace {
        binder: p,
        place,
        body: closure,
    });
    let result = k.finish_body(lp);
    assert!(
        matches!(result, Err(HirError::NotCapturable { binder, .. }) if binder == p),
        "{result:?}"
    );
}

#[test]
fn test_lookup_local_finds_a_place_binder() {
    let mut k = Kit::new();
    let base = k.b.int(0);
    let place = k.b.expr(Expr::Deref(base));
    let p_name = k.name("p");
    let p = k.b.new_binder(p_name, BinderKind::Place);
    let use_p = k.b.name_expr(p_name);
    let lp = k.b.expr(Expr::LetPlace {
        binder: p,
        place,
        body: use_p,
    });
    let hir = k.finish_body(lp).unwrap();
    let Expr::Path(path) = *hir.expr(use_p) else {
        panic!()
    };
    assert_eq!(hir.lookup_local(path, p_name), Some(p));
}

#[test]
fn test_pow_is_a_compound_operator() {
    let mut k = Kit::new();
    let x = k.binder("x", BinderKind::Local);
    let pat = k.b.bind(x);
    let one = k.b.int(1);
    let decl = k.b.let_stmt(pat, Some(one));
    let target = k.b.use_binder(x);
    let two = k.b.int(2);
    let assign = k.b.expr(Expr::Assign {
        target,
        op: Some(Op::new(OpKind::Pow)),
        value: two,
    });
    let body = k.b.block(&[decl], Some(assign));
    assert!(k.finish_body(body).is_ok());
}

// ------------------------------------------------------ P22 yields and not

#[test]
fn test_keyed_yield_and_yield_from_in_a_generator() {
    let mut k = Kit::new();
    let key = k.b.str_lit("k");
    let value = k.b.int(1);
    let y = k.b.expr(Expr::Yield {
        key: Some(key),
        value: Some(value),
    });
    let inner = k.b.int(0);
    let yf = k.b.expr(Expr::YieldFrom(inner));
    let s1 = k.b.expr_stmt(y);
    let body = k.b.block(&[s1], Some(yf));
    let f = k.func_fx("gen", &[], Effects::YIELD, body);
    let (hir, names) = k.finish_items_keep(&[f]);
    let text = hir_lang::print(&hir.unwrap(), &names);
    assert!(
        text.contains("(yield\n          (key\n            (lit \"k\"))\n          (lit 1))"),
        "{text}"
    );
    assert!(text.contains("(yield-from\n        (lit 0))"), "{text}");
}

#[test]
fn test_not_and_bit_not_are_distinct_ops() {
    assert_eq!(OpKind::Not.name(), "not");
    assert_eq!(OpKind::BitNot.name(), "bit_not");
    assert_eq!(OpKind::BitNot.arity(), 1);
    let mut k = Kit::new();
    let x = k.b.int(5);
    let e = k.b.op(OpKind::BitNot, &[x]);
    let (hir, names) = k.finish_body_keep(e);
    assert!(hir_lang::print(&hir.unwrap(), &names).contains("(op bit_not\n"));
}

// ------------------------------------------------------- P23 pow, saturate

#[test]
fn test_pow_consults_overflow_and_promote_needs_a_dynamic_result() {
    assert_eq!(OpKind::Pow.default_policy().overflow, Some(Overflow::Error));
    let pow = Op::new(OpKind::Pow).with_overflow(Overflow::Promote);
    assert!(pow.policy_matches() && pow.promotes());

    // `promote` in an array length (an integer constant context) is rejected.
    let mut k = Kit::new();
    let two = k.b.int(2);
    let three = k.b.int(3);
    let len = k.b.op_with(pow, &[two, three]);
    let elem = k.b.ty(Ty::Prim(Prim::U8));
    let arr = k.b.ty(Ty::Array { elem, len });
    let body = let_ty(&mut k, arr);
    assert_eq!(
        malformed(k.finish_body(body)),
        Malformed::PromoteOnStaticResult
    );
}

#[test]
fn test_shift_saturate_is_a_policy() {
    let mut k = Kit::new();
    let one = k.b.int(1);
    let n = k.b.int(70);
    let e =
        k.b.op_with(Op::new(OpKind::Shl).with_shift(Shift::Saturate), &[one, n]);
    let (hir, names) = k.finish_body_keep(e);
    assert!(hir_lang::print(&hir.unwrap(), &names).contains("(op shl shift=saturate"));
}

// ------------------------------------------------- P12 two-round lenient

#[test]
fn test_lenient_repairs_references_to_lost_binders_silently() {
    // match 1 { x | y => x }: one problem; the use of `x` loses its binder.
    let mut k = Kit::new();
    let x = k.binder("x", BinderKind::Local);
    let y = k.binder("y", BinderKind::Local);
    let (px, py) = (k.b.bind(x), k.b.bind(y));
    let alts = k.b.list(&[px, py]);
    let or = k.b.pat(Pat::Or(alts));
    let body = k.b.use_binder(x);
    let scrutinee = k.b.int(1);
    let arms = k.b.list(&[Arm {
        pat: or,
        guard: None,
        body,
    }]);
    let m = k.b.expr(Expr::Match { scrutinee, arms });
    let f = k.func_fx("main", &[], Effects::NONE, m);
    let root = k.b.module(None, &[f]);
    let (hir, problems) = k.b.finish_lenient(root).unwrap();
    assert_eq!(problems, [HirError::OrPatternBinders { pat: or }]);
    let Expr::Path(p) = *hir.expr(body) else {
        panic!()
    };
    assert_eq!(hir.path(p).res, Res::Err);
    assert_eq!(hir.validate(), Ok(()));
}

#[test]
fn test_lenient_reports_every_repeat_of_a_binding_and_keeps_the_first() {
    // let (x, x, x) = (1, 2, 3); x
    let mut k = Kit::new();
    let x = k.binder("x", BinderKind::Local);
    let pats = [k.b.bind(x), k.b.bind(x), k.b.bind(x)];
    let elems = k.b.list(&pats);
    let tuple = k.b.pat(Pat::Tuple { elems, rest: None });
    let vals = [k.b.int(1), k.b.int(2), k.b.int(3)];
    let vals = k.b.list(&vals);
    let init = k.b.expr(Expr::Tuple(vals));
    let decl = k.b.let_stmt(tuple, Some(init));
    let use_x = k.b.use_binder(x);
    let body = k.b.block(&[decl], Some(use_x));
    let f = k.func_fx("main", &[], Effects::NONE, body);
    let root = k.b.module(None, &[f]);
    let (hir, problems) = k.b.finish_lenient(root).unwrap();
    assert_eq!(
        problems,
        [
            HirError::DuplicateBinding {
                binder: x,
                pat: tuple
            },
            HirError::DuplicateBinding {
                binder: x,
                pat: tuple
            },
        ]
    );
    assert_eq!(hir.pat(pats[1]), &Pat::Wild);
    assert_eq!(hir.pat(pats[2]), &Pat::Wild);
    let Expr::Path(p) = *hir.expr(use_x) else {
        panic!()
    };
    assert_eq!(hir.path(p).res, Res::Local(x));
}

#[test]
fn test_an_orphaned_subtree_is_reported_once_at_its_top() {
    let build = || {
        let mut k = Kit::new();
        let one = k.b.int(1);
        let orphan = k.b.block(&[], Some(one)); // never attached
        let root = k.b.module(None, &[]);
        (k, root, orphan)
    };
    let (k, root, orphan) = build();
    assert_eq!(
        k.b.finish(root),
        Err(HirError::Unreachable {
            node: NodeRef::Expr(orphan)
        })
    );
    let (k, root, orphan) = build();
    let (_, problems) = k.b.finish_lenient(root).unwrap();
    assert_eq!(
        problems,
        [HirError::Unreachable {
            node: NodeRef::Expr(orphan)
        }]
    );
}

#[test]
fn test_lenient_reports_independent_problems_in_one_pass() {
    // A scan problem (a keyed yield without value) and two walk problems.
    let mut k = Kit::new();
    let key = k.b.int(0);
    let bad_yield = k.b.expr(Expr::Yield {
        key: Some(key),
        value: None,
    });
    let stray = k.b.expr(Expr::Break {
        label: None,
        value: None,
    });
    let value = k.b.int(1);
    let outside = k.b.expr(Expr::Yield {
        key: None,
        value: Some(value),
    });
    let s1 = k.b.expr_stmt(bad_yield);
    let s2 = k.b.expr_stmt(stray);
    let body = k.b.block(&[s1, s2], Some(outside));
    let f = k.func_fx("main", &[], Effects::NONE, body);
    let root = k.b.module(None, &[f]);
    let (hir, problems) = k.b.finish_lenient(root).unwrap();
    assert_eq!(problems.len(), 3, "{problems:?}");
    assert!(problems.contains(&HirError::Malformed {
        site: Site::Node(NodeRef::Expr(bad_yield)),
        problem: Malformed::YieldKey
    }));
    assert_eq!(hir.validate(), Ok(()));
}

// ------------------------------------------------------------ P13 lookup_in

#[test]
fn test_lookup_local_in_searches_another_namespace() {
    // fn make<T>() { T::new }
    let mut k = Kit::new();
    let t = k.name("T");
    let tp = k.b.new_binder(t, BinderKind::TypeParam);
    let params = k.b.list(&[GenericParam::new(tp)]);
    let new = k.name("new");
    let origin = k.b.origin();
    let segs =
        k.b.list(&[Segment::new(t, origin), Segment::new(new, origin)]);
    let path = k.b.path(Path::new(segs, Ns::Value));
    let use_new = k.b.expr(Expr::Path(path));
    let body = k.b.block(&[], Some(use_new));
    let make = k.name("make");
    let item = k.b.item(Item::new(
        Some(make),
        ItemKind::Fn(FnDef {
            generics: Generics {
                params,
                ..Generics::default()
            },
            body: Some(body),
            ..FnDef::default()
        }),
    ));
    let hir = k.finish_items(&[item]).unwrap();
    assert_eq!(hir.lookup_local(path, t), None);
    assert_eq!(hir.lookup_local_in(path, t, Ns::Type), Some(tp));
    assert_eq!(hir.lookup_local_in(path, t, Ns::Pattern), None);
    assert_eq!(hir.lookup_local_in(path, t, Ns::Import), None);
}
