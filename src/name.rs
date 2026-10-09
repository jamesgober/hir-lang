//! Binders, paths, namespaces, and resolutions.

use intern_lang::Symbol;

use crate::{
    def::DefId,
    id::{BinderId, List, TyId},
    lit::Prim,
    origin::{Name, Origin},
    ty::GenericArg,
};

/// What a binder names. Each kind has its own binding sites; see the spec,
/// §3.1.
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BinderKind {
    /// A local variable: bound by a `let`, an arm pattern, a `static` or
    /// `global` declaration.
    Local,
    /// A parameter bound by a parameter pattern.
    Param,
    /// A closure-local variable: bound by an explicit capture or as a
    /// closure's self binder.
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

    /// Returns the namespace a path naming this binder is in, or `None` for
    /// labels (which are never named by paths).
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{BinderKind, Ns};
    ///
    /// assert_eq!(BinderKind::ConstParam.ns(), Some(Ns::Value));
    /// assert_eq!(BinderKind::Label.ns(), None);
    /// ```
    #[must_use]
    pub const fn ns(self) -> Option<Ns> {
        match self {
            Self::Local | Self::Param | Self::Capture | Self::ConstParam => Some(Ns::Value),
            Self::TypeParam => Some(Ns::Type),
            Self::Region => Some(Ns::Region),
            Self::Label => None,
        }
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

/// The namespace a path is looked up in, fixed by the path's context.
///
/// # Examples
///
/// ```
/// use hir_lang::Ns;
///
/// assert_eq!(Ns::Value.name(), "value");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Ns {
    /// Expression paths, capture sources, `global` declarations.
    Value,
    /// Type paths and the paths of record expressions/patterns.
    Type,
    /// Constructor, constant, and identifier paths in patterns.
    Pattern,
    /// Explicit regions in reference types, bounds, and generic arguments.
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

/// What a path (or its resolved prefix) refers to. Lowering may set it;
/// resolve-lang sets the rest with [`Hir::resolve`](crate::Hir::resolve).
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
    /// An item or variant, in this unit or another.
    Def(DefId),
    /// A primitive type.
    Prim(Prim),
    /// A host or standard-library symbol bound by the language's `[stdlib]`
    /// table (resolved by host-lang, not by HIR).
    Extern(Symbol),
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

/// Where a path starts.
///
/// # Examples
///
/// ```
/// use hir_lang::PathRoot;
///
/// assert_eq!(PathRoot::default(), PathRoot::Relative);
/// assert!(PathRoot::StaticType.is_type_root());
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PathRoot {
    /// Looked up from the current scope.
    #[default]
    Relative,
    /// `::a` — from the root module of the package.
    Global,
    /// `self::a` (Rust) — from the current module.
    SelfModule,
    /// `super::a`, `super::super::a` — from an enclosing module, this many
    /// levels up (at least 1).
    Super(u8),
    /// `Self::a` (Rust), `self::a` (PHP) — relative to the enclosing
    /// impl/interface/class type.
    SelfType,
    /// `parent::a` (PHP) — relative to the enclosing class's base class.
    ParentType,
    /// `static::a` (PHP) — relative to the run-time class (late static
    /// binding).
    StaticType,
}

impl PathRoot {
    /// Returns `true` for the roots that start at a type (`Self`, `parent`,
    /// `static`), whose tails may be resolved by type.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::PathRoot;
    ///
    /// assert!(PathRoot::ParentType.is_type_root());
    /// assert!(!PathRoot::Global.is_type_root());
    /// ```
    #[must_use]
    pub const fn is_type_root(self) -> bool {
        matches!(self, Self::SelfType | Self::ParentType | Self::StaticType)
    }
}

/// A qualified self type: `<T>::a` or `<T as Tr>::a`.
///
/// The first `trait_len` segments of the path name the trait (none for
/// `<T>::a`); the rest are resolved relative to `ty` (by typeck).
///
/// # Examples
///
/// ```
/// use hir_lang::{Builder, QSelf, Ty};
///
/// let mut b = Builder::new();
/// let t = b.ty(Ty::SelfTy);
/// let q = QSelf { ty: t, trait_len: 1 };
/// assert_eq!(q.trait_len, 1);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct QSelf {
    /// The self type.
    pub ty: TyId,
    /// How many leading segments name the trait.
    pub trait_len: u32,
}

/// One segment of a path: a hygienic name, its generic arguments, and its own
/// origin (what an LSP renames).
///
/// # Examples
///
/// ```
/// use hir_lang::{Name, Origin, Segment};
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
    /// Generic arguments (`Vec::<T>`, `Iterator<Item = u8>`, `Ref<'a>`).
    pub args: List<GenericArg>,
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

/// A name reference: root, optional qualified self, segments, namespace, and
/// a partial resolution.
///
/// `res` names what the first `segments.len() - unresolved` segments refer to
/// (the resolved prefix); the remaining `unresolved` segments are resolved by
/// type (associated items, `Vec::new`, `T::Item`). A path with no segments and
/// `res: Err` is the error path.
///
/// # Examples
///
/// ```
/// use hir_lang::{List, Ns, Path, PathRoot, Res};
///
/// let p = Path::new(List::EMPTY, Ns::Type);
/// assert_eq!((p.root, p.res, p.unresolved), (PathRoot::Relative, Res::Unresolved, 0));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Path {
    /// The segments (at least one, except for the error path).
    pub segments: List<Segment>,
    /// The namespace, which must match the path's context.
    pub ns: Ns,
    /// Where lookup starts.
    pub root: PathRoot,
    /// `<T>::…` or `<T as Tr>::…`.
    pub qself: Option<QSelf>,
    /// What the resolved prefix refers to.
    pub res: Res,
    /// How many trailing segments are left to type-directed resolution.
    pub unresolved: u32,
}

impl Path {
    /// An unresolved relative path.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{List, Ns, Path};
    ///
    /// assert!(Path::new(List::EMPTY, Ns::Value).qself.is_none());
    /// ```
    #[must_use]
    pub const fn new(segments: List<Segment>, ns: Ns) -> Self {
        Self {
            segments,
            ns,
            root: PathRoot::Relative,
            qself: None,
            res: Res::Unresolved,
            unresolved: 0,
        }
    }
}
