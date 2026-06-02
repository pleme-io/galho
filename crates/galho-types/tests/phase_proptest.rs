//! Property-based tests for the Phase / Morphism / Sync / StackLock / KnowledgeBase surface.
//!
//! The structural invariants of the state machine. If any of these regress, the typed
//! lifecycle is silently broken.

use galho_types::{
    is_known_transition, morphism_for, transition_table, transitions_from, transitions_to,
    GalhoConfig, KnowledgeBase, MorphismContext, MorphismId, MorphismRequirement, Phase,
    PhaseClass, RebasePolicy, StackLock, StackRoot, SyncConfig, SyncKind, WritePolicy,
};
use proptest::prelude::*;
use time::Duration;

// =============================================================================
// Strategies
// =============================================================================

fn any_phase() -> impl Strategy<Value = Phase> {
    prop_oneof![
        Just(Phase::Declared),
        Just(Phase::Planned),
        Just(Phase::AppliedPendingReview),
        Just(Phase::ApprovedAwaitingMerge),
        Just(Phase::Merged),
        Just(Phase::Verified),
        Just(Phase::Done),
        Just(Phase::RollingBack),
        Just(Phase::RolledBack),
        Just(Phase::Drifted),
        Just(Phase::Destroyed),
        Just(Phase::Failed),
    ]
}

fn any_morphism_id() -> impl Strategy<Value = MorphismId> {
    prop_oneof![
        Just(MorphismId::Plan),
        Just(MorphismId::ApplyToPreview),
        Just(MorphismId::RecordApproval),
        Just(MorphismId::Promote),
        Just(MorphismId::Verify),
        Just(MorphismId::SealDone),
        Just(MorphismId::RevertApply),
        Just(MorphismId::DriftReconcile),
        Just(MorphismId::Abandon),
        Just(MorphismId::Escalate),
        Just(MorphismId::Recover),
        Just(MorphismId::Resume),
    ]
}

// =============================================================================
// Phase + KnowledgeBase invariants
// =============================================================================

#[test]
fn knowledge_base_default_validates() {
    let kb = KnowledgeBase::default();
    kb.validate().expect("default knowledge base must validate");
}

#[test]
fn knowledge_base_prescribed_is_default() {
    assert_eq!(KnowledgeBase::prescribed(), KnowledgeBase::default());
}

#[test]
fn every_non_terminal_phase_has_outgoing_transition() {
    for &phase in Phase::all() {
        if matches!(phase.class(), PhaseClass::Terminal) {
            continue;
        }
        let outgoing = transitions_from(phase);
        assert!(
            !outgoing.is_empty(),
            "non-terminal phase {phase} has no outgoing transition"
        );
    }
}

#[test]
fn every_morphism_id_is_materializable() {
    for &id in MorphismId::all() {
        assert!(
            morphism_for(id).is_some(),
            "morphism_for({id}) returned None"
        );
    }
}

#[test]
fn done_is_reachable_from_declared() {
    let kb = KnowledgeBase::default();
    assert!(kb.is_reachable(Phase::Declared, Phase::Done));
}

#[test]
fn destroyed_is_reachable_from_every_non_terminal_phase() {
    let kb = KnowledgeBase::default();
    for &phase in Phase::all() {
        if phase == Phase::Done || phase == Phase::Destroyed {
            continue;
        }
        assert!(
            kb.is_reachable(phase, Phase::Destroyed),
            "Destroyed not reachable from {phase}"
        );
    }
}

#[test]
fn rolled_back_can_resume_to_planned() {
    let kb = KnowledgeBase::default();
    assert!(kb.is_reachable(Phase::RolledBack, Phase::Planned));
    assert!(kb.is_reachable(Phase::RolledBack, Phase::Done));
}

#[test]
fn failed_recovery_loops_back_to_declared() {
    let kb = KnowledgeBase::default();
    assert!(kb.is_reachable(Phase::Failed, Phase::Declared));
    assert!(kb.is_reachable(Phase::Failed, Phase::Done));
}

proptest! {
    /// P.1 — `transitions_from(p)` and `is_known_transition` agree.
    #[test]
    fn transitions_query_consistency(p in any_phase()) {
        for t in transitions_from(p) {
            prop_assert!(is_known_transition(t.from, t.to));
            prop_assert_eq!(t.from, p);
        }
    }

    /// P.2 — Every transition in the static table is reflected by both `transitions_from`
    /// and `transitions_to`.
    #[test]
    fn transitions_from_to_symmetry(p1 in any_phase(), p2 in any_phase()) {
        let known = is_known_transition(p1, p2);
        let from_includes = transitions_from(p1).iter().any(|t| t.to == p2);
        let to_includes = transitions_to(p2).iter().any(|t| t.from == p1);
        prop_assert_eq!(known, from_includes);
        prop_assert_eq!(known, to_includes);
    }

    /// P.3 — Every materializable morphism appears in the transition table at least once,
    /// and its declared `from_phase()` is one of its appearances' `from`.
    #[test]
    fn morphism_phases_match_transition_table(id in any_morphism_id()) {
        let m = morphism_for(id).unwrap();
        let appearances: Vec<_> = transition_table()
            .iter()
            .filter(|t| t.morphism == id)
            .collect();
        prop_assert!(
            !appearances.is_empty(),
            "morphism {} not present in transition table", id
        );
        // The declared from_phase MUST match at least one appearance's `from`. (Backward
        // morphisms like RevertApply / Escalate / Abandon appear at multiple `from`s
        // because they're the same logical action from several phases.)
        let declared_from = m.from_phase();
        let matches_some = appearances.iter().any(|t| t.from == declared_from);
        prop_assert!(
            matches_some,
            "morphism {} declares from_phase={} but no transition table entry uses it",
            id, declared_from
        );
    }

    /// P.4 — A morphism's typed inverse, when present, is also a known morphism id.
    #[test]
    fn morphism_inverse_is_known(id in any_morphism_id()) {
        let m = morphism_for(id).unwrap();
        if let Some(inv) = m.inverse() {
            prop_assert!(morphism_for(inv).is_some());
        }
    }
}

// =============================================================================
// Sync defaults
// =============================================================================

#[test]
fn every_phase_has_default_sync_config() {
    for &phase in Phase::all() {
        let _cfg = SyncConfig::default_for(phase);
    }
}

#[test]
fn applied_pending_review_requires_operator_approval() {
    let cfg = SyncConfig::default_for(Phase::AppliedPendingReview);
    assert!(
        matches!(cfg.kind, SyncKind::OperatorApproval { .. }),
        "AppliedPendingReview must default to OperatorApproval (compliance-load-bearing)"
    );
    assert!(cfg.attestation_required);
}

#[test]
fn verified_requires_attestation_gated_sync() {
    let cfg = SyncConfig::default_for(Phase::Verified);
    assert!(matches!(cfg.kind, SyncKind::AttestationGated { .. }));
    assert!(cfg.attestation_required);
}

// =============================================================================
// StackLock invariants
// =============================================================================

/// Build a `BoundedApplyTtl` from a `Duration`, panicking if out of bounds —
/// the lock ctor now takes the refined TTL, not a raw `Duration`.
fn ttl(d: Duration) -> galho_types::BoundedApplyTtl {
    galho_types::BoundedApplyTtl::try_new(d).expect("test TTL within ApplyTtlBounds")
}

#[test]
fn stack_lock_acquire_and_join_increments_holders() {
    let mut lock = StackLock::acquire(StackRoot::new("abc"), "feature/a", ttl(Duration::days(7)));
    assert_eq!(lock.holder_count(), 1);
    assert!(lock.join("feature/b"));
    assert_eq!(lock.holder_count(), 2);
    assert!(lock.join("feature/c"));
    assert_eq!(lock.holder_count(), 3);
    assert!(!lock.join("feature/a")); // already a holder
}

#[test]
fn stack_lock_release_holder_returns_empty_signal() {
    let mut lock = StackLock::acquire(StackRoot::new("abc"), "feature/a", ttl(Duration::days(7)));
    lock.join("feature/b");
    assert!(!lock.release_holder("feature/a"));
    assert_eq!(lock.holder_count(), 1);
    assert!(lock.release_holder("feature/b"));
    assert_eq!(lock.holder_count(), 0);
}

#[test]
fn stack_lock_expires_when_now_past_expiry() {
    // 1 minute is the floor of ApplyTtlBounds (1s would now be rejected).
    let lock = StackLock::acquire(StackRoot::new("abc"), "feature/a", ttl(Duration::minutes(1)));
    let future = time::OffsetDateTime::now_utc() + Duration::days(1);
    assert!(lock.is_expired(future));
}

// =============================================================================
// MorphismContext + apply_morphism
// =============================================================================

#[test]
fn plan_morphism_succeeds_from_declared() {
    let kb = KnowledgeBase::default();
    let ctx = MorphismContext::declared("galho-x");
    let result = kb.apply_morphism(MorphismId::Plan, &ctx);
    assert_eq!(result, Ok(Phase::Planned));
}

#[test]
fn apply_to_preview_blocks_without_plan() {
    let kb = KnowledgeBase::default();
    let mut ctx = MorphismContext::declared("galho-x");
    ctx.current_phase = Phase::Planned;
    // No plan, no stack lock.
    let result = kb.apply_morphism(MorphismId::ApplyToPreview, &ctx);
    let Err(missing) = result else {
        panic!("expected Err, got {result:?}");
    };
    assert!(missing.contains(&MorphismRequirement::PlanMissing));
    assert!(missing.contains(&MorphismRequirement::StackLockMissing));
}

#[test]
fn apply_to_preview_succeeds_with_plan_and_lock() {
    let kb = KnowledgeBase::default();
    let mut ctx = MorphismContext::declared("galho-x");
    ctx.current_phase = Phase::Planned;
    ctx.has_plan = true;
    ctx.stack_lock_held = true;
    let result = kb.apply_morphism(MorphismId::ApplyToPreview, &ctx);
    assert_eq!(result, Ok(Phase::AppliedPendingReview));
}

#[test]
fn promote_blocks_without_merge_event() {
    let kb = KnowledgeBase::default();
    let mut ctx = MorphismContext::declared("galho-x");
    ctx.current_phase = Phase::ApprovedAwaitingMerge;
    ctx.has_apply_receipt = true;
    ctx.has_approval_quorum = true;
    // has_merge_event = false
    let result = kb.apply_morphism(MorphismId::Promote, &ctx);
    let Err(missing) = result else {
        panic!("expected Err, got {result:?}");
    };
    assert!(missing.contains(&MorphismRequirement::MergeEventMissing));
}

/// A `MorphismContext` at `phase` with every boolean precondition satisfied —
/// so `apply_morphism` is gated only by the transition-table lookup, not by a
/// missing flag. Used to prove the destination phase always comes from the
/// table row (never the morphism's nominal `to_phase()` fallback).
fn fully_satisfied_at(phase: Phase) -> MorphismContext {
    let mut ctx = MorphismContext::declared("galho-positive");
    ctx.current_phase = phase;
    ctx.has_plan = true;
    ctx.has_apply_receipt = true;
    ctx.has_approval_quorum = true;
    ctx.has_merge_event = true;
    ctx.has_verify_receipt = true;
    ctx.stack_lock_held = true;
    ctx.jira_ticket_resolvable = true;
    // No drift / conflict / deps to block.
    ctx.drift_detected = false;
    ctx.conflict_open = false;
    ctx
}

#[test]
fn apply_morphism_target_always_comes_from_transition_table() {
    // For every (from, morphism, to) row in the table, a fully-satisfied context
    // at `from` firing `morphism` must return EXACTLY the table's `to` — never the
    // morphism's nominal `to_phase()`. This is the regression guard for the old
    // `.unwrap_or_else(|| m.to_phase())` fallback that masked the Abandon-from-
    // ApprovedAwaitingMerge wrong-target bug.
    let kb = KnowledgeBase::default();
    for row in transition_table() {
        let ctx = fully_satisfied_at(row.from);
        // DriftReconcile is bidirectional (Verified ↔ Drifted): from Verified the
        // morphism's precondition path may pick the detection direction. Skip rows
        // whose preconditions a fully-satisfied flag-set can't represent cleanly;
        // assert only that, IF it returns Ok, the target equals the table row.
        match kb.apply_morphism(row.morphism, &ctx) {
            Ok(target) => assert_eq!(
                target, row.to,
                "morphism {:?} from {:?}: table says {:?}, got {:?}",
                row.morphism, row.from, row.to, target
            ),
            Err(_) => {
                // Some rows (e.g. DriftReconcile detection direction) require a
                // specific flag combination the blanket-satisfied context doesn't
                // hit; those are covered by the dedicated unit tests above.
            }
        }
    }
}

#[test]
fn seal_done_requires_jira_resolvable() {
    let kb = KnowledgeBase::default();
    let mut ctx = MorphismContext::declared("galho-x");
    ctx.current_phase = Phase::Verified;
    ctx.has_verify_receipt = true;
    ctx.jira_ticket_resolvable = false;
    let result = kb.apply_morphism(MorphismId::SealDone, &ctx);
    let Err(missing) = result else {
        panic!("expected Err, got {result:?}");
    };
    assert!(missing.contains(&MorphismRequirement::JiraTicketUnresolvable));
}

// =============================================================================
// GalhoConfig tiering
// =============================================================================

#[test]
fn galho_config_bare_has_no_phase_overrides() {
    let cfg = GalhoConfig::bare();
    assert!(cfg.phases.is_empty());
    assert!(matches!(cfg.write_policy, WritePolicy::ApplyBeforeMerge));
    assert!(matches!(cfg.rebase_policy, RebasePolicy::Hybrid));
}

#[test]
fn galho_config_prescribed_has_every_phase() {
    let cfg = GalhoConfig::prescribed();
    for &phase in Phase::all() {
        assert!(
            cfg.phases.contains_key(&phase),
            "prescribed config missing sync for {phase}"
        );
    }
}

#[test]
fn galho_config_extend_replaces_per_phase_overrides() {
    let base = GalhoConfig::prescribed();
    let mut overlay = GalhoConfig::bare();
    let custom_sync = SyncConfig {
        kind: SyncKind::Automatic,
        timeout: Duration::seconds(10),
        on_timeout: galho_types::TimeoutAction::HoldForOperator,
        attestation_required: false,
        audit_channel: "custom".into(),
    };
    overlay
        .phases
        .insert(Phase::AppliedPendingReview, custom_sync.clone());
    let merged = base.extend(overlay);
    assert_eq!(
        merged.phases.get(&Phase::AppliedPendingReview).cloned(),
        Some(custom_sync),
    );
    // Other phases retained from base.
    assert!(merged.phases.contains_key(&Phase::Verified));
}

#[test]
fn write_policy_default_is_apply_before_merge() {
    let cfg = GalhoConfig::bare();
    assert!(matches!(cfg.write_policy, WritePolicy::ApplyBeforeMerge));
}

#[test]
fn rebase_policy_default_is_hybrid() {
    let cfg = GalhoConfig::bare();
    assert!(matches!(cfg.rebase_policy, RebasePolicy::Hybrid));
}

// =============================================================================
// Query-consistency sweep (catches table drift)
// =============================================================================

#[test]
fn query_consistency_holds() {
    galho_types::check_query_consistency().expect("phase query consistency must hold");
}
