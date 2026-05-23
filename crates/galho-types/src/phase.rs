//! Typed phase of a galho's progression — the state-machine surface.
//!
//! See `pleme-io/theory/GALHO.md` §IV-bis. Three layers:
//!
//! - [`Phase`] — the typed lifecycle position. Forward arc handles apply-before-merge
//!   as the default (matching empirical Akeyless / compliance-regulated workflow);
//!   backward arc covers rollback + drift + abandon.
//! - [`PhaseClass`] — coarse grouping (Forward / Backward / Terminal). Useful for
//!   operator UX (which phases CAN we move forward from?) and for KnowledgeBase queries.
//! - [`PhaseTransition`] — typed (from, to) tuple with associated `MorphismId`. The
//!   transition table is dense + exhaustive; orphan transitions are a compile error
//!   via the `expect_known_transition` helper.

use serde::{Deserialize, Serialize};

/// The full phase enumeration. Forward arc + backward arc + terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    // ===== Forward arc (apply-before-merge default) =====
    /// Galho exists; no plan yet. Initial state on `galho new`.
    Declared,
    /// `Plan<S>` computed + invariant-proven + cost-estimated.
    Planned,
    /// Plan applied to a preview / shared-state env; cloud reality materialized
    /// before merge so reviewers see the actual provisioned resources.
    /// This is the compliance-load-bearing phase (SOC 2 CC8.1 + PCI-DSS 6.5).
    AppliedPendingReview,
    /// Reviewer quorum signed; carve gate + galho gate both green.
    ApprovedAwaitingMerge,
    /// PR merged into base; root-galho DAG advanced; reconciler converging cloud.
    Merged,
    /// Live cloud verified to match merged state (smoke green, drift = 0).
    Verified,
    /// Jira-sync to Done complete; OutcomeChain sealed for this galho.
    Done,

    // ===== Backward arc =====
    /// Executing typed reverse-traversal of the apply DAG. Holds the stack lock
    /// throughout to prevent concurrent forward attempts.
    RollingBack,
    /// Preview env (or merged env) restored to pre-apply state. Attested.
    RolledBack,
    /// Post-Verified drift detected. Reconciler attempting auto-correct OR
    /// escalating to operator per [`crate::sync::SyncConfig`].
    Drifted,

    // ===== Terminal =====
    /// Galho destroyed (PR closed without merge, explicit `galho destroy`, or
    /// stack abandoned). Stack lock released; DAG nodes preserved per GC policy.
    Destroyed,
    /// Hard failure requiring operator intervention. Stack lock held until recovery.
    Failed,
}

impl Phase {
    /// Coarse classification for operator UX + KnowledgeBase queries.
    #[must_use]
    pub fn class(&self) -> PhaseClass {
        match self {
            Self::Declared
            | Self::Planned
            | Self::AppliedPendingReview
            | Self::ApprovedAwaitingMerge
            | Self::Merged
            | Self::Verified => PhaseClass::Forward,

            Self::RollingBack | Self::RolledBack | Self::Drifted => PhaseClass::Backward,

            Self::Done | Self::Destroyed => PhaseClass::Terminal,
            Self::Failed => PhaseClass::Failure,
        }
    }

    /// Is this a terminal phase? Forward / backward / failure phases are NOT terminal —
    /// the operator can still move them.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self.class(), PhaseClass::Terminal)
    }

    /// Stable string identifier for telemetry, audit-chain entries, and operator UX.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Declared => "declared",
            Self::Planned => "planned",
            Self::AppliedPendingReview => "applied_pending_review",
            Self::ApprovedAwaitingMerge => "approved_awaiting_merge",
            Self::Merged => "merged",
            Self::Verified => "verified",
            Self::Done => "done",
            Self::RollingBack => "rolling_back",
            Self::RolledBack => "rolled_back",
            Self::Drifted => "drifted",
            Self::Destroyed => "destroyed",
            Self::Failed => "failed",
        }
    }

    /// Every phase known to galho. Static; ordering matches enum declaration.
    #[must_use]
    pub fn all() -> &'static [Phase] {
        &[
            Self::Declared,
            Self::Planned,
            Self::AppliedPendingReview,
            Self::ApprovedAwaitingMerge,
            Self::Merged,
            Self::Verified,
            Self::Done,
            Self::RollingBack,
            Self::RolledBack,
            Self::Drifted,
            Self::Destroyed,
            Self::Failed,
        ]
    }
}

impl std::fmt::Display for Phase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Coarse phase class. The operator can ask "which forward morphisms are available?"
/// by filtering on `PhaseClass::Forward`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhaseClass {
    /// In the forward arc; advancing toward Done.
    Forward,
    /// In the backward arc; reverting toward Destroyed or recovering toward Verified.
    Backward,
    /// Terminal (Done / Destroyed); no further transitions.
    Terminal,
    /// Failed; requires operator intervention.
    Failure,
}

/// A directed transition between two phases. Carries the `MorphismId` of the
/// concrete morphism that executes the move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PhaseTransition {
    pub from: Phase,
    pub to: Phase,
    pub morphism: MorphismId,
}

/// Stable identifier for each morphism. Matches the morphism's `name()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MorphismId {
    // forward
    Plan,
    ApplyToPreview,
    RecordApproval,
    Promote,
    Verify,
    SealDone,

    // backward
    RevertApply,
    DriftReconcile,
    Abandon,
    Escalate,

    // recovery
    Recover,
    Resume,
}

impl MorphismId {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::ApplyToPreview => "apply_to_preview",
            Self::RecordApproval => "record_approval",
            Self::Promote => "promote",
            Self::Verify => "verify",
            Self::SealDone => "seal_done",
            Self::RevertApply => "revert_apply",
            Self::DriftReconcile => "drift_reconcile",
            Self::Abandon => "abandon",
            Self::Escalate => "escalate",
            Self::Recover => "recover",
            Self::Resume => "resume",
        }
    }

    /// All morphism ids known to galho. Static; matches enum declaration order.
    #[must_use]
    pub fn all() -> &'static [MorphismId] {
        &[
            Self::Plan,
            Self::ApplyToPreview,
            Self::RecordApproval,
            Self::Promote,
            Self::Verify,
            Self::SealDone,
            Self::RevertApply,
            Self::DriftReconcile,
            Self::Abandon,
            Self::Escalate,
            Self::Recover,
            Self::Resume,
        ]
    }
}

impl std::fmt::Display for MorphismId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The canonical forward-and-backward transition table for galho.
/// Static; exhaustive over all reachable (from, to) pairs. Orphan phases (those with
/// no incoming or outgoing transitions) are caught by the `proptest` suite.
#[must_use]
pub fn transition_table() -> &'static [PhaseTransition] {
    use Phase::*;
    use MorphismId::*;
    &[
        // Forward arc — apply-before-merge default (the compliance-driven shape).
        PhaseTransition { from: Declared,              to: Planned,                morphism: Plan },
        PhaseTransition { from: Planned,               to: AppliedPendingReview,   morphism: ApplyToPreview },
        PhaseTransition { from: AppliedPendingReview,  to: ApprovedAwaitingMerge,  morphism: RecordApproval },
        PhaseTransition { from: ApprovedAwaitingMerge, to: Merged,                 morphism: Promote },
        PhaseTransition { from: Merged,                to: Verified,               morphism: Verify },
        PhaseTransition { from: Verified,              to: Done,                   morphism: SealDone },

        // Backward arc — rollback from preview.
        PhaseTransition { from: AppliedPendingReview,  to: RollingBack,            morphism: RevertApply },
        PhaseTransition { from: ApprovedAwaitingMerge, to: RollingBack,            morphism: RevertApply },
        PhaseTransition { from: RollingBack,           to: RolledBack,             morphism: RevertApply },

        // Drift detection + recovery.
        PhaseTransition { from: Verified,              to: Drifted,                morphism: DriftReconcile },
        PhaseTransition { from: Drifted,               to: Verified,               morphism: DriftReconcile },
        PhaseTransition { from: Drifted,               to: Failed,                 morphism: Escalate },

        // Abandon: from any non-terminal phase to Destroyed.
        PhaseTransition { from: Declared,              to: Destroyed,              morphism: Abandon },
        PhaseTransition { from: Planned,               to: Destroyed,              morphism: Abandon },
        PhaseTransition { from: AppliedPendingReview,  to: Destroyed,              morphism: Abandon },
        PhaseTransition { from: ApprovedAwaitingMerge, to: Destroyed,              morphism: Abandon },
        PhaseTransition { from: RolledBack,            to: Destroyed,              morphism: Abandon },
        PhaseTransition { from: Failed,                to: Destroyed,              morphism: Abandon },

        // Failure paths.
        PhaseTransition { from: AppliedPendingReview,  to: Failed,                 morphism: Escalate },
        PhaseTransition { from: Merged,                to: Failed,                 morphism: Escalate },
        PhaseTransition { from: RollingBack,           to: Failed,                 morphism: Escalate },

        // Recovery from Failed back into the arc.
        PhaseTransition { from: Failed,                to: Declared,               morphism: Recover },

        // Resume from RolledBack (operator chooses to retry the apply).
        PhaseTransition { from: RolledBack,            to: Planned,                morphism: Resume },
    ]
}

/// All transitions leaving `from`. Static lookup via the canonical table.
#[must_use]
pub fn transitions_from(from: Phase) -> Vec<PhaseTransition> {
    transition_table()
        .iter()
        .filter(|t| t.from == from)
        .copied()
        .collect()
}

/// All transitions arriving at `to`.
#[must_use]
pub fn transitions_to(to: Phase) -> Vec<PhaseTransition> {
    transition_table()
        .iter()
        .filter(|t| t.to == to)
        .copied()
        .collect()
}

/// Is `(from, to)` a known transition? Used by the morphism precondition checks
/// and by the controller to validate Galho CR updates.
#[must_use]
pub fn is_known_transition(from: Phase, to: Phase) -> bool {
    transition_table().iter().any(|t| t.from == from && t.to == to)
}
