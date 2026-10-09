//! Scoping, frames, captures, hygiene, and `Hir::resolve`.

mod common;

use common::Kit;
use hir_lang::{
    Arm, BinderKind, Capture, CaptureMode, Closure, Effects, Expansion, ExpnId, ExpnKind, Expr,
    FnDef, GenericParam, Generics, HirError, ImplDef, Item, ItemKind, List, Name, Ns, Param, Pat,
    PathId, Prim, Res, Span, Ty,
};

/// An unresolved value path to `name`, wrapped in an expression.
fn name_use(k: &mut Kit, name: &str) -> (hir_lang::ExprId, PathId) {
    let n = k.name(name);
    let p = k.b.name_path(n, Ns::Value);
    (k.b.expr(Expr::Path(p)), p)
}

#[test]
fn test_resolve_local_in_scope_succeeds_and_persists() {
    let mut k = Kit::new();
    let x = k.binder("x", BinderKind::Local);
    let px = k.b.bind(x);
    let one = k.b.int(1);
    let s = k.b.let_stmt(px, Some(one));
    let (use_x, path) = name_use(&mut k, "x");
    let body = k.b.block(&[s], Some(use_x));
    let mut hir = k.finish_body(body).unwrap();
    assert!(hir.can_reference(path, x));
    hir.resolve(path, Res::Local(x)).unwrap();
    assert_eq!(hir.path(path).res, Res::Local(x));
    // Overwriting with another valid resolution is allowed.
    hir.resolve(path, Res::Err).unwrap();
    assert_eq!(hir.path(path).res, Res::Err);
}

#[test]
fn test_resolve_rejects_every_kind_of_bad_resolution_without_changing_the_path() {
    let mut k = Kit::new();
    // fn f(p) { { let inner = 1; } ; p_use }  plus a nested fn that names p.
    let (param, p) = {
        let n = k.name("p");
        k.b.local_param(n)
    };
    let inner = k.binder("inner", BinderKind::Local);
    let pin = k.b.bind(inner);
    let one = k.b.int(1);
    let s_inner = k.b.let_stmt(pin, Some(one));
    let blk = k.b.block(&[s_inner], None);
    let s_blk = k.b.expr_stmt(blk);
    let (use_here, here) = name_use(&mut k, "inner");
    let (nested_use, nested_path) = name_use(&mut k, "p");
    let nested_body = k.b.block(&[], Some(nested_use));
    let nested = k.func_fx("nested", &[], Effects::NONE, nested_body);
    let decl = k.b.stmt(hir_lang::Stmt::Item(nested));
    let body = k.b.block(&[s_blk, decl], Some(use_here));
    let f = k.func_fx("f", &[param], Effects::NONE, body);
    let mut hir = k.finish_items(&[f]).unwrap();

    assert_eq!(
        hir.resolve(here, Res::Local(inner)),
        Err(HirError::OutOfScope {
            path: here,
            binder: inner
        })
    );
    assert_eq!(
        hir.resolve(nested_path, Res::Local(p)),
        Err(HirError::NotCapturable {
            path: nested_path,
            binder: p
        })
    );
    assert_eq!(
        hir.resolve(here, Res::Prim(Prim::Bool)),
        Err(HirError::Resolution {
            path: here,
            res: Res::Prim(Prim::Bool)
        })
    );
    assert!(matches!(
        hir.resolve(
            here,
            Res::Local(hir_lang::BinderId::from_index(999).unwrap())
        ),
        Err(HirError::Dangling { .. })
    ));
    assert!(matches!(
        hir.resolve(PathId::from_index(999).unwrap(), Res::Err),
        Err(HirError::Dangling { .. })
    ));
    // A function item may be named from a value path; a module may not.
    let f_def = hir.def(hir_lang::Def::Item(f));
    let root_def = hir.def(hir_lang::Def::Item(hir.root()));
    assert!(hir.resolve(here, Res::Def(f_def)).is_ok());
    assert!(matches!(
        hir.resolve(here, Res::Def(root_def)),
        Err(HirError::Resolution { .. })
    ));
    assert_eq!(hir.path(here).res, Res::Def(f_def));
    assert!(!hir.can_reference(nested_path, p));
}

#[test]
fn test_arm_binders_visible_in_guard_and_body_only() {
    let mut k = Kit::new();
    let x = k.binder("x", BinderKind::Local);
    let px = k.b.bind(x);
    let (g, guard_path) = name_use(&mut k, "x");
    let (body, body_path) = name_use(&mut k, "x");
    let w = k.b.pat(Pat::Wild);
    let (other, other_path) = name_use(&mut k, "x");
    let scrutinee = k.b.int(5);
    let arms = k.b.list(&[
        Arm {
            pat: px,
            guard: Some(g),
            body,
        },
        Arm {
            pat: w,
            guard: None,
            body: other,
        },
    ]);
    let m = k.b.expr(Expr::Match { scrutinee, arms });
    let hir = k.finish_body(m).unwrap();
    assert!(hir.can_reference(guard_path, x));
    assert!(hir.can_reference(body_path, x));
    assert!(!hir.can_reference(other_path, x));
}

#[test]
fn test_param_defaults_see_earlier_params_only() {
    let mut k = Kit::new();
    let a = k.binder("a", BinderKind::Param);
    let bb = k.binder("b", BinderKind::Param);
    let pa = k.b.bind(a);
    let (da, da_path) = name_use(&mut k, "b");
    let param_a = k.b.param(Param {
        default: Some(da),
        ..Param::new(pa)
    });
    let pb = k.b.bind(bb);
    let (db, db_path) = name_use(&mut k, "a");
    let param_b = k.b.param(Param {
        default: Some(db),
        ..Param::new(pb)
    });
    let body = k.b.block(&[], None);
    let f = k.func_fx("f", &[param_a, param_b], Effects::NONE, body);
    let hir = k.finish_items(&[f]).unwrap();
    assert!(!hir.can_reference(da_path, bb), "a's default cannot see b");
    assert!(hir.can_reference(db_path, a), "b's default sees a");
    assert!(
        !hir.can_reference(da_path, a),
        "a's default cannot see a itself"
    );
}

#[test]
fn test_impl_generics_visible_in_methods_but_fn_generics_not_in_nested_fns() {
    let mut k = Kit::new();
    // impl<T> Foo { fn get() -> T }
    let t = k.binder("T", BinderKind::TypeParam);
    let tname = k.name("T");
    let ret_path = k.b.name_path(tname, Ns::Type);
    let ret = k.b.ty(Ty::Path(ret_path));
    let body = k.b.block(&[], None);
    let mname = k.name("get");
    let method = k.b.item(Item::new(
        Some(mname),
        ItemKind::Fn(FnDef {
            ret: Some(ret),
            body: Some(body),
            ..FnDef::default()
        }),
    ));
    let gparams = k.b.list(&[GenericParam::new(t)]);
    let self_ty = k.b.ty(Ty::Any);
    let items = k.b.list(&[method]);
    let imp = k.b.item(Item::new(
        None,
        ItemKind::Impl(ImplDef {
            generics: Generics {
                params: gparams,
                preds: List::EMPTY,
            },
            items,
            ..ImplDef::new(self_ty)
        }),
    ));

    // fn outer<U>() { fn inner() -> U {} }
    let u = k.binder("U", BinderKind::TypeParam);
    let uname = k.name("U");
    let inner_ret_path = k.b.name_path(uname, Ns::Type);
    let inner_ret = k.b.ty(Ty::Path(inner_ret_path));
    let inner_body = k.b.block(&[], None);
    let iname = k.name("inner");
    let inner = k.b.item(Item::new(
        Some(iname),
        ItemKind::Fn(FnDef {
            ret: Some(inner_ret),
            body: Some(inner_body),
            ..FnDef::default()
        }),
    ));
    let decl = k.b.stmt(hir_lang::Stmt::Item(inner));
    let outer_body = k.b.block(&[decl], None);
    let ugp = k.b.list(&[GenericParam::new(u)]);
    let oname = k.name("outer");
    let outer = k.b.item(Item::new(
        Some(oname),
        ItemKind::Fn(FnDef {
            generics: Generics {
                params: ugp,
                preds: List::EMPTY,
            },
            body: Some(outer_body),
            ..FnDef::default()
        }),
    ));
    let mut hir = k.finish_items(&[imp, outer]).unwrap();
    assert!(hir.resolve(ret_path, Res::Local(t)).is_ok());
    assert!(matches!(
        hir.resolve(inner_ret_path, Res::Local(u)),
        Err(HirError::NotCapturable { .. })
    ));
}

#[test]
fn test_const_param_visible_in_array_length() {
    // fn f<const N: usize>() { let a: [u8; N]; }
    let mut k = Kit::new();
    let n = k.binder("N", BinderKind::ConstParam);
    let usize_ty = k.b.ty(Ty::Prim(Prim::Usize));
    let (len, len_path) = name_use(&mut k, "N");
    let u8_ty = k.b.ty(Ty::Prim(Prim::U8));
    let arr = k.b.ty(Ty::Array { elem: u8_ty, len });
    let a = k.binder("a", BinderKind::Local);
    let pa = k.b.bind(a);
    let s = k.b.stmt(hir_lang::Stmt::Let {
        pat: pa,
        ty: Some(arr),
        init: None,
        else_: None,
    });
    let body = k.b.block(&[s], None);
    let gp = k.b.list(&[GenericParam {
        ty: Some(usize_ty),
        ..GenericParam::new(n)
    }]);
    let fname = k.name("f");
    let f = k.b.item(Item::new(
        Some(fname),
        ItemKind::Fn(FnDef {
            generics: Generics {
                params: gp,
                preds: List::EMPTY,
            },
            body: Some(body),
            ..FnDef::default()
        }),
    ));
    let mut hir = k.finish_items(&[f]).unwrap();
    assert!(hir.resolve(len_path, Res::Local(n)).is_ok());
}

#[test]
fn test_explicit_captures_rebind_inside_and_are_excluded_from_implicit() {
    // fn f(total, scale) { closure use (&total) implicit=by-value { total; scale } }
    let mut k = Kit::new();
    let tn = k.name("total");
    let (pt, total) = k.b.local_param(tn);
    let sn = k.name("scale");
    let (ps, scale) = k.b.local_param(sn);
    let tname = k.name("total");
    let outer = k.b.resolved_path(tname, Ns::Value, Res::Local(total));
    let inner_total = k.binder("total", BinderKind::Capture);
    let captures = k.b.list(&[Capture {
        outer,
        binder: inner_total,
        mode: CaptureMode::ByMutRef,
    }]);
    let use_total = k.b.use_binder(inner_total);
    let use_scale = k.b.use_binder(scale);
    let s1 = k.b.expr_stmt(use_total);
    let cbody = k.b.block(&[s1], Some(use_scale));
    let c = k.b.expr(Expr::Closure(Closure {
        params: List::EMPTY,
        ret: None,
        body: cbody,
        effects: Effects::NONE,
        implicit: Some(CaptureMode::ByValue),
        captures,
        self_binder: None,
        defaults: hir_lang::DefaultEval::PerCall,
    }));
    let body = k.b.block(&[], Some(c));
    let f = k.func_fx("f", &[pt, ps], Effects::NONE, body);
    let hir = k.finish_items(&[f]).unwrap();
    assert_eq!(hir.implicit_captures(c), vec![scale]);
    assert!(hir.implicit_captures(body).is_empty(), "not a closure");
}

#[test]
fn test_explicit_capture_of_out_of_scope_variable_is_rejected() {
    let mut k = Kit::new();
    let ghost = k.binder("ghost", BinderKind::Local);
    let pg = k.b.bind(ghost);
    let one = k.b.int(1);
    let s = k.b.let_stmt(pg, Some(one));
    let blk = k.b.block(&[s], None);
    let s_blk = k.b.expr_stmt(blk);
    let gname = k.name("ghost");
    let outer = k.b.resolved_path(gname, Ns::Value, Res::Local(ghost));
    let inner = k.binder("ghost", BinderKind::Capture);
    let captures = k.b.list(&[Capture {
        outer,
        binder: inner,
        mode: CaptureMode::ByValue,
    }]);
    let cbody = k.b.use_binder(inner);
    let c = k.b.expr(Expr::Closure(Closure {
        params: List::EMPTY,
        ret: None,
        body: cbody,
        effects: Effects::NONE,
        implicit: None,
        captures,
        self_binder: None,
        defaults: hir_lang::DefaultEval::PerCall,
    }));
    let body = k.b.block(&[s_blk], Some(c));
    assert!(matches!(
        k.finish_body(body),
        Err(HirError::OutOfScope { binder, .. }) if binder == ghost
    ));
}

#[test]
fn test_hygiene_template_temporary_and_user_variable_coexist() {
    // A `foreach` desugaring introduces `it` with the template's mark while the
    // user also has an `it`. Both are distinct binders; lowering resolves the
    // template's own reference directly, and the user's `it` resolves by name.
    let mut k = Kit::new();
    let it_sym = k.sym("it");
    let foreach = k.sym("foreach");
    let e = k.b.expansion(Expansion {
        kind: ExpnKind::Desugar,
        name: foreach,
        call_site: Span::new(20, 60),
        parent: ExpnId::ROOT,
        def_site: ExpnId::ROOT,
    });
    let user_it =
        k.b.binder(hir_lang::Binder::new(Name::new(it_sym), BinderKind::Local));
    let pu = k.b.bind(user_it);
    let zero = k.b.int(0);
    let s_user = k.b.let_stmt(pu, Some(zero));

    k.b.set_expansion(e);
    let tmp_it = k.b.binder(hir_lang::Binder::new(
        Name::marked(it_sym, e),
        BinderKind::Local,
    ));
    let pt = k.b.bind(tmp_it);
    let one = k.b.int(1);
    let s_tmp = k.b.let_stmt(pt, Some(one));
    let tmp_use = k.b.use_binder(tmp_it);
    k.b.set_expansion(ExpnId::ROOT);

    let user_path = k.b.name_path(Name::new(it_sym), Ns::Value);
    let user_use = k.b.expr(Expr::Path(user_path));
    let s_tmp_use = k.b.expr_stmt(tmp_use);
    let body = k.b.block(&[s_user, s_tmp, s_tmp_use], Some(user_use));
    let mut hir = k.finish_body(body).unwrap();

    // Both binders are in scope at the user's reference; the names differ by mark.
    assert!(hir.can_reference(user_path, user_it));
    assert!(hir.can_reference(user_path, tmp_it));
    assert_ne!(
        hir.binder(user_it).unwrap().name,
        hir.binder(tmp_it).unwrap().name
    );
    hir.resolve(user_path, Res::Local(user_it)).unwrap();
    assert_eq!(hir.origin(hir_lang::NodeRef::Expr(tmp_use)).expn, e);
    assert_eq!(hir.expansion(e).unwrap().name, foreach);
    assert!(hir.print_names_contains_mark());
}

trait PrintCheck {
    fn print_names_contains_mark(&self) -> bool;
}

impl PrintCheck for hir_lang::Hir {
    fn print_names_contains_mark(&self) -> bool {
        let names = intern_lang::Interner::new();
        hir_lang::print(self, &names).contains("'e1")
    }
}

#[test]
fn test_labels_do_not_cross_closures() {
    let mut k = Kit::new();
    let l = k.binder("outer", BinderKind::Label);
    let br = k.b.expr(Expr::Break {
        label: Some(l),
        value: None,
    });
    let inner_loop = k.b.expr(Expr::Loop {
        label: None,
        body: br,
        step: None,
    });
    let c = k.b.expr(Expr::Closure(Closure {
        params: List::EMPTY,
        ret: None,
        body: inner_loop,
        effects: Effects::NONE,
        implicit: Some(CaptureMode::Infer),
        captures: List::EMPTY,
        self_binder: None,
        defaults: hir_lang::DefaultEval::PerCall,
    }));
    let lp = k.b.expr(Expr::Loop {
        label: Some(l),
        body: c,
        step: None,
    });
    assert!(matches!(
        k.finish_body(lp),
        Err(HirError::Jump {
            problem: hir_lang::JumpProblem::LabelNotInScope,
            ..
        })
    ));
}

#[test]
fn test_loop_step_break_and_labeled_block_break_are_valid() {
    // 'blk: { loop [step: break] { if c { break 'blk 1 } else { continue } } }
    let mut k = Kit::new();
    let blk_label = k.binder("blk", BinderKind::Label);
    let one = k.b.int(1);
    let br = k.b.expr(Expr::Break {
        label: Some(blk_label),
        value: Some(one),
    });
    let cont = k.b.expr(Expr::Continue { label: None });
    let c = k.b.lit(hir_lang::Lit::Bool(true));
    let iff = k.b.expr(Expr::If {
        cond: c,
        then: br,
        else_: Some(cont),
    });
    // `break` may leave a loop from its step; `continue` may not (it would
    // re-run the step).
    let step = k.b.expr(Expr::Break {
        label: None,
        value: None,
    });
    let lp = k.b.expr(Expr::Loop {
        label: None,
        body: iff,
        step: Some(step),
    });
    let blk = k.b.expr(Expr::Block(hir_lang::Block {
        tail: Some(lp),
        label: Some(blk_label),
        ..hir_lang::Block::default()
    }));
    assert!(k.finish_body(blk).is_ok());
}

#[test]
fn test_or_pattern_alternatives_share_binders() {
    let mut k = Kit::new();
    let x = k.binder("x", BinderKind::Local);
    let (p1, p2) = (k.b.bind(x), k.b.bind(x));
    let some = k.name("Some");
    let ctor_path = k.b.name_path(some, Ns::Pattern);
    let elems = k.b.list(&[p1]);
    let ctor = k.b.pat(Pat::Ctor {
        path: ctor_path,
        elems,
        rest: None,
    });
    let alts = k.b.list(&[ctor, p2]);
    let or = k.b.pat(Pat::Or(alts));
    let (body, path) = name_use(&mut k, "x");
    let scrutinee = k.b.int(0);
    let arms = k.b.list(&[Arm {
        pat: or,
        guard: None,
        body,
    }]);
    let m = k.b.expr(Expr::Match { scrutinee, arms });
    let mut hir = k.finish_body(m).unwrap();
    hir.resolve(path, Res::Local(x)).unwrap();
}
