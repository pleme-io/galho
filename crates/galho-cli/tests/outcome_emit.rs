//! Integration tests proving every Runtime state transition emits a typed OutcomeEvent.
//!
//! Uses `MemoryOutcomeEmitter` to capture events; asserts on event kind, count, ordering,
//! and embedded context (from/to phase, morphism, stack-root).

use std::sync::Arc;

use galho_cli::{Runtime, RuntimeBackend};
use galho_storage::backends::MemoryBackend;
use galho_types::{
    MemoryOutcomeEmitter, MorphismId, OutcomeEmitter, OutcomeEventType, Phase, StackRoot,
};

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn make_runtime() -> (Runtime, Arc<MemoryOutcomeEmitter>) {
    let emitter = Arc::new(MemoryOutcomeEmitter::new());
    let emitter_dyn: Arc<dyn OutcomeEmitter> = emitter.clone();
    let backend = RuntimeBackend::Memory(Arc::new(MemoryBackend::new()));
    (Runtime::with_emitter(backend, emitter_dyn), emitter)
}

#[test]
fn new_galho_emits_created_event() {
    rt().block_on(async {
        let (r, em) = make_runtime();
        r.new_galho("feature/x").await.unwrap();
        let snap = em.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].event_type, OutcomeEventType::GalhoCreated);
        assert_eq!(snap[0].galho_name, "feature/x");
        assert_eq!(snap[0].from_phase, Some(Phase::Declared));
    });
}

#[test]
fn each_morphism_emits_one_morphism_fired_event() {
    rt().block_on(async {
        let (r, em) = make_runtime();
        r.new_galho("feature/x").await.unwrap();
        r.fire_morphism("feature/x", MorphismId::Plan, None).await.unwrap();
        // Created + Plan-fired = 2 events.
        assert_eq!(em.count_of(OutcomeEventType::GalhoCreated), 1);
        assert_eq!(em.count_of(OutcomeEventType::MorphismFired), 1);

        let plan_event = em
            .snapshot()
            .into_iter()
            .find(|e| e.event_type == OutcomeEventType::MorphismFired)
            .unwrap();
        assert_eq!(plan_event.morphism, Some(MorphismId::Plan));
        assert_eq!(plan_event.from_phase, Some(Phase::Declared));
        assert_eq!(plan_event.to_phase, Some(Phase::Planned));
    });
}

#[test]
fn stack_lock_acquire_and_join_emit_distinct_events() {
    rt().block_on(async {
        let (r, em) = make_runtime();
        r.new_galho("feature/a").await.unwrap();
        r.new_galho("feature/b").await.unwrap();

        r.fire_morphism("feature/a", MorphismId::Plan, None).await.unwrap();
        r.fire_morphism("feature/b", MorphismId::Plan, None).await.unwrap();

        // First Apply acquires; second joins.
        r.fire_morphism("feature/a", MorphismId::ApplyToPreview, Some("root-1".into()))
            .await
            .unwrap();
        r.fire_morphism("feature/b", MorphismId::ApplyToPreview, Some("root-1".into()))
            .await
            .unwrap();

        assert_eq!(em.count_of(OutcomeEventType::StackLockAcquired), 1);
        assert_eq!(em.count_of(OutcomeEventType::StackLockJoined), 1);

        let acquired = em
            .snapshot()
            .into_iter()
            .find(|e| e.event_type == OutcomeEventType::StackLockAcquired)
            .unwrap();
        assert_eq!(acquired.galho_name, "feature/a");
        assert_eq!(acquired.stack_lock_root, Some(StackRoot::new("root-1")));

        let joined = em
            .snapshot()
            .into_iter()
            .find(|e| e.event_type == OutcomeEventType::StackLockJoined)
            .unwrap();
        assert_eq!(joined.galho_name, "feature/b");
    });
}

#[test]
fn destroyed_phase_emits_dedicated_event() {
    rt().block_on(async {
        let (r, em) = make_runtime();
        r.new_galho("feature/abandoned").await.unwrap();
        r.fire_morphism("feature/abandoned", MorphismId::Plan, None).await.unwrap();
        r.fire_morphism("feature/abandoned", MorphismId::Abandon, None).await.unwrap();

        // Abandon → Destroyed produces BOTH a MorphismFired and a GalhoDestroyed event.
        assert_eq!(em.count_of(OutcomeEventType::GalhoDestroyed), 1);
        let destroyed = em
            .snapshot()
            .into_iter()
            .find(|e| e.event_type == OutcomeEventType::GalhoDestroyed)
            .unwrap();
        assert_eq!(destroyed.galho_name, "feature/abandoned");
        assert_eq!(destroyed.to_phase, Some(Phase::Destroyed));
    });
}

#[test]
fn blocked_morphism_does_not_emit_morphism_fired() {
    rt().block_on(async {
        let (r, em) = make_runtime();
        r.new_galho("feature/blocked").await.unwrap();
        // ApplyToPreview from Declared (wrong phase) → preconditions fail.
        let result = r
            .fire_morphism("feature/blocked", MorphismId::ApplyToPreview, Some("root".into()))
            .await;
        assert!(result.is_err());
        // GalhoCreated emitted; no MorphismFired since the morphism didn't fire.
        // Stack lock acquire ALSO happens before the precondition check (the pre-action
        // block fires it for the side-effect); document that here.
        assert_eq!(em.count_of(OutcomeEventType::MorphismFired), 0);
    });
}

#[test]
fn full_forward_arc_emits_exactly_n_morphism_events() {
    rt().block_on(async {
        let (r, em) = make_runtime();
        r.new_galho("feature/done").await.unwrap();
        r.fire_morphism("feature/done", MorphismId::Plan, None).await.unwrap();
        r.fire_morphism("feature/done", MorphismId::ApplyToPreview, Some("r".into()))
            .await.unwrap();
        r.confirm_approval("feature/done", "reviewer").await.unwrap();
        r.fire_morphism("feature/done", MorphismId::RecordApproval, Some("rev".into()))
            .await.unwrap();
        r.fire_morphism("feature/done", MorphismId::Promote, None).await.unwrap();
        r.fire_morphism("feature/done", MorphismId::Verify, None).await.unwrap();
        r.fire_morphism("feature/done", MorphismId::SealDone, None).await.unwrap();

        // 6 forward morphisms fired.
        assert_eq!(em.count_of(OutcomeEventType::MorphismFired), 6);
        // Plus 1 created + 1 stack-acquired + 1 sync-confirmed = 9 total non-destroyed events.
        assert_eq!(em.count_of(OutcomeEventType::GalhoCreated), 1);
        assert_eq!(em.count_of(OutcomeEventType::StackLockAcquired), 1);
        assert_eq!(em.count_of(OutcomeEventType::SyncConfirmed), 1);
        // No destroyed event (Done is terminal but not Destroyed).
        assert_eq!(em.count_of(OutcomeEventType::GalhoDestroyed), 0);
    });
}

#[test]
fn outcome_events_serialize_to_json_and_back() {
    rt().block_on(async {
        let (r, em) = make_runtime();
        r.new_galho("feature/serde").await.unwrap();
        r.fire_morphism("feature/serde", MorphismId::Plan, None).await.unwrap();
        let snap = em.snapshot();
        for event in &snap {
            let json = serde_json::to_string(event).unwrap();
            let round: galho_types::OutcomeEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(&round, event);
        }
    });
}

#[test]
fn noop_emitter_drops_events_silently() {
    use galho_types::NoopOutcomeEmitter;
    rt().block_on(async {
        let emitter: Arc<dyn OutcomeEmitter> = Arc::new(NoopOutcomeEmitter);
        let backend = RuntimeBackend::Memory(Arc::new(MemoryBackend::new()));
        let r = Runtime::with_emitter(backend, emitter);
        r.new_galho("x").await.unwrap();
        r.fire_morphism("x", MorphismId::Plan, None).await.unwrap();
        // No way to observe events; just verify it didn't crash.
    });
}
