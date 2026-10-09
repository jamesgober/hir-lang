# hir-lang &mdash; API Reference

> Complete reference for every public item in `hir-lang`, with examples.
> **Status: pre-1.0 (0.4.0).** 0.4 is a breaking revision of 0.3 for gaps the
> LSF2 spec and the Mox sketch found: union and intersection types, place-once
> compound assignment and the value of assignments, keyed yields and
> `yield from`, logical versus bitwise `not`, `pow` and `shift = saturate`
> (OPS v2), lenient repair in at most two rounds, and `lookup_local_in`. (0.3
> closed the review gaps of 0.2: units, partial resolution, lenient
> validation, PHP/Mox and low-level forms.) The surface is frozen only after lower-lang,
> resolve-lang, and typeck-lang have used it end to end (LexerSketch decision
> D18). The normative definition of HIR is the LexerSketch spec
> `specs/HIR.md`; this file documents the Rust API that implements it. See
> [`../dev/ROADMAP.md`](../dev/ROADMAP.md).

<sub>Copyright &copy; 2026 <strong>James Gober</strong>.</sub>

## Table of contents

- [Overview](#overview)
- [Installation](#installation)
- [Quick start](#quick-start)
- [Concepts](#concepts)
  - [Storage: arenas, ids, lists](#storage-arenas-ids-lists)
  - [Units and definition ids](#units-and-definition-ids)
  - [Origins and expansions](#origins-and-expansions)
  - [Binders, scopes, and frames](#binders-scopes-and-frames)
  - [Resolution](#resolution)
  - [Operations and policies](#operations-and-policies)
  - [Effects](#effects)
  - [Strict and lenient validation](#strict-and-lenient-validation)
  - [Canonical order](#canonical-order)
- [`Builder`](#builder)
- [`Hir`](#hir)
- [Free functions](#free-functions)
- [Items](#items)
- [Expressions and statements](#expressions-and-statements)
- [Intrinsics and inline assembly](#intrinsics-and-inline-assembly)
- [Patterns](#patterns)
- [Types, bounds, generic arguments, effect sets](#types-bounds-generic-arguments-effect-sets)
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
[`finish`](#builderfinish--builderfinish_lenient) validates and returns a
[`Hir`](#hir), which can be read, walked, printed, and resolved in place.

| Item | Kind | Purpose |
|---|---|---|
| [`Builder`](#builder) | struct | Creates nodes, lists, text, attributes, expansions; copies subtrees; `finish` / `finish_lenient` validate. |
| [`Hir`](#hir) | struct | A validated HIR of one unit: accessors, `resolve`/`resolve_partial`, `lookup_local`/`lookup_local_in`, `can_reference`, implicit captures, walking. |
| [`walk`](#walk), [`print`](#print), [`print_with`](#print_with), [`print_into`](#print_into) | functions | The Tier-1 traversal and the debug printer. |
| [`UnitId`](#unitid), [`DefId`](#defid), [`Def`](#def) | types | Compilation units and definitions across units. |
| [`Item`](#item), [`ItemKind`](#itemkind) and the `*Def` records | types | Declarations. |
| [`Expr`](#expr), [`Stmt`](#stmt) and their records | enums | Expressions and statements. |
| [`Intrinsic`](#intrinsic), [`Asm`](#asm) | types | Atomics, volatile, named intrinsics; structured inline assembly. |
| [`Pat`](#pat) and its records | enum | Patterns. |
| [`Ty`](#ty), [`Bound`](#bound), [`GenericArg`](#genericarg), [`Effects`](#effects-1) | types | Type terms (all optional), bounds, generic arguments, effect sets. |
| [`Binder`](#binder), [`Path`](#path), [`PathRoot`](#pathroot), [`QSelf`](#qself), [`Res`](#res), [`Ns`](#ns) | types | Binding and name references. |
| [`Lit`](#lit), [`Prim`](#prim) | types | Literals and primitive types. |
| [`Op`](#op), [`OpKind`](#opkind), [`Policy`](#policy) | types | Intrinsic operations from `specs/OPS.md`. |
| [Ids](#ids), [`NodeRef`](#noderef), [`List`](#list), [`TextRef`](#textref) | types | Dense typed handles. |
| [`Origin`](#origin), [`Expansion`](#expansion), [`Name`](#name), [`Ident`](#ident) | types | Where nodes came from; hygienic names. |
| [`Event`](#event), [`Frame`](#frame), [`Control`](#control) | enums | Walk events, including scopes, binders, and frames. |
| [`HirError`](#hirerror) and its details | enums | Why a HIR or a resolution was rejected. |

## Installation

```toml
[dependencies]
hir-lang = "0.4"
intern-lang = "1"   # names are intern_lang::Symbol
```

Without the standard library:

```toml
[dependencies]
hir-lang = { version = "0.4", default-features = false }
```

## Quick start

```rust
use hir_lang::{Builder, Expr, Name, OpKind, Res};
use intern_lang::Interner;

let mut names = Interner::new();
let mut b = Builder::new();

// fn inc(n) { n + 1 }   with `n` left for a resolver to resolve
let n_name = Name::new(names.intern("n"));
let (param, _n) = b.local_param(n_name);
let use_n = b.name_expr(n_name);
let one = b.int(1);
let sum = b.op(OpKind::Add, &[use_n, one]);
let body = b.block(&[], Some(sum));
let inc = b.func(Name::new(names.intern("inc")), &[param], body);
let root = b.module(None, &[inc]);

let mut hir = b.finish(root)?;            // validated
let Expr::Path(path) = *hir.expr(use_n) else { unreachable!() };
let n = hir.lookup_local(path, n_name).expect("in scope");  // the scope rule, reused
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

The **live** nodes form one tree rooted at a module: every live node except the
root has exactly one parent and is reachable. Sharing a node between two
parents and cycles are rejected. Unreachable nodes are rejected too, except
**dead error forms** (an error item, expression, statement, pattern, type, or
path; a field or unit variant with nothing in it; a parameter whose pattern is
`Pat::Err`), which lenient repairs leave behind and which are harmless.

### Units and definition ids

A `Hir` is one compilation unit, identified by a host-assigned
[`UnitId`](#unitid) (`Builder::for_unit`; `Builder::new` builds unit 0).
Resolutions name items and sum variants only through a [`DefId`](#defid):
`(unit, Def::Item | Def::Variant)`. A `DefId` for this unit is checked (the
definition exists and fits the namespace); one for another unit is accepted
as is, for the host to check against that unit.

Ids are unforgeable across `Hir`s of the same unit: [`Builder::def`](#builderunit--builderdef)
and [`Hir::def`](#hirunit--hirdef) mint ids carrying a private tag of their
issuer, and a tagged `DefId` from a different `Hir` of this unit is rejected
with [`ForeignDef`](#hirerror). [`DefId::foreign`](#defid) makes untagged ids
(for other units and for decoders). Equality and hashing ignore the tag.

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

Variables, parameters, captures, generic parameters, place aliases, and labels are
[`Binder`](#binder)s with unique ids. Each is bound by at most one construct
(the alternatives of an or-pattern bind the same set, with the same modes); a
binder bound nowhere is allowed and is in no scope. A binder is visible:

| Bound by | Visible in |
|---|---|
| `let` pattern, `static`, `global` statement | later statements and the tail of the block |
| parameter pattern | later parameters' defaults (per-call evaluation), the return/throws types, the body |
| `match`/`catch` arm pattern | the arm's guard and body |
| explicit capture, a closure's `self_binder` | the closure's parameters and body |
| [`Expr::LetPlace`](#expr) binder (kind `Place`) | the `let_place` body |
| generic parameter | the whole item |
| loop/block label | the loop's body and step, or the block |

**Frames** further limit what can be referenced: items, closures, parameter
defaults, and constant contexts (array lengths, const generic arguments,
discriminants, field defaults) each start a frame. Value binders cross only
closures that allow implicit captures, and parameter defaults; type-level
binders also cross constant contexts and the member items of an impl,
interface, or class. A `Place` binder crosses **no** frame (its operands live
in the frame that evaluated them). A default evaluated once at definition
([`DefaultEval::Once`](#expr-records), Python) cannot see the function's own
parameters. A module with a body is a frame like a function's.

### Resolution

A name reference is a [`Path`](#path) node with a namespace ([`Ns`](#ns), fixed
by its parent), a root (`::`, `self::`, `super::`, `Self::`, `parent::`,
`static::`), an optional qualified self (`<T as Tr>::`), and a resolution slot:
a [`Res`](#res) plus the number of trailing segments left **unresolved** for
type-directed resolution (`Vec::new`, `T::Item`, `Self::Output`).

Lowering may fill slots it knows (template temporaries); resolve-lang fills
the rest with [`Hir::resolve`](#hirresolve--hirresolve_partial) /
`resolve_partial`, which check shape, namespace, scope, and frames in O(1)
using the index the validator computed. [`Hir::lookup_local`](#hirlookup_local--hirlookup_local_in)
answers "which binder named `x` is in scope here" in O(log n + d) (and
`lookup_local_in` the same in another namespace, for `T::new`), and the walk
reports [scope, binder, and frame events](#event), so the resolver never
re-derives the scope rules. The HIR stays valid after every call.

### Operations and policies

Arithmetic and conversions are [`Op`](#op)s named after `specs/OPS.md`. Each op
instance carries **exactly** the policy fields its operation consults
(`overflow`, `div_zero`, `shift`, `float_to_int`), fixed at lowering so every
execution tier behaves the same. [`Op::new`](#op) gives the OPS defaults.

### Effects

`throw`, `try`, `await`, `yield` (with an optional key), `yield from`,
`spawn`, and `defer` are explicit forms. Functions, closures, and module bodies
declare [`Effects`](#effects-1); the validator accepts `await` only in `ASYNC`
frames, `yield` and `yield from` only in `YIELD` frames, and `throw` only in
`THROWS` frames or inside a `try` body of the same frame. A `yield` inside a
`finally` or `defer` is an ordinary suspension (LSB rule 12); only a generator
that yields while being closed fails, at run time. `ASYNC | YIELD` is an async
generator. `return` needs a function, closure, or module body; jumps may leave
a `finally` (overriding the pending completion) but never a `defer`. A `match`
with no matching arm raises the runtime error `NoMatch` (E0200, spec §8.10).

Declared effects describe the explicit forms only: any fallible operation can
raise at run time per the language's error model, and the body graph (spec
§19, hir-lang 0.5) gives each one an unwind edge.

### Strict and lenient validation

[`Builder::finish`](#builderfinish--builderfinish_lenient) is strict: it
returns the first problem. [`Builder::finish_lenient`](#builderfinish--builderfinish_lenient)
collects every problem, repairs each (the offending node becomes its error
form, or a narrower fix: an error resolution, a wildcard binding, a corrected
binder kind), and returns the valid `Hir` with the problems in source order;
it fails only on capacity overflow or a root that is missing or not a module.
Each round also removes the consequences of its own repairs (nodes cut off by a
repair, references to binders whose site disappeared) without reporting them,
so at most **two rounds** are needed: one for the node-local checks, one for
the tree walk (spec §13.2). Both modes are total (never panic) and linear.
Before validating, both normalize union and intersection types (spec §5.1).
The checks, in order:

1. every id, list, and text range in bounds; expansions ordered;
2. each node's own shape: literals, op arity and policies, assignment and
   place forms (including `let_place`), parameter order, record shapes, unique
   member names, asm templates, intrinsic orderings, keyed yields, and the
   canonical form of unions and intersections;
3. one walk checking the tree, binder sites, namespaces and path shapes,
   jumps, effects, and item placement;
4. unreachable live nodes (each orphaned subtree once, at its top);
5. the scope and frames of every resolved local.

### Canonical order

Children are visited in the order a node's fields are listed (which is also
evaluation order). The walker, the validator's scopes, the error order, and the
printer all derive from one definition of that order.

## `Builder`

```rust,ignore
pub struct Builder { /* private */ }
```

Builds a HIR bottom-up. It never panics: an arena that would outgrow `u32`
indexes is recorded and reported by `finish`. Ids it issues are only
meaningful for the HIR it finishes.

### `Builder::new` / `Builder::for_unit`

```rust,ignore
pub fn new() -> Builder
pub fn for_unit(unit: UnitId) -> Builder
```

An empty builder whose current origin is `0..0` in source. `new` (also
`Default`) builds unit 0; `for_unit` builds the given unit.

```rust
use hir_lang::{Builder, Origin, UnitId};

assert_eq!(Builder::new().origin(), Origin::default());
assert_eq!(Builder::for_unit(UnitId::new(3)).unit(), UnitId::new(3));
```

### `Builder::unit` / `Builder::def`

```rust,ignore
pub fn unit(&self) -> UnitId
pub fn def(&self, def: Def) -> DefId
```

`def` mints a [`DefId`](#defid) for a definition of this unit, tagged so that a
different `Hir` of the same unit rejects it.

```rust
use hir_lang::{Builder, Def, DefId, Item, ItemKind, Name, Ns, Res, UnitId};
use intern_lang::Interner;

let mut names = Interner::new();
let mut b = Builder::for_unit(UnitId::new(1));
let max = b.item(Item::new(Some(Name::new(names.intern("MAX"))), ItemKind::Const { ty: None, value: None }));
assert_eq!(b.def(Def::Item(max)), DefId::foreign(UnitId::new(1), Def::Item(max)));
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

Store literal payloads (strings; byte strings, big-integer digits, asm
templates and constraints).

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

### `Builder::copy_subtree`

```rust,ignore
pub fn copy_subtree(&mut self, node: NodeRef) -> NodeRef
```

Copies a subtree for template instantiation, inlining, or unrolling. Every
binder **bound inside** it gets a fresh binder in the copy (references inside
follow); references to outer binders are kept; origins and attributes are
copied. The copy is unattached. Iterative. On a foreign id or overflow, the
overflow is recorded (reported by `finish`) and `node` is returned.

```rust
use hir_lang::{BinderKind, Builder, Expr, Name, NodeRef};
use intern_lang::Interner;

let mut names = Interner::new();
let mut b = Builder::new();
let t = b.new_binder(Name::new(names.intern("t")), BinderKind::Local);
let pat = b.bind(t);
let one = b.int(1);
let decl = b.let_stmt(pat, Some(one));
let use_t = b.use_binder(t);
let original = b.block(&[decl], Some(use_t));
let NodeRef::Expr(copy) = b.copy_subtree(NodeRef::Expr(original)) else { unreachable!() };
let pair = b.list(&[original, copy]);
let both = b.expr(Expr::Tuple(pair));
let body = b.block(&[], Some(both));
let f = b.func(Name::new(names.intern("f")), &[], body);
let root = b.module(None, &[f]);
assert_eq!(b.finish(root)?.count(hir_lang::IdKind::Binder), 2);
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
| `call(callee, &[ExprId]) -> ExprId` | `Expr::Call` with positional, by-value arguments |
| `new_binder(name, kind) -> BinderId` | an immutable binder |
| `bind(binder) -> PatId` | a by-value `Pat::Bind` |
| `local_param(name) -> (ParamId, BinderId)` | a `Normal` parameter binding a fresh `Param` binder |
| `name_path(name, ns) -> PathId` | an unresolved one-segment relative path |
| `resolved_path(name, ns, res) -> PathId` | a one-segment path with a full resolution |
| `name_expr(name) -> ExprId` | an expression naming `name`, unresolved |
| `use_binder(binder) -> ExprId` | an expression already resolved to `binder` (hygiene by construction) |
| `block(&[StmtId], Option<ExprId>) -> ExprId` | an unlabeled block |
| `let_stmt(pat, Option<ExprId>) -> StmtId` | `let pat = init;` |
| `expr_stmt(ExprId) -> StmtId` | an expression statement |
| `func(name, &[ParamId], body) -> ItemId` | a private function with no generics or effects |
| `module(Option<Name>, &[ItemId]) -> ItemId` | a private module without a body |

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

### `Builder::finish` / `Builder::finish_lenient`

```rust,ignore
pub fn finish(self, root: ItemId) -> Result<Hir, HirError>
pub fn finish_lenient(self, root: ItemId) -> Result<(Hir, Vec<HirError>), HirError>
```

Validate everything built with `root` (a module) as the root.

`finish` is strict. **Errors:** [`CapacityExceeded`](#hirerror) if building
overflowed; otherwise the first problem (see
[Strict and lenient validation](#strict-and-lenient-validation)).

`finish_lenient` is for user code: every problem is collected and repaired,
and the repaired, valid `Hir` is returned with the problems sorted by source
position (span start, then discovery order). Consequences of a repair (the
children of a replaced node, references to binders it bound) are fixed in the
same round, silently; at most two rounds are needed (spec §13.2). **Errors:**
only where no repair exists: `CapacityExceeded`, a missing root (`Dangling`),
or `RootNotModule`.

Both first bring every union and intersection into canonical form (flatten,
hoist `Nullable` out of unions, sort, drop repeats, collapse a single member;
spec §5.1), so lowering builds them in source order and never compares types.

```rust
use hir_lang::{Builder, Expr, HirError, IntLit, JumpProblem, Lit, NodeRef, Prim};

// strict: an orphan expression is rejected
let mut b = Builder::new();
let orphan = b.int(1);
let root = b.module(None, &[]);
assert_eq!(b.finish(root), Err(HirError::Unreachable { node: NodeRef::Expr(orphan) }));

// lenient: a stray `break` and an out-of-range literal become diagnostics
let mut b = Builder::new();
let stray = b.expr(Expr::Break { label: None, value: None });
let big = b.lit(Lit::Int(IntLit::new(300).with_suffix(Prim::U8)));
let (s1, s2) = (b.expr_stmt(stray), b.expr_stmt(big));
let body = b.block(&[s1, s2], None);
let f = b.func(hir_lang::Name::new(intern_lang::Interner::new().intern("f")), &[], body);
let root = b.module(None, &[f]);
let (hir, problems) = b.finish_lenient(root)?;
assert_eq!(problems.len(), 2);
assert!(problems.contains(&HirError::Jump { expr: stray, problem: JumpProblem::BreakOutsideLoop }));
assert_eq!(hir.expr(stray), &Expr::Err);
# Ok::<(), HirError>(())
```

## `Hir`

```rust,ignore
pub struct Hir { /* private */ }   // Clone, Debug, PartialEq, Eq
```

A validated HIR of one unit. Accessors never panic in release builds: a
foreign id reads as an error node (`ItemKind::Err`, `Expr::Err`, `Stmt::Err`,
`Pat::Err`, `Ty::Err`, the error path, an empty field, a `Pat::Err`
parameter) and is a `debug_assert!` failure in debug builds. The `get_*`
variants return `None` instead. Records without an error form (variants,
binders, expansions) are `Option`-only.

### Node accessors

```rust,ignore
pub fn root(&self) -> ItemId
pub fn item(&self, id: ItemId) -> &Item        pub fn get_item(&self, id: ItemId) -> Option<&Item>
pub fn expr(&self, id: ExprId) -> &Expr        pub fn get_expr(&self, id: ExprId) -> Option<&Expr>
pub fn stmt(&self, id: StmtId) -> &Stmt        pub fn get_stmt(&self, id: StmtId) -> Option<&Stmt>
pub fn pat(&self, id: PatId) -> &Pat           pub fn get_pat(&self, id: PatId) -> Option<&Pat>
pub fn ty(&self, id: TyId) -> &Ty              pub fn get_ty(&self, id: TyId) -> Option<&Ty>
pub fn path(&self, id: PathId) -> &Path        pub fn get_path(&self, id: PathId) -> Option<&Path>
pub fn field(&self, id: FieldId) -> &FieldDef  pub fn get_field(&self, id: FieldId) -> Option<&FieldDef>
pub fn param(&self, id: ParamId) -> &Param     pub fn get_param(&self, id: ParamId) -> Option<&Param>
pub fn variant(&self, id: VariantId) -> Option<&Variant>
pub fn binder(&self, id: BinderId) -> Option<&Binder>
pub fn expansion(&self, id: ExpnId) -> Option<&Expansion>
pub fn variant_owner(&self, id: VariantId) -> Option<ItemId>
pub fn count(&self, kind: IdKind) -> usize
```

`count` is the arena length: ids of that kind are exactly `0..count`, so side
tables are plain vectors. `variant_owner` returns the sum declaring a variant.

```rust
use hir_lang::{Builder, Expr, ExprId, IdKind, ItemKind};

let mut b = Builder::new();
let one = b.int(1);
let body = b.block(&[], Some(one));
let f = b.func(hir_lang::Name::new(intern_lang::Interner::new().intern("f")), &[], body);
let root = b.module(None, &[f]);
let hir = b.finish(root)?;
assert!(matches!(hir.item(f).kind, ItemKind::Fn(_)));
assert!(matches!(hir.expr(one), Expr::Lit(_)));
assert_eq!(hir.count(IdKind::Expr), 2);
assert!(hir.get_expr(ExprId::from_index(99).unwrap()).is_none());
# Ok::<(), hir_lang::HirError>(())
```

### `Hir::unit` / `Hir::def`

```rust,ignore
pub fn unit(&self) -> UnitId
pub fn def(&self, def: Def) -> DefId
```

The unit, and a tagged [`DefId`](#defid) for one of its definitions.

```rust
use hir_lang::{Builder, Def, UnitId};

let mut b = Builder::for_unit(UnitId::new(4));
let root = b.module(None, &[]);
let hir = b.finish(root)?;
assert_eq!(hir.def(Def::Item(root)).unit(), UnitId::new(4));
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

### `Hir::resolve` / `Hir::resolve_partial`

```rust,ignore
pub fn resolve(&mut self, path: PathId, res: Res) -> Result<(), HirError>
pub fn resolve_partial(&mut self, path: PathId, res: Res, unresolved: u32) -> Result<(), HirError>
```

Set a path's resolution after an O(1) check. `resolve` resolves every
segment; `resolve_partial` resolves the first `segments - unresolved` and
leaves the rest for type-directed resolution. On error nothing changes.

| Error | When |
|---|---|
| [`Dangling`](#hirerror) | `path`, or the binder or this unit's definition in `res`, does not exist |
| [`ForeignDef`](#hirerror) | `res` names this unit through a `DefId` minted by another `Hir`/builder |
| [`Malformed`](#hirerror) (`PathShape`) | `unresolved` does not fit the path's segments, root, or qualified self |
| [`Resolution`](#hirerror) | the namespace cannot name `res` (with unresolved segments: `res` has no associated items) |
| [`OutOfScope`](#hirerror) | `res` is a binder not in scope at the path |
| [`NotCapturable`](#hirerror) | `res` is a binder behind a frame the path may not cross |

```rust
use hir_lang::{BinderKind, Builder, Def, Expr, HirError, Item, ItemKind, Name, Ns, Path, RecordDef, Res, Segment};
use intern_lang::Interner;

let mut names = Interner::new();
let mut b = Builder::new();
// `Point::new` — resolve-lang binds `Point`, typeck resolves `new`.
let point = b.item(Item::new(Some(Name::new(names.intern("Point"))), ItemKind::Record(RecordDef::default())));
let segs = [
    Segment::new(Name::new(names.intern("Point")), b.origin()),
    Segment::new(Name::new(names.intern("new")), b.origin()),
];
let segments = b.list(&segs);
let ctor = b.path(Path::new(segments, Ns::Value));
let call = b.expr(Expr::Path(ctor));
let s0 = b.expr_stmt(call);
// `x` used before its `let`.
let early = b.name_expr(Name::new(names.intern("x")));
let s1 = b.expr_stmt(early);
let x = b.new_binder(Name::new(names.intern("x")), BinderKind::Local);
let px = b.bind(x);
let one = b.int(1);
let s2 = b.let_stmt(px, Some(one));
let body = b.block(&[s0, s1, s2], None);
let f = b.func(Name::new(names.intern("f")), &[], body);
let root = b.module(None, &[point, f]);
let mut hir = b.finish(root)?;

let point_def = hir.def(Def::Item(point));
hir.resolve_partial(ctor, Res::Def(point_def), 1)?;
assert_eq!(hir.path(ctor).unresolved, 1);

let Expr::Path(p) = *hir.expr(early) else { unreachable!() };
assert_eq!(hir.resolve(p, Res::Local(x)), Err(HirError::OutOfScope { path: p, binder: x }));
assert_eq!(hir.path(p).res, Res::Unresolved);
# Ok::<(), hir_lang::HirError>(())
```

### `Hir::lookup_local` / `Hir::lookup_local_in`

```rust,ignore
pub fn lookup_local(&self, path: PathId, name: Name) -> Option<BinderId>
pub fn lookup_local_in(&self, path: PathId, name: Name, ns: Ns) -> Option<BinderId>
```

The lexically innermost binder named `name` (symbol and mark) in the path's
namespace that is in scope at `path`, in O(log n + d) (d = nesting depth of
same-named binders). Frames are not filtered: a result behind a frame the path
may not cross is then reported by `resolve` as `NotCapturable`.
`lookup_local_in` searches the namespace `ns` instead, for a prefix that lives
elsewhere than the whole path (the type parameter `T` of the value path
`T::new`); `Ns::Pattern` searches value binders, `Ns::Import` finds none.

```rust
use hir_lang::{BinderKind, Builder, Expr, Name};
use intern_lang::Interner;

// { let x = 1; { let x = 2; x } }  — the inner `x` wins.
let mut names = Interner::new();
let x = Name::new(names.intern("x"));
let mut b = Builder::new();
let outer = b.new_binder(x, BinderKind::Local);
let inner = b.new_binder(x, BinderKind::Local);
let (po, pi) = (b.bind(outer), b.bind(inner));
let (one, two) = (b.int(1), b.int(2));
let (so, si) = (b.let_stmt(po, Some(one)), b.let_stmt(pi, Some(two)));
let use_x = b.name_expr(x);
let inner_block = b.block(&[si], Some(use_x));
let body = b.block(&[so], Some(inner_block));
let f = b.func(Name::new(names.intern("f")), &[], body);
let root = b.module(None, &[f]);
let hir = b.finish(root)?;
let Expr::Path(p) = *hir.expr(use_x) else { unreachable!() };
assert_eq!(hir.lookup_local(p, x), Some(inner));
# Ok::<(), hir_lang::HirError>(())
```

### `Hir::can_reference`

```rust,ignore
pub fn can_reference(&self, path: PathId, binder: BinderId) -> bool
```

Whether `resolve(path, Res::Local(binder))` would succeed: the binder's kind
fits the namespace, it is in scope, and no frame forbids it.

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

### `Hir::implicit_captures` / `Hir::all_implicit_captures`

```rust,ignore
pub fn implicit_captures(&self, closure: ExprId) -> Vec<BinderId>
pub fn all_implicit_captures(&self) -> Vec<(ExprId, Vec<BinderId>)>
```

The value binders defined outside a closure that its body or parameter
defaults reference through resolved paths, in first-use order. Explicit
captures and the closure's `self_binder` are excluded. `implicit_captures` is
linear in the closure's size (empty for a non-closure);
`all_implicit_captures` computes every closure's set in one walk (closures in
preorder, only those with captures), linear in the HIR plus the output.

```rust
use hir_lang::{Builder, CaptureMode, Closure, Expr, Name};
use intern_lang::Interner;

let mut names = Interner::new();
let mut b = Builder::new();
let (p, k) = b.local_param(Name::new(names.intern("k")));
let body = b.use_binder(k);
let c = b.expr(Expr::Closure(Closure { implicit: Some(CaptureMode::ByValue), ..Closure::new(body) }));
let fbody = b.block(&[], Some(c));
let f = b.func(Name::new(names.intern("f")), &[p], fbody);
let root = b.module(None, &[f]);
let hir = b.finish(root)?;
assert_eq!(hir.implicit_captures(c), vec![k]);
assert_eq!(hir.all_implicit_captures(), vec![(c, vec![k])]);
# Ok::<(), hir_lang::HirError>(())
```

### `Hir::walk_from` / `Hir::children_into`

```rust,ignore
pub fn walk_from<F: FnMut(Event) -> Control>(&self, start: NodeRef, f: F)
pub fn children_into(&self, node: NodeRef, out: &mut Vec<NodeRef>)
```

`walk_from` delivers [events](#event) for the subtree of `start` in canonical
order with an explicit stack (any depth is safe): node entry and exit, and the
scopes, binders, and frames. `children_into` appends a node's direct children.

```rust
use hir_lang::{Builder, Control, Event, Frame, NodeRef};

let mut b = Builder::new();
let body = b.block(&[], None);
let f = b.func(hir_lang::Name::new(intern_lang::Interner::new().intern("f")), &[], body);
let root = b.module(None, &[f]);
let hir = b.finish(root)?;
let mut events = Vec::new();
hir.walk_from(NodeRef::Item(f), |e| { events.push(e); Control::Continue });
assert_eq!(events.first(), Some(&Event::Enter(NodeRef::Item(f))));
assert!(events.contains(&Event::FrameOpen(Frame::Item(f))));
assert_eq!(events.last(), Some(&Event::Leave(NodeRef::Item(f))));
# Ok::<(), hir_lang::HirError>(())
```

### `Hir::validate`

```rust,ignore
pub fn validate(&self) -> Result<(), HirError>
```

Re-runs the strict validator. Always `Ok` for a `Hir` from this crate
(`resolve` preserves validity, and lenient repairs produce valid HIR);
offered for tests and debug builds of consumers.

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

The Tier-1 traversal: every live node from the root, in canonical preorder.

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
for impls, mixin uses, and glob imports; optional for modules, imports (the
alias), and errors.

### `ItemKind`

| Variant | Fields | Notes |
|---|---|---|
| `Fn(FnDef)` | generics, params, ret, effects, throws, abi, body, defaults | body absent only in an interface, a class (abstract), or with an `abi` |
| `Record(RecordDef)` | generics, shape, fields, is_union | a union has named fields |
| `Sum(SumDef)` | generics, variants | variant names unique |
| `Class(ClassDef)` | generics, bases, interfaces, fields, items, is_abstract, is_final, mixin | members: `Fn`, `Const`, `Global`, `Alias`, `MixinUse` |
| `Interface(InterfaceDef)` | generics, supers, items | members: `Fn`, `Const`, `AssocType` |
| `Impl(ImplDef)` | generics, interface, self_ty, items | members: `Fn`, `Const`, `Alias` |
| `Alias { generics, ty }` | | a type alias or an impl's associated type |
| `AssocType { bounds: List<Bound>, default }` | | only in an interface |
| `Const { ty, value }` | | value absent only in an interface |
| `Global { ty, mutable, init }` | | globals and statics |
| `Module { items, body: Option<ExprId>, effects }` | | the root is a module; `body` is top-level code (scripts, NOML, REPL entries) |
| `Import { path, glob }` | | `Import`-namespace path |
| `MixinUse(MixinUseDef)` | mixins, rules | PHP `use T { … }` in a class; expanded by resolve-lang |
| `Err` | | a declaration that failed to lower |

`ItemKind::name()` returns the printer's spelling (`"fn"`, `"record"`,
`"mixin-use"`, ...).

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
| `FnDef` (`Default`) | `generics: Generics`, `params: List<ParamId>`, `ret: Option<TyId>`, `effects: Effects`, `throws: Option<TyId>`, `abi: Option<Symbol>`, `body: Option<ExprId>`, `defaults: DefaultEval` |
| `RecordDef` (`Default`) | `generics`, `shape: Shape`, `fields: List<FieldId>`, `is_union: bool` |
| `SumDef` (`Default`) | `generics`, `variants: List<VariantId>` |
| `ClassDef` (`Default`) | `generics`, `bases: List<TyId>` (method-resolution order), `interfaces: List<TyId>`, `fields`, `items: List<ItemId>`, `is_abstract`, `is_final`, `mixin: bool` (a PHP trait) |
| `InterfaceDef` (`Default`) | `generics`, `supers: List<TyId>`, `items` |
| `ImplDef` (`ImplDef::new(self_ty)`) | `generics`, `interface: Option<TyId>`, `self_ty: TyId`, `items` |
| `MixinUseDef` (`Default`) | `mixins: List<TyId>`, `rules: List<MixinRule>` |
| `MixinRule` | `method: Ident`, `from: Option<TyId>`, `action: MixinAction` |
| `MixinAction` | `Insteadof(List<TyId>)`, `Alias { name: Option<Ident>, vis: Option<Vis> }` |
| `Generics` (`Default`) | `params: List<GenericParam>`, `preds: List<WherePred>` |
| `GenericParam` (`GenericParam::new(binder)`) | `binder`, `bounds: List<Bound>`, `ty: Option<TyId>` (a const parameter), `default: Option<GenericArg>` |
| `WherePred` | `subject: Bound` (a type or a region), `bounds: List<Bound>` |
| `FieldDef` (`FieldDef::named(ident)`, `Default`) | `name: Option<Ident>`, `vis: Vis`, `ty: Option<TyId>`, `default: Option<ExprId>` (a constant context) |
| `Variant` | `name: Ident`, `shape: Shape`, `fields: List<FieldId>`, `discriminant: Option<ExprId>` (an integer constant context) |
| `Param` (`Param::new(pat)`) | `pat: PatId`, `ty: Option<TyId>`, `default: Option<ExprId>` (its own frame), `kind: ParamKind`, `by_ref: bool` (PHP `&$x`) |
| `Shape` | `Named` (default), `Tuple`, `Unit` |
| `Vis` | `Private` (default), `Protected`, `Package`, `Public` |
| `ParamKind` | `Receiver`, `PositionalOnly`, `Normal` (default), `Rest`, `NamedOnly`, `RestNamed` — in this order; `Receiver`/`Rest`/`RestNamed` at most once and without defaults; `Receiver` only first, only in interface/impl/class members |

Visibility is recorded, not enforced: each language's access rules are a
resolve-lang policy over `Vis` (spec §10).

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
| `MethodCall { receiver, method, generic_args: List<GenericArg>, args }` | method call (dispatched by type or runtime class) |
| `DynMethodCall { receiver, name, args }` | `$o->$m()`: method chosen by a run-time string |
| `Field { base, member, span }` | `base.name` / `base.0` |
| `DynField { base, name }` | `$o->$p`: member chosen by a run-time string |
| `Index { base, index }` | `base[index]` |
| `VarVar(ExprId)` | `$$name`: the local named at run time |
| `Append(ExprId)` | `$a[]`: the next slot; only as an assignment target or place argument |
| `Op { op, args }` | intrinsic op; `args.len()` = arity |
| `Cast { expr, ty, policy }` | `as`; policy has exactly `overflow` and `float_to_int` |
| `Intrinsic { kind, generic_args, args }` | atomics, volatile, named intrinsics ([below](#intrinsic)) |
| `Asm(Asm)` | structured inline assembly ([below](#asm)) |
| `Assign { target, op, value }` | `target` must be a place, evaluated once; compound `op` only `add sub mul div floor_div rem floor_mod and or xor shl shr pow`; **evaluates to the value written** (`a = b = 3`) |
| `LetPlace { binder, place, body }` | evaluate the operands of the place `place` **once** and name the place `binder` (kind `Place`) in `body`, for reads and writes: how `.=` with a host operator, `??=`, and `++`/`--` evaluate their place once; evaluates to `body` |
| `RefAssign { target, source }` | `$a = &$b`: both places |
| `Deref(ExprId)`, `Borrow { kind, expr }` | `*e`, `&e` / `&mut e` / raw |
| `Block(Block)` | statements + tail + optional label |
| `If { cond, then, else_ }` | |
| `Match { scrutinee, arms }` | first matching arm wins |
| `Loop { label, body, step }` | the one loop; `step` runs after each iteration and on `continue` (a `continue` of this loop inside its own step is invalid) |
| `Break { label, value }`, `Continue { label }`, `Return(Option)` | jumps; may leave `finally`, never `defer` |
| `Closure(Closure)` | lambda with explicit or implicit captures |
| `Throw`, `Try { body, catches, finally }`, `Await`, `Spawn` | effects |
| `Yield { key, value }` | yield `value` (null if absent) under `key` (the runtime's automatic key if absent; a key needs a value); evaluates to the value sent |
| `YieldFrom(ExprId)` | delegate to an inner generator or iterable, forwarding keys, sent values, and thrown errors; evaluates to its return value |
| `Err` | failed to lower |

**Places** are `Path`, `Field`, `DynField`, `Index`, `Deref`, `VarVar`,
`Append` (write positions only), and `Err`.

#### Expr records

`Block { stmts, tail, label, is_unsafe }` (`Default`),
`Arm { pat, guard, body }`,
`Arg { kind: ArgKind, value, place: bool }` (`Arg::positional`; `place`
passes the value as a place for a by-reference parameter, decided by the
callee, at run time for dynamic calls; the value must be a place and the kind
positional or named),
`ArgKind = Positional | Named(Ident) | Spread | SpreadNamed`,
`FieldInit { name: Ident, value }`, `MapEntry { key: Option<ExprId>, value }`,
`Member = Named(Symbol) | Index(u32)`,
`BorrowKind = Shared | Mut | RawConst | RawMut`,
`Closure { params, ret, body, effects, implicit: Option<CaptureMode>, captures, self_binder: Option<BinderId>, defaults: DefaultEval }`
(`Closure::new(body)`; `self_binder` names the closure inside its body for
recursion),
`DefaultEval = PerCall (default) | Once`,
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

A compound assignment with a host operator, `$a[f()] .= "x"`, evaluates
`$a` and `f()` once:

```rust
use hir_lang::{BinderKind, Builder, Expr, Name, Ns, Res};
use intern_lang::Interner;

let mut names = Interner::new();
let mut b = Builder::new();
let (pa, a) = b.local_param(Name::new(names.intern("a")));
let f = b.name_expr(Name::new(names.intern("f")));
let call_f = b.call(f, &[]);
let base = b.use_binder(a);
let place = b.expr(Expr::Index { base, index: call_f });
// let_place p = $a[f()] in p = mox.concat(p, "x")
let p = b.new_binder(Name::new(names.intern("p")), BinderKind::Place);
let (target, read) = (b.use_binder(p), b.use_binder(p));
let concat = b.resolved_path(Name::new(names.intern("concat")), Ns::Value, Res::Extern(names.intern("mox.concat")));
let concat = b.expr(Expr::Path(concat));
let x = b.str_lit("x");
let value = b.call(concat, &[read, x]);
let body = b.expr(Expr::Assign { target, op: None, value });
let once = b.expr(Expr::LetPlace { binder: p, place, body });
let fbody = b.block(&[], Some(once));
let func = b.func(Name::new(names.intern("append")), &[pa], fbody);
let root = b.module(None, &[func]);
assert!(b.finish(root).is_ok());
```

### `Stmt`

| Variant | Meaning |
|---|---|
| `Let { pat, ty, init, else_ }` | binders visible in later statements; `else_` requires `init` |
| `Expr(ExprId)` | evaluated for effect |
| `Item(ItemId)` | a local item (never sees the enclosing function's locals) |
| `Defer(ExprId)` | runs at block exit; no jump may leave it, `return` invalid inside |
| `Static { binder, ty, init }` | function-static local (PHP `static $n = 0;`) |
| `Global { binder, path }` | `global $x;`: a local aliasing the global `path` (`Value` namespace) |
| `Err` | failed to lower |

## Intrinsics and inline assembly

### `Intrinsic`

`#[non_exhaustive]`: `AtomicLoad(MemOrder)`, `AtomicStore(MemOrder)`,
`AtomicRmw(RmwOp, MemOrder)`, `AtomicCmpXchg { success, failure }`,
`Fence(MemOrder)`, `VolatileLoad`, `VolatileStore`, `Named(Symbol)` (any
arity). `arity()` (`None` for `Named`), `orderings_ok()` (C++20 rules: loads
not `Release`/`AcqRel`, stores not `Acquire`/`AcqRel`, fences not `Relaxed`,
cmpxchg failure not `Release`/`AcqRel` and not stronger than success).
`MemOrder = Relaxed | Acquire | Release | AcqRel | SeqCst`;
`RmwOp = Xchg | Add | Sub | And | Or | Xor | Nand | Min | Max`.

```rust
use hir_lang::{Intrinsic, MemOrder};

assert!(Intrinsic::AtomicLoad(MemOrder::Acquire).orderings_ok());
assert!(!Intrinsic::Fence(MemOrder::Relaxed).orderings_ok());
```

### `Asm`

`Asm { template: TextRef, operands: List<AsmOperand>, options: AsmOptions }`.
The template is UTF-8; `{N}` names operand `N` (`{{`/`}}` are literal braces).
`AsmOperand { dir: AsmDir, constraint: TextRef, expr }`,
`AsmDir = In | Out | InOut | Const | Sym` (outputs are places, `Sym` operands
are paths). `AsmOptions` is a bit set: `NONE PURE NOMEM READONLY NOSTACK
PRESERVES_FLAGS NORETURN`, with `union` and `contains`. The instructions
themselves are checked by the target's assembler, not here.

```rust
use hir_lang::AsmOptions;

let o = AsmOptions::NOMEM.union(AsmOptions::NOSTACK);
assert!(o.contains(AsmOptions::NOSTACK) && !o.contains(AsmOptions::PURE));
```

## Patterns

### `Pat`

| Variant | Meaning |
|---|---|
| `Wild` | `_` |
| `Bind { binder, mode, sub }` | bind (by value, `ref`, `ref mut`), optionally `x @ sub` |
| `Ident { binder, path }` | a bare identifier: matches the constant/unit variant `path` resolves to, else binds `binder` (resolve-lang decides) |
| `Lit(Lit)` | equality with a literal |
| `Range { lo: Option<PatId>, hi: Option<PatId>, inclusive }` | at least one bound; each a `Lit` (integer, char, or float, one class) or a constant `Path` pattern |
| `Tuple { elems, rest }`, `Ctor { path, elems, rest }` | `rest` = position of `..` (at most the element count) |
| `Record { path, fields, rest }` | field names unique; no path = anonymous record |
| `Path(PathId)` | unit variant, unit record, constant (`Pattern` namespace) |
| `Slice { prefix, rest: Option<SliceRest> }` | `SliceRest { bind, suffix }` |
| `Or(List<PatId>)` | non-empty; alternatives bind the same binders with the same modes |
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

## Types, bounds, generic arguments, effect sets

### `Ty`

`Infer`, `Prim(Prim)`, `Path(PathId)`, `Tuple(List<TyId>)`,
`Array { elem, len }` (`len` an integer constant context), `Slice(TyId)`,
`Ref { mutable, region: Option<PathId>, inner }`, `Ptr { mutable, inner }`,
`Fn { params, ret, effects, throws, abi: Option<Symbol> }`, `Nullable(TyId)`,
`Any` (gradual), `Object(List<Bound>)` (`dyn A + 'r`), `Impl(List<Bound>)`
(opaque `impl A`), `Union(List<TyId>)` (`int|string`), `Intersection(List<TyId>)`
(`A&B`), `Never`, `SelfTy`, `Err`. Every annotation slot in HIR is optional;
dynamic languages create no types at all. There is no `null` type: `T|null` is
`Nullable(T)`.

**Canonical form of unions and intersections** (spec §5.1): at least two
members; no union directly in a union (nor a `Nullable`: write
`Nullable(Union)`), no intersection directly in an intersection; members
strictly sorted by their key (the type's token stream: tags, primitive kinds,
path roots and segment names, list lengths; never resolutions), so no member
repeats. Members containing an error form or a constant expression, or larger
than 64 type/path nodes, have no key and may stand anywhere. `finish`
normalizes; the validator checks. Normalization rewrites only tree-shaped
parts: a union whose member list is out of range, or whose members have a
second parent, is left as it is, so the validator still reports the lowering
bug.

```rust
use hir_lang::{Builder, Name, Pat, Prim, Stmt, Ty};
use intern_lang::Interner;

// let _: str | i64 | i64;   becomes   i64 | str
let mut names = Interner::new();
let mut b = Builder::new();
let s = b.ty(Ty::Prim(Prim::Str));
let i = b.ty(Ty::Prim(Prim::I64));
let again = b.ty(Ty::Prim(Prim::I64));
let members = b.list(&[s, i, again]);
let union = b.ty(Ty::Union(members));
let pat = b.pat(Pat::Wild);
let decl = b.stmt(Stmt::Let { pat, ty: Some(union), init: None, else_: None });
let body = b.block(&[decl], None);
let f = b.func(Name::new(names.intern("f")), &[], body);
let root = b.module(None, &[f]);
let hir = b.finish(root)?;
let Ty::Union(sorted) = *hir.ty(union) else { unreachable!() };
assert_eq!(hir.list(sorted), [i, s]);
# Ok::<(), hir_lang::HirError>(())
```

### `Bound`

`Ty(TyId)` or `Region(PathId)`: one bound of a generic parameter, a
where-predicate, an associated type, a trait object, or an `impl` type.

### `GenericArg`

`Ty(TyId)`, `Const(ExprId)` (a constant context), `Region(PathId)`,
`Binding { name: Ident, ty }` (`Iterator<Item = u8>`),
`Constraint { name: Ident, bounds: List<Bound> }` (`Iterator<Item: Show>`).
Used in path segments, method calls, intrinsics, and generic defaults.

### Effects

```rust,ignore
pub struct Effects(/* private */);   // NONE, THROWS, ASYNC, YIELD, UNSAFE
pub const fn union(self, other: Effects) -> Effects
pub const fn contains(self, other: Effects) -> bool
pub const fn is_empty(self) -> bool
```

```rust
use hir_lang::Effects;

let async_gen = Effects::ASYNC.union(Effects::YIELD);
assert!(async_gen.contains(Effects::YIELD) && !async_gen.contains(Effects::THROWS));
```

## Names, binders, paths

### `Binder`

```rust,ignore
pub struct Binder { pub name: Name, pub kind: BinderKind, pub mutable: bool }
```

`Binder::new(name, kind)`, `with_mutable(bool)`.
`BinderKind = Local | Param | Capture | TypeParam | ConstParam | Region | Label | Place`,
with `is_value()` (`Local`, `Param`, `Capture`, `Place`), `is_type_level()`
(`TypeParam`, `ConstParam`, `Region`), `ns()` (the namespace a binder of that
kind is referenced in; `None` for labels), and `name()`.

### `Path`

```rust,ignore
pub struct Path { pub segments: List<Segment>, pub ns: Ns, pub root: PathRoot,
                  pub qself: Option<QSelf>, pub res: Res, pub unresolved: u32 }
pub struct Segment { pub name: Name, pub args: List<GenericArg>, pub origin: Origin }
```

`Path::new(segments, ns)` is relative, unqualified, and unresolved;
`Segment::new(name, origin)` has no generic arguments. The **error path** (no
segments, `Res::Err`) is valid; any other path needs a segment. Shape rules
for `unresolved`: at most the segment count; an empty resolved prefix only
after a type root or a qualified self, with `res = Unresolved`; with a
qualified self, the resolved prefix never extends past the trait.

### `PathRoot`

`Relative` (default), `Global` (`::a`), `SelfModule` (`self::`),
`Super(u8)` (`super::` n times, n ≥ 1; PHP/Python `parent` imports),
`SelfType` (`Self::`, PHP `self::`), `ParentType` (PHP `parent::`),
`StaticType` (PHP `static::`, late static binding); `is_type_root()` for the
last three.

### `QSelf`

`QSelf { ty: TyId, trait_len: u32 }`: `<T>::a` (`trait_len = 0`) or
`<T as a::Tr>::b` (the first `trait_len` segments are the trait). Requires a
relative root and at least one segment after the trait.

### `Ns`

`Value` (expression paths, capture sources, `global` declarations), `Type`
(type paths, record expression/pattern paths), `Pattern` (constructor,
constant, and identifier patterns), `Region`, `Import`. Fixed by the parent
node; `name()` spells it.

### `Res`

`Unresolved` (default), `Local(BinderId)`, `Def(DefId)`, `Prim(Prim)`,
`Extern(Symbol)` (a host/stdlib symbol), `Err`; `is_unresolved()`. Allowed
for a full resolution, per namespace:

| `ns` | `Local` kinds | this unit's `Def` items | `Def` variants | other units | `Prim` | `Extern` |
|---|---|---|---|---|---|---|
| `Value` | `Local` `Param` `Capture` `Place` `ConstParam` | `Fn` `Const` `Global` `Record` `Class` `Err` | yes | yes | no | yes |
| `Type` | `TypeParam` | `Record` `Sum` `Class` `Interface` `Alias` `AssocType` `Err` | yes | yes | yes | yes |
| `Pattern` | none | `Const` `Record` `Class` `Err` | yes | yes | no | yes |
| `Region` | `Region` | none | no | no | no | no |
| `Import` | none | all but `Impl`, `Import`, `MixinUse` | yes | yes | no | yes |

A resolved **prefix** (`unresolved > 0`) may name a type parameter, a record,
sum, class, interface, alias, associated type, module, or error item,
another unit's definition, a primitive, or an extern; never a variant, never
in `Region`.

### `UnitId`

`UnitId::new(u32)`, `as_u32()`, `Default` (unit 0). Host-assigned; unique
among units that refer to each other, and stable across rebuilds.

### `Def`

`Item(ItemId)` or `Variant(VariantId)`.

### `DefId`

```rust,ignore
pub struct DefId { /* unit, def, private tag */ }
pub const fn foreign(unit: UnitId, def: Def) -> DefId   // untagged
pub const fn unit(self) -> UnitId
pub const fn def(self) -> Def
```

Equality, ordering, and hashing compare `unit` and `def` only.

```rust
use hir_lang::{Def, DefId, ItemId, Res, UnitId};

// a reference into another unit: accepted as is, checked by the host
let other = DefId::foreign(UnitId::new(9), Def::Item(ItemId::from_index(3).unwrap()));
assert_eq!(other.unit(), UnitId::new(9));
let _res = Res::Def(other);
```

## Literals and primitive types

### `Prim`

`Bool`, `I8`…`I64`, `Isize`, `U8`…`U64`, `Usize`, `F32`, `F64`, `Char`, `Str`;
`is_int`, `is_signed`, `is_float`, `bits` (`isize`/`usize` count as 64), `name`.

### `Lit`

`Null`, `Bool(bool)`, `Int(IntLit)`, `Float(FloatLit)`, `Char(char)`,
`Str(TextRef)` (UTF-8), `Bytes(TextRef)`, `BigInt(TextRef)` (decimal digits,
optional `-`).

- `IntLit { value: u64, negative: bool, suffix: Option<Prim> }`: `IntLit::new`,
  `IntLit::signed(i64)`, `with_suffix`, `fits(Prim)`. A suffixed literal must
  fit; `-0` (`value = 0, negative`) is invalid.
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

Every operation of `specs/OPS.md` (v2) §3–§5: `Add Sub Mul Neg Abs Div FloorDiv
Rem FloorMod And Or Xor Not BitNot Shl Shr Pow Eq Ne Lt Le Gt Ge Min Max IeeeRem
Sqrt Fma Floor Ceil Trunc Round RoundEven TotalCmp`, and the conversions `IntCast(Prim)
FloatToInt(Prim) Zext(Prim) Sext(Prim) Narrow(Prim) IntToFloat(Prim)
Bitcast(Prim) BoolToInt(Prim) F32ToF64 F64ToF32 CharFromU32`. `Narrow` is OPS's
integer `trunc` conversion. Methods: `name()` (OPS spelling), `arity()`,
`target()`, `default_policy()`.

OPS's `not` is split: `Not` (`not`) is **logical** negation (bool; on dynamic
values the negation of truthiness, LSB `dlnot`), `BitNot` (`bit_not`) the
**bitwise** complement of an integer (LSB `dnot`). `Pow` (`pow`, OPS v2)
consults `overflow`: integer powers are exact or follow it, a negative
exponent is `NegativeExponent` unless the policy is `promote` (which gives the
`f64` power); float `pow` ignores the policy.

| Ops | Policy fields |
|---|---|
| `add sub mul neg abs pow int_cast` | `overflow` |
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
`Shift = Error | Mask | Saturate` (`Saturate`: PHP shifts, OPS v2: an amount at
or above the width gives `0`, or `-1` for `shr` of a negative value; a negative
amount is still an error), `FloatToInt = Error | Saturate`.

`Overflow::Promote` (OPS §2, Mox/PHP): when the exact integer result is not
representable, the result is the `f64` nearest the exact mathematical result;
`div` with `promote` yields the exact quotient as `f64` whenever the division
is inexact or overflows. It needs a dynamically typed result: the validator
rejects it (`PromoteOnStaticResult`) on `int_cast<T>`, on a cast to anything
but `Any`, and directly inside an integer constant context.

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
`Segment`, `GenericArg`, `Bound`, `FieldPat`, `Attr`, `AttrArg`, `AsmOperand`,
`MixinRule`.

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

### `Event`

`#[non_exhaustive]`: `Enter(NodeRef)`, `Leave(NodeRef)`, `ScopeOpen`,
`ScopeClose`, `Bind(BinderId)` (the binder becomes visible here and stays
visible until the `ScopeClose` matching the innermost `ScopeOpen` around it),
`FrameOpen(Frame)`, `FrameClose`; `node()` returns the node of an
`Enter`/`Leave`.

### `Frame`

`#[non_exhaustive]`: `Item(ItemId)`, `Closure(ExprId)`, `Default(ParamId)`,
`Const` (array length, const argument, discriminant, field default).

### `Control`

`Continue | Skip | Stop`. `Skip` skips the children of the entered node (its
`Leave` still arrives, and the scope and frame events of the skipped subtree
are not delivered).

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
appended at the node's end. A path prints inline when it directly follows its
parent's header, else on its own line; roots print as `::`, `self::`,
`super::`, `Self::`, `parent::`, `static::`, qualified selves as `<…>::`.
Binders print as `name%id`, marks as `'eN`, resolutions as `→ local x%3`,
`→ item 4`, `→ variant 2`, `→ item u7:3` (another unit), `→ prim i32`,
`→ extern name`, with ` +N` for unresolved segments. Non-node children
appear as groups (`(arm …)`, `(guard …)`, `(arg name …)`, `(arg place …)`,
`(capture x%2 by-value …)`, `(binding …)`, `(operand …)`, `(rule …)`,
`(key …)` for a yield's key). 0.4 headers: `union`, `intersection`,
`let-place p%N`, `yield-from`, `op bit_not`, `op pow`, `shift=saturate`.
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

`#[non_exhaustive]`. Each variant names its site; `node()` returns the node
it is about, when there is one.

| Variant | Meaning | What to do |
|---|---|---|
| `CapacityExceeded { what: Capacity }` | an arena/pool passed `u32::MAX - 1` entries, or the total node count did | split the input |
| `Dangling { site, kind, index }` | an id names nothing | a lowering bug, or ids mixed between builders |
| `ListOutOfBounds { site }`, `TextOutOfBounds { site }` | a list/text range lies outside its pool | same |
| `ExpansionOrder { expn }` | an expansion refers to itself or a later one | record parents first |
| `RootNotModule` | the root is not a module | |
| `SharedNode { node }` | a node has two parents (or is its own ancestor) | build a fresh node per use, or `copy_subtree` |
| `Unreachable { node }` | a node is not in the tree; for an orphaned subtree, its top (for an orphaned cycle, its first node) | attach or drop it |
| `Malformed { site, problem: Malformed }` | a node's own shape is wrong | see below |
| `DuplicateName { node, name, index }` | two members of one list share a name; `index` is the first repetition in list order | |
| `Policy { expr }` | an op/cast/compound assignment lacks or has extra policy fields | use `Op::new` defaults |
| `Arity { expr, expected, found }` | wrong operand count | |
| `BinderBoundTwice { binder, node }` | bound by two constructs | |
| `DuplicateBinding { binder, pat }` | bound twice in one pattern alternative | |
| `OrPatternBinders { pat }` | alternatives bind different sets | |
| `BinderKind { binder, expected }` | kind does not fit the site | |
| `PathNamespace { path, expected }` | namespace does not match the parent | |
| `Resolution { path, res }` | the namespace (or a prefix) cannot name `res` | |
| `ForeignDef { path }` | a `DefId` claims this unit but was minted by another `Hir` | mint ids with this `Hir`'s `def` |
| `OutOfScope { path, binder }` | binder not in scope at the path | |
| `NotCapturable { path, binder }` | binder behind a nested item, a constant context, a non-capturing closure, or a once-evaluated default | |
| `Jump { expr, problem: JumpProblem }` | `BreakOutsideLoop`, `ContinueOutsideLoop`, `LabelNotInScope`, `ContinueToBlock`, `OutOfDefer`, `ContinueInStep` | |
| `Effect { expr, problem: EffectProblem }` | `ReturnOutsideFunction`, `AwaitOutsideAsync`, `YieldOutsideGenerator`, `ThrowNotAllowed` | declare the effect or wrap in `try` |

### `Malformed`

`#[non_exhaustive]`: `EmptyPath`, `EmptyOrPattern`, `RestOutOfRange`,
`RangeWithoutBounds`, `RangeBoundKinds`, `LiteralOutOfRange`, `LiteralSuffix`,
`InexactF32`, `InvalidUtf8`, `InvalidBigInt`, `ConversionTarget`,
`AssignTarget`, `CompoundAssignOp`, `LetElseWithoutInit`, `ShapeFields`,
`MissingName`, `UnexpectedName`, `MissingBody`, `MissingConstValue`,
`ItemPlacement`, `ReceiverPlacement`, `ParamOrder`, `ParamDefault`,
`InferCapture`, `EmptyAttrArg`, `AttrOrder`, `PromoteOnStaticResult`,
`PathShape`, `RangeBound`, `NegativeZero`, `AsmTemplate`, `AsmOperand`,
`TypeArity`, `TypeNesting`, `TypeOrder`, `YieldKey`,
`IntrinsicArity`, `MemOrder`, `PlaceArg`, `AppendContext`, `OrPatternModes`.

### `Site`, `Capacity`, `JumpProblem`, `EffectProblem`

`Site = Node(NodeRef) | Binder(BinderId) | Expansion(ExpnId) | Root | Attrs(NodeRef)`;
`Capacity = Arena(IdKind) | Pool | Text | Total` (`#[non_exhaustive]`);
`JumpProblem` and `EffectProblem` as listed above (`#[non_exhaustive]`). All
error types implement `Display`; `HirError` implements `core::error::Error`.

```rust
use hir_lang::{Builder, Expr, HirError, List, Malformed, Ns, Path};

let mut b = Builder::new();
let empty = b.path(Path::new(List::EMPTY, Ns::Value)); // no segments, not the error path
let e = b.expr(Expr::Path(empty));
let body = b.block(&[], Some(e));
let f = b.func(hir_lang::Name::new(intern_lang::Interner::new().intern("f")), &[], body);
let root = b.module(None, &[f]);
let err = b.finish(root).unwrap_err();
assert!(matches!(err, HirError::Malformed { problem: Malformed::EmptyPath, .. }));
assert_eq!(err.to_string(), "path 0: a path has no segments");
```

### Runtime errors defined by HIR

Not Rust types: the run-time error kinds HIR's own forms raise, for bcgen-lang
and the T0 evaluator (spec §8.10). HIR owns codes E0200–E0299 (OPS owns
E0001–E0099, LSB E0100–E0199).

| Code | Kind | Raised by |
|---|---|---|
| E0200 | `NoMatch` | a `match` none of whose arms matches (Mox maps it to `UnhandledMatchError`) |

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
| Lenient repair rounds | 2 (spec §13.2; checked by a debug assertion); a hard limit proportional to the arena sizes turns a broken argument into an error instead of a hang | — |
| Nodes in a union member's key | 64 (larger members are unkeyed, exempt from ordering) | — |
| Nesting depth | none (every algorithm is iterative) | — |

## Stability

Pre-1.0; 0.4 broke 0.3 and 0.3 broke 0.2 (see the CHANGELOG for the migration
lists). The node
forms are deliberately **not** `#[non_exhaustive]`: every consumer must handle
every form, and a new form is a breaking change for all of them regardless.
`Intrinsic`, `Event`, `Frame`, the error enums, and `PrintOptions` are
`#[non_exhaustive]`. The canonical order and the printed form are part of the
contract (snapshot tests depend on them). The 0.5 body graph, textual syntax,
and binary encoding (spec §19–§21) are additive.
