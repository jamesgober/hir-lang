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

[Unreleased]: https://github.com/jamesgober/hir-lang/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/jamesgober/hir-lang/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/jamesgober/hir-lang/releases/tag/v0.1.0
