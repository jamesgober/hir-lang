//! The Tier-1 path: build a function bottom-up, finish (validate), walk, and
//! print it.
//!
//! ```sh
//! cargo run --example quickstart
//! ```

use hir_lang::{Builder, HirError, Name, NodeRef, OpKind, Span};
use intern_lang::Interner;

fn main() -> Result<(), HirError> {
    let mut names = Interner::new();
    let mut b = Builder::new();

    // fn area(w, h) { w * h }   (source offsets as a lowering would set them)
    b.set_span(Span::new(8, 9));
    let (pw, w) = b.local_param(Name::new(names.intern("w")));
    b.set_span(Span::new(11, 12));
    let (ph, h) = b.local_param(Name::new(names.intern("h")));
    b.set_span(Span::new(16, 21));
    let (uw, uh) = (b.use_binder(w), b.use_binder(h));
    let product = b.op(OpKind::Mul, &[uw, uh]);
    let body = b.block(&[], Some(product));
    b.set_span(Span::new(0, 23));
    let area = b.func(Name::new(names.intern("area")), &[pw, ph], body);
    let root = b.module(None, &[area]);

    let hir = b.finish(root)?;

    let mut nodes = 0;
    hir_lang::walk(&hir, |_| nodes += 1);
    println!(
        "{nodes} nodes; `w * h` spans {}",
        hir.origin(NodeRef::Expr(product)).span
    );
    println!("{}", hir_lang::print(&hir, &names));
    Ok(())
}
