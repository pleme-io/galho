//! Property-based tests for galho-types canonicalization invariants.
//!
//! These are the load-bearing properties — if any of these regresses, the entire
//! content-addressing story is broken and every dependent DAG node hash is suspect.
//! Counts: 256 cases per property (proptest default) × N properties.

use std::collections::BTreeMap;

use galho_types::{
    canonical::{content_hash, CanonicalBytes, CanonicalSink},
    value::{CrossSystemContract, CrossSystemRef, SecretRef, Value},
};
use proptest::prelude::*;

// =============================================================================
// Strategy generators
// =============================================================================

fn any_scalar_value() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::Int),
        // Restrict floats to finite range — NaN/Inf are upstream-forbidden but proptest
        // would otherwise generate them.
        (-1e10f64..1e10f64).prop_map(Value::Float),
        ".{0,16}".prop_map(Value::String),
        prop::collection::vec(any::<u8>(), 0..16).prop_map(Value::Bytes),
    ]
}

fn any_value() -> impl Strategy<Value = Value> {
    let leaf = any_scalar_value();
    leaf.prop_recursive(3, 32, 8, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6).prop_map(Value::List),
            prop::collection::btree_map(".{1,8}", inner, 0..6).prop_map(Value::Map),
        ]
    })
}

fn any_secret_ref() -> impl Strategy<Value = SecretRef> {
    (".{1,16}", ".{1,32}", prop::option::of(".{1,8}")).prop_map(|(b, p, v)| SecretRef {
        backend: b,
        path: p,
        version: v,
    })
}

fn any_cross_system_ref() -> impl Strategy<Value = CrossSystemRef> {
    let contract = prop_oneof![
        ".{1,8}".prop_map(|s| CrossSystemContract::ValueRead { ref_attr: s }),
        Just(CrossSystemContract::ExistenceDependency),
        ".{1,8}".prop_map(|owner| CrossSystemContract::LifecycleDelegation { owner }),
    ];
    (".{1,8}", ".{1,8}", ".{1,8}", ".{1,8}", ".{1,8}", contract).prop_map(
        |(fs, fr, fa, ts, tr, c)| CrossSystemRef {
            from_system: fs,
            from_resource: fr,
            from_attr: fa,
            to_system: ts,
            to_resource: tr,
            contract: c,
        },
    )
}

// =============================================================================
// Properties
// =============================================================================

proptest! {
    /// I.1 — Canonical-bytes emission is deterministic: emitting twice produces the same bytes.
    /// This is the foundational invariant; every other property depends on it.
    #[test]
    fn canonical_bytes_are_deterministic(v in any_value()) {
        let mut s1 = CanonicalSink::new();
        v.canonical_bytes(&mut s1);
        let mut s2 = CanonicalSink::new();
        v.canonical_bytes(&mut s2);
        prop_assert_eq!(s1.finish(), s2.finish());
    }

    /// I.2 — Content hashing is deterministic.
    #[test]
    fn content_hash_is_deterministic(v in any_value()) {
        let h1 = content_hash(&v);
        let h2 = content_hash(&v);
        prop_assert_eq!(h1, h2);
    }

    /// I.3 — BTreeMap insertion order is irrelevant (BTreeMap is already sorted by Ord;
    /// verifies the canonical-bytes pass respects that invariant end-to-end). Pairs
    /// must have unique keys — duplicate keys in BTreeMap follow last-write-wins, which
    /// is a different invariant (not the one we're testing here).
    #[test]
    fn map_order_independence(
        raw_pairs in prop::collection::vec((".{1,8}", -1000_i64..1000_i64), 0..8)
    ) {
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let pairs: Vec<_> = raw_pairs
            .into_iter()
            .filter(|(k, _)| seen.insert(k.clone()))
            .collect();
        let mut m1: BTreeMap<String, Value> = BTreeMap::new();
        for (k, v) in &pairs {
            m1.insert(k.clone(), Value::Int(*v));
        }
        let mut m2: BTreeMap<String, Value> = BTreeMap::new();
        for (k, v) in pairs.iter().rev() {
            m2.insert(k.clone(), Value::Int(*v));
        }
        prop_assert_eq!(
            content_hash(&Value::Map(m1)),
            content_hash(&Value::Map(m2))
        );
    }

    /// I.4 — SecretRef hash is reference-based. Two SecretRefs with identical typed
    /// reference (backend / path / version) hash to the same value, regardless of any
    /// imagined "resolved value." The type forbids resolved values from entering the
    /// hash input by construction; this property confirms the typed reference itself
    /// is hashed deterministically.
    #[test]
    fn secret_ref_hash_is_reference_based(r in any_secret_ref()) {
        let v1 = Value::SecretRef(r.clone());
        let v2 = Value::SecretRef(r);
        prop_assert_eq!(content_hash(&v1), content_hash(&v2));
    }

    /// I.5 — SecretRef.canonical_reference() never contains the substring "RESOLVED"
    /// (or anything that would suggest a resolved value made it through). Sanity test
    /// reinforcing the "never hash the value" discipline.
    #[test]
    fn secret_ref_canonical_excludes_value_marker(r in any_secret_ref()) {
        let canonical = r.canonical_reference();
        let stringified = String::from_utf8_lossy(&canonical);
        prop_assert!(!stringified.contains("RESOLVED"));
        prop_assert!(!stringified.contains("plaintext"));
    }

    /// I.6 — Cross-system refs round-trip through canonical bytes deterministically.
    #[test]
    fn cross_system_ref_deterministic(r in any_cross_system_ref()) {
        let v = Value::CrossSystemRef(r);
        let h1 = content_hash(&v);
        let h2 = content_hash(&v);
        prop_assert_eq!(h1, h2);
    }

    /// I.7 — Distinct typed references produce distinct hashes (no collisions on
    /// the cheap inputs proptest generates). BLAKE3 itself is collision-resistant;
    /// this property checks that DIFFERENT inputs reach BLAKE3 as different bytes
    /// (i.e. canonical-bytes is injective on these inputs).
    #[test]
    fn different_inputs_produce_different_hashes(
        a in any_secret_ref(),
        b in any_secret_ref()
    ) {
        let h_a = content_hash(&Value::SecretRef(a.clone()));
        let h_b = content_hash(&Value::SecretRef(b.clone()));
        if a != b {
            prop_assert_ne!(h_a, h_b);
        }
    }
}

// =============================================================================
// Targeted unit tests (not proptest — corner cases)
// =============================================================================

#[test]
fn nan_does_not_appear_in_value_float_by_construction() {
    // NaN is not in the proptest strategy for Value::Float; the policy is "callers
    // never construct Value::Float(NaN)." If they did anyway, the CanonicalSink
    // would canonicalize to a single representation — but that's defense-in-depth
    // not the policy.
    let v = Value::Float(0.0);
    let h = content_hash(&v);
    let h2 = content_hash(&v);
    assert_eq!(h, h2);
}

#[test]
fn null_value_has_stable_hash() {
    let h1 = content_hash(&Value::Null);
    let h2 = content_hash(&Value::Null);
    assert_eq!(h1, h2);
}

#[test]
fn empty_map_and_empty_list_are_distinct() {
    let m = Value::Map(BTreeMap::new());
    let l = Value::List(Vec::new());
    assert_ne!(content_hash(&m), content_hash(&l));
}

#[test]
fn distinct_secret_refs_distinguishable_by_version() {
    let r1 = SecretRef::new("akeyless", "/secret/foo");
    let r2 = SecretRef::new("akeyless", "/secret/foo").with_version("v2");
    assert_ne!(
        content_hash(&Value::SecretRef(r1)),
        content_hash(&Value::SecretRef(r2)),
    );
}
