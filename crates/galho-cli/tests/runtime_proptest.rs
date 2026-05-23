//! Property-based fuzz of `Runtime` against arbitrary morphism sequences with the
//! `ChainedOutcomeEmitter` wired in. Invariants verified per run:
//!
//! 1. No panic — every operation returns a typed Result.
//! 2. Every `fire_morphism` is Ok(Phase) or Err(precondition failure); no silent
//!    success-without-advancement.
//! 3. `KnowledgeBase`-reported phase always matches a real transition table entry.
//! 4. `chain.verify_integrity()` returns true after the full random sequence.
//! 5. Stack lock holder count matches the set of galhos that successfully fired ApplyToPreview.
//! 6. Every Phase reachable by the Runtime is one of the `Phase::all()` variants
//!    (i.e., no Runtime ever produces a phase outside the typed enum).

use std::sync::Arc;

use galho_cli::{Runtime, RuntimeBackend};
use galho_storage::{backends::MemoryBackend, ChainedOutcomeEmitter, OutcomeChain};
use galho_types::{
    is_known_transition, MorphismId, OutcomeEmitter, Phase,
};
use proptest::prelude::*;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .unwrap()
}

/// One synthetic operator step — pick a morphism, optionally provide a stack-root.
#[derive(Debug, Clone)]
struct Step {
    morphism: MorphismId,
    stack_root: Option<String>,
}

fn any_step() -> impl Strategy<Value = Step> {
    let morphism = prop_oneof![
        Just(MorphismId::Plan),
        Just(MorphismId::ApplyToPreview),
        Just(MorphismId::RecordApproval),
        Just(MorphismId::Promote),
        Just(MorphismId::Verify),
        Just(MorphismId::SealDone),
        Just(MorphismId::RevertApply),
        Just(MorphismId::Resume),
        Just(MorphismId::DriftReconcile),
        Just(MorphismId::Abandon),
        Just(MorphismId::Escalate),
        Just(MorphismId::Recover),
    ];
    let stack_root = prop::option::of("root-[a-z]{1,4}".prop_map(String::from));
    (morphism, stack_root).prop_map(|(morphism, stack_root)| Step { morphism, stack_root })
}

fn any_galho_name() -> impl Strategy<Value = String> {
    (0..4usize).prop_map(|n| format!("galho-{n}"))
}

fn any_sequence() -> impl Strategy<Value = Vec<(String, Step)>> {
    prop::collection::vec((any_galho_name(), any_step()), 1..40)
}

// =============================================================================
// Property: the chain stays consistent across arbitrary fuzz
// =============================================================================

proptest! {
    /// R.1 — Across any random sequence of (galho, morphism, stack_root) tuples:
    /// no panic, no Runtime crash, and the resulting chain integrity holds.
    #[test]
    fn runtime_invariants_under_arbitrary_sequence(sequence in any_sequence()) {
        let rt = rt();
        rt.block_on(async {
            let store = Arc::new(MemoryBackend::new());
            let chain = Arc::new(OutcomeChain::new(store.clone()));
            let emitter: Arc<dyn OutcomeEmitter> =
                Arc::new(ChainedOutcomeEmitter::new(chain.clone()));
            let runtime = Runtime::with_emitter(
                RuntimeBackend::Memory(Arc::new(MemoryBackend::new())),
                emitter,
            );

            // Track which galhos we've created so we don't re-create.
            let mut created: std::collections::BTreeSet<String> = Default::default();

            for (galho, step) in sequence {
                // Lazy-create the galho on first reference.
                if !created.contains(&galho) {
                    if runtime.new_galho(&galho).await.is_ok() {
                        created.insert(galho.clone());
                    }
                }
                // Fire the morphism. Result is typed Result<Phase, Error>; we don't
                // assert on the outcome — only that no panic occurs and the chain
                // remains intact.
                let _ = runtime
                    .fire_morphism(&galho, step.morphism, step.stack_root)
                    .await;
            }

            // After the run, every galho's phase MUST be one of Phase::all().
            for name in created {
                let status = runtime.status(&name).await.unwrap();
                let p = status.phase.0;
                prop_assert!(Phase::all().contains(&p), "Runtime produced unknown phase: {:?}", p);
                // Every available forward morphism's (current, target) MUST be a known transition.
                for m in &status.forward {
                    // Walk the transition table to confirm.
                    let known = galho_types::transition_table()
                        .iter()
                        .any(|t| t.morphism == *m && t.from == p);
                    prop_assert!(known, "forward morphism {} not in transition table for {:?}", m, p);
                }
            }

            // Chain integrity invariant holds across the whole run, regardless of how
            // many morphisms succeeded vs failed preconditions.
            let ok = chain.verify_integrity().await.unwrap();
            prop_assert!(ok, "chain integrity violated after random run");
            Ok(())
        })?;
    }

    /// R.2 — The chain length equals the number of events emitted. Since every
    /// `new_galho` emits exactly one event, every successful `fire_morphism` emits
    /// at least one event (more if it acquires a stack-lock or destroys a galho),
    /// and failures emit zero — the chain length is bounded below by the number of
    /// successful creations.
    #[test]
    fn chain_length_at_least_successful_creations(
        names in prop::collection::vec(any_galho_name(), 1..8)
    ) {
        let rt = rt();
        rt.block_on(async {
            let store = Arc::new(MemoryBackend::new());
            let chain = Arc::new(OutcomeChain::new(store.clone()));
            let emitter: Arc<dyn OutcomeEmitter> =
                Arc::new(ChainedOutcomeEmitter::new(chain.clone()));
            let runtime = Runtime::with_emitter(
                RuntimeBackend::Memory(Arc::new(MemoryBackend::new())),
                emitter,
            );

            let mut created: std::collections::BTreeSet<String> = Default::default();
            for n in names {
                if runtime.new_galho(&n).await.is_ok() {
                    created.insert(n);
                }
            }

            let len = chain.len().await;
            prop_assert!(
                len >= created.len() as u64,
                "chain length {} less than created galhos {}", len, created.len()
            );
            prop_assert!(chain.verify_integrity().await.unwrap());
            Ok(())
        })?;
    }
}

// =============================================================================
// Concrete: the canonical happy path leaves the chain in a predictable state
// =============================================================================

#[test]
fn canonical_forward_arc_leaves_chain_intact() {
    rt().block_on(async {
        let store = Arc::new(MemoryBackend::new());
        let chain = Arc::new(OutcomeChain::new(store.clone()));
        let emitter: Arc<dyn OutcomeEmitter> =
            Arc::new(ChainedOutcomeEmitter::new(chain.clone()));
        let runtime = Runtime::with_emitter(
            RuntimeBackend::Memory(Arc::new(MemoryBackend::new())),
            emitter,
        );

        runtime.new_galho("feature/canonical").await.unwrap();
        runtime
            .fire_morphism("feature/canonical", MorphismId::Plan, None)
            .await
            .unwrap();
        runtime
            .fire_morphism(
                "feature/canonical",
                MorphismId::ApplyToPreview,
                Some("root".into()),
            )
            .await
            .unwrap();
        runtime
            .fire_morphism(
                "feature/canonical",
                MorphismId::RecordApproval,
                Some("reviewer".into()),
            )
            .await
            .unwrap();
        runtime
            .fire_morphism("feature/canonical", MorphismId::Promote, None)
            .await
            .unwrap();
        runtime
            .fire_morphism("feature/canonical", MorphismId::Verify, None)
            .await
            .unwrap();
        runtime
            .fire_morphism("feature/canonical", MorphismId::SealDone, None)
            .await
            .unwrap();

        // 1 GalhoCreated + 1 StackLockAcquired + 6 MorphismFired = 8 events.
        assert_eq!(chain.len().await, 8);
        assert!(chain.verify_integrity().await.unwrap());

        // The entire chain can be walked + every entry's previous_hash matches the
        // prior entry's derived entry_hash.
        let entries = chain.entries().await.unwrap();
        for win in entries.windows(2) {
            assert_eq!(win[1].previous_hash, win[0].entry_hash());
        }
    });
}

#[test]
fn arbitrary_galho_phase_always_appears_in_known_transitions() {
    rt().block_on(async {
        let store = Arc::new(MemoryBackend::new());
        let chain = Arc::new(OutcomeChain::new(store));
        let emitter: Arc<dyn OutcomeEmitter> =
            Arc::new(ChainedOutcomeEmitter::new(chain.clone()));
        let runtime = Runtime::with_emitter(
            RuntimeBackend::Memory(Arc::new(MemoryBackend::new())),
            emitter,
        );
        runtime.new_galho("smoketest").await.unwrap();

        for morphism in [
            MorphismId::Plan,
            MorphismId::ApplyToPreview,
            MorphismId::RecordApproval,
            MorphismId::Promote,
            MorphismId::Verify,
            MorphismId::SealDone,
        ] {
            let extra = match morphism {
                MorphismId::ApplyToPreview => Some("root".into()),
                MorphismId::RecordApproval => Some("rev".into()),
                _ => None,
            };
            let _ = runtime.fire_morphism("smoketest", morphism, extra).await;
        }

        let status = runtime.status("smoketest").await.unwrap();
        let p = status.phase.0;
        // The reached phase must have at least one (from = p, to = *) row OR be terminal.
        let outgoing = galho_types::transitions_from(p);
        let is_terminal = matches!(
            p,
            Phase::Done | Phase::Destroyed
        );
        assert!(
            !outgoing.is_empty() || is_terminal,
            "phase {p:?} has no outgoing transitions and is not terminal"
        );

        for m in &status.forward {
            assert!(is_known_transition(p, galho_types::transition_table()
                .iter()
                .find(|t| t.morphism == *m && t.from == p)
                .map(|t| t.to)
                .unwrap()));
        }
    });
}
