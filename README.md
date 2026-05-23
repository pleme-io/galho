# galho

> Branch-aware typed IaC state. Every PR touching infrastructure gets an isolated typed state-projection that inherits from its base.

`galho` (Brazilian-Portuguese: tree branch) lifts Infrastructure-as-Code state into a typed, content-addressed Merkle DAG so branches stop clobbering shared state and plans stop being text-in-a-PR-comment.

## What problem this solves

Git assumes immutable history + deferred conflict resolution at merge time. IaC `apply` is an immediate global mutation on shared state. These two models break each other:

- **Branch-clobber.** Engineer A applies their branch; engineer B's apply overwrites it minutes later.
- **Plan-staleness.** A plan in PR-A is stale the moment PR-B's apply lands. No rebase for plans.
- **Apply-before-vs-after-merge.** The industry has named the dilemma but not solved it. Both paths force serialization at a different layer.
- **Review fatigue.** Plan output is text; reviewers can't mechanically compare it to "what state exists post-merge."

`galho` fills the missing primitive — a typed, content-addressed, branchable state value with a real three-way merge algebra.

## Status

**v0.1 (M1)** — `galho-types` + `galho-storage` complete with proptest coverage:

- Canonical Resource Graph IR (flat resource graph; adapter-agnostic).
- `CanonicalBytes` trait emitting canonical bytes; consumes `tameshi::canonicalize::Canonicalizer` for BLAKE3 hashing.
- `TypedState<S>` with `PhantomData<S: IaCSystem>` — `Plan<Terraform>` cannot apply to `TypedState<Crossplane>` at compile time.
- `Plan<S>` as typed value (not text).
- `TypedConflict` with Structural / Semantic / CrossSystem classes.
- Three-way merge algebra with 13-case matrix + per-attribute-path refinement (disjoint-attribute changes are clean; same-path divergence surfaces a typed conflict).
- Git-style content-addressed object store with `MemoryBackend` + `LocalFsBackend`, BLAKE3 content addressing, atomic CAS on named refs, reference-counted GC.
- 36 passing tests across both crates.

## Roadmap

M2 — `galho-cli` for local workflows.
M3 — `galho-terraform` IaCSystem adapter via `magma::StateBackend` (terraform first-class).
M4 — `galho-controller` on engenho via `engenho_controllers::Controller`; GitHub webhook auto-create.
M5 — typed plan rendering via vitrine into PR bodies.
M6 — multi-IaC adapters (crossplane, helm).
M7 — OutcomeChain + AnomalyChain integration; `kensa verify`.
M8 — fleet rollout; pangea-operator repo catalog entry.

See [`theory/GALHO.md`](https://github.com/pleme-io/theory/blob/main/GALHO.md) for the canonical destination spec.

## License

MIT.
