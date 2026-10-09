//! The lowering examples of `specs/HIR.md` §9, built through the builder,
//! validated, and printed. Each expected text is the spec's example verbatim;
//! when the LexerSketch plan is checked out next to this crate, the test also
//! confirms the spec still contains it, so the spec cannot drift from what the
//! validator accepts.

mod common;

use common::Kit;
use hir_lang::{
    Arm, BinderKind, CaptureMode, Closure, Def, Effects, Expr, FieldPat, FloatLit, Item, ItemKind,
    Lit, Member, Name, Ns, OpKind, Param, ParamKind, Pat, Prim, Res, Shape, Span, SumDef, Ty,
    Variant,
};

/// Prints `hir` and checks the spec (if present) contains the text.
fn check(hir: &hir_lang::Hir, names: &intern_lang::Interner, expected: &str) {
    assert_eq!(hir.validate(), Ok(()));
    let text = hir_lang::print(hir, names);
    assert_eq!(text, expected);
    let spec = concat!(env!("CARGO_MANIFEST_DIR"), "/../_lexersketch/specs/HIR.md");
    if let Ok(spec) = std::fs::read_to_string(spec) {
        assert!(
            spec.replace("\r\n", "\n").contains(expected),
            "specs/HIR.md no longer contains:\n{expected}"
        );
    }
}

/// §9.1 Kraken `match` on a sum.
#[test]
fn test_spec_example_kraken_match() {
    let mut k = Kit::new();
    let f64_ty = k.b.ty(Ty::Prim(Prim::F64));
    let radius = k.b.field(hir_lang::FieldDef {
        ty: Some(f64_ty),
        ..hir_lang::FieldDef::default()
    });
    let circle_fields = k.b.list(&[radius]);
    let circle_name = k.ident("Circle");
    let circle = k.b.variant(Variant {
        name: circle_name,
        shape: Shape::Tuple,
        fields: circle_fields,
        discriminant: None,
    });
    let (wn, hn) = (k.ident("w"), k.ident("h"));
    let fw = k.b.field(hir_lang::FieldDef::named(wn));
    let fh = k.b.field(hir_lang::FieldDef::named(hn));
    let rect_fields = k.b.list(&[fw, fh]);
    let rect_name = k.ident("Rect");
    let rect = k.b.variant(Variant {
        name: rect_name,
        shape: Shape::Named,
        fields: rect_fields,
        discriminant: None,
    });
    let variants = k.b.list(&[circle, rect]);
    let shape_name = k.name("Shape");
    let shape = k.b.item(Item::new(
        Some(shape_name),
        ItemKind::Sum(SumDef {
            variants,
            ..SumDef::default()
        }),
    ));
    let sn = k.name("shape");
    let (ps, shape_b) = k.b.local_param(sn);

    // Shape.Circle(r) if r > 0.0 => 2.5 * r * r
    let r = k.binder("r", BinderKind::Local);
    let pr = k.b.bind(r);
    let circle_def = k.b.def(Def::Variant(circle));
    let cn = k.name("Circle");
    let circle_path = k.b.resolved_path(cn, Ns::Pattern, Res::Def(circle_def));
    let elems = k.b.list(&[pr]);
    let ctor = k.b.pat(Pat::Ctor {
        path: circle_path,
        elems,
        rest: None,
    });
    let ur = k.b.use_binder(r);
    let zero = k.b.lit(Lit::Float(FloatLit::new(0.0)));
    let guard = k.b.op(OpKind::Gt, &[ur, zero]);
    let k25 = k.b.lit(Lit::Float(FloatLit::new(2.5)));
    let (ur1, ur2) = (k.b.use_binder(r), k.b.use_binder(r));
    let m1 = k.b.op(OpKind::Mul, &[k25, ur1]);
    let area1 = k.b.op(OpKind::Mul, &[m1, ur2]);
    // Shape.Rect { w, h } => w * h
    let (wb, hb) = (
        k.binder("w", BinderKind::Local),
        k.binder("h", BinderKind::Local),
    );
    let (pw, ph) = (k.b.bind(wb), k.b.bind(hb));
    let fields = k.b.list(&[
        FieldPat { name: wn, pat: pw },
        FieldPat { name: hn, pat: ph },
    ]);
    let rect_def = k.b.def(Def::Variant(rect));
    let rn = k.name("Rect");
    let rect_path = k.b.resolved_path(rn, Ns::Type, Res::Def(rect_def));
    let record = k.b.pat(Pat::Record {
        path: Some(rect_path),
        fields,
        rest: false,
    });
    let (uw, uh) = (k.b.use_binder(wb), k.b.use_binder(hb));
    let area2 = k.b.op(OpKind::Mul, &[uw, uh]);
    // _ => 0.0
    let wild = k.b.pat(Pat::Wild);
    let zero2 = k.b.lit(Lit::Float(FloatLit::new(0.0)));
    let arms = k.b.list(&[
        Arm {
            pat: ctor,
            guard: Some(guard),
            body: area1,
        },
        Arm {
            pat: record,
            guard: None,
            body: area2,
        },
        Arm {
            pat: wild,
            guard: None,
            body: zero2,
        },
    ]);
    let scrutinee = k.b.use_binder(shape_b);
    let m = k.b.expr(Expr::Match { scrutinee, arms });
    let body = k.b.block(&[], Some(m));
    let area = k.func_fx("area", &[ps], Effects::NONE, body);
    let root = k.b.module(None, &[shape, area]);
    let hir = k.b.finish(root).unwrap();
    check(&hir, &k.names, KRAKEN);
}

const KRAKEN: &str = r#"(module
  (sum Shape
    (variant Circle tuple
      (field
        (prim f64)))
    (variant Rect
      (field w)
      (field h)))
  (fn area
    (param normal
      (bind shape%0))
    (block
      (match
        (use (path shape → local shape%0))
        (arm
          (ctor (path Circle → variant 0)
            (bind r%1))
          (guard
            (op gt
              (use (path r → local r%1))
              (lit 0.0)))
          (op mul overflow=error
            (op mul overflow=error
              (lit 2.5)
              (use (path r → local r%1)))
            (use (path r → local r%1))))
        (arm
          (record (path Rect → variant 1)
            (field w
              (bind w%2))
            (field h
              (bind h%3)))
          (op mul overflow=error
            (use (path w → local w%2))
            (use (path h → local h%3))))
        (arm
          (wild)
          (lit 0.0))))))"#;

/// §9.2 Mercury closure with an implicit capture by reference.
#[test]
fn test_spec_example_mercury_closure() {
    let mut k = Kit::new();
    let scale = k.binder("scale", BinderKind::Local);
    let ps = k.b.bind(scale);
    let three = k.b.int(3);
    let s1 = k.b.let_stmt(ps, Some(three));
    let x = k.binder("x", BinderKind::Param);
    let px = k.b.bind(x);
    let int_name = k.name("int");
    let int_path = k.b.resolved_path(int_name, Ns::Type, Res::Prim(Prim::I64));
    let int_ty = k.b.ty(Ty::Path(int_path));
    let param = k.b.param(Param {
        ty: Some(int_ty),
        ..Param::new(px)
    });
    let (ux, us) = (k.b.use_binder(x), k.b.use_binder(scale));
    let body = k.b.op(OpKind::Mul, &[ux, us]);
    let params = k.b.list(&[param]);
    let closure = k.b.expr(Expr::Closure(Closure {
        params,
        implicit: Some(CaptureMode::ByRef),
        ..Closure::new(body)
    }));
    let f = k.binder("f", BinderKind::Local);
    let pf = k.b.bind(f);
    let s2 = k.b.let_stmt(pf, Some(closure));
    let block = k.b.block(&[s1, s2], None);
    let (hir, names) = k.finish_body_keep(block);
    let hir = hir.unwrap();
    assert_eq!(hir.implicit_captures(closure), vec![scale]);
    check(&hir, &names, MERCURY);
}

const MERCURY: &str = r#"(module
  (fn main
    (block
      (let
        (bind scale%0)
        (lit 3))
      (let
        (bind f%2)
        (closure implicit=by-ref
          (param normal
            (bind x%1)
            (type (path int → prim i64)))
          (op mul overflow=error
            (use (path x → local x%1))
            (use (path scale → local scale%0))))))))"#;

/// §9.3 HQL: the query structure belongs to the plan target; HIR carries the
/// scalar expressions (here the `WHERE age > 30` predicate over a row).
#[test]
fn test_spec_example_hql_predicate() {
    let mut k = Kit::new();
    let rn = k.name("row");
    let (param, row) = k.b.local_param(rn);
    let ur = k.b.use_binder(row);
    let age = k.sym("age");
    let field = k.b.expr(Expr::Field {
        base: ur,
        member: Member::Named(age),
        span: Span::empty(0),
    });
    let thirty = k.b.int(30);
    let cmp = k.b.op(OpKind::Gt, &[field, thirty]);
    let params = k.b.list(&[param]);
    let pred = k.b.expr(Expr::Closure(Closure {
        params,
        ..Closure::new(cmp)
    }));
    let (hir, names) = k.finish_body_keep(pred);
    let hir = hir.unwrap();
    check(&hir, &names, HQL);
}

const HQL: &str = r#"(module
  (fn main
    (closure implicit=none
      (param normal
        (bind row%0))
      (op gt
        (field age
          (use (path row → local row%0)))
        (lit 30)))))"#;

/// §9.4 Mox `foreach` desugared through a template whose temporary is
/// hygienic and resolved by the lowering itself.
#[test]
fn test_spec_example_mox_foreach() {
    let mut k = Kit::new();
    let it = k.sym("it");
    let fe = k.sym("foreach");
    let e1 = k.b.expansion(hir_lang::Expansion {
        kind: hir_lang::ExpnKind::Desugar,
        name: fe,
        call_site: Span::empty(0),
        parent: hir_lang::ExpnId::ROOT,
        def_site: hir_lang::ExpnId::ROOT,
    });
    let items_n = k.name("items");
    let (pi, items) = k.b.local_param(items_n);
    k.b.set_expansion(e1);
    let tmp = k.b.binder(hir_lang::Binder::new(
        Name::marked(it, e1),
        BinderKind::Local,
    ));
    let ptmp = k.b.bind(tmp);
    let iter_n = k.name("mox.iter");
    let iter = k.b.name_expr(iter_n);
    let ui = k.b.use_binder(items);
    let start = k.b.call(iter, &[ui]);
    let s1 = k.b.let_stmt(ptmp, Some(start));
    let next_n = k.name("mox.next");
    let next = k.b.name_expr(next_n);
    let ut = k.b.use_binder(tmp);
    let step = k.b.call(next, &[ut]);
    let kb = k.binder("k", BinderKind::Local);
    let vb = k.binder("v", BinderKind::Local);
    let (pk, pv) = (k.b.bind(kb), k.b.bind(vb));
    let pair = k.b.list(&[pk, pv]);
    let tuple = k.b.pat(Pat::Tuple {
        elems: pair,
        rest: None,
    });
    k.b.set_expansion(hir_lang::ExpnId::ROOT);
    let echo_n = k.name("echo");
    let echo = k.b.name_expr(echo_n);
    let (uk, uv) = (k.b.use_binder(kb), k.b.use_binder(vb));
    let body = k.b.call(echo, &[uk, uv]);
    k.b.set_expansion(e1);
    let null = k.b.pat(Pat::Lit(Lit::Null));
    let stop = k.b.expr(Expr::Break {
        label: None,
        value: None,
    });
    let arms = k.b.list(&[
        Arm {
            pat: null,
            guard: None,
            body: stop,
        },
        Arm {
            pat: tuple,
            guard: None,
            body,
        },
    ]);
    let m = k.b.expr(Expr::Match {
        scrutinee: step,
        arms,
    });
    let lp = k.b.expr(Expr::Loop {
        label: None,
        body: m,
        step: None,
    });
    let block = k.b.block(&[s1], Some(lp));
    k.b.set_expansion(hir_lang::ExpnId::ROOT);
    let f = k.func_fx("main", &[pi], Effects::NONE, block);
    let (hir, names) = k.finish_items_keep(&[f]);
    let hir = hir.unwrap();
    check(&hir, &names, MOX);
}

const MOX: &str = r#"(module
  (fn main
    (param normal
      (bind items%0))
    (block
      (let
        (bind it%1'e1)
        (call
          (use (path mox.iter))
          (use (path items → local items%0))))
      (loop
        (match
          (call
            (use (path mox.next))
            (use (path it'e1 → local it%1'e1)))
          (arm
            (lit null)
            (break))
          (arm
            (tuple
              (bind k%2)
              (bind v%3))
            (call
              (use (path echo))
              (use (path k → local k%2))
              (use (path v → local v%3)))))))))"#;

/// §9.5 Iron function with default, variadic, and named-only parameters.
#[test]
fn test_spec_example_iron_params() {
    let mut k = Kit::new();
    let msg = k.binder("msg", BinderKind::Param);
    let pm = k.b.bind(msg);
    let str_ty = k.b.ty(Ty::Prim(Prim::Str));
    let p_msg = k.b.param(Param {
        ty: Some(str_ty),
        ..Param::new(pm)
    });
    let level = k.binder("level", BinderKind::Param);
    let pl = k.b.bind(level);
    let level_name = k.name("Level");
    let level_path = k.b.name_path(level_name, Ns::Type);
    let level_ty = k.b.ty(Ty::Path(level_path));
    let info_n = k.name("Level.Info");
    let info = k.b.name_expr(info_n);
    let p_level = k.b.param(Param {
        ty: Some(level_ty),
        default: Some(info),
        ..Param::new(pl)
    });
    let args = k.binder("args", BinderKind::Param);
    let pa = k.b.bind(args);
    let any = k.b.ty(Ty::Any);
    let p_args = k.b.param(Param {
        ty: Some(any),
        kind: ParamKind::Rest,
        ..Param::new(pa)
    });
    let tag = k.binder("tag", BinderKind::Param);
    let pt = k.b.bind(tag);
    let str_ty2 = k.b.ty(Ty::Prim(Prim::Str));
    let app = k.b.str_lit("app");
    let p_tag = k.b.param(Param {
        ty: Some(str_ty2),
        default: Some(app),
        kind: ParamKind::NamedOnly,
        ..Param::new(pt)
    });
    let body = k.b.block(&[], None);
    let f = k.func_fx("log", &[p_msg, p_level, p_args, p_tag], Effects::NONE, body);
    let (hir, names) = k.finish_items_keep(&[f]);
    let hir = hir.unwrap();
    check(&hir, &names, IRON);
}

const IRON: &str = r#"(module
  (fn log
    (param normal
      (bind msg%0)
      (prim str))
    (param normal
      (bind level%1)
      (type (path Level))
      (default
        (use (path Level.Info))))
    (param rest
      (bind args%2)
      (any))
    (param named-only
      (bind tag%3)
      (prim str)
      (default
        (lit "app")))
    (block)))"#;

/// §9.6 Mox: a union parameter type and `$a[$k] .= "!"`, whose place is
/// evaluated once through `let_place` (the compound template's binder).
#[test]
fn test_spec_example_mox_compound_assignment() {
    let mut k = Kit::new();
    // function tag(string|int|null $k, array $a) { $a[$k] .= "!"; }
    let st = k.b.ty(Ty::Prim(Prim::Str));
    let int = k.b.ty(Ty::Prim(Prim::I64));
    let members = k.b.list(&[st, int]);
    let union = k.b.ty(Ty::Union(members));
    let k_ty = k.b.ty(Ty::Nullable(union));
    let kb = k.binder("k", BinderKind::Param);
    let k_pat = k.b.bind(kb);
    let pk = k.b.param(Param {
        ty: Some(k_ty),
        ..Param::new(k_pat)
    });
    let a_name = k.name("a");
    let (pa, ab) = k.b.local_param(a_name);
    let compound = k.sym("compound_assign");
    let e1 = k.b.expansion(hir_lang::Expansion {
        kind: hir_lang::ExpnKind::Template,
        name: compound,
        call_site: Span::empty(0),
        parent: hir_lang::ExpnId::ROOT,
        def_site: hir_lang::ExpnId::ROOT,
    });
    let (ua, uk) = (k.b.use_binder(ab), k.b.use_binder(kb));
    let place = k.b.expr(Expr::Index {
        base: ua,
        index: uk,
    });
    k.b.set_expansion(e1);
    let p_sym = k.sym("p");
    let p = k.b.binder(hir_lang::Binder::new(
        Name::marked(p_sym, e1),
        BinderKind::Place,
    ));
    let target = k.b.use_binder(p);
    let read = k.b.use_binder(p);
    let concat_name = k.name("concat");
    let concat_sym = k.sym("mox.concat");
    let concat =
        k.b.resolved_path(concat_name, Ns::Value, Res::Extern(concat_sym));
    let concat = k.b.expr(Expr::Path(concat));
    k.b.set_expansion(hir_lang::ExpnId::ROOT);
    let bang = k.b.str_lit("!");
    k.b.set_expansion(e1);
    let value = k.b.call(concat, &[read, bang]);
    let assign = k.b.expr(Expr::Assign {
        target,
        op: None,
        value,
    });
    let let_place = k.b.expr(Expr::LetPlace {
        binder: p,
        place,
        body: assign,
    });
    k.b.set_expansion(hir_lang::ExpnId::ROOT);
    let stmt = k.b.expr_stmt(let_place);
    let body = k.b.block(&[stmt], None);
    let tag = k.func_fx("tag", &[pk, pa], Effects::NONE, body);
    let (hir, names) = k.finish_items_keep(&[tag]);
    let hir = hir.unwrap();
    check(&hir, &names, MOX_COMPOUND);
}

const MOX_COMPOUND: &str = r#"(module
  (fn tag
    (param normal
      (bind k%0)
      (nullable
        (union
          (prim i64)
          (prim str))))
    (param normal
      (bind a%1))
    (block
      (do
        (let-place p%2'e1
          (index
            (use (path a → local a%1))
            (use (path k → local k%0)))
          (assign
            (use (path p'e1 → local p%2'e1))
            (call
              (use (path concat → extern mox.concat))
              (use (path p'e1 → local p%2'e1))
              (lit "!"))))))))"#;
