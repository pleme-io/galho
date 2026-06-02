//! `PhaseMorphism` trait + concrete morphisms.
//!
//! Each morphism is a typed transition between phases with explicit preconditions,
//! invariant checks, and (for forward morphisms in the apply-before-merge arc) a
//! typed inverse. Composes with `iac-forge::Morphism` at the higher integration
//! layer — galho-types intentionally avoids the iac-forge dep to stay lean.

use serde::{Deserialize, Serialize};

use crate::phase::{MorphismId, Phase};

// MorphismContext + MorphismRequirement need serde for Runtime checkpoint/restore.
// Other typed primitives in this module either derive their own serde or are pure
// trait surfaces (no data).

/// One typed phase transition. Implementors:
///
/// - Declare the `(from, to)` phase pair.
/// - Express preconditions on the morphism context (galho head hash, stack lock state,
///   reviewer quorum, drift status, etc.).
/// - Declare a typed inverse where possible (backward arc reversibility).
///
/// Forward apply / verify / promote logic lives in the adapter and the controller;
/// the morphism objects in galho-types own the *contract*, not the execution.
pub trait PhaseMorphism: Send + Sync + 'static {
    fn id(&self) -> MorphismId;
    fn from_phase(&self) -> Phase;
    fn to_phase(&self) -> Phase;

    /// Human-readable label for the morphism. Defaults to `id().as_str()`.
    fn label(&self) -> &'static str {
        self.id().as_str()
    }

    /// Preconditions on the morphism context. Returns the list of unmet conditions
    /// (empty means "all preconditions satisfied").
    fn check_preconditions(&self, ctx: &MorphismContext) -> Vec<MorphismRequirement>;

    /// Typed inverse morphism if this is reversible. `None` for terminal moves.
    fn inverse(&self) -> Option<MorphismId>;

    /// Does this morphism touch cloud state? Forward morphisms in the apply arc do;
    /// metadata-only morphisms (Plan, RecordApproval, SealDone) don't. Used by the
    /// audit chain to classify entries.
    fn touches_cloud(&self) -> bool;
}

/// Context the morphism evaluates against. Owned by the controller / CLI; passed
/// by reference to `check_preconditions`. Serializable so Runtime can checkpoint
/// to + restore from an `ObjectStore` across process restarts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MorphismContext {
    /// Sealed: the only legitimate mutation path is [`crate::KnowledgeBase::advance`]
    /// (which runs the typed preconditions first). Read via [`MorphismContext::current_phase`].
    /// Keeping the field `pub(crate)` makes the precondition-bypassing direct write
    /// (`ctx.current_phase = …`) impossible outside this crate. The serde wire name is
    /// preserved so checkpoints round-trip unchanged.
    #[serde(rename = "current_phase")]
    pub(crate) current_phase: Phase,
    pub galho_name: String,
    pub has_plan: bool,
    pub has_apply_receipt: bool,
    pub has_approval_quorum: bool,
    pub has_merge_event: bool,
    pub has_verify_receipt: bool,
    pub stack_lock_held: bool,
    pub drift_detected: bool,
    pub conflict_open: bool,
    pub jira_ticket_resolvable: bool,
    /// Per-phase confirmations recorded via the Sync flow. Keyed on the Phase whose
    /// boundary is being gated; value is the set of roles that have signed off. Used by
    /// the controller / Runtime to compute `has_approval_quorum` against typed
    /// `OperatorApproval { quorum }` syncs.
    pub confirmations: std::collections::BTreeMap<Phase, std::collections::BTreeSet<String>>,
    /// Names of galhos this galho depends on. Promote is gated by ALL dependencies
    /// reaching a "done-enough" phase (Verified or Done) — operationally enforces the
    /// stacked-PR dependency-ordering invariant carve creates at PR-stack time.
    #[serde(default)]
    pub depends_on: std::collections::BTreeSet<String>,
    /// Tracks which deps have been observed as `done-enough`. The Runtime maintains
    /// this as deps advance — when this becomes a superset of `depends_on`, the
    /// `DependencyNotMet` check passes for Promote.
    #[serde(default)]
    pub deps_satisfied: std::collections::BTreeSet<String>,
}

impl MorphismContext {
    /// A minimal context at the `Declared` phase. Test fixture + initial-state factory.
    #[must_use]
    pub fn declared(galho_name: impl Into<String>) -> Self {
        Self {
            current_phase: Phase::Declared,
            galho_name: galho_name.into(),
            has_plan: false,
            has_apply_receipt: false,
            has_approval_quorum: false,
            has_merge_event: false,
            has_verify_receipt: false,
            stack_lock_held: false,
            drift_detected: false,
            conflict_open: false,
            jira_ticket_resolvable: false,
            confirmations: std::collections::BTreeMap::new(),
            depends_on: std::collections::BTreeSet::new(),
            deps_satisfied: std::collections::BTreeSet::new(),
        }
    }

    /// Same as `declared` but with explicit dependencies.
    #[must_use]
    pub fn declared_with_deps(
        galho_name: impl Into<String>,
        deps: impl IntoIterator<Item = String>,
    ) -> Self {
        let mut ctx = Self::declared(galho_name);
        ctx.depends_on = deps.into_iter().collect();
        ctx
    }

    /// The current phase. The only read path now that the field is sealed.
    #[must_use]
    pub fn current_phase(&self) -> Phase {
        self.current_phase
    }

    /// Set the current phase. `pub(crate)` so the ONLY external mutation path is
    /// [`crate::KnowledgeBase::advance`], which runs preconditions first — direct
    /// `ctx.current_phase = …` from outside the crate is impossible.
    pub(crate) fn set_phase(&mut self, phase: Phase) {
        self.current_phase = phase;
    }

    /// Test-fixture constructor: a `Declared`-shaped context placed at an arbitrary
    /// phase. Needed by the external proptest crate (which can't write the
    /// `pub(crate)` field directly). Production code advances via
    /// [`crate::KnowledgeBase::advance`].
    #[must_use]
    pub fn at_phase(galho_name: impl Into<String>, phase: Phase) -> Self {
        let mut ctx = Self::declared(galho_name);
        ctx.current_phase = phase;
        ctx
    }

    /// Count confirmations for a given phase.
    #[must_use]
    pub fn confirmation_count(&self, phase: Phase) -> usize {
        self.confirmations.get(&phase).map_or(0, std::collections::BTreeSet::len)
    }

    /// Has the given role already confirmed at the given phase?
    #[must_use]
    pub fn has_confirmed(&self, phase: Phase, role: &str) -> bool {
        self.confirmations
            .get(&phase)
            .is_some_and(|set| set.contains(role))
    }

    /// All dependencies satisfied? True when `deps_satisfied ⊇ depends_on`.
    #[must_use]
    pub fn all_deps_satisfied(&self) -> bool {
        self.depends_on.is_subset(&self.deps_satisfied)
    }

    /// List of currently-unmet dependencies (deps that haven't reached "done-enough").
    #[must_use]
    pub fn unmet_deps(&self) -> Vec<String> {
        self.depends_on
            .difference(&self.deps_satisfied)
            .cloned()
            .collect()
    }
}

/// A typed precondition that wasn't met. Surfaced to the operator + audit chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MorphismRequirement {
    WrongPhase { expected: Phase, actual: Phase },
    PlanMissing,
    ApplyReceiptMissing,
    ApprovalQuorumMissing,
    MergeEventMissing,
    VerifyReceiptMissing,
    StackLockMissing,
    DriftPresent,
    ConflictOpen,
    JiraTicketUnresolvable,
    /// One or more galhos this galho depends on have not yet reached a "done-enough"
    /// phase (Verified / Done). Enforces stacked-PR dependency ordering.
    DependencyNotMet { unmet: Vec<String> },
    /// No `(from, morphism)` row exists in the static transition table — the
    /// preconditions passed but the FSM has no declared destination for this
    /// move. Surfaced as a hard typed error instead of silently substituting
    /// the morphism's nominal `to_phase()` (which masks wrong-target bugs for
    /// multi-destination morphisms like Abandon / RevertApply / Escalate).
    MissingTransitionRow { from: Phase, morphism: MorphismId },
}

// ===== Concrete forward morphisms =====

macro_rules! morphism {
    (
        $name:ident,
        from: $from:ident,
        to: $to:ident,
        id: $id:ident,
        cloud: $cloud:literal,
        inverse: $inverse:expr,
        preconditions: |$ctx:ident, $missing:ident| $body:block
    ) => {
        /// Forward morphism: see the transition table in `phase.rs`.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct $name;

        impl PhaseMorphism for $name {
            fn id(&self) -> MorphismId { MorphismId::$id }
            fn from_phase(&self) -> Phase { Phase::$from }
            fn to_phase(&self) -> Phase { Phase::$to }
            fn inverse(&self) -> Option<MorphismId> { $inverse }
            fn touches_cloud(&self) -> bool { $cloud }

            fn check_preconditions(&self, $ctx: &MorphismContext) -> Vec<MorphismRequirement> {
                let mut $missing: Vec<MorphismRequirement> = Vec::new();
                if $ctx.current_phase != Phase::$from {
                    $missing.push(MorphismRequirement::WrongPhase {
                        expected: Phase::$from,
                        actual: $ctx.current_phase,
                    });
                }
                $body
                $missing
            }
        }
    };
}

morphism! {
    Plan,
    from: Declared,
    to: Planned,
    id: Plan,
    cloud: false,
    inverse: Some(MorphismId::Abandon),
    preconditions: |_ctx, _missing| { /* no extra requirements beyond phase */ }
}

morphism! {
    ApplyToPreview,
    from: Planned,
    to: AppliedPendingReview,
    id: ApplyToPreview,
    cloud: true,
    inverse: Some(MorphismId::RevertApply),
    preconditions: |ctx, missing| {
        if !ctx.has_plan { missing.push(MorphismRequirement::PlanMissing); }
        if !ctx.stack_lock_held { missing.push(MorphismRequirement::StackLockMissing); }
        if ctx.conflict_open { missing.push(MorphismRequirement::ConflictOpen); }
    }
}

morphism! {
    RecordApproval,
    from: AppliedPendingReview,
    to: ApprovedAwaitingMerge,
    id: RecordApproval,
    cloud: false,
    inverse: Some(MorphismId::RevertApply),
    preconditions: |ctx, missing| {
        if !ctx.has_apply_receipt { missing.push(MorphismRequirement::ApplyReceiptMissing); }
        if !ctx.has_approval_quorum { missing.push(MorphismRequirement::ApprovalQuorumMissing); }
    }
}

morphism! {
    Promote,
    from: ApprovedAwaitingMerge,
    to: Merged,
    id: Promote,
    cloud: false,
    inverse: None,
    preconditions: |ctx, missing| {
        if !ctx.has_approval_quorum { missing.push(MorphismRequirement::ApprovalQuorumMissing); }
        if !ctx.has_merge_event { missing.push(MorphismRequirement::MergeEventMissing); }
        if !ctx.all_deps_satisfied() {
            missing.push(MorphismRequirement::DependencyNotMet { unmet: ctx.unmet_deps() });
        }
    }
}

morphism! {
    Verify,
    from: Merged,
    to: Verified,
    id: Verify,
    cloud: false,
    inverse: Some(MorphismId::DriftReconcile),
    preconditions: |ctx, missing| {
        if !ctx.has_verify_receipt { missing.push(MorphismRequirement::VerifyReceiptMissing); }
    }
}

morphism! {
    SealDone,
    from: Verified,
    to: Done,
    id: SealDone,
    cloud: false,
    inverse: None,
    preconditions: |ctx, missing| {
        if !ctx.jira_ticket_resolvable {
            missing.push(MorphismRequirement::JiraTicketUnresolvable);
        }
    }
}

// ===== Backward morphisms =====

/// Backward morphism: revert a preview-applied galho. Holds the stack lock throughout.
/// Re-targets to `RolledBack` once the reverse-traversal completes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RevertApply;

impl PhaseMorphism for RevertApply {
    fn id(&self) -> MorphismId { MorphismId::RevertApply }
    fn from_phase(&self) -> Phase { Phase::AppliedPendingReview }
    fn to_phase(&self) -> Phase { Phase::RollingBack }
    fn inverse(&self) -> Option<MorphismId> { Some(MorphismId::Resume) }
    fn touches_cloud(&self) -> bool { true }

    fn check_preconditions(&self, ctx: &MorphismContext) -> Vec<MorphismRequirement> {
        let mut missing = Vec::new();
        if !matches!(
            ctx.current_phase,
            Phase::AppliedPendingReview | Phase::ApprovedAwaitingMerge | Phase::RollingBack
        ) {
            missing.push(MorphismRequirement::WrongPhase {
                expected: Phase::AppliedPendingReview,
                actual: ctx.current_phase,
            });
        }
        if !ctx.has_apply_receipt {
            missing.push(MorphismRequirement::ApplyReceiptMissing);
        }
        missing
    }
}

/// Drift reconcile: bidirectional between Verified and Drifted. The controller fires
/// this when scheduled drift check finds divergence; the operator can also fire it
/// explicitly via `galho reconcile`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriftReconcile;

impl PhaseMorphism for DriftReconcile {
    fn id(&self) -> MorphismId { MorphismId::DriftReconcile }
    fn from_phase(&self) -> Phase { Phase::Verified } // primary direction
    fn to_phase(&self) -> Phase { Phase::Drifted } // detection; recovery is Drifted → Verified
    fn inverse(&self) -> Option<MorphismId> { Some(MorphismId::DriftReconcile) } // self-inverse
    fn touches_cloud(&self) -> bool { true }

    fn check_preconditions(&self, ctx: &MorphismContext) -> Vec<MorphismRequirement> {
        let mut missing = Vec::new();
        if !matches!(ctx.current_phase, Phase::Verified | Phase::Drifted) {
            missing.push(MorphismRequirement::WrongPhase {
                expected: Phase::Verified,
                actual: ctx.current_phase,
            });
        }
        missing
    }
}

/// Abandon: from any non-terminal phase to Destroyed. Releases the stack lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Abandon;

impl PhaseMorphism for Abandon {
    fn id(&self) -> MorphismId { MorphismId::Abandon }
    fn from_phase(&self) -> Phase { Phase::Declared } // any non-terminal phase; uses transition table at runtime
    fn to_phase(&self) -> Phase { Phase::Destroyed }
    fn inverse(&self) -> Option<MorphismId> { None } // terminal
    fn touches_cloud(&self) -> bool { true } // tears down preview env if applied

    fn check_preconditions(&self, ctx: &MorphismContext) -> Vec<MorphismRequirement> {
        let mut missing = Vec::new();
        if matches!(ctx.current_phase, Phase::Done | Phase::Destroyed) {
            missing.push(MorphismRequirement::WrongPhase {
                expected: Phase::Declared, // any non-terminal
                actual: ctx.current_phase,
            });
        }
        missing
    }
}

/// Escalate: from any operational phase to Failed. Triggered by repeated timeouts
/// or operator-declared severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Escalate;

impl PhaseMorphism for Escalate {
    fn id(&self) -> MorphismId { MorphismId::Escalate }
    fn from_phase(&self) -> Phase { Phase::AppliedPendingReview }
    fn to_phase(&self) -> Phase { Phase::Failed }
    fn inverse(&self) -> Option<MorphismId> { Some(MorphismId::Recover) }
    fn touches_cloud(&self) -> bool { false }

    fn check_preconditions(&self, _ctx: &MorphismContext) -> Vec<MorphismRequirement> {
        // Operator-driven; intentionally low precondition bar.
        Vec::new()
    }
}

/// Recover: from Failed back to Declared. Operator-driven break-glass-style move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recover;

impl PhaseMorphism for Recover {
    fn id(&self) -> MorphismId { MorphismId::Recover }
    fn from_phase(&self) -> Phase { Phase::Failed }
    fn to_phase(&self) -> Phase { Phase::Declared }
    fn inverse(&self) -> Option<MorphismId> { Some(MorphismId::Escalate) }
    fn touches_cloud(&self) -> bool { false }

    fn check_preconditions(&self, ctx: &MorphismContext) -> Vec<MorphismRequirement> {
        let mut missing = Vec::new();
        if ctx.current_phase != Phase::Failed {
            missing.push(MorphismRequirement::WrongPhase {
                expected: Phase::Failed,
                actual: ctx.current_phase,
            });
        }
        missing
    }
}

/// Resume: from RolledBack back to Planned. The operator retries the apply after a
/// rollback (typically after fixing the underlying issue).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resume;

impl PhaseMorphism for Resume {
    fn id(&self) -> MorphismId { MorphismId::Resume }
    fn from_phase(&self) -> Phase { Phase::RolledBack }
    fn to_phase(&self) -> Phase { Phase::Planned }
    fn inverse(&self) -> Option<MorphismId> { Some(MorphismId::RevertApply) }
    fn touches_cloud(&self) -> bool { false }

    fn check_preconditions(&self, ctx: &MorphismContext) -> Vec<MorphismRequirement> {
        let mut missing = Vec::new();
        if ctx.current_phase != Phase::RolledBack {
            missing.push(MorphismRequirement::WrongPhase {
                expected: Phase::RolledBack,
                actual: ctx.current_phase,
            });
        }
        if !ctx.has_plan {
            missing.push(MorphismRequirement::PlanMissing);
        }
        missing
    }
}

/// Materialize a typed `PhaseMorphism` trait object by id. Returns `None` for unknown
/// ids; in practice the [`MorphismId`] enum is exhaustive so callers can `.expect()`.
#[must_use]
pub fn morphism_for(id: MorphismId) -> Option<Box<dyn PhaseMorphism>> {
    match id {
        MorphismId::Plan => Some(Box::new(Plan)),
        MorphismId::ApplyToPreview => Some(Box::new(ApplyToPreview)),
        MorphismId::RecordApproval => Some(Box::new(RecordApproval)),
        MorphismId::Promote => Some(Box::new(Promote)),
        MorphismId::Verify => Some(Box::new(Verify)),
        MorphismId::SealDone => Some(Box::new(SealDone)),
        MorphismId::RevertApply => Some(Box::new(RevertApply)),
        MorphismId::DriftReconcile => Some(Box::new(DriftReconcile)),
        MorphismId::Abandon => Some(Box::new(Abandon)),
        MorphismId::Escalate => Some(Box::new(Escalate)),
        MorphismId::Recover => Some(Box::new(Recover)),
        MorphismId::Resume => Some(Box::new(Resume)),
    }
}
