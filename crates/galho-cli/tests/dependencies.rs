//! Tests for typed per-galho dependencies. `Promote` is gated until every declared
//! dep reaches `Verified` or `Done` — operationally enforces the stacked-PR
//! dependency-ordering invariant carve creates at PR-stack time.

use galho_cli::Runtime;
use galho_types::{MorphismId, Phase};

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// Drive a galho all the way through to `Verified` (one step before Done). Used to
/// satisfy a downstream galho's dependency.
async fn drive_to_verified(r: &Runtime, name: &str, stack_root: &str) {
    r.fire_morphism(name, MorphismId::Plan, None).await.unwrap();
    r.fire_morphism(name, MorphismId::ApplyToPreview, Some(stack_root.into()))
        .await
        .unwrap();
    r.confirm_approval(name, "reviewer").await.unwrap();
    r.fire_morphism(name, MorphismId::RecordApproval, Some("reviewer".into()))
        .await
        .unwrap();
    r.fire_morphism(name, MorphismId::Promote, None).await.unwrap();
    r.fire_morphism(name, MorphismId::Verify, None).await.unwrap();
}

#[test]
fn galho_with_unsatisfied_dep_is_blocked_at_promote() {
    rt().block_on(async {
        let r = Runtime::with_memory();
        // Two galhos: b depends on a.
        r.new_galho("a").await.unwrap();
        r.new_galho_with_deps("b", vec!["a".into()]).await.unwrap();

        // Drive b to ApprovedAwaitingMerge — but a hasn't reached Verified.
        r.fire_morphism("b", MorphismId::Plan, None).await.unwrap();
        r.fire_morphism("b", MorphismId::ApplyToPreview, Some("root".into()))
            .await
            .unwrap();
        r.confirm_approval("b", "reviewer").await.unwrap();
        r.fire_morphism("b", MorphismId::RecordApproval, Some("reviewer".into()))
            .await
            .unwrap();

        // Promote should fail with DependencyNotMet.
        let err = r
            .fire_morphism("b", MorphismId::Promote, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("DependencyNotMet"),
            "expected DependencyNotMet, got: {err}"
        );
        // The unmet list should mention 'a'.
        assert!(err.contains("\"a\""), "expected unmet=[a], got: {err}");
    });
}

#[test]
fn promote_unblocks_after_dep_reaches_verified() {
    rt().block_on(async {
        let r = Runtime::with_memory();
        r.new_galho("a").await.unwrap();
        r.new_galho_with_deps("b", vec!["a".into()]).await.unwrap();

        // Drive a to Verified first.
        drive_to_verified(&r, "a", "stack-1").await;
        assert_eq!(r.status("a").await.unwrap().phase.0, Phase::Verified);

        // Now drive b. Its Promote should succeed because a is Verified.
        r.fire_morphism("b", MorphismId::Plan, None).await.unwrap();
        r.fire_morphism("b", MorphismId::ApplyToPreview, Some("stack-1".into()))
            .await
            .unwrap();
        r.confirm_approval("b", "reviewer").await.unwrap();
        r.fire_morphism("b", MorphismId::RecordApproval, Some("reviewer".into()))
            .await
            .unwrap();
        let p = r.fire_morphism("b", MorphismId::Promote, None).await.unwrap();
        assert_eq!(p, Phase::Merged);
    });
}

#[test]
fn dependency_satisfaction_propagates_across_phase_changes() {
    rt().block_on(async {
        let r = Runtime::with_memory();
        r.new_galho("a").await.unwrap();
        r.new_galho_with_deps("b", vec!["a".into()]).await.unwrap();

        // Build b up to ApprovedAwaitingMerge.
        r.fire_morphism("b", MorphismId::Plan, None).await.unwrap();
        r.fire_morphism("b", MorphismId::ApplyToPreview, Some("root".into()))
            .await
            .unwrap();
        r.confirm_approval("b", "reviewer").await.unwrap();
        r.fire_morphism("b", MorphismId::RecordApproval, Some("reviewer".into()))
            .await
            .unwrap();

        // Still blocked.
        assert!(r
            .fire_morphism("b", MorphismId::Promote, None)
            .await
            .is_err());

        // Now drive a forward. The propagate_dep_satisfaction call after a's Verify
        // should mark a as satisfying b's dep.
        drive_to_verified(&r, "a", "root").await;

        // Promote b — should now succeed.
        let p = r.fire_morphism("b", MorphismId::Promote, None).await.unwrap();
        assert_eq!(p, Phase::Merged);
    });
}

#[test]
fn multiple_dependencies_all_must_be_satisfied() {
    rt().block_on(async {
        let r = Runtime::with_memory();
        r.new_galho("a").await.unwrap();
        r.new_galho("b").await.unwrap();
        r.new_galho_with_deps("c", vec!["a".into(), "b".into()])
            .await
            .unwrap();

        // Build c up to ApprovedAwaitingMerge.
        r.fire_morphism("c", MorphismId::Plan, None).await.unwrap();
        r.fire_morphism("c", MorphismId::ApplyToPreview, Some("root".into()))
            .await
            .unwrap();
        r.confirm_approval("c", "reviewer").await.unwrap();
        r.fire_morphism("c", MorphismId::RecordApproval, Some("reviewer".into()))
            .await
            .unwrap();

        // Satisfy a only. c should still be blocked (b is unmet).
        drive_to_verified(&r, "a", "root").await;
        let err = r
            .fire_morphism("c", MorphismId::Promote, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("\"b\""), "expected b in unmet, got: {err}");
        assert!(!err.contains("\"a\""), "a should now be satisfied: {err}");

        // Satisfy b. c can now Promote.
        drive_to_verified(&r, "b", "root").await;
        let p = r.fire_morphism("c", MorphismId::Promote, None).await.unwrap();
        assert_eq!(p, Phase::Merged);
    });
}

#[test]
fn galho_with_no_deps_promotes_freely() {
    rt().block_on(async {
        let r = Runtime::with_memory();
        r.new_galho("solo").await.unwrap();
        r.fire_morphism("solo", MorphismId::Plan, None).await.unwrap();
        r.fire_morphism("solo", MorphismId::ApplyToPreview, Some("root".into()))
            .await
            .unwrap();
        r.confirm_approval("solo", "reviewer").await.unwrap();
        r.fire_morphism("solo", MorphismId::RecordApproval, Some("reviewer".into()))
            .await
            .unwrap();
        let p = r.fire_morphism("solo", MorphismId::Promote, None).await.unwrap();
        assert_eq!(p, Phase::Merged);
    });
}

#[test]
fn missing_dep_galho_blocks_promote() {
    rt().block_on(async {
        let r = Runtime::with_memory();
        // 'b' depends on 'nonexistent' — should never satisfy.
        r.new_galho_with_deps("b", vec!["nonexistent".into()])
            .await
            .unwrap();
        r.fire_morphism("b", MorphismId::Plan, None).await.unwrap();
        r.fire_morphism("b", MorphismId::ApplyToPreview, Some("root".into()))
            .await
            .unwrap();
        r.confirm_approval("b", "reviewer").await.unwrap();
        r.fire_morphism("b", MorphismId::RecordApproval, Some("reviewer".into()))
            .await
            .unwrap();
        let err = r
            .fire_morphism("b", MorphismId::Promote, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("DependencyNotMet"));
        assert!(err.contains("\"nonexistent\""));
    });
}

#[test]
fn done_phase_also_satisfies_dependency() {
    rt().block_on(async {
        let r = Runtime::with_memory();
        r.new_galho("a").await.unwrap();
        r.new_galho_with_deps("b", vec!["a".into()]).await.unwrap();

        // Drive a all the way to Done.
        drive_to_verified(&r, "a", "root").await;
        r.fire_morphism("a", MorphismId::SealDone, None).await.unwrap();
        assert_eq!(r.status("a").await.unwrap().phase.0, Phase::Done);

        // b promotes successfully because a is Done.
        r.fire_morphism("b", MorphismId::Plan, None).await.unwrap();
        r.fire_morphism("b", MorphismId::ApplyToPreview, Some("root".into()))
            .await
            .unwrap();
        r.confirm_approval("b", "reviewer").await.unwrap();
        r.fire_morphism("b", MorphismId::RecordApproval, Some("reviewer".into()))
            .await
            .unwrap();
        let p = r.fire_morphism("b", MorphismId::Promote, None).await.unwrap();
        assert_eq!(p, Phase::Merged);
    });
}
