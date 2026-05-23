//! Integration tests for galho-controller's tick loop.
//!
//! The controller fires Automatic-sync forward morphisms. Non-Automatic syncs
//! (OperatorApproval / ExternalSignal / etc.) require external action — the
//! controller skips them, so the tick is bounded + observable.

use std::sync::Arc;

use engenho_controllers::Controller;
use galho_cli::Runtime;
use galho_controller::GalhoController;
use galho_types::{MorphismId, Phase};

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn tick_advances_galho_at_automatic_sync_phase() {
    rt().block_on(async {
        let runtime = Arc::new(Runtime::with_memory());
        runtime.new_galho("feature/a").await.unwrap();
        let controller = GalhoController::new(runtime.clone());

        // Declared has Automatic sync; the controller should advance it to Planned.
        let report = controller.tick().await.unwrap();
        assert_eq!(report.objects_examined, 1);
        assert_eq!(report.objects_changed, 1);
        assert_eq!(report.objects_skipped, 0);

        let s = runtime.status("feature/a").await.unwrap();
        assert_eq!(s.phase.0, Phase::Planned);
    });
}

#[test]
fn tick_stops_at_operator_approval_phase() {
    rt().block_on(async {
        let runtime = Arc::new(Runtime::with_memory());
        runtime.new_galho("feature/b").await.unwrap();
        let controller = GalhoController::new(runtime.clone());

        // Manually advance to AppliedPendingReview (Automatic phases skipped by controller).
        runtime
            .fire_morphism("feature/b", MorphismId::Plan, None)
            .await
            .unwrap();
        runtime
            .fire_morphism(
                "feature/b",
                MorphismId::ApplyToPreview,
                Some("root-abc".into()),
            )
            .await
            .unwrap();
        let s = runtime.status("feature/b").await.unwrap();
        assert_eq!(s.phase.0, Phase::AppliedPendingReview);

        // The controller should NOT advance — AppliedPendingReview has OperatorApproval sync.
        let report = controller.tick().await.unwrap();
        assert_eq!(report.objects_examined, 1);
        assert_eq!(report.objects_changed, 0, "controller should not advance past operator-approval gate");
        assert_eq!(report.objects_skipped, 1);

        let s = runtime.status("feature/b").await.unwrap();
        assert_eq!(s.phase.0, Phase::AppliedPendingReview);
    });
}

#[test]
fn tick_handles_multiple_galhos_in_one_pass() {
    rt().block_on(async {
        let runtime = Arc::new(Runtime::with_memory());
        runtime.new_galho("feature/a").await.unwrap();
        runtime.new_galho("feature/b").await.unwrap();
        runtime.new_galho("feature/c").await.unwrap();

        // Pre-advance feature/c to AppliedPendingReview so it's at a non-Automatic gate.
        runtime.fire_morphism("feature/c", MorphismId::Plan, None).await.unwrap();
        runtime
            .fire_morphism("feature/c", MorphismId::ApplyToPreview, Some("root".into()))
            .await
            .unwrap();

        let controller = GalhoController::new(runtime.clone());
        let report = controller.tick().await.unwrap();

        assert_eq!(report.objects_examined, 3);
        // Three are at Declared after init; controller advances them all to Planned (Automatic).
        // BUT — feature/c is at AppliedPendingReview after our pre-advance, so it's skipped.
        // feature/a and feature/b are at Declared → advance to Planned.
        assert_eq!(report.objects_changed, 2);
        assert_eq!(report.objects_skipped, 1);
    });
}

#[test]
fn tick_skips_terminal_galhos() {
    rt().block_on(async {
        let runtime = Arc::new(Runtime::with_memory());
        runtime.new_galho("feature/done").await.unwrap();

        // Walk all the way to Done.
        runtime.fire_morphism("feature/done", MorphismId::Plan, None).await.unwrap();
        runtime
            .fire_morphism("feature/done", MorphismId::ApplyToPreview, Some("r".into()))
            .await
            .unwrap();
        runtime
            .confirm_approval("feature/done", "reviewer")
            .await
            .unwrap();
        runtime
            .fire_morphism("feature/done", MorphismId::RecordApproval, Some("reviewer".into()))
            .await
            .unwrap();
        runtime.fire_morphism("feature/done", MorphismId::Promote, None).await.unwrap();
        runtime.fire_morphism("feature/done", MorphismId::Verify, None).await.unwrap();
        runtime.fire_morphism("feature/done", MorphismId::SealDone, None).await.unwrap();

        let s = runtime.status("feature/done").await.unwrap();
        assert_eq!(s.phase.0, Phase::Done);

        let controller = GalhoController::new(runtime.clone());
        let report = controller.tick().await.unwrap();
        assert_eq!(report.objects_examined, 1);
        assert_eq!(report.objects_changed, 0);
        assert_eq!(report.objects_skipped, 1);
    });
}

#[test]
fn repeated_ticks_drive_galho_through_automatic_phases() {
    rt().block_on(async {
        let runtime = Arc::new(Runtime::with_memory());
        runtime.new_galho("feature/loop").await.unwrap();
        let controller = GalhoController::new(runtime.clone());

        // Each tick advances exactly one Automatic-sync phase.
        // Declared → Planned (Automatic sync at Declared)
        let r1 = controller.tick().await.unwrap();
        assert_eq!(r1.objects_changed, 1);
        let s = runtime.status("feature/loop").await.unwrap();
        assert_eq!(s.phase.0, Phase::Planned);

        // Planned has Automatic sync too; next tick advances to... wait, ApplyToPreview
        // requires --stack-root which the controller doesn't supply. Controller skips.
        let r2 = controller.tick().await.unwrap();
        assert_eq!(
            r2.objects_changed, 0,
            "ApplyToPreview requires stack_root which controller doesn't supply"
        );
        assert_eq!(r2.objects_skipped, 1);
    });
}

#[test]
fn controller_name_is_galho() {
    let runtime = Arc::new(Runtime::with_memory());
    let controller = GalhoController::new(runtime);
    assert_eq!(controller.name(), "galho");
}
