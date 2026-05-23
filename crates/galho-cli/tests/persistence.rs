//! Tests for Runtime persistence: checkpoint → drop → restore_from preserves
//! every galho's typed state (phase, confirmations, flags) + every active stack lock.
//!
//! Closes the cross-session operational gap. Without this, every Ctrl-C wipes the
//! galho world; with this, the operator can resume tomorrow exactly where they
//! stopped today.

use std::sync::Arc;

use galho_cli::{Runtime, RuntimeBackend};
use galho_storage::{backends::MemoryBackend, ObjectStore};
use galho_types::{MorphismId, NoopOutcomeEmitter, OutcomeEmitter, Phase};

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn fresh_backend() -> (RuntimeBackend, Arc<MemoryBackend>) {
    let s = Arc::new(MemoryBackend::new());
    (RuntimeBackend::Memory(s.clone()), s)
}

#[test]
fn checkpoint_then_restore_preserves_single_galho_phase() {
    rt().block_on(async {
        let (backend, store) = fresh_backend();
        let r1 = Runtime::with_emitter(backend, Arc::new(NoopOutcomeEmitter));
        r1.new_galho("g").await.unwrap();
        r1.fire_morphism("g", MorphismId::Plan, None).await.unwrap();
        r1.checkpoint().await.unwrap();
        drop(r1);

        let emitter: Arc<dyn OutcomeEmitter> = Arc::new(NoopOutcomeEmitter);
        let r2 = Runtime::restore_from(RuntimeBackend::Memory(store), emitter)
            .await
            .unwrap();
        let s = r2.status("g").await.unwrap();
        assert_eq!(s.phase.0, Phase::Planned);
    });
}

#[test]
fn checkpoint_then_restore_preserves_confirmations() {
    rt().block_on(async {
        let (backend, store) = fresh_backend();
        let r1 = Runtime::with_emitter(backend, Arc::new(NoopOutcomeEmitter));
        r1.new_galho("g").await.unwrap();
        r1.fire_morphism("g", MorphismId::Plan, None).await.unwrap();
        r1.fire_morphism("g", MorphismId::ApplyToPreview, Some("root".into()))
            .await
            .unwrap();
        // Record a confirmation pre-checkpoint.
        let outcome = r1.confirm_approval("g", "reviewer").await.unwrap();
        assert!(outcome.quorum_reached);

        r1.checkpoint().await.unwrap();
        drop(r1);

        // Restore. The confirmation should be preserved — RecordApproval should fire
        // immediately without a second confirm_approval call.
        let emitter: Arc<dyn OutcomeEmitter> = Arc::new(NoopOutcomeEmitter);
        let r2 = Runtime::restore_from(RuntimeBackend::Memory(store), emitter)
            .await
            .unwrap();
        let next = r2
            .fire_morphism("g", MorphismId::RecordApproval, Some("reviewer".into()))
            .await
            .unwrap();
        assert_eq!(next, Phase::ApprovedAwaitingMerge);
    });
}

#[test]
fn checkpoint_then_restore_preserves_stack_lock_holders() {
    rt().block_on(async {
        let (backend, store) = fresh_backend();
        let r1 = Runtime::with_emitter(backend, Arc::new(NoopOutcomeEmitter));
        r1.new_galho("a").await.unwrap();
        r1.new_galho("b").await.unwrap();
        r1.fire_morphism("a", MorphismId::Plan, None).await.unwrap();
        r1.fire_morphism("b", MorphismId::Plan, None).await.unwrap();
        r1.fire_morphism("a", MorphismId::ApplyToPreview, Some("shared".into()))
            .await
            .unwrap();
        r1.fire_morphism("b", MorphismId::ApplyToPreview, Some("shared".into()))
            .await
            .unwrap();
        r1.checkpoint().await.unwrap();
        drop(r1);

        let emitter: Arc<dyn OutcomeEmitter> = Arc::new(NoopOutcomeEmitter);
        let r2 = Runtime::restore_from(RuntimeBackend::Memory(store), emitter)
            .await
            .unwrap();
        let sa = r2.status("a").await.unwrap();
        let sb = r2.status("b").await.unwrap();
        assert_eq!(sa.stack_lock_root.as_deref(), Some("shared"));
        assert_eq!(sb.stack_lock_root.as_deref(), Some("shared"));
        assert_eq!(sa.stack_lock_holders, 2);
        assert_eq!(sb.stack_lock_holders, 2);
    });
}

#[test]
fn restore_from_empty_store_yields_empty_runtime() {
    rt().block_on(async {
        let (backend, _store) = fresh_backend();
        let emitter: Arc<dyn OutcomeEmitter> = Arc::new(NoopOutcomeEmitter);
        let r = Runtime::restore_from(backend, emitter).await.unwrap();
        assert!(r.list_galhos().await.is_empty());
    });
}

#[test]
fn checkpoint_is_idempotent_across_multiple_calls() {
    rt().block_on(async {
        let (backend, store) = fresh_backend();
        let r = Runtime::with_emitter(backend, Arc::new(NoopOutcomeEmitter));
        r.new_galho("g").await.unwrap();
        r.fire_morphism("g", MorphismId::Plan, None).await.unwrap();

        let count_before = store.list_objects().await.unwrap().len();
        r.checkpoint().await.unwrap();
        let count_after_first = store.list_objects().await.unwrap().len();
        r.checkpoint().await.unwrap();
        let count_after_second = store.list_objects().await.unwrap().len();

        // Content-addressed storage means a second identical checkpoint adds no new
        // objects (idempotent at the storage layer).
        assert!(count_after_first > count_before);
        assert_eq!(count_after_first, count_after_second);
    });
}

#[test]
fn full_arc_then_restore_can_continue_from_done() {
    rt().block_on(async {
        let (backend, store) = fresh_backend();
        let r1 = Runtime::with_emitter(backend, Arc::new(NoopOutcomeEmitter));
        r1.new_galho("g").await.unwrap();
        r1.fire_morphism("g", MorphismId::Plan, None).await.unwrap();
        r1.fire_morphism("g", MorphismId::ApplyToPreview, Some("root".into()))
            .await
            .unwrap();
        r1.confirm_approval("g", "reviewer").await.unwrap();
        r1.fire_morphism("g", MorphismId::RecordApproval, Some("reviewer".into()))
            .await
            .unwrap();
        r1.fire_morphism("g", MorphismId::Promote, None).await.unwrap();
        r1.fire_morphism("g", MorphismId::Verify, None).await.unwrap();
        r1.fire_morphism("g", MorphismId::SealDone, None).await.unwrap();
        r1.checkpoint().await.unwrap();
        drop(r1);

        let emitter: Arc<dyn OutcomeEmitter> = Arc::new(NoopOutcomeEmitter);
        let r2 = Runtime::restore_from(RuntimeBackend::Memory(store), emitter)
            .await
            .unwrap();
        let s = r2.status("g").await.unwrap();
        assert_eq!(s.phase.0, Phase::Done);
        // No forward morphisms from Done.
        assert!(s.forward.is_empty());
    });
}
