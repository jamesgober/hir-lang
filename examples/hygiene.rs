//! Lowering a Mox-style `foreach` through a desugaring template, then
//! resolving the user's names with a toy resolver.
//!
//! The template introduces a temporary `it`; the user's code also has a
//! variable `it`. The template's name carries the expansion's mark, and the
//! template's own reference is resolved by the lowering itself, so the two can
//! never be confused: the resolver below matches names by symbol *and* mark,
//! and `Hir::can_reference` keeps it to binders in scope.
//!
//! ```sh
//! cargo run --example hygiene
//! ```

use hir_lang::{
    Arm, BinderKind, Builder, Expansion, ExpnId, ExpnKind, Expr, HirError, IdKind, Lit, Name, Ns,
    Pat, PathId, Res, Span,
};
use intern_lang::Interner;

fn main() -> Result<(), HirError> {
    let mut names = Interner::new();
    let it = names.intern("it");
    let mut b = Builder::new();

    // User code: let it = 10;
    b.set_span(Span::new(0, 12));
    let user_it = b.new_binder(Name::new(it), BinderKind::Local);
    let user_pat = b.bind(user_it);
    let ten = b.int(10);
    let user_let = b.let_stmt(user_pat, Some(ten));

    // foreach ($items as $v) { echo it; }  — desugared by a template.
    let foreach = b.expansion(Expansion {
        kind: ExpnKind::Desugar,
        name: names.intern("foreach"),
        call_site: Span::new(13, 50),
        parent: ExpnId::ROOT,
        def_site: ExpnId::ROOT,
    });
    b.set_expansion(foreach);
    let tmp = b.new_binder(Name::marked(it, foreach), BinderKind::Local);
    let tmp_pat = b.bind(tmp);
    let items = b.name_expr(Name::new(names.intern("items")));
    let iter = b.name_expr(Name::new(names.intern("iter")));
    let start = b.call(iter, &[items]);
    let tmp_let = b.let_stmt(tmp_pat, Some(start));

    let next = b.name_expr(Name::new(names.intern("next")));
    let tmp_use = b.use_binder(tmp); // resolved by the template: hygiene by construction
    let step = b.call(next, &[tmp_use]);
    let v = b.new_binder(Name::new(names.intern("v")), BinderKind::Local);
    let v_pat = b.bind(v);
    // The loop body is user text substituted into the template: ROOT mark.
    b.set_expansion(ExpnId::ROOT);
    let echo = b.name_expr(Name::new(names.intern("echo")));
    let user_ref = b.name_expr(Name::new(it)); // which `it`? the resolver decides
    let body = b.call(echo, &[user_ref]);
    b.set_expansion(foreach);
    let null = b.pat(Pat::Lit(Lit::Null));
    let stop = b.expr(Expr::Break {
        label: None,
        value: None,
    });
    let arms = b.list(&[
        Arm {
            pat: null,
            guard: None,
            body: stop,
        },
        Arm {
            pat: v_pat,
            guard: None,
            body,
        },
    ]);
    let m = b.expr(Expr::Match {
        scrutinee: step,
        arms,
    });
    let lp = b.expr(Expr::Loop {
        label: None,
        body: m,
        step: None,
    });
    let desugared = b.block(&[tmp_let], Some(lp));
    b.set_expansion(ExpnId::ROOT);
    let s2 = b.expr_stmt(desugared);
    let main_body = b.block(&[user_let, s2], None);
    let main = b.func(Name::new(names.intern("main")), &[], main_body);
    let root = b.module(None, &[main]);
    let mut hir = b.finish(root)?;

    // A toy resolver: for each unresolved value path, pick the innermost
    // binder with the same name (symbol and mark) that the path can reference.
    for i in 0..hir.count(IdKind::Path) {
        let Some(path) = PathId::from_index(i) else {
            continue;
        };
        let p = *hir.path(path);
        if !p.res.is_unresolved() || p.ns != Ns::Value {
            continue;
        }
        let Some(name) = hir.list(p.segments).first().map(|s| s.name) else {
            continue;
        };
        let found = (0..hir.count(IdKind::Binder))
            .rev()
            .filter_map(hir_lang::BinderId::from_index)
            .find(|b| {
                hir.binder(*b).is_some_and(|x| x.name == name) && hir.can_reference(path, *b)
            });
        if let Some(binder) = found {
            hir.resolve(path, Res::Local(binder))?;
        }
    }

    let Expr::Path(user_path) = *hir.expr(user_ref) else {
        return Ok(());
    };
    assert_eq!(hir.path(user_path).res, Res::Local(user_it));
    println!("the user's `it` resolved to the user's binder, not the template's");
    println!("{}", hir_lang::print(&hir, &names));
    Ok(())
}
