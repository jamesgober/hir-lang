//! # hir_lang
//!
//! The high-level intermediate representation (HIR) of LexerSketch: the one
//! structured, expression-oriented IR that every forged language lowers into,
//! so that name resolution, type checking, pattern compilation, capabilities,
//! the evaluator, and the lowering to SSA are written once.
//!
//! The full definition is `specs/HIR.md` in the LexerSketch plan; this crate
//! implements its node forms, storage, builder, validator, traversal, and
//! debug printer.
//!
//! ## The lazy path
//!
//! Build bottom-up with a [`Builder`] (every node captures the builder's
//! current origin), [`finish`](Builder::finish) to validate, then read, walk,
//! or print:
//!
//! ```
//! use hir_lang::{Builder, Name, OpKind, Span};
//! use intern_lang::Interner;
//!
//! let mut names = Interner::new();
//! let mut b = Builder::new();
//!
//! // fn double(n) { n * 2 }
//! b.set_span(Span::new(10, 11));
//! let (param, n) = b.local_param(Name::new(names.intern("n")));
//! b.set_span(Span::new(15, 20));
//! let use_n = b.use_binder(n);
//! let two = b.int(2);
//! let product = b.op(OpKind::Mul, &[use_n, two]);
//! let body = b.block(&[], Some(product));
//! let double = b.func(Name::new(names.intern("double")), &[param], body);
//! let root = b.module(None, &[double]);
//!
//! let hir = b.finish(root)?;
//!
//! let mut nodes = 0;
//! hir_lang::walk(&hir, |_| nodes += 1);
//! assert_eq!(nodes, 9); // module, fn, param, pattern, block, op, use, path, literal
//! assert!(hir_lang::print(&hir, &names).contains("(op mul overflow=error"));
//! # Ok::<(), hir_lang::HirError>(())
//! ```
//!
//! ## What a `Hir` guarantees
//!
//! A [`Hir`] exists only after the validator accepted it, and the validator is
//! total: on any arena it accepts or returns a precise [`HirError`], never
//! panics, and runs in linear time. Accepted HIR forms one tree; every id
//! resolves; every binder is bound exactly once and referenced only where it
//! is in scope and reachable across frames; every jump has a target; effect
//! forms sit only in frames that allow them; every op carries exactly the
//! policy fields `specs/OPS.md` says it consults. Storage is flat, so cloning,
//! comparing, printing, walking, and dropping never recurse, at any depth.
//!
//! ## `no_std`
//!
//! The crate needs only `alloc`. The default `std` feature is additive.

#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(unused_must_use)]
#![deny(unused_results)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::todo)]
#![deny(clippy::unimplemented)]
#![deny(clippy::unreachable)]
#![deny(clippy::print_stdout)]
#![deny(clippy::print_stderr)]
#![deny(clippy::dbg_macro)]

extern crate alloc;

mod builder;
mod copy;
mod def;
mod error;
mod expr;
mod hir;
mod id;
mod intrinsic;
mod item;
mod lit;
mod name;
mod ops;
mod origin;
mod pat;
mod print;
mod store;
mod ty;
mod validate;
mod walk;

pub use builder::Builder;
pub use def::{Def, DefId, UnitId};
pub use error::{Capacity, EffectProblem, HirError, JumpProblem, Malformed, Site};
pub use expr::{
    Arg, ArgKind, Arm, Block, BorrowKind, Capture, CaptureMode, Closure, DefaultEval, Expr,
    FieldInit, MapEntry, Member, Stmt,
};
pub use hir::Hir;
pub use id::{
    BinderId, ExprId, FieldId, IdKind, ItemId, List, NodeRef, ParamId, PatId, PathId, StmtId,
    TextRef, TyId, VariantId,
};
pub use intern_lang::Symbol;
pub use intrinsic::{Asm, AsmDir, AsmOperand, AsmOptions, Intrinsic, MemOrder, RmwOp};
pub use item::{
    Attr, AttrArg, AttrValue, ClassDef, FieldDef, FnDef, GenericParam, Generics, ImplDef,
    InterfaceDef, Item, ItemKind, MixinAction, MixinRule, MixinUseDef, Param, ParamKind, RecordDef,
    Shape, SumDef, Variant, Vis, WherePred,
};
pub use lit::{FloatLit, IntLit, Lit, Prim};
pub use name::{Binder, BinderKind, Ns, Path, PathRoot, QSelf, Res, Segment};
pub use ops::{DivZero, FloatToInt, Op, OpKind, Overflow, Policy, Shift};
pub use origin::{Expansion, ExpnId, ExpnKind, Ident, Name, Origin};
pub use pat::{BindMode, FieldPat, Pat, SliceRest};
pub use print::{PrintOptions, print, print_into, print_with};
pub use span_lang::Span;
pub use store::Pooled;
pub use ty::{Bound, Effects, GenericArg, Ty};
pub use walk::{Control, Event, Frame};

/// Visits every node of `hir` in canonical preorder, from the root.
///
/// The lazy path for analyses that look at nodes one at a time. For enter and
/// leave events, skipping, or stopping, use [`Hir::walk_from`].
///
/// # Examples
///
/// ```
/// use hir_lang::{Builder, NodeRef};
///
/// let mut b = Builder::new();
/// let root = b.module(None, &[]);
/// let hir = b.finish(root)?;
/// let mut seen = Vec::new();
/// hir_lang::walk(&hir, |node| seen.push(node));
/// assert_eq!(seen, [NodeRef::Item(root)]);
/// # Ok::<(), hir_lang::HirError>(())
/// ```
pub fn walk<F: FnMut(NodeRef)>(hir: &Hir, f: F) {
    walk::walk_nodes(hir.store(), NodeRef::Item(hir.root()), f);
}

/// Compiles and runs the `rust` code blocks in `README.md` and `docs/API.md` as
/// part of `cargo test`, so the published examples cannot drift from the API.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
#[doc = include_str!("../docs/API.md")]
pub struct MarkdownDocTests;

#[cfg(test)]
mod tests {
    use super::*;

    /// The README states node sizes; keep them from growing unnoticed.
    #[test]
    fn test_node_sizes_stay_small() {
        let sizes = [
            core::mem::size_of::<Expr>(),
            core::mem::size_of::<Pat>(),
            core::mem::size_of::<Ty>(),
            core::mem::size_of::<Stmt>(),
            core::mem::size_of::<Path>(),
            core::mem::size_of::<Item>(),
            core::mem::size_of::<Origin>(),
            core::mem::size_of::<Option<ExprId>>(),
        ];
        // Paths grew in 0.3 (root, qualified self, partial resolution, a
        // unit-qualified `DefId`); patterns shrank (range bounds are nodes).
        assert_eq!(sizes, [40, 32, 24, 20, 40, 72, 12, 4]);
    }
}
