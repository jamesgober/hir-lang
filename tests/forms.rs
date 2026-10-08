//! One HIR that uses every node form, validated, walked, and printed against a
//! golden snapshot (the printer is deterministic across platforms).

mod common;

use common::Kit;
use hir_lang::{
    Arg, ArgKind, Arm, Attr, AttrArg, AttrValue, BindMode, BinderKind, Block, BorrowKind, Capture,
    CaptureMode, ClassDef, Closure, Effects, Expr, FieldDef, FieldInit, FieldPat, FloatLit, FnDef,
    GenericParam, Generics, Hir, IdKind, ImplDef, IntLit, InterfaceDef, Item, ItemKind, List, Lit,
    MapEntry, Member, NodeRef, Ns, Op, OpKind, Overflow, Param, ParamKind, Pat, Policy, Prim,
    RecordDef, Res, Shape, SliceRest, Span, Stmt, SumDef, Ty, Variant, Vis, WherePred,
};

/// Builds the kitchen-sink HIR. Returns it with the interner.
pub fn kitchen_sink() -> (Hir, intern_lang::Interner) {
    let mut k = Kit::new();

    // import std::io as io; import std::*;
    let std = k.name("std");
    let io = k.name("io");
    let seg_std = hir_lang::Segment::new(std, k.b.origin());
    let seg_io = hir_lang::Segment::new(io, k.b.origin());
    let segs = k.b.list(&[seg_std, seg_io]);
    let io_path = k.b.path(hir_lang::Path {
        segments: segs,
        ns: Ns::Import,
        res: Res::Unresolved,
        global: true,
    });
    let import_io = k.b.item(Item::new(
        Some(io),
        ItemKind::Import {
            path: io_path,
            glob: false,
        },
    ));
    let std_path = k.b.name_path(std, Ns::Import);
    let import_glob = k.b.item(Item::new(
        None,
        ItemKind::Import {
            path: std_path,
            glob: true,
        },
    ));

    // record Point { x: i32, pub y: i32 = 0 }
    let i32_ty = k.b.ty(Ty::Prim(Prim::I32));
    let fx = k.ident("x");
    let field_x = k.b.field(FieldDef {
        ty: Some(i32_ty),
        ..FieldDef::named(fx)
    });
    let i32_ty2 = k.b.ty(Ty::Prim(Prim::I32));
    let zero = k.b.int(0);
    let fy = k.ident("y");
    let field_y = k.b.field(FieldDef {
        ty: Some(i32_ty2),
        default: Some(zero),
        vis: Vis::Public,
        ..FieldDef::named(fy)
    });
    let point_fields = k.b.list(&[field_x, field_y]);
    let point_name = k.name("Point");
    let point = k.b.item(
        Item::new(
            Some(point_name),
            ItemKind::Record(RecordDef {
                fields: point_fields,
                ..RecordDef::default()
            }),
        )
        .with_vis(Vis::Public),
    );

    // sum Shape<T> { Circle(f64), Rect { w: f64 }, Empty = 3 }
    let t = k.binder("T", BinderKind::TypeParam);
    let f64_ty = k.b.ty(Ty::Prim(Prim::F64));
    let circle_field = k.b.field(FieldDef {
        ty: Some(f64_ty),
        ..FieldDef::default()
    });
    let circle_fields = k.b.list(&[circle_field]);
    let circle_name = k.ident("Circle");
    let circle = k.b.variant(Variant {
        name: circle_name,
        shape: Shape::Tuple,
        fields: circle_fields,
        discriminant: None,
    });
    let f64_ty2 = k.b.ty(Ty::Prim(Prim::F64));
    let fw = k.ident("w");
    let w_field = k.b.field(FieldDef {
        ty: Some(f64_ty2),
        ..FieldDef::named(fw)
    });
    let rect_fields = k.b.list(&[w_field]);
    let rect_name = k.ident("Rect");
    let rect = k.b.variant(Variant {
        name: rect_name,
        shape: Shape::Named,
        fields: rect_fields,
        discriminant: None,
    });
    let three = k.b.int(3);
    let empty_name = k.ident("Empty");
    let empty = k.b.variant(Variant {
        name: empty_name,
        shape: Shape::Unit,
        fields: List::EMPTY,
        discriminant: Some(three),
    });
    let variants = k.b.list(&[circle, rect, empty]);
    let shape_gp = k.b.list(&[GenericParam::new(t)]);
    let shape_name = k.name("Shape");
    let shape = k.b.item(Item::new(
        Some(shape_name),
        ItemKind::Sum(SumDef {
            generics: Generics {
                params: shape_gp,
                preds: List::EMPTY,
            },
            variants,
        }),
    ));

    // interface Show { fn show(self) -> str; const N: i32; type Out = i32 }
    let self_b = k.binder("self", BinderKind::Param);
    let self_pat = k.b.bind(self_b);
    let self_ty = k.b.ty(Ty::SelfTy);
    let recv = k.b.param(Param {
        ty: Some(self_ty),
        kind: ParamKind::Receiver,
        ..Param::new(self_pat)
    });
    let str_ty = k.b.ty(Ty::Prim(Prim::Str));
    let recv_list = k.b.list(&[recv]);
    let show_name = k.name("show");
    let show_decl = k.b.item(Item::new(
        Some(show_name),
        ItemKind::Fn(FnDef {
            params: recv_list,
            ret: Some(str_ty),
            ..FnDef::default()
        }),
    ));
    let n_name = k.name("N");
    let n_decl = k.b.item(Item::new(
        Some(n_name),
        ItemKind::Const {
            ty: None,
            value: None,
        },
    ));
    let i32_default = k.b.ty(Ty::Prim(Prim::I32));
    let out_name = k.name("Out");
    let out_decl = k.b.item(Item::new(
        Some(out_name),
        ItemKind::AssocType {
            bounds: List::EMPTY,
            default: Some(i32_default),
        },
    ));
    let show_items = k.b.list(&[show_decl, n_decl, out_decl]);
    let iface_name = k.name("Show");
    let iface = k.b.item(Item::new(
        Some(iface_name),
        ItemKind::Interface(InterfaceDef {
            items: show_items,
            ..InterfaceDef::default()
        }),
    ));

    // impl Show for Point { fn show(self) -> str { "p" } const N = 1; type Out = i32 }
    let self2 = k.binder("self", BinderKind::Param);
    let self2_pat = k.b.bind(self2);
    let recv2 = k.b.param(Param {
        kind: ParamKind::Receiver,
        ..Param::new(self2_pat)
    });
    let p_lit = k.b.str_lit("p");
    let show_body = k.b.block(&[], Some(p_lit));
    let recv2_list = k.b.list(&[recv2]);
    let show2_name = k.name("show");
    let show_impl = k.b.item(Item::new(
        Some(show2_name),
        ItemKind::Fn(FnDef {
            params: recv2_list,
            body: Some(show_body),
            ..FnDef::default()
        }),
    ));
    let one = k.b.int(1);
    let n2_name = k.name("N");
    let n_impl = k.b.item(Item::new(
        Some(n2_name),
        ItemKind::Const {
            ty: None,
            value: Some(one),
        },
    ));
    let i32_out = k.b.ty(Ty::Prim(Prim::I32));
    let out2_name = k.name("Out");
    let out_impl = k.b.item(Item::new(
        Some(out2_name),
        ItemKind::Alias {
            generics: Generics::default(),
            ty: i32_out,
        },
    ));
    let show_ref = k.name("Show");
    let show_path = k.b.name_path(show_ref, Ns::Type);
    let show_ty = k.b.ty(Ty::Path(show_path));
    let point_ref = k.name("Point");
    let point_path = k.b.resolved_path(point_ref, Ns::Type, Res::Item(point));
    let point_ty = k.b.ty(Ty::Path(point_path));
    let impl_items = k.b.list(&[show_impl, n_impl, out_impl]);
    let imp = k.b.item(Item::new(
        None,
        ItemKind::Impl(ImplDef {
            interface: Some(show_ty),
            items: impl_items,
            ..ImplDef::new(point_ty)
        }),
    ));

    // abstract class Animal { name; fn speak(self); static COUNT = 0; }
    let fname = k.ident("name");
    let name_field = k.b.field(FieldDef::named(fname));
    let self3 = k.binder("self", BinderKind::Param);
    let self3_pat = k.b.bind(self3);
    let recv3 = k.b.param(Param {
        kind: ParamKind::Receiver,
        ..Param::new(self3_pat)
    });
    let recv3_list = k.b.list(&[recv3]);
    let speak_name = k.name("speak");
    let speak = k.b.item(Item::new(
        Some(speak_name),
        ItemKind::Fn(FnDef {
            params: recv3_list,
            ..FnDef::default()
        }),
    ));
    let zero2 = k.b.int(0);
    let count_name = k.name("COUNT");
    let count = k.b.item(Item::new(
        Some(count_name),
        ItemKind::Global {
            ty: None,
            mutable: true,
            init: Some(zero2),
        },
    ));
    let class_fields = k.b.list(&[name_field]);
    let class_items = k.b.list(&[speak, count]);
    let animal_name = k.name("Animal");
    let animal = k.b.item(Item::new(
        Some(animal_name),
        ItemKind::Class(ClassDef {
            fields: class_fields,
            items: class_items,
            is_abstract: true,
            ..ClassDef::default()
        }),
    ));

    // const MAX: i64 = 100; alias Id = u64; module inner {}
    let i64_ty = k.b.ty(Ty::Prim(Prim::I64));
    let hundred = k.b.lit(Lit::Int(IntLit::new(100).with_suffix(Prim::I64)));
    let max_name = k.name("MAX");
    let max = k.b.item(Item::new(
        Some(max_name),
        ItemKind::Const {
            ty: Some(i64_ty),
            value: Some(hundred),
        },
    ));
    let u64_ty = k.b.ty(Ty::Prim(Prim::U64));
    let id_name = k.name("Id");
    let alias = k.b.item(Item::new(
        Some(id_name),
        ItemKind::Alias {
            generics: Generics::default(),
            ty: u64_ty,
        },
    ));
    let inner_name = k.name("inner");
    let inner_mod = k.b.module(Some(inner_name), &[]);

    let main = main_fn(&mut k);

    let root_name = k.name("app");
    let root = k.b.module(
        Some(root_name),
        &[
            import_io,
            import_glob,
            point,
            shape,
            iface,
            imp,
            animal,
            max,
            alias,
            inner_mod,
            main,
        ],
    );
    let inline = k.ident("inline");
    let budget = k.ident("budget");
    let cpu = k.ident("cpu");
    let c_name = k.ident("C");
    let args = k.b.list(&[
        AttrArg {
            key: Some(cpu),
            value: Some(AttrValue::Lit(Lit::Int(IntLit::new(10)))),
        },
        AttrArg {
            key: None,
            value: Some(AttrValue::Name(c_name)),
        },
    ]);
    k.b.attach(
        NodeRef::Item(main),
        &[
            Attr {
                name: inline,
                args: List::EMPTY,
            },
            Attr { name: budget, args },
        ],
    );
    let hir = k.b.finish(root).unwrap();
    (hir, k.names)
}

/// `fn main<U, const M: usize>(a: i32, ...rest, *, z = 1) throws async yield -> U
///  where U: Show { ... every expression form ... }`
fn main_fn(k: &mut Kit) -> hir_lang::ItemId {
    let u = k.binder("U", BinderKind::TypeParam);
    let m = k.binder("M", BinderKind::ConstParam);
    let usize_ty = k.b.ty(Ty::Prim(Prim::Usize));
    let show = k.name("Show");
    let show_path = k.b.name_path(show, Ns::Type);
    let show_bound = k.b.ty(Ty::Path(show_path));
    let u_name = k.name("U");
    let u_path = k.b.resolved_path(u_name, Ns::Type, Res::Local(u));
    let u_ty = k.b.ty(Ty::Path(u_path));
    let bounds = k.b.list(&[show_bound]);
    let preds = k.b.list(&[WherePred { ty: u_ty, bounds }]);
    let params_g = k.b.list(&[
        GenericParam::new(u),
        GenericParam {
            ty: Some(usize_ty),
            ..GenericParam::new(m)
        },
    ]);

    let a_name = k.name("a");
    let (pa, a) = k.b.local_param(a_name);
    let rest_b = k.binder("rest", BinderKind::Param);
    let rest_pat = k.b.bind(rest_b);
    let p_rest = k.b.param(Param {
        kind: ParamKind::Rest,
        ..Param::new(rest_pat)
    });
    let z_b = k.binder("z", BinderKind::Param);
    let z_pat = k.b.bind(z_b);
    let z_default = k.b.int(1);
    let p_z = k.b.param(Param {
        kind: ParamKind::NamedOnly,
        default: Some(z_default),
        ..Param::new(z_pat)
    });

    let mut stmts = Vec::new();

    // let (x, mut y): (i32, i32) = (1, 2);
    let x = k.binder("x", BinderKind::Local);
    let y_name = k.name("y");
    let y =
        k.b.binder(hir_lang::Binder::new(y_name, BinderKind::Local).with_mutable(true));
    let (px, py) = (k.b.bind(x), k.b.bind(y));
    let tuple_elems = k.b.list(&[px, py]);
    let tuple_pat = k.b.pat(Pat::Tuple {
        elems: tuple_elems,
        rest: None,
    });
    let i32a = k.b.ty(Ty::Prim(Prim::I32));
    let i32b = k.b.ty(Ty::Prim(Prim::I32));
    let tys = k.b.list(&[i32a, i32b]);
    let tuple_ty = k.b.ty(Ty::Tuple(tys));
    let (one, two) = (k.b.int(1), k.b.int(2));
    let vals = k.b.list(&[one, two]);
    let tuple = k.b.expr(Expr::Tuple(vals));
    stmts.push(k.b.stmt(Stmt::Let {
        pat: tuple_pat,
        ty: Some(tuple_ty),
        init: Some(tuple),
        else_: None,
    }));

    // let [first, mid @ .., last] = [1; 3] else { return };
    let first = k.binder("first", BinderKind::Local);
    let mid = k.binder("mid", BinderKind::Local);
    let last = k.binder("last", BinderKind::Local);
    let (pf, pm, pl) = (k.b.bind(first), k.b.bind(mid), k.b.bind(last));
    let prefix = k.b.list(&[pf]);
    let suffix = k.b.list(&[pl]);
    let slice_pat = k.b.pat(Pat::Slice {
        prefix,
        rest: Some(SliceRest {
            bind: Some(pm),
            suffix,
        }),
    });
    let (elem, cnt) = (k.b.int(1), k.b.int(3));
    let repeat = k.b.expr(Expr::Repeat { elem, count: cnt });
    let ret = k.b.expr(Expr::Return(None));
    let else_blk = k.b.block(&[], Some(ret));
    stmts.push(k.b.stmt(Stmt::Let {
        pat: slice_pat,
        ty: None,
        init: Some(repeat),
        else_: Some(else_blk),
    }));

    // defer print("bye");
    let print_name = k.name("print");
    let print_fn = k.b.name_expr(print_name);
    let bye = k.b.str_lit("bye");
    let call = k.b.call(print_fn, &[bye]);
    stmts.push(k.b.stmt(Stmt::Defer(call)));

    // fn helper() {}
    let hbody = k.b.block(&[], None);
    let hname = k.name("helper");
    let helper = k.b.func(hname, &[], hbody);
    stmts.push(k.b.stmt(Stmt::Item(helper)));

    // y += a; *ptr = 1 (as deref place), x.0, arr[0], &mut y, a as u8
    let y_target = k.b.use_binder(y);
    let a_use = k.b.use_binder(a);
    let compound = k.b.expr(Expr::Assign {
        target: y_target,
        op: Some(Op::new(OpKind::Add).with_overflow(Overflow::Wrap)),
        value: a_use,
    });
    stmts.push(k.b.expr_stmt(compound));
    let y_use = k.b.use_binder(y);
    let borrow = k.b.expr(Expr::Borrow {
        kind: BorrowKind::Mut,
        expr: y_use,
    });
    let deref = k.b.expr(Expr::Deref(borrow));
    let seven = k.b.int(7);
    let assign = k.b.expr(Expr::Assign {
        target: deref,
        op: None,
        value: seven,
    });
    stmts.push(k.b.expr_stmt(assign));
    let x_use = k.b.use_binder(x);
    let field = k.b.expr(Expr::Field {
        base: x_use,
        member: Member::Index(0),
        span: Span::empty(0),
    });
    let zero = k.b.int(0);
    let index = k.b.expr(Expr::Index {
        base: field,
        index: zero,
    });
    let u8_ty = k.b.ty(Ty::Prim(Prim::U8));
    let cast = k.b.expr(Expr::Cast {
        expr: index,
        ty: u8_ty,
        policy: Policy::CAST,
    });
    stmts.push(k.b.expr_stmt(cast));

    // match a { 0 => .., 1..=5 if true => .., Shape.Rect { w, .. } => .., &q | &q => .., is str s => .., null => .., _ => .. }
    let arms = match_arms(k);
    let a_scrut = k.b.use_binder(a);
    let m_expr = k.b.expr(Expr::Match {
        scrutinee: a_scrut,
        arms,
    });
    stmts.push(k.b.expr_stmt(m_expr));

    // 'outer: loop [step: y = y] { if true { break 'outer } else { continue } }
    let outer = k.binder("outer", BinderKind::Label);
    let br = k.b.expr(Expr::Break {
        label: Some(outer),
        value: None,
    });
    let cont = k.b.expr(Expr::Continue { label: None });
    let t = k.b.lit(Lit::Bool(true));
    let iff = k.b.expr(Expr::If {
        cond: t,
        then: br,
        else_: Some(cont),
    });
    let y_step = k.b.use_binder(y);
    let y_val = k.b.use_binder(y);
    let step = k.b.expr(Expr::Assign {
        target: y_step,
        op: None,
        value: y_val,
    });
    let lp = k.b.expr(Expr::Loop {
        label: Some(outer),
        body: iff,
        step: Some(step),
    });
    stmts.push(k.b.expr_stmt(lp));

    // closure use (a) implicit=infer async (v) => v + x; then spawn(it), await it
    let a_name2 = k.name("a");
    let a_outer = k.b.resolved_path(a_name2, Ns::Value, Res::Local(a));
    let a_cap = k.binder("a", BinderKind::Capture);
    let captures = k.b.list(&[Capture {
        outer: a_outer,
        binder: a_cap,
        mode: CaptureMode::ByValue,
    }]);
    let v_name = k.name("v");
    let (pv, v) = k.b.local_param(v_name);
    let (vu, xu) = (k.b.use_binder(v), k.b.use_binder(x));
    let au = k.b.use_binder(a_cap);
    let sum = k.b.op(OpKind::Add, &[vu, xu]);
    let sum2 = k.b.op(OpKind::Add, &[sum, au]);
    let cparams = k.b.list(&[pv]);
    let i64_ret = k.b.ty(Ty::Prim(Prim::I64));
    let closure = k.b.expr(Expr::Closure(Closure {
        params: cparams,
        ret: Some(i64_ret),
        body: sum2,
        effects: Effects::ASYNC,
        implicit: Some(CaptureMode::Infer),
        captures,
    }));
    let spawn = k.b.expr(Expr::Spawn(closure));
    let awaited = k.b.expr(Expr::Await(spawn));
    stmts.push(k.b.expr_stmt(awaited));

    // try { throw "e" } catch (e) { yield e } finally { 0 }
    let e_lit = k.b.str_lit("e");
    let throw = k.b.expr(Expr::Throw(e_lit));
    let e_b = k.binder("e", BinderKind::Local);
    let pe = k.b.bind(e_b);
    let str_ty = k.b.ty(Ty::Prim(Prim::Str));
    let is_str = k.b.pat(Pat::TypeTest {
        ty: str_ty,
        pat: Some(pe),
    });
    let eu = k.b.use_binder(e_b);
    let yl = k.b.expr(Expr::Yield(Some(eu)));
    let catches = k.b.list(&[Arm {
        pat: is_str,
        guard: None,
        body: yl,
    }]);
    let fin = k.b.int(0);
    let try_ = k.b.expr(Expr::Try {
        body: throw,
        catches,
        finally: Some(fin),
    });
    stmts.push(k.b.expr_stmt(try_));

    // obj.method::<i32>(1, level = 2, ...xs, **kw); Point { x: 1, ..base }; [k => v, 1]; [1, 2]
    let x_recv = k.b.use_binder(x);
    let i32_arg = k.b.ty(Ty::Prim(Prim::I32));
    let generic_args = k.b.list(&[i32_arg]);
    let (a1, a2, a3, a4) = (k.b.int(1), k.b.int(2), k.b.int(3), k.b.int(4));
    let level = k.ident("level");
    let args = k.b.list(&[
        Arg::positional(a1),
        Arg {
            kind: ArgKind::Named(level),
            value: a2,
        },
        Arg {
            kind: ArgKind::Spread,
            value: a3,
        },
        Arg {
            kind: ArgKind::SpreadNamed,
            value: a4,
        },
    ]);
    let method = k.ident("method");
    let mcall = k.b.expr(Expr::MethodCall {
        receiver: x_recv,
        method,
        generic_args,
        args,
    });
    stmts.push(k.b.expr_stmt(mcall));
    let point_name = k.name("Point");
    let point_path = k.b.name_path(point_name, Ns::Type);
    let fx = k.ident("x");
    let one = k.b.int(1);
    let inits = k.b.list(&[FieldInit {
        name: fx,
        value: one,
    }]);
    let base = k.b.use_binder(y);
    let record = k.b.expr(Expr::Record {
        path: Some(point_path),
        fields: inits,
        base: Some(base),
    });
    stmts.push(k.b.expr_stmt(record));
    let key = k.b.str_lit("k");
    let (val, val2) = (k.b.int(5), k.b.int(6));
    let entries = k.b.list(&[
        MapEntry {
            key: Some(key),
            value: val,
        },
        MapEntry {
            key: None,
            value: val2,
        },
    ]);
    let map = k.b.expr(Expr::Map(entries));
    stmts.push(k.b.expr_stmt(map));
    let half =
        k.b.lit(Lit::Float(FloatLit::new(0.5).with_suffix(Prim::F32)));
    let ch = k.b.lit(Lit::Char('\n'));
    let bytes = k.b.bytes(b"\x00\xff");
    let by = k.b.lit(Lit::Bytes(bytes));
    let big_text = k.b.text("123456789012345678901234567890");
    let big = k.b.lit(Lit::BigInt(big_text));
    let null = k.b.lit(Lit::Null);
    let arr_items = k.b.list(&[half, ch, by, big, null]);
    let arr = k.b.expr(Expr::Array(arr_items));
    stmts.push(k.b.expr_stmt(arr));
    let unsafe_tail = k.b.int(9);
    let unsafe_blk = k.b.expr(Expr::Block(Block {
        tail: Some(unsafe_tail),
        is_unsafe: true,
        ..Block::default()
    }));
    stmts.push(k.b.expr_stmt(unsafe_blk));
    stmts.push(k.b.stmt(Stmt::Err));
    let err = k.b.expr(Expr::Err);
    stmts.push(k.b.expr_stmt(err));

    // types: fn(i32) -> i32 throws str, &'r mut [u8], *const T?, dyn Show, !, _, any, [u8; M]
    let i32p = k.b.ty(Ty::Prim(Prim::I32));
    let i32r = k.b.ty(Ty::Prim(Prim::I32));
    let strt = k.b.ty(Ty::Prim(Prim::Str));
    let fparams = k.b.list(&[i32p]);
    let fn_ty = k.b.ty(Ty::Fn {
        params: fparams,
        ret: i32r,
        effects: Effects::THROWS,
        throws: Some(strt),
    });
    let u8t = k.b.ty(Ty::Prim(Prim::U8));
    let slice_t = k.b.ty(Ty::Slice(u8t));
    let ref_t = k.b.ty(Ty::Ref {
        mutable: true,
        region: None,
        inner: slice_t,
    });
    let any = k.b.ty(Ty::Any);
    let nullable = k.b.ty(Ty::Nullable(any));
    let ptr = k.b.ty(Ty::Ptr {
        mutable: false,
        inner: nullable,
    });
    let show2 = k.name("Show");
    let show2_path = k.b.name_path(show2, Ns::Type);
    let show2_ty = k.b.ty(Ty::Path(show2_path));
    let objs = k.b.list(&[show2_ty]);
    let obj = k.b.ty(Ty::Object(objs));
    let never = k.b.ty(Ty::Never);
    let infer = k.b.ty(Ty::Infer);
    let u8e = k.b.ty(Ty::Prim(Prim::U8));
    let m_name = k.name("M");
    let m_len = k.b.resolved_path(m_name, Ns::Value, Res::Local(m));
    let len = k.b.expr(Expr::Path(m_len));
    let arr_t = k.b.ty(Ty::Array { elem: u8e, len });
    let four = k.b.int(4);
    let const_arg = k.b.ty(Ty::Const(four));
    let err_t = k.b.ty(Ty::Err);
    let all = k.b.list(&[
        fn_ty, ref_t, ptr, obj, never, infer, arr_t, const_arg, err_t,
    ]);
    let all_ty = k.b.ty(Ty::Tuple(all));
    let w = k.b.pat(Pat::Wild);
    stmts.push(k.b.stmt(Stmt::Let {
        pat: w,
        ty: Some(all_ty),
        init: None,
        else_: None,
    }));

    let a_ret = k.b.use_binder(a);
    let ret_expr = k.b.expr(Expr::Return(Some(a_ret)));
    let body = k.b.block(&stmts, Some(ret_expr));
    let params = k.b.list(&[pa, p_rest, p_z]);
    let u_name2 = k.name("U");
    let u_path2 = k.b.resolved_path(u_name2, Ns::Type, Res::Local(u));
    let ret_ty = k.b.ty(Ty::Path(u_path2));
    let main_name = k.name("main");
    k.b.item(
        Item::new(
            Some(main_name),
            ItemKind::Fn(FnDef {
                generics: Generics {
                    params: params_g,
                    preds,
                },
                params,
                ret: Some(ret_ty),
                effects: Effects::THROWS.union(Effects::ASYNC).union(Effects::YIELD),
                throws: None,
                abi: None,
                body: Some(body),
            }),
        )
        .with_vis(Vis::Public),
    )
}

fn match_arms(k: &mut Kit) -> List<Arm> {
    let mut arms = Vec::new();
    let lit0 = k.b.pat(Pat::Lit(Lit::Int(IntLit::new(0))));
    let b0 = k.b.int(10);
    arms.push(Arm {
        pat: lit0,
        guard: None,
        body: b0,
    });
    let range = k.b.pat(Pat::Range {
        lo: Some(Lit::Int(IntLit::new(1))),
        hi: Some(Lit::Int(IntLit::new(5))),
        inclusive: true,
    });
    let guard = k.b.lit(Lit::Bool(true));
    let b1 = k.b.int(11);
    arms.push(Arm {
        pat: range,
        guard: Some(guard),
        body: b1,
    });
    let w = k.binder("w", BinderKind::Local);
    let pw = k.b.pat(Pat::Bind {
        binder: w,
        mode: BindMode::Ref,
        sub: None,
    });
    let wi = k.ident("w");
    let fields = k.b.list(&[FieldPat { name: wi, pat: pw }]);
    let rect = k.name("Rect");
    let rect_path = k.b.name_path(rect, Ns::Type);
    let rec_pat = k.b.pat(Pat::Record {
        path: Some(rect_path),
        fields,
        rest: true,
    });
    let wu = k.b.use_binder(w);
    arms.push(Arm {
        pat: rec_pat,
        guard: None,
        body: wu,
    });
    let q = k.binder("q", BinderKind::Local);
    let (q1, q2) = (k.b.bind(q), k.b.bind(q));
    let r1 = k.b.pat(Pat::Ref {
        mutable: false,
        inner: q1,
    });
    let circle = k.name("Circle");
    let circle_path = k.b.name_path(circle, Ns::Pattern);
    let ctor_elems = k.b.list(&[q2]);
    let ctor = k.b.pat(Pat::Ctor {
        path: circle_path,
        elems: ctor_elems,
        rest: None,
    });
    let alts = k.b.list(&[r1, ctor]);
    let or = k.b.pat(Pat::Or(alts));
    let qu = k.b.use_binder(q);
    arms.push(Arm {
        pat: or,
        guard: None,
        body: qu,
    });
    let empty = k.name("Empty");
    let empty_path = k.b.name_path(empty, Ns::Pattern);
    let path_pat = k.b.pat(Pat::Path(empty_path));
    let b4 = k.b.int(14);
    arms.push(Arm {
        pat: path_pat,
        guard: None,
        body: b4,
    });
    let null = k.b.pat(Pat::Lit(Lit::Null));
    let b5 = k.b.int(15);
    arms.push(Arm {
        pat: null,
        guard: None,
        body: b5,
    });
    let err = k.b.pat(Pat::Err);
    let b6 = k.b.int(16);
    arms.push(Arm {
        pat: err,
        guard: None,
        body: b6,
    });
    let wild = k.b.pat(Pat::Wild);
    let b7 = k.b.int(17);
    arms.push(Arm {
        pat: wild,
        guard: None,
        body: b7,
    });
    k.b.list(&arms)
}

#[test]
fn test_kitchen_sink_validates_and_walks_every_node() {
    let (hir, _names) = kitchen_sink();
    let mut seen = 0usize;
    hir_lang::walk(&hir, |_| seen += 1);
    let total: usize = [
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
    .sum();
    assert_eq!(seen, total, "the tree reaches every node exactly once");
}

#[test]
fn test_kitchen_sink_print_snapshot() {
    let (hir, names) = kitchen_sink();
    let text = hir_lang::print(&hir, &names);
    assert_eq!(text, SNAPSHOT.trim_end());
}

#[test]
fn test_kitchen_sink_is_deterministic() {
    let (a, na) = kitchen_sink();
    let (b, nb) = kitchen_sink();
    assert_eq!(a, b);
    assert_eq!(hir_lang::print(&a, &na), hir_lang::print(&b, &nb));
    assert_eq!(format!("{a:?}"), format!("{b:?}"));
}

#[test]
fn test_kitchen_sink_origins_print() {
    let (hir, names) = kitchen_sink();
    let text = hir_lang::print_with(
        &hir,
        &names,
        hir_lang::PrintOptions::default().with_origins(true),
    );
    // Every node line carries its origin; group lines (arm, guard, ...) do not.
    let node_lines = text.lines().filter(|l| l.contains(" @")).count();
    assert!(node_lines > 100);
    assert!(text.starts_with("(module app @0..0"));
}

const SNAPSHOT: &str = r##"(module app
  (import io (path ::std::io))
  (import glob (path std))
  (record Point pub
    (field x
      (prim i32))
    (field y pub
      (prim i32)
      (default
        (lit 0))))
  (sum Shape
    (generic T%0 type-param)
    (variant Circle tuple
      (field
        (prim f64)))
    (variant Rect
      (field w
        (prim f64)))
    (variant Empty unit
      (discriminant
        (lit 3))))
  (interface Show
    (fn show
      (param receiver
        (bind self%1)
        (self-type))
      (ret
        (prim str)))
    (const N)
    (assoc-type Out
      (default
        (prim i32))))
  (impl
    (interface
      (type (path Show)))
    (type (path Point → item 2))
    (fn show
      (param receiver
        (bind self%2))
      (block
        (lit "p")))
    (const N
      (lit 1))
    (alias Out
      (prim i32)))
  (class Animal abstract
    (field name)
    (fn speak
      (param receiver
        (bind self%3)))
    (global COUNT mut
      (lit 0)))
  (const MAX
    (prim i64)
    (lit 100i64))
  (alias Id
    (prim u64))
  (module inner)
  (fn main pub throws async yield @inline @budget(cpu = 10, C)
    (generic U%4 type-param)
    (generic M%5 const-param
      (type
        (prim usize)))
    (where
      (type (path U → local U%4))
      (type (path Show)))
    (param normal
      (bind a%6))
    (param rest
      (bind rest%7))
    (param named-only
      (bind z%8)
      (default
        (lit 1)))
    (ret
      (type (path U → local U%4)))
    (block
      (let
        (tuple
          (bind x%9)
          (bind y%10))
        (tuple-type
          (prim i32)
          (prim i32))
        (tuple
          (lit 1)
          (lit 2)))
      (let
        (slice
          (bind first%11)
          (rest
            (as
              (bind mid%12))
            (bind last%13)))
        (repeat
          (lit 1)
          (lit 3))
        (else
          (block
            (return))))
      (defer
        (call
          (use (path print))
          (lit "bye")))
      (decl
        (fn helper
          (block)))
      (do
        (assign add overflow=wrap
          (use (path y → local y%10))
          (use (path a → local a%6))))
      (do
        (assign
          (deref
            (borrow mut
              (use (path y → local y%10))))
          (lit 7)))
      (do
        (cast overflow=error float_to_int=error
          (index
            (field 0
              (use (path x → local x%9)))
            (lit 0))
          (prim u8)))
      (do
        (match
          (use (path a → local a%6))
          (arm
            (lit 0)
            (lit 10))
          (arm
            (range 1..=5)
            (guard
              (lit true))
            (lit 11))
          (arm
            (record .. (path Rect)
              (field w
                (bind w%14 ref)))
            (use (path w → local w%14)))
          (arm
            (or
              (ref
                (bind q%15))
              (ctor (path Circle)
                (bind q%15)))
            (use (path q → local q%15)))
          (arm
            (pat (path Empty))
            (lit 14))
          (arm
            (lit null)
            (lit 15))
          (arm
            (error)
            (lit 16))
          (arm
            (wild)
            (lit 17))))
      (do
        (loop 'outer%16
          (if
            (lit true)
            (break 'outer%16)
            (continue))
          (step
            (assign
              (use (path y → local y%10))
              (use (path y → local y%10))))))
      (do
        (await
          (spawn
            (closure implicit=infer async
              (capture a%17 by-value (path a → local a%6))
              (param normal
                (bind v%18))
              (ret
                (prim i64))
              (op add overflow=error
                (op add overflow=error
                  (use (path v → local v%18))
                  (use (path x → local x%9)))
                (use (path a → local a%17)))))))
      (do
        (try
          (throw
            (lit "e"))
          (catch
            (is
              (prim str)
              (bind e%19))
            (yield
              (use (path e → local e%19))))
          (finally
            (lit 0))))
      (do
        (method method
          (use (path x → local x%9))
          (prim i32)
          (lit 1)
          (arg level
            (lit 2))
          (spread
            (lit 3))
          (spread-named
            (lit 4))))
      (do
        (record (path Point)
          (init x
            (lit 1))
          (base
            (use (path y → local y%10)))))
      (do
        (map
          (entry
            (lit "k")
            (lit 5))
          (entry
            (lit 6))))
      (do
        (array
          (lit 0.5f32)
          (lit '\n')
          (lit b"\x00\xff")
          (lit 123456789012345678901234567890n)
          (lit null)))
      (do
        (block unsafe
          (lit 9)))
      (error)
      (do
        (error))
      (let
        (wild)
        (tuple-type
          (fn-type throws
            (prim i32)
            (ret
              (prim i32))
            (throws
              (prim str)))
          (ref-type mut
            (slice-type
              (prim u8)))
          (ptr
            (nullable
              (any)))
          (object
            (type (path Show)))
          (never)
          (infer)
          (array-type
            (prim u8)
            (use (path M → local M%5)))
          (const-arg
            (lit 4))
          (error)))
      (return
        (use (path a → local a%6))))))
"##;
