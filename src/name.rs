//! Binders, paths, namespaces, and resolutions.

use crate::{
    id::{BinderId, ItemId, List, TyId, VariantId},
    lit::Prim,
    origin::{Name, Origin},
};

/// What a binder names. Each kind has exactly one kind of binding site; see the
/// spec, §3.1.
///
/// # Examples
///
/// ```
/// use hir_lang::BinderKind;
///
/// assert!(BinderKind::Param.is_value());
/// assert!(BinderKind::TypeParam.is_type_level());
/// assert!(!BinderKind::Label.is_value());
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BinderKind {
    /// A local variable bound by a `let` or arm pattern.
    Local,
    /// A parameter bound by a parameter pattern.
    Param,
    /// A closure-local variable bound by an explicit capture.
    Capture,
    /// A generic type parameter.
    TypeParam,
    /// A generic constant parameter.
    ConstParam,
    /// A region (lifetime) parameter.
    Region,
    /// A loop or block label.
    Label,
}

impl BinderKind {
    /// Returns `true` for run-time variables: `Local`, `Param`, `Capture`.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::BinderKind;
    ///
    /// assert!(BinderKind::Capture.is_value());
    /// assert!(!BinderKind::ConstParam.is_value());
    /// ```
    #[must_use]
    pub const fn is_value(self) -> bool {
        matches!(self, Self::Local | Self::Param | Self::Capture)
    }

    /// Returns `true` for compile-time binders: `TypeParam`, `ConstParam`,
    /// `Region`. These may be referenced across closures and constant contexts.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::BinderKind;
    ///
    /// assert!(BinderKind::Region.is_type_level());
    /// assert!(!BinderKind::Local.is_type_level());
    /// ```
    #[must_use]
    pub const fn is_type_level(self) -> bool {
        matches!(self, Self::TypeParam | Self::ConstParam | Self::Region)
    }

    /// Returns the kind's spelling, as the printer and errors use it.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::BinderKind;
    ///
    /// assert_eq!(BinderKind::TypeParam.name(), "type-param");
    /// ```
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Param => "param",
            Self::Capture => "capture",
            Self::TypeParam => "type-param",
            Self::ConstParam => "const-param",
            Self::Region => "region",
            Self::Label => "label",
        }
    }
}

/// A binder: one variable, parameter, capture, generic parameter, or label.
///
/// Binder ids are unique across a `Hir`. A reference to a local is a binder id
/// (in a path's [`Res`]), never a name to be looked up again.
///
/// # Examples
///
/// ```
/// use hir_lang::{Binder, BinderKind, Name};
/// use intern_lang::Interner;
///
/// let mut names = Interner::new();
/// let x = Binder::new(Name::new(names.intern("x")), BinderKind::Local);
/// assert!(!x.mutable);
/// assert!(x.with_mutable(true).mutable);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Binder {
    /// The hygienic name.
    pub name: Name,
    /// What it binds.
    pub kind: BinderKind,
    /// Declared mutable (`let mut`, `var`).
    pub mutable: bool,
}

impl Binder {
    /// An immutable binder.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Binder, BinderKind, Name};
    /// use intern_lang::Interner;
    ///
    /// let mut names = Interner::new();
    /// let t = Binder::new(Name::new(names.intern("T")), BinderKind::TypeParam);
    /// assert_eq!(t.kind, BinderKind::TypeParam);
    /// ```
    #[must_use]
    pub const fn new(name: Name, kind: BinderKind) -> Self {
        Self {
            name,
            kind,
            mutable: false,
        }
    }

    /// Returns the binder with its mutability set.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Binder, BinderKind, Name};
    /// use intern_lang::Interner;
    ///
    /// let mut names = Interner::new();
    /// let n = Binder::new(Name::new(names.intern("n")), BinderKind::Local).with_mutable(true);
    /// assert!(n.mutable);
    /// ```
    #[must_use]
    pub const fn with_mutable(mut self, mutable: bool) -> Self {
        self.mutable = mutable;
        self
    }
}

/// The namespace a path is looked up in, fixed by the path's parent node.
///
/// # Examples
///
/// ```
/// use hir_lang::Ns;
///
/// assert_eq!(Ns::Value.name(), "value");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ns {
    /// Expression paths and capture sources.
    Value,
    /// Type paths and the paths of record expressions/patterns.
    Type,
    /// Constructor and constant paths in patterns.
    Pattern,
    /// Explicit regions in reference types.
    Region,
    /// Import paths.
    Import,
}

impl Ns {
    /// Returns the namespace's spelling.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::Ns;
    ///
    /// assert_eq!(Ns::Pattern.name(), "pattern");
    /// ```
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Value => "value",
            Self::Type => "type",
            Self::Pattern => "pattern",
            Self::Region => "region",
            Self::Import => "import",
        }
    }
}

/// What a path refers to. Lowering may set it; resolve-lang sets the rest with
/// [`Hir::resolve`](crate::Hir::resolve).
///
/// # Examples
///
/// ```
/// use hir_lang::{Prim, Res};
///
/// assert!(Res::Unresolved.is_unresolved());
/// assert!(!Res::Prim(Prim::Bool).is_unresolved());
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Res {
    /// Not resolved yet.
    #[default]
    Unresolved,
    /// A binder: a local, parameter, capture, or generic parameter.
    Local(BinderId),
    /// An item.
    Item(ItemId),
    /// A sum variant.
    Variant(VariantId),
    /// A primitive type.
    Prim(Prim),
    /// Resolution failed and was reported; consumers suppress cascades.
    Err,
}

impl Res {
    /// Returns `true` for [`Res::Unresolved`].
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::Res;
    ///
    /// assert!(!Res::Err.is_unresolved());
    /// ```
    #[must_use]
    pub const fn is_unresolved(self) -> bool {
        matches!(self, Self::Unresolved)
    }
}

/// One segment of a path: a hygienic name, its generic arguments, and its own
/// origin (what an LSP renames).
///
/// # Examples
///
/// ```
/// use hir_lang::{List, Name, Origin, Segment};
/// use intern_lang::Interner;
///
/// let mut names = Interner::new();
/// let seg = Segment::new(Name::new(names.intern("Vec")), Origin::default());
/// assert!(seg.args.is_empty());
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Segment {
    /// The name.
    pub name: Name,
    /// Generic arguments (`Vec::<T>`); constants appear as `Ty::Const`.
    pub args: List<TyId>,
    /// The segment's own origin.
    pub origin: Origin,
}

impl Segment {
    /// A segment without generic arguments.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Name, Origin, Segment};
    /// use intern_lang::Interner;
    ///
    /// let mut names = Interner::new();
    /// let seg = Segment::new(Name::new(names.intern("io")), Origin::default());
    /// assert_eq!(seg.origin, Origin::default());
    /// ```
    #[must_use]
    pub const fn new(name: Name, origin: Origin) -> Self {
        Self {
            name,
            args: List::EMPTY,
            origin,
        }
    }
}

/// A name reference: segments, its namespace, and its resolution slot.
///
/// # Examples
///
/// ```
/// use hir_lang::{Builder, Name, Ns, Path, Res};
/// use intern_lang::Interner;
///
/// let mut names = Interner::new();
/// let mut b = Builder::new();
/// let path = b.name_path(Name::new(names.intern("print")), Ns::Value);
/// let _ = path;
/// let p = Path { segments: hir_lang::List::EMPTY, ns: Ns::Type, res: Res::Unresolved, global: false };
/// assert_eq!(p.ns, Ns::Type);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Path {
    /// The segments (at least one).
    pub segments: List<Segment>,
    /// The namespace, which must match the path's parent.
    pub ns: Ns,
    /// What the path refers to.
    pub res: Res,
    /// A leading `::` (resolve from the root, not the current scope).
    pub global: bool,
}
