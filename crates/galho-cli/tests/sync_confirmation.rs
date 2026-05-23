//! Tests for the typed Sync confirmation flow.
//!
//! `OperatorApproval { roles, quorum }` syncs are now operational: `Runtime::confirm_approval`
//! validates role, records confirmation, and sets `has_approval_quorum` only when the typed
//! quorum is met. Pre-quorum `fire_morphism(RecordApproval, _)` calls FAIL with `ApprovalQuorumMissing`.

use galho_cli::Runtime;
use galho_types::{MorphismId, Phase};

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn confirm_approval_records_role_and_returns_outcome() {
    rt().block_on(async {
        let r = Runtime::with_memory();
        r.new_galho("g").await.unwrap();
        r.fire_morphism("g", MorphismId::Plan, None).await.unwrap();
        r.fire_morphism("g", MorphismId::ApplyToPreview, Some("root".into()))
            .await
            .unwrap();

        // Default sync at AppliedPendingReview is OperatorApproval { roles, quorum: 1 }.
        let outcome = r.confirm_approval("g", "reviewer").await.unwrap();
        assert!(outcome.quorum_reached);
        assert_eq!(outcome.count, 1);
        assert_eq!(outcome.quorum, 1);
        assert_eq!(outcome.phase, Phase::AppliedPendingReview);
    });
}

#[test]
fn pre_quorum_record_approval_fails_with_approval_quorum_missing() {
    rt().block_on(async {
        let r = Runtime::with_memory();
        r.new_galho("g").await.unwrap();
        r.fire_morphism("g", MorphismId::Plan, None).await.unwrap();
        r.fire_morphism("g", MorphismId::ApplyToPreview, Some("root".into()))
            .await
            .unwrap();
        // No confirm_approval — quorum not met.
        let err = r
            .fire_morphism("g", MorphismId::RecordApproval, Some("reviewer".into()))
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("ApprovalQuorumMissing"),
            "got: {err}"
        );
    });
}

#[test]
fn confirm_approval_then_record_approval_advances() {
    rt().block_on(async {
        let r = Runtime::with_memory();
        r.new_galho("g").await.unwrap();
        r.fire_morphism("g", MorphismId::Plan, None).await.unwrap();
        r.fire_morphism("g", MorphismId::ApplyToPreview, Some("root".into()))
            .await
            .unwrap();

        let outcome = r.confirm_approval("g", "reviewer").await.unwrap();
        assert!(outcome.quorum_reached);

        let next = r
            .fire_morphism("g", MorphismId::RecordApproval, Some("reviewer".into()))
            .await
            .unwrap();
        assert_eq!(next, Phase::ApprovedAwaitingMerge);
    });
}

#[test]
fn unauthorized_role_rejected() {
    rt().block_on(async {
        let r = Runtime::with_memory();
        r.new_galho("g").await.unwrap();
        r.fire_morphism("g", MorphismId::Plan, None).await.unwrap();
        r.fire_morphism("g", MorphismId::ApplyToPreview, Some("root".into()))
            .await
            .unwrap();

        let err = r
            .confirm_approval("g", "random-stranger")
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("not authorized"),
            "got: {err}"
        );
    });
}

#[test]
fn confirm_approval_outside_operator_approval_phase_rejected() {
    rt().block_on(async {
        let r = Runtime::with_memory();
        r.new_galho("g").await.unwrap();
        // Still at Declared — Automatic sync, not OperatorApproval.
        let err = r
            .confirm_approval("g", "reviewer")
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("not OperatorApproval"),
            "got: {err}"
        );
    });
}

#[test]
fn idempotent_confirmation_does_not_double_count() {
    rt().block_on(async {
        let r = Runtime::with_memory();
        r.new_galho("g").await.unwrap();
        r.fire_morphism("g", MorphismId::Plan, None).await.unwrap();
        r.fire_morphism("g", MorphismId::ApplyToPreview, Some("root".into()))
            .await
            .unwrap();

        // Same role twice — BTreeSet dedupes, count stays at 1.
        let o1 = r.confirm_approval("g", "reviewer").await.unwrap();
        let o2 = r.confirm_approval("g", "reviewer").await.unwrap();
        assert_eq!(o1.count, 1);
        assert_eq!(o2.count, 1);
    });
}
