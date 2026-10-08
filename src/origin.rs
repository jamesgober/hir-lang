//! Origins, expansion records, and hygienic names.
//!
//! Every node remembers where it came from: a source span plus the expansion
//! (macro, lowering template, or built-in desugaring) that produced it. Names
//! carry a hygiene mark, so a template's temporaries can never be confused with
//! the user's variables of the same spelling.

use core::fmt;

use intern_lang::Symbol;
use span_lang::Span;

/// The id of an expansion record, or [`ExpnId::ROOT`] for text written directly
/// in source.
///
/// Expansion ids double as hygiene marks (see [`Name`]): the text an expansion
/// writes is marked with that expansion's id.
///
/// # Examples
///
/// ```
/// use hir_lang::ExpnId;
///
/// assert!(ExpnId::ROOT.is_root());
/// assert_eq!(ExpnId::ROOT.as_u32(), 0);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ExpnId(u32);

impl ExpnId {
    /// Source text: no expansion produced it.
    pub const ROOT: Self = Self(0);

    /// Returns `true` for [`ExpnId::ROOT`].
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::ExpnId;
    ///
    /// assert!(ExpnId::from_u32(0).is_root());
    /// assert!(!ExpnId::from_u32(1).is_root());
    /// ```
    #[must_use]
    pub const fn is_root(self) -> bool {
        self.0 == 0
    }

    /// Returns the raw id: `0` for the root, `n` for the `n`-th expansion record.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::ExpnId;
    ///
    /// assert_eq!(ExpnId::from_u32(4).as_u32(), 4);
    /// ```
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self.0
    }

    /// Rebuilds an expansion id from its raw value (for decoders and tests).
    ///
    /// Whether the id names a record is checked by the validator wherever it is
    /// stored in a `Hir`.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::ExpnId;
    ///
    /// assert_eq!(ExpnId::from_u32(0), ExpnId::ROOT);
    /// ```
    #[must_use]
    pub const fn from_u32(raw: u32) -> Self {
        Self(raw)
    }
}

impl fmt::Debug for ExpnId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ExpnId({})", self.0)
    }
}

/// Where a node came from: its source span and the expansion that produced it.
///
/// The builder stamps its *current origin* onto every node and binder it
/// creates, so no node exists without one.
///
/// # Examples
///
/// ```
/// use hir_lang::{ExpnId, Origin, Span};
///
/// let origin = Origin::new(Span::new(10, 20));
/// assert_eq!(origin.expn, ExpnId::ROOT);
/// assert_eq!(Origin::default().span, Span::empty(0));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Origin {
    /// The source span, in the session's global byte space.
    pub span: Span,
    /// The expansion that produced the node; [`ExpnId::ROOT`] for source text.
    pub expn: ExpnId,
}

impl Origin {
    /// An origin in source text (no expansion).
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Origin, Span};
    ///
    /// assert!(Origin::new(Span::new(0, 3)).expn.is_root());
    /// ```
    #[must_use]
    pub const fn new(span: Span) -> Self {
        Self {
            span,
            expn: ExpnId::ROOT,
        }
    }

    /// An origin produced by the expansion `expn`.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{ExpnId, Origin, Span};
    ///
    /// let origin = Origin::expanded(Span::new(0, 3), ExpnId::from_u32(2));
    /// assert_eq!(origin.expn.as_u32(), 2);
    /// ```
    #[must_use]
    pub const fn expanded(span: Span, expn: ExpnId) -> Self {
        Self { span, expn }
    }
}

impl Default for Origin {
    fn default() -> Self {
        Self::new(Span::empty(0))
    }
}

/// What kind of expansion produced some HIR.
///
/// # Examples
///
/// ```
/// use hir_lang::ExpnKind;
///
/// assert_ne!(ExpnKind::Macro, ExpnKind::Desugar);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExpnKind {
    /// A user macro invocation; the record's `name` is the macro's name.
    Macro,
    /// A lowering template from the language's sketch; `name` is the template's.
    Template,
    /// A built-in desugaring (`while`, `for`, `foreach`, `?`); `name` says which.
    Desugar,
}

/// One expansion: what produced it, where it was invoked, and how it nests.
///
/// Records are created with [`Builder::expansion`](crate::Builder::expansion) and
/// may refer only to earlier expansions, so every backtrace terminates.
///
/// # Examples
///
/// ```
/// use hir_lang::{Builder, ExpnId, ExpnKind, Expansion, Span};
/// use intern_lang::Interner;
///
/// let mut names = Interner::new();
/// let mut b = Builder::new();
/// let expn = b.expansion(Expansion {
///     kind: ExpnKind::Desugar,
///     name: names.intern("while"),
///     call_site: Span::new(0, 30),
///     parent: ExpnId::ROOT,
///     def_site: ExpnId::ROOT,
/// });
/// assert_eq!(expn.as_u32(), 1);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Expansion {
    /// Macro, template, or desugaring.
    pub kind: ExpnKind,
    /// The macro's, template's, or construct's name.
    pub name: Symbol,
    /// The span of the invocation or sugared construct.
    pub call_site: Span,
    /// The expansion the call site itself came from; following `parent` to the
    /// root is the "in this expansion" backtrace.
    pub parent: ExpnId,
    /// The hygiene context of the template's own text at its definition: the
    /// definition-site fallback for names the expansion does not bind.
    pub def_site: ExpnId,
}

/// A hygienic name: a symbol plus the mark of the expansion that wrote it.
///
/// Two names are the same identifier only if both fields agree. User-written
/// names carry [`ExpnId::ROOT`]; a template's own names carry the template's
/// expansion id, so its temporaries can neither capture nor be captured by a
/// user variable with the same spelling.
///
/// # Examples
///
/// ```
/// use hir_lang::{ExpnId, Name};
/// use intern_lang::Interner;
///
/// let mut names = Interner::new();
/// let it = names.intern("it");
/// let user = Name::new(it);
/// let template = Name::marked(it, ExpnId::from_u32(1));
/// assert_ne!(user, template);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Name {
    /// The spelling.
    pub sym: Symbol,
    /// The hygiene mark: the expansion that wrote the name.
    pub mark: ExpnId,
}

impl Name {
    /// A name written directly in source (mark [`ExpnId::ROOT`]).
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::Name;
    /// use intern_lang::Interner;
    ///
    /// let mut names = Interner::new();
    /// assert!(Name::new(names.intern("x")).mark.is_root());
    /// ```
    #[must_use]
    pub const fn new(sym: Symbol) -> Self {
        Self {
            sym,
            mark: ExpnId::ROOT,
        }
    }

    /// A name written by the expansion `mark`.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{ExpnId, Name};
    /// use intern_lang::Interner;
    ///
    /// let mut names = Interner::new();
    /// let tmp = Name::marked(names.intern("tmp"), ExpnId::from_u32(3));
    /// assert_eq!(tmp.mark.as_u32(), 3);
    /// ```
    #[must_use]
    pub const fn marked(sym: Symbol, mark: ExpnId) -> Self {
        Self { sym, mark }
    }
}

/// A member name with its own span: field names, named arguments, method names,
/// and attribute names.
///
/// Member names are resolved by type (or by the runtime), not by scope, so they
/// carry no hygiene mark; the span is what an LSP renames.
///
/// # Examples
///
/// ```
/// use hir_lang::{Ident, Span};
/// use intern_lang::Interner;
///
/// let mut names = Interner::new();
/// let field = Ident::new(names.intern("width"), Span::new(4, 9));
/// assert_eq!(field.span.len(), 5);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Ident {
    /// The spelling.
    pub sym: Symbol,
    /// The span of the name itself.
    pub span: Span,
}

impl Ident {
    /// Pairs a symbol with its span.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Ident, Span};
    /// use intern_lang::Interner;
    ///
    /// let mut names = Interner::new();
    /// let id = Ident::new(names.intern("len"), Span::empty(0));
    /// assert!(id.span.is_empty());
    /// ```
    #[must_use]
    pub const fn new(sym: Symbol, span: Span) -> Self {
        Self { sym, span }
    }
}
