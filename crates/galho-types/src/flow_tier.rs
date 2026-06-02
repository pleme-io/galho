//! §XIX — the golden-path-favoring lifecycle (the golden attractor), mechanized.
//!
//! Step #1 ([`crate::comfort`]) proved every berth *rests* comfortably. This is
//! the complementary half: the **golden-return** structure — which edges keep a
//! change on golden rails (touch no cloud) vs cross a cloud ceiling, the
//! **golden subgraph** (phases reachable from `Declared` on golden edges only),
//! and the **two distance measures** the attractor uses to steer a diverged
//! change back.
//!
//! Per §XIX.6 this is the C1 (`NoDependentTypes`) ceiling: graph reachability is
//! not a Rust compile-time property, so these are **mechanical-CI forcing
//! functions** ([`tests/golden_attractor.rs`]), never types. What they buy:
//!
//! * **`edge_tier` is total + per-edge** — every one of the 23
//!   [`crate::phase::transition_table`] rows resolves to exactly one tier; a new
//!   edge without a tier fails CI. `(from, morphism)` is a unique key (the two
//!   `DriftReconcile` and three `RevertApply` edges disambiguate by `from`).
//! * **The golden subgraph is computed, not asserted** — BFS over the golden
//!   edges, so the shipped golden region can never drift from the shipped table.
//! * **Two honest distances** — `via_golden` (return on golden rails, `None` if
//!   the only return crosses a ceiling) and `any_edge` (return at all, a crossing
//!   counts as a step). Keeping them distinct is the whole point: a single
//!   full-table BFS would silently count a cloud-apply as "toward golden".

use crate::phase::{MorphismId, Phase, transition_table};
use std::collections::{BTreeSet, VecDeque};

/// The irreducible per-edge ceiling a [`EdgeTier::CeilingCrossing`] touches.
///
/// §XIX.4 keys the per-edge taxonomy on C5/C2 only; C1 (no-dependent-types),
/// C3 (wire-erasure), and C4 (shared-account quota) annotate *guarantees* over
/// the apply/promote arc as a whole, not single transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Ceiling {
    /// C5 — a real, non-transactional cloud apply / destroy / reverse cutover.
    NonTransactionalIo,
    /// C2 — a live-cloud observation (drift detect, escalation on divergence).
    ExternalObservation,
}

/// Per-edge golden classification (§XIX.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EdgeTier {
    /// Touches no cloud — a `Plan<S>` value, a DAG commit, a hash compare, a
    /// cursor reset. Free to traverse; no witness required (§XIX.5).
    GoldenPreserving,
    /// Crosses a cloud ceiling; requires an explicit gated `CeilingCrossing`
    /// witness at the typestate destination (§XIX.5).
    CeilingCrossing(Ceiling),
}

impl EdgeTier {
    /// Does this edge keep the change on golden rails (no cloud touch)?
    #[must_use]
    pub const fn is_golden(self) -> bool {
        matches!(self, Self::GoldenPreserving)
    }
}

/// The §XIX.4 EdgeTier mapping — the sole authority, keyed per-edge by
/// `(from, morphism)`. Returns `None` for any pair that is not a shipped
/// [`transition_table`] edge.
///
/// Tally (§XIX.4): GoldenPreserving = 9, CeilingCrossing = 14
/// (NonTransactionalIo = 12, ExternalObservation = 2).
#[must_use]
pub fn edge_tier(from: Phase, morphism: MorphismId) -> Option<EdgeTier> {
    use Ceiling::{ExternalObservation as Obs, NonTransactionalIo as Io};
    use EdgeTier::{CeilingCrossing as X, GoldenPreserving as G};
    use MorphismId::{
        Abandon, ApplyToPreview, DriftReconcile, Escalate, Plan, Promote, RecordApproval, Recover,
        Resume, RevertApply, SealDone, Verify,
    };
    use Phase::{
        AppliedPendingReview, ApprovedAwaitingMerge, Declared, Drifted, Failed, Merged, Planned,
        RolledBack, RollingBack, Verified,
    };
    Some(match (from, morphism) {
        // ── Forward arc ──────────────────────────────────────────────────
        (Declared, Plan) => G,                              // #1  read-only plan
        (Planned, ApplyToPreview) => X(Io),                 // #2  real tofu apply
        (AppliedPendingReview, RecordApproval) => G,        // #3  git/PR fact
        (ApprovedAwaitingMerge, Promote) => G,              // #4  DAG commit
        (Merged, Verify) => G,                              // #5  hash compare
        (Verified, SealDone) => G,                          // #6  terminal seal
        // ── Backward arc ─────────────────────────────────────────────────
        (AppliedPendingReview, RevertApply) => X(Io),       // #7
        (ApprovedAwaitingMerge, RevertApply) => X(Io),      // #8
        (RollingBack, RevertApply) => X(Io),                // #9
        (Verified, DriftReconcile) => X(Obs),               // #10 drift detect
        (Drifted, DriftReconcile) => X(Io),                 // #11 heal = re-apply
        (Drifted, Escalate) => X(Obs),                      // #12
        // ── Abandon (per-edge: golden iff nothing applied) ───────────────
        (Declared, Abandon) => G,                           // #13 metadata-only
        (Planned, Abandon) => G,                            // #14 metadata-only
        (AppliedPendingReview, Abandon) => X(Io),           // #15 real destroy
        (ApprovedAwaitingMerge, Abandon) => X(Io),          // #16
        (RolledBack, Abandon) => X(Io),                     // #17
        (Failed, Abandon) => X(Io),                         // #18
        // ── Escalate-to-Failed ───────────────────────────────────────────
        (AppliedPendingReview, Escalate) => X(Io),          // #19
        (Merged, Escalate) => X(Io),                        // #20 post-promote reconcile
        (RollingBack, Escalate) => X(Io),                   // #21
        // ── Recovery ─────────────────────────────────────────────────────
        (Failed, Recover) => G,                             // #22 cursor reset
        (RolledBack, Resume) => G,                          // #23 fresh plan
        _ => return None,
    })
}

/// Golden out-edges from a phase (filtered [`transition_table`]).
fn golden_targets(p: Phase) -> impl Iterator<Item = Phase> {
    transition_table()
        .iter()
        .filter(move |t| t.from == p && edge_tier(t.from, t.morphism).is_some_and(EdgeTier::is_golden))
        .map(|t| t.to)
}

/// All out-edge targets from a phase.
fn any_targets(p: Phase) -> impl Iterator<Item = Phase> {
    transition_table().iter().filter(move |t| t.from == p).map(|t| t.to)
}

/// The **golden subgraph**: phases reachable from `Declared` traversing only
/// `GoldenPreserving` edges. The C1 graph property — computed, never a type.
#[must_use]
pub fn golden_subgraph() -> BTreeSet<Phase> {
    let mut seen = BTreeSet::from([Phase::Declared]);
    let mut q = VecDeque::from([Phase::Declared]);
    while let Some(p) = q.pop_front() {
        for to in golden_targets(p) {
            if seen.insert(to) {
                q.push_back(to);
            }
        }
    }
    seen
}

/// Min edges to **re-enter the golden subgraph on golden rails** (traversing
/// `GoldenPreserving` edges only). `Some(0)` if already golden; `None` if the
/// only return crosses a ceiling (e.g. `Drifted`, `RollingBack`).
#[must_use]
pub fn distance_to_golden_via_golden(p: Phase) -> Option<usize> {
    let golden = golden_subgraph();
    if golden.contains(&p) {
        return Some(0);
    }
    let mut seen = BTreeSet::from([p]);
    let mut q = VecDeque::from([(p, 0usize)]);
    while let Some((cur, d)) = q.pop_front() {
        for to in golden_targets(cur) {
            if golden.contains(&to) {
                return Some(d + 1);
            }
            if seen.insert(to) {
                q.push_back((to, d + 1));
            }
        }
    }
    None
}

/// Min edges to re-enter the golden subgraph traversing **any** edge (a crossing
/// counts as a step — this is *not* "return on golden rails"). `Some(0)` if
/// already golden. `None` only if the subgraph is unreachable (never, per the
/// §VII reachability theorem — every phase reaches `Destroyed` ≤ 7).
#[must_use]
pub fn distance_to_golden_any_edge(p: Phase) -> Option<usize> {
    let golden = golden_subgraph();
    if golden.contains(&p) {
        return Some(0);
    }
    let mut seen = BTreeSet::from([p]);
    let mut q = VecDeque::from([(p, 0usize)]);
    while let Some((cur, d)) = q.pop_front() {
        for to in any_targets(cur) {
            if golden.contains(&to) {
                return Some(d + 1);
            }
            if seen.insert(to) {
                q.push_back((to, d + 1));
            }
        }
    }
    None
}
