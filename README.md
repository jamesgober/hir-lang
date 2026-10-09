<h1 align="center">
    <img width="99" alt="Rust logo" src="https://raw.githubusercontent.com/jamesgober/rust-collection/72baabd71f00e14aa9184efcb16fa3deddda3a0a/assets/rust-logo.svg">
    <br>
    <b>hir-lang</b>
    <br>
    <sub><sup>LEXERSKETCH HIGH-LEVEL IR</sup></sub>
</h1>

<div align="center">
    <a href="https://crates.io/crates/hir-lang"><img alt="Crates.io" src="https://img.shields.io/crates/v/hir-lang"></a>
    <a href="https://crates.io/crates/hir-lang"><img alt="Downloads" src="https://img.shields.io/crates/d/hir-lang?color=%230099ff"></a>
    <a href="https://docs.rs/hir-lang"><img alt="docs.rs" src="https://img.shields.io/docsrs/hir-lang"></a>
    <a href="https://github.com/jamesgober/hir-lang/actions"><img alt="CI" src="https://github.com/jamesgober/hir-lang/actions/workflows/ci.yml/badge.svg"></a>
    <a href="https://github.com/rust-lang/rfcs/blob/master/text/2495-min-rust-version.md"><img alt="MSRV" src="https://img.shields.io/badge/MSRV-1.85%2B-blue"></a>
</div>

<br>

<div align="left">
    <p>
        <strong>hir-lang</strong> is the high-level intermediate representation of the LexerSketch toolchain: the one structured, expression-oriented IR that every forged language lowers into, so that name resolution, type checking, pattern compilation, capability passes, the evaluator, and the lowering to SSA are written once and serve every language &mdash; dynamic (Mox), gradual (Mercury), static with generics and ownership (Iron, Kraken), low-level (Zero), query (HQL), and data (NOML).
    </p>
    <p>
        Binders carry unique ids and hygiene marks, so a lowering template's temporaries can never capture a user's variables. Types are optional, so dynamic languages simply leave them out. Patterns, effects (<code>throw</code>, <code>try</code>, <code>await</code>, <code>yield</code>, <code>spawn</code>, <code>defer</code>), and intrinsic operations with their overflow and division policies are explicit. Every node records its source span and the expansion that produced it. Paths resolve partially, the way rustc does (<code>Vec::new</code>, <code>&lt;T as Add&gt;::Output</code>), and across compilation units. A validator stands at the door: a <code>Hir</code> exists only if it is well-formed, and a well-formed <code>Hir</code> cannot make a consumer panic &mdash; while lenient validation turns each user mistake into a reported error node instead of rejecting the whole unit.
    </p>
    <br>
    <hr>
    <p>
        <strong>MSRV is 1.85+</strong> (Rust 2024 edition). <code>no_std</code>-compatible (needs only <code>alloc</code>), <code>#![forbid(unsafe_code)]</code>, two dependencies from the family: <a href="https://crates.io/crates/span-lang"><code>span-lang</code></a> 0.4 and <a href="https://crates.io/crates/intern-lang"><code>intern-lang</code></a> 1.
    </p>
    <blockquote>
        <strong>Status: 0.3.0.</strong> A breaking revision of 0.2.0 that closes the gaps an adversarial review found (units, partial resolution, lenient validation, PHP/Mox and low-level forms, resolver support). The data model, builder, strict and lenient validator, traversal, and debug printer are implemented. The body-graph view for flow-sensitive analysis, the round-trippable textual syntax, and the binary encoding are specified and land in 0.5; they are not in this release. The API is frozen at 1.0 only after lower-lang, resolve-lang, and typeck-lang have used it end to end. See <a href="./CHANGELOG.md"><code>CHANGELOG.md</code></a> and the <a href="./dev/ROADMAP.md"><code>ROADMAP</code></a>.
    </blockquote>
</div>

<hr>
<br>

## The model

- A **[`Builder`](./docs/API.md#builder)** creates nodes bottom-up &mdash; items, expressions, statements, patterns, types, paths, fields, variants, parameters &mdash; for one compilation unit, and stamps each with its current **origin** (span plus expansion). [`finish`](./docs/API.md#builderfinish--builderfinish_lenient) validates strictly; `finish_lenient` repairs every problem to an error node and reports them all in source order. [`copy_subtree`](./docs/API.md#buildercopy_subtree) duplicates a subtree with fresh binders.
- A **[`Hir`](./docs/API.md#hir)** is the validated result: dense arenas of `Copy` nodes linked by 4-byte typed ids, with accessors, O(1)-checked [`resolve` / `resolve_partial`](./docs/API.md#hirresolve--hirresolve_partial) for name resolution in place (including other units' definitions), [`lookup_local`](./docs/API.md#hirlookup_local) for the scope rule, [`implicit_captures`](./docs/API.md#hirimplicit_captures--hirall_implicit_captures) for closure conversion, and an iterative [walker](./docs/API.md#hirwalk_from--hirchildren_into) that also reports scopes, binders, and frames.
- **[`walk`](./docs/API.md#walk)** and **[`print`](./docs/API.md#print)** are the one-call entry points: visit every node, or render the deterministic debug form.
- A **[`HirError`](./docs/API.md#hirerror)** names exactly what is wrong and where: a dangling id, a shared node, a binder used out of scope, a `break` without a loop, an `await` outside an async function, an op without its policy.

<br>

What it guarantees, and how each guarantee is checked:

| Guarantee | How it is held |
|---|---|
| The validator is total: any arena is accepted or rejected with a precise error, never a panic. | A property test builds 2,048 arbitrary arenas per run from random nodes with random ids (50,000 in a one-off run); every accepted one is then walked, printed, and re-validated. |
| Scoping, frames, captures, and jumps follow the spec, including which error is reported first. | A reference implementation (a recursive environment walk) runs alongside every generated program; the validator's single-pass interval algorithm must agree on the result and on the exact first error. 1,024 programs per run, 20,000 in a one-off run, no disagreement. |
| Builder output with in-scope references always validates; each class of corruption is rejected with its own error. | Random programs whose references are drawn from the reference's legal set always validate; targeted corruptions (shared node, orphan, dangling id, missing policy, wrong arity, wrong binder kind, stray `break`, use before `let`) each produce their error, and an unbound binder (valid since 0.3) is accepted. |
| `resolve` keeps a `Hir` valid. | Random sequences of resolutions: accepted ones take effect, rejected ones change nothing, and the result re-validates. |
| Lenient validation always yields a valid `Hir`, and agrees with strict validation. | On 2,048 arbitrary arenas per run, `finish_lenient` either fails for a reason with no repair (missing or non-module root) or returns a `Hir` that re-validates, deterministically; on 1,024 generated programs per run it reports no problems exactly when strict `finish` succeeds, and strict's error is always among its problems. |
| The spec's examples are real. | Each lowering example in `specs/HIR.md` §9 is built, validated, and printed by a test that checks the printed form appears verbatim in the spec. |
| Every node carries an origin; construction is deterministic. | Property tests check every node's and binder's origin, and that equal input gives equal `Hir`s and byte-identical printed and debug forms; a golden snapshot covers every node form. |
| Arbitrarily deep HIR builds, validates, walks, prints, clones, compares, and drops without recursion. | Tests on ~1M-node chains (nested ops, blocks, loops, closures, patterns, types, a 250,000-binder `let` chain) on the default 2 MiB test stack. |

<hr>
<br>

## Installation

```toml
[dependencies]
hir-lang = "0.3"
intern-lang = "1"
```

Without the standard library:

```toml
[dependencies]
hir-lang = { version = "0.3", default-features = false }
```

<hr>
<br>

## Quick start

Build, validate, walk, print:

```rust
use hir_lang::{Builder, Name, OpKind, Span};
use intern_lang::Interner;

let mut names = Interner::new();
let mut b = Builder::new();

// fn area(w, h) { w * h }
b.set_span(Span::new(8, 9));
let (pw, w) = b.local_param(Name::new(names.intern("w")));
let (ph, h) = b.local_param(Name::new(names.intern("h")));
let (uw, uh) = (b.use_binder(w), b.use_binder(h));
let product = b.op(OpKind::Mul, &[uw, uh]);
let body = b.block(&[], Some(product));
let area = b.func(Name::new(names.intern("area")), &[pw, ph], body);
let root = b.module(None, &[area]);

let hir = b.finish(root)?;

let mut nodes = 0;
hir_lang::walk(&hir, |_| nodes += 1);
assert_eq!(nodes, 12);
assert!(hir_lang::print(&hir, &names).contains("(op mul overflow=error"));
# Ok::<(), hir_lang::HirError>(())
```

### Resolving names in place

Lowering leaves user names unresolved; a resolver fills them through a checked
setter, and the `Hir` stays valid:

```rust
use hir_lang::{BinderKind, Builder, Expr, HirError, Name, Res};
use intern_lang::Interner;

let mut names = Interner::new();
let mut b = Builder::new();
let x = b.new_binder(Name::new(names.intern("x")), BinderKind::Local);
let px = b.bind(x);
let one = b.int(1);
let decl = b.let_stmt(px, Some(one));
let use_x = b.name_expr(Name::new(names.intern("x")));
let body = b.block(&[decl], Some(use_x));
let f = b.func(Name::new(names.intern("f")), &[], body);
let root = b.module(None, &[f]);
let mut hir = b.finish(root)?;

let Expr::Path(path) = *hir.expr(use_x) else { unreachable!() };
assert!(hir.can_reference(path, x));
hir.resolve(path, Res::Local(x))?;
assert_eq!(hir.path(path).res, Res::Local(x));
# Ok::<(), HirError>(())
```

### Mistakes are values

Strict validation returns the first problem; lenient validation repairs every
problem to an error node and returns a valid `Hir` alongside the list:

```rust
use hir_lang::{Builder, Expr, HirError, JumpProblem};

let build = || {
    let mut b = Builder::new();
    let stray = b.expr(Expr::Break { label: None, value: None });
    let body = b.block(&[], Some(stray));
    let f = b.func(hir_lang::Name::new(intern_lang::Interner::new().intern("f")), &[], body);
    let root = b.module(None, &[f]);
    (b, root, stray)
};

let (b, root, stray) = build();
let problem = HirError::Jump { expr: stray, problem: JumpProblem::BreakOutsideLoop };
assert_eq!(b.finish(root), Err(problem));

let (b, root, stray) = build();
let (hir, problems) = b.finish_lenient(root)?;
assert_eq!(problems, [problem]);
assert_eq!(hir.expr(stray), &Expr::Err);
# Ok::<(), HirError>(())
```

<hr>
<br>

## Examples

Runnable programs in [`examples/`](./examples):

| Example | What it shows |
|---|---|
| [`quickstart`](./examples/quickstart.rs) | The Tier-1 path: build a function with origins, finish, walk, print. |
| [`hygiene`](./examples/hygiene.rs) | A Mox-style `foreach` lowered through a desugaring template whose temporary `it` coexists with the user's `it`; a toy resolver resolves names by symbol and mark with `can_reference` and `resolve`. |

<hr>
<br>

## Performance

Storage is flat: nodes are `Copy` values in per-kind arenas (an expression is 40 bytes, a pattern 32, a path 40), children are 4-byte ids, and lists live out of line in pools, so building is a push per node with nothing allocated per node. The validator is one arena scan plus one explicit-stack walk that computes every binder's scope as a preorder interval; after that, `resolve` checks scope and frames with a few comparisons.

Measured with the benchmarks in [`benches/`](./benches) (library only), Windows 11, x86_64, Rust stable, release profile. Linux numbers are recorded by the release gate.

| Benchmark | What it measures | Windows |
|---|---|---:|
| `1m/build` | Build a module of 7,500 functions (825,001 nodes) | ~32 ms |
| `1m/validate` | `finish` on that module | ~27 ms (31 M nodes/s) |
| `1m/walk` | Visit every node (`walk`) | ~6.7 ms (123 M nodes/s) |
| `1m/print` | Render the debug form | ~51 ms |
| `1m/clone_eq_drop` | Clone, compare, and drop the whole `Hir` | ~15 ms |
| `1m/resolve_every_path` | `resolve` all 180,000 paths, each scope-checked | ~1.4 ms (~8 ns each) |
| `deep_1m_chain/validate` | `finish` on a 1,000,000-deep expression chain | ~44 ms |
| `deep_1m_chain/walk` | Walk it with `walk_from` (all events) | ~26 ms |

The 82,501-node workload scales proportionally (validate ~2.1 ms, walk ~0.6 ms). Against 0.2.0: `walk` is ~30% faster (a node-only path); `walk_from`, which now also reports scopes, binders, and frames, costs about twice 0.2's enter/leave walk; `clone_eq_drop` is ~20% and `resolve` ~25% slower (larger paths, more checks); deep-chain validation ~25% slower.

```bash
cargo bench --bench bench
```

<hr>
<br>

## Design notes

- **One loop.** `while`, C `for`, `do-while`, and `for-each` lower to one `Loop` with an optional `step` that runs after each iteration and on `continue`; that makes the C-style desugarings exact without rewriting the body. `for-each` goes through each language's iteration protocol in its lowering template. Every analysis handles one loop form instead of four.
- **Resolution in place, checked, partial.** Lowering runs before resolution, so paths carry a resolution slot covering a prefix of their segments (the rest are resolved by type, as in rustc). Template temporaries are resolved by the lowering itself (hygiene by construction); resolve-lang fills the rest with `resolve`/`resolve_partial`, whose O(1) check uses the scope intervals the validator computed, and finds candidates with `lookup_local` and the walk's scope events instead of re-deriving the rules.
- **Units, and ids that cannot be mixed up.** Definitions are named by `DefId { unit, def }`, so references cross units; ids minted by one `Hir` are tagged and rejected by another `Hir` of the same unit.
- **Broken input is normal input.** `finish_lenient` turns each offending node into its error form and reports every problem once, in source order; cascades from a repair are suppressed.
- **Explicit captures, derived implicit ones.** Syntax that names captures (PHP `use`, C++ capture lists) records them with fresh inner binders; closures that capture implicitly declare a default mode, and the capture set is derived by `implicit_captures` rather than stored, so it can never go stale.
- **Ops carry exactly their policy.** An `add` without an overflow policy, or a comparison with one, is rejected, so no execution tier ever guesses.
- **Exhaustive node enums.** Node forms are not `#[non_exhaustive]`: every consumer must handle every form, and a new form is a breaking change for all of them anyway.
- **No recursion anywhere.** Validation, walking, printing, capture analysis, cloning, comparison, and drop are iterative or flat. The printer caps indentation, so its output is linear at any depth.

<hr>
<br>

## Testing

The suite runs on Windows, Linux, and macOS through the CI matrix, on stable and the 1.85 MSRV:

```bash
cargo test                       # unit + integration + property + doctests
cargo test --no-default-features # no_std + alloc
cargo clippy --all-targets --all-features -- -D warnings
cargo bench --bench bench
```

[`tests/errors.rs`](./tests/errors.rs) produces every error variant from the smallest HIR that has it; [`tests/resolve.rs`](./tests/resolve.rs) covers scoping, frames, captures, hygiene, and `resolve`; [`tests/features.rs`](./tests/features.rs) covers the 0.3 forms and APIs (units, partial resolution, lenient mode, `lookup_local`, walk events, subtree copy); [`tests/spec_examples.rs`](./tests/spec_examples.rs) checks the spec's §9 examples verbatim; [`tests/forms.rs`](./tests/forms.rs) builds one HIR using every node form against a golden snapshot; [`tests/deep.rs`](./tests/deep.rs) runs the million-node depth tests; [`tests/properties.rs`](./tests/properties.rs) holds the property tests and the reference implementation. Every `rust` example in this README and in [`docs/API.md`](./docs/API.md) is compiled and run as a doctest.

<hr>
<br>

## Cross-platform support

- Linux (x86_64, aarch64)
- macOS (x86_64, Apple Silicon)
- Windows (x86_64)

The crate uses no operating-system facilities and no platform-specific code; the same HIR validates the same way and prints the same bytes everywhere.

<hr>
<br>

## Contributing

See [`REPS.md`](./REPS.md) for the engineering standards every change is held to, [`dev/DIRECTIVES.md`](./dev/DIRECTIVES.md) for the invariants and the definition of done, and [`dev/ROADMAP.md`](./dev/ROADMAP.md) for what comes next. Before a PR: `cargo fmt --all`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test --all-features` must be clean.

<br>

<div id="license">
    <h2>License</h2>
    <p>Licensed under either of</p>
    <ul>
        <li><b>Apache License, Version 2.0</b> &mdash; <a href="./LICENSE-APACHE">LICENSE-APACHE</a></li>
        <li><b>MIT License</b> &mdash; <a href="./LICENSE-MIT">LICENSE-MIT</a></li>
    </ul>
    <p>at your option.</p>
</div>

<div align="center">
  <h2></h2>
  <sup>COPYRIGHT <small>&copy;</small> 2026 <strong>James Gober <me@jamesgober.com>.</strong></sup>
</div>
