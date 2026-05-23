//! Tests for the typed compliance verifier.
//!
//! Builds an `OutcomeChain` with deliberate compliant + non-compliant sequences,
//! verifies against `ComplianceBaseline::prescribed(regime)` for each regime, and
//! asserts the report matches expected status + violations.

use std::sync::Arc;

use galho_storage::{backends::MemoryBackend, verify, verify_regime, OutcomeChain};
use galho_types::{
    ComplianceBaseline, ComplianceRegime, ComplianceRule, ComplianceStatus, MorphismId,
    OutcomeEvent, OutcomeEventType, Passaporte, Phase,
};

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn event(kind: OutcomeEventType, galho: &str) -> OutcomeEvent {
    OutcomeEvent::new(kind, galho).with_actor(Passaporte::new("test"))
}

fn morphism_event(galho: &str, m: MorphismId, from: Phase, to: Phase) -> OutcomeEvent {
    event(OutcomeEventType::MorphismFired, galho)
        .with_morphism(m)
        .with_phase_transition(from, to)
}

#[test]
fn fully_compliant_chain_passes_pci_dss() {
    rt().block_on(async {
        let store = Arc::new(MemoryBackend::new());
        let chain = OutcomeChain::new(store);

        // Build a clean forward-arc chain.
        chain.append(event(OutcomeEventType::GalhoCreated, "g")).await.unwrap();
        chain
            .append(morphism_event("g", MorphismId::Plan, Phase::Declared, Phase::Planned))
            .await
            .unwrap();
        chain
            .append(event(OutcomeEventType::StackLockAcquired, "g"))
            .await
            .unwrap();
        chain
            .append(morphism_event(
                "g",
                MorphismId::ApplyToPreview,
                Phase::Planned,
                Phase::AppliedPendingReview,
            ))
            .await
            .unwrap();
        chain
            .append(event(OutcomeEventType::SyncConfirmed, "g"))
            .await
            .unwrap();
        chain
            .append(morphism_event(
                "g",
                MorphismId::RecordApproval,
                Phase::AppliedPendingReview,
                Phase::ApprovedAwaitingMerge,
            ))
            .await
            .unwrap();
        chain
            .append(morphism_event(
                "g",
                MorphismId::Promote,
                Phase::ApprovedAwaitingMerge,
                Phase::Merged,
            ))
            .await
            .unwrap();
        chain
            .append(morphism_event(
                "g",
                MorphismId::Verify,
                Phase::Merged,
                Phase::Verified,
            ))
            .await
            .unwrap();
        chain
            .append(morphism_event(
                "g",
                MorphismId::SealDone,
                Phase::Verified,
                Phase::Done,
            ))
            .await
            .unwrap();

        let report = verify_regime(&chain, ComplianceRegime::PciDss4_0).await.unwrap();
        assert_eq!(report.status, ComplianceStatus::Compliant);
        assert!(report.violations.is_empty());
        assert_eq!(report.total_events, 9);
    });
}

#[test]
fn promote_without_approval_violates_pci_dss() {
    rt().block_on(async {
        let store = Arc::new(MemoryBackend::new());
        let chain = OutcomeChain::new(store);

        // Promote without prior SyncConfirmed.
        chain.append(event(OutcomeEventType::GalhoCreated, "g")).await.unwrap();
        chain
            .append(morphism_event(
                "g",
                MorphismId::Promote,
                Phase::ApprovedAwaitingMerge,
                Phase::Merged,
            ))
            .await
            .unwrap();

        let report = verify_regime(&chain, ComplianceRegime::PciDss4_0).await.unwrap();
        assert_eq!(report.status, ComplianceStatus::NonCompliant);
        assert!(report.violations.iter().any(|v| matches!(
            v.rule,
            ComplianceRule::PromoteRequiresPriorApproval
        )));
    });
}

#[test]
fn verify_without_apply_violates_baseline() {
    rt().block_on(async {
        let store = Arc::new(MemoryBackend::new());
        let chain = OutcomeChain::new(store);

        chain.append(event(OutcomeEventType::GalhoCreated, "g")).await.unwrap();
        chain
            .append(morphism_event(
                "g",
                MorphismId::Verify,
                Phase::Merged,
                Phase::Verified,
            ))
            .await
            .unwrap();

        let report = verify_regime(&chain, ComplianceRegime::SocII).await.unwrap();
        assert_eq!(report.status, ComplianceStatus::NonCompliant);
        assert!(report.violations.iter().any(|v| matches!(
            v.rule,
            ComplianceRule::VerifyRequiresPriorApply
        )));
    });
}

#[test]
fn done_without_verified_violates_baseline() {
    rt().block_on(async {
        let store = Arc::new(MemoryBackend::new());
        let chain = OutcomeChain::new(store);

        chain.append(event(OutcomeEventType::GalhoCreated, "g")).await.unwrap();
        chain
            .append(morphism_event(
                "g",
                MorphismId::SealDone,
                Phase::Verified,
                Phase::Done,
            ))
            .await
            .unwrap();

        let report = verify_regime(&chain, ComplianceRegime::FedRampModerate).await.unwrap();
        assert_eq!(report.status, ComplianceStatus::NonCompliant);
        assert!(report.violations.iter().any(|v| matches!(
            v.rule,
            ComplianceRule::DoneRequiresVerified
        )));
    });
}

#[test]
fn apply_without_lock_violates_baseline() {
    rt().block_on(async {
        let store = Arc::new(MemoryBackend::new());
        let chain = OutcomeChain::new(store);

        chain.append(event(OutcomeEventType::GalhoCreated, "g")).await.unwrap();
        chain
            .append(morphism_event(
                "g",
                MorphismId::ApplyToPreview,
                Phase::Planned,
                Phase::AppliedPendingReview,
            ))
            .await
            .unwrap();

        let report = verify_regime(&chain, ComplianceRegime::PciDss4_0).await.unwrap();
        assert_eq!(report.status, ComplianceStatus::NonCompliant);
        assert!(report.violations.iter().any(|v| matches!(
            v.rule,
            ComplianceRule::ApplyRequiresStackLock
        )));
    });
}

#[test]
fn revert_without_apply_violates_baseline() {
    rt().block_on(async {
        let store = Arc::new(MemoryBackend::new());
        let chain = OutcomeChain::new(store);

        chain.append(event(OutcomeEventType::GalhoCreated, "g")).await.unwrap();
        chain
            .append(morphism_event(
                "g",
                MorphismId::RevertApply,
                Phase::AppliedPendingReview,
                Phase::RollingBack,
            ))
            .await
            .unwrap();

        let baseline = ComplianceBaseline::prescribed(ComplianceRegime::PciDss4_0);
        let report = verify(&chain, &baseline).await.unwrap();
        assert!(report.violations.iter().any(|v| matches!(
            v.rule,
            ComplianceRule::RevertRequiresPriorApply
        )));
    });
}

#[test]
fn galho_summary_groups_violations_per_galho() {
    rt().block_on(async {
        let store = Arc::new(MemoryBackend::new());
        let chain = OutcomeChain::new(store);

        // Two galhos, each promote-without-approval.
        chain.append(event(OutcomeEventType::GalhoCreated, "a")).await.unwrap();
        chain.append(event(OutcomeEventType::GalhoCreated, "b")).await.unwrap();
        chain
            .append(morphism_event("a", MorphismId::Promote, Phase::ApprovedAwaitingMerge, Phase::Merged))
            .await
            .unwrap();
        chain
            .append(morphism_event("b", MorphismId::Promote, Phase::ApprovedAwaitingMerge, Phase::Merged))
            .await
            .unwrap();
        chain
            .append(morphism_event("b", MorphismId::Verify, Phase::Merged, Phase::Verified))
            .await
            .unwrap();

        let report = verify_regime(&chain, ComplianceRegime::PciDss4_0).await.unwrap();
        assert_eq!(report.status, ComplianceStatus::NonCompliant);
        // galho_summary should have both galhos with violation counts.
        assert!(report.galho_summary.contains_key("a"));
        assert!(report.galho_summary.contains_key("b"));
        // b has both a promote-without-approval + verify-without-apply.
        assert!(report.galho_summary["b"] >= report.galho_summary["a"]);
    });
}

#[test]
fn baseline_with_extra_rules_extends_check_set() {
    rt().block_on(async {
        let store = Arc::new(MemoryBackend::new());
        let chain = OutcomeChain::new(store);

        chain.append(event(OutcomeEventType::GalhoCreated, "g")).await.unwrap();

        let baseline = ComplianceBaseline::prescribed(ComplianceRegime::PciDss4_0)
            .with_extra_rules(vec![ComplianceRule::NoPhaseStutter]);

        let report = verify(&chain, &baseline).await.unwrap();
        assert!(report.rules_evaluated.contains(&ComplianceRule::NoPhaseStutter));
    });
}

#[test]
fn empty_chain_is_trivially_compliant() {
    rt().block_on(async {
        let store = Arc::new(MemoryBackend::new());
        let chain = OutcomeChain::new(store);
        let report = verify_regime(&chain, ComplianceRegime::PciDss4_0).await.unwrap();
        assert_eq!(report.status, ComplianceStatus::Compliant);
        assert_eq!(report.total_events, 0);
    });
}

#[test]
fn compliance_report_serializes_to_json() {
    rt().block_on(async {
        let store = Arc::new(MemoryBackend::new());
        let chain = OutcomeChain::new(store);
        chain.append(event(OutcomeEventType::GalhoCreated, "g")).await.unwrap();

        let report = verify_regime(&chain, ComplianceRegime::SocII).await.unwrap();
        let json = serde_json::to_string(&report).unwrap();
        let round: galho_types::ComplianceReport = serde_json::from_str(&json).unwrap();
        assert_eq!(round, report);
    });
}
