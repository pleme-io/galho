# galho

skip-format-ban: migration in progress — see `crates/galho-cli/tests/format_ban_guard.rs` for the typed migration ratchet (count cannot grow). 41 remaining call sites as of 2026-06-02 (CEILING pinned in `format_ban_guard.rs`) after extracting `TextMarker` + `SyncSummary` typed Display newtypes; ratchet test fails if any new code grows the count. Remove waiver + add workspace `clippy.toml` (`disallowed_macros = ["std::format"]`) when CEILING reaches 0.

Branch-aware typed IaC state (caixa Biblioteca kind).

> **Theory:** `pleme-io/theory/GALHO.md`
> **Operator doc:** `pleme-io/docs/galho.md`
> **Skill:** `pleme-io/blackmatter-claude/skills/galho/SKILL.md` (TODO — write once M2 ships the CLI)
> **Family:** carve (PR shape, sibling), vitrine (PR evidence, sibling), magma (execution layer; consumed at M3), engenho-controllers (Controller trait; consumed at M4), tameshi (canonicalization + audit chain; consumed already).

`galho` lifts IaC state into a typed, content-addressed Merkle DAG so branches stop clobbering shared state and plans stop being text-in-a-PR-comment.

## Workspace layout (v0.1 — M1)

```
crates/
  galho-types/      canonical Resource Graph IR + CanonicalBytes (consumes
                    tameshi::Canonicalizer) + TypedState<S> + Plan<S> +
                    TypedConflict + three-way merge algebra. 24 tests.
  galho-storage/    git-style content-addressed object store + Merkle DAG +
                    reference-counted GC. Backends: MemoryBackend (tests),
                    LocalFsBackend (canonical dev). 12 tests.
```

## Crates (M2+, per `theory/GALHO.md` §XII.2)

```
crates/
  galho-terraform   reference IaCSystem adapter via magma::StateBackend
  galho-helm        Helm release state adapter
  galho-crossplane  managed-resource CR adapter
  galho-controller  engenho_controllers::Controller impl
  galho-cli         operator-facing CLI binary
```

## Build / Test

```bash
nix build .#galho
nix run   .#galho
cargo build --workspace
cargo test  --workspace            # tests pass across all crates
```

## Conventions

- Rust edition 2024, MIT license (workspace defaults).
- substrate's `rust-workspace-release-flake.nix` for the flake (`nix run .#galho` → `galho-cli`).
- caixa-native: `caixa.lisp` declares Biblioteca kind.
- Consumes tameshi via path dep during in-tree development; release workflow swaps to git dep before publish.
- Property-based testing via `proptest`. 243 passing tests across the workspace covering canonicalization, three-way merge, the phase FSM, the sealed-type invariants, and the storage CAS/lineage algebra.

## What this library deliberately doesn't do

- **Doesn't execute plans against cloud.** That's magma's job. galho is the *typed state algebra*; magma is the *executor*. The forthcoming `galho-magma` (M3) is the adapter between them.
- **Doesn't author IaC declarations.** That's terreno's job. galho operates on the rendered state, not the source declarations.
- **Doesn't run the controller loop itself.** That's engenho's `ControllerRuntime` job (consumed at M4 via `engenho_controllers::Controller`).
- **Doesn't manage secrets.** Cofre owns secret materialization. galho's IR carries `SecretRef` typed references; resolution happens at apply time by the adapter calling cofre. Secret values never enter the canonical hash input.
- **Doesn't make merge-order policy decisions.** carve gate sequences PR merges; galho gate sequences IaC deploys. Both run as siblings, both must pass.

## Consumption posture (per the org Prime Directive three-site rule)

Two consumed traits, no premature extractions:

- `tameshi::canonicalize::Canonicalizer` — canonical second consumer; advances the trait toward fleet-wide saturation.
- `engenho_controllers::Controller` — canonical second consumer outside engenho's own controllers (consumed at M4 by `galho-controller`).

Two deferred extractions with named triggers in the spec:

- Merkle-DAG / content-addressed object-store primitive: re-evaluate when carve's backup-tag chain or shigoto's audit-log emitter develops the same shape.
- Typed three-way-merge primitive: re-evaluate when tatara-lisp, typescape, or caixa develop AST-merge needs (`dq-merge` is the untyped reference for the merge-detection strategy).

## Status

v0.1 — all 5 crates real (galho-types / -storage / -cli / -controller / -terraform). 243 tests green; format ratchet 41/41.

**FIXABLE-backlog landed 2026-06-02** (local commits `602a243`..`aac811e`; closes the unrepresentability-model FIXABLE rows from `theory/ECLUSA.md` §XVIII.4 / `theory/UNREPRESENTABILITY.md` toward golden-capable flows):
- `galho gate` deploy-order subcommand (sibling of `carve gate`).
- `Runtime::deliver_signal` — typed `ExternalSignal` (discriminant-matched) ingress + controller pass-through (closes the M4.5 webhook gap).
- `MorphismRequirement::MissingTransitionRow` — hard error replacing the silent `unwrap_or_else(m.to_phase())` fallback.
- `AppliedStatus` smart-ctor — all-zeros Applied hash made unrepresentable.
- Sealed `StackLock` (private fields + `try_from_parts` + hand-written `Deserialize`) + `Refined<Duration, ApplyTtlBounds>` bounded TTL.
- `TypedState<S>` serde `try_from` re-tag asserts `meta.iac_system == S::id()` (closes cross-system type-confusion).
- `advance_ref` lineage check + `AdvanceOutcome`/`Divergence` + `Addressed` witness + `cas_delete_ref` (both backends).
- Sealed `MorphismContext.current_phase` behind `KnowledgeBase::advance` (closes the precondition bypass) + additive `Galho<P>` phantom-typestate facade (forward arc; illegal transition = compile error).

**Not yet pushed** — galho has no GitHub remote; commits are local-only. **Next:** create `pleme-io/galho` + push; then the CEILING controllers (drift / quota lease / reap `ReapJob`), galho-terraform via magma (M2/M3), and the `Galho<P>` backward+failure arc (9b v2).
