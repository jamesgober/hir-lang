# hir-lang &mdash; API Reference

> Complete reference for every public item in `hir-lang`, with examples.
> **Status: pre-1.0 (0.2.0, Foundation).** The surface is designed for the 1.0
> path and frozen only after lower-lang, resolve-lang, and typeck-lang have used
> it end to end (LexerSketch decision D18). The normative definition of HIR is
> the LexerSketch spec `specs/HIR.md`; this file documents the Rust API that
> implements it. See [`../dev/ROADMAP.md`](../dev/ROADMAP.md).

<sub>Copyright &copy; 2026 <strong>James Gober</strong>.</sub>

## Table of contents

- [Overview](#overview)
- [Installation](#installation)
- [Quick start](#quick-start)
- [Concepts](#concepts)
  - [Storage: arenas, ids, lists](#storage-arenas-ids-lists)
  - [Origins and expansions](#origins-and-expansions)
  - [Binders, scopes, and frames](#binders-scopes-and-frames)
  - [Resolution](#resolution)
  - [Operations and policies](#operations-and-policies)
  - [Effects](#effects)
  - [What the validator checks](#what-the-validator-checks)
  - [Canonical order](#canonical-order)
- [`Builder`](#builder)
- [`Hir`](#hir)
- [Free functions](#free-functions)
- [Items](#items)
- [Expressions and statements](#expressions-and-statements)
- [Patterns](#patterns)
- [Types and effect sets](#types-and-effect-sets)
- [Names, binders, paths](#names-binders-paths)
- [Literals and primitive types](#literals-and-primitive-types)
- [Operations](#operations)
- [Ids, lists, and node references](#ids-lists-and-node-references)
- [Origins](#origins)
- [Traversal](#traversal)
- [Printing](#printing)
- [Errors](#errors)
- [Re-exports](#re-exports)
- [Feature flags](#feature-flags)
- [Limits](#limits)
- [Stability](#stability)

## Overview

`hir-lang` defines the high-level IR every LexerSketch language lowers into. A
[`Builder`](#builder) creates nodes bottom-up and stamps each with its origin;
[`finish`](#builderfinish) validates and returns a [`Hir`](#hir), which can be
read, walked, printed, and resolved in place.

| Item | Kind | Purpose |
|---|---|---|
| [`Builder`](#builder) | struct | Creates nodes, lists, text, attributes, expansions; `finish` validates. |
| [`Hir`](#hir) | struct | A validated HIR: accessors, `resolve`, `can_reference`, `implicit_captures`, walking. |
| [`walk`](#walk), [`print`](#print), [`print_with`](#print_with), [`print_into`](#print_into) | functions | The Tier-1 traversal and the debug printer. |
| [`Item`](#item), [`ItemKind`](#itemkind) and the `*Def` records | types | Declarations. |
| [`Expr`](#expr), [`Stmt`](#stmt) and their records | enums | Expressions and statements. |
| [`Pat`](#pat) and its records | enum | Patterns. |
| [`Ty`](#ty), [`Effects`](#effects-1) | types | Type terms (all optional) and effect sets. |
| [`Binder`](#binder), [`Path`](#path), [`Res`](#res), [`Ns`](#ns) | types | Binding and name references. |
| [`Lit`](#lit), [`Prim`](#prim) | types | Literals and primitive types. |
| [`Op`](#op), [`OpKind`](#opkind), [`Policy`](#policy) | types | Intrinsic operations from `specs/OPS.md`. |
| [Ids](#ids), [`NodeRef`](#noderef), [`List`](#list), [`TextRef`](#textref) | types | Dense typed handles. |
| [`Origin`](#origin), [`Expansion`](#expansion), [`Name`](#name), [`Ident`](#ident) | types | Where nodes came from; hygienic names. |
| [`HirError`](#hirerror) and its details | enums | Why a HIR or a resolution was rejected. |

## Installation

```toml
[dependencies]
hir-lang = "0.2"
intern-lang = "1"   # names are intern_lang::Symbol
```

Without the standard library:

```toml
[dependencies]
hir-lang = { version = "0.2", default-features = false }
```

## Quick start

```rust
use hir_lang::{Builder, Expr, Name, OpKind, Res};
use intern_lang::Interner;

let mut names = Interner::new();
let mut b = Builder::new();

// fn inc(n) { n + 1 }   with `n` left for a resolver to resolve
let (param, n) = b.local_param(Name::new(names.intern("n")));
let use_n = b.name_expr(Name::new(names.intern("n")));
let one = b.int(1);
let sum = b.op(OpKind::Add, &[use_n, one]);
let body = b.block(&[], Some(sum));
let inc = b.func(Name::new(names.intern("inc")), &[param], body);
let root = b.module(None, &[inc]);

let mut hir = b.finish(root)?;            // validated
let Expr::Path(path) = *hir.expr(use_n) else { unreachable!() };
hir.resolve(path, Res::Local(n))?;        // checked in O(1)

assert!(hir_lang::print(&hir, &names).contains("(use (path n → local n%0))"));
# Ok::<(), hir_lang::HirError>(())
```

## Concepts

### Storage: arenas, ids, lists

Each node kind lives in a dense arena: items, expressions, statements,
patterns, types, paths, field definitions, variants, and parameters (the nine
kinds of [`NodeRef`](#noderef)), plus binders and expansion records. Nodes are
`Copy` values that reference each other through 4-byte ids. Variable-length
children are [`List`](#list)s into per-type pools, and literal text lives in one
byte pool ([`TextRef`](#textref)). No node owns a heap allocation, so cloning,
comparing, debug-printing, and dropping a `Hir` are flat loops at any depth.

The nodes form **one tree** rooted at a module: every node except the root has
exactly one parent, and every node is reachable. Sharing a node between two
parents, cycles, and orphans are rejected.

### Origins and expansions

Every node and binder has an [`Origin`](#origin): a [`Span`](#re-exports) and the
[`ExpnId`](#expnid) of the expansion (macro, lowering template, or desugaring)
that produced it. The builder keeps a current origin that every created node
captures; lowering sets the span as it walks the source tree. Expansion records
([`Expansion`](#expansion)) form backtraces: each refers only to earlier ones.

A [`Name`](#name) is a symbol plus a hygiene mark (an `ExpnId`). Text a template
writes is marked with that template's expansion, so a template's temporaries
can never be confused with user variables of the same spelling.

### Binders, scopes, and frames

Variables, parameters, captures, generic parameters, and labels are
[`Binder`](#binder)s with unique ids. Each is bound at exactly one site (the
alternatives of an or-pattern bind the same set). A binder is visible:

| Bound by | Visible in |
|---|---|
| `let` pattern | later statements and the tail of the block |
| parameter pattern | later parameters' defaults, the return/throws types, the body |
| `match`/`catch` arm pattern | the arm's guard and body |
| explicit capture | the closure's parameters and body |
| generic parameter | the whole item |
| loop/block label | the loop's body and step, or the block |

**Frames** further limit what can be referenced: items, closures, and constant
contexts (array lengths, const generic arguments, discriminants, field
defaults) each start a frame. Value binders cross only closures that allow
implicit captures; type-level binders also cross constant contexts and the
member items of an impl, interface, or class.

### Resolution

A name reference is a [`Path`](#path) node with a namespace ([`Ns`](#ns), fixed by
its parent) and a resolution slot ([`Res`](#res)). Lowering may fill slots it
knows (template temporaries); resolve-lang fills the rest with
[`Hir::resolve`](#hirresolve), which checks namespace, scope, and frames in O(1)
using the index the validator computed. The HIR stays valid after every call.

### Operations and policies

Arithmetic and conversions are [`Op`](#op)s named after `specs/OPS.md`. Each op
instance carries **exactly** the policy fields its operation consults
(`overflow`, `div_zero`, `shift`, `float_to_int`), fixed at lowering so every
execution tier behaves the same. [`Op::new`](#op) gives the OPS defaults.

### Effects

`throw`, `try`, `await`, `yield`, `spawn`, and `defer` are explicit forms.
Functions and closures declare [`Effects`](#effects-1); the validator accepts
`await` only in `ASYNC` frames, `yield` only in `YIELD` frames, and `throw` only
in `THROWS` frames or inside a `try` body of the same frame. `return` needs a
function or closure; no jump may leave a `defer` or `finally`.

### What the validator checks

[`Builder::finish`](#builderfinish) runs it; it is total (any arena is accepted or
rejected with a precise [`HirError`](#hirerror), never a panic) and linear. In
order: every id, list, and text range in bounds; expansions ordered; each
node's own shape (literals, op arity and policies, assignment targets,
parameter order, record shapes, unique member names); then one walk checking
the tree, binder sites, namespaces, jumps, effects, and item placement; then
unreachable nodes; then unbound binders; then the scope of every resolved
local.

### Canonical order

Children are visited in the order a node's fields are listed (which is also
evaluation order). The walker, the validator's scopes, and the printer all
derive from one definition of that order.

## `Builder`

```rust,ignore
pub struct Builder { /* private */ }
```

Builds a HIR bottom-up. It never panics: an arena that would outgrow `u32`
indexes is recorded and reported by `finish`. Ids it issues are only
meaningful for the HIR it finishes.

### `Builder::new`

```rust,ignore
pub fn new() -> Builder
```

An empty builder whose current origin is `0..0` in source. Also `Default`.

```rust
use hir_lang::{Builder, Origin};

assert_eq!(Builder::new().origin(), Origin::default());
```

### `Builder::origin` / `set_origin` / `set_span` / `set_expansion`

```rust,ignore
pub fn origin(&self) -> Origin
pub fn set_origin(&mut self, origin: Origin)
pub fn set_span(&mut self, span: Span)
pub fn set_expansion(&mut self, expn: ExpnId)
```

Read or change the current origin. Every node and binder created afterwards
captures it.

```rust
use hir_lang::{Builder, ExpnId, NodeRef, Span};

let mut b = Builder::new();
b.set_span(Span::new(4, 9));
b.set_expansion(ExpnId::ROOT);
let root = b.module(None, &[]);
let hir = b.finish(root)?;
assert_eq!(hir.origin(NodeRef::Item(root)).span, Span::new(4, 9));
# Ok::<(), hir_lang::HirError>(())
```

### `Builder::expansion`

```rust,ignore
pub fn expansion(&mut self, expansion: Expansion) -> ExpnId
```

Records an expansion and returns its id, which is also its hygiene mark.
`parent` and `def_site` must be earlier expansions or the root; otherwise
`finish` returns [`ExpansionOrder`](#hirerror).

```rust
use hir_lang::{Builder, ExpnId, ExpnKind, Expansion, Span};
use intern_lang::Interner;

let mut names = Interner::new();
let mut b = Builder::new();
let e = b.expansion(Expansion {
    kind: ExpnKind::Template,
    name: names.intern("for_in"),
    call_site: Span::new(0, 30),
    parent: ExpnId::ROOT,
    def_site: ExpnId::ROOT,
});
let root = b.module(None, &[]);
let hir = b.finish(root)?;
assert_eq!(hir.expansion(e).map(|x| x.kind), Some(ExpnKind::Template));
# Ok::<(), hir_lang::HirError>(())
```

### Node constructors

```rust,ignore
pub fn binder(&mut self, binder: Binder) -> BinderId
pub fn item(&mut self, item: Item) -> ItemId
pub fn expr(&mut self, expr: Expr) -> ExprId
pub fn stmt(&mut self, stmt: Stmt) -> StmtId
pub fn pat(&mut self, pat: Pat) -> PatId
pub fn ty(&mut self, ty: Ty) -> TyId
pub fn path(&mut self, path: Path) -> PathId
pub fn field(&mut self, field: FieldDef) -> FieldId
pub fn variant(&mut self, variant: Variant) -> VariantId
pub fn param(&mut self, param: Param) -> ParamId
```

Create one node (or binder) at the current origin and return its id. Nothing
is checked until `finish`.

```rust
use hir_lang::{Builder, Expr, Prim, Ty};

let mut b = Builder::new();
let int = b.ty(Ty::Prim(Prim::I64));
let unit = b.expr(Expr::Tuple(hir_lang::List::EMPTY));
assert_eq!((int.index(), unit.index()), (0, 0));
```

### `Builder::list`

```rust,ignore
pub fn list<T: Pooled>(&mut self, elems: &[T]) -> List<T>
```

Copies `elems` into their pool. Works for every list element type
([`Pooled`](#pooled)).

```rust
use hir_lang::Builder;

let mut b = Builder::new();
let (a, c) = (b.int(1), b.int(2));
assert_eq!(b.list(&[a, c]).len(), 2);
```

### `Builder::text` / `Builder::bytes`

```rust,ignore
pub fn text(&mut self, text: &str) -> TextRef
pub fn bytes(&mut self, bytes: &[u8]) -> TextRef
```

Store literal payloads (strings; byte strings and big-integer digits).

```rust
use hir_lang::{Builder, Lit};

let mut b = Builder::new();
let t = b.text("hello");
let _lit = b.lit(Lit::Str(t));
assert_eq!(t.len(), 5);
```

### `Builder::attach`

```rust,ignore
pub fn attach(&mut self, node: NodeRef, attrs: &[Attr])
```

Attaches attributes to any node; repeated calls for one node accumulate in
order. Read them back with [`Hir::attrs`](#hirattrs).

```rust
use hir_lang::{Attr, Builder, Ident, List, NodeRef, Span};
use intern_lang::Interner;

let mut names = Interner::new();
let mut b = Builder::new();
let root = b.module(None, &[]);
b.attach(NodeRef::Item(root), &[Attr { name: Ident::new(names.intern("test"), Span::empty(0)), args: List::EMPTY }]);
assert_eq!(b.finish(root)?.attrs(NodeRef::Item(root)).len(), 1);
# Ok::<(), hir_lang::HirError>(())
```

### Conveniences

| Method | Creates |
|---|---|
| `lit(lit) -> ExprId` | `Expr::Lit` |
| `int(i64) -> ExprId` | an unsuffixed integer literal |
| `str_lit(&str) -> ExprId` | a string literal |
| `op(kind, &[ExprId]) -> ExprId` | `Expr::Op` at the OPS default policy |
| `op_with(op, &[ExprId]) -> ExprId` | `Expr::Op` with an explicit policy |
| `call(callee, &[ExprId]) -> ExprId` | `Expr::Call` with positional arguments |
| `new_binder(name, kind) -> BinderId` | an immutable binder |
| `bind(binder) -> PatId` | a by-value `Pat::Bind` |
| `local_param(name) -> (ParamId, BinderId)` | a `Normal` parameter binding a fresh `Param` binder |
| `name_path(name, ns) -> PathId` | an unresolved one-segment path |
| `resolved_path(name, ns, res) -> PathId` | a one-segment path with a resolution |
| `name_expr(name) -> ExprId` | an expression naming `name`, unresolved |
| `use_binder(binder) -> ExprId` | an expression already resolved to `binder` (hygiene by construction) |
| `block(&[StmtId], Option<ExprId>) -> ExprId` | an unlabeled block |
| `let_stmt(pat, Option<ExprId>) -> StmtId` | `let pat = init;` |
| `expr_stmt(ExprId) -> StmtId` | an expression statement |
| `func(name, &[ParamId], body) -> ItemId` | a private function with no generics or effects |
| `module(Option<Name>, &[ItemId]) -> ItemId` | a private module |

```rust
use hir_lang::{Builder, Name, OpKind, Op, Overflow};
use intern_lang::Interner;

let mut names = Interner::new();
let mut b = Builder::new();
let (p, x) = b.local_param(Name::new(names.intern("x")));
let ux = b.use_binder(x);
let two = b.int(2);
let wrapped = b.op_with(Op::new(OpKind::Mul).with_overflow(Overflow::Wrap), &[ux, two]);
let body = b.block(&[], Some(wrapped));
let f = b.func(Name::new(names.intern("double")), &[p], body);
let root = b.module(None, &[f]);
assert!(b.finish(root).is_ok());
```

### `Builder::finish`

```rust,ignore
pub fn finish(self, root: ItemId) -> Result<Hir, HirError>
```

Validates everything built with `root` (a module) as the root.

**Errors:** [`CapacityExceeded`](#hirerror) if building overflowed; otherwise
the first validation error (see [What the validator checks](#what-the-validator-checks)).

```rust
use hir_lang::{Builder, Expr, HirError, NodeRef};

let mut b = Builder::new();
let stray = b.expr(Expr::Err);
let root = b.module(None, &[]);
assert_eq!(b.finish(root), Err(HirError::Unreachable { node: NodeRef::Expr(stray) }));
```

## `Hir`

```rust,ignore
pub struct Hir { /* private */ }   // Clone, Debug, PartialEq, Eq
```

A validated HIR. Accessors never panic: a foreign id reads as an error node
(`ItemKind::Err`, `Expr::Err`, `Stmt::Err`, `Pat::Err`, `Ty::Err`, an empty
path resolved to `Res::Err`, an empty field), or `None` for the records that
have no error form.

### Node accessors

```rust,ignore
pub fn root(&self) -> ItemId
pub fn item(&self, id: ItemId) -> &Item
pub fn expr(&self, id: ExprId) -> &Expr
pub fn stmt(&self, id: StmtId) -> &Stmt
pub fn pat(&self, id: PatId) -> &Pat
pub fn ty(&self, id: TyId) -> &Ty
pub fn path(&self, id: PathId) -> &Path
pub fn field(&self, id: FieldId) -> &FieldDef
pub fn variant(&self, id: VariantId) -> Option<&Variant>
pub fn param(&self, id: ParamId) -> Option<&Param>
pub fn binder(&self, id: BinderId) -> Option<&Binder>
pub fn expansion(&self, id: ExpnId) -> Option<&Expansion>
pub fn variant_owner(&self, id: VariantId) -> Option<ItemId>
pub fn count(&self, kind: IdKind) -> usize
```

`count` is the arena length: ids of that kind are exactly `0..count`, so side
tables are plain vectors. `variant_owner` returns the sum declaring a variant.

```rust
use hir_lang::{Builder, Expr, IdKind, ItemKind};

let mut b = Builder::new();
let one = b.int(1);
let body = b.block(&[], Some(one));
let f = b.func(hir_lang::Name::new(intern_lang::Interner::new().intern("f")), &[], body);
let root = b.module(None, &[f]);
let hir = b.finish(root)?;
assert!(matches!(hir.item(f).kind, ItemKind::Fn(_)));
assert!(matches!(hir.expr(one), Expr::Lit(_)));
assert_eq!(hir.count(IdKind::Expr), 2);
# Ok::<(), hir_lang::HirError>(())
```

### `Hir::origin` / `Hir::binder_origin`

```rust,ignore
pub fn origin(&self, node: NodeRef) -> Origin
pub fn binder_origin(&self, id: BinderId) -> Origin
```

Where a node (or a binder's name) came from.

```rust
use hir_lang::{Builder, NodeRef, Span};

let mut b = Builder::new();
b.set_span(Span::new(2, 3));
let root = b.module(None, &[]);
assert_eq!(b.finish(root)?.origin(NodeRef::Item(root)).span, Span::new(2, 3));
# Ok::<(), hir_lang::HirError>(())
```

### `Hir::list` / `Hir::text` / `Hir::str`

```rust,ignore
pub fn list<T: Pooled>(&self, list: List<T>) -> &[T]
pub fn text(&self, text: TextRef) -> &[u8]
pub fn str(&self, text: TextRef) -> &str
```

Read a list's elements or a literal's payload (`str` returns `""` for non-UTF-8,
which string literals never are).

```rust
use hir_lang::{Builder, Expr, Lit};

let mut b = Builder::new();
let s = b.str_lit("ok");
let body = b.block(&[], Some(s));
let f = b.func(hir_lang::Name::new(intern_lang::Interner::new().intern("f")), &[], body);
let root = b.module(None, &[f]);
let hir = b.finish(root)?;
let Expr::Lit(Lit::Str(t)) = *hir.expr(s) else { unreachable!() };
assert_eq!(hir.str(t), "ok");
# Ok::<(), hir_lang::HirError>(())
```

### `Hir::attrs`

```rust,ignore
pub fn attrs(&self, node: NodeRef) -> &[Attr]
```

A node's attributes in attach order, by binary search over the sorted table.

```rust
use hir_lang::{Builder, NodeRef};

let mut b = Builder::new();
let root = b.module(None, &[]);
assert!(b.finish(root)?.attrs(NodeRef::Item(root)).is_empty());
# Ok::<(), hir_lang::HirError>(())
```

### `Hir::resolve`

```rust,ignore
pub fn resolve(&mut self, path: PathId, res: Res) -> Result<(), HirError>
```

Sets a path's resolution after an O(1) check. On error nothing changes.

| Error | When |
|---|---|
| [`Dangling`](#hirerror) | `path`, or the binder/item/variant in `res`, is not in this HIR |
| [`Resolution`](#hirerror) | the path's namespace cannot name `res` |
| [`OutOfScope`](#hirerror) | `res` is a binder not in scope at the path |
| [`NotCapturable`](#hirerror) | `res` is a binder behind a frame the path may not cross |

```rust
use hir_lang::{BinderKind, Builder, Expr, HirError, Name, Res};
use intern_lang::Interner;

let mut names = Interner::new();
let mut b = Builder::new();
let early = b.name_expr(Name::new(names.intern("x")));
let s1 = b.expr_stmt(early);
let x = b.new_binder(Name::new(names.intern("x")), BinderKind::Local);
let px = b.bind(x);
let one = b.int(1);
let s2 = b.let_stmt(px, Some(one));
let body = b.block(&[s1, s2], None);
let f = b.func(Name::new(names.intern("f")), &[], body);
let root = b.module(None, &[f]);
let mut hir = b.finish(root)?;

let Expr::Path(p) = *hir.expr(early) else { unreachable!() };
// `x` is used before its `let`.
assert_eq!(hir.resolve(p, Res::Local(x)), Err(HirError::OutOfScope { path: p, binder: x }));
assert_eq!(hir.path(p).res, Res::Unresolved);
# Ok::<(), hir_lang::HirError>(())
```

### `Hir::can_reference`

```rust,ignore
pub fn can_reference(&self, path: PathId, binder: BinderId) -> bool
```

Whether `resolve(path, Res::Local(binder))` would succeed: the binder's kind
fits the namespace, it is in scope, and no frame forbids it. A resolver uses it
to filter candidates.

```rust
use hir_lang::{Builder, Expr, Name};
use intern_lang::Interner;

let mut names = Interner::new();
let mut b = Builder::new();
let (p, n) = b.local_param(Name::new(names.intern("n")));
let use_n = b.name_expr(Name::new(names.intern("n")));
let body = b.block(&[], Some(use_n));
let f = b.func(Name::new(names.intern("f")), &[p], body);
let root = b.module(None, &[f]);
let hir = b.finish(root)?;
let Expr::Path(path) = *hir.expr(use_n) else { unreachable!() };
assert!(hir.can_reference(path, n));
# Ok::<(), hir_lang::HirError>(())
```

### `Hir::implicit_captures`

```rust,ignore
pub fn implicit_captures(&self, closure: ExprId) -> Vec<BinderId>
```

The value binders defined outside a closure that its body or parameter
defaults reference through resolved paths, in first-use order. Explicit
captures are excluded (they are in `Closure::captures`). Empty for a
non-closure. Linear in the closure's size.

```rust
use hir_lang::{Builder, CaptureMode, Closure, Effects, Expr, List, Name};
use intern_lang::Interner;

let mut names = Interner::new();
let mut b = Builder::new();
let (p, k) = b.local_param(Name::new(names.intern("k")));
let body = b.use_binder(k);
let c = b.expr(Expr::Closure(Closure {
    params: List::EMPTY, ret: None, body, effects: Effects::NONE,
    implicit: Some(CaptureMode::ByValue), captures: List::EMPTY,
}));
let fbody = b.block(&[], Some(c));
let f = b.func(Name::new(names.intern("f")), &[p], fbody);
let root = b.module(None, &[f]);
assert_eq!(b.finish(root)?.implicit_captures(c), vec![k]);
# Ok::<(), hir_lang::HirError>(())
```

### `Hir::walk_from` / `Hir::children_into`

```rust,ignore
pub fn walk_from<F: FnMut(Event) -> Control>(&self, start: NodeRef, f: F)
pub fn children_into(&self, node: NodeRef, out: &mut Vec<NodeRef>)
```

`walk_from` delivers `Enter`/`Leave` [events](#traversal) for the subtree of
`start` in canonical order with an explicit stack (any depth is safe).
`children_into` appends a node's direct children.

```rust
use hir_lang::{Builder, Control, Event, NodeRef};

let mut b = Builder::new();
let root = b.module(None, &[]);
let hir = b.finish(root)?;
let mut events = Vec::new();
hir.walk_from(NodeRef::Item(root), |e| { events.push(e); Control::Continue });
assert_eq!(events, [Event::Enter(NodeRef::Item(root)), Event::Leave(NodeRef::Item(root))]);
# Ok::<(), hir_lang::HirError>(())
```

### `Hir::validate`

```rust,ignore
pub fn validate(&self) -> Result<(), HirError>
```

Re-runs the validator. Always `Ok` for a `Hir` from this crate (`resolve`
preserves validity); offered for tests and debug builds of consumers.

```rust
use hir_lang::Builder;

let mut b = Builder::new();
let root = b.module(None, &[]);
assert_eq!(b.finish(root)?.validate(), Ok(()));
# Ok::<(), hir_lang::HirError>(())
```

## Free functions

### `walk`

```rust,ignore
pub fn walk<F: FnMut(NodeRef)>(hir: &Hir, f: F)
```

The Tier-1 traversal: every node from the root, in canonical preorder.

```rust
use hir_lang::Builder;

let mut b = Builder::new();
let root = b.module(None, &[]);
let hir = b.finish(root)?;
let mut n = 0;
hir_lang::walk(&hir, |_| n += 1);
assert_eq!(n, 1);
# Ok::<(), hir_lang::HirError>(())
```

### `print`

```rust,ignore
pub fn print<L: Lookup>(hir: &Hir, names: &L) -> String
```

The deterministic debug form; see [Printing](#printing).

### `print_with`

```rust,ignore
pub fn print_with<L: Lookup>(hir: &Hir, names: &L, options: PrintOptions) -> String
```

### `print_into`

```rust,ignore
pub fn print_into<L: Lookup, W: fmt::Write>(hir: &Hir, names: &L, options: PrintOptions, out: &mut W) -> fmt::Result
```

**Errors:** only the sink's own write errors.

## Items

### `Item`

```rust,ignore
pub struct Item { pub name: Option<Name>, pub name_span: Span, pub vis: Vis, pub kind: ItemKind }
```

A declaration. `Item::new(name, kind)` makes a private item; `with_vis` and
`with_name_span` adjust it. Names are required for functions, records, sums,
classes, interfaces, aliases, associated types, constants, and globals; absent
for impls and glob imports; optional for modules, imports (the alias), and
errors.

### `ItemKind`

| Variant | Fields | Notes |
|---|---|---|
| `Fn(FnDef)` | generics, params, ret, effects, throws, abi, body | body absent only in an interface, a class (abstract), or with an `abi` |
| `Record(RecordDef)` | generics, shape, fields | |
| `Sum(SumDef)` | generics, variants | variant names unique |
| `Class(ClassDef)` | generics, base, interfaces, fields, items, is_abstract, is_final | members: `Fn`, `Const`, `Global`, `Alias` |
| `Interface(InterfaceDef)` | generics, supers, items | members: `Fn`, `Const`, `AssocType` |
| `Impl(ImplDef)` | generics, interface, self_ty, items | members: `Fn`, `Const`, `Alias` |
| `Alias { generics, ty }` | | a type alias or an impl's associated type |
| `AssocType { bounds, default }` | | only in an interface |
| `Const { ty, value }` | | value absent only in an interface |
| `Global { ty, mutable, init }` | | globals and statics |
| `Module { items }` | | the root is a module |
| `Import { path, glob }` | | `Import`-namespace path |
| `Err` | | a declaration that failed to lower |

`ItemKind::name()` returns the printer's spelling (`"fn"`, `"record"`, ...).

```rust
use hir_lang::{Builder, Ident, Item, ItemKind, Name, RecordDef, FieldDef, Prim, Shape, Span, Ty};
use intern_lang::Interner;

let mut names = Interner::new();
let mut b = Builder::new();
let int = b.ty(Ty::Prim(Prim::I32));
let x = b.field(FieldDef { ty: Some(int), ..FieldDef::named(Ident::new(names.intern("x"), Span::empty(0))) });
let fields = b.list(&[x]);
let point = b.item(Item::new(
    Some(Name::new(names.intern("Point"))),
    ItemKind::Record(RecordDef { shape: Shape::Named, fields, ..RecordDef::default() }),
));
let root = b.module(None, &[point]);
assert!(b.finish(root).is_ok());
```

### Item records

| Type | Fields |
|---|---|
| `FnDef` (`Default`) | `generics: Generics`, `params: List<ParamId>`, `ret: Option<TyId>`, `effects: Effects`, `throws: Option<TyId>`, `abi: Option<Symbol>`, `body: Option<ExprId>` |
| `RecordDef` (`Default`) | `generics`, `shape: Shape`, `fields: List<FieldId>` |
| `SumDef` (`Default`) | `generics`, `variants: List<VariantId>` |
| `ClassDef` (`Default`) | `generics`, `base: Option<TyId>`, `interfaces: List<TyId>`, `fields`, `items: List<ItemId>`, `is_abstract`, `is_final` |
| `InterfaceDef` (`Default`) | `generics`, `supers: List<TyId>`, `items` |
| `ImplDef` (`ImplDef::new(self_ty)`) | `generics`, `interface: Option<TyId>`, `self_ty: TyId`, `items` |
| `Generics` (`Default`) | `params: List<GenericParam>`, `preds: List<WherePred>` |
| `GenericParam` (`GenericParam::new(binder)`) | `binder`, `bounds: List<TyId>`, `ty: Option<TyId>` (a const parameter), `default: Option<TyId>` |
| `WherePred` | `ty: TyId`, `bounds: List<TyId>` |
| `FieldDef` (`FieldDef::named(ident)`, `Default`) | `name: Option<Ident>`, `vis: Vis`, `ty: Option<TyId>`, `default: Option<ExprId>` (a constant context) |
| `Variant` | `name: Ident`, `shape: Shape`, `fields: List<FieldId>`, `discriminant: Option<ExprId>` (a constant context) |
| `Param` (`Param::new(pat)`) | `pat: PatId`, `ty: Option<TyId>`, `default: Option<ExprId>`, `kind: ParamKind` |
| `Shape` | `Named` (default), `Tuple`, `Unit` |
| `Vis` | `Private` (default), `Package`, `Public` |
| `ParamKind` | `Receiver`, `PositionalOnly`, `Normal` (default), `Rest`, `NamedOnly`, `RestNamed` — in this order; `Receiver`/`Rest`/`RestNamed` at most once and without defaults; `Receiver` only first, only in interface/impl/class members |

### Attributes

| Type | Fields |
|---|---|
| `Attr` | `name: Ident`, `args: List<AttrArg>` |
| `AttrArg` | `key: Option<Ident>`, `value: Option<AttrValue>` (not both absent) |
| `AttrValue` | `Lit(Lit)`, `Name(Ident)` |

Attributes are data, never code. Attach with [`Builder::attach`](#builderattach).

## Expressions and statements

### `Expr`

| Variant | Meaning |
|---|---|
| `Lit(Lit)` | a literal |
| `Path(PathId)` | a reference (`Value` namespace) |
| `Tuple(List<ExprId>)`, `Array(List<ExprId>)` | aggregates; `()` is unit |
| `Repeat { elem, count }` | `[elem; count]` |
| `Record { path, fields, base }` | struct literal or anonymous record; field names unique |
| `Map(List<MapEntry>)` | ordered key→value literal; an absent key means "next index" |
| `Call { callee, args }` | call; named arguments unique |
| `MethodCall { receiver, method, generic_args, args }` | method call (dispatched by type or runtime class) |
| `Field { base, member, span }` | `base.name` / `base.0` |
| `Index { base, index }` | `base[index]` |
| `Op { op, args }` | intrinsic op; `args.len()` = arity |
| `Cast { expr, ty, policy }` | `as`; policy has exactly `overflow` and `float_to_int` |
| `Assign { target, op, value }` | `target` must be a place (`Path`, `Field`, `Index`, `Deref`, `Err`); `op` must be binary |
| `Deref(ExprId)`, `Borrow { kind, expr }` | `*e`, `&e` / `&mut e` / raw |
| `Block(Block)` | statements + tail + optional label |
| `If { cond, then, else_ }` | |
| `Match { scrutinee, arms }` | first matching arm wins |
| `Loop { label, body, step }` | the one loop; `step` runs after each iteration and on `continue` |
| `Break { label, value }`, `Continue { label }`, `Return(Option)` | jumps |
| `Closure(Closure)` | lambda with explicit captures |
| `Throw`, `Try { body, catches, finally }`, `Await`, `Yield(Option)`, `Spawn` | effects |
| `Err` | failed to lower |

Records: `Block { stmts, tail, label, is_unsafe }` (`Default`),
`Arm { pat, guard, body }`, `Arg { kind: ArgKind, value }` (`Arg::positional`),
`ArgKind = Positional | Named(Ident) | Spread | SpreadNamed`,
`FieldInit { name: Ident, value }`, `MapEntry { key: Option<ExprId>, value }`,
`Member = Named(Symbol) | Index(u32)`,
`BorrowKind = Shared | Mut | RawConst | RawMut`,
`Closure { params, ret, body, effects, implicit: Option<CaptureMode>, captures }`,
`Capture { outer: PathId, binder: BinderId, mode }`,
`CaptureMode = Infer | ByRef | ByMutRef | ByValue` (`Infer` only as a closure's implicit mode).

```rust
use hir_lang::{BinderKind, Block, Builder, Expr, Name};
use intern_lang::Interner;

// 'l: loop { break 'l 7 }
let mut names = Interner::new();
let mut b = Builder::new();
let l = b.new_binder(Name::new(names.intern("l")), BinderKind::Label);
let seven = b.int(7);
let brk = b.expr(Expr::Break { label: Some(l), value: Some(seven) });
let lp = b.expr(Expr::Loop { label: Some(l), body: brk, step: None });
let body = b.expr(Expr::Block(Block { tail: Some(lp), ..Block::default() }));
let f = b.func(Name::new(names.intern("f")), &[], body);
let root = b.module(None, &[f]);
assert!(b.finish(root).is_ok());
```

### `Stmt`

| Variant | Meaning |
|---|---|
| `Let { pat, ty, init, else_ }` | binders visible in later statements; `else_` requires `init` |
| `Expr(ExprId)` | evaluated for effect |
| `Item(ItemId)` | a local item (never sees the enclosing function's locals) |
| `Defer(ExprId)` | runs at block exit; may not jump out of itself |
| `Err` | failed to lower |

## Patterns

### `Pat`

| Variant | Meaning |
|---|---|
| `Wild` | `_` |
| `Bind { binder, mode, sub }` | bind (by value, `ref`, `ref mut`), optionally `x @ sub` |
| `Lit(Lit)` | equality with a literal |
| `Range { lo, hi, inclusive }` | at least one bound; bounds both integer, char, or float |
| `Tuple { elems, rest }`, `Ctor { path, elems, rest }` | `rest` = position of `..` (at most the element count) |
| `Record { path, fields, rest }` | field names unique; no path = anonymous record |
| `Path(PathId)` | unit variant, unit record, constant (`Pattern` namespace) |
| `Slice { prefix, rest: Option<SliceRest> }` | `SliceRest { bind, suffix }` |
| `Or(List<PatId>)` | non-empty; alternatives bind the same binders |
| `Ref { mutable, inner }` | `&p` |
| `TypeTest { ty, pat }` | runtime/static type test (`catch (E e)`) |
| `Err` | failed to lower |

Records: `FieldPat { name: Ident, pat }`, `SliceRest { bind: Option<PatId>, suffix }`,
`BindMode = Value | Ref | RefMut`. Patterns contain no expressions; a guard
belongs to the arm.

```rust
use hir_lang::{Arm, BinderKind, Builder, Expr, Name, Pat};
use intern_lang::Interner;

// match 5 { x | x => x }  — both alternatives bind the same `x`
let mut names = Interner::new();
let mut b = Builder::new();
let x = b.new_binder(Name::new(names.intern("x")), BinderKind::Local);
let (a, c) = (b.bind(x), b.bind(x));
let alts = b.list(&[a, c]);
let or = b.pat(Pat::Or(alts));
let body = b.use_binder(x);
let scrutinee = b.int(5);
let arms = b.list(&[Arm { pat: or, guard: None, body }]);
let m = b.expr(Expr::Match { scrutinee, arms });
let fbody = b.block(&[], Some(m));
let f = b.func(Name::new(names.intern("f")), &[], fbody);
let root = b.module(None, &[f]);
assert!(b.finish(root).is_ok());
```

## Types and effect sets

### `Ty`

`Infer`, `Prim(Prim)`, `Path(PathId)`, `Tuple(List<TyId>)`,
`Array { elem, len }` (`len` a constant context), `Slice(TyId)`,
`Ref { mutable, region: Option<PathId>, inner }`, `Ptr { mutable, inner }`,
`Fn { params, ret, effects, throws }`, `Nullable(TyId)`, `Any` (gradual),
`Object(List<TyId>)`, `Never`, `SelfTy`, `Const(ExprId)` (a constant generic
argument), `Err`. Every annotation slot in HIR is optional; dynamic languages
create no types at all.

### Effects

```rust,ignore
pub struct Effects(/* private */);   // NONE, THROWS, ASYNC, YIELD, UNSAFE
pub const fn union(self, other: Effects) -> Effects
pub const fn contains(self, other: Effects) -> bool
pub const fn is_empty(self) -> bool
```

```rust
use hir_lang::Effects;

let fx = Effects::ASYNC.union(Effects::THROWS);
assert!(fx.contains(Effects::THROWS) && !fx.contains(Effects::YIELD));
```

## Names, binders, paths

### `Binder`

```rust,ignore
pub struct Binder { pub name: Name, pub kind: BinderKind, pub mutable: bool }
```

`Binder::new(name, kind)`, `with_mutable(bool)`.
`BinderKind = Local | Param | Capture | TypeParam | ConstParam | Region | Label`,
with `is_value()` (`Local`, `Param`, `Capture`), `is_type_level()`
(`TypeParam`, `ConstParam`, `Region`), and `name()`.

### `Path`

```rust,ignore
pub struct Path { pub segments: List<Segment>, pub ns: Ns, pub res: Res, pub global: bool }
pub struct Segment { pub name: Name, pub args: List<TyId>, pub origin: Origin }
```

At least one segment. `Segment::new(name, origin)` has no generic arguments.

### `Ns`

`Value` (expression paths, capture sources), `Type` (type paths, record
expression/pattern paths), `Pattern` (constructor and constant patterns),
`Region`, `Import`. Fixed by the parent node; `name()` spells it.

### `Res`

`Unresolved` (default), `Local(BinderId)`, `Item(ItemId)`, `Variant(VariantId)`,
`Prim(Prim)`, `Err`; `is_unresolved()`. Allowed per namespace:

| `ns` | `Local` kinds | `Item` kinds | `Variant` | `Prim` |
|---|---|---|---|---|
| `Value` | `Local` `Param` `Capture` `ConstParam` | `Fn` `Const` `Global` `Record` `Class` `Err` | yes | no |
| `Type` | `TypeParam` | `Record` `Sum` `Class` `Interface` `Alias` `AssocType` `Err` | yes | yes |
| `Pattern` | none | `Const` `Record` `Class` `Err` | yes | no |
| `Region` | `Region` | none | no | no |
| `Import` | none | all but `Impl` and `Import` | yes | no |

## Literals and primitive types

### `Prim`

`Bool`, `I8`…`I64`, `Isize`, `U8`…`U64`, `Usize`, `F32`, `F64`, `Char`, `Str`;
`is_int`, `is_signed`, `is_float`, `bits` (`isize`/`usize` count as 64), `name`.

### `Lit`

`Null`, `Bool(bool)`, `Int(IntLit)`, `Float(FloatLit)`, `Char(char)`,
`Str(TextRef)` (UTF-8), `Bytes(TextRef)`, `BigInt(TextRef)` (decimal digits,
optional `-`).

- `IntLit { value: u64, negative: bool, suffix: Option<Prim> }`: `IntLit::new`,
  `IntLit::signed(i64)`, `with_suffix`, `fits(Prim)`. A suffixed literal must fit.
- `FloatLit { bits: u64, suffix: Option<Prim> }`: `FloatLit::new(f64)`,
  `with_suffix`, `value`, `is_exact` (an `f32` literal must be exact in `f32`).

```rust
use hir_lang::{FloatLit, IntLit, Prim};

assert!(IntLit::signed(-128).with_suffix(Prim::I8).fits(Prim::I8));
assert!(!FloatLit::new(0.1).with_suffix(Prim::F32).is_exact());
```

## Operations

### `Op`

```rust,ignore
pub struct Op { pub kind: OpKind, pub policy: Policy }
pub const fn new(kind: OpKind) -> Op            // OPS default policy
pub const fn with_overflow(self, Overflow) -> Op
pub const fn with_div_zero(self, DivZero) -> Op
pub const fn with_shift(self, Shift) -> Op
pub const fn with_float_to_int(self, FloatToInt) -> Op
pub const fn policy_matches(self) -> bool       // exactly the fields the op consults
pub const fn promotes(self) -> bool             // overflow policy is `promote`
```

### `OpKind`

Every operation of `specs/OPS.md` §3–§5: `Add Sub Mul Neg Abs Div FloorDiv Rem
FloorMod And Or Xor Not Shl Shr Eq Ne Lt Le Gt Ge Min Max IeeeRem Sqrt Fma Floor
Ceil Trunc Round RoundEven TotalCmp`, and the conversions `IntCast(Prim)
FloatToInt(Prim) Zext(Prim) Sext(Prim) Narrow(Prim) IntToFloat(Prim)
Bitcast(Prim) BoolToInt(Prim) F32ToF64 F64ToF32 CharFromU32`. `Narrow` is OPS's
integer `trunc` conversion. Methods: `name()` (OPS spelling), `arity()`,
`target()`, `default_policy()`.

| Ops | Policy fields |
|---|---|
| `add sub mul neg abs int_cast` | `overflow` |
| `div floor_div` | `overflow`, `div_zero` |
| `rem floor_mod` | `div_zero` |
| `shl shr` | `shift` |
| `float_to_int` | `float_to_int` |
| everything else | none |

### `Policy`

```rust,ignore
pub struct Policy { pub overflow: Option<Overflow>, pub div_zero: Option<DivZero>,
                    pub shift: Option<Shift>, pub float_to_int: Option<FloatToInt> }
```

`Policy::NONE`, `Policy::CAST` (what a cast carries), and `with_*` setters.
`Overflow = Error | Wrap | Trap | Promote`, `DivZero = Error | Trap`,
`Shift = Error | Mask`, `FloatToInt = Error | Saturate`.

```rust
use hir_lang::{Op, OpKind, Overflow, Policy};

assert!(Op::new(OpKind::Div).policy_matches());
assert!(!Op { kind: OpKind::Add, policy: Policy::NONE }.policy_matches());
assert_eq!(Op::new(OpKind::Add).with_overflow(Overflow::Wrap).policy.overflow, Some(Overflow::Wrap));
```

## Ids, lists, and node references

### Ids

`ItemId`, `ExprId`, `StmtId`, `PatId`, `TyId`, `PathId`, `FieldId`,
`VariantId`, `ParamId`, `BinderId`: 4-byte `Copy` handles (`Option<Id>` is also
4 bytes) with `index()` and `from_index(usize) -> Option<Self>` (for side
tables, decoders, and tests; the validator checks every stored id).

### `NodeRef`

The sum of the nine node ids; `index()`, `kind() -> IdKind`. Ordered by kind,
then index.

### `IdKind`

`Item Expr Stmt Pat Ty Path Field Variant Param Binder Expansion`; used by
[`Hir::count`](#node-accessors) and errors. `Display` spells it.

### `List`

`List<T>`: 8 bytes, `EMPTY`, `len`, `is_empty`, `start`, `from_raw(start, len)`.
Made by [`Builder::list`](#builderlist), read by [`Hir::list`](#hirlist--hirtext--hirstr).

### `TextRef`

A run of the text pool: `from_raw`, `start`, `len`, `is_empty`.

### `Pooled`

The sealed trait of list element types: every id that appears in lists and
`Arg`, `FieldInit`, `MapEntry`, `Arm`, `Capture`, `GenericParam`, `WherePred`,
`Segment`, `FieldPat`, `Attr`, `AttrArg`.

## Origins

### `Origin`

`Origin { span: Span, expn: ExpnId }`; `Origin::new(span)` (source),
`Origin::expanded(span, expn)`, `Default` (`0..0` in source).

### `ExpnId`

`ROOT` (source text), `is_root`, `as_u32`, `from_u32`. Doubles as a hygiene mark.

### `Expansion`

`Expansion { kind: ExpnKind, name: Symbol, call_site: Span, parent: ExpnId, def_site: ExpnId }`,
`ExpnKind = Macro | Template | Desugar`.

### `Name`

`Name { sym: Symbol, mark: ExpnId }`: `Name::new(sym)` (source),
`Name::marked(sym, mark)`. Equal only if both parts are equal.

### `Ident`

`Ident { sym: Symbol, span: Span }`: a member name (field, method, named
argument, attribute) with its own span; no mark.

## Traversal

`Event = Enter(NodeRef) | Leave(NodeRef)` (`node()`), and
`Control = Continue | Skip | Stop` (`Skip` skips the children of the entered
node; its `Leave` still arrives).

```rust
use hir_lang::{Builder, Control, Event, NodeRef};

let mut b = Builder::new();
let one = b.int(1);
let body = b.block(&[], Some(one));
let f = b.func(hir_lang::Name::new(intern_lang::Interner::new().intern("f")), &[], body);
let root = b.module(None, &[f]);
let hir = b.finish(root)?;
let mut entered = 0;
hir.walk_from(NodeRef::Item(root), |e| {
    if let Event::Enter(n) = e {
        entered += 1;
        if n == NodeRef::Item(f) { return Control::Skip; }
    }
    Control::Continue
});
assert_eq!(entered, 2); // the module and the skipped fn
# Ok::<(), hir_lang::HirError>(())
```

## Printing

One node per line: `(kind inline-data`, children indented two spaces, `)`
appended at the node's end. Paths print inline on their parent's line;
binders print as `name%id`, marks as `'eN`, resolutions as `→ local x%3`,
`→ item 4`, `→ variant 2`, `→ prim i32`; non-node children appear as groups
(`(arm …)`, `(guard …)`, `(arg name …)`, `(capture x%2 by-value …)`).
Indentation stops growing after 64 levels, so the output is linear in the node
count at any depth. The output is identical on every platform.

`PrintOptions` (`#[non_exhaustive]`, `Default`): `origins: bool`
(`with_origins`) appends ` @start..end` and `#eN` to each node.

```rust
use hir_lang::{Builder, Name, PrintOptions, Span};
use intern_lang::Interner;

let mut names = Interner::new();
let mut b = Builder::new();
b.set_span(Span::new(0, 14));
let body = b.block(&[], None);
let f = b.func(Name::new(names.intern("main")), &[], body);
let root = b.module(None, &[f]);
let hir = b.finish(root)?;
assert_eq!(hir_lang::print(&hir, &names), "(module\n  (fn main\n    (block)))");
assert_eq!(
    hir_lang::print_with(&hir, &names, PrintOptions::default().with_origins(true)),
    "(module @0..14\n  (fn main @0..14\n    (block @0..14)))"
);
# Ok::<(), hir_lang::HirError>(())
```

## Errors

### `HirError`

`#[non_exhaustive]`. Each variant names its site.

| Variant | Meaning | What to do |
|---|---|---|
| `CapacityExceeded { what: Capacity }` | an arena/pool passed `u32::MAX - 1` entries, or the total node count did | split the input |
| `Dangling { site, kind, index }` | an id names nothing | a lowering bug, or ids mixed between builders |
| `ListOutOfBounds { site }`, `TextOutOfBounds { site }` | a list/text range lies outside its pool | same |
| `ExpansionOrder { expn }` | an expansion refers to itself or a later one | record parents first |
| `RootNotModule` | the root is not a module | |
| `SharedNode { node }` | a node has two parents (or is its own ancestor) | build a fresh node per use |
| `Unreachable { node }` | a node is not in the tree | attach or drop it |
| `Malformed { site, problem: Malformed }` | a node's own shape is wrong | see below |
| `DuplicateName { node, name }` | two members of one list share a name | |
| `Policy { expr }` | an op/cast/compound assignment lacks or has extra policy fields | use `Op::new` defaults |
| `Arity { expr, expected, found }` | wrong operand count | |
| `BinderNotBound { binder }` | a binder has no binding site | |
| `BinderBoundTwice { binder, node }` | bound by two constructs | |
| `DuplicateBinding { binder, pat }` | bound twice in one pattern alternative | |
| `OrPatternBinders { pat }` | alternatives bind different sets | |
| `BinderKind { binder, expected }` | kind does not fit the site | |
| `PathNamespace { path, expected }` | namespace does not match the parent | |
| `Resolution { path, res }` | the namespace cannot name `res` | |
| `OutOfScope { path, binder }` | binder not in scope at the path | |
| `NotCapturable { path, binder }` | binder behind a nested item, a constant context, or a non-capturing closure | |
| `Jump { expr, problem: JumpProblem }` | `BreakOutsideLoop`, `ContinueOutsideLoop`, `LabelNotInScope`, `ContinueToBlock`, `OutOfDefer` | |
| `Effect { expr, problem: EffectProblem }` | `ReturnOutsideFunction`, `AwaitOutsideAsync`, `YieldOutsideGenerator`, `ThrowNotAllowed` | declare the effect or wrap in `try` |

### `Malformed`

`#[non_exhaustive]`: `EmptyPath`, `EmptyOrPattern`, `RestOutOfRange`,
`RangeWithoutBounds`, `RangeBoundKinds`, `LiteralOutOfRange`, `LiteralSuffix`,
`InexactF32`, `InvalidUtf8`, `InvalidBigInt`, `ConversionTarget`,
`AssignTarget`, `CompoundAssignOp`, `LetElseWithoutInit`, `ShapeFields`,
`MissingName`, `UnexpectedName`, `MissingBody`, `MissingConstValue`,
`ItemPlacement`, `ReceiverPlacement`, `ParamOrder`, `ParamDefault`,
`InferCapture`, `EmptyAttrArg`, `AttrOrder`, `PromoteOnStaticResult`.

`Overflow::Promote` (OPS §2, for Mox/PHP: an overflowing integer result becomes
the nearest `f64`) needs a dynamically typed result. The validator rejects it
with `PromoteOnStaticResult` where HIR fixes a static result type: on
`int_cast<T>`, on a cast to anything but `Any`, and directly inside an integer
constant context (array length, const generic argument, discriminant). Static
results known only after inference are typeck-lang's to reject.

### `Site`, `Capacity`, `JumpProblem`, `EffectProblem`

`Site = Node(NodeRef) | Binder(BinderId) | Expansion(ExpnId) | Root | Attrs(NodeRef)`;
`Capacity = Arena(IdKind) | Pool | Text | Total` (`#[non_exhaustive]`);
`JumpProblem` and `EffectProblem` as listed above (`#[non_exhaustive]`). All
error types implement `Display`; `HirError` implements `core::error::Error`.

```rust
use hir_lang::{Builder, Expr, HirError, List, Malformed, Ns, Path, Res};

let mut b = Builder::new();
let empty = b.path(Path { segments: List::EMPTY, ns: Ns::Value, res: Res::Unresolved, global: false });
let e = b.expr(Expr::Path(empty));
let body = b.block(&[], Some(e));
let f = b.func(hir_lang::Name::new(intern_lang::Interner::new().intern("f")), &[], body);
let root = b.module(None, &[f]);
let err = b.finish(root).unwrap_err();
assert!(matches!(err, HirError::Malformed { problem: Malformed::EmptyPath, .. }));
assert_eq!(err.to_string(), "path 0: a path has no segments");
```

## Re-exports

| Item | From | Why |
|---|---|---|
| `Span` | `span-lang` 0.4 | origins; the version syntax-lang and diag-lang use |
| `Symbol` | `intern-lang` 1 | names |

## Feature flags

| Feature | Default | Effect |
|---|---|---|
| `std` | yes | Forwards to `span-lang/std` and `intern-lang/std`. Without it the crate is `no_std` and needs only `alloc`; the API is identical. |

## Limits

| Limit | Value | Error |
|---|---|---|
| Entries per arena, pool, or the text pool | `u32::MAX - 1` | `CapacityExceeded` |
| Total nodes across arenas | `u32::MAX - 1` | `CapacityExceeded { what: Total }` |
| Nesting depth | none (every algorithm is iterative) | — |

## Stability

Pre-1.0. The node forms are deliberately **not** `#[non_exhaustive]`: every
consumer must handle every form, and a new form is a breaking change for all of
them regardless. Error enums and `PrintOptions` are `#[non_exhaustive]`. The
canonical order and the printed form are part of the contract (snapshot tests
depend on them). The 0.5 textual syntax and binary encoding (spec §20–§21) are
additive.
