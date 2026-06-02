//! §VI.4 — the always-restable comfort matrix (golden-path attraction, made mechanical).
//!
//! ECLUSA §VI states the *always-restable* invariant: every reachable [`Phase`]
//! is a **comfortable berth** — a state a change can rest in indefinitely
//! without holding an environment hostage, while always retaining a
//! surgery-free exit toward a good terminal. That is the structural half of
//! golden-path attraction: a change forced off-golden can always *park
//! comfortably* and then return.
//!
//! This module turns that prose law into typed data + a forcing-function test
//! ([`tests/always_restable_matrix.rs`]). The compounding payoff:
//!
//! * **Totality by construction** — [`Phase::all`] ↔ [`COMFORT_MATRIX`] is a
//!   proven bijection, so a new `Phase` variant *without* a comfort row fails
//!   CI. The law cannot silently lose coverage.
//! * **The as-shipped failures are typed, not prose** — the two berths that are
//!   *not* comfortable today (A.1 `RolledBack` holds the StackLock; A.2 multi-IaC
//!   partial-promote at `Merged`) are [`ComfortTier::Fails`] cells. Resolving one
//!   is a *visible matrix edit* the test enforces — landing the fix without
//!   flipping the cell (or vice versa) fails CI.
//! * **The berth-cost ↔ hostage law is checked** — a `ParkCost::None` berth
//!   cannot claim a TTL-bounded-lock mitigation; a `ParkCost::TtlBounded` berth
//!   cannot claim a `Guaranteed` no-hostage. The cost and the comfort can't drift.
//!
//! The matrix owns *comfort*; the FSM ([`crate::phase::transitions_from`]) owns
//! *edges*. The test ties them: every non-terminal berth must have a real
//! outgoing edge (a surgery-free exit), so comfort claims can never outrun the
//! state machine.

use crate::phase::Phase;

/// What it costs siblings / the team to leave a change parked at a berth.
///
/// This is the *liveness* dimension of comfort: a berth with a non-`None` cost
/// can still be comfortable (the cost is bounded + by-design), but the cost is
/// the thing a misconfiguration could turn into a hostage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParkCost {
    /// Nothing held — no StackLock, no live cloud resources, root untouched.
    /// The maximally-comfortable cost.
    None,
    /// A `StackLock` is held, bounded by its TTL + holder quorum. Comfortable
    /// while the bound holds; a misconfigured infinite TTL is the regression.
    TtlBounded,
    /// Not a rest berth — the flow drives straight through, book-ended by
    /// non-transient comfortable berths (e.g. `Merged` mid-promote). Parking
    /// here is not expected to last; comfort is asserted on the book-ends.
    Transient,
}

/// Per-criterion comfort strength, severity-orderable (worst = highest).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ComfortTier {
    /// Structurally / type-enforced — cannot regress without a compile error.
    Guaranteed,
    /// Bounded by a TTL / timer (StackLock TTL, drift-detection cron latency).
    /// A real guarantee, with a latency/expiry caveat.
    MitigatedTtl,
    /// Holds by a config convention or a deliberate by-design choice (e.g. the
    /// no-lock-at-`Planned` convention, the lock-held-by-design at `Failed`).
    MitigatedConfig,
    /// NOT comfortable as-shipped. The `&'static str` names the tracked fix
    /// (e.g. `"A.1"`, `"A.2"`) so the matrix doubles as the live defect ledger.
    Fails(&'static str),
}

impl ComfortTier {
    /// Severity rank — higher is worse. Lets a berth report its weakest cell.
    #[must_use]
    pub const fn severity(self) -> u8 {
        match self {
            Self::Guaranteed => 0,
            Self::MitigatedTtl => 1,
            Self::MitigatedConfig => 2,
            Self::Fails(_) => 3,
        }
    }

    /// Is this an as-shipped failure?
    #[must_use]
    pub const fn is_fail(self) -> bool {
        matches!(self, Self::Fails(_))
    }
}

/// One berth's comfort classification across the four §VI.2 criteria.
#[derive(Debug, Clone, Copy)]
pub struct ComfortRow {
    /// The berth this row classifies.
    pub phase: Phase,
    /// What it costs to park here.
    pub park_cost: ParkCost,
    /// No environment is held hostage (no lock starves same-stack siblings).
    pub no_hostage: ComfortTier,
    /// No code-change flow is blocked (git history stays mergeable; no PR held
    /// hostage by this berth).
    pub no_code_flow: ComfortTier,
    /// No IaC/env state is left half-applied or silently diverged.
    pub no_env_flow: ComfortTier,
    /// A typed surgery-free exit toward a good terminal exists.
    pub clean_exit: ComfortTier,
}

impl ComfortRow {
    /// The four criteria as `(name, tier)` pairs — for total iteration.
    #[must_use]
    pub fn criteria(&self) -> [(&'static str, ComfortTier); 4] {
        [
            ("no_hostage", self.no_hostage),
            ("no_code_flow", self.no_code_flow),
            ("no_env_flow", self.no_env_flow),
            ("clean_exit", self.clean_exit),
        ]
    }

    /// The weakest (highest-severity) criterion — the berth's overall comfort.
    #[must_use]
    pub fn worst_tier(&self) -> ComfortTier {
        self.criteria()
            .into_iter()
            .map(|(_, t)| t)
            .max_by_key(|t| t.severity())
            .expect("criteria() is never empty")
    }

    /// A berth is *comfortable as-shipped* when no criterion fails.
    #[must_use]
    pub fn is_comfortable(&self) -> bool {
        !self.worst_tier().is_fail()
    }

    /// The set of `(criterion, fix)` failures on this berth (usually empty).
    #[must_use]
    pub fn failures(&self) -> Vec<(&'static str, &'static str)> {
        self.criteria()
            .into_iter()
            .filter_map(|(name, t)| match t {
                ComfortTier::Fails(fix) => Some((name, fix)),
                _ => None,
            })
            .collect()
    }
}

/// The §VI.2 comfort matrix — exactly one row per [`Phase`]. The single source
/// of truth for berth comfort. [`tests/always_restable_matrix.rs`] proves it
/// total (bijective with [`Phase::all`]), FSM-consistent (every non-terminal
/// berth has a real exit), and ledger-accurate (the `Fails` cells match the
/// tracked A.1/A.2 fixes exactly).
///
/// Reading the matrix top to bottom is reading the §VI.2 berth table: the two
/// good terminals (`Done`, `Destroyed`) are all-`Guaranteed` + `None`-cost; the
/// pre-apply golden-capable berths (`Declared`, `Planned`) hold nothing; the
/// applied/approved berths hold a TTL-bounded lock; and exactly two cells fail.
pub const COMFORT_MATRIX: &[ComfortRow] = &[
    // ── Forward arc ────────────────────────────────────────────────────────
    ComfortRow {
        phase: Phase::Declared,
        park_cost: ParkCost::None,
        no_hostage: ComfortTier::Guaranteed,
        no_code_flow: ComfortTier::Guaranteed,
        no_env_flow: ComfortTier::Guaranteed,
        clean_exit: ComfortTier::Guaranteed,
    },
    ComfortRow {
        phase: Phase::Planned,
        park_cost: ParkCost::None,
        // no-lock-at-Plan is a convention today, not a type invariant (A.3).
        no_hostage: ComfortTier::MitigatedConfig,
        no_code_flow: ComfortTier::Guaranteed,
        no_env_flow: ComfortTier::Guaranteed,
        clean_exit: ComfortTier::Guaranteed,
    },
    ComfortRow {
        phase: Phase::AppliedPendingReview,
        park_cost: ParkCost::TtlBounded,
        no_hostage: ComfortTier::MitigatedTtl, // StackLock TTL + holder quorum.
        no_code_flow: ComfortTier::Guaranteed,
        // PerPrStateKey disjoint key (M1); GalhoBranchState is M4 for unrep.
        no_env_flow: ComfortTier::MitigatedConfig,
        clean_exit: ComfortTier::Guaranteed, // Abandon is a real tofu destroy.
    },
    ComfortRow {
        phase: Phase::ApprovedAwaitingMerge,
        park_cost: ParkCost::TtlBounded,
        no_hostage: ComfortTier::MitigatedTtl,
        no_code_flow: ComfortTier::Guaranteed,
        no_env_flow: ComfortTier::Guaranteed,
        clean_exit: ComfortTier::Guaranteed,
    },
    ComfortRow {
        phase: Phase::Merged,
        park_cost: ParkCost::Transient, // pass-through, not a rest berth.
        no_hostage: ComfortTier::Guaranteed,
        no_code_flow: ComfortTier::Guaranteed,
        // A.2 — multi-IaC promote can leave the shared root half-applied with
        // no atomic-root-commit barrier across IaC systems.
        no_env_flow: ComfortTier::Fails("A.2"),
        clean_exit: ComfortTier::Guaranteed,
    },
    ComfortRow {
        phase: Phase::Verified,
        park_cost: ParkCost::None,
        no_hostage: ComfortTier::Guaranteed,
        no_code_flow: ComfortTier::Guaranteed,
        // drift detection leans on the daily cron until the M5 in-cluster
        // controller — latency, not absence.
        no_env_flow: ComfortTier::MitigatedTtl,
        clean_exit: ComfortTier::Guaranteed,
    },
    ComfortRow {
        phase: Phase::Done,
        park_cost: ParkCost::None,
        no_hostage: ComfortTier::Guaranteed,
        no_code_flow: ComfortTier::Guaranteed,
        no_env_flow: ComfortTier::Guaranteed,
        clean_exit: ComfortTier::Guaranteed, // terminal ACCEPTED — arrival is the exit.
    },
    // ── Backward arc ───────────────────────────────────────────────────────
    ComfortRow {
        phase: Phase::RollingBack,
        park_cost: ParkCost::TtlBounded, // lock held by design during reverse-traversal.
        no_hostage: ComfortTier::MitigatedConfig,
        no_code_flow: ComfortTier::Guaranteed,
        no_env_flow: ComfortTier::MitigatedConfig, // transient restore in progress.
        clean_exit: ComfortTier::MitigatedConfig,
    },
    ComfortRow {
        phase: Phase::RolledBack,
        park_cost: ParkCost::TtlBounded,
        // A.1 — release_holder fires only on terminal/Abandon, so the StackLock
        // stays held at this non-terminal rest and starves same-stack siblings.
        no_hostage: ComfortTier::Fails("A.1"),
        no_code_flow: ComfortTier::Guaranteed,
        no_env_flow: ComfortTier::Guaranteed,
        clean_exit: ComfortTier::Guaranteed, // Resume → Planned re-enters golden.
    },
    ComfortRow {
        phase: Phase::Drifted,
        park_cost: ParkCost::None, // no lock held at rest.
        no_hostage: ComfortTier::Guaranteed,
        no_code_flow: ComfortTier::Guaranteed,
        // root IS diverged; comfort = detected + attested + policy-deferred,
        // not converged. A first-class "known-diverged" berth.
        no_env_flow: ComfortTier::MitigatedConfig,
        clean_exit: ComfortTier::Guaranteed,
    },
    // ── Terminal ───────────────────────────────────────────────────────────
    ComfortRow {
        phase: Phase::Destroyed,
        park_cost: ParkCost::None,
        no_hostage: ComfortTier::Guaranteed,
        no_code_flow: ComfortTier::Guaranteed,
        no_env_flow: ComfortTier::Guaranteed,
        clean_exit: ComfortTier::Guaranteed, // terminal CLEANLY-ABANDONED.
    },
    ComfortRow {
        phase: Phase::Failed,
        park_cost: ParkCost::TtlBounded, // lock held by design so break-glass can't race.
        no_hostage: ComfortTier::MitigatedConfig,
        no_code_flow: ComfortTier::Guaranteed,
        no_env_flow: ComfortTier::Guaranteed,
        clean_exit: ComfortTier::Guaranteed, // Recover → Declared re-enters golden.
    },
];

/// Look up a phase's comfort row.
#[must_use]
pub fn comfort_of(phase: Phase) -> Option<&'static ComfortRow> {
    COMFORT_MATRIX.iter().find(|r| r.phase == phase)
}
