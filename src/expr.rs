//! Expressions and statements.

use span_lang::Span;

use crate::{
    id::{BinderId, ExprId, ItemId, List, ParamId, PatId, PathId, StmtId, TyId},
    intrinsic::{Asm, Intrinsic},
    lit::Lit,
    ops::{Op, Policy},
    origin::Ident,
    ty::{Effects, GenericArg},
};

/// A block: statements, an optional tail value, an optional label.
///
/// # Examples
///
/// ```
/// use hir_lang::{Block, List};
///
/// let empty = Block { stmts: List::EMPTY, tail: None, label: None, is_unsafe: false };
/// assert!(empty.stmts.is_empty());
/// assert_eq!(Block::default(), empty);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Block {
    /// The statements, in order.
    pub stmts: List<StmtId>,
    /// The block's value; unit when absent.
    pub tail: Option<ExprId>,
    /// A label (`Label` binder) that `break 'label value` may target.
    pub label: Option<BinderId>,
    /// An `unsafe { }` block.
    pub is_unsafe: bool,
}

/// One arm of a `match` or one `catch` clause of a `try`.
///
/// The pattern's binders are visible in the guard and the body.
///
/// # Examples
///
/// ```
/// use hir_lang::{Arm, Builder, Pat};
///
/// let mut b = Builder::new();
/// let pat = b.pat(Pat::Wild);
/// let body = b.int(0);
/// let arm = Arm { pat, guard: None, body };
/// assert_eq!(arm.body, body);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Arm {
    /// The pattern.
    pub pat: PatId,
    /// A condition evaluated after the pattern binds.
    pub guard: Option<ExprId>,
    /// The arm's value.
    pub body: ExprId,
}

/// How an argument is passed.
///
/// # Examples
///
/// ```
/// use hir_lang::ArgKind;
///
/// assert_eq!(ArgKind::default(), ArgKind::Positional);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ArgKind {
    /// By position.
    #[default]
    Positional,
    /// By parameter name (`f(level = 3)`); names are unique per call.
    Named(Ident),
    /// A sequence spread as positional arguments (`f(...xs)`, `f(*xs)`).
    Spread,
    /// A mapping spread as named arguments (`f(**kw)`).
    SpreadNamed,
}

/// One argument of a call.
///
/// # Examples
///
/// ```
/// use hir_lang::{Arg, ArgKind, Builder};
///
/// let mut b = Builder::new();
/// let value = b.int(1);
/// assert_eq!(Arg::positional(value).kind, ArgKind::Positional);
/// assert!(!Arg::positional(value).place);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Arg {
    /// How it is passed.
    pub kind: ArgKind,
    /// The argument expression.
    pub value: ExprId,
    /// The argument is passed as a place, so a by-reference parameter of the
    /// callee (PHP `&$x`) aliases it; the callee (statically, or at run time
    /// for dynamic calls) decides whether it is taken by reference. `value`
    /// must be a place expression; not allowed on spreads.
    pub place: bool,
}

impl Arg {
    /// A positional argument.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Arg, Builder};
    ///
    /// let mut b = Builder::new();
    /// let v = b.int(2);
    /// assert_eq!(Arg::positional(v).value, v);
    /// ```
    #[must_use]
    pub const fn positional(value: ExprId) -> Self {
        Self {
            kind: ArgKind::Positional,
            value,
            place: false,
        }
    }
}

/// One `name: value` entry of a record literal.
///
/// # Examples
///
/// ```
/// use hir_lang::{Builder, FieldInit, Ident, Span};
/// use intern_lang::Interner;
///
/// let mut names = Interner::new();
/// let mut b = Builder::new();
/// let value = b.int(3);
/// let init = FieldInit { name: Ident::new(names.intern("x"), Span::empty(0)), value };
/// assert_eq!(init.value, value);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FieldInit {
    /// The field name (unique per literal).
    pub name: Ident,
    /// The field's value.
    pub value: ExprId,
}

/// One entry of an ordered map literal.
///
/// # Examples
///
/// ```
/// use hir_lang::{Builder, MapEntry};
///
/// let mut b = Builder::new();
/// let v = b.int(1);
/// let next_index = MapEntry { key: None, value: v };
/// assert!(next_index.key.is_none());
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MapEntry {
    /// The key; absent means "the next index" (PHP arrays, NOML tables).
    pub key: Option<ExprId>,
    /// The value.
    pub value: ExprId,
}

/// The member a field access names.
///
/// # Examples
///
/// ```
/// use hir_lang::Member;
///
/// assert_eq!(Member::Index(0), Member::Index(0));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Member {
    /// A named field or property.
    Named(intern_lang::Symbol),
    /// A tuple field by position.
    Index(u32),
}

/// The kind of a borrow expression.
///
/// # Examples
///
/// ```
/// use hir_lang::BorrowKind;
///
/// assert_ne!(BorrowKind::Shared, BorrowKind::Mut);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BorrowKind {
    /// `&e`.
    Shared,
    /// `&mut e`.
    Mut,
    /// `&raw const e`.
    RawConst,
    /// `&raw mut e`.
    RawMut,
}

/// How a closure captures a variable.
///
/// # Examples
///
/// ```
/// use hir_lang::CaptureMode;
///
/// assert_ne!(CaptureMode::ByValue, CaptureMode::Infer);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CaptureMode {
    /// Decided per variable by typeck or a capability (Rust-style closures).
    /// Valid only as a closure's implicit mode.
    Infer,
    /// By shared reference.
    ByRef,
    /// By mutable reference.
    ByMutRef,
    /// By value (move or copy).
    ByValue,
}

/// One explicit capture: the outer variable, the closure-local binder it
/// initializes, and the mode.
///
/// # Examples
///
/// ```
/// use hir_lang::CaptureMode;
///
/// // PHP: function () use (&$total) { ... } captures `total` by mutable reference.
/// let mode = CaptureMode::ByMutRef;
/// assert_ne!(mode, CaptureMode::Infer);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Capture {
    /// The captured variable, evaluated outside the closure (`Value` namespace).
    pub outer: PathId,
    /// The binder (kind `Capture`) visible inside the closure.
    pub binder: BinderId,
    /// Never [`CaptureMode::Infer`].
    pub mode: CaptureMode,
}

/// When parameter defaults are evaluated, and what they can see.
///
/// # Examples
///
/// ```
/// use hir_lang::DefaultEval;
///
/// assert_eq!(DefaultEval::default(), DefaultEval::PerCall);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DefaultEval {
    /// At each call, in the callee, with the preceding parameters visible
    /// (Kotlin, C++, Iron).
    #[default]
    PerCall,
    /// Once, when the function is defined; no parameter is visible (Python).
    Once,
}

/// A closure (lambda); see the spec, §8.10.
///
/// # Examples
///
/// ```
/// use hir_lang::{Builder, CaptureMode, Closure, Effects, List};
///
/// let mut b = Builder::new();
/// let body = b.int(1);
/// let thunk = Closure {
///     implicit: Some(CaptureMode::Infer),
///     ..Closure::new(body)
/// };
/// assert_eq!(thunk.body, body);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Closure {
    /// The parameters.
    pub params: List<ParamId>,
    /// The declared return type.
    pub ret: Option<TyId>,
    /// The body.
    pub body: ExprId,
    /// Declared effects.
    pub effects: Effects,
    /// The mode for implicit captures, or `None` to forbid them.
    pub implicit: Option<CaptureMode>,
    /// Explicit captures, in order.
    pub captures: List<Capture>,
    /// A binder (kind `Capture`) naming the closure itself inside its body, for
    /// recursive closures.
    pub self_binder: Option<BinderId>,
    /// When parameter defaults are evaluated.
    pub defaults: DefaultEval,
}

impl Closure {
    /// A closure with no parameters, effects, or captures, that forbids
    /// implicit captures.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Builder, Closure};
    ///
    /// let mut b = Builder::new();
    /// let body = b.int(0);
    /// assert!(Closure::new(body).implicit.is_none());
    /// ```
    #[must_use]
    pub const fn new(body: ExprId) -> Self {
        Self {
            params: List::EMPTY,
            ret: None,
            body,
            effects: Effects::NONE,
            implicit: None,
            captures: List::EMPTY,
            self_binder: None,
            defaults: DefaultEval::PerCall,
        }
    }
}

/// An expression; see the spec, §8. Children are evaluated in the order the
/// fields are listed.
///
/// # Examples
///
/// ```
/// use hir_lang::{Builder, Expr, OpKind};
///
/// let mut b = Builder::new();
/// let one = b.int(1);
/// let two = b.int(2);
/// let sum = b.op(OpKind::Add, &[one, two]);
/// let hir_expr = b.expr(Expr::Return(Some(sum)));
/// assert_ne!(hir_expr, sum);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Expr {
    /// A literal.
    Lit(Lit),
    /// A reference to a local, item, variant, or constructor.
    Path(PathId),
    /// A tuple; `()` is unit.
    Tuple(List<ExprId>),
    /// An array or list literal.
    Array(List<ExprId>),
    /// `[elem; count]`.
    Repeat {
        /// The repeated element.
        elem: ExprId,
        /// The count.
        count: ExprId,
    },
    /// A struct literal (`path` set) or anonymous record/object.
    Record {
        /// The record or struct-like variant (`Type` namespace).
        path: Option<PathId>,
        /// Field initializers, unique by name.
        fields: List<FieldInit>,
        /// `..base`.
        base: Option<ExprId>,
    },
    /// An ordered key→value literal.
    Map(List<MapEntry>),
    /// A call of a function value.
    Call {
        /// The callee.
        callee: ExprId,
        /// The arguments; named arguments are unique.
        args: List<Arg>,
    },
    /// A method call, dispatched on the receiver's type or runtime class.
    MethodCall {
        /// The receiver.
        receiver: ExprId,
        /// The method name.
        method: Ident,
        /// Explicit generic arguments (`x.collect::<T>()`).
        generic_args: List<GenericArg>,
        /// The arguments.
        args: List<Arg>,
    },
    /// `base.member`.
    Field {
        /// The accessed value.
        base: ExprId,
        /// The member.
        member: Member,
        /// The span of the member name.
        span: Span,
    },
    /// `base->$name`: a member chosen at run time by a string (Mox).
    DynField {
        /// The accessed value.
        base: ExprId,
        /// The member name, evaluated.
        name: ExprId,
    },
    /// `$receiver->$name(args)`: a method chosen at run time (Mox).
    DynMethodCall {
        /// The receiver.
        receiver: ExprId,
        /// The method name, evaluated.
        name: ExprId,
        /// The arguments.
        args: List<Arg>,
    },
    /// `$$name`: the local variable whose name is computed at run time (Mox);
    /// a place.
    VarVar(ExprId),
    /// `base[]`: the slot after the last element, created on write (Mox
    /// arrays); a place, valid only as a write target.
    Append(ExprId),
    /// `base[index]`.
    Index {
        /// The indexed value.
        base: ExprId,
        /// The index.
        index: ExprId,
    },
    /// An intrinsic operation (`specs/OPS.md`) with its policy.
    Op {
        /// The op instance.
        op: Op,
        /// Operands; as many as the op's arity.
        args: List<ExprId>,
    },
    /// A type-directed conversion (`as`).
    Cast {
        /// The converted value.
        expr: ExprId,
        /// The target type.
        ty: TyId,
        /// Exactly `overflow` and `float_to_int`.
        policy: Policy,
    },
    /// `target = value` or `target op= value`; `target` is a place expression and
    /// is evaluated once. Evaluates to the value written to the place (after
    /// any conversion the store performs), so `a = b = 3` assigns 3 to both.
    /// A compound operator is an OPS op; any other operator (concatenation,
    /// a host function, a template) uses [`Expr::LetPlace`].
    Assign {
        /// The place.
        target: ExprId,
        /// The compound operator (a binary op), if any.
        op: Option<Op>,
        /// The assigned value.
        value: ExprId,
    },
    /// `let_place binder = place in body`: evaluate the operands of the place
    /// expression `place` (its base, index, member name) **once**, and make
    /// `binder` (kind [`Place`](crate::BinderKind::Place)) name that place in
    /// `body`, for reads and writes. Evaluates to `body`. This is how a
    /// compound assignment with a non-OPS operator (`$a[f()] .= "x"`), `??=`,
    /// and `++`/`--` evaluate their place exactly once. The binder is visible
    /// only in `body` and never across a frame (no closure may capture it).
    LetPlace {
        /// The place alias.
        binder: BinderId,
        /// The place expression whose operands are evaluated once.
        place: ExprId,
        /// The body, where `binder` names the place.
        body: ExprId,
    },
    /// `target = &source`: make the place `target` an alias of the place
    /// `source` (PHP reference assignment). Evaluates to the value of the
    /// aliased place.
    RefAssign {
        /// The place that becomes an alias.
        target: ExprId,
        /// The aliased place.
        source: ExprId,
    },
    /// `*e`.
    Deref(ExprId),
    /// `&e`, `&mut e`, or a raw borrow.
    Borrow {
        /// The borrow kind.
        kind: BorrowKind,
        /// The borrowed place (or a temporary).
        expr: ExprId,
    },
    /// A block.
    Block(Block),
    /// `if cond { then } else { else_ }`.
    If {
        /// The condition.
        cond: ExprId,
        /// The value when true.
        then: ExprId,
        /// The value when false; unit when absent.
        else_: Option<ExprId>,
    },
    /// `match scrutinee { arms }`; the first matching arm wins.
    Match {
        /// The matched value.
        scrutinee: ExprId,
        /// The arms, in order.
        arms: List<Arm>,
    },
    /// The one loop primitive: repeat `body`; after each iteration that ends
    /// normally or by `continue`, run `step`.
    Loop {
        /// A label (`Label` binder).
        label: Option<BinderId>,
        /// The body.
        body: ExprId,
        /// Runs between iterations (C `for` increment, `do-while` test).
        step: Option<ExprId>,
    },
    /// Exit the innermost loop, or the labeled loop or block, with a value.
    Break {
        /// The target label.
        label: Option<BinderId>,
        /// The value.
        value: Option<ExprId>,
    },
    /// Start the next iteration of the innermost or labeled loop.
    Continue {
        /// The target label.
        label: Option<BinderId>,
    },
    /// Return from the innermost function or closure.
    Return(Option<ExprId>),
    /// A closure.
    Closure(Closure),
    /// Raise an exception or error.
    Throw(ExprId),
    /// `try { body } catch … finally { … }`.
    Try {
        /// The protected body.
        body: ExprId,
        /// Catch clauses, tried in order.
        catches: List<Arm>,
        /// Runs on every exit. Unlike a `defer` body, a jump may leave it
        /// (overriding the pending completion); it may suspend (LSB rule 12).
        finally: Option<ExprId>,
    },
    /// Suspend until an awaitable completes.
    Await(ExprId),
    /// Yield from a generator; evaluates to the value sent on resumption.
    /// Without `value`, yields null (and `key` must be absent too); without
    /// `key`, the runtime supplies the language's automatic key (PHP: one more
    /// than the largest integer key so far).
    Yield {
        /// The explicit key (`yield $k => $v`).
        key: Option<ExprId>,
        /// The yielded value.
        value: Option<ExprId>,
    },
    /// Delegate to an inner generator or iterable (`yield from`): every value
    /// it yields is yielded with its key, sent values and thrown errors are
    /// forwarded to it, and the expression evaluates to its return value.
    YieldFrom(ExprId),
    /// Start a task running a zero-parameter callable; evaluates to a handle.
    Spawn(ExprId),
    /// Inline assembly.
    Asm(Asm),
    /// An atomic, volatile, fence, or named backend intrinsic.
    Intrinsic {
        /// Which intrinsic.
        kind: Intrinsic,
        /// Explicit generic arguments (the accessed type).
        generic_args: List<GenericArg>,
        /// The operands.
        args: List<ExprId>,
    },
    /// An expression that failed to lower.
    Err,
}

/// A statement inside a block; see the spec, §7.
///
/// # Examples
///
/// ```
/// use hir_lang::{Builder, Stmt};
///
/// let mut b = Builder::new();
/// let e = b.int(5);
/// let stmt = b.stmt(Stmt::Expr(e));
/// let _ = stmt;
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stmt {
    /// `let pat: ty = init else { else_ };`
    Let {
        /// The pattern; its binders are visible in later statements.
        pat: PatId,
        /// The annotation.
        ty: Option<TyId>,
        /// The initializer; absent for a declaration.
        init: Option<ExprId>,
        /// The diverging `else` of let-else (requires `init`).
        else_: Option<ExprId>,
    },
    /// An expression evaluated for its effect.
    Expr(ExprId),
    /// A local item.
    Item(ItemId),
    /// Run the expression when the enclosing block exits, last-registered first.
    Defer(ExprId),
    /// `static $x = init;`: a function-static variable, initialized the first
    /// time the statement runs and kept across calls (PHP, C).
    Static {
        /// The variable (kind `Local`), visible in later statements.
        binder: BinderId,
        /// The annotation.
        ty: Option<TyId>,
        /// The initializer.
        init: Option<ExprId>,
    },
    /// `global $x;`: binds a local that aliases the global variable `path`
    /// (PHP).
    Global {
        /// The local (kind `Local`), visible in later statements.
        binder: BinderId,
        /// The global (`Value` namespace).
        path: PathId,
    },
    /// A statement that failed to lower.
    Err,
}
