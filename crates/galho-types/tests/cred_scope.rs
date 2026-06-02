//! §III.6 / §XIX.5 — credential-safety forcing-function test.
//!
//! Proves the safety half of golden-path attraction: a cloud mutation requires
//! a write credential, a plan/observe needs only read, and the per-edge required
//! scope can never disagree with the golden/ceiling classification.

use galho_types::cred::{CeilingCrossing, CredScope, CrossingRefused, RequiredScope, required_scope};
use galho_types::flow_tier::{Ceiling, EdgeTier, edge_tier};
use galho_types::phase::{MorphismId, Phase, transition_table};
use galho_types::{BoundedApplyTtl, SecretRef};
use time::Duration;

fn read_cred() -> CredScope {
    CredScope::ReadOnlyPlan { cofre_ref: SecretRef::new("akeyless", "/plan") }
}
fn write_cred() -> CredScope {
    CredScope::WriteApply {
        cofre_ref: SecretRef::new("akeyless", "/apply"),
        env: "prod".into(),
        ttl: BoundedApplyTtl::try_new(Duration::hours(1)).expect("1h within ApplyTtlBounds"),
    }
}

/// Satisfaction lattice: `Write` provides both; `ReadOnly` provides only reads.
#[test]
fn required_scope_satisfaction_lattice() {
    assert!(RequiredScope::ReadOnly.satisfied_by(RequiredScope::ReadOnly));
    assert!(RequiredScope::ReadOnly.satisfied_by(RequiredScope::Write));
    assert!(RequiredScope::Write.satisfied_by(RequiredScope::Write));
    assert!(!RequiredScope::Write.satisfied_by(RequiredScope::ReadOnly), "read cannot satisfy write");
}

/// CredScope reports the scope it provides.
#[test]
fn cred_scope_provides_the_right_scope() {
    assert_eq!(read_cred().provides(), RequiredScope::ReadOnly);
    assert_eq!(write_cred().provides(), RequiredScope::Write);
}

/// `required_scope` is total over the shipped table and matches `flow_tier`
/// exactly: `Write` iff the edge is `CeilingCrossing(NonTransactionalIo)`.
/// Tally: 12 Write (the NTIO crossings), 11 ReadOnly (9 golden + 2 ExtObs).
#[test]
fn required_scope_is_total_and_matches_flow_tier() {
    let (mut write, mut read) = (0usize, 0usize);
    for t in transition_table() {
        let req = required_scope(t.from, t.morphism)
            .unwrap_or_else(|| panic!("edge {:?}/{:?} has no required scope", t.from, t.morphism));
        let tier = edge_tier(t.from, t.morphism).expect("table edge has a tier");
        let needs_write = matches!(tier, EdgeTier::CeilingCrossing(Ceiling::NonTransactionalIo));
        assert_eq!(
            req == RequiredScope::Write,
            needs_write,
            "edge {:?}/{:?}: required {req:?} disagrees with tier {tier:?}",
            t.from,
            t.morphism
        );
        if req == RequiredScope::Write {
            write += 1;
        } else {
            read += 1;
        }
    }
    assert_eq!(write, 12, "12 NonTransactionalIo crossings need Write");
    assert_eq!(read, 11, "9 golden + 2 ExternalObservation edges need ReadOnly");
}

/// `required_scope` is `None` for non-edges.
#[test]
fn required_scope_none_for_non_edges() {
    assert_eq!(required_scope(Phase::Merged, MorphismId::Plan), None);
}

/// `authorize` refuses golden edges (they need no witness).
#[test]
fn authorize_refuses_golden_edges() {
    let r = CeilingCrossing::authorize(Phase::Declared, MorphismId::Plan, &read_cred());
    assert_eq!(r.unwrap_err(), CrossingRefused::EdgeIsGolden(Phase::Declared, MorphismId::Plan));
}

/// `authorize` refuses non-edges.
#[test]
fn authorize_refuses_non_edges() {
    let r = CeilingCrossing::authorize(Phase::Merged, MorphismId::Plan, &write_cred());
    assert_eq!(r.unwrap_err(), CrossingRefused::NoSuchEdge(Phase::Merged, MorphismId::Plan));
}

/// `authorize` refuses a read credential on a write (apply) crossing.
#[test]
fn authorize_refuses_underscoped_credential() {
    let r = CeilingCrossing::authorize(Phase::Planned, MorphismId::ApplyToPreview, &read_cred());
    assert_eq!(
        r.unwrap_err(),
        CrossingRefused::InsufficientCred {
            required: RequiredScope::Write,
            provided: RequiredScope::ReadOnly
        }
    );
}

/// `authorize` mints a witness for the `#2` apply crossing with a write cred.
#[test]
fn authorize_mints_for_write_crossing() {
    let w = CeilingCrossing::authorize(Phase::Planned, MorphismId::ApplyToPreview, &write_cred())
        .expect("write cred authorizes the apply crossing");
    assert_eq!(w.ceiling(), Ceiling::NonTransactionalIo);
    assert_eq!(w.edge(), (Phase::Planned, MorphismId::ApplyToPreview));
}

/// A read cred suffices for a read-only `ExternalObservation` crossing
/// (drift detect) — least-privilege, refining §XIX.5's blanket WriteApply.
#[test]
fn authorize_mints_for_read_only_crossing_with_read_cred() {
    let w = CeilingCrossing::authorize(Phase::Verified, MorphismId::DriftReconcile, &read_cred())
        .expect("read cred suffices for a drift-observe crossing");
    assert_eq!(w.ceiling(), Ceiling::ExternalObservation);
}

/// `satisfies_edge` least-privilege: WriteApply satisfies every crossing;
/// ReadOnlyPlan satisfies golden + ExternalObservation but NOT NonTransactionalIo.
#[test]
fn satisfies_edge_is_least_privilege() {
    let (read, write) = (read_cred(), write_cred());
    // apply (NTIO): write yes, read no.
    assert!(write.satisfies_edge(Phase::Planned, MorphismId::ApplyToPreview));
    assert!(!read.satisfies_edge(Phase::Planned, MorphismId::ApplyToPreview));
    // drift observe (ExtObs): both yes.
    assert!(write.satisfies_edge(Phase::Verified, MorphismId::DriftReconcile));
    assert!(read.satisfies_edge(Phase::Verified, MorphismId::DriftReconcile));
    // plan (golden): both yes.
    assert!(read.satisfies_edge(Phase::Declared, MorphismId::Plan));
}
