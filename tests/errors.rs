//! Every validation error, each produced by the smallest HIR that has it, and
//! each checked for the exact variant (and where it matters, the exact site).

mod common;

use common::Kit;
use hir_lang::{
    Arg, ArgKind, Arm, Attr, AttrArg, BinderKind, Block, Builder, Capture, CaptureMode, ClassDef,
    Closure, EffectProblem, Effects, Expansion, ExpnId, ExpnKind, Expr, FieldDef, FieldInit,
    FieldPat, FloatLit, FnDef, GenericParam, Generics, HirError, IdKind, ImplDef, IntLit,
    InterfaceDef, Item, ItemId, ItemKind, JumpProblem, List, Lit, Malformed, Name, NodeRef, Ns, Op,
    OpKind, Origin, Param, ParamKind, Pat, Path, Policy, Prim, RecordDef, Res, Shape, Site,
    SliceRest, Span, Stmt, SumDef, TextRef, Ty, Variant,
};

fn malformed(result: Result<hir_lang::Hir, HirError>) -> Malformed {
    match result {
        Err(HirError::Malformed { problem, .. }) => problem,
        other => panic!("expected Malformed, got {other:?}"),
    }
}

fn int(k: &mut Kit, v: i64) -> hir_lang::ExprId {
    k.b.int(v)
}

// ------------------------------------------------------------------ tables

#[test]
fn test_finish_dangling_root_returns_dangling_at_root() {
    let err = Builder::new()
        .finish(ItemId::from_index(0).unwrap())
        .unwrap_err();
    assert_eq!(
        err,
        HirError::Dangling {
            site: Site::Root,
            kind: IdKind::Item,
            index: 0
        }
    );
}

#[test]
fn test_finish_root_not_module_returns_root_not_module() {
    let mut k = Kit::new();
    let one = int(&mut k, 1);
    let body = k.b.block(&[], Some(one));
    let f = k.func_fx("main", &[], Effects::NONE, body);
    assert_eq!(k.b.finish(f), Err(HirError::RootNotModule));
}

#[test]
fn test_dangling_child_expr_names_parent_site() {
    let mut k = Kit::new();
    let ghost = hir_lang::ExprId::from_index(500).unwrap();
    let neg = k.b.op(OpKind::Neg, &[ghost]);
    let err = k.finish_body(neg).unwrap_err();
    assert_eq!(
        err,
        HirError::Dangling {
            site: Site::Node(NodeRef::Expr(neg)),
            kind: IdKind::Expr,
            index: 500
        }
    );
}

#[test]
fn test_dangling_binder_in_res_is_rejected() {
    let mut k = Kit::new();
    let ghost = hir_lang::BinderId::from_index(9).unwrap();
    let use_ghost = k.b.use_binder(ghost);
    assert!(matches!(
        k.finish_body(use_ghost),
        Err(HirError::Malformed {
            problem: Malformed::EmptyPath,
            ..
        }) | Err(HirError::Dangling { .. })
    ));
}

#[test]
fn test_dangling_res_item_is_rejected() {
    let mut k = Kit::new();
    let name = k.name("f");
    let path =
        k.b.resolved_path(name, Ns::Value, Res::Item(ItemId::from_index(77).unwrap()));
    let e = k.b.expr(Expr::Path(path));
    assert!(matches!(
        k.finish_body(e),
        Err(HirError::Dangling {
            kind: IdKind::Item,
            index: 77,
            ..
        })
    ));
}

#[test]
fn test_dangling_expansion_in_origin_is_rejected() {
    let mut k = Kit::new();
    k.b.set_expansion(ExpnId::from_u32(3));
    let one = int(&mut k, 1);
    k.b.set_expansion(ExpnId::ROOT);
    assert!(matches!(
        k.finish_body(one),
        Err(HirError::Dangling {
            kind: IdKind::Expansion,
            index: 3,
            ..
        })
    ));
}

#[test]
fn test_dangling_mark_in_binder_name_is_rejected() {
    let mut k = Kit::new();
    let sym = k.sym("t");
    let b = k.b.binder(hir_lang::Binder::new(
        Name::marked(sym, ExpnId::from_u32(1)),
        BinderKind::Local,
    ));
    assert!(matches!(
        k.finish_items(&[]),
        Err(HirError::Dangling {
            site: Site::Binder(id),
            kind: IdKind::Expansion,
            ..
        }) if id == b
    ));
}

#[test]
fn test_expansion_referring_to_itself_is_rejected() {
    let mut k = Kit::new();
    let name = k.sym("m");
    let e = k.b.expansion(Expansion {
        kind: ExpnKind::Macro,
        name,
        call_site: Span::empty(0),
        parent: ExpnId::from_u32(1),
        def_site: ExpnId::ROOT,
    });
    assert_eq!(
        k.finish_items(&[]),
        Err(HirError::ExpansionOrder { expn: e })
    );
}

#[test]
fn test_list_out_of_bounds_is_rejected() {
    let mut k = Kit::new();
    let t = k.b.expr(Expr::Tuple(List::from_raw(40, 2)));
    assert_eq!(
        k.finish_body(t),
        Err(HirError::ListOutOfBounds {
            site: Site::Node(NodeRef::Expr(t))
        })
    );
}

#[test]
fn test_text_out_of_bounds_is_rejected() {
    let mut k = Kit::new();
    let s = k.b.lit(Lit::Str(TextRef::from_raw(0, 99)));
    assert_eq!(
        k.finish_body(s),
        Err(HirError::TextOutOfBounds {
            site: Site::Node(NodeRef::Expr(s))
        })
    );
}

#[test]
fn test_attr_target_dangling_and_empty_arg_are_rejected() {
    let mut k = Kit::new();
    let name = k.ident("inline");
    let attr = Attr {
        name,
        args: List::EMPTY,
    };
    k.b.attach(
        NodeRef::Expr(hir_lang::ExprId::from_index(5).unwrap()),
        &[attr],
    );
    assert!(matches!(
        k.finish_items(&[]),
        Err(HirError::Dangling {
            site: Site::Attrs(_),
            ..
        })
    ));

    let mut k = Kit::new();
    let root = k.b.module(None, &[]);
    let args = k.b.list(&[AttrArg {
        key: None,
        value: None,
    }]);
    let name = k.ident("repr");
    k.b.attach(NodeRef::Item(root), &[Attr { name, args }]);
    assert_eq!(malformed(k.b.finish(root)), Malformed::EmptyAttrArg);
}

// -------------------------------------------------------------------- tree

#[test]
fn test_shared_node_is_rejected() {
    let mut k = Kit::new();
    let one = int(&mut k, 1);
    let elems = k.b.list(&[one, one]);
    let pair = k.b.expr(Expr::Tuple(elems));
    assert_eq!(
        k.finish_body(pair),
        Err(HirError::SharedNode {
            node: NodeRef::Expr(one)
        })
    );
}

#[test]
fn test_self_parent_is_rejected_as_shared() {
    let mut k = Kit::new();
    // Expression 0 lists itself as its only element: a one-node cycle.
    let first = hir_lang::ExprId::from_index(0).unwrap();
    let list = k.b.list(&[first]);
    let cyc = k.b.expr(Expr::Tuple(list));
    assert_eq!(cyc, first);
    assert_eq!(
        k.finish_body(cyc),
        Err(HirError::SharedNode {
            node: NodeRef::Expr(cyc)
        })
    );
}

#[test]
fn test_orphan_node_is_unreachable() {
    let mut k = Kit::new();
    let orphan = k.b.pat(Pat::Wild);
    assert_eq!(
        k.finish_items(&[]),
        Err(HirError::Unreachable {
            node: NodeRef::Pat(orphan)
        })
    );
}

#[test]
fn test_cycle_detached_from_root_is_unreachable() {
    let mut k = Kit::new();
    // Two blocks that contain each other, reachable from nothing.
    let a = hir_lang::ExprId::from_index(1).unwrap();
    let b0 = k.b.expr(Expr::Block(Block {
        tail: Some(a),
        ..Block::default()
    }));
    let _b1 = k.b.expr(Expr::Block(Block {
        tail: Some(b0),
        ..Block::default()
    }));
    assert!(matches!(
        k.finish_items(&[]),
        Err(HirError::Unreachable { .. })
    ));
}

// --------------------------------------------------------------- binders

#[test]
fn test_unbound_binder_is_rejected() {
    let mut k = Kit::new();
    let b = k.binder("x", BinderKind::Local);
    assert_eq!(
        k.finish_items(&[]),
        Err(HirError::BinderNotBound { binder: b })
    );
}

#[test]
fn test_binder_bound_twice_is_rejected() {
    let mut k = Kit::new();
    let x = k.binder("x", BinderKind::Local);
    let (p1, p2) = (k.b.bind(x), k.b.bind(x));
    let (i1, i2) = (int(&mut k, 1), int(&mut k, 2));
    let s1 = k.b.let_stmt(p1, Some(i1));
    let s2 = k.b.let_stmt(p2, Some(i2));
    let body = k.b.block(&[s1, s2], None);
    assert_eq!(
        k.finish_body(body),
        Err(HirError::BinderBoundTwice {
            binder: x,
            node: NodeRef::Stmt(s2)
        })
    );
}

#[test]
fn test_duplicate_binding_in_one_pattern_is_rejected() {
    let mut k = Kit::new();
    let x = k.binder("x", BinderKind::Local);
    let (p1, p2) = (k.b.bind(x), k.b.bind(x));
    let elems = k.b.list(&[p1, p2]);
    let tuple = k.b.pat(Pat::Tuple { elems, rest: None });
    let init = int(&mut k, 1);
    let s = k.b.let_stmt(tuple, Some(init));
    let body = k.b.block(&[s], None);
    assert_eq!(
        k.finish_body(body),
        Err(HirError::DuplicateBinding {
            binder: x,
            pat: tuple
        })
    );
}

#[test]
fn test_or_pattern_with_different_binders_is_rejected() {
    let mut k = Kit::new();
    let x = k.binder("x", BinderKind::Local);
    let y = k.binder("y", BinderKind::Local);
    let (px, py) = (k.b.bind(x), k.b.bind(y));
    let alts = k.b.list(&[px, py]);
    let or = k.b.pat(Pat::Or(alts));
    let scrutinee = int(&mut k, 1);
    let body = int(&mut k, 0);
    let arms = k.b.list(&[Arm {
        pat: or,
        guard: None,
        body,
    }]);
    let m = k.b.expr(Expr::Match { scrutinee, arms });
    assert_eq!(
        k.finish_body(m),
        Err(HirError::OrPatternBinders { pat: or })
    );
}

#[test]
fn test_binder_kind_mismatch_is_rejected() {
    // A `Param` binder bound by a `let`.
    let mut k = Kit::new();
    let x = k.binder("x", BinderKind::Param);
    let p = k.b.bind(x);
    let init = int(&mut k, 1);
    let s = k.b.let_stmt(p, Some(init));
    let body = k.b.block(&[s], None);
    assert_eq!(
        k.finish_body(body),
        Err(HirError::BinderKind {
            binder: x,
            expected: BinderKind::Local
        })
    );

    // A type parameter binder used as a constant parameter.
    let mut k = Kit::new();
    let t = k.binder("T", BinderKind::TypeParam);
    let usize_ty = k.b.ty(Ty::Prim(Prim::Usize));
    let params = k.b.list(&[GenericParam {
        ty: Some(usize_ty),
        ..GenericParam::new(t)
    }]);
    let body = k.b.block(&[], None);
    let name = k.name("f");
    let f = k.b.item(Item::new(
        Some(name),
        ItemKind::Fn(FnDef {
            generics: Generics {
                params,
                preds: List::EMPTY,
            },
            body: Some(body),
            ..FnDef::default()
        }),
    ));
    assert_eq!(
        k.finish_items(&[f]),
        Err(HirError::BinderKind {
            binder: t,
            expected: BinderKind::ConstParam
        })
    );
}

// ------------------------------------------------------------ resolution

#[test]
fn test_path_namespace_mismatch_is_rejected() {
    let mut k = Kit::new();
    let name = k.name("T");
    let p = k.b.name_path(name, Ns::Type);
    let e = k.b.expr(Expr::Path(p));
    assert_eq!(
        k.finish_body(e),
        Err(HirError::PathNamespace {
            path: p,
            expected: Ns::Value
        })
    );
}

#[test]
fn test_resolution_to_wrong_kind_is_rejected() {
    let mut k = Kit::new();
    let name = k.name("i32");
    let p = k.b.resolved_path(name, Ns::Value, Res::Prim(Prim::I32));
    let e = k.b.expr(Expr::Path(p));
    assert_eq!(
        k.finish_body(e),
        Err(HirError::Resolution {
            path: p,
            res: Res::Prim(Prim::I32)
        })
    );
}

#[test]
fn test_reference_before_let_is_out_of_scope() {
    let mut k = Kit::new();
    let x = k.binder("x", BinderKind::Local);
    let xname = k.name("x");
    let path = k.b.resolved_path(xname, Ns::Value, Res::Local(x));
    let early = k.b.expr(Expr::Path(path));
    let early_stmt = k.b.expr_stmt(early);
    let p = k.b.bind(x);
    let init = int(&mut k, 1);
    let s = k.b.let_stmt(p, Some(init));
    let body = k.b.block(&[early_stmt, s], None);
    assert_eq!(
        k.finish_body(body),
        Err(HirError::OutOfScope { path, binder: x })
    );
}

#[test]
fn test_reference_in_own_initializer_is_out_of_scope() {
    let mut k = Kit::new();
    let x = k.binder("x", BinderKind::Local);
    let init = k.b.use_binder(x);
    let p = k.b.bind(x);
    let s = k.b.let_stmt(p, Some(init));
    let body = k.b.block(&[s], None);
    assert!(matches!(
        k.finish_body(body),
        Err(HirError::OutOfScope { binder, .. }) if binder == x
    ));
}

#[test]
fn test_local_from_nested_item_is_not_capturable() {
    let mut k = Kit::new();
    let x = k.binder("x", BinderKind::Local);
    let p = k.b.bind(x);
    let init = int(&mut k, 1);
    let s = k.b.let_stmt(p, Some(init));
    let inner_use = k.b.use_binder(x);
    let inner_body = k.b.block(&[], Some(inner_use));
    let inner = k.func_fx("inner", &[], Effects::NONE, inner_body);
    let decl = k.b.stmt(Stmt::Item(inner));
    let body = k.b.block(&[s, decl], None);
    assert!(matches!(
        k.finish_body(body),
        Err(HirError::NotCapturable { binder, .. }) if binder == x
    ));
}

#[test]
fn test_local_from_constant_context_is_not_capturable() {
    // let n = 3; let a: [u8; n] = ...  (an array length cannot see locals)
    let mut k = Kit::new();
    let n = k.binder("n", BinderKind::Local);
    let pn = k.b.bind(n);
    let three = int(&mut k, 3);
    let s1 = k.b.let_stmt(pn, Some(three));
    let len = k.b.use_binder(n);
    let u8_ty = k.b.ty(Ty::Prim(Prim::U8));
    let arr = k.b.ty(Ty::Array { elem: u8_ty, len });
    let a = k.binder("a", BinderKind::Local);
    let pa = k.b.bind(a);
    let s2 = k.b.stmt(Stmt::Let {
        pat: pa,
        ty: Some(arr),
        init: None,
        else_: None,
    });
    let body = k.b.block(&[s1, s2], None);
    assert!(matches!(
        k.finish_body(body),
        Err(HirError::NotCapturable { binder, .. }) if binder == n
    ));
}

#[test]
fn test_closure_without_implicit_captures_cannot_see_outer_local() {
    let mut k = Kit::new();
    let vname = k.name("v");
    let (p, v) = k.b.local_param(vname);
    let use_v = k.b.use_binder(v);
    let c = k.b.expr(Expr::Closure(Closure {
        params: List::EMPTY,
        ret: None,
        body: use_v,
        effects: Effects::NONE,
        implicit: None,
        captures: List::EMPTY,
    }));
    let body = k.b.block(&[], Some(c));
    let f = k.func_fx("f", &[p], Effects::NONE, body);
    assert!(matches!(
        k.finish_items(&[f]),
        Err(HirError::NotCapturable { binder, .. }) if binder == v
    ));
}

// ------------------------------------------------------------ control flow

#[test]
fn test_jump_problems_each_reported() {
    // break outside a loop
    let mut k = Kit::new();
    let br = k.b.expr(Expr::Break {
        label: None,
        value: None,
    });
    assert_eq!(
        k.finish_body(br),
        Err(HirError::Jump {
            expr: br,
            problem: JumpProblem::BreakOutsideLoop
        })
    );

    // continue outside a loop (a labeled block is not a loop)
    let mut k = Kit::new();
    let cont = k.b.expr(Expr::Continue { label: None });
    let blk = k.b.block(&[], Some(cont));
    assert_eq!(
        k.finish_body(blk),
        Err(HirError::Jump {
            expr: cont,
            problem: JumpProblem::ContinueOutsideLoop
        })
    );

    // a label that is not an enclosing loop
    let mut k = Kit::new();
    let l = k.binder("l", BinderKind::Label);
    let br = k.b.expr(Expr::Break {
        label: Some(l),
        value: None,
    });
    let inner = k.b.expr(Expr::Loop {
        label: None,
        body: br,
        step: None,
    });
    let unit = k.b.block(&[], None);
    let labeled = k.b.expr(Expr::Loop {
        label: Some(l),
        body: unit,
        step: None,
    });
    let s1 = k.b.expr_stmt(labeled);
    let body = k.b.block(&[s1], Some(inner));
    assert_eq!(
        k.finish_body(body),
        Err(HirError::Jump {
            expr: br,
            problem: JumpProblem::LabelNotInScope
        })
    );

    // continue to a labeled block
    let mut k = Kit::new();
    let l = k.binder("l", BinderKind::Label);
    let cont = k.b.expr(Expr::Continue { label: Some(l) });
    let lp = k.b.expr(Expr::Loop {
        label: None,
        body: cont,
        step: None,
    });
    let blk = k.b.expr(Expr::Block(Block {
        tail: Some(lp),
        label: Some(l),
        ..Block::default()
    }));
    assert_eq!(
        k.finish_body(blk),
        Err(HirError::Jump {
            expr: cont,
            problem: JumpProblem::ContinueToBlock
        })
    );

    // break out of a defer
    let mut k = Kit::new();
    let br = k.b.expr(Expr::Break {
        label: None,
        value: None,
    });
    let d = k.b.stmt(Stmt::Defer(br));
    let inner = k.b.block(&[d], None);
    let lp = k.b.expr(Expr::Loop {
        label: None,
        body: inner,
        step: None,
    });
    assert_eq!(
        k.finish_body(lp),
        Err(HirError::Jump {
            expr: br,
            problem: JumpProblem::OutOfDefer
        })
    );

    // return out of a finally
    let mut k = Kit::new();
    let ret = k.b.expr(Expr::Return(None));
    let body = int(&mut k, 1);
    let t = k.b.expr(Expr::Try {
        body,
        catches: List::EMPTY,
        finally: Some(ret),
    });
    assert_eq!(
        k.finish_body(t),
        Err(HirError::Jump {
            expr: ret,
            problem: JumpProblem::OutOfDefer
        })
    );
}

#[test]
fn test_break_cannot_cross_closure_boundary() {
    let mut k = Kit::new();
    let br = k.b.expr(Expr::Break {
        label: None,
        value: None,
    });
    let c = k.b.expr(Expr::Closure(Closure {
        params: List::EMPTY,
        ret: None,
        body: br,
        effects: Effects::NONE,
        implicit: Some(CaptureMode::Infer),
        captures: List::EMPTY,
    }));
    let lp = k.b.expr(Expr::Loop {
        label: None,
        body: c,
        step: None,
    });
    assert_eq!(
        k.finish_body(lp),
        Err(HirError::Jump {
            expr: br,
            problem: JumpProblem::BreakOutsideLoop
        })
    );
}

#[test]
fn test_effect_problems_each_reported() {
    // return in a constant initializer
    let mut k = Kit::new();
    let ret = k.b.expr(Expr::Return(None));
    let name = k.name("C");
    let c = k.b.item(Item::new(
        Some(name),
        ItemKind::Const {
            ty: None,
            value: Some(ret),
        },
    ));
    assert_eq!(
        k.finish_items(&[c]),
        Err(HirError::Effect {
            expr: ret,
            problem: EffectProblem::ReturnOutsideFunction
        })
    );

    for (form, problem) in [
        ("await", EffectProblem::AwaitOutsideAsync),
        ("yield", EffectProblem::YieldOutsideGenerator),
        ("throw", EffectProblem::ThrowNotAllowed),
    ] {
        let mut k = Kit::new();
        let x = int(&mut k, 1);
        let e = match form {
            "await" => k.b.expr(Expr::Await(x)),
            "yield" => k.b.expr(Expr::Yield(Some(x))),
            _ => k.b.expr(Expr::Throw(x)),
        };
        assert_eq!(k.finish_body(e), Err(HirError::Effect { expr: e, problem }));
    }
}

#[test]
fn test_effects_allowed_in_matching_frames() {
    let mut k = Kit::new();
    let x = int(&mut k, 1);
    let a = k.b.expr(Expr::Await(x));
    let y = int(&mut k, 2);
    let yl = k.b.expr(Expr::Yield(Some(y)));
    let z = int(&mut k, 3);
    let th = k.b.expr(Expr::Throw(z));
    let s1 = k.b.expr_stmt(a);
    let s2 = k.b.expr_stmt(yl);
    let body = k.b.block(&[s1, s2], Some(th));
    let fx = Effects::ASYNC.union(Effects::YIELD).union(Effects::THROWS);
    assert!(k.finish_body_fx(body, fx).is_ok());

    // throw inside a try body needs no THROWS.
    let mut k = Kit::new();
    let z = int(&mut k, 3);
    let th = k.b.expr(Expr::Throw(z));
    let pat = k.b.pat(Pat::Wild);
    let handler = int(&mut k, 0);
    let catches = k.b.list(&[Arm {
        pat,
        guard: None,
        body: handler,
    }]);
    let t = k.b.expr(Expr::Try {
        body: th,
        catches,
        finally: None,
    });
    assert!(k.finish_body(t).is_ok());
}

// ---------------------------------------------------------------- ops

#[test]
fn test_op_arity_and_policy_errors() {
    let mut k = Kit::new();
    let one = int(&mut k, 1);
    let add = k.b.op(OpKind::Add, &[one]);
    assert_eq!(
        k.finish_body(add),
        Err(HirError::Arity {
            expr: add,
            expected: 2,
            found: 1
        })
    );

    let mut k = Kit::new();
    let (a, b) = (int(&mut k, 1), int(&mut k, 2));
    let missing = k.b.op_with(
        Op {
            kind: OpKind::Add,
            policy: Policy::NONE,
        },
        &[a, b],
    );
    assert_eq!(
        k.finish_body(missing),
        Err(HirError::Policy { expr: missing })
    );

    let mut k = Kit::new();
    let (a, b) = (int(&mut k, 1), int(&mut k, 2));
    let extra = k.b.op_with(
        Op::new(OpKind::Eq).with_overflow(hir_lang::Overflow::Wrap),
        &[a, b],
    );
    assert_eq!(k.finish_body(extra), Err(HirError::Policy { expr: extra }));

    let mut k = Kit::new();
    let a = int(&mut k, 1);
    let ty = k.b.ty(Ty::Prim(Prim::U8));
    let cast = k.b.expr(Expr::Cast {
        expr: a,
        ty,
        policy: Policy::NONE,
    });
    assert_eq!(k.finish_body(cast), Err(HirError::Policy { expr: cast }));

    let mut k = Kit::new();
    let a = int(&mut k, 1);
    let bad = k.b.op(OpKind::IntToFloat(Prim::I32), &[a]);
    assert_eq!(malformed(k.finish_body(bad)), Malformed::ConversionTarget);
}

#[test]
fn test_assignment_errors() {
    let mut k = Kit::new();
    let (a, b) = (int(&mut k, 1), int(&mut k, 2));
    let asg = k.b.expr(Expr::Assign {
        target: a,
        op: None,
        value: b,
    });
    assert_eq!(malformed(k.finish_body(asg)), Malformed::AssignTarget);

    let mut k = Kit::new();
    let x = k.binder("x", BinderKind::Local);
    let p = k.b.bind(x);
    let s = k.b.let_stmt(p, None);
    let target = k.b.use_binder(x);
    let v = int(&mut k, 2);
    let asg = k.b.expr(Expr::Assign {
        target,
        op: Some(Op::new(OpKind::Neg)),
        value: v,
    });
    let body = k.b.block(&[s], Some(asg));
    assert_eq!(malformed(k.finish_body(body)), Malformed::CompoundAssignOp);
}

// ------------------------------------------------------------ literals

#[test]
fn test_literal_errors() {
    let cases: [(Lit, Malformed); 4] = [
        (
            Lit::Int(IntLit::new(256).with_suffix(Prim::U8)),
            Malformed::LiteralOutOfRange,
        ),
        (
            Lit::Int(IntLit::new(1).with_suffix(Prim::F32)),
            Malformed::LiteralSuffix,
        ),
        (
            Lit::Float(FloatLit::new(1.0).with_suffix(Prim::I8)),
            Malformed::LiteralSuffix,
        ),
        (
            Lit::Float(FloatLit::new(0.1).with_suffix(Prim::F32)),
            Malformed::InexactF32,
        ),
    ];
    for (lit, problem) in cases {
        let mut k = Kit::new();
        let e = k.b.lit(lit);
        assert_eq!(malformed(k.finish_body(e)), problem, "{lit:?}");
    }

    let mut k = Kit::new();
    let bad = k.b.bytes(&[0xff, 0xfe]);
    let e = k.b.lit(Lit::Str(bad));
    assert_eq!(malformed(k.finish_body(e)), Malformed::InvalidUtf8);

    for digits in ["", "-", "12a", "--1", "+5"] {
        let mut k = Kit::new();
        let t = k.b.text(digits);
        let e = k.b.lit(Lit::BigInt(t));
        assert_eq!(
            malformed(k.finish_body(e)),
            Malformed::InvalidBigInt,
            "{digits:?}"
        );
    }
    let mut k = Kit::new();
    let t = k.b.text("-123456789012345678901234567890");
    let e = k.b.lit(Lit::BigInt(t));
    assert!(k.finish_body(e).is_ok());
}

// ------------------------------------------------------------ patterns

fn finish_with_pat(mut k: Kit, pat: hir_lang::PatId) -> Result<hir_lang::Hir, HirError> {
    let scrutinee = k.b.int(0);
    let body = k.b.int(1);
    let arms = k.b.list(&[Arm {
        pat,
        guard: None,
        body,
    }]);
    let m = k.b.expr(Expr::Match { scrutinee, arms });
    k.finish_body(m)
}

#[test]
fn test_pattern_shape_errors() {
    let mut k = Kit::new();
    let w = k.b.pat(Pat::Wild);
    let elems = k.b.list(&[w]);
    let p = k.b.pat(Pat::Tuple {
        elems,
        rest: Some(2),
    });
    assert_eq!(malformed(finish_with_pat(k, p)), Malformed::RestOutOfRange);

    let mut k = Kit::new();
    let p = k.b.pat(Pat::Range {
        lo: None,
        hi: None,
        inclusive: true,
    });
    assert_eq!(
        malformed(finish_with_pat(k, p)),
        Malformed::RangeWithoutBounds
    );

    let mut k = Kit::new();
    let p = k.b.pat(Pat::Range {
        lo: Some(Lit::Int(IntLit::new(1))),
        hi: Some(Lit::Char('z')),
        inclusive: true,
    });
    assert_eq!(malformed(finish_with_pat(k, p)), Malformed::RangeBoundKinds);

    let mut k = Kit::new();
    let p = k.b.pat(Pat::Range {
        lo: Some(Lit::Bool(false)),
        hi: None,
        inclusive: false,
    });
    assert_eq!(malformed(finish_with_pat(k, p)), Malformed::RangeBoundKinds);

    let mut k = Kit::new();
    let p = k.b.pat(Pat::Or(List::EMPTY));
    assert_eq!(malformed(finish_with_pat(k, p)), Malformed::EmptyOrPattern);

    let mut k = Kit::new();
    let w1 = k.b.pat(Pat::Wild);
    let name = k.ident("x");
    let fields =
        k.b.list(&[FieldPat { name, pat: w1 }, FieldPat { name, pat: w1 }]);
    let p = k.b.pat(Pat::Record {
        path: None,
        fields,
        rest: false,
    });
    assert!(matches!(
        finish_with_pat(k, p),
        Err(HirError::DuplicateName { .. })
    ));
}

#[test]
fn test_slice_pattern_with_rest_is_valid() {
    let mut k = Kit::new();
    let a = k.binder("a", BinderKind::Local);
    let r = k.binder("r", BinderKind::Local);
    let z = k.binder("z", BinderKind::Local);
    let (pa, pr, pz) = (k.b.bind(a), k.b.bind(r), k.b.bind(z));
    let prefix = k.b.list(&[pa]);
    let suffix = k.b.list(&[pz]);
    let p = k.b.pat(Pat::Slice {
        prefix,
        rest: Some(SliceRest {
            bind: Some(pr),
            suffix,
        }),
    });
    assert!(finish_with_pat(k, p).is_ok());
}

#[test]
fn test_let_else_without_init_is_rejected() {
    let mut k = Kit::new();
    let w = k.b.pat(Pat::Wild);
    let ret = k.b.expr(Expr::Return(None));
    let s = k.b.stmt(Stmt::Let {
        pat: w,
        ty: None,
        init: None,
        else_: Some(ret),
    });
    let body = k.b.block(&[s], None);
    assert_eq!(
        malformed(k.finish_body(body)),
        Malformed::LetElseWithoutInit
    );
}

#[test]
fn test_empty_path_is_rejected() {
    let mut k = Kit::new();
    let p = k.b.path(Path {
        segments: List::EMPTY,
        ns: Ns::Value,
        res: Res::Unresolved,
        global: false,
    });
    let e = k.b.expr(Expr::Path(p));
    assert_eq!(malformed(k.finish_body(e)), Malformed::EmptyPath);
}

// --------------------------------------------------------------- items

#[test]
fn test_item_name_rules() {
    let mut k = Kit::new();
    let body = k.b.block(&[], None);
    let f = k.b.item(Item::new(
        None,
        ItemKind::Fn(FnDef {
            body: Some(body),
            ..FnDef::default()
        }),
    ));
    assert_eq!(malformed(k.finish_items(&[f])), Malformed::MissingName);

    let mut k = Kit::new();
    let any = k.b.ty(Ty::Any);
    let name = k.name("I");
    let i =
        k.b.item(Item::new(Some(name), ItemKind::Impl(ImplDef::new(any))));
    assert_eq!(malformed(k.finish_items(&[i])), Malformed::UnexpectedName);

    let mut k = Kit::new();
    let seg_name = k.name("std");
    let path = k.b.name_path(seg_name, Ns::Import);
    let alias = k.name("s");
    let g = k.b.item(Item::new(
        Some(alias),
        ItemKind::Import { path, glob: true },
    ));
    assert_eq!(malformed(k.finish_items(&[g])), Malformed::UnexpectedName);
}

#[test]
fn test_item_placement_and_bodies() {
    // An associated type in a module.
    let mut k = Kit::new();
    let name = k.name("Out");
    let at = k.b.item(Item::new(
        Some(name),
        ItemKind::AssocType {
            bounds: List::EMPTY,
            default: None,
        },
    ));
    assert_eq!(malformed(k.finish_items(&[at])), Malformed::ItemPlacement);

    // A function without a body in a module.
    let mut k = Kit::new();
    let name = k.name("decl");
    let f =
        k.b.item(Item::new(Some(name), ItemKind::Fn(FnDef::default())));
    assert_eq!(malformed(k.finish_items(&[f])), Malformed::MissingBody);

    // ...is fine as a foreign declaration or in an interface.
    let mut k = Kit::new();
    let name = k.name("puts");
    let abi = Some(k.sym("C"));
    let f = k.b.item(Item::new(
        Some(name),
        ItemKind::Fn(FnDef {
            abi,
            ..FnDef::default()
        }),
    ));
    assert!(k.finish_items(&[f]).is_ok());

    // A constant without a value in a module.
    let mut k = Kit::new();
    let name = k.name("C");
    let c = k.b.item(Item::new(
        Some(name),
        ItemKind::Const {
            ty: None,
            value: None,
        },
    ));
    assert_eq!(
        malformed(k.finish_items(&[c])),
        Malformed::MissingConstValue
    );

    // A global in an interface.
    let mut k = Kit::new();
    let name = k.name("G");
    let g = k.b.item(Item::new(
        Some(name),
        ItemKind::Global {
            ty: None,
            mutable: false,
            init: None,
        },
    ));
    let items = k.b.list(&[g]);
    let iname = k.name("I");
    let i = k.b.item(Item::new(
        Some(iname),
        ItemKind::Interface(InterfaceDef {
            items,
            ..InterfaceDef::default()
        }),
    ));
    assert_eq!(malformed(k.finish_items(&[i])), Malformed::ItemPlacement);
}

#[test]
fn test_parameter_rules() {
    // Receiver outside a member.
    let mut k = Kit::new();
    let s = k.binder("self", BinderKind::Param);
    let ps = k.b.bind(s);
    let recv = k.b.param(Param {
        kind: ParamKind::Receiver,
        ..Param::new(ps)
    });
    let body = k.b.block(&[], None);
    let f = k.func_fx("f", &[recv], Effects::NONE, body);
    assert_eq!(
        malformed(k.finish_items(&[f])),
        Malformed::ReceiverPlacement
    );

    // Receiver in a closure.
    let mut k = Kit::new();
    let s = k.binder("self", BinderKind::Param);
    let ps = k.b.bind(s);
    let recv = k.b.param(Param {
        kind: ParamKind::Receiver,
        ..Param::new(ps)
    });
    let params = k.b.list(&[recv]);
    let body = int(&mut k, 0);
    let c = k.b.expr(Expr::Closure(Closure {
        params,
        ret: None,
        body,
        effects: Effects::NONE,
        implicit: None,
        captures: List::EMPTY,
    }));
    assert_eq!(malformed(k.finish_body(c)), Malformed::ReceiverPlacement);

    // Named-only before rest.
    let mut k = Kit::new();
    let (a, b) = (
        k.binder("a", BinderKind::Param),
        k.binder("b", BinderKind::Param),
    );
    let (pa, pb) = (k.b.bind(a), k.b.bind(b));
    let named = k.b.param(Param {
        kind: ParamKind::NamedOnly,
        ..Param::new(pa)
    });
    let rest = k.b.param(Param {
        kind: ParamKind::Rest,
        ..Param::new(pb)
    });
    let body = k.b.block(&[], None);
    let f = k.func_fx("f", &[named, rest], Effects::NONE, body);
    assert_eq!(malformed(k.finish_items(&[f])), Malformed::ParamOrder);

    // Two rest parameters.
    let mut k = Kit::new();
    let (a, b) = (
        k.binder("a", BinderKind::Param),
        k.binder("b", BinderKind::Param),
    );
    let (pa, pb) = (k.b.bind(a), k.b.bind(b));
    let r1 = k.b.param(Param {
        kind: ParamKind::Rest,
        ..Param::new(pa)
    });
    let r2 = k.b.param(Param {
        kind: ParamKind::Rest,
        ..Param::new(pb)
    });
    let body = k.b.block(&[], None);
    let f = k.func_fx("f", &[r1, r2], Effects::NONE, body);
    assert_eq!(malformed(k.finish_items(&[f])), Malformed::ParamOrder);

    // A rest parameter with a default.
    let mut k = Kit::new();
    let a = k.binder("a", BinderKind::Param);
    let pa = k.b.bind(a);
    let d = int(&mut k, 1);
    let r = k.b.param(Param {
        kind: ParamKind::Rest,
        default: Some(d),
        ..Param::new(pa)
    });
    let body = k.b.block(&[], None);
    let f = k.func_fx("f", &[r], Effects::NONE, body);
    assert_eq!(malformed(k.finish_items(&[f])), Malformed::ParamDefault);
}

#[test]
fn test_shape_and_duplicate_member_errors() {
    // A unit record with a field.
    let mut k = Kit::new();
    let fname = k.ident("x");
    let field = k.b.field(FieldDef::named(fname));
    let fields = k.b.list(&[field]);
    let name = k.name("R");
    let r = k.b.item(Item::new(
        Some(name),
        ItemKind::Record(RecordDef {
            shape: Shape::Unit,
            fields,
            ..RecordDef::default()
        }),
    ));
    assert_eq!(malformed(k.finish_items(&[r])), Malformed::ShapeFields);

    // Duplicate field names.
    let mut k = Kit::new();
    let fname = k.ident("x");
    let f1 = k.b.field(FieldDef::named(fname));
    let f2 = k.b.field(FieldDef::named(fname));
    let fields = k.b.list(&[f1, f2]);
    let name = k.name("R");
    let r = k.b.item(Item::new(
        Some(name),
        ItemKind::Record(RecordDef {
            fields,
            ..RecordDef::default()
        }),
    ));
    let sym = k.sym("x");
    assert_eq!(
        k.finish_items(&[r]),
        Err(HirError::DuplicateName {
            node: NodeRef::Item(r),
            name: sym
        })
    );

    // Duplicate variant names.
    let mut k = Kit::new();
    let vname = k.ident("A");
    let v1 = k.b.variant(Variant {
        name: vname,
        shape: Shape::Unit,
        fields: List::EMPTY,
        discriminant: None,
    });
    let v2 = k.b.variant(Variant {
        name: vname,
        shape: Shape::Unit,
        fields: List::EMPTY,
        discriminant: None,
    });
    let variants = k.b.list(&[v1, v2]);
    let name = k.name("S");
    let s = k.b.item(Item::new(
        Some(name),
        ItemKind::Sum(SumDef {
            variants,
            ..SumDef::default()
        }),
    ));
    assert!(matches!(
        k.finish_items(&[s]),
        Err(HirError::DuplicateName { .. })
    ));

    // A class field without a name.
    let mut k = Kit::new();
    let field = k.b.field(FieldDef::default());
    let fields = k.b.list(&[field]);
    let name = k.name("C");
    let c = k.b.item(Item::new(
        Some(name),
        ItemKind::Class(ClassDef {
            fields,
            ..ClassDef::default()
        }),
    ));
    assert_eq!(malformed(k.finish_items(&[c])), Malformed::ShapeFields);

    // Duplicate named arguments and field initializers.
    let mut k = Kit::new();
    let callee = int(&mut k, 0);
    let (v1, v2) = (int(&mut k, 1), int(&mut k, 2));
    let key = k.ident("level");
    let args = k.b.list(&[
        Arg {
            kind: ArgKind::Named(key),
            value: v1,
        },
        Arg {
            kind: ArgKind::Named(key),
            value: v2,
        },
    ]);
    let call = k.b.expr(Expr::Call { callee, args });
    assert!(matches!(
        k.finish_body(call),
        Err(HirError::DuplicateName { node: NodeRef::Expr(e), .. }) if e == call
    ));

    let mut k = Kit::new();
    let (v1, v2) = (int(&mut k, 1), int(&mut k, 2));
    let key = k.ident("x");
    let fields = k.b.list(&[
        FieldInit {
            name: key,
            value: v1,
        },
        FieldInit {
            name: key,
            value: v2,
        },
    ]);
    let rec = k.b.expr(Expr::Record {
        path: None,
        fields,
        base: None,
    });
    assert!(matches!(
        k.finish_body(rec),
        Err(HirError::DuplicateName { .. })
    ));
}

#[test]
fn test_explicit_infer_capture_is_rejected() {
    let mut k = Kit::new();
    let vname = k.name("v");
    let (p, v) = k.b.local_param(vname);
    let vname = k.name("v");
    let outer = k.b.resolved_path(vname, Ns::Value, Res::Local(v));
    let inner = k.binder("v", BinderKind::Capture);
    let captures = k.b.list(&[Capture {
        outer,
        binder: inner,
        mode: CaptureMode::Infer,
    }]);
    let body = k.b.use_binder(inner);
    let c = k.b.expr(Expr::Closure(Closure {
        params: List::EMPTY,
        ret: None,
        body,
        effects: Effects::NONE,
        implicit: None,
        captures,
    }));
    let fbody = k.b.block(&[], Some(c));
    let f = k.func_fx("f", &[p], Effects::NONE, fbody);
    assert_eq!(malformed(k.finish_items(&[f])), Malformed::InferCapture);
}

#[test]
fn test_set_origin_is_captured_by_the_root() {
    let mut k = Kit::new();
    k.b.set_origin(Origin::new(Span::new(1, 2)));
    let root = k.b.module(None, &[]);
    let hir = k.b.finish(root).unwrap();
    assert_eq!(hir.origin(NodeRef::Item(root)).span, Span::new(1, 2));
}

#[test]
fn test_every_error_displays_actionable_text() {
    let errors = [
        HirError::CapacityExceeded {
            what: hir_lang::Capacity::Pool,
        },
        HirError::RootNotModule,
        HirError::Policy {
            expr: hir_lang::ExprId::from_index(1).unwrap(),
        },
        HirError::Jump {
            expr: hir_lang::ExprId::from_index(1).unwrap(),
            problem: JumpProblem::OutOfDefer,
        },
        HirError::Effect {
            expr: hir_lang::ExprId::from_index(1).unwrap(),
            problem: EffectProblem::ThrowNotAllowed,
        },
        HirError::Malformed {
            site: Site::Root,
            problem: Malformed::ParamOrder,
        },
    ];
    for e in errors {
        let text = e.to_string();
        assert!(text.len() > 10, "{text}");
        assert!(!text.contains("failed"), "{text}");
    }
}

// ------------------------------------------------------------- promote

#[test]
fn test_promote_allowed_on_dynamic_results() {
    // PHP_INT_MAX + 1 in a dynamically typed function body.
    let mut k = Kit::new();
    let (a, b) = (int(&mut k, i64::MAX), int(&mut k, 1));
    let sum = k.b.op_with(
        Op::new(OpKind::Add).with_overflow(hir_lang::Overflow::Promote),
        &[a, b],
    );
    // ... and cast to the dynamic type.
    let any = k.b.ty(Ty::Any);
    let cast = k.b.expr(Expr::Cast {
        expr: sum,
        ty: any,
        policy: Policy::CAST.with_overflow(hir_lang::Overflow::Promote),
    });
    let hir = k.finish_body(cast).unwrap();
    let names = intern_lang::Interner::new();
    assert!(hir_lang::print(&hir, &names).contains("op add overflow=promote"));

    // A field default is a constant context but not an integer one.
    let mut k = Kit::new();
    let (a, b) = (int(&mut k, i64::MAX), int(&mut k, 1));
    let sum = k.b.op_with(
        Op::new(OpKind::Add).with_overflow(hir_lang::Overflow::Promote),
        &[a, b],
    );
    let fname = k.ident("big");
    let field = k.b.field(FieldDef {
        default: Some(sum),
        ..FieldDef::named(fname)
    });
    let fields = k.b.list(&[field]);
    let cname = k.name("C");
    let c = k.b.item(Item::new(
        Some(cname),
        ItemKind::Class(ClassDef {
            fields,
            ..ClassDef::default()
        }),
    ));
    assert!(k.finish_items(&[c]).is_ok());
}

#[test]
fn test_promote_rejected_on_static_results() {
    // int_cast<i32> has a static result.
    let mut k = Kit::new();
    let a = int(&mut k, 1);
    let e = k.b.op_with(
        Op::new(OpKind::IntCast(Prim::I32)).with_overflow(hir_lang::Overflow::Promote),
        &[a],
    );
    assert_eq!(
        malformed(k.finish_body(e)),
        Malformed::PromoteOnStaticResult
    );

    // A cast to i64.
    let mut k = Kit::new();
    let a = int(&mut k, 1);
    let i64_ty = k.b.ty(Ty::Prim(Prim::I64));
    let cast = k.b.expr(Expr::Cast {
        expr: a,
        ty: i64_ty,
        policy: Policy::CAST.with_overflow(hir_lang::Overflow::Promote),
    });
    assert_eq!(
        malformed(k.finish_body(cast)),
        Malformed::PromoteOnStaticResult
    );

    // An array length: [u8; 1 + 2] with promote.
    let mut k = Kit::new();
    let (a, b) = (int(&mut k, 1), int(&mut k, 2));
    let len = k.b.op_with(
        Op::new(OpKind::Add).with_overflow(hir_lang::Overflow::Promote),
        &[a, b],
    );
    let u8_ty = k.b.ty(Ty::Prim(Prim::U8));
    let arr = k.b.ty(Ty::Array { elem: u8_ty, len });
    let w = k.b.pat(Pat::Wild);
    let s = k.b.stmt(Stmt::Let {
        pat: w,
        ty: Some(arr),
        init: None,
        else_: None,
    });
    let body = k.b.block(&[s], None);
    assert_eq!(
        k.finish_body(body),
        Err(HirError::Malformed {
            site: Site::Node(NodeRef::Expr(len)),
            problem: Malformed::PromoteOnStaticResult
        })
    );
}
