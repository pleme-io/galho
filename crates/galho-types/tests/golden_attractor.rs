//! §XIX — golden-attractor forcing-function test (C1 graph properties via CI).
//!
//! The complement to `always_restable_matrix.rs`: that proved every berth
//! *rests* comfortably; this proves the *golden-return* structure is real and
//! the shipped table matches the §XIX.4 authority.
//!
//! 1. **`edge_tier` total + correct** — every transition_table row → exactly one
//!    tier; the 9/14 (12 NTIO / 2 ExtObs) §XIX.4 tally holds.
//! 2. **golden subgraph computed from the shipped table** — pins the mechanical
//!    truth (and surfaces any drift from the §XIX.6 prose).
//! 3. **two distance measures honest** — `via_golden` (golden rails) ⊆ reachable
//!    `any_edge`; the golden-rails-returnable set is exactly the computed set.
//! 4. **every excursion reaches a good terminal** (§VII μ-bound), even where the
//!    return is not on golden rails.

use galho_types::flow_tier::{
    Ceiling, EdgeTier, distance_to_golden_any_edge, distance_to_golden_via_golden, edge_tier,
    golden_subgraph,
};
use galho_types::phase::{Phase, PhaseClass, transition_table, transitions_from};
use std::collections::BTreeSet;

/// `edge_tier` resolves every shipped edge to exactly one tier, and the §XIX.4
/// tally holds: 9 GoldenPreserving, 14 CeilingCrossing (12 NonTransactionalIo,
/// 2 ExternalObservation). A new edge without a tier fails here.
#[test]
fn edge_tier_is_total_and_matches_the_xix4_tally() {
    let (mut golden, mut io, mut obs) = (0usize, 0usize, 0usize);
    for t in transition_table() {
        match edge_tier(t.from, t.morphism) {
            None => panic!("edge {:?} --{:?}--> {:?} has no EdgeTier", t.from, t.morphism, t.to),
            Some(EdgeTier::GoldenPreserving) => golden += 1,
            Some(EdgeTier::CeilingCrossing(Ceiling::NonTransactionalIo)) => io += 1,
            Some(EdgeTier::CeilingCrossing(Ceiling::ExternalObservation)) => obs += 1,
        }
    }
    assert_eq!(transition_table().len(), 23, "expected 23 shipped edges");
    assert_eq!(golden, 9, "GoldenPreserving tally (§XIX.4)");
    assert_eq!(io, 12, "CeilingCrossing(NonTransactionalIo) tally");
    assert_eq!(obs, 2, "CeilingCrossing(ExternalObservation) tally");
    assert_eq!(golden + io + obs, 23);
}

/// `edge_tier` returns `None` for non-shipped `(from, morphism)` pairs (it keys
/// the real table, not a permissive per-morphism guess).
#[test]
fn edge_tier_rejects_non_edges() {
    // Plan only exists from Declared; Plan-from-Merged is not an edge.
    assert!(edge_tier(Phase::Merged, galho_types::phase::MorphismId::Plan).is_none());
    // SealDone only from Verified.
    assert!(edge_tier(Phase::Declared, galho_types::phase::MorphismId::SealDone).is_none());
}

/// The golden subgraph computed by BFS over the shipped golden edges. This pins
/// the MECHANICAL truth of the shipped 23-edge table.
///
/// FINDING (corrects §XIX.6 prose): the shipped golden subgraph is
/// `{Declared, Planned, Destroyed}` — NOT the 7-phase set the prose claims. The
/// prose assumes ApplyAfterMerge golden edges (`RecordApproval`/`Promote` *from
/// Planned*, no apply) that are NOT in the shipped table: the only entry to the
/// approval/merge/verify/done arc is the `#2 ApplyToPreview` *crossing*. So in
/// the shipped machine the only golden-reachable phases from genesis are the
/// pre-apply berths (`Declared`, `Planned`) + the clean metadata-abandon
/// (`Destroyed`). This MATCHES §XVIII.4's deeper honesty ("no golden flow can
/// include a real cloud apply") — the apply ceiling is unavoidable on the path
/// to `Done`. (Theory follow-up: §XIX.6's stated subgraph needs correcting or
/// the ApplyAfterMerge edges need landing in the table.)
#[test]
fn golden_subgraph_is_the_shipped_pre_apply_region() {
    let g = golden_subgraph();
    let want: BTreeSet<Phase> =
        [Phase::Declared, Phase::Planned, Phase::Destroyed].into_iter().collect();
    assert_eq!(
        g, want,
        "shipped golden subgraph drifted.\n got:  {g:?}\n want: {want:?}\n\
         (the path past `Planned` to approval/Done crosses the #2 apply ceiling; \
          landing ApplyAfterMerge golden edges from `Planned` would widen this)"
    );
}

/// The golden-rails-returnable set: phases with a finite `via_golden` distance.
/// Exactly the golden subgraph (distance 0) plus `Failed` (Recover #22 → Declared)
/// and `RolledBack` (Resume #23 → Planned). Every other off-golden phase returns
/// only by crossing a ceiling (`via_golden = None`).
#[test]
fn golden_rails_return_set_is_exact() {
    let mut on_rails: BTreeSet<Phase> = BTreeSet::new();
    for p in Phase::all() {
        if distance_to_golden_via_golden(*p).is_some() {
            on_rails.insert(*p);
        }
    }
    let want: BTreeSet<Phase> = [
        Phase::Declared,
        Phase::Planned,
        Phase::Destroyed,
        Phase::Failed,
        Phase::RolledBack,
    ]
    .into_iter()
    .collect();
    assert_eq!(
        on_rails, want,
        "golden-rails-returnable set drifted.\n got:  {on_rails:?}\n want: {want:?}"
    );
    // Spot-check the two non-trivial golden returns.
    assert_eq!(distance_to_golden_via_golden(Phase::Failed), Some(1), "Failed --Recover--> Declared");
    assert_eq!(distance_to_golden_via_golden(Phase::RolledBack), Some(1), "RolledBack --Resume--> Planned");
}

/// Honesty: `via_golden` never beats `any_edge` (golden rails are a subset of all
/// returns), and a `None` golden return implies the only return crosses a ceiling.
#[test]
fn via_golden_is_never_shorter_than_any_edge() {
    for p in Phase::all() {
        match (distance_to_golden_via_golden(*p), distance_to_golden_any_edge(*p)) {
            (Some(g), Some(a)) => assert!(g >= a, "{p:?}: via_golden {g} < any_edge {a}"),
            (Some(g), None) => panic!("{p:?}: via_golden {g} but any_edge None — impossible"),
            (None, _) => { /* return only via a ceiling crossing (or terminal) — honest */ }
        }
    }
}

/// §VII reachability: every non-terminal phase reaches a good terminal in ≤7
/// (the μ-bound). Done is the absorbing ACCEPTED terminal (no out-edges, not in
/// the golden subgraph); every other phase can reach `Destroyed`/`Declared` (∈
/// subgraph) by some edge.
#[test]
fn every_non_terminal_reaches_the_golden_subgraph_within_mu() {
    for p in Phase::all() {
        if p.class() == PhaseClass::Terminal {
            assert!(transitions_from(*p).is_empty(), "terminal {p:?} has out-edges");
            continue;
        }
        let d = distance_to_golden_any_edge(*p)
            .unwrap_or_else(|| panic!("non-terminal {p:?} cannot reach the golden subgraph"));
        assert!(d <= 7, "{p:?}: distance-to-golden {d} exceeds the §VII μ-bound of 7");
    }
}
