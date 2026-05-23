//! Integration test: walk a galho through the full forward arc Declared → Done.
//!
//! Exercises Runtime::new_galho + Runtime::fire_morphism for every forward morphism
//! and asserts that (a) preconditions surface when missing, (b) phases advance
//! correctly when satisfied, (c) stack-lock acquire / join holds across the arc,
//! (d) backward morphisms recover state correctly.

use galho_cli::Runtime;
use galho_types::{MorphismId, Phase};

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn full_forward_arc_declared_to_done() {
    rt().block_on(async {
        let r = Runtime::with_memory();
        r.new_galho("feature/test").await.unwrap();

        // Declared
        let s = r.status("feature/test").await.unwrap();
        assert_eq!(s.phase.0, Phase::Declared);
        assert!(s.forward.contains(&MorphismId::Plan));

        // Plan → Planned
        let p = r.fire_morphism("feature/test", MorphismId::Plan, None).await.unwrap();
        assert_eq!(p, Phase::Planned);

        // ApplyToPreview without stack_root → preconditions block.
        let err = r
            .fire_morphism("feature/test", MorphismId::ApplyToPreview, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("--stack-root required"), "got: {err}");

        // ApplyToPreview with stack_root → AppliedPendingReview.
        let p = r
            .fire_morphism(
                "feature/test",
                MorphismId::ApplyToPreview,
                Some("abc123".into()),
            )
            .await
            .unwrap();
        assert_eq!(p, Phase::AppliedPendingReview);
        let s = r.status("feature/test").await.unwrap();
        assert_eq!(s.stack_lock_root.as_deref(), Some("abc123"));
        assert_eq!(s.stack_lock_holders, 1);

        // Confirm via typed OperatorApproval sync (default quorum = 1 at this phase).
        let outcome = r.confirm_approval("feature/test", "reviewer").await.unwrap();
        assert!(outcome.quorum_reached);

        // RecordApproval → ApprovedAwaitingMerge (now that quorum is met).
        let p = r
            .fire_morphism(
                "feature/test",
                MorphismId::RecordApproval,
                Some("reviewer".into()),
            )
            .await
            .unwrap();
        assert_eq!(p, Phase::ApprovedAwaitingMerge);

        // Promote → Merged.
        let p = r
            .fire_morphism("feature/test", MorphismId::Promote, None)
            .await
            .unwrap();
        assert_eq!(p, Phase::Merged);

        // Verify → Verified.
        let p = r
            .fire_morphism("feature/test", MorphismId::Verify, None)
            .await
            .unwrap();
        assert_eq!(p, Phase::Verified);

        // SealDone → Done.
        let p = r
            .fire_morphism("feature/test", MorphismId::SealDone, None)
            .await
            .unwrap();
        assert_eq!(p, Phase::Done);

        let s = r.status("feature/test").await.unwrap();
        assert!(s.forward.is_empty(), "Done is terminal; no forward morphisms");
    });
}

#[test]
fn rollback_arc_revert_apply_and_resume() {
    rt().block_on(async {
        let r = Runtime::with_memory();
        r.new_galho("feature/rollback").await.unwrap();
        r.fire_morphism("feature/rollback", MorphismId::Plan, None).await.unwrap();
        r.fire_morphism(
            "feature/rollback",
            MorphismId::ApplyToPreview,
            Some("def456".into()),
        )
        .await
        .unwrap();

        // RevertApply: AppliedPendingReview → RollingBack.
        let p = r
            .fire_morphism("feature/rollback", MorphismId::RevertApply, None)
            .await
            .unwrap();
        assert_eq!(p, Phase::RollingBack);

        // Another RevertApply moves RollingBack → RolledBack.
        let p = r
            .fire_morphism("feature/rollback", MorphismId::RevertApply, None)
            .await
            .unwrap();
        assert_eq!(p, Phase::RolledBack);

        // Resume: RolledBack → Planned. Plan flag is still set from the earlier Plan
        // morphism (RevertApply doesn't clear it — only the apply receipt + lock).
        let p = r
            .fire_morphism("feature/rollback", MorphismId::Resume, None)
            .await
            .unwrap();
        assert_eq!(p, Phase::Planned);
    });
}

#[test]
fn abandon_from_any_non_terminal_destroys() {
    rt().block_on(async {
        let r = Runtime::with_memory();
        r.new_galho("feature/abandon").await.unwrap();
        r.fire_morphism("feature/abandon", MorphismId::Plan, None).await.unwrap();

        let p = r
            .fire_morphism("feature/abandon", MorphismId::Abandon, None)
            .await
            .unwrap();
        assert_eq!(p, Phase::Destroyed);
    });
}

#[test]
fn stack_lock_joins_when_two_galhos_share_root() {
    rt().block_on(async {
        let r = Runtime::with_memory();
        r.new_galho("feature/a").await.unwrap();
        r.new_galho("feature/b").await.unwrap();

        r.fire_morphism("feature/a", MorphismId::Plan, None).await.unwrap();
        r.fire_morphism("feature/b", MorphismId::Plan, None).await.unwrap();

        r.fire_morphism("feature/a", MorphismId::ApplyToPreview, Some("shared-root".into()))
            .await
            .unwrap();
        r.fire_morphism("feature/b", MorphismId::ApplyToPreview, Some("shared-root".into()))
            .await
            .unwrap();

        let sa = r.status("feature/a").await.unwrap();
        let sb = r.status("feature/b").await.unwrap();

        assert_eq!(sa.stack_lock_root.as_deref(), Some("shared-root"));
        assert_eq!(sb.stack_lock_root.as_deref(), Some("shared-root"));
        assert_eq!(sa.stack_lock_holders, 2);
        assert_eq!(sb.stack_lock_holders, 2);
    });
}
