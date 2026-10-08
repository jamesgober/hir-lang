//! The deterministic debug printer (spec §17).
//!
//! One node per line: `(kind inline-data` with its children indented below and
//! `)` appended at the node's end. Paths print inline on their parent's line.
//! Indentation stops growing after [`MAX_INDENT`] levels so the output stays
//! linear in the node count at any depth.

use alloc::{string::String, vec::Vec};
use core::fmt::{self, Write};

use intern_lang::{Lookup, Symbol};

use crate::{
    expr::{ArgKind, BorrowKind, CaptureMode, Expr, Member, Stmt},
    hir::Hir,
    id::{BinderId, NodeRef},
    item::{AttrValue, ItemKind, ParamKind, Shape, Vis},
    lit::Lit,
    name::{Res, Segment},
    ops::{DivZero, FloatToInt, Op, Overflow, Policy, Shift},
    origin::{ExpnId, Name, Origin},
    pat::{BindMode, Pat},
    store::Store,
    ty::{Effects, Ty},
    walk::{Group, Mark, Step, expand},
};

/// Indentation stops growing past this depth.
const MAX_INDENT: usize = 64;

/// Options for [`print_with`] and [`print_into`].
///
/// # Examples
///
/// ```
/// use hir_lang::PrintOptions;
///
/// assert!(!PrintOptions::default().origins);
/// assert!(PrintOptions::default().with_origins(true).origins);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct PrintOptions {
    /// Print each node's origin as ` @start..end` (and `#eN` when expanded).
    pub origins: bool,
}

impl PrintOptions {
    /// Returns the options with origin printing set.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::PrintOptions;
    ///
    /// assert!(PrintOptions::default().with_origins(true).origins);
    /// ```
    #[must_use]
    pub const fn with_origins(mut self, origins: bool) -> Self {
        self.origins = origins;
        self
    }
}

/// Prints `hir` in the debug form, resolving names through `names`.
///
/// The same HIR always prints the same text, on every platform. Symbols
/// `names` does not know print as `$id`.
///
/// # Examples
///
/// ```
/// use hir_lang::{Builder, Name, OpKind};
/// use intern_lang::Interner;
///
/// let mut names = Interner::new();
/// let mut b = Builder::new();
/// let (one, two) = (b.int(1), b.int(2));
/// let sum = b.op(OpKind::Add, &[one, two]);
/// let body = b.block(&[], Some(sum));
/// let f = b.func(Name::new(names.intern("f")), &[], body);
/// let root = b.module(None, &[f]);
/// let hir = b.finish(root)?;
///
/// assert_eq!(
///     hir_lang::print(&hir, &names),
///     "(module\n  (fn f\n    (block\n      (op add overflow=error\n        (lit 1)\n        (lit 2)))))"
/// );
/// # Ok::<(), hir_lang::HirError>(())
/// ```
#[must_use]
pub fn print<L: Lookup>(hir: &Hir, names: &L) -> String {
    print_with(hir, names, PrintOptions::default())
}

/// Prints `hir` with options; see [`print()`].
///
/// # Examples
///
/// ```
/// use hir_lang::{Builder, PrintOptions, Span};
/// use intern_lang::Interner;
///
/// let names = Interner::new();
/// let mut b = Builder::new();
/// b.set_span(Span::new(0, 9));
/// let root = b.module(None, &[]);
/// let hir = b.finish(root)?;
/// let text = hir_lang::print_with(&hir, &names, PrintOptions::default().with_origins(true));
/// assert_eq!(text, "(module @0..9)");
/// # Ok::<(), hir_lang::HirError>(())
/// ```
#[must_use]
pub fn print_with<L: Lookup>(hir: &Hir, names: &L, options: PrintOptions) -> String {
    let mut out = String::new();
    // Writing into a `String` cannot fail; an error here would mean a broken
    // `fmt::Write` implementation in `alloc`, so the partial text is returned.
    if print_into(hir, names, options, &mut out).is_err() {
        return out;
    }
    out
}

/// Prints `hir` into any `fmt::Write` sink; see [`print()`].
///
/// # Errors
///
/// Returns the sink's error, if writing to it fails.
///
/// # Examples
///
/// ```
/// use hir_lang::{Builder, PrintOptions};
/// use intern_lang::Interner;
///
/// let names = Interner::new();
/// let mut b = Builder::new();
/// let root = b.module(None, &[]);
/// let hir = b.finish(root)?;
/// let mut out = String::new();
/// hir_lang::print_into(&hir, &names, PrintOptions::default(), &mut out).unwrap();
/// assert_eq!(out, "(module)");
/// # Ok::<(), hir_lang::HirError>(())
/// ```
pub fn print_into<L: Lookup, W: Write>(
    hir: &Hir,
    names: &L,
    options: PrintOptions,
    out: &mut W,
) -> fmt::Result {
    let mut p = Printer {
        s: hir.store(),
        names,
        options,
        out,
        depth: 0,
        first: true,
        groups: Vec::new(),
    };
    p.run(NodeRef::Item(hir.root()))
}

struct Printer<'a, L, W> {
    s: &'a Store,
    names: &'a L,
    options: PrintOptions,
    out: &'a mut W,
    depth: usize,
    first: bool,
    /// For each open group: whether it printed a bracket.
    groups: Vec<bool>,
}

impl<L: Lookup, W: Write> Printer<'_, L, W> {
    fn run(&mut self, root: NodeRef) -> fmt::Result {
        let mut stack = alloc::vec![Step::Enter(root)];
        let mut buf = Vec::new();
        while let Some(step) = stack.pop() {
            match step {
                Step::Enter(node) => {
                    self.open_node(node)?;
                    stack.push(Step::Leave(node));
                    buf.clear();
                    expand(self.s, node, &mut buf);
                    stack.extend(buf.drain(..).rev());
                }
                Step::Leave(_) => {
                    self.depth = self.depth.saturating_sub(1);
                    self.out.write_char(')')?;
                }
                Step::Mark(Mark::Open(group)) => self.open_group(group)?,
                Step::Mark(Mark::Close) => {
                    if self.groups.pop().unwrap_or(false) {
                        self.depth = self.depth.saturating_sub(1);
                        self.out.write_char(')')?;
                    }
                }
                Step::Mark(_) => {}
            }
        }
        Ok(())
    }

    fn line(&mut self) -> fmt::Result {
        if self.first {
            self.first = false;
            return Ok(());
        }
        self.out.write_char('\n')?;
        for _ in 0..self.depth.min(MAX_INDENT) {
            self.out.write_str("  ")?;
        }
        Ok(())
    }

    fn open_node(&mut self, node: NodeRef) -> fmt::Result {
        if matches!(node, NodeRef::Path(_)) {
            // Paths print on their parent's line.
            self.out.write_str(" (")?;
        } else {
            self.line()?;
            self.out.write_char('(')?;
        }
        self.header(node)?;
        if self.options.origins {
            self.origin(self.s.origin(node))?;
        }
        for attr in self.s.attrs(node) {
            self.out.write_str(" @")?;
            self.sym(attr.name.sym)?;
            let args = self.s.list(attr.args);
            if !args.is_empty() {
                self.out.write_char('(')?;
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        self.out.write_str(", ")?;
                    }
                    if let Some(key) = arg.key {
                        self.sym(key.sym)?;
                        if arg.value.is_some() {
                            self.out.write_str(" = ")?;
                        }
                    }
                    match arg.value {
                        Some(AttrValue::Lit(lit)) => self.lit(lit)?,
                        Some(AttrValue::Name(name)) => self.sym(name.sym)?,
                        None => {}
                    }
                }
                self.out.write_char(')')?;
            }
        }
        self.depth += 1;
        Ok(())
    }

    fn open_group(&mut self, group: Group) -> fmt::Result {
        if matches!(group, Group::Arg(ArgKind::Positional)) {
            self.groups.push(false);
            return Ok(());
        }
        self.line()?;
        self.out.write_char('(')?;
        match group {
            Group::Tag(tag) => self.out.write_str(tag)?,
            Group::Arm => self.out.write_str("arm")?,
            Group::Catch => self.out.write_str("catch")?,
            Group::Generic(b) => {
                self.out.write_str("generic ")?;
                self.binder(b)?;
                if let Some(binder) = self.s.binder(b) {
                    write!(self.out, " {}", binder.kind.name())?;
                }
            }
            Group::Where => self.out.write_str("where")?,
            Group::Capture(c) => {
                self.out.write_str("capture ")?;
                self.binder(c.binder)?;
                self.out.write_char(' ')?;
                self.out.write_str(capture_mode(c.mode))?;
            }
            Group::FieldInit(name) => {
                self.out.write_str("init ")?;
                self.sym(name.sym)?;
            }
            Group::Entry => self.out.write_str("entry")?,
            Group::Arg(kind) => match kind {
                ArgKind::Named(name) => {
                    self.out.write_str("arg ")?;
                    self.sym(name.sym)?;
                }
                ArgKind::Spread => self.out.write_str("spread")?,
                ArgKind::SpreadNamed => self.out.write_str("spread-named")?,
                ArgKind::Positional => {}
            },
            Group::FieldPat(name) => {
                self.out.write_str("field ")?;
                self.sym(name.sym)?;
            }
            Group::SliceRest => self.out.write_str("rest")?,
            Group::SegmentArgs(i) => write!(self.out, "args {i}")?,
        }
        self.groups.push(true);
        self.depth += 1;
        Ok(())
    }

    // ------------------------------------------------------------ atoms

    fn sym(&mut self, sym: Symbol) -> fmt::Result {
        let out = &mut *self.out;
        match self.names.resolve_with(sym, |s| out.write_str(s)) {
            Some(result) => result,
            None => write!(self.out, "${}", sym.as_u32()),
        }
    }

    fn mark(&mut self, mark: ExpnId) -> fmt::Result {
        if mark.is_root() {
            Ok(())
        } else {
            write!(self.out, "'e{}", mark.as_u32())
        }
    }

    fn name(&mut self, name: Name) -> fmt::Result {
        self.sym(name.sym)?;
        self.mark(name.mark)
    }

    fn binder(&mut self, b: BinderId) -> fmt::Result {
        match self.s.binder(b) {
            Some(binder) => {
                let name = binder.name;
                self.sym(name.sym)?;
                write!(self.out, "%{}", b.index())?;
                self.mark(name.mark)
            }
            None => write!(self.out, "?%{}", b.index()),
        }
    }

    fn origin(&mut self, origin: Origin) -> fmt::Result {
        write!(
            self.out,
            " @{}..{}",
            origin.span.start().to_u32(),
            origin.span.end().to_u32()
        )?;
        if !origin.expn.is_root() {
            write!(self.out, "#e{}", origin.expn.as_u32())?;
        }
        Ok(())
    }

    fn lit(&mut self, lit: Lit) -> fmt::Result {
        match lit {
            Lit::Null => self.out.write_str("null"),
            Lit::Bool(b) => write!(self.out, "{b}"),
            Lit::Int(int) => {
                if int.negative {
                    self.out.write_char('-')?;
                }
                write!(self.out, "{}", int.value)?;
                if let Some(p) = int.suffix {
                    self.out.write_str(p.name())?;
                }
                Ok(())
            }
            Lit::Float(f) => {
                write!(self.out, "{:?}", f.value())?;
                if let Some(p) = f.suffix {
                    self.out.write_str(p.name())?;
                }
                Ok(())
            }
            Lit::Char(c) => write!(self.out, "'{}'", c.escape_debug()),
            Lit::Str(t) => {
                let bytes = t.range().and_then(|r| self.s.text.get(r)).unwrap_or(&[]);
                let text = core::str::from_utf8(bytes).unwrap_or("");
                write!(self.out, "\"{}\"", text.escape_debug())
            }
            Lit::Bytes(t) => {
                let bytes = t.range().and_then(|r| self.s.text.get(r)).unwrap_or(&[]);
                self.out.write_str("b\"")?;
                for b in bytes {
                    write!(self.out, "{}", b.escape_ascii())?;
                }
                self.out.write_char('"')
            }
            Lit::BigInt(t) => {
                let bytes = t.range().and_then(|r| self.s.text.get(r)).unwrap_or(&[]);
                write!(self.out, "{}n", core::str::from_utf8(bytes).unwrap_or(""))
            }
        }
    }

    fn policy(&mut self, policy: Policy) -> fmt::Result {
        if let Some(o) = policy.overflow {
            let v = match o {
                Overflow::Error => "error",
                Overflow::Wrap => "wrap",
                Overflow::Trap => "trap",
                Overflow::Promote => "promote",
            };
            write!(self.out, " overflow={v}")?;
        }
        if let Some(d) = policy.div_zero {
            let v = match d {
                DivZero::Error => "error",
                DivZero::Trap => "trap",
            };
            write!(self.out, " div_zero={v}")?;
        }
        if let Some(s) = policy.shift {
            let v = match s {
                Shift::Error => "error",
                Shift::Mask => "mask",
            };
            write!(self.out, " shift={v}")?;
        }
        if let Some(f) = policy.float_to_int {
            let v = match f {
                FloatToInt::Error => "error",
                FloatToInt::Saturate => "saturate",
            };
            write!(self.out, " float_to_int={v}")?;
        }
        Ok(())
    }

    fn op(&mut self, op: Op) -> fmt::Result {
        self.out.write_str(op.kind.name())?;
        if let Some(t) = op.kind.target() {
            write!(self.out, "<{}>", t.name())?;
        }
        self.policy(op.policy)
    }

    fn effects(&mut self, effects: Effects) -> fmt::Result {
        for (bit, name) in [
            (Effects::THROWS, " throws"),
            (Effects::ASYNC, " async"),
            (Effects::YIELD, " yield"),
            (Effects::UNSAFE, " unsafe"),
        ] {
            if effects.contains(bit) && !bit.is_empty() {
                self.out.write_str(name)?;
            }
        }
        Ok(())
    }

    fn label(&mut self, label: Option<BinderId>) -> fmt::Result {
        if let Some(l) = label {
            self.out.write_str(" '")?;
            self.binder(l)?;
        }
        Ok(())
    }

    // ---------------------------------------------------------- headers

    fn header(&mut self, node: NodeRef) -> fmt::Result {
        match node {
            NodeRef::Item(id) => match self.s.item(id) {
                Some(item) => {
                    let item = *item;
                    self.out.write_str(item.kind.name())?;
                    if let Some(name) = item.name {
                        self.out.write_char(' ')?;
                        self.name(name)?;
                    }
                    match item.vis {
                        Vis::Private => {}
                        Vis::Package => self.out.write_str(" package")?,
                        Vis::Public => self.out.write_str(" pub")?,
                    }
                    self.item_details(&item.kind)
                }
                None => self.out.write_str("?"),
            },
            NodeRef::Expr(id) => match self.s.expr(id) {
                Some(expr) => self.expr_header(*expr),
                None => self.out.write_str("?"),
            },
            NodeRef::Stmt(id) => self.out.write_str(match self.s.stmt(id) {
                Some(Stmt::Let { .. }) => "let",
                Some(Stmt::Expr(_)) => "do",
                Some(Stmt::Item(_)) => "decl",
                Some(Stmt::Defer(_)) => "defer",
                Some(Stmt::Err) => "error",
                None => "?",
            }),
            NodeRef::Pat(id) => match self.s.pat(id) {
                Some(pat) => self.pat_header(*pat),
                None => self.out.write_str("?"),
            },
            NodeRef::Ty(id) => match self.s.ty(id) {
                Some(ty) => self.ty_header(*ty),
                None => self.out.write_str("?"),
            },
            NodeRef::Path(id) => match self.s.path(id) {
                Some(path) => {
                    let path = *path;
                    self.out.write_str("path ")?;
                    if path.global {
                        self.out.write_str("::")?;
                    }
                    let segments: Vec<Segment> = self.s.list(path.segments).to_vec();
                    for (i, seg) in segments.iter().enumerate() {
                        if i > 0 {
                            self.out.write_str("::")?;
                        }
                        self.name(seg.name)?;
                    }
                    self.res(path.res)
                }
                None => self.out.write_str("?"),
            },
            NodeRef::Field(id) => match self.s.field(id) {
                Some(field) => {
                    let field = *field;
                    self.out.write_str("field")?;
                    if let Some(name) = field.name {
                        self.out.write_char(' ')?;
                        self.sym(name.sym)?;
                    }
                    if field.vis == Vis::Public {
                        self.out.write_str(" pub")?;
                    } else if field.vis == Vis::Package {
                        self.out.write_str(" package")?;
                    }
                    Ok(())
                }
                None => self.out.write_str("?"),
            },
            NodeRef::Variant(id) => match self.s.variant(id) {
                Some(variant) => {
                    let variant = *variant;
                    self.out.write_str("variant ")?;
                    self.sym(variant.name.sym)?;
                    self.out.write_str(shape(variant.shape))
                }
                None => self.out.write_str("?"),
            },
            NodeRef::Param(id) => match self.s.param(id) {
                Some(param) => {
                    let kind = match param.kind {
                        ParamKind::Receiver => "receiver",
                        ParamKind::PositionalOnly => "positional-only",
                        ParamKind::Normal => "normal",
                        ParamKind::Rest => "rest",
                        ParamKind::NamedOnly => "named-only",
                        ParamKind::RestNamed => "rest-named",
                    };
                    write!(self.out, "param {kind}")
                }
                None => self.out.write_str("?"),
            },
        }
    }

    fn res(&mut self, res: Res) -> fmt::Result {
        match res {
            Res::Unresolved => Ok(()),
            Res::Local(b) => {
                self.out.write_str(" → local ")?;
                self.binder(b)
            }
            Res::Item(i) => write!(self.out, " → item {}", i.index()),
            Res::Variant(v) => write!(self.out, " → variant {}", v.index()),
            Res::Prim(p) => write!(self.out, " → prim {}", p.name()),
            Res::Err => self.out.write_str(" → error"),
        }
    }

    fn item_details(&mut self, kind: &ItemKind) -> fmt::Result {
        match kind {
            ItemKind::Fn(f) => {
                self.effects(f.effects)?;
                if let Some(abi) = f.abi {
                    self.out.write_str(" abi=")?;
                    self.sym(abi)?;
                }
                Ok(())
            }
            ItemKind::Record(r) => self.out.write_str(shape(r.shape)),
            ItemKind::Class(c) => {
                if c.is_abstract {
                    self.out.write_str(" abstract")?;
                }
                if c.is_final {
                    self.out.write_str(" final")?;
                }
                Ok(())
            }
            ItemKind::Global { mutable: true, .. } => self.out.write_str(" mut"),
            ItemKind::Import { glob: true, .. } => self.out.write_str(" glob"),
            _ => Ok(()),
        }
    }

    fn expr_header(&mut self, expr: Expr) -> fmt::Result {
        match expr {
            Expr::Lit(lit) => {
                self.out.write_str("lit ")?;
                self.lit(lit)
            }
            Expr::Path(_) => self.out.write_str("use"),
            Expr::Tuple(_) => self.out.write_str("tuple"),
            Expr::Array(_) => self.out.write_str("array"),
            Expr::Repeat { .. } => self.out.write_str("repeat"),
            Expr::Record { .. } => self.out.write_str("record"),
            Expr::Map(_) => self.out.write_str("map"),
            Expr::Call { .. } => self.out.write_str("call"),
            Expr::MethodCall { method, .. } => {
                self.out.write_str("method ")?;
                self.sym(method.sym)
            }
            Expr::Field { member, .. } => {
                self.out.write_str("field ")?;
                match member {
                    Member::Named(sym) => self.sym(sym),
                    Member::Index(i) => write!(self.out, "{i}"),
                }
            }
            Expr::Index { .. } => self.out.write_str("index"),
            Expr::Op { op, .. } => {
                self.out.write_str("op ")?;
                self.op(op)
            }
            Expr::Cast { policy, .. } => {
                self.out.write_str("cast")?;
                self.policy(policy)
            }
            Expr::Assign { op, .. } => {
                self.out.write_str("assign")?;
                if let Some(op) = op {
                    self.out.write_char(' ')?;
                    self.op(op)?;
                }
                Ok(())
            }
            Expr::Deref(_) => self.out.write_str("deref"),
            Expr::Borrow { kind, .. } => self.out.write_str(match kind {
                BorrowKind::Shared => "borrow",
                BorrowKind::Mut => "borrow mut",
                BorrowKind::RawConst => "borrow raw-const",
                BorrowKind::RawMut => "borrow raw-mut",
            }),
            Expr::Block(block) => {
                self.out.write_str("block")?;
                self.label(block.label)?;
                if block.is_unsafe {
                    self.out.write_str(" unsafe")?;
                }
                Ok(())
            }
            Expr::If { .. } => self.out.write_str("if"),
            Expr::Match { .. } => self.out.write_str("match"),
            Expr::Loop { label, .. } => {
                self.out.write_str("loop")?;
                self.label(label)
            }
            Expr::Break { label, .. } => {
                self.out.write_str("break")?;
                self.label(label)
            }
            Expr::Continue { label } => {
                self.out.write_str("continue")?;
                self.label(label)
            }
            Expr::Return(_) => self.out.write_str("return"),
            Expr::Closure(c) => {
                self.out.write_str("closure implicit=")?;
                self.out
                    .write_str(c.implicit.map_or("none", capture_mode))?;
                self.effects(c.effects)
            }
            Expr::Throw(_) => self.out.write_str("throw"),
            Expr::Try { .. } => self.out.write_str("try"),
            Expr::Await(_) => self.out.write_str("await"),
            Expr::Yield(_) => self.out.write_str("yield"),
            Expr::Spawn(_) => self.out.write_str("spawn"),
            Expr::Err => self.out.write_str("error"),
        }
    }

    fn pat_header(&mut self, pat: Pat) -> fmt::Result {
        match pat {
            Pat::Wild => self.out.write_str("wild"),
            Pat::Bind { binder, mode, .. } => {
                self.out.write_str("bind ")?;
                self.binder(binder)?;
                self.out.write_str(match mode {
                    BindMode::Value => "",
                    BindMode::Ref => " ref",
                    BindMode::RefMut => " ref-mut",
                })
            }
            Pat::Lit(lit) => {
                self.out.write_str("lit ")?;
                self.lit(lit)
            }
            Pat::Range { lo, hi, inclusive } => {
                self.out.write_str("range ")?;
                if let Some(lo) = lo {
                    self.lit(lo)?;
                }
                self.out.write_str(if inclusive { "..=" } else { ".." })?;
                if let Some(hi) = hi {
                    self.lit(hi)?;
                }
                Ok(())
            }
            Pat::Tuple { rest, .. } => {
                self.out.write_str("tuple")?;
                rest_at(self.out, rest)
            }
            Pat::Ctor { rest, .. } => {
                self.out.write_str("ctor")?;
                rest_at(self.out, rest)
            }
            Pat::Record { rest, .. } => {
                self.out.write_str("record")?;
                if rest {
                    self.out.write_str(" ..")?;
                }
                Ok(())
            }
            Pat::Path(_) => self.out.write_str("pat"),
            Pat::Slice { .. } => self.out.write_str("slice"),
            Pat::Or(_) => self.out.write_str("or"),
            Pat::Ref { mutable, .. } => self.out.write_str(if mutable { "ref mut" } else { "ref" }),
            Pat::TypeTest { .. } => self.out.write_str("is"),
            Pat::Err => self.out.write_str("error"),
        }
    }

    fn ty_header(&mut self, ty: Ty) -> fmt::Result {
        match ty {
            Ty::Infer => self.out.write_str("infer"),
            Ty::Prim(p) => write!(self.out, "prim {}", p.name()),
            Ty::Path(_) => self.out.write_str("type"),
            Ty::Tuple(_) => self.out.write_str("tuple-type"),
            Ty::Array { .. } => self.out.write_str("array-type"),
            Ty::Slice(_) => self.out.write_str("slice-type"),
            Ty::Ref { mutable, .. } => {
                self.out
                    .write_str(if mutable { "ref-type mut" } else { "ref-type" })
            }
            Ty::Ptr { mutable, .. } => self.out.write_str(if mutable { "ptr mut" } else { "ptr" }),
            Ty::Fn { effects, .. } => {
                self.out.write_str("fn-type")?;
                self.effects(effects)
            }
            Ty::Nullable(_) => self.out.write_str("nullable"),
            Ty::Any => self.out.write_str("any"),
            Ty::Object(_) => self.out.write_str("object"),
            Ty::Never => self.out.write_str("never"),
            Ty::SelfTy => self.out.write_str("self-type"),
            Ty::Const(_) => self.out.write_str("const-arg"),
            Ty::Err => self.out.write_str("error"),
        }
    }
}

fn rest_at<W: Write>(out: &mut W, rest: Option<u32>) -> fmt::Result {
    match rest {
        Some(at) => write!(out, " rest={at}"),
        None => Ok(()),
    }
}

fn shape(shape: Shape) -> &'static str {
    match shape {
        Shape::Named => "",
        Shape::Tuple => " tuple",
        Shape::Unit => " unit",
    }
}

fn capture_mode(mode: CaptureMode) -> &'static str {
    match mode {
        CaptureMode::Infer => "infer",
        CaptureMode::ByRef => "by-ref",
        CaptureMode::ByMutRef => "by-mut-ref",
        CaptureMode::ByValue => "by-value",
    }
}
