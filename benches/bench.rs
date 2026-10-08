//! Criterion benchmarks at realistic scale (100k to 1M nodes).
//!
//! The workload is a module of functions shaped like lowered code: each takes
//! two parameters, binds a run of locals from arithmetic on earlier ones (every
//! reference resolved, so every one is scope-checked), matches on a value, and
//! loops with a labeled break. Node counts are printed in the benchmark ids.

use std::hint::black_box;

use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use hir_lang::{
    Arm, Binder, BinderKind, Builder, Expr, Hir, IdKind, IntLit, ItemId, Lit, Name, NodeRef,
    OpKind, Pat, PathId, Res,
};
use intern_lang::Interner;

/// Builds `funcs` functions with `lets` locals each.
fn program(names: &mut Interner, funcs: usize, lets: usize) -> (Builder, ItemId) {
    let x = Name::new(names.intern("x"));
    let mut b = Builder::new();
    let mut items = Vec::with_capacity(funcs);
    for f in 0..funcs {
        let (pa, a) = b.local_param(x);
        let (pb, p) = b.local_param(x);
        let mut stmts = Vec::with_capacity(lets + 2);
        let mut prev = a;
        for i in 0..lets {
            let local = b.binder(Binder::new(x, BinderKind::Local));
            let lhs = b.use_binder(prev);
            let rhs = b.use_binder(p);
            let three = b.int(i as i64);
            let scaled = b.op(OpKind::Mul, &[rhs, three]);
            let sum = b.op(OpKind::Add, &[lhs, scaled]);
            let pat = b.bind(local);
            stmts.push(b.let_stmt(pat, Some(sum)));
            prev = local;
        }
        // match prev { 0 => a, _ => p }
        let scrutinee = b.use_binder(prev);
        let zero = b.pat(Pat::Lit(Lit::Int(IntLit::new(0))));
        let wild = b.pat(Pat::Wild);
        let (ua, up) = (b.use_binder(a), b.use_binder(p));
        let arms = b.list(&[
            Arm {
                pat: zero,
                guard: None,
                body: ua,
            },
            Arm {
                pat: wild,
                guard: None,
                body: up,
            },
        ]);
        let m = b.expr(Expr::Match { scrutinee, arms });
        stmts.push(b.expr_stmt(m));
        // 'l: loop { break 'l prev }
        let label = b.binder(Binder::new(x, BinderKind::Label));
        let value = b.use_binder(prev);
        let br = b.expr(Expr::Break {
            label: Some(label),
            value: Some(value),
        });
        let lp = b.expr(Expr::Loop {
            label: Some(label),
            body: br,
            step: None,
        });
        let body = b.block(&stmts, Some(lp));
        let name = Name::new(names.intern(&format!("f{f}")));
        items.push(b.func(name, &[pa, pb], body));
    }
    let root = b.module(None, &items);
    (b, root)
}

fn node_count(hir: &Hir) -> usize {
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

fn scales(c: &mut Criterion) {
    // About 100k and 1M nodes: 13 nodes per local plus per-function overhead.
    for (label, funcs, lets) in [("100k", 750, 10), ("1m", 7_500, 10)] {
        let mut names = Interner::new();
        let (b, root) = program(&mut names, funcs, lets);
        let hir = b.finish(root).expect("benchmark program validates");
        let nodes = node_count(&hir);
        let mut group = c.benchmark_group(format!("{label}_{nodes}_nodes"));
        group.throughput(Throughput::Elements(nodes as u64));
        group.sample_size(20);

        group.bench_function("build", |bench| {
            bench.iter(|| {
                let mut names = Interner::new();
                black_box(program(&mut names, funcs, lets))
            });
        });
        group.bench_function("validate", |bench| {
            bench.iter_batched(
                || {
                    let mut names = Interner::new();
                    program(&mut names, funcs, lets)
                },
                |(b, root)| black_box(b.finish(root)),
                BatchSize::LargeInput,
            );
        });
        group.bench_function("walk", |bench| {
            bench.iter(|| {
                let mut n = 0usize;
                hir_lang::walk(&hir, |_| n += 1);
                black_box(n)
            });
        });
        group.bench_function("print", |bench| {
            bench.iter(|| black_box(hir_lang::print(&hir, &names).len()));
        });
        group.bench_function("clone_eq_drop", |bench| {
            bench.iter(|| {
                let copy = hir.clone();
                black_box(copy == hir)
            });
        });
        let paths = hir.count(IdKind::Path);
        let targets: Vec<(PathId, Res)> = (0..paths)
            .map(|i| {
                let p = PathId::from_index(i).unwrap();
                (p, hir.path(p).res)
            })
            .collect();
        group.bench_function("resolve_every_path", |bench| {
            bench.iter_batched_ref(
                || hir.clone(),
                |h| {
                    for (p, res) in &targets {
                        black_box(h.resolve(*p, *res)).ok();
                    }
                },
                BatchSize::LargeInput,
            );
        });
        group.finish();
    }
}

fn deep(c: &mut Criterion) {
    let mut group = c.benchmark_group("deep_1m_chain");
    group.sample_size(10);
    group.throughput(Throughput::Elements(1_000_000));
    let build = || {
        let mut names = Interner::new();
        let mut b = Builder::new();
        let mut e = b.int(1);
        for _ in 0..1_000_000 {
            e = b.op(OpKind::Neg, &[e]);
        }
        let body = b.block(&[], Some(e));
        let f = b.func(Name::new(names.intern("deep")), &[], body);
        let root = b.module(None, &[f]);
        (b, root)
    };
    group.bench_function("validate", |bench| {
        bench.iter_batched(
            build,
            |(b, root)| black_box(b.finish(root)),
            BatchSize::LargeInput,
        );
    });
    let (b, root) = build();
    let hir = b.finish(root).expect("deep chain validates");
    group.bench_function("walk", |bench| {
        bench.iter(|| {
            let mut n = 0usize;
            hir.walk_from(NodeRef::Item(hir.root()), |_| {
                n += 1;
                hir_lang::Control::Continue
            });
            black_box(n)
        });
    });
    group.finish();
}

criterion_group!(benches, scales, deep);
criterion_main!(benches);
