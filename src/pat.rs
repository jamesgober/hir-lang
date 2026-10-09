//! Patterns. Patterns are expression-free: an arm's guard belongs to the arm.

use crate::{
    id::{BinderId, List, PatId, PathId, TyId},
    lit::Lit,
    origin::Ident,
};

/// How a binding pattern binds its value.
///
/// # Examples
///
/// ```
/// use hir_lang::BindMode;
///
/// assert_ne!(BindMode::Value, BindMode::Ref);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BindMode {
    /// By value (move or copy, decided by type).
    #[default]
    Value,
    /// By shared reference (`ref x`).
    Ref,
    /// By mutable reference (`ref mut x`).
    RefMut,
}

/// One `name: pattern` entry of a record pattern.
///
/// # Examples
///
/// ```
/// use hir_lang::{Builder, FieldPat, Ident, Pat, Span};
/// use intern_lang::Interner;
///
/// let mut names = Interner::new();
/// let mut b = Builder::new();
/// let wild = b.pat(Pat::Wild);
/// let entry = FieldPat { name: Ident::new(names.intern("x"), Span::empty(0)), pat: wild };
/// assert_eq!(entry.pat, wild);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FieldPat {
    /// The field name.
    pub name: Ident,
    /// The pattern for the field's value.
    pub pat: PatId,
}

/// The `..` part of a slice pattern: an optional pattern for the middle and the
/// elements after it.
///
/// # Examples
///
/// ```
/// use hir_lang::{List, SliceRest};
///
/// let rest = SliceRest { bind: None, suffix: List::EMPTY };
/// assert!(rest.suffix.is_empty());
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SliceRest {
    /// A pattern matched against the middle sub-slice (`rest @ ..`), if any.
    pub bind: Option<PatId>,
    /// Elements after the `..`.
    pub suffix: List<PatId>,
}

/// A pattern; see the spec, §6.
///
/// # Examples
///
/// ```
/// use hir_lang::{BindMode, Binder, BinderKind, Builder, Name, Pat};
/// use intern_lang::Interner;
///
/// let mut names = Interner::new();
/// let mut b = Builder::new();
/// let x = b.binder(Binder::new(Name::new(names.intern("x")), BinderKind::Local));
/// let bind = b.pat(Pat::Bind { binder: x, mode: BindMode::Value, sub: None });
/// let wild = b.pat(Pat::Wild);
/// let pair = b.list(&[bind, wild]);
/// let tuple = b.pat(Pat::Tuple { elems: pair, rest: None });
/// assert_ne!(tuple, bind);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pat {
    /// `_`.
    Wild,
    /// Binds the matched value to `binder`, optionally matching `sub` too
    /// (`x @ sub`).
    Bind {
        /// The binder (kind `Local` or `Param` by site).
        binder: BinderId,
        /// By value or by reference.
        mode: BindMode,
        /// A sub-pattern the value must also match.
        sub: Option<PatId>,
    },
    /// Equal to a literal.
    Lit(Lit),
    /// A bare identifier whose meaning resolve-lang decides: if `path`
    /// resolves to a constant, unit variant, or unit record, the pattern
    /// matches that value; otherwise (unresolved or `Err`) it binds `binder`.
    /// Structurally it always counts as `binder`'s binding site.
    Ident {
        /// The binder (kind `Local` or `Param` by site).
        binder: BinderId,
        /// The same name as a `Pattern`-namespace path.
        path: PathId,
    },
    /// Within a range. Each bound is a child pattern that is a `Lit` or a
    /// `Path` (a constant); at least one bound; literal bounds are of one
    /// class (integer, char, or float).
    Range {
        /// Lower bound, inclusive.
        lo: Option<PatId>,
        /// Upper bound.
        hi: Option<PatId>,
        /// `..=` rather than `..`.
        inclusive: bool,
    },
    /// A tuple; `rest` is the position of `..`, at most the element count.
    Tuple {
        /// Element patterns.
        elems: List<PatId>,
        /// Where `..` sits, if present.
        rest: Option<u32>,
    },
    /// A tuple-like constructor: `Some(x)`, `Point(x, y)`.
    Ctor {
        /// The constructor (`Pattern` namespace).
        path: PathId,
        /// Field patterns.
        elems: List<PatId>,
        /// Where `..` sits, if present.
        rest: Option<u32>,
    },
    /// A record or struct-like variant: `P { x, y: 0, .. }`; no path destructures
    /// an anonymous record or object.
    Record {
        /// The record or variant (`Type` namespace), if named.
        path: Option<PathId>,
        /// Field patterns, unique by name.
        fields: List<FieldPat>,
        /// Whether `..` ignores the remaining fields.
        rest: bool,
    },
    /// A unit variant, unit record, or constant (`Pattern` namespace).
    Path(PathId),
    /// A slice or array: `[a, b, rest @ .., z]`.
    Slice {
        /// Elements before the `..` (all of them, when there is no `..`).
        prefix: List<PatId>,
        /// The `..` and what follows it.
        rest: Option<SliceRest>,
    },
    /// Any alternative; each alternative binds the same binders.
    Or(List<PatId>),
    /// Dereference a borrow: `&p`, `&mut p`.
    Ref {
        /// `&mut`.
        mutable: bool,
        /// The pattern for the referent.
        inner: PatId,
    },
    /// The value's type is `ty` (runtime type for dynamic languages); used by
    /// `catch` clauses and type-switching `match`.
    TypeTest {
        /// The tested type.
        ty: TyId,
        /// A pattern the value must then match.
        pat: Option<PatId>,
    },
    /// A pattern that failed to lower.
    Err,
}
