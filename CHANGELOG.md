<h1 align="center">
    <img width="90px" height="auto" src="https://raw.githubusercontent.com/jamesgober/jamesgober/main/media/icons/hexagon-3.svg" alt="Triple Hexagon">
    <br><b>CHANGELOG</b>
</h1>
<p>
  All notable changes to <code>hir-lang</code> will be documented in this file. The format is based on <a href="https://keepachangelog.com/en/1.1.0/">Keep a Changelog</a>,
  and this project adheres to <a href="https://semver.org/spec/v2.0.0.html/">Semantic Versioning</a>.
</p>

---

## [Unreleased]

---

## [0.4.0] - 2026-10-09

A breaking 0.x revision for the HIR gaps that writing the LSF2 spec and the Mox
sketch found (ISSUES P12, P13, P20–P23): union and intersection types, place-once
compound assignment and the value of assignments, keyed yields and `yield from`,
logical versus bitwise `not`, the `NoMatch` runtime error, `pow` and
`shift = saturate` from OPS v2, lenient repair proven to need at most two rounds,
and `lookup_local_in`. The spec `specs/HIR.md` is revised to v3.

### Breaking

- **`Expr::Yield(Option<ExprId>)` is now `Expr::Yield { key, value }`** (PHP/Mox
  `yield $k => $v`); `Expr::YieldFrom(ExprId)` and `Expr::LetPlace { binder, place,
  body }` are new expression forms, and `Ty::Union` / `Ty::Intersection` new type
  forms, so exhaustive matches on `Expr` and `Ty` need arms for them.
- **`OpKind::Not` is logical negation only**; the bitwise complement is the new
  `OpKind::BitNot` (`bit_not`). Lowerings that used `not` on integers must switch.
  `OpKind::Pow` is new, and `Shift::Saturate` is a new policy value.
- **`BinderKind::Place`** is new (the binder of `let_place`); exhaustive matches on
  `BinderKind` need it.
- **`EffectProblem::YieldInCleanup` is removed**: a `yield` inside `finally` or
  `defer` is valid (LSB rule 12; yielding while a generator is being closed is a
  run-time error, `CloseIgnored`).
- **New `Malformed` problems**: `TypeArity`, `TypeNesting`, `TypeOrder`, `YieldKey`.
- **`finish` and `finish_lenient` normalize unions and intersections** before
  validating (flatten, hoist `Nullable`, sort, drop repeats, collapse a single
  member), so a union node may come back sorted, wrapped in `Nullable`, or replaced
  by its only member.
- **Unreachable reporting**: an orphaned subtree is reported once, at its top (strict
  mode: the first top in arena order; before, the first orphan in arena order,
  usually a child).
- **Lenient repair** reports a duplicate binding at every repeat (not just the
  first) and every problem of an or-pattern's alternatives; a second binding site is
  reported as `BinderBoundTwice` without a kind change for the binder.

### Added

- **Union and intersection types** (P20) with a canonical form (spec §5.1): at
  least two members, no same-kind nesting (no `Nullable` in a union), members
  strictly ordered by a resolution-independent key; members with error forms,
  constant expressions, or more than 64 nodes are exempt, which keeps every check
  linear.
- **`Expr::LetPlace`** (P21): evaluate a place's operands once and name the place,
  for `.=` with a host operator, `**=`, `%=`, `??=`, and `++`/`--` templates; the
  alias crosses no frame. `Assign` is documented to evaluate to the assigned value,
  and the spec gives bcgen-lang's lowering of both.
- **Keyed `Yield` and `YieldFrom`** (P22), mapped onto LSB `yield_kv` and a
  delegation loop; the **`NoMatch`** runtime error (E0200, in the new HIR range
  E0200–E0299) for a `match` without a matching arm.
- **`OpKind::Pow`, `OpKind::BitNot`, `Shift::Saturate`** (P22/P23, OPS v2); `pow`
  is a valid compound operator and consults `overflow` (`promote` allowed on dynamic
  results).
- **`Hir::lookup_local_in(path, name, ns)`** (P13).
- Tests: `tests/features_0_4.rs` (20), canonical-form unit tests (6), a §9.6 spec
  example (Mox `.=` with a union parameter), the garbage generator extended with
  the 0.4 forms; benchmarks for 100,000 unions and a 100,000-deep union nest.

### Fixed

- **Lenient repair terminates in at most two rounds without a fallback** (P12).
  Each round now removes the consequences of its own repairs (nodes it cut off,
  references to binders whose site disappeared or whose kind it changed) before
  the next pass, and reports every later-round problem instead of treating it as a
  cascade. The 0.3 loop (16 rounds, then emptying the root module) is gone; a
  debug assertion checks the bound under the lenient property tests, and a hard
  limit turns a broken argument into an error rather than a hang.
- The or-pattern check kept the *last* binding mode of a repeated binder in the
  first alternative; it keeps the first, matching the repair.

---

## [0.3.0] - 2026-10-08

A breaking 0.x revision that closes the design gaps an adversarial review of
0.2.0 found, while breaking is cheap: compilation units and cross-unit
definitions, partial (rustc-style) path resolution, lenient validation that
turns user errors into diagnostics, the PHP/Mox forms, low-level forms
(unions, atomics, inline assembly), and the scope/frame information a resolver
needs. The spec (`specs/HIR.md`) is revised to v2 to match; its §9 examples are
now built, validated, and printed by a test that checks the spec verbatim.

### Breaking

- **Resolutions name definitions through units.** `Res::Item(ItemId)` and
  `Res::Variant(VariantId)` are replaced by `Res::Def(DefId)`, where
  `DefId { unit: UnitId, def: Def::Item | Def::Variant }`. Mint this unit's
  ids with `Builder::def` / `Hir::def`; other units' with `DefId::foreign`.
  `Res::Extern(Symbol)` is new (host/stdlib symbols).
- **`Path`** gained `root: PathRoot` (replaces `global: bool`), `qself:
  Option<QSelf>`, and `unresolved: u32`; build paths with `Path::new(segments,
  ns)`. `Segment::args` is now `List<GenericArg>` (was `List<TyId>`).
- **Types.** `Ty::Const` is removed (use `GenericArg::Const`); `Ty::Object`
  takes `List<Bound>`; `Ty::Fn` gained `abi`. Bounds everywhere are
  `List<Bound>` (generic parameters, where-predicates, associated types);
  `WherePred { ty, bounds }` became `WherePred { subject: Bound, bounds }`;
  `GenericParam::default` is `Option<GenericArg>`.
- **Patterns.** `Pat::Range` bounds are child patterns (`Option<PatId>`, a
  literal or constant path) instead of inline literals.
- **Items.** `ClassDef::base: Option<TyId>` became `bases: List<TyId>`;
  `ItemKind::Module` gained `body` and `effects`; `FnDef` gained `defaults`;
  `RecordDef` gained `is_union`; `ClassDef` gained `mixin`; `Param` gained
  `by_ref`; `Vis` gained `Protected`; `ItemKind::MixinUse` is new.
- **Expressions.** `Arg` gained `place`; `Closure` gained `self_binder` and
  `defaults` (use `Closure::new(body)` with struct update);
  `MethodCall::generic_args` is `List<GenericArg>`; new variants `DynField`,
  `DynMethodCall`, `VarVar`, `Append`, `RefAssign`, `Asm`, `Intrinsic`; new
  statements `Stmt::Static` and `Stmt::Global`.
- **Walk events.** `Event` is `#[non_exhaustive]` and also reports
  `ScopeOpen`, `ScopeClose`, `Bind`, `FrameOpen(Frame)`, `FrameClose`;
  matches on `Event` need a wildcard arm.
- **Errors.** `HirError::BinderNotBound` is removed (unbound binders are now
  valid); `DuplicateName` gained `index`; `ForeignDef` is new; new
  `Malformed`, `JumpProblem::ContinueInStep`, and
  `EffectProblem::YieldInCleanup` problems.
- **Validity changed.** Jumps out of `finally` are now allowed (they override
  the pending completion; `defer` still forbids them); dead error-form nodes
  are allowed; a `continue` inside its own loop's `step`, `yield` inside
  `defer`/`finally`, `-0` integer literals, compound assignment with a
  non-arithmetic operator, and or-patterns whose alternatives bind with
  different modes are now rejected.
- **Accessors.** `Hir::param` returns `&Param` (was `Option`), like the other
  total accessors; foreign ids are a `debug_assert!` failure in debug builds.

### Added

- `UnitId`, `Builder::for_unit`, `Builder::unit`, `Hir::unit`; tagged
  `DefId`s that a different `Hir` of the same unit rejects (`ForeignDef`).
- Partial resolution: `Hir::resolve_partial(path, res, unresolved)`, path
  roots (`::`, `self::`, `super::`, `Self::`, `parent::`, `static::`), and
  qualified selves (`<T as Tr>::Out`).
- `Builder::finish_lenient`: collects every problem, repairs each node to its
  error form (or a narrower fix), suppresses cascades, and returns a valid
  `Hir` with the problems in source order. `HirError::node()`.
- `Hir::lookup_local` (innermost in-scope binder by name, O(log n + d)) and
  scope/binder/frame walk events.
- `Hir::get_*` accessors returning `Option`, and
  `Hir::all_implicit_captures` (every closure in one walk).
- `Builder::copy_subtree`: subtree copy with fresh binders for everything
  bound inside.
- PHP/Mox forms: by-reference parameters and place arguments, reference
  assignment, `static`/`global` locals, dynamic members and calls,
  variable-variables, append places, class `bases`, traits (`mixin` classes
  and `MixinUse` with `insteadof`/`as` rules), `protected`.
- Low-level forms: unions, function-type ABIs, `Intrinsic` (atomics with C++20
  memory-order checks, fences, volatile, `Named`), structured inline
  assembly (`Asm` with template placeholder checks).
- Generic arguments: associated-type bindings and constraints, regions;
  `Ty::Impl`; region bounds.
- `Pat::Ident` (binding-or-constant, decided by resolve-lang), recursive
  closures (`self_binder`), parameter defaults in their own frame with
  `DefaultEval::Once` for Python, module bodies with effects.
- Tests: `tests/features.rs` (0.3 forms and APIs), `tests/spec_examples.rs`
  (spec §9 examples, checked verbatim against the spec), lenient-mode
  property tests (always valid, agrees with strict).

### Changed

- Spec `specs/HIR.md` v2: units, partial resolution, lenient validation,
  finally/generator semantics (throw-into, close, async generators) and the
  LSB mapping, the corrected effects claim (declared effects do not bound
  run-time failures), the body-graph design for NLL with unwind edges for
  every fallible operation (implementation stays in 0.5), the encoding's
  symbol table, HQL's plan-target split, and corrected §9 examples.
- `implicit_captures` is linear in the closure (was O(n · depth)); duplicate
  and or-pattern checks use stamp arrays instead of per-construct vectors.
- `Pat` shrank from 56 to 32 bytes; `Path` grew from 20 to 40 bytes and `Ty`
  from 20 to 24 (`Expr` stays 40, `Item` 72).

---

## [0.2.0] - 2026-10-08

The foundation: the HIR data model of the LexerSketch spec (`specs/HIR.md`
§1–§17), with dense arena storage, a builder that captures origins, a total
validator, iterative traversal, in-place checked resolution, and a
deterministic debug printer.

### Added

- **Node forms.** Items (`Fn` with default/named/variadic/receiver
  parameters, `Record`, `Sum`, `Class`, `Interface`, `Impl`, `Alias`,
  `AssocType`, `Const`, `Global`, `Module`, `Import`, `Err`); expressions
  (literals, paths, aggregates, maps, calls, method calls, field and index
  access, intrinsic ops, casts, assignment, borrows, blocks, `if`, `match`,
  one `Loop` with a `step`, labeled `break`/`continue`, `return`, closures with
  explicit captures and an implicit-capture mode, `throw`, `try`/`catch`/
  `finally`, `await`, `yield`, `spawn`, `Err`); statements (`let` and
  let-else, expression, local item, `defer`, `Err`); patterns (wildcard,
  binding with mode and sub-pattern, literal, range, tuple, constructor,
  record, path, slice with rest, or, reference, type test, `Err`); optional
  type terms (including `Any`, `Nullable`, `Object`, `Const`, function types
  with effects); attributes on any node.
- **Storage.** Dense per-kind arenas of `Copy` nodes with 4-byte typed ids
  (`Option<Id>` is 4 bytes), out-of-line `List<T>` pools, a text pool for
  literal payloads, parallel origin arrays, and a sorted attribute table.
  Clone, equality, debug, and drop never recurse.
- **Origins and hygiene.** `Origin` (span-lang 0.4 `Span` plus `ExpnId`),
  expansion records with ordered parents and definition sites, and `Name`
  (symbol plus hygiene mark). The builder stamps its current origin on every
  node and binder.
- **Binders and resolution.** Unique binder ids with kinds (`Local`, `Param`,
  `Capture`, `TypeParam`, `ConstParam`, `Region`, `Label`); paths with a
  namespace fixed by their parent and a resolution slot; `Hir::resolve` checks
  namespace, scope, and frames in O(1) and keeps the HIR valid;
  `Hir::can_reference` and `Hir::implicit_captures`.
- **Intrinsic operations.** `Op`/`OpKind` for every operation of `specs/OPS.md`
  with a per-instance `Policy` that must carry exactly the fields the op
  consults, including the `promote` overflow policy (OPS section 2) for
  dynamically typed results, rejected where HIR fixes a static result type.
- **Validator** (`Builder::finish`, `Hir::validate`): total and linear; checks
  ids and ranges, expansion order, the tree property, binder sites and kinds,
  or-pattern binder sets, namespaces and resolutions, scopes and frames
  (captures, nested items, constant contexts), jump targets, effect
  placement, op arity and policies, literals, and node shapes. 23
  `#[non_exhaustive]` error variants (27 shape problems) with precise sites.
- **Traversal.** `walk` (Tier-1), `Hir::walk_from` with enter/leave events and
  skip/stop, `Hir::children_into`; one canonical order shared by the walker,
  the validator, and the printer.
- **Printer.** `print`, `print_with` (optional origins), `print_into`:
  deterministic S-expressions, linear output at any depth.
- Examples `quickstart` and `hygiene`; criterion benchmarks at 82k, 825k, and
  1,000,000 nodes; integration, depth, and property tests (with a reference
  implementation of scoping and jumps).
- The specification `specs/HIR.md` in the LexerSketch plan, including the
  0.5 designs for the body-graph view (decision D12), the textual syntax, and
  the binary encoding.

### Changed

- `span-lang` 0.4 and `intern-lang` 1 wired as dependencies; the `std`
  feature forwards to both.

---

## [0.1.0] - 2026-10-08

Initial scaffold and repository bootstrap. No domain logic yet &mdash; this release establishes the structure, tooling, and quality gates the implementation will be built on.

### Added

- `Cargo.toml` with crate metadata, Rust 2024 edition, MSRV 1.85.
- Dual `Apache-2.0 OR MIT` license files.
- `README.md`, `CHANGELOG.md`, and a documentation skeleton.
- `REPS.md` compliance baseline.
- `.github/workflows/ci.yml` CI matrix; `deny.toml`, `clippy.toml`, `rustfmt.toml`.
- `dev/DIRECTIVES.md` and `dev/ROADMAP.md` (committed engineering standards + plan).

[Unreleased]: https://github.com/jamesgober/hir-lang/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/jamesgober/hir-lang/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/jamesgober/hir-lang/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/jamesgober/hir-lang/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/jamesgober/hir-lang/releases/tag/v0.1.0
