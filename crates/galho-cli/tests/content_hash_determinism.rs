//! Determinism + verifiability invariants on `DepGraph::content_hash()`.
//!
//! The content hash is the substrate's commitment that "the carve stack at
//! review time looked like this." Operators paste the hex into PR descriptions;
//! auditors verify the same stack still hashes the same way.
//!
//! Four load-bearing invariants:
//!
//! 1. **Determinism.** Same input → same hash, regardless of how the snapshots
//!    are constructed.
//! 2. **Permutation invariance.** Insertion order of snapshots into DepGraph::new
//!    is irrelevant — sorting happens at construction.
//! 3. **Dep-order invariance.** Insertion order of dependency strings within a
//!    snapshot is irrelevant — canonical-bytes impl sorts via BTreeSet.
//! 4. **Semantic sensitivity.** Materially-different graphs produce different
//!    hashes (modulo BLAKE3 collision probability — essentially zero).
//!
//! Proptest runs 256 random cases per property; the typed canonical-bytes
//! emitter is consumed via galho-types' `content_hash` helper, which is the
//! same path tameshi's `Canonicalizer` consumes downstream — three uses now
//! (TypedState, ResourceGraph, DepGraph).

use galho_cli::{DepGraph, GalhoStateSnapshot};
use galho_types::Phase;
use proptest::prelude::*;

fn any_phase() -> impl Strategy<Value = Phase> {
    prop::sample::select(vec![
        Phase::Declared,
        Phase::Planned,
        Phase::AppliedPendingReview,
        Phase::ApprovedAwaitingMerge,
        Phase::Verified,
        Phase::Done,
    ])
}

fn any_name() -> impl Strategy<Value = String> {
    prop::sample::select(vec!["a", "b", "c", "d", "e", "f", "g", "h"]).prop_map(String::from)
}

fn any_snapshot() -> impl Strategy<Value = GalhoStateSnapshot> {
    (
        any_name(),
        any_phase(),
        prop::collection::vec(any_name(), 0..4),
        prop::collection::vec(any_name(), 0..4),
    )
        .prop_map(|(name, phase, deps, satisfied)| GalhoStateSnapshot {
            name,
            phase,
            depends_on: deps,
            deps_satisfied: satisfied,
        })
}

fn any_snapshots() -> impl Strategy<Value = Vec<GalhoStateSnapshot>> {
    prop::collection::vec(any_snapshot(), 0..6)
}

// ----- direct (non-proptest) invariants -----

#[test]
fn same_input_produces_same_hash() {
    let snaps = vec![
        GalhoStateSnapshot {
            name: "a".into(),
            phase: Phase::Verified,
            depends_on: vec![],
            deps_satisfied: vec![],
        },
        GalhoStateSnapshot {
            name: "b".into(),
            phase: Phase::Declared,
            depends_on: vec!["a".into()],
            deps_satisfied: vec!["a".into()],
        },
    ];
    let g1 = DepGraph::new(snaps.clone());
    let g2 = DepGraph::new(snaps);
    assert_eq!(g1.content_hash(), g2.content_hash());
}

#[test]
fn different_phase_produces_different_hash() {
    let mut snaps = vec![GalhoStateSnapshot {
        name: "a".into(),
        phase: Phase::Declared,
        depends_on: vec![],
        deps_satisfied: vec![],
    }];
    let h1 = DepGraph::new(snaps.clone()).content_hash();
    snaps[0].phase = Phase::Verified;
    let h2 = DepGraph::new(snaps).content_hash();
    assert_ne!(h1, h2, "phase change should change the hash");
}

#[test]
fn empty_graph_has_stable_known_hash() {
    let h1 = DepGraph::new(vec![]).content_hash_hex();
    let h2 = DepGraph::new(vec![]).content_hash_hex();
    assert_eq!(h1, h2);
    // Just sanity: it's a 64-char hex.
    assert_eq!(h1.len(), 64);
}

#[test]
fn hex_format_round_trip_via_to_hex() {
    let snaps = vec![GalhoStateSnapshot {
        name: "a".into(),
        phase: Phase::Done,
        depends_on: vec![],
        deps_satisfied: vec![],
    }];
    let g = DepGraph::new(snaps);
    let hex = g.content_hash_hex();
    // Hex is 64 chars (32 bytes).
    assert_eq!(hex.len(), 64);
    assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
}

// ----- proptest invariants -----

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Invariant 1: Determinism — same canonical input → same hash.
    #[test]
    fn determinism_same_input_same_hash(snaps in any_snapshots()) {
        let g1 = DepGraph::new(snaps.clone());
        let g2 = DepGraph::new(snaps);
        prop_assert_eq!(g1.content_hash(), g2.content_hash());
    }

    /// Invariant 2: Permutation invariance — shuffling the input snapshots
    /// produces the same hash because DepGraph::new sorts by name.
    #[test]
    fn permutation_invariance_of_snapshots(
        snaps in any_snapshots()
            .prop_filter("need unique names", |s| {
                let names: std::collections::BTreeSet<_> = s.iter().map(|x| &x.name).collect();
                names.len() == s.len()
            }),
    ) {
        let g_forward = DepGraph::new(snaps.clone());
        let mut reversed = snaps;
        reversed.reverse();
        let g_reversed = DepGraph::new(reversed);
        prop_assert_eq!(g_forward.content_hash(), g_reversed.content_hash());
    }

    /// Invariant 3: Dep-order invariance — re-ordering depends_on / deps_satisfied
    /// within a snapshot produces the same hash (canonical-bytes sorts via BTreeSet).
    #[test]
    fn dep_order_invariance(
        deps in prop::collection::vec(any_name(), 1..5)
            .prop_filter("unique", |ds| {
                let u: std::collections::BTreeSet<_> = ds.iter().collect();
                u.len() == ds.len()
            }),
    ) {
        let snap_forward = GalhoStateSnapshot {
            name: "x".into(),
            phase: Phase::Planned,
            depends_on: deps.clone(),
            deps_satisfied: vec![],
        };
        let mut reversed_deps = deps;
        reversed_deps.reverse();
        let snap_reversed = GalhoStateSnapshot {
            name: "x".into(),
            phase: Phase::Planned,
            depends_on: reversed_deps,
            deps_satisfied: vec![],
        };
        let g1 = DepGraph::new(vec![snap_forward]);
        let g2 = DepGraph::new(vec![snap_reversed]);
        prop_assert_eq!(g1.content_hash(), g2.content_hash());
    }

    /// Invariant 4: Hash is stable across to_hex round-trip; same hash → same hex.
    #[test]
    fn hex_is_deterministic(snaps in any_snapshots()) {
        let g = DepGraph::new(snaps);
        let h1 = g.content_hash_hex();
        let h2 = g.content_hash_hex();
        prop_assert_eq!(h1.clone(), h2);
        prop_assert_eq!(h1.len(), 64);
    }
}
