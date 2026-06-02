//! §VI.4 — the always-restable comfort-matrix forcing-function test.
//!
//! ECLUSA §VI's golden-path-attraction law: *every reachable `Phase` is a
//! comfortable berth* — a change can rest there indefinitely without holding an
//! environment hostage, while always retaining a surgery-free exit toward a good
//! terminal. This test makes that law mechanical (the keystone the eclusa
//! orchestrator is built on):
//!
//! 1. **Totality** — the comfort matrix is a bijection with `Phase::all()`; a
//!    new `Phase` variant without a comfort row fails CI.
//! 2. **Surgery-free exit** — every non-terminal berth has a real outgoing FSM
//!    edge (the comfort claim can't outrun the state machine).
//! 3. **Defect ledger** — the `Fails` cells match the tracked A.1/A.2 fixes
//!    exactly; resolving one without flipping its cell (or a new failure) fails.
//! 4. **Cost ↔ hostage law** — `ParkCost` and the `no_hostage` tier can't drift.
//! 5. **Good terminals** — `Done`/`Destroyed` are maximally comfortable.

use galho_types::comfort::{COMFORT_MATRIX, ComfortTier, ParkCost, comfort_of};
use galho_types::phase::{Phase, PhaseClass, transitions_from};

/// Totality: exactly one `ComfortRow` per `Phase`, no orphans, no duplicates.
/// A new `Phase` variant without a row fails here — *this* is the forcing
/// function that keeps §VI.2 coverage total by construction.
#[test]
fn every_phase_has_exactly_one_comfort_row() {
    for p in Phase::all() {
        let n = COMFORT_MATRIX.iter().filter(|r| r.phase == *p).count();
        assert_eq!(n, 1, "Phase {p:?} must have exactly one ComfortRow, found {n}");
    }
    assert_eq!(
        COMFORT_MATRIX.len(),
        Phase::all().len(),
        "COMFORT_MATRIX ({}) must have exactly one row per Phase ({}) — a new Phase needs a §VI.2 berth row",
        COMFORT_MATRIX.len(),
        Phase::all().len(),
    );
    for r in COMFORT_MATRIX {
        assert!(Phase::all().contains(&r.phase), "orphan ComfortRow for {:?}", r.phase);
        assert!(comfort_of(r.phase).is_some(), "comfort_of({:?}) round-trip failed", r.phase);
    }
}

/// Surgery-free exit: every non-terminal berth has ≥1 real outgoing FSM edge;
/// terminals have none. Ties the comfort matrix to the state machine so a berth
/// can never *claim* comfort while being a dead-end (golden attraction's floor).
#[test]
fn every_non_terminal_berth_has_a_surgery_free_exit() {
    for p in Phase::all() {
        let outgoing = transitions_from(*p);
        if p.class() == PhaseClass::Terminal {
            assert!(
                outgoing.is_empty(),
                "terminal {p:?} must have no outgoing edges, found {}",
                outgoing.len()
            );
        } else {
            assert!(
                !outgoing.is_empty(),
                "non-terminal berth {p:?} must offer a surgery-free exit, found none"
            );
        }
    }
}

/// The live defect ledger: the set of as-shipped berth failures is *exactly* the
/// two tracked fixes. A regression that introduces a new `Fails` is caught; a
/// fix (A.1/A.2) landed without flipping its cell — or a cell flipped without
/// landing the fix — is caught. The matrix is the single source of truth for
/// "which berths aren't comfortable yet."
#[test]
fn comfort_failures_are_exactly_the_tracked_set() {
    let mut got: Vec<(Phase, &'static str, &'static str)> = COMFORT_MATRIX
        .iter()
        .flat_map(|r| r.failures().into_iter().map(move |(crit, fix)| (r.phase, crit, fix)))
        .collect();
    got.sort();

    let mut want = vec![
        // A.1 — RolledBack holds the StackLock at non-terminal rest (release_holder
        // fires only on terminal/Abandon) → starves same-stack siblings.
        (Phase::RolledBack, "no_hostage", "A.1"),
        // A.2 — multi-IaC promote can leave the shared root half-applied with no
        // atomic-root-commit barrier across IaC systems.
        (Phase::Merged, "no_env_flow", "A.2"),
    ];
    want.sort();

    assert_eq!(
        got, want,
        "\nas-shipped berth failures drifted from the tracked set.\n  got:  {got:?}\n  want: {want:?}\n\
         (landing A.1/A.2 → flip the cell to Guaranteed/MitigatedConfig in comfort.rs AND remove its row here;\n\
          a new failure → add it here with a tracked fix id)"
    );
}

/// `ParkCost` ↔ `no_hostage` consistency law: a `None`-cost berth holds no lock,
/// so it can't claim a TTL-bounded-lock mitigation; a `TtlBounded` berth holds a
/// lock, so it can't claim a `Guaranteed` no-hostage. Cost and comfort stay tied.
#[test]
fn park_cost_and_hostage_tier_are_consistent() {
    for r in COMFORT_MATRIX {
        match r.park_cost {
            ParkCost::None => assert_ne!(
                r.no_hostage,
                ComfortTier::MitigatedTtl,
                "{:?}: ParkCost::None holds no lock, so no_hostage can't be MitigatedTtl",
                r.phase
            ),
            ParkCost::TtlBounded => assert_ne!(
                r.no_hostage,
                ComfortTier::Guaranteed,
                "{:?}: ParkCost::TtlBounded holds a lock, so no_hostage can't be Guaranteed",
                r.phase
            ),
            ParkCost::Transient => {}
        }
    }
}

/// The two good terminals (the closed absorbing set) are maximally comfortable:
/// all four criteria `Guaranteed`, `None`-cost — arrival *is* the surgery-free exit.
#[test]
fn good_terminals_are_maximally_comfortable() {
    for p in [Phase::Done, Phase::Destroyed] {
        let r = comfort_of(p).expect("terminal has a comfort row");
        assert_eq!(r.park_cost, ParkCost::None, "{p:?} terminal must be None-cost");
        assert!(r.is_comfortable(), "{p:?} terminal must be comfortable");
        for (crit, tier) in r.criteria() {
            assert_eq!(tier, ComfortTier::Guaranteed, "{p:?}.{crit} must be Guaranteed");
        }
    }
}

/// No silent middle: a berth is either comfortable, or it names ≥1 tracked fix.
#[test]
fn every_berth_is_comfortable_or_names_a_fix() {
    for r in COMFORT_MATRIX {
        if !r.is_comfortable() {
            assert!(!r.failures().is_empty(), "{:?} not comfortable but names no fix", r.phase);
        }
    }
}
