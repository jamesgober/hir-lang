//! Validation and resolution errors.

use core::fmt;

use intern_lang::Symbol;

use crate::{
    id::{BinderId, ExprId, IdKind, NodeRef, PatId, PathId},
    name::{BinderKind, Ns, Res},
    origin::ExpnId,
};

/// Where an error was found: a node, a binder, an expansion record, the root,
/// or a node's attribute list.
///
/// # Examples
///
/// ```
/// use hir_lang::{ExprId, NodeRef, Site};
///
/// let site = Site::Node(NodeRef::Expr(ExprId::from_index(0).unwrap()));
/// assert_eq!(site.to_string(), "expression 0");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Site {
    /// A tree node.
    Node(NodeRef),
    /// A binder record.
    Binder(BinderId),
    /// An expansion record.
    Expansion(ExpnId),
    /// The root passed to `finish`.
    Root,
    /// The attribute list of a node.
    Attrs(NodeRef),
}

impl fmt::Display for Site {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Node(node) => write!(f, "{} {}", node.kind(), node.index()),
            Self::Binder(b) => write!(f, "binder {}", b.index()),
            Self::Expansion(e) => write!(f, "expansion {}", e.as_u32()),
            Self::Root => f.write_str("the root"),
            Self::Attrs(node) => write!(f, "the attributes of {} {}", node.kind(), node.index()),
        }
    }
}

/// A structural problem inside one node; carried by [`HirError::Malformed`].
///
/// # Examples
///
/// ```
/// use hir_lang::Malformed;
///
/// assert_eq!(Malformed::EmptyPath.to_string(), "a path has no segments");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Malformed {
    /// A path has no segments.
    EmptyPath,
    /// An or-pattern has no alternatives.
    EmptyOrPattern,
    /// A tuple or constructor pattern's `..` position is past its elements.
    RestOutOfRange,
    /// A range pattern has neither bound.
    RangeWithoutBounds,
    /// A range pattern's bounds are not both integers, both chars, or both floats.
    RangeBoundKinds,
    /// An integer literal does not fit its suffix type.
    LiteralOutOfRange,
    /// A literal's suffix is not of the literal's class (e.g. `1f32` on an
    /// integer, or a suffix on a char).
    LiteralSuffix,
    /// An `f32` literal is not exactly representable in `f32`.
    InexactF32,
    /// A string literal's payload is not valid UTF-8.
    InvalidUtf8,
    /// A big-integer literal is not an optional `-` followed by decimal digits.
    InvalidBigInt,
    /// A conversion op's target type has the wrong class.
    ConversionTarget,
    /// An assignment target is not a place expression.
    AssignTarget,
    /// A compound assignment's operator is not a binary op.
    CompoundAssignOp,
    /// A let-else has no initializer.
    LetElseWithoutInit,
    /// A record's or variant's fields do not fit its shape.
    ShapeFields,
    /// An item kind that requires a name has none.
    MissingName,
    /// An item kind that takes no name (an impl, a glob import) has one.
    UnexpectedName,
    /// A function has no body outside an interface, a class, or a foreign
    /// declaration.
    MissingBody,
    /// A constant has no value outside an interface.
    MissingConstValue,
    /// An item kind is not allowed in its container.
    ItemPlacement,
    /// A receiver parameter is not first, is in a closure, or is in a function
    /// that is not a member of an interface, impl, or class.
    ReceiverPlacement,
    /// Parameter kinds are out of order or repeated.
    ParamOrder,
    /// A receiver or rest parameter has a default.
    ParamDefault,
    /// An explicit capture uses `CaptureMode::Infer`.
    InferCapture,
    /// An attribute argument has neither key nor value.
    EmptyAttrArg,
    /// The attribute table is not sorted by target with one entry per target.
    AttrOrder,
    /// The `promote` overflow policy is used where HIR fixes a static result
    /// type: on `int_cast`, on a cast to a type other than `Any`, or in an
    /// integer constant context (array length, const generic argument,
    /// discriminant). `promote` needs a dynamically typed result.
    PromoteOnStaticResult,
}

impl fmt::Display for Malformed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::EmptyPath => "a path has no segments",
            Self::EmptyOrPattern => "an or-pattern has no alternatives",
            Self::RestOutOfRange => "the `..` position is past the last element",
            Self::RangeWithoutBounds => "a range pattern has no bounds",
            Self::RangeBoundKinds => {
                "range bounds must both be integers, both chars, or both floats"
            }
            Self::LiteralOutOfRange => "an integer literal does not fit its suffix type",
            Self::LiteralSuffix => "a literal's suffix does not match its kind",
            Self::InexactF32 => "an f32 literal is not exactly representable in f32",
            Self::InvalidUtf8 => "a string literal is not valid UTF-8",
            Self::InvalidBigInt => "a big-integer literal is not decimal digits",
            Self::ConversionTarget => "a conversion's target type has the wrong class",
            Self::AssignTarget => "an assignment target is not a place expression",
            Self::CompoundAssignOp => "a compound assignment's operator is not binary",
            Self::LetElseWithoutInit => "a let-else has no initializer",
            Self::ShapeFields => "the fields do not fit the declared shape",
            Self::MissingName => "this kind of item needs a name",
            Self::UnexpectedName => "this kind of item takes no name",
            Self::MissingBody => {
                "a function needs a body outside interfaces, classes, and foreign declarations"
            }
            Self::MissingConstValue => "a constant needs a value outside interfaces",
            Self::ItemPlacement => "this kind of item is not allowed in its container",
            Self::ReceiverPlacement => {
                "a receiver must be the first parameter of an interface, impl, or class member"
            }
            Self::ParamOrder => "parameter kinds are out of order or repeated",
            Self::ParamDefault => "receiver and rest parameters cannot have defaults",
            Self::InferCapture => "an explicit capture cannot use the `Infer` mode",
            Self::EmptyAttrArg => "an attribute argument has neither key nor value",
            Self::AttrOrder => "the attribute table is not sorted by target",
            Self::PromoteOnStaticResult => {
                "the `promote` overflow policy needs a dynamically typed result"
            }
        })
    }
}

/// A jump that has no valid target; carried by [`HirError::Jump`].
///
/// # Examples
///
/// ```
/// use hir_lang::JumpProblem;
///
/// assert_eq!(JumpProblem::ContinueToBlock.to_string(), "`continue` targets a block, not a loop");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum JumpProblem {
    /// `break` without a label outside any loop of the current function.
    BreakOutsideLoop,
    /// `continue` without a label outside any loop of the current function.
    ContinueOutsideLoop,
    /// The label is not an enclosing loop or block of the current function.
    LabelNotInScope,
    /// `continue` names a labeled block.
    ContinueToBlock,
    /// A jump would leave a `defer` or `finally` body.
    OutOfDefer,
}

impl fmt::Display for JumpProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::BreakOutsideLoop => "`break` outside a loop",
            Self::ContinueOutsideLoop => "`continue` outside a loop",
            Self::LabelNotInScope => "the label is not an enclosing loop or block",
            Self::ContinueToBlock => "`continue` targets a block, not a loop",
            Self::OutOfDefer => "a jump would leave a `defer` or `finally` body",
        })
    }
}

/// An effect form outside a frame that allows it; carried by
/// [`HirError::Effect`].
///
/// # Examples
///
/// ```
/// use hir_lang::EffectProblem;
///
/// assert_eq!(EffectProblem::AwaitOutsideAsync.to_string(), "`await` outside an async function");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum EffectProblem {
    /// `return` outside a function or closure body.
    ReturnOutsideFunction,
    /// `await` in a frame without `ASYNC`.
    AwaitOutsideAsync,
    /// `yield` in a frame without `YIELD`.
    YieldOutsideGenerator,
    /// `throw` in a frame without `THROWS` and outside a `try` body.
    ThrowNotAllowed,
}

impl fmt::Display for EffectProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ReturnOutsideFunction => "`return` outside a function",
            Self::AwaitOutsideAsync => "`await` outside an async function",
            Self::YieldOutsideGenerator => "`yield` outside a generator",
            Self::ThrowNotAllowed => "`throw` in a function that does not throw, outside `try`",
        })
    }
}

/// What a container overflowed; carried by [`HirError::CapacityExceeded`].
///
/// # Examples
///
/// ```
/// use hir_lang::{Capacity, IdKind};
///
/// assert_eq!(Capacity::Arena(IdKind::Expr).to_string(), "the expression arena");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Capacity {
    /// A node, binder, or expansion arena.
    Arena(IdKind),
    /// A list pool.
    Pool,
    /// The text pool.
    Text,
    /// The total node count across all arenas (positions are 32-bit).
    Total,
}

impl fmt::Display for Capacity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Arena(kind) => write!(f, "the {kind} arena"),
            Self::Pool => f.write_str("a list pool"),
            Self::Text => f.write_str("the text pool"),
            Self::Total => f.write_str("the total node count"),
        }
    }
}

/// Why a HIR was rejected by the validator, or a resolution by
/// [`Hir::resolve`](crate::Hir::resolve).
///
/// Every variant names the offending node, binder, path, or expansion, so a
/// lowering bug can be traced to the construct that produced it (each node has
/// an origin). The validator reports the first problem in a deterministic order
/// (spec §13).
///
/// # Examples
///
/// ```
/// use hir_lang::{Builder, HirError, ItemId};
///
/// // A root id this builder never issued.
/// let err = Builder::new().finish(ItemId::from_index(5).unwrap()).unwrap_err();
/// assert!(matches!(err, HirError::Dangling { .. }));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum HirError {
    /// An arena or pool grew past `u32::MAX - 1` entries while building. Split
    /// the input into several HIRs.
    CapacityExceeded {
        /// What overflowed.
        what: Capacity,
    },
    /// An id does not name an existing node, binder, or expansion. A lowering
    /// bug, or ids mixed between builders.
    Dangling {
        /// Where the id is stored.
        site: Site,
        /// The kind of id.
        kind: IdKind,
        /// Its index.
        index: usize,
    },
    /// A list lies outside its pool.
    ListOutOfBounds {
        /// Where the list is stored.
        site: Site,
    },
    /// A text reference lies outside the text pool.
    TextOutOfBounds {
        /// Where the reference is stored.
        site: Site,
    },
    /// An expansion refers to itself or a later expansion, so a backtrace could
    /// loop.
    ExpansionOrder {
        /// The offending expansion.
        expn: ExpnId,
    },
    /// The root item is not a module.
    RootNotModule,
    /// A node is the child of more than one parent (or of itself).
    SharedNode {
        /// The node.
        node: NodeRef,
    },
    /// A node is not reachable from the root.
    Unreachable {
        /// The node.
        node: NodeRef,
    },
    /// A node's own structure is invalid.
    Malformed {
        /// The node.
        site: Site,
        /// What is wrong.
        problem: Malformed,
    },
    /// Two members of one list share a name (fields, variants, named arguments,
    /// field initializers, field patterns).
    DuplicateName {
        /// The node owning the list.
        node: NodeRef,
        /// The repeated name.
        name: Symbol,
    },
    /// An op instance does not carry exactly the policy fields its op consults
    /// (or a cast does not carry exactly `overflow` and `float_to_int`).
    Policy {
        /// The `Op`, `Cast`, or compound `Assign` expression.
        expr: ExprId,
    },
    /// An op has the wrong number of operands.
    Arity {
        /// The `Op` expression.
        expr: ExprId,
        /// The op's arity.
        expected: usize,
        /// The number of operands given.
        found: usize,
    },
    /// A binder is never bound by any binding site.
    BinderNotBound {
        /// The binder.
        binder: BinderId,
    },
    /// A binder is bound by two binding constructs.
    BinderBoundTwice {
        /// The binder.
        binder: BinderId,
        /// The second binding site found.
        node: NodeRef,
    },
    /// A binder appears twice in one pattern alternative.
    DuplicateBinding {
        /// The binder.
        binder: BinderId,
        /// The pattern (the binding construct's root or the alternative).
        pat: PatId,
    },
    /// The alternatives of an or-pattern bind different binders.
    OrPatternBinders {
        /// The or-pattern.
        pat: PatId,
    },
    /// A binder's kind does not fit its binding site.
    BinderKind {
        /// The binder.
        binder: BinderId,
        /// The kind the site requires.
        expected: BinderKind,
    },
    /// A path's namespace does not match its parent node.
    PathNamespace {
        /// The path.
        path: PathId,
        /// The namespace its parent requires.
        expected: Ns,
    },
    /// A resolution names something the path's namespace cannot refer to (or an
    /// id that does not exist).
    Resolution {
        /// The path.
        path: PathId,
        /// The rejected resolution.
        res: Res,
    },
    /// A path resolves to a binder that is not in scope where the path is.
    OutOfScope {
        /// The path.
        path: PathId,
        /// The binder.
        binder: BinderId,
    },
    /// A path resolves to a binder outside a frame it may not cross: a local of
    /// an enclosing function from a nested item, a local from a constant
    /// context, or a local from a closure that forbids implicit captures.
    NotCapturable {
        /// The path.
        path: PathId,
        /// The binder.
        binder: BinderId,
    },
    /// A `break` or `continue` has no valid target.
    Jump {
        /// The jump.
        expr: ExprId,
        /// Why.
        problem: JumpProblem,
    },
    /// An effect form is outside a frame that allows it.
    Effect {
        /// The expression.
        expr: ExprId,
        /// Why.
        problem: EffectProblem,
    },
}

impl fmt::Display for HirError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CapacityExceeded { what } => write!(f, "{what} exceeded its capacity"),
            Self::Dangling { site, kind, index } => {
                write!(f, "{site} refers to {kind} {index}, which does not exist")
            }
            Self::ListOutOfBounds { site } => write!(f, "a list in {site} lies outside its pool"),
            Self::TextOutOfBounds { site } => {
                write!(f, "a text reference in {site} lies outside the text pool")
            }
            Self::ExpansionOrder { expn } => write!(
                f,
                "expansion {} refers to itself or a later expansion",
                expn.as_u32()
            ),
            Self::RootNotModule => f.write_str("the root item is not a module"),
            Self::SharedNode { node } => {
                write!(
                    f,
                    "{} {} has more than one parent",
                    node.kind(),
                    node.index()
                )
            }
            Self::Unreachable { node } => write!(
                f,
                "{} {} is not reachable from the root",
                node.kind(),
                node.index()
            ),
            Self::Malformed { site, problem } => write!(f, "{site}: {problem}"),
            Self::DuplicateName { node, name } => write!(
                f,
                "{} {} has two members named by symbol {}",
                node.kind(),
                node.index(),
                name.as_u32()
            ),
            Self::Policy { expr } => write!(
                f,
                "expression {} does not carry exactly the policy fields its operation uses",
                expr.index()
            ),
            Self::Arity {
                expr,
                expected,
                found,
            } => write!(
                f,
                "expression {}: the operation takes {expected} operands, {found} given",
                expr.index()
            ),
            Self::BinderNotBound { binder } => {
                write!(f, "binder {} is never bound", binder.index())
            }
            Self::BinderBoundTwice { binder, node } => write!(
                f,
                "binder {} is bound a second time at {} {}",
                binder.index(),
                node.kind(),
                node.index()
            ),
            Self::DuplicateBinding { binder, pat } => write!(
                f,
                "binder {} is bound twice in pattern {}",
                binder.index(),
                pat.index()
            ),
            Self::OrPatternBinders { pat } => write!(
                f,
                "the alternatives of or-pattern {} bind different binders",
                pat.index()
            ),
            Self::BinderKind { binder, expected } => write!(
                f,
                "binder {} is bound where a {} binder is required",
                binder.index(),
                expected.name()
            ),
            Self::PathNamespace { path, expected } => write!(
                f,
                "path {} must be in the {} namespace",
                path.index(),
                expected.name()
            ),
            Self::Resolution { path, res } => write!(
                f,
                "path {} cannot resolve to {res:?} in its namespace",
                path.index()
            ),
            Self::OutOfScope { path, binder } => write!(
                f,
                "path {} refers to binder {}, which is not in scope there",
                path.index(),
                binder.index()
            ),
            Self::NotCapturable { path, binder } => write!(
                f,
                "path {} refers to binder {} across a function, constant, or non-capturing closure boundary",
                path.index(),
                binder.index()
            ),
            Self::Jump { expr, problem } => write!(f, "expression {}: {problem}", expr.index()),
            Self::Effect { expr, problem } => write!(f, "expression {}: {problem}", expr.index()),
        }
    }
}

impl core::error::Error for HirError {}
