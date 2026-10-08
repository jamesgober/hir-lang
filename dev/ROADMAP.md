# hir-lang - Roadmap

> Path from scaffold to a stable 1.0. Hard parts are front-loaded; each phase has hard exit criteria.
> Master plan: ../_lexersketch/ROADMAP.md and ../_lexersketch/NEW-LIBS.md
>
> **Anti-deferral rule:** no listed hard task moves to a later phase unless this file records the move and the reason.

## v0.1.0 - Scaffold (DONE)
Compiles, CI green, structure correct, no domain logic.
- [x] Manifest, README, CHANGELOG, REPS, dual license, CI, deny, clippy, rustfmt, DIRECTIVES, ROADMAP.

## v0.2.0 - Foundation
- [ ] Core forms: items (fn, record, sum, interface, alias, const, module), expressions, statements, patterns, type terms (optional), effects.
- [ ] Dense arena storage with typed ids; builder with origin capture; validator with precise errors.
- [ ] Wires span-lang and intern-lang. Property tests for builder/validator invariants.

## v0.5.0 - Implementation
- [ ] Textual form (printer + parser) with round-trip property tests.
- [ ] Body-graph view: control flow over places for flow-sensitive capabilities (ownership checking, D12).
- [ ] Versioned binary encoding; iterative rewriter; benchmarks at 100k-1M nodes.

## v0.9.0 - Hardening
- [ ] Fuzz targets (decoder, textual parser, validator); budgets on decode size and depth.
- [ ] Audit against lower-lang, resolve-lang, typeck-lang, and match-lang as real consumers.

## v1.0.0 - Stable
- [ ] Frozen only after lower-lang, resolve-lang, and typeck-lang exercise the API end to end (LexerSketch D18).
