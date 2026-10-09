//! The 0.3 features: units and definition ids, partial resolution, lenient
//! finish, scope events and `lookup_local`, identifier patterns, module
//! bodies, default evaluation, Mox places, recursive closures, subtree
//! copies, asm and intrinsics, and the tightened shape rules.

mod common;

use common::Kit;
use hir_lang::{
    Arg, ArgKind, Arm, Asm, AsmDir, AsmOperand, AsmOptions, BinderKind, Builder, CaptureMode,
    Closure, Control, Def, DefId, DefaultEval, Effects, Event, Expr, FnDef, Frame, HirError,
    IdKind, IntLit, Intrinsic, Item, ItemKind, JumpProblem, List, Lit, Malformed, MemOrder, Name,
    NodeRef, Ns, Op, OpKind, Param, Pat, Path, PathRoot, QSelf, RecordDef, Res, Segment, Stmt, Ty,
    UnitId,
};

fn malformed(result: Result<hir_lang::Hir, HirError>) -> Malformed {
    match result {
        Err(HirError::Malformed { problem, .. }) => problem,
        other => panic!("expected Malformed, got {other:?}"),
    }
}

// ------------------------------------------------------------- B1 units

#[test]
fn test_defid_from_another_builder_of_same_unit_is_rejected() {
    // Two builders for unit 0: a DefId minted by one cannot name the other's items.
    let mut other = Builder::new();
    let ghost_root = other.module(None, &[]);
    let foreign = other.def(Def::Item(ghost_root));

    let mut k = Kit::new();
    let name = k.name("f");
    let path = k.b.resolved_path(name, Ns::Value, Res::Def(foreign));
    let e = k.b.expr(Expr::Path(path));
    assert_eq!(k.finish_body(e), Err(HirError::ForeignDef { path }));
}

#[test]
fn test_defid_of_another_unit_is_accepted_and_resolve_checks_local_ones() {
    let mut k = Kit {
        names: intern_lang::Interner::new(),
        b: Builder::for_unit(UnitId::new(1)),
    };
    let other_unit = DefId::foreign(
        UnitId::new(2),
        Def::Item(hir_lang::ItemId::from_index(40).unwrap()),
    );
    let name = k.name("ext");
    let path = k.b.resolved_path(name, Ns::Value, Res::Def(other_unit));
    let e = k.b.expr(Expr::Path(path));
    let mut hir = k.finish_body(e).unwrap();
    assert_eq!(hir.unit(), UnitId::new(1));

    // A local DefId must exist; one minted by this Hir always carries its tag.
    let dangling = hir.def(Def::Item(hir_lang::ItemId::from_index(99).unwrap()));
    assert!(matches!(
        hir.resolve(path, Res::Def(dangling)),
        Err(HirError::Dangling {
            kind: IdKind::Item,
            ..
        })
    ));
    // An untagged id for this unit (a decoder's) is checked by range only.
    let main = hir.root();
    let untagged = DefId::foreign(UnitId::new(1), Def::Item(main));
    assert!(matches!(
        hir.resolve(path, Res::Def(untagged)),
        Err(HirError::Resolution { .. })
    ));
}

// ------------------------------------------------- B2 partial resolution

fn two_segments(k: &mut Kit, a: &str, b: &str) -> List<Segment> {
    let (a, b) = (k.name(a), k.name(b));
    let o = k.b.origin();
    k.b.list(&[Segment::new(a, o), Segment::new(b, o)])
}

#[test]
fn test_partial_resolution_shapes() {
    let mut k = Kit::new();
    // T::Item with T a type parameter: prefix `T`, tail by type.
    let t = k.binder("T", BinderKind::TypeParam);
    let segs = two_segments(&mut k, "T", "Item");
    let assoc = k.b.path(Path::new(segs, Ns::Type));
    let assoc_ty = k.b.ty(Ty::Path(assoc));
    // Self::Output: entirely type-relative.
    let out = k.name("Output");
    let o = k.b.origin();
    let segs = k.b.list(&[Segment::new(out, o)]);
    let self_out = k.b.path(Path {
        root: PathRoot::SelfType,
        ..Path::new(segs, Ns::Type)
    });
    let self_ty = k.b.ty(Ty::Path(self_out));
    // <T as Add>::Output
    let t2 = k.name("T");
    let tp = k.b.resolved_path(t2, Ns::Type, Res::Local(t));
    let tty = k.b.ty(Ty::Path(tp));
    let segs = two_segments(&mut k, "Add", "Output");
    let qpath = k.b.path(Path {
        qself: Some(QSelf {
            ty: tty,
            trait_len: 1,
        }),
        ..Path::new(segs, Ns::Type)
    });
    let q_ty = k.b.ty(Ty::Path(qpath));
    let tys = k.b.list(&[assoc_ty, self_ty, q_ty]);
    let tuple = k.b.ty(Ty::Tuple(tys));
    let w = k.b.pat(Pat::Wild);
    let s = k.b.stmt(Stmt::Let {
        pat: w,
        ty: Some(tuple),
        init: None,
        else_: None,
    });
    let body = k.b.block(&[s], None);
    let gp = k.b.list(&[hir_lang::GenericParam::new(t)]);
    let fname = k.name("f");
    let f = k.b.item(Item::new(
        Some(fname),
        ItemKind::Fn(FnDef {
            generics: hir_lang::Generics {
                params: gp,
                preds: List::EMPTY,
            },
            body: Some(body),
            ..FnDef::default()
        }),
    ));
    let mut hir = k.finish_items(&[f]).unwrap();

    hir.resolve_partial(assoc, Res::Local(t), 1).unwrap();
    assert_eq!(hir.path(assoc).unresolved, 1);
    // Fully resolving `T::Item` to `T` is a namespace-correct but wrong-shape
    // claim only typeck can refute; a value-only binder cannot be a prefix.
    hir.resolve_partial(self_out, Res::Unresolved, 1).unwrap();
    // More unresolved segments than segments.
    assert!(matches!(
        hir.resolve_partial(assoc, Res::Local(t), 3),
        Err(HirError::Malformed {
            problem: Malformed::PathShape,
            ..
        })
    ));
    // An empty prefix needs a type root or qualified self.
    assert!(matches!(
        hir.resolve_partial(assoc, Res::Unresolved, 2),
        Err(HirError::Malformed {
            problem: Malformed::PathShape,
            ..
        })
    ));
    // A resolved prefix cannot reach past the qualified self's trait.
    let add = hir.def(Def::Item(f));
    assert!(matches!(
        hir.resolve_partial(qpath, Res::Def(add), 0),
        Err(HirError::Malformed {
            problem: Malformed::PathShape,
            ..
        })
    ));
}

#[test]
fn test_qself_with_global_root_is_malformed() {
    let mut k = Kit::new();
    let any = k.b.ty(Ty::Any);
    let segs = two_segments(&mut k, "Tr", "X");
    let p = k.b.path(Path {
        root: PathRoot::Global,
        qself: Some(QSelf {
            ty: any,
            trait_len: 1,
        }),
        ..Path::new(segs, Ns::Type)
    });
    let t = k.b.ty(Ty::Path(p));
    let w = k.b.pat(Pat::Wild);
    let s = k.b.stmt(Stmt::Let {
        pat: w,
        ty: Some(t),
        init: None,
        else_: None,
    });
    let body = k.b.block(&[s], None);
    assert_eq!(malformed(k.finish_body(body)), Malformed::PathShape);
}

// ----------------------------------------------------- B3 lenient finish

#[test]
fn test_lenient_finish_reports_every_problem_in_source_order() {
    let mut k = Kit::new();
    k.b.set_span(hir_lang::Span::new(30, 35));
    let stray = k.b.expr(Expr::Break {
        label: None,
        value: None,
    });
    k.b.set_span(hir_lang::Span::new(10, 15));
    let big =
        k.b.lit(Lit::Int(IntLit::new(300).with_suffix(hir_lang::Prim::U8)));
    k.b.set_span(hir_lang::Span::new(20, 25));
    let awaited = k.b.int(1);
    let aw = k.b.expr(Expr::Await(awaited));
    k.b.set_span(hir_lang::Span::new(0, 40));
    let (s1, s2, s3) = (k.b.expr_stmt(stray), k.b.expr_stmt(big), k.b.expr_stmt(aw));
    let body = k.b.block(&[s1, s2, s3], None);
    let f = k.func_fx("main", &[], Effects::NONE, body);
    let root = k.b.module(None, &[f]);
    let (hir, problems) = k.b.finish_lenient(root).unwrap();
    assert_eq!(problems.len(), 3);
    // Sorted by span: the literal (10), the await (20), the break (30).
    assert!(matches!(
        problems[0],
        HirError::Malformed {
            problem: Malformed::LiteralOutOfRange,
            ..
        }
    ));
    assert!(matches!(problems[1], HirError::Effect { .. }));
    assert!(matches!(
        problems[2],
        HirError::Jump {
            problem: JumpProblem::BreakOutsideLoop,
            ..
        }
    ));
    assert_eq!(hir.expr(stray), &Expr::Err);
    assert_eq!(hir.expr(big), &Expr::Err);
    assert_eq!(hir.validate(), Ok(()));
}

#[test]
fn test_lenient_finish_does_not_report_cascades() {
    // A malformed let pattern is repaired; the later use of its binder becomes
    // `Res::Err` silently instead of a second, misleading problem.
    let mut k = Kit::new();
    let x = k.binder("x", BinderKind::Local);
    let px = k.b.bind(x);
    let elems = k.b.list(&[px]);
    let bad = k.b.pat(Pat::Tuple {
        elems,
        rest: Some(5),
    });
    let one = k.b.int(1);
    let s = k.b.let_stmt(bad, Some(one));
    let use_x = k.b.use_binder(x);
    let body = k.b.block(&[s], Some(use_x));
    let f = k.func_fx("f", &[], Effects::NONE, body);
    let root = k.b.module(None, &[f]);
    let (hir, problems) = k.b.finish_lenient(root).unwrap();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(matches!(
        problems[0],
        HirError::Malformed {
            problem: Malformed::RestOutOfRange,
            ..
        }
    ));
    let Expr::Path(p) = *hir.expr(use_x) else {
        panic!()
    };
    assert_eq!(hir.path(p).res, Res::Err);
    assert_eq!(hir.validate(), Ok(()));
}

#[test]
fn test_lenient_finish_fails_only_without_a_repair() {
    let mut b = Builder::new();
    let e = b.int(1);
    let f = b.func(Name::new(intern_lang::Interner::new().intern("f")), &[], e);
    assert_eq!(
        b.finish_lenient(f).map(|_| ()),
        Err(HirError::RootNotModule)
    );
    let b = Builder::new();
    assert!(matches!(
        b.finish_lenient(hir_lang::ItemId::from_index(0).unwrap()),
        Err(HirError::Dangling { .. })
    ));
}

// --------------------------------------------- H5 events, lookup_local

#[test]
fn test_walk_reports_scopes_binds_and_frames() {
    let mut k = Kit::new();
    let n = k.name("n");
    let (p, nb) = k.b.local_param(n);
    let use_n = k.b.use_binder(nb);
    let c = k.b.expr(Expr::Closure(Closure {
        implicit: Some(CaptureMode::Infer),
        ..Closure::new(use_n)
    }));
    let body = k.b.block(&[], Some(c));
    let f = k.func_fx("f", &[p], Effects::NONE, body);
    let hir = k.finish_items(&[f]).unwrap();
    let mut log = Vec::new();
    hir.walk_from(NodeRef::Item(f), |e| {
        match e {
            Event::Bind(b) => log.push(format!("bind {}", b.index())),
            Event::FrameOpen(Frame::Item(_)) => log.push("frame item".into()),
            Event::FrameOpen(Frame::Closure(_)) => log.push("frame closure".into()),
            Event::FrameClose => log.push("frame close".into()),
            Event::ScopeOpen => log.push("scope".into()),
            Event::ScopeClose => log.push("unscope".into()),
            _ => {}
        }
        Control::Continue
    });
    assert_eq!(
        log,
        [
            // fn frame and scope, `n` bound, the body block's scope, the
            // closure's frame and scope inside it, then everything closes.
            "frame item",
            "scope",
            "bind 0",
            "scope",
            "frame closure",
            "scope",
            "unscope",
            "frame close",
            "unscope",
            "unscope",
            "frame close"
        ]
    );
}

#[test]
fn test_lookup_local_follows_shadowing_namespaces_and_hygiene() {
    let mut k = Kit::new();
    let x = k.name("x");
    let outer = k.b.binder(hir_lang::Binder::new(x, BinderKind::Local));
    let po = k.b.bind(outer);
    let one = k.b.int(1);
    let so = k.b.let_stmt(po, Some(one));
    // A template's `x` (different mark) in an inner block does not shadow.
    let sym = k.sym("x");
    let tmpl = k.b.expansion(hir_lang::Expansion {
        kind: hir_lang::ExpnKind::Template,
        name: sym,
        call_site: hir_lang::Span::empty(0),
        parent: hir_lang::ExpnId::ROOT,
        def_site: hir_lang::ExpnId::ROOT,
    });
    let hidden = k.b.binder(hir_lang::Binder::new(
        Name::marked(sym, tmpl),
        BinderKind::Local,
    ));
    let ph = k.b.bind(hidden);
    let two = k.b.int(2);
    let sh = k.b.let_stmt(ph, Some(two));
    let early = k.b.name_path(x, Ns::Value);
    let early_e = k.b.expr(Expr::Path(early));
    let early_s = k.b.expr_stmt(early_e);
    let use_path = k.b.name_path(x, Ns::Value);
    let use_x = k.b.expr(Expr::Path(use_path));
    let inner = k.b.block(&[sh], Some(use_x));
    let body = k.b.block(&[early_s, so], Some(inner));
    let hir = k.finish_body(body).unwrap();
    assert_eq!(hir.lookup_local(use_path, x), Some(outer));
    assert_eq!(
        hir.lookup_local(use_path, Name::marked(sym, tmpl)),
        Some(hidden)
    );
    assert_eq!(hir.lookup_local(early, x), None, "before the let");
}

// ------------------------------------------------- H7, H8, M5, M6, M7

#[test]
fn test_ident_pattern_binds_and_resolves_either_way() {
    let mut k = Kit::new();
    let a = k.binder("A", BinderKind::Local);
    let an = k.name("A");
    let ap = k.b.name_path(an, Ns::Pattern);
    let ident = k.b.pat(Pat::Ident {
        binder: a,
        path: ap,
    });
    let body = k.b.use_binder(a);
    let scrutinee = k.b.int(0);
    let arms = k.b.list(&[Arm {
        pat: ident,
        guard: None,
        body,
    }]);
    let m = k.b.expr(Expr::Match { scrutinee, arms });
    let mut hir = k.finish_body(m).unwrap();
    // resolve-lang decides it is a binding: the path resolves to nothing.
    hir.resolve(ap, Res::Err).unwrap();
    assert_eq!(hir.validate(), Ok(()));
}

#[test]
fn test_module_body_frames() {
    // module { let x = 1; return x; fn g() { x } }  — return is allowed at top
    // level; a nested fn cannot see the script's locals.
    let mut k = Kit::new();
    let x = k.binder("x", BinderKind::Local);
    let px = k.b.bind(x);
    let one = k.b.int(1);
    let s1 = k.b.let_stmt(px, Some(one));
    let ux = k.b.use_binder(x);
    let ret = k.b.expr(Expr::Return(Some(ux)));
    let s2 = k.b.expr_stmt(ret);
    let inner_use = k.b.use_binder(x);
    let gbody = k.b.block(&[], Some(inner_use));
    let g = k.func_fx("g", &[], Effects::NONE, gbody);
    let s3 = k.b.stmt(Stmt::Item(g));
    let body = k.b.block(&[s1, s2, s3], None);
    let root = k.b.item(Item::new(
        None,
        ItemKind::Module {
            items: List::EMPTY,
            body: Some(body),
            effects: Effects::NONE,
        },
    ));
    assert!(matches!(
        k.b.finish(root),
        Err(HirError::NotCapturable { binder, .. }) if binder == x
    ));
}

#[test]
fn test_defaults_once_cannot_see_parameters_and_defaults_cannot_return() {
    let build = |defaults: DefaultEval, ret_in_default: bool| {
        let mut k = Kit::new();
        let an = k.name("a");
        let (pa, a) = k.b.local_param(an);
        let bb = k.binder("b", BinderKind::Param);
        let pb_pat = k.b.bind(bb);
        let default = if ret_in_default {
            k.b.expr(Expr::Return(None))
        } else {
            k.b.use_binder(a)
        };
        let pb = k.b.param(Param {
            default: Some(default),
            ..Param::new(pb_pat)
        });
        let body = k.b.block(&[], None);
        let params = k.b.list(&[pa, pb]);
        let fname = k.name("f");
        let f = k.b.item(Item::new(
            Some(fname),
            ItemKind::Fn(FnDef {
                params,
                body: Some(body),
                defaults,
                ..FnDef::default()
            }),
        ));
        (k.finish_items(&[f]), a, default)
    };
    let (ok, _, _) = build(DefaultEval::PerCall, false);
    assert!(ok.is_ok());
    let (once, a, _) = build(DefaultEval::Once, false);
    assert!(matches!(once, Err(HirError::NotCapturable { binder, .. }) if binder == a));
    let (ret, _, d) = build(DefaultEval::PerCall, true);
    assert!(matches!(ret, Err(HirError::Effect { expr, .. }) if expr == d));
}

#[test]
fn test_mox_place_rules() {
    // `$a[]` only as a write target.
    let mut k = Kit::new();
    let x = k.binder("a", BinderKind::Local);
    let px = k.b.bind(x);
    let s = k.b.let_stmt(px, None);
    let ux = k.b.use_binder(x);
    let append = k.b.expr(Expr::Append(ux));
    let read = k.b.expr_stmt(append);
    let body = k.b.block(&[s, read], None);
    assert_eq!(malformed(k.finish_body(body)), Malformed::AppendContext);

    // A place argument must be a place.
    let mut k = Kit::new();
    let callee = k.b.int(0);
    let value = k.b.int(1);
    let args = k.b.list(&[Arg {
        kind: ArgKind::Positional,
        value,
        place: true,
    }]);
    let call = k.b.expr(Expr::Call { callee, args });
    assert_eq!(malformed(k.finish_body(call)), Malformed::PlaceArg);

    // Reference assignment needs places on both sides.
    let mut k = Kit::new();
    let x = k.binder("a", BinderKind::Local);
    let px = k.b.bind(x);
    let s = k.b.let_stmt(px, None);
    let ux = k.b.use_binder(x);
    let lit = k.b.int(3);
    let r = k.b.expr(Expr::RefAssign {
        target: ux,
        source: lit,
    });
    let st = k.b.expr_stmt(r);
    let body = k.b.block(&[s, st], None);
    assert_eq!(malformed(k.finish_body(body)), Malformed::AssignTarget);
}

#[test]
fn test_recursive_closure_via_self_binder() {
    let mut k = Kit::new();
    let me = k.binder("go", BinderKind::Capture);
    let me_use = k.b.use_binder(me);
    let call = k.b.call(me_use, &[]);
    let c = k.b.expr(Expr::Closure(Closure {
        self_binder: Some(me),
        ..Closure::new(call)
    }));
    let hir = k.finish_body(c).unwrap();
    assert!(
        hir.implicit_captures(c).is_empty(),
        "the self binder is not a capture"
    );
}

// ------------------------------------------------------------- M11 copy

#[test]
fn test_copy_subtree_gets_fresh_binders_and_keeps_outer_references() {
    let mut k = Kit::new();
    let pn = k.name("p");
    let (param, p) = k.b.local_param(pn);
    let t = k.binder("t", BinderKind::Local);
    let pt = k.b.bind(t);
    let up = k.b.use_binder(p);
    let s = k.b.let_stmt(pt, Some(up));
    let ut = k.b.use_binder(t);
    let original = k.b.block(&[s], Some(ut));
    let NodeRef::Expr(copy) = k.b.copy_subtree(NodeRef::Expr(original)) else {
        panic!()
    };
    let pair = k.b.list(&[original, copy]);
    let both = k.b.expr(Expr::Tuple(pair));
    let body = k.b.block(&[], Some(both));
    let f = k.func_fx("f", &[param], Effects::NONE, body);
    let hir = k.finish_items(&[f]).unwrap();
    assert_eq!(hir.count(IdKind::Binder), 3, "p, t, and a fresh t");
    let names = intern_lang::Interner::new();
    let text = hir_lang::print(&hir, &names);
    assert!(text.contains("$2%1") && text.contains("$2%2"), "{text}");
}

#[test]
fn test_copy_subtree_of_a_deep_chain_is_iterative() {
    let mut k = Kit::new();
    let mut e = k.b.int(1);
    for _ in 0..200_000 {
        e = k.b.op(OpKind::Neg, &[e]);
    }
    let NodeRef::Expr(copy) = k.b.copy_subtree(NodeRef::Expr(e)) else {
        panic!()
    };
    let pair = k.b.list(&[e, copy]);
    let both = k.b.expr(Expr::Tuple(pair));
    let hir = k.finish_body(both).unwrap();
    assert_eq!(hir.count(IdKind::Expr), 2 * 200_001 + 1);
}

// ---------------------------------------------- M3 asm and intrinsics

#[test]
fn test_asm_and_intrinsic_rules() {
    let asm = |template: &str, dir: AsmDir, place: bool| {
        let mut k = Kit::new();
        let x = k.binder("x", BinderKind::Local);
        let px = k.b.bind(x);
        let s = k.b.let_stmt(px, None);
        let operand = if place { k.b.use_binder(x) } else { k.b.int(1) };
        let reg = k.b.text("reg");
        let ops = k.b.list(&[AsmOperand {
            dir,
            constraint: reg,
            expr: operand,
        }]);
        let template = k.b.text(template);
        let a = k.b.expr(Expr::Asm(Asm {
            template,
            operands: ops,
            options: AsmOptions::NONE,
        }));
        let st = k.b.expr_stmt(a);
        let body = k.b.block(&[s, st], None);
        k.finish_body(body)
    };
    assert!(asm("nop {0}", AsmDir::Out, true).is_ok());
    assert_eq!(
        malformed(asm("nop {1}", AsmDir::In, true)),
        Malformed::AsmTemplate
    );
    assert_eq!(
        malformed(asm("nop {0}", AsmDir::Out, false)),
        Malformed::AsmOperand
    );

    let intrinsic = |kind: Intrinsic, n: usize| {
        let mut k = Kit::new();
        let args: Vec<_> = (0..n).map(|i| k.b.int(i as i64)).collect();
        let args = k.b.list(&args);
        let e = k.b.expr(Expr::Intrinsic {
            kind,
            generic_args: List::EMPTY,
            args,
        });
        k.finish_body(e)
    };
    assert!(intrinsic(Intrinsic::AtomicLoad(MemOrder::Acquire), 1).is_ok());
    assert_eq!(
        malformed(intrinsic(Intrinsic::AtomicLoad(MemOrder::Release), 1)),
        Malformed::MemOrder
    );
    assert_eq!(
        malformed(intrinsic(Intrinsic::Fence(MemOrder::SeqCst), 1)),
        Malformed::IntrinsicArity
    );
    let named = intern_lang::Interner::new().intern("prefetch");
    assert!(intrinsic(Intrinsic::Named(named), 3).is_ok());
}

// ------------------------------------------------------- low findings

#[test]
fn test_tightened_shape_rules() {
    // -0
    let mut k = Kit::new();
    let e = k.b.lit(Lit::Int(IntLit {
        value: 0,
        negative: true,
        suffix: None,
    }));
    assert_eq!(malformed(k.finish_body(e)), Malformed::NegativeZero);

    // compound assignment with a comparison
    let mut k = Kit::new();
    let x = k.binder("x", BinderKind::Local);
    let px = k.b.bind(x);
    let s = k.b.let_stmt(px, None);
    let ux = k.b.use_binder(x);
    let one = k.b.int(1);
    let asg = k.b.expr(Expr::Assign {
        target: ux,
        op: Some(Op::new(OpKind::Eq)),
        value: one,
    });
    let st = k.b.expr_stmt(asg);
    let body = k.b.block(&[s, st], None);
    assert_eq!(malformed(k.finish_body(body)), Malformed::CompoundAssignOp);

    // or-pattern alternatives with different binding modes
    let mut k = Kit::new();
    let x = k.binder("x", BinderKind::Local);
    let by_value = k.b.bind(x);
    let by_ref = k.b.pat(Pat::Bind {
        binder: x,
        mode: hir_lang::BindMode::Ref,
        sub: None,
    });
    let alts = k.b.list(&[by_value, by_ref]);
    let or = k.b.pat(Pat::Or(alts));
    let body = k.b.int(0);
    let scrutinee = k.b.int(1);
    let arms = k.b.list(&[Arm {
        pat: or,
        guard: None,
        body,
    }]);
    let m = k.b.expr(Expr::Match { scrutinee, arms });
    assert_eq!(malformed(k.finish_body(m)), Malformed::OrPatternModes);

    // union with tuple shape
    let mut k = Kit::new();
    let name = k.name("U");
    let u = k.b.item(Item::new(
        Some(name),
        ItemKind::Record(RecordDef {
            shape: hir_lang::Shape::Tuple,
            is_union: true,
            ..RecordDef::default()
        }),
    ));
    assert_eq!(malformed(k.finish_items(&[u])), Malformed::ShapeFields);
}

#[test]
fn test_duplicates_are_reported_at_the_first_repetition_in_source_order() {
    // Fields b, a, b, a: the first repeated member in list order is index 2.
    let mut k = Kit::new();
    let (a, b) = (k.ident("a"), k.ident("b"));
    let fields: Vec<_> = [b, a, b, a]
        .iter()
        .map(|n| k.b.field(hir_lang::FieldDef::named(*n)))
        .collect();
    let fields = k.b.list(&fields);
    let name = k.name("R");
    let r = k.b.item(Item::new(
        Some(name),
        ItemKind::Record(RecordDef {
            fields,
            ..RecordDef::default()
        }),
    ));
    match k.finish_items(&[r]) {
        Err(HirError::DuplicateName { index, name, .. }) => {
            assert_eq!(index, 2);
            assert_eq!(name, b.sym);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn test_all_implicit_captures_matches_per_closure_queries() {
    let mut k = Kit::new();
    let vn = k.name("v");
    let (p, v) = k.b.local_param(vn);
    let wn = k.name("w");
    let (pw, w) = k.b.local_param(wn);
    let uv = k.b.use_binder(v);
    let inner = k.b.expr(Expr::Closure(Closure {
        implicit: Some(CaptureMode::Infer),
        ..Closure::new(uv)
    }));
    let uw = k.b.use_binder(w);
    let pair = k.b.list(&[inner, uw]);
    let tuple = k.b.expr(Expr::Tuple(pair));
    let outer = k.b.expr(Expr::Closure(Closure {
        implicit: Some(CaptureMode::ByValue),
        ..Closure::new(tuple)
    }));
    let body = k.b.block(&[], Some(outer));
    let f = k.func_fx("f", &[p, pw], Effects::NONE, body);
    let hir = k.finish_items(&[f]).unwrap();
    let all = hir.all_implicit_captures();
    assert_eq!(all, vec![(outer, vec![v, w]), (inner, vec![v])]);
    for (c, set) in &all {
        assert_eq!(&hir.implicit_captures(*c), set);
    }
    let _ = Op::new(OpKind::Add);
}
