//! Public builders + proptest strategies for galho-types consumers.
//!
//! Gated behind the `testing` feature flag — off by default for production builds
//! (proptest is not pulled in). Downstream test crates + adapter integration tests
//! opt in via:
//!
//! ```toml
//! [dev-dependencies]
//! galho-types = { workspace = true, features = ["testing"] }
//! ```
//!
//! Compounding rationale: every adapter (galho-terraform, galho-helm, galho-crossplane,
//! …) will need to construct `Resource` / `ResourceGraph` / `Value` fixtures in its own
//! test suite. Centralizing the builders here ensures every adapter speaks one fixture
//! vocabulary and prevents reproducing the same 80-line helper file N times.

use std::collections::{BTreeMap, BTreeSet};

use proptest::prelude::*;

use crate::ir::{GraphRoot, Provenance, Resource, ResourceGraph, ResourceId, ResourceKind, ResourceStatus};
use crate::value::Value;
use crate::AttrPath;

// =============================================================================
// Builders
// =============================================================================

/// A graph root with `iac_system = "test"`, `schema_version = "0.1.0"`. Useful for
/// algebra tests that don't care which IaCSystem is involved.
#[must_use]
pub fn test_root() -> GraphRoot {
    GraphRoot {
        iac_system: "test".into(),
        schema_version: "0.1.0".into(),
    }
}

/// An empty `ResourceGraph` with the canonical test root.
#[must_use]
pub fn empty_graph() -> ResourceGraph {
    ResourceGraph {
        root: test_root(),
        resources: BTreeMap::new(),
        edges: BTreeSet::new(),
    }
}

/// Build a `Resource` with an id, a single attribute path per `(key, value)` pair,
/// pending status, and default provenance. Kind defaults to `"test_kind"`.
#[must_use]
pub fn make_resource(id: &str, attrs: Vec<(&str, Value)>) -> Resource {
    let mut attr_map = BTreeMap::new();
    for (k, v) in attrs {
        attr_map.insert(AttrPath::new([k]), v);
    }
    Resource {
        id: ResourceId::new(id),
        kind: ResourceKind::new("test_kind"),
        attrs: attr_map,
        deps: BTreeSet::new(),
        status: ResourceStatus::Pending,
        provenance: Provenance::default(),
    }
}

/// A `ResourceGraph` containing the given resources. Edges are empty; tests that need
/// edges should construct them explicitly via `g.edges.insert(...)`.
#[must_use]
pub fn graph_with(resources: Vec<Resource>) -> ResourceGraph {
    let mut g = empty_graph();
    for r in resources {
        g.resources.insert(r.id.clone(), r);
    }
    g
}

// =============================================================================
// Proptest strategies
// =============================================================================

/// Any scalar `Value` (no nested collections, no SecretRef / CrossSystemRef). Floats
/// restricted to a finite range so NaN never appears.
pub fn any_scalar_value() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::Int),
        (-1e10f64..1e10f64).prop_map(Value::Float),
        ".{0,16}".prop_map(Value::String),
        prop::collection::vec(any::<u8>(), 0..16).prop_map(Value::Bytes),
    ]
}

/// Any `Value` up to a small recursive depth. Excludes SecretRef / CrossSystemRef.
pub fn any_value() -> impl Strategy<Value = Value> {
    let leaf = any_scalar_value();
    leaf.prop_recursive(3, 32, 8, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6).prop_map(Value::List),
            prop::collection::btree_map(".{1,8}", inner, 0..6).prop_map(Value::Map),
        ]
    })
}

/// A `Resource` with an id drawn from `0..id_pool_size` (so generated graphs have
/// overlapping resource sets — useful for merge / diff property tests). Attributes
/// are scalar-only to keep generation cheap.
pub fn any_resource(id_pool_size: usize) -> impl Strategy<Value = Resource> {
    let id = (0..id_pool_size).prop_map(|n| format!("r{n}"));
    let attrs = prop::collection::vec((".{1,4}", any_scalar_value()), 0..4);
    (id, attrs).prop_map(|(id, kvs)| {
        let mut map = BTreeMap::new();
        for (k, v) in kvs {
            map.insert(AttrPath::new([k]), v);
        }
        Resource {
            id: ResourceId::new(id),
            kind: ResourceKind::new("test_kind"),
            attrs: map,
            deps: BTreeSet::new(),
            status: ResourceStatus::Pending,
            provenance: Provenance::default(),
        }
    })
}

/// A small `ResourceGraph` (up to 4 resources, drawn from an id pool of size 4 so
/// adjacent graphs overlap). Suitable for proptest of merge / diff / apply.
pub fn any_graph() -> impl Strategy<Value = ResourceGraph> {
    prop::collection::vec(any_resource(4), 0..4).prop_map(|rs| {
        let mut by_id: BTreeMap<ResourceId, Resource> = BTreeMap::new();
        for r in rs {
            by_id.insert(r.id.clone(), r);
        }
        ResourceGraph {
            root: test_root(),
            resources: by_id,
            edges: BTreeSet::new(),
        }
    })
}
