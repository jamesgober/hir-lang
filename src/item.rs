//! Items (declarations), their parts, and attributes.

use intern_lang::Symbol;
use span_lang::Span;

use crate::{
    expr::DefaultEval,
    id::{BinderId, ExprId, FieldId, ItemId, List, ParamId, PatId, PathId, TyId, VariantId},
    lit::Lit,
    origin::{Ident, Name},
    ty::{Bound, Effects, GenericArg},
};

/// Visibility of an item or field.
///
/// # Examples
///
/// ```
/// use hir_lang::Vis;
///
/// assert_eq!(Vis::default(), Vis::Private);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Vis {
    /// Visible in the declaring module (and, by language rule, its children).
    #[default]
    Private,
    /// Visible in the declaring class and its subclasses.
    Protected,
    /// Visible in the declaring package/crate.
    Package,
    /// Visible everywhere.
    Public,
}

/// One generic parameter: its binder, bounds, and optional type and default.
///
/// A parameter with a `ty` is a constant parameter (binder kind `ConstParam`);
/// one without is a type parameter or region (`TypeParam` or `Region`).
///
/// # Examples
///
/// ```
/// use hir_lang::{Binder, BinderKind, Builder, GenericParam, List, Name};
/// use intern_lang::Interner;
///
/// let mut names = Interner::new();
/// let mut b = Builder::new();
/// let t = b.binder(Binder::new(Name::new(names.intern("T")), BinderKind::TypeParam));
/// let gp = GenericParam::new(t);
/// assert!(gp.ty.is_none() && gp.bounds.is_empty());
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GenericParam {
    /// The parameter's binder.
    pub binder: BinderId,
    /// Interface and region bounds.
    pub bounds: List<Bound>,
    /// The type of a constant parameter.
    pub ty: Option<TyId>,
    /// The default: a type, or `GenericArg::Const` for a constant parameter.
    pub default: Option<GenericArg>,
}

impl GenericParam {
    /// A parameter with no bounds, type, or default.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Binder, BinderKind, Builder, GenericParam, Name};
    /// use intern_lang::Interner;
    ///
    /// let mut names = Interner::new();
    /// let mut b = Builder::new();
    /// let u = b.binder(Binder::new(Name::new(names.intern("U")), BinderKind::TypeParam));
    /// assert_eq!(GenericParam::new(u).binder, u);
    /// ```
    #[must_use]
    pub const fn new(binder: BinderId) -> Self {
        Self {
            binder,
            bounds: List::EMPTY,
            ty: None,
            default: None,
        }
    }
}

/// A `where` predicate: `T: A + 'r` or `'a: 'b`.
///
/// # Examples
///
/// ```
/// use hir_lang::{Bound, Builder, List, Ty, WherePred};
///
/// let mut b = Builder::new();
/// let ty = b.ty(Ty::SelfTy);
/// let pred = WherePred { subject: Bound::Ty(ty), bounds: List::EMPTY };
/// assert_eq!(pred.subject, Bound::Ty(ty));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WherePred {
    /// The constrained type or region.
    pub subject: Bound,
    /// Its bounds.
    pub bounds: List<Bound>,
}

/// The generic parameters and predicates of an item.
///
/// # Examples
///
/// ```
/// use hir_lang::Generics;
///
/// assert!(Generics::default().params.is_empty());
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Generics {
    /// The parameters; each is visible in the whole item.
    pub params: List<GenericParam>,
    /// `where` predicates.
    pub preds: List<WherePred>,
}

/// How a parameter receives its argument. Parameters appear in this order;
/// `Receiver`, `Rest`, and `RestNamed` at most once each.
///
/// # Examples
///
/// ```
/// use hir_lang::ParamKind;
///
/// assert_eq!(ParamKind::default(), ParamKind::Normal);
/// assert!(ParamKind::Receiver < ParamKind::NamedOnly);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ParamKind {
    /// `self`: the first parameter of a method in an interface, impl, or class.
    Receiver,
    /// By position only.
    PositionalOnly,
    /// By position or by name.
    #[default]
    Normal,
    /// Collects the remaining positional arguments (`...args`, `*args`).
    Rest,
    /// By name only.
    NamedOnly,
    /// Collects the remaining named arguments (`**kwargs`).
    RestNamed,
}

/// A function or closure parameter.
///
/// # Examples
///
/// ```
/// use hir_lang::{Builder, Param, ParamKind, Pat};
///
/// let mut b = Builder::new();
/// let pat = b.pat(Pat::Wild);
/// let p = Param::new(pat);
/// assert_eq!(p.kind, ParamKind::Normal);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Param {
    /// The pattern; its binders (kind `Param`) are visible in later parameters
    /// and the body.
    pub pat: PatId,
    /// The annotation.
    pub ty: Option<TyId>,
    /// The default, evaluated at each call with earlier parameters visible.
    pub default: Option<ExprId>,
    /// How the argument is received.
    pub kind: ParamKind,
    /// The parameter aliases the caller's place (PHP `&$x`); the matching
    /// argument should be a place (`Arg::place`).
    pub by_ref: bool,
}

impl Param {
    /// A `Normal` parameter without type or default.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Param, Pat};
    ///
    /// let mut b = Builder::new();
    /// let pat = b.pat(Pat::Wild);
    /// assert!(Param::new(pat).default.is_none());
    /// ```
    #[must_use]
    pub const fn new(pat: PatId) -> Self {
        Self {
            pat,
            ty: None,
            default: None,
            kind: ParamKind::Normal,
            by_ref: false,
        }
    }
}

/// The field layout of a record or variant.
///
/// # Examples
///
/// ```
/// use hir_lang::Shape;
///
/// assert_eq!(Shape::default(), Shape::Named);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Shape {
    /// Named fields: `{ x: i32 }`.
    #[default]
    Named,
    /// Positional fields: `(i32, i32)`.
    Tuple,
    /// No fields.
    Unit,
}

/// A field of a record, class, or variant.
///
/// # Examples
///
/// ```
/// use hir_lang::{FieldDef, Ident, Span};
/// use intern_lang::Interner;
///
/// let mut names = Interner::new();
/// let f = FieldDef::named(Ident::new(names.intern("x"), Span::empty(0)));
/// assert!(f.ty.is_none());
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FieldDef {
    /// The name; absent for positional fields.
    pub name: Option<Ident>,
    /// Visibility.
    pub vis: Vis,
    /// The type; absent in dynamic languages.
    pub ty: Option<TyId>,
    /// A default value (a constant context).
    pub default: Option<ExprId>,
}

impl FieldDef {
    /// A named field without type or default.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{FieldDef, Ident, Span, Vis};
    /// use intern_lang::Interner;
    ///
    /// let mut names = Interner::new();
    /// let f = FieldDef::named(Ident::new(names.intern("y"), Span::empty(0)));
    /// assert_eq!(f.vis, Vis::Private);
    /// ```
    #[must_use]
    pub const fn named(name: Ident) -> Self {
        Self {
            name: Some(name),
            vis: Vis::Private,
            ty: None,
            default: None,
        }
    }
}

/// A variant of a sum.
///
/// # Examples
///
/// ```
/// use hir_lang::{Ident, List, Shape, Span, Variant};
/// use intern_lang::Interner;
///
/// let mut names = Interner::new();
/// let none = Variant {
///     name: Ident::new(names.intern("None"), Span::empty(0)),
///     shape: Shape::Unit,
///     fields: List::EMPTY,
///     discriminant: None,
/// };
/// assert_eq!(none.shape, Shape::Unit);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Variant {
    /// The name (unique per sum).
    pub name: Ident,
    /// The field layout.
    pub shape: Shape,
    /// The fields.
    pub fields: List<FieldId>,
    /// An explicit discriminant (a constant context).
    pub discriminant: Option<ExprId>,
}

/// A function: signature and optional body.
///
/// # Examples
///
/// ```
/// use hir_lang::{Effects, FnDef};
///
/// let decl = FnDef::default();
/// assert!(decl.body.is_none());
/// assert_eq!(decl.effects, Effects::NONE);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FnDef {
    /// Generic parameters.
    pub generics: Generics,
    /// Parameters, in kind order.
    pub params: List<ParamId>,
    /// The declared return type.
    pub ret: Option<TyId>,
    /// Declared effects.
    pub effects: Effects,
    /// The thrown type, if declared.
    pub throws: Option<TyId>,
    /// A foreign calling convention (`extern "C"`); allows a missing body.
    pub abi: Option<Symbol>,
    /// The body; absent for required, abstract, and foreign functions.
    pub body: Option<ExprId>,
    /// When parameter defaults are evaluated.
    pub defaults: DefaultEval,
}

/// A record (struct).
///
/// # Examples
///
/// ```
/// use hir_lang::{RecordDef, Shape};
///
/// assert_eq!(RecordDef::default().shape, Shape::Named);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RecordDef {
    /// Generic parameters.
    pub generics: Generics,
    /// The field layout.
    pub shape: Shape,
    /// The fields.
    pub fields: List<FieldId>,
    /// An untagged union: the fields overlap (Zero, Kraken); shape `Named`.
    pub is_union: bool,
}

/// A sum (enum, tagged union).
///
/// # Examples
///
/// ```
/// use hir_lang::SumDef;
///
/// assert!(SumDef::default().variants.is_empty());
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SumDef {
    /// Generic parameters.
    pub generics: Generics,
    /// The variants (unique names).
    pub variants: List<VariantId>,
}

/// A class: fields, base classes, interfaces, and members. Also a PHP trait
/// (`mixin`), whose members are copied into the classes that use it.
///
/// # Examples
///
/// ```
/// use hir_lang::ClassDef;
///
/// let class = ClassDef::default();
/// assert!(class.bases.is_empty() && !class.is_abstract);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ClassDef {
    /// Generic parameters.
    pub generics: Generics,
    /// Base classes (one for PHP, several for Python), in resolution order.
    pub bases: List<TyId>,
    /// Implemented interfaces.
    pub interfaces: List<TyId>,
    /// Instance fields (named, unique).
    pub fields: List<FieldId>,
    /// Members: `Fn`, `Const`, `Global` (statics), `Alias`, `MixinUse`.
    pub items: List<ItemId>,
    /// Cannot be instantiated; may declare abstract methods.
    pub is_abstract: bool,
    /// Cannot be extended.
    pub is_final: bool,
    /// A PHP trait: a mixin of members, used by classes, never instantiated.
    pub mixin: bool,
}

/// What a mixin rule does with a method.
///
/// # Examples
///
/// ```
/// use hir_lang::{List, MixinAction};
///
/// assert!(matches!(MixinAction::Insteadof(List::EMPTY), MixinAction::Insteadof(_)));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MixinAction {
    /// `A::m insteadof B, C`: take `m` from this mixin, not from those.
    Insteadof(List<TyId>),
    /// `A::m as protected n`: also expose `m` under a new name and/or
    /// visibility.
    Alias {
        /// The new name.
        name: Option<Ident>,
        /// The new visibility.
        vis: Option<Vis>,
    },
}

/// One conflict-resolution rule of a mixin use.
///
/// # Examples
///
/// ```
/// use hir_lang::{Ident, MixinAction, MixinRule, Span};
/// use intern_lang::Interner;
///
/// let mut names = Interner::new();
/// let rule = MixinRule {
///     method: Ident::new(names.intern("hello"), Span::empty(0)),
///     from: None,
///     action: MixinAction::Alias { name: None, vis: None },
/// };
/// assert!(rule.from.is_none());
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MixinRule {
    /// The method.
    pub method: Ident,
    /// The mixin it comes from (`A::m`), if qualified.
    pub from: Option<TyId>,
    /// What to do.
    pub action: MixinAction,
}

/// `use A, B { rules }` inside a class: copy the members of mixins (PHP
/// traits). resolve-lang expands it after resolution.
///
/// # Examples
///
/// ```
/// use hir_lang::MixinUseDef;
///
/// assert!(MixinUseDef::default().mixins.is_empty());
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct MixinUseDef {
    /// The mixins (paths to `mixin` classes).
    pub mixins: List<TyId>,
    /// Conflict-resolution rules.
    pub rules: List<MixinRule>,
}

/// An interface (trait).
///
/// # Examples
///
/// ```
/// use hir_lang::InterfaceDef;
///
/// assert!(InterfaceDef::default().supers.is_empty());
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct InterfaceDef {
    /// Generic parameters.
    pub generics: Generics,
    /// Super-interfaces.
    pub supers: List<TyId>,
    /// Members: `Fn` (body = default method), `Const`, `AssocType`.
    pub items: List<ItemId>,
}

/// An implementation block.
///
/// # Examples
///
/// ```
/// use hir_lang::{Builder, ImplDef, Ty};
///
/// let mut b = Builder::new();
/// let self_ty = b.ty(Ty::SelfTy);
/// let inherent = ImplDef::new(self_ty);
/// assert!(inherent.interface.is_none());
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ImplDef {
    /// Generic parameters.
    pub generics: Generics,
    /// The implemented interface; absent for an inherent impl.
    pub interface: Option<TyId>,
    /// The implementing type.
    pub self_ty: TyId,
    /// Members: `Fn`, `Const`, `Alias`.
    pub items: List<ItemId>,
}

impl ImplDef {
    /// An inherent impl with no generics or members.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, ImplDef, Ty};
    ///
    /// let mut b = Builder::new();
    /// let t = b.ty(Ty::Any);
    /// assert!(ImplDef::new(t).items.is_empty());
    /// ```
    #[must_use]
    pub const fn new(self_ty: TyId) -> Self {
        Self {
            generics: Generics {
                params: List::EMPTY,
                preds: List::EMPTY,
            },
            interface: None,
            self_ty,
            items: List::EMPTY,
        }
    }
}

/// What an item declares; see the spec, §10.
///
/// # Examples
///
/// ```
/// use hir_lang::{ItemKind, List};
///
/// let module = ItemKind::Module { items: List::EMPTY, body: None, effects: hir_lang::Effects::NONE };
/// assert!(matches!(module, ItemKind::Module { .. }));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ItemKind {
    /// A function.
    Fn(FnDef),
    /// A record.
    Record(RecordDef),
    /// A sum.
    Sum(SumDef),
    /// A class.
    Class(ClassDef),
    /// An interface.
    Interface(InterfaceDef),
    /// An implementation.
    Impl(ImplDef),
    /// A type alias, or an associated type's definition inside an impl.
    Alias {
        /// Generic parameters.
        generics: Generics,
        /// The aliased type.
        ty: TyId,
    },
    /// An associated type declared by an interface.
    AssocType {
        /// Interface and region bounds.
        bounds: List<Bound>,
        /// A default.
        default: Option<TyId>,
    },
    /// A compile-time constant.
    Const {
        /// The annotation.
        ty: Option<TyId>,
        /// The value; absent only in an interface.
        value: Option<ExprId>,
    },
    /// A global or static variable.
    Global {
        /// The annotation.
        ty: Option<TyId>,
        /// Assignable after initialization.
        mutable: bool,
        /// The initializer.
        init: Option<ExprId>,
    },
    /// A module; the root of every `Hir` is one.
    Module {
        /// The items.
        items: List<ItemId>,
        /// Top-level code run when the module loads (PHP and Python scripts,
        /// NOML, REPL entries); a frame of its own that may `return`.
        body: Option<ExprId>,
        /// The effects the top-level code may perform (`await` at top level
        /// needs `ASYNC`).
        effects: Effects,
    },
    /// A mixin use inside a class (PHP `use T;`).
    MixinUse(MixinUseDef),
    /// An import: `use a::b`, `use a::b as c` (the item's name is the alias),
    /// `use a::*` (`glob`, no name).
    Import {
        /// The imported path (`Import` namespace).
        path: PathId,
        /// A glob import.
        glob: bool,
    },
    /// A declaration that failed to lower.
    Err,
}

impl ItemKind {
    /// Returns the kind's spelling, as the printer and errors use it.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::ItemKind;
    ///
    /// assert_eq!(ItemKind::Err.name(), "error");
    /// ```
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Fn(_) => "fn",
            Self::Record(_) => "record",
            Self::Sum(_) => "sum",
            Self::Class(_) => "class",
            Self::Interface(_) => "interface",
            Self::Impl(_) => "impl",
            Self::Alias { .. } => "alias",
            Self::AssocType { .. } => "assoc-type",
            Self::Const { .. } => "const",
            Self::Global { .. } => "global",
            Self::Module { .. } => "module",
            Self::MixinUse(_) => "mixin-use",
            Self::Import { .. } => "import",
            Self::Err => "error",
        }
    }
}

/// A declaration.
///
/// # Examples
///
/// ```
/// use hir_lang::{Item, ItemKind, List, Name, Vis};
/// use intern_lang::Interner;
///
/// let mut names = Interner::new();
/// let m = Item::new(Some(Name::new(names.intern("app"))), ItemKind::Module { items: List::EMPTY, body: None, effects: hir_lang::Effects::NONE })
///     .with_vis(Vis::Public);
/// assert_eq!(m.vis, Vis::Public);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Item {
    /// The hygienic name; required for most kinds (spec §10.1).
    pub name: Option<Name>,
    /// The span of the name itself.
    pub name_span: Span,
    /// Visibility.
    pub vis: Vis,
    /// What it declares.
    pub kind: ItemKind,
}

impl Item {
    /// A private item; the name span is empty (set it with
    /// [`with_name_span`](Self::with_name_span)).
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Item, ItemKind};
    ///
    /// let broken = Item::new(None, ItemKind::Err);
    /// assert!(broken.name.is_none());
    /// ```
    #[must_use]
    pub const fn new(name: Option<Name>, kind: ItemKind) -> Self {
        Self {
            name,
            name_span: Span::empty(0),
            vis: Vis::Private,
            kind,
        }
    }

    /// Returns the item with its visibility set.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Item, ItemKind, Vis};
    ///
    /// assert_eq!(Item::new(None, ItemKind::Err).with_vis(Vis::Package).vis, Vis::Package);
    /// ```
    #[must_use]
    pub const fn with_vis(mut self, vis: Vis) -> Self {
        self.vis = vis;
        self
    }

    /// Returns the item with its name span set.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Item, ItemKind, Span};
    ///
    /// let item = Item::new(None, ItemKind::Err).with_name_span(Span::new(3, 7));
    /// assert_eq!(item.name_span, Span::new(3, 7));
    /// ```
    #[must_use]
    pub const fn with_name_span(mut self, span: Span) -> Self {
        self.name_span = span;
        self
    }
}

/// An attribute value.
///
/// # Examples
///
/// ```
/// use hir_lang::{AttrValue, IntLit, Lit};
///
/// let v = AttrValue::Lit(Lit::Int(IntLit::new(10)));
/// assert!(matches!(v, AttrValue::Lit(_)));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AttrValue {
    /// A literal: `budget(cpu = 10)`.
    Lit(Lit),
    /// A bare name: `repr(C)`.
    Name(Ident),
}

/// One attribute argument: `key = value`, `key`, or `value`.
///
/// # Examples
///
/// ```
/// use hir_lang::{AttrArg, AttrValue, Ident, Span};
/// use intern_lang::Interner;
///
/// let mut names = Interner::new();
/// let c = AttrArg { key: None, value: Some(AttrValue::Name(Ident::new(names.intern("C"), Span::empty(0)))) };
/// assert!(c.key.is_none());
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AttrArg {
    /// The key, if any.
    pub key: Option<Ident>,
    /// The value, if any (not both absent).
    pub value: Option<AttrValue>,
}

/// An attribute: `@inline`, `#[repr(C)]`, `@budget(cpu = 10, heat = 3)`.
///
/// Attributes are data, never code. They attach to any node with
/// [`Builder::attach`](crate::Builder::attach).
///
/// # Examples
///
/// ```
/// use hir_lang::{Attr, Ident, List, Span};
/// use intern_lang::Interner;
///
/// let mut names = Interner::new();
/// let inline = Attr { name: Ident::new(names.intern("inline"), Span::empty(0)), args: List::EMPTY };
/// assert!(inline.args.is_empty());
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Attr {
    /// The attribute's name.
    pub name: Ident,
    /// Its arguments.
    pub args: List<AttrArg>,
}
