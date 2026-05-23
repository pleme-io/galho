//! Tests for `OutcomeChain` — the hash-linked typed audit chain.
//!
//! Load-bearing invariant: `entry[i].previous_hash == entry[i-1].entry_hash` for every
//! adjacent pair. If this regresses, the chain stops being an audit chain.

use std::sync::Arc;

use galho_storage::{backends::MemoryBackend, ChainedOutcomeEmitter, OutcomeChain};
use galho_types::{
    MorphismId, OutcomeEmitter, OutcomeEvent, OutcomeEventType, Passaporte, Phase,
};

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .unwrap()
}

fn sample_event(galho: &str, morphism: MorphismId, from: Phase, to: Phase) -> OutcomeEvent {
    OutcomeEvent::new(OutcomeEventType::MorphismFired, galho)
        .with_actor(Passaporte::new("test-actor"))
        .with_phase_transition(from, to)
        .with_morphism(morphism)
}

#[test]
fn empty_chain_has_zero_length_and_zero_head() {
    rt().block_on(async {
        let store = Arc::new(MemoryBackend::new());
        let chain = OutcomeChain::new(store);
        assert_eq!(chain.len().await, 0);
        assert!(chain.is_empty().await);
        assert_eq!(chain.head_hash().await, galho_types::Blake3Hash::from([0u8; 32]));
        assert!(chain.verify_integrity().await.unwrap());
    });
}

#[test]
fn single_append_advances_head_and_sequence() {
    rt().block_on(async {
        let store = Arc::new(MemoryBackend::new());
        let chain = OutcomeChain::new(store);
        let event = sample_event("feature/x", MorphismId::Plan, Phase::Declared, Phase::Planned);
        let entry = chain.append(event.clone()).await.unwrap();
        assert_eq!(entry.sequence, 0);
        assert_eq!(entry.previous_hash, galho_types::Blake3Hash::from([0u8; 32]));
        assert_ne!(entry.entry_hash(), entry.previous_hash);
        assert_eq!(chain.len().await, 1);
        assert_eq!(chain.head_hash().await, entry.entry_hash());
        assert!(chain.verify_integrity().await.unwrap());
    });
}

#[test]
fn append_chains_previous_hash_to_prior_entry_hash() {
    rt().block_on(async {
        let store = Arc::new(MemoryBackend::new());
        let chain = OutcomeChain::new(store);

        let e1 = chain
            .append(sample_event("x", MorphismId::Plan, Phase::Declared, Phase::Planned))
            .await
            .unwrap();
        let e2 = chain
            .append(sample_event(
                "x",
                MorphismId::ApplyToPreview,
                Phase::Planned,
                Phase::AppliedPendingReview,
            ))
            .await
            .unwrap();
        let e3 = chain
            .append(sample_event(
                "x",
                MorphismId::RecordApproval,
                Phase::AppliedPendingReview,
                Phase::ApprovedAwaitingMerge,
            ))
            .await
            .unwrap();

        // Chain invariant: each previous_hash = prior entry_hash.
        assert_eq!(e2.previous_hash, e1.entry_hash());
        assert_eq!(e3.previous_hash, e2.entry_hash());
        // Sequences are monotonic.
        assert_eq!(e1.sequence, 0);
        assert_eq!(e2.sequence, 1);
        assert_eq!(e3.sequence, 2);
        // Verify integrity passes.
        assert!(chain.verify_integrity().await.unwrap());
        assert_eq!(chain.len().await, 3);
    });
}

#[test]
fn entries_returns_chain_in_sequence_order() {
    rt().block_on(async {
        let store = Arc::new(MemoryBackend::new());
        let chain = OutcomeChain::new(store);

        for i in 0..5 {
            let e = sample_event(
                &format!("galho-{i}"),
                MorphismId::Plan,
                Phase::Declared,
                Phase::Planned,
            );
            chain.append(e).await.unwrap();
        }

        let entries = chain.entries().await.unwrap();
        assert_eq!(entries.len(), 5);
        for (i, entry) in entries.iter().enumerate() {
            assert_eq!(entry.sequence, i as u64);
            assert_eq!(entry.event.galho_name, format!("galho-{i}"));
        }
        // Chain invariant holds across the entire run.
        assert!(chain.verify_integrity().await.unwrap());
    });
}

#[test]
fn entry_at_specific_sequence_returns_correct_entry() {
    rt().block_on(async {
        let store = Arc::new(MemoryBackend::new());
        let chain = OutcomeChain::new(store);

        chain
            .append(sample_event("a", MorphismId::Plan, Phase::Declared, Phase::Planned))
            .await
            .unwrap();
        chain
            .append(sample_event("b", MorphismId::Plan, Phase::Declared, Phase::Planned))
            .await
            .unwrap();
        chain
            .append(sample_event("c", MorphismId::Plan, Phase::Declared, Phase::Planned))
            .await
            .unwrap();

        let mid = chain.entry_at(1).await.unwrap().unwrap();
        assert_eq!(mid.event.galho_name, "b");
        assert_eq!(mid.sequence, 1);

        let absent = chain.entry_at(99).await.unwrap();
        assert!(absent.is_none());
    });
}

#[test]
fn chained_outcome_emitter_appends_via_emitter_trait() {
    rt().block_on(async {
        let store = Arc::new(MemoryBackend::new());
        let chain = Arc::new(OutcomeChain::new(store));
        let emitter = ChainedOutcomeEmitter::new(chain.clone());

        let e1 = sample_event("x", MorphismId::Plan, Phase::Declared, Phase::Planned);
        let e2 = sample_event(
            "x",
            MorphismId::ApplyToPreview,
            Phase::Planned,
            Phase::AppliedPendingReview,
        );

        emitter.emit(&e1).unwrap();
        emitter.emit(&e2).unwrap();

        assert_eq!(chain.len().await, 2);
        assert!(chain.verify_integrity().await.unwrap());
        let entries = chain.entries().await.unwrap();
        assert_eq!(entries[0].event.morphism, Some(MorphismId::Plan));
        assert_eq!(entries[1].event.morphism, Some(MorphismId::ApplyToPreview));
        assert_eq!(entries[1].previous_hash, entries[0].entry_hash());
    });
}

#[test]
fn same_event_in_different_positions_gets_different_hashes() {
    // Hash chain ensures that emit-event(E) at seq=0 vs emit-event(E) at seq=5 produces
    // distinct entry hashes even though the embedded event is identical.
    rt().block_on(async {
        let store = Arc::new(MemoryBackend::new());
        let chain = OutcomeChain::new(store);

        let e = sample_event("x", MorphismId::Plan, Phase::Declared, Phase::Planned);
        let first = chain.append(e.clone()).await.unwrap();

        // Fill in some intermediate entries.
        for _ in 0..3 {
            chain
                .append(sample_event(
                    "intermediate",
                    MorphismId::Promote,
                    Phase::ApprovedAwaitingMerge,
                    Phase::Merged,
                ))
                .await
                .unwrap();
        }
        let later = chain.append(e).await.unwrap();

        assert_ne!(first.entry_hash(), later.entry_hash());
        assert_eq!(first.sequence, 0);
        assert_eq!(later.sequence, 4);
    });
}

#[test]
fn restore_from_existing_store_recovers_head_and_sequence() {
    rt().block_on(async {
        let store = Arc::new(MemoryBackend::new());
        {
            let chain = OutcomeChain::new(store.clone());
            for _ in 0..3 {
                chain
                    .append(sample_event(
                        "x",
                        MorphismId::Plan,
                        Phase::Declared,
                        Phase::Planned,
                    ))
                    .await
                    .unwrap();
            }
            assert_eq!(chain.len().await, 3);
        }
        let restored = OutcomeChain::restore(store).await.unwrap();
        assert_eq!(restored.len().await, 3);
        assert!(restored.verify_integrity().await.unwrap());
    });
}
