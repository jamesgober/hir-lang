# hir-lang - Roadmap

> Path from scaffold to a stable 1.0. Hard parts are front-loaded; each phase has hard exit criteria.
> Master plan: ../_lexersketch/ROADMAP.md and ../_lexersketch/NEW-LIBS.md
>
> **Anti-deferral rule:** no listed hard task moves to a later phase unless this file records the move and the reason.

## v0.1.0 - Scaffold (DONE)
Compiles, CI green, structure correct, no domain logic.
- [x] Manifest, README, CHANGELOG, REPS, dual license, CI, deny, clippy, rustfmt, DIRECTIVES, ROADMAP.

## v0.2.0 - Foundation (DONE)
- [x] Core forms: items (fn, record, sum, interface, alias, const, module), expressions, statements, patterns, type terms (optional), effects.
- [x] Dense arena storage with typed ids; builder with origin capture; validator with precise errors.
- [x] Wires span-lang and intern-lang. Property tests for builder/validator invariants.

Delivered:
- The spec, `../_lexersketch/specs/HIR.md`: every node form, storage, origins and hygiene,
  binders, scopes and frames, resolution, types-optional mode, patterns, effects (with the
  mapping onto LSB suspension instructions), intrinsic ops and policies (OPS section 2,
  `promote` included), items, attributes, error nodes, the validation contract, canonical
  order, and the debug printer. It also designs the 0.5 work so the 0.2 data model already
  carries it: the body-graph view (D12 resolved as option (a), with an Iron ownership example),
  the textual syntax as an `.lsf`, and the binary encoding.
- Node forms: items `Fn` (generics, default/named/variadic/receiver parameters, effects,
  throws, abi), `Record`, `Sum`, `Class`, `Interface`, `Impl`, `Alias`, `AssocType`, `Const`,
  `Global`, `Module`, `Import`, `Err`; 30 expression forms including one `Loop` with a `step`
  (decision: `while`/`for`/`do-while`/`for-each` are lowered; spec 8.7), closures with explicit
  captures and an implicit-capture mode, `throw`/`try`/`await`/`yield`/`spawn`; statements with
  let-else and `defer`; 13 pattern forms (guard on the arm, `TypeTest` for catch clauses); 16
  optional type forms; attributes on any node.
- Storage: dense per-kind arenas of `Copy` nodes, 4-byte `NonZeroU32` ids, out-of-line list
  pools, a text pool, parallel origin arrays, a sorted attribute table. Clone, equality,
  debug, and drop are flat.
- `Builder` (bottom-up, current-origin capture, sticky non-panicking overflow) and
  `Builder::finish`.
- The validator: total and linear (one arena scan, one explicit-stack walk, deferred scope
  checks); 23 `HirError` variants and 27 `Malformed` shape problems.
- `Hir`: total accessors (foreign ids read as error nodes), `resolve` (O(1) checks from the
  validator's scope intervals and frame floors), `can_reference`, `implicit_captures`,
  `validate`, `walk_from`, `children_into`, `attrs`, `count`, `variant_owner`.
- Tier-1 `walk` and `print` (`print_with`, `print_into`): deterministic, linear output at any
  depth.
- Tests: 47 error-path, 12 scoping/resolution, 4 every-form (golden snapshot), 8 depth
  (~1M nodes), 9 property tests including a reference implementation of scoping, frames, and
  jumps (differential, exact first error), totality over arbitrary arenas, targeted mutations,
  `resolve` preserving validity, origins, determinism; 14 unit tests; doctests for every public
  item, README, and docs/API.md. Benchmarks at 82k, 825k, and 1M nodes. Examples `quickstart`
  and `hygiene`.

Moved forward from v0.5.0 (delivered here): benchmarks at 100k-1M nodes.

Dependency wiring (decided here, recorded per the anti-deferral rule):
- **span-lang 0.4: wired.** `Origin.span` is a `span_lang::Span`, re-exported. 0.4 is the
  version syntax-lang and diag-lang use, so spans flow from the CST to diagnostics unchanged.
- **intern-lang 1: wired.** Every name is a `Symbol` (4 bytes, integer equality) from the
  session's interner, shared with macro-lang so hygiene marks and names line up; the printer
  takes any `intern_lang::Lookup`. A `Hir` does not own an interner (one interner serves a
  whole session, so names compare across files). Literal text is kept out of the interner
  (the HIR's own text pool), so data never grows the symbol table.
- **type-lang: not wired.** NEW-LIBS lists it for type terms, but HIR's type terms are surface
  annotations (optional, unresolved, with paths and constant expressions); type-lang's terms
  are the unifier's inference variables. typeck-lang maps one onto the other. Wiring it here
  would force every dynamic language to carry a unifier dependency for nothing.
- **diag-lang: not wired.** HIR reports structural errors as `HirError` values naming nodes;
  every node has an origin, so the consumer that turns an error into a user diagnostic (only
  lowering bugs reach the validator) uses diag-lang itself. Converting here would add a
  dependency for a path user code never takes.
- **serde: not added.** The versioned binary encoding is ours (spec section 21, v0.5).

Not in this milestone (scheduled by this roadmap, not deferred): the textual form and its
round-trip property, the body-graph view, and the binary encoding with its round-trip and
byte-identical determinism properties (v0.5.0); fuzz targets for the decoder and textual
parser (v0.9.0). The DIRECTIVES section 4 determinism invariant is tested in 0.2 on equality,
the printed form, and the debug form, since there is no encoding yet.

## v0.3.0 - Review fixes (DONE, breaking)
An adversarial review of 0.2.0 found design gaps; they are closed here, while breaking is cheap
(0.2.0 was published, so this is a breaking 0.x minor). Spec `specs/HIR.md` revised to v2.

Status of every finding (fixed / fixed differently / deferred):

| ID | Finding | Status |
|---|---|---|
| B1 | No units, cross-unit refs, forgeable local ids | **Fixed differently.** `UnitId` + `DefId { unit, def: Def::Item \| Def::Variant }` behind `Res::Def`, plus `Res::Extern`. Unforgeability uses a per-builder tag carried by minted ids (not a nonce in `ItemId`, which would grow every id): a tagged `DefId` for this unit from another `Hir` is rejected (`ForeignDef`). Untagged ids (`DefId::foreign`, needed by decoders) for this unit are only range/kind-checked; tags are a process counter, off on targets without 32-bit atomics. |
| B2 | No partial resolution | **Fixed.** `Path { root, qself, res, unresolved }` (rustc model), `resolve_partial`, shape rules, prefix namespace rules. |
| B3 | One user error rejects the unit | **Fixed.** `finish_lenient`: every problem repaired to an error form, cascades suppressed, problems in source order, valid `Hir` returned; strict `finish` kept. Problems are `HirError` values (diag-lang still not wired, see 0.2). |
| H1 | Spec §9.3/§9.5 examples invalid | **Fixed differently.** No parser exists before 0.5, so `tests/spec_examples.rs` builds each §9 example, validates it, and checks its printed form appears verbatim in the spec. |
| H2 | Mox references | **Fixed, one part differently.** `Param.by_ref`, `RefAssign`, `Stmt::Static`, `Stmt::Global`; `ArgKind::Place` became `Arg.place: bool` so a named argument can also be a place. |
| H3 | finally/generator semantics | **Fixed.** Jumps out of `finally` allowed (ban kept for `defer`); throw-into and close specified (yield has an unwind edge); async generators specified; LSB mapping updated; `yield` in cleanup rejected. |
| H4 | Effects claimed bounded; no unwind edges | **Fixed in the spec; implemented with the body graph in 0.5.** Claim corrected; §19 gives every call, fallible op, index, conversion, throw and suspension an unwind edge. |
| H5 | Resolver must re-derive scopes | **Fixed.** `Event` is `#[non_exhaustive]` with `ScopeOpen/ScopeClose/Bind/FrameOpen/FrameClose`; `Hir::lookup_local`. |
| H6 | Classes/traits/visibility/path roots | **Fixed.** `ClassDef.bases`, `mixin` classes + `MixinUse` (insteadof/as; expansion is resolve-lang's), `Vis::Protected`, `PathRoot` (super/self/Self/parent/static). |
| H7 | Bare identifier patterns | **Fixed.** `Pat::Ident { binder, path }`. |
| H8 | No top-level code | **Fixed.** `Module { items, body, effects }`. |
| M1 | Accessor inconsistency | **Fixed.** `get_*` accessors; `param` total like `path`/`field`; debug-assert on foreign ids. `variant`/`binder`/`expansion` stay `Option`-only (no error form to return). |
| M2 | Body graph for NLL | **Designed (spec §19.2); implementation scheduled for 0.5** as already planned (coordinator decision). |
| M3 | Unions, ABI, atomics, asm, escape hatch | **Fixed.** `RecordDef.is_union`, `Ty::Fn.abi`, `Intrinsic` (atomics/fence/volatile with C++20 ordering checks, `Named`), structured `Expr::Asm`. |
| M4 | Generic-arg gaps | **Fixed.** `GenericArg::{Binding, Constraint, Region, Const}`, `Bound::Region`, `Ty::Impl`, constant paths as range bounds. |
| M5 | Default frames, visibility policy | **Fixed.** `Frame::Default` with `DefaultEval::{PerCall, Once}`; visibility recorded, enforcement is a resolve-lang policy (spec §10). |
| M6 | Dynamic place forms | **Fixed.** `DynField`, `DynMethodCall`, `VarVar`, `Append`. |
| M7 | Recursive closures | **Fixed.** `Closure.self_binder`. |
| M8 | Encoding symbols; duplicate order | **Fixed** (duplicates reported at first repetition in source order); the symbol table is specified in §21 and implemented with the encoding in 0.5. |
| M9 | `continue` in own step; yield/await in dropped generator's finally | **Fixed.** `ContinueInStep`; `yield` in `defer`/`finally` rejected (`YieldInCleanup`), `await` allowed. |
| M10 | HQL query bodies | **Fixed** (spec §9.3: plan target owns query structure; HIR carries scalar expressions). |
| M11 | Subtree copy | **Fixed.** `Builder::copy_subtree` with fresh inner binders. |
| Low | Pat literal pooling | **Fixed differently.** Range bounds became child patterns (`Pat` 56 → 32 bytes) instead of a literal pool. |
| Low | `implicit_captures` O(n·depth) | **Fixed.** Linear; plus `all_implicit_captures`. |
| Low | Per-construct `Vec`s in validator | **Fixed.** Stamp arrays for duplicate-binding and or-pattern checks. |
| Low | `IntLit` −0 | **Fixed.** `Malformed::NegativeZero`. |
| Low | Or-pattern binding modes | **Fixed.** `Malformed::OrPatternModes`. |
| Low | Compound assignment operators | **Fixed.** Only arithmetic, bitwise, shifts. |
| Low | `promote` wording | **Fixed** (spec §12 matches OPS §2, including `div`). |

Deferred by this release (recorded per the anti-deferral rule): the body-graph implementation
(M2, H4's unwind edges as code) and the encoding's symbol table (M8) stay in v0.5.0 with the rest
of the body graph and the encoding, because both need the 0.5 consumers' shape; the 0.3 data
model already carries everything they read.

Delivered: the forms and APIs listed above; tests `tests/features.rs` (20) and
`tests/spec_examples.rs` (5), two lenient-mode property tests; every earlier test updated to
0.3 semantics.

## v0.5.0 - Implementation
- [ ] Textual form (printer + parser) with round-trip property tests.
- [ ] Body-graph view: control flow over places for flow-sensitive capabilities (ownership checking, D12), per spec §19 (NLL inputs, unwind edges for every fallible op; review findings M2/H4).
- [ ] Versioned binary encoding with the symbol table of spec §21 (review finding M8); iterative rewriter. (Benchmarks at 100k-1M nodes: delivered in 0.2.0.)

## v0.9.0 - Hardening
- [ ] Fuzz targets (decoder, textual parser, validator); budgets on decode size and depth.
- [ ] Audit against lower-lang, resolve-lang, typeck-lang, and match-lang as real consumers.

## v1.0.0 - Stable
- [ ] Frozen only after lower-lang, resolve-lang, and typeck-lang exercise the API end to end (LexerSketch D18).
