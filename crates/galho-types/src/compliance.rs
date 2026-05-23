//! Typed compliance baselines + violation taxonomy + verification reports.
//!
//! See `pleme-io/theory/GALHO.md` §X. galho-types owns the rule definitions + report
//! shapes (pure types, no I/O); galho-storage owns the verifier that walks an
//! `OutcomeChain` against a baseline. The CLI exposes `galho audit --baseline <regime>`.
//!
//! Empirical grounding: SOC 2 CC8.1 (change management) + PCI-DSS 6.5 (review of
//! actual provisioned change) + FIPS 140-3 L3 (typed evidence trail) require that the
//! chain DEMONSTRATE certain ordering / completeness invariants. The rules below are
//! the typed encoding of those requirements.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::sync::ComplianceRegime;

/// One typed compliance rule. Each rule is a queryable predicate over the audit chain.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ComplianceRule {
    /// Every `Promote` morphism must be preceded by at least one `SyncConfirmed`
    /// (operator approval) event for the same galho. SOC 2 CC8.1 / PCI-DSS 6.5.
    PromoteRequiresPriorApproval,

    /// Every `Verify` morphism must be preceded by an `ApplyToPreview` for the same
    /// galho (you can't verify what wasn't applied).
    VerifyRequiresPriorApply,

    /// Every galho that reaches `SealDone` must have passed through `Verified`.
    /// Closes the "Done" loop — no skipping verification.
    DoneRequiresVerified,

    /// Every `RevertApply` must be preceded by an `ApplyToPreview` for the same galho
    /// (you can't revert what wasn't applied).
    RevertRequiresPriorApply,

    /// At least one stack-lock-acquired or stack-lock-joined event must precede every
    /// `ApplyToPreview`. Enforces the per-stack-root lock primitive.
    ApplyRequiresStackLock,

    /// No two consecutive `MorphismFired` events for the same galho may produce the
    /// same destination phase (no silent stutter). Tests transition table correctness.
    NoPhaseStutter,

    /// Chain integrity invariant must hold. Always required; surfaced as a typed
    /// violation when it doesn't.
    ChainIntegrityHolds,
}

impl ComplianceRule {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PromoteRequiresPriorApproval => "promote_requires_prior_approval",
            Self::VerifyRequiresPriorApply => "verify_requires_prior_apply",
            Self::DoneRequiresVerified => "done_requires_verified",
            Self::RevertRequiresPriorApply => "revert_requires_prior_apply",
            Self::ApplyRequiresStackLock => "apply_requires_stack_lock",
            Self::NoPhaseStutter => "no_phase_stutter",
            Self::ChainIntegrityHolds => "chain_integrity_holds",
        }
    }
}

impl std::fmt::Display for ComplianceRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A baseline is the set of rules for a given regime. Each `ComplianceRegime` has a
/// `prescribed_default()` baseline encoding its evidence requirements.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComplianceBaseline {
    pub regime: ComplianceRegime,
    pub rules: Vec<ComplianceRule>,
}

impl ComplianceBaseline {
    /// Fleet-prescribed baseline for a given regime. Operators can extend / override
    /// via `with_extra_rules` for organization-specific requirements.
    #[must_use]
    pub fn prescribed(regime: ComplianceRegime) -> Self {
        use ComplianceRule::*;
        let rules = match &regime {
            // PCI-DSS 4.0 + SOC 2 share the core change-management invariants. Both
            // require evidence-of-review-of-materialized-change.
            ComplianceRegime::PciDss4_0
            | ComplianceRegime::SocII
            | ComplianceRegime::Iso27001
            | ComplianceRegime::Iso27701 => vec![
                ChainIntegrityHolds,
                PromoteRequiresPriorApproval,
                VerifyRequiresPriorApply,
                DoneRequiresVerified,
                ApplyRequiresStackLock,
                NoPhaseStutter,
                RevertRequiresPriorApply,
            ],
            // FedRAMP + HIPAA stricter — add explicit chain integrity AND require
            // the apply-receipt-before-promote ordering directly.
            ComplianceRegime::FedRampModerate
            | ComplianceRegime::FedRampHigh
            | ComplianceRegime::Hipaa
            | ComplianceRegime::Dora
            | ComplianceRegime::Fips140_3 => vec![
                ChainIntegrityHolds,
                PromoteRequiresPriorApproval,
                VerifyRequiresPriorApply,
                DoneRequiresVerified,
                ApplyRequiresStackLock,
                NoPhaseStutter,
                RevertRequiresPriorApply,
            ],
            ComplianceRegime::Custom(_) => vec![ChainIntegrityHolds],
        };
        Self { regime, rules }
    }

    /// Append additional rules to a baseline. Useful for org-specific requirements.
    #[must_use]
    pub fn with_extra_rules(mut self, extra: Vec<ComplianceRule>) -> Self {
        for r in extra {
            if !self.rules.contains(&r) {
                self.rules.push(r);
            }
        }
        self
    }
}

/// One observed violation of a `ComplianceRule`. Carries enough context for the
/// operator to locate + remediate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComplianceViolation {
    pub rule: ComplianceRule,
    pub galho_name: Option<String>,
    pub sequence: Option<u64>,
    pub detail: String,
}

/// Verifier output. Surfaces overall status + per-galho violation count + the full
/// violations list. JSON-friendly for downstream tooling (CI, dashboards, kensa).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComplianceReport {
    pub regime: ComplianceRegime,
    pub status: ComplianceStatus,
    pub violations: Vec<ComplianceViolation>,
    pub rules_evaluated: Vec<ComplianceRule>,
    pub galho_summary: BTreeMap<String, u32>, // galho_name → violation count
    pub total_events: u64,
}

/// Overall pass/fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComplianceStatus {
    Compliant,
    NonCompliant,
}

impl ComplianceStatus {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Compliant => "compliant",
            Self::NonCompliant => "non_compliant",
        }
    }
}

impl std::fmt::Display for ComplianceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl ComplianceReport {
    /// Build a report from the verification outcome. Status derives from violations
    /// list — empty means Compliant, non-empty means NonCompliant.
    #[must_use]
    pub fn from_violations(
        regime: ComplianceRegime,
        violations: Vec<ComplianceViolation>,
        rules_evaluated: Vec<ComplianceRule>,
        total_events: u64,
    ) -> Self {
        let status = if violations.is_empty() {
            ComplianceStatus::Compliant
        } else {
            ComplianceStatus::NonCompliant
        };
        let mut galho_summary: BTreeMap<String, u32> = BTreeMap::new();
        for v in &violations {
            if let Some(name) = &v.galho_name {
                *galho_summary.entry(name.clone()).or_default() += 1;
            }
        }
        Self {
            regime,
            status,
            violations,
            rules_evaluated,
            galho_summary,
            total_events,
        }
    }

    #[must_use]
    pub fn is_compliant(&self) -> bool {
        matches!(self.status, ComplianceStatus::Compliant)
    }

    #[must_use]
    pub fn violation_count(&self) -> usize {
        self.violations.len()
    }
}
