//! Walk an `OutcomeChain` against a typed `ComplianceBaseline` and produce a typed
//! `ComplianceReport`. Pure function over chain entries (modulo the async I/O to read
//! them); deterministic given the same chain.
//!
//! Each `ComplianceRule` is implemented as a pass over the chain's events with
//! per-galho state tracking (which morphisms have fired, which approvals confirmed).

use std::collections::{BTreeMap, BTreeSet};

use galho_types::{
    ComplianceBaseline, ComplianceRegime, ComplianceReport, ComplianceRule,
    ComplianceViolation, MorphismId, OutcomeEvent, OutcomeEventType, Phase,
};

use crate::object_store::{ObjectStore, StoreError};
use crate::outcome_chain::OutcomeChain;

/// Walk a chain against a baseline. Returns the typed `ComplianceReport`.
pub async fn verify<S: ObjectStore>(
    chain: &OutcomeChain<S>,
    baseline: &ComplianceBaseline,
) -> Result<ComplianceReport, StoreError> {
    let entries = chain.entries().await?;
    let total_events = entries.len() as u64;
    let events: Vec<&OutcomeEvent> = entries.iter().map(|e| &e.event).collect();

    let mut violations = Vec::new();

    for rule in &baseline.rules {
        match rule {
            ComplianceRule::ChainIntegrityHolds => {
                let ok = chain.verify_integrity().await?;
                if !ok {
                    violations.push(ComplianceViolation {
                        rule: rule.clone(),
                        galho_name: None,
                        sequence: None,
                        detail: "chain integrity check failed (hash chain broken)".into(),
                    });
                }
            }
            ComplianceRule::PromoteRequiresPriorApproval => {
                check_promote_requires_approval(&events, rule, &mut violations);
            }
            ComplianceRule::VerifyRequiresPriorApply => {
                check_verify_requires_prior_apply(&events, rule, &mut violations);
            }
            ComplianceRule::DoneRequiresVerified => {
                check_done_requires_verified(&events, rule, &mut violations);
            }
            ComplianceRule::RevertRequiresPriorApply => {
                check_revert_requires_prior_apply(&events, rule, &mut violations);
            }
            ComplianceRule::ApplyRequiresStackLock => {
                check_apply_requires_stack_lock(&events, rule, &mut violations);
            }
            ComplianceRule::NoPhaseStutter => {
                check_no_phase_stutter(&events, rule, &mut violations);
            }
        }
    }

    Ok(ComplianceReport::from_violations(
        baseline.regime.clone(),
        violations,
        baseline.rules.clone(),
        total_events,
    ))
}

/// Top-level convenience: build the prescribed baseline for the regime + verify.
pub async fn verify_regime<S: ObjectStore>(
    chain: &OutcomeChain<S>,
    regime: ComplianceRegime,
) -> Result<ComplianceReport, StoreError> {
    let baseline = ComplianceBaseline::prescribed(regime);
    verify(chain, &baseline).await
}

// ----- Per-rule check helpers -----

fn check_promote_requires_approval(
    events: &[&OutcomeEvent],
    rule: &ComplianceRule,
    violations: &mut Vec<ComplianceViolation>,
) {
    let mut approved_galhos: BTreeSet<&str> = BTreeSet::new();
    for (i, e) in events.iter().enumerate() {
        if e.event_type == OutcomeEventType::SyncConfirmed {
            approved_galhos.insert(e.galho_name.as_str());
        }
        if e.event_type == OutcomeEventType::MorphismFired
            && e.morphism == Some(MorphismId::Promote)
            && !approved_galhos.contains(e.galho_name.as_str())
        {
            violations.push(ComplianceViolation {
                rule: rule.clone(),
                galho_name: Some(e.galho_name.clone()),
                sequence: Some(i as u64),
                detail: format!(
                    "promote at seq={i} for galho '{}' has no prior SyncConfirmed",
                    e.galho_name
                ),
            });
        }
    }
}

fn check_verify_requires_prior_apply(
    events: &[&OutcomeEvent],
    rule: &ComplianceRule,
    violations: &mut Vec<ComplianceViolation>,
) {
    let mut applied: BTreeSet<&str> = BTreeSet::new();
    for (i, e) in events.iter().enumerate() {
        if e.event_type == OutcomeEventType::MorphismFired
            && e.morphism == Some(MorphismId::ApplyToPreview)
        {
            applied.insert(e.galho_name.as_str());
        }
        if e.event_type == OutcomeEventType::MorphismFired
            && e.morphism == Some(MorphismId::Verify)
            && !applied.contains(e.galho_name.as_str())
        {
            violations.push(ComplianceViolation {
                rule: rule.clone(),
                galho_name: Some(e.galho_name.clone()),
                sequence: Some(i as u64),
                detail: format!(
                    "verify at seq={i} for galho '{}' has no prior ApplyToPreview",
                    e.galho_name
                ),
            });
        }
    }
}

fn check_done_requires_verified(
    events: &[&OutcomeEvent],
    rule: &ComplianceRule,
    violations: &mut Vec<ComplianceViolation>,
) {
    let mut verified: BTreeSet<&str> = BTreeSet::new();
    for (i, e) in events.iter().enumerate() {
        if e.event_type == OutcomeEventType::MorphismFired
            && e.morphism == Some(MorphismId::Verify)
            && e.to_phase == Some(Phase::Verified)
        {
            verified.insert(e.galho_name.as_str());
        }
        if e.event_type == OutcomeEventType::MorphismFired
            && e.morphism == Some(MorphismId::SealDone)
            && !verified.contains(e.galho_name.as_str())
        {
            violations.push(ComplianceViolation {
                rule: rule.clone(),
                galho_name: Some(e.galho_name.clone()),
                sequence: Some(i as u64),
                detail: format!(
                    "seal_done at seq={i} for galho '{}' has no prior Verified",
                    e.galho_name
                ),
            });
        }
    }
}

fn check_revert_requires_prior_apply(
    events: &[&OutcomeEvent],
    rule: &ComplianceRule,
    violations: &mut Vec<ComplianceViolation>,
) {
    let mut applied: BTreeSet<&str> = BTreeSet::new();
    for (i, e) in events.iter().enumerate() {
        if e.event_type == OutcomeEventType::MorphismFired
            && e.morphism == Some(MorphismId::ApplyToPreview)
        {
            applied.insert(e.galho_name.as_str());
        }
        if e.event_type == OutcomeEventType::MorphismFired
            && e.morphism == Some(MorphismId::RevertApply)
            && !applied.contains(e.galho_name.as_str())
        {
            violations.push(ComplianceViolation {
                rule: rule.clone(),
                galho_name: Some(e.galho_name.clone()),
                sequence: Some(i as u64),
                detail: format!(
                    "revert_apply at seq={i} for galho '{}' has no prior ApplyToPreview",
                    e.galho_name
                ),
            });
        }
    }
}

fn check_apply_requires_stack_lock(
    events: &[&OutcomeEvent],
    rule: &ComplianceRule,
    violations: &mut Vec<ComplianceViolation>,
) {
    let mut locked: BTreeSet<&str> = BTreeSet::new();
    for (i, e) in events.iter().enumerate() {
        if matches!(
            e.event_type,
            OutcomeEventType::StackLockAcquired | OutcomeEventType::StackLockJoined
        ) {
            locked.insert(e.galho_name.as_str());
        }
        if e.event_type == OutcomeEventType::MorphismFired
            && e.morphism == Some(MorphismId::ApplyToPreview)
            && !locked.contains(e.galho_name.as_str())
        {
            violations.push(ComplianceViolation {
                rule: rule.clone(),
                galho_name: Some(e.galho_name.clone()),
                sequence: Some(i as u64),
                detail: format!(
                    "apply_to_preview at seq={i} for galho '{}' has no prior StackLock event",
                    e.galho_name
                ),
            });
        }
    }
}

fn check_no_phase_stutter(
    events: &[&OutcomeEvent],
    rule: &ComplianceRule,
    violations: &mut Vec<ComplianceViolation>,
) {
    // For each galho, track its most-recent to_phase; consecutive MorphismFired events
    // with to_phase == prior to_phase + from_phase matches indicate a no-op stutter.
    let mut last_phase: BTreeMap<&str, Phase> = BTreeMap::new();
    for (i, e) in events.iter().enumerate() {
        if e.event_type != OutcomeEventType::MorphismFired {
            continue;
        }
        let Some(to) = e.to_phase else { continue };
        let Some(from) = e.from_phase else { continue };
        if from == to {
            // legitimate self-loop (e.g. DriftReconcile from Drifted to Drifted is allowed
            // by the transition table; not a violation).
            continue;
        }
        if let Some(prev) = last_phase.get(e.galho_name.as_str()) {
            if *prev == to {
                violations.push(ComplianceViolation {
                    rule: rule.clone(),
                    galho_name: Some(e.galho_name.clone()),
                    sequence: Some(i as u64),
                    detail: format!(
                        "phase stutter at seq={i} for galho '{}' ({:?} → {:?} then again to {:?})",
                        e.galho_name, from, to, to
                    ),
                });
            }
        }
        last_phase.insert(e.galho_name.as_str(), to);
    }
}
