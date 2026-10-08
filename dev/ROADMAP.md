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

## v0.5.0 - Implementation
- [ ] Textual form (printer + parser) with round-trip property tests.
- [ ] Body-graph view: control flow over places for flow-sensitive capabilities (ownership checking, D12).
- [ ] Versioned binary encoding; iterative rewriter. (Benchmarks at 100k-1M nodes: delivered in 0.2.0.)

## v0.9.0 - Hardening
- [ ] Fuzz targets (decoder, textual parser, validator); budgets on decode size and depth.
- [ ] Audit against lower-lang, resolve-lang, typeck-lang, and match-lang as real consumers.

## v1.0.0 - Stable
- [ ] Frozen only after lower-lang, resolve-lang, and typeck-lang exercise the API end to end (LexerSketch D18).
