//! Tests for the `TypedState<S>` serde re-tag — closes the type-confusion hole.
//!
//! Bytes written for one IaC system must NOT decode into `TypedState<OtherSystem>`.
//! Deserialization asserts `meta.iac_system == S::id()`; serialization is
//! unchanged so on-disk bytes + content hashes stay stable.
//!
//! Tests live under tests/ → exempt from the format!() ratchet.

use std::collections::{BTreeMap, BTreeSet};

use galho_types::{
    ApplySemantics, GraphRoot, IaCSystem, IaCSystemId, ResourceGraph, TypedState,
    TypedStateDecodeError,
};
use galho_types::{AdapterState, Passaporte, StateMeta};
use time::OffsetDateTime;

/// An empty `ResourceGraph` whose root declares the given IaC system.
fn empty_graph(system: &str) -> ResourceGraph {
    ResourceGraph {
        root: GraphRoot {
            iac_system: system.to_string(),
            schema_version: "0.1.0".into(),
        },
        resources: BTreeMap::new(),
        edges: BTreeSet::new(),
    }
}

#[derive(Debug, Clone, PartialEq)]
struct TestSystem;
impl IaCSystem for TestSystem {
    fn id() -> IaCSystemId {
        IaCSystemId::new("test")
    }
    fn schema_version() -> &'static str {
        "0.1.0"
    }
    fn apply_semantics() -> ApplySemantics {
        ApplySemantics::PartialProgress
    }
}

#[derive(Debug, Clone, PartialEq)]
struct CrossplaneTestSystem;
impl IaCSystem for CrossplaneTestSystem {
    fn id() -> IaCSystemId {
        IaCSystemId::new("crossplane")
    }
    fn schema_version() -> &'static str {
        "0.1.0"
    }
    fn apply_semantics() -> ApplySemantics {
        ApplySemantics::ContinuousReconciliation
    }
}

fn meta_for(system: &str) -> StateMeta {
    StateMeta {
        created_at: OffsetDateTime::UNIX_EPOCH,
        created_by: Passaporte::new("system"),
        parent_hashes: vec![],
        iac_system: system.to_string(),
        galho_name: "galho-x".into(),
        commit_message: None,
    }
}

#[test]
fn typed_state_roundtrips_for_matching_system() {
    let state = TypedState::<TestSystem>::new(
        empty_graph("test"),
        AdapterState::default(),
        meta_for("test"),
    );
    let bytes = serde_json::to_vec(&state).unwrap();
    let back: TypedState<TestSystem> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(back, state);
}

#[test]
fn typed_state_refuses_wrong_system_bytes() {
    // Bytes written for the "crossplane" system must NOT decode as TypedState<TestSystem>.
    let crossplane_state = TypedState::<CrossplaneTestSystem>::new(
        empty_graph("crossplane"),
        AdapterState::default(),
        meta_for("crossplane"),
    );
    let bytes = serde_json::to_vec(&crossplane_state).unwrap();

    let result: Result<TypedState<TestSystem>, _> = serde_json::from_slice(&bytes);
    let err = result.expect_err("crossplane bytes must not decode as TypedState<TestSystem>");
    // serde wraps the TryFrom error message; assert it mentions the mismatch.
    let msg = err.to_string();
    assert!(
        msg.contains("system mismatch") || msg.contains("crossplane") || msg.contains("test"),
        "expected system-mismatch error, got: {msg}"
    );
}

#[test]
fn typed_state_decode_error_names_both_systems() {
    // Direct exercise of the typed error via the public TryFrom path is internal;
    // assert the error type's Display shape is stable for downstream matchers.
    let err = TypedStateDecodeError::SystemMismatch {
        expected: "test".into(),
        found: "crossplane".into(),
    };
    let msg = err.to_string();
    assert!(msg.contains("test"));
    assert!(msg.contains("crossplane"));
}
