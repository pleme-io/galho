//! Property-based tests for the Plan derivation algebra.
//!
//! The load-bearing invariant: `apply_changes(g, diff_graphs(g, g')) == g'`.
//! If this regresses, plan-as-typed-value is broken and every replay / rebase /
//! adapter-driven apply becomes suspect.
//!
//! Builders + strategies extracted to `galho_types::testing`.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use galho_types::{
    apply_changes, diff_attrs, diff_graphs,
    testing::{any_graph, empty_graph, graph_with, make_resource},
    AttrPath, ResourceId, TypedChange, Value, ValueDiff,
};
use proptest::prelude::*;

// =============================================================================
// Properties
// =============================================================================

proptest! {
    /// D.1 — Round-trip identity: applying the diff back to the base gives the target.
    /// THE load-bearing property for plan-as-typed-value. Verified for arbitrary
    /// graph pairs.
    #[test]
    fn diff_then_apply_is_identity(base in any_graph(), target in any_graph()) {
        let mut target = target;
        target.root = base.root.clone();   // align roots
        let changes = diff_graphs(&base, &target);
        let result = apply_changes(&base, &changes);
        // Resources match.
        prop_assert_eq!(&result.resources, &target.resources);
        // Edges: apply_changes filters edges to those between live resources; both
        // base + target have empty edge sets in this strategy, so the result also
        // has empty edges. (Edge round-trip is a separate property; see D.5.)
    }

    /// D.2 — Empty diff is no-op: diffing a graph with itself yields no changes.
    #[test]
    fn diff_self_is_empty(g in any_graph()) {
        let changes = diff_graphs(&g, &g);
        prop_assert!(changes.is_empty());
    }

    /// D.3 — Empty changes are no-op: applying the empty change list yields the base.
    #[test]
    fn apply_empty_is_identity(g in any_graph()) {
        let result = apply_changes(&g, &[]);
        prop_assert_eq!(&result.resources, &g.resources);
    }

    /// D.4 — Diff is informative: if base != target (resource-set differs), the diff is
    /// non-empty. (Same resource-set with different attributes also produces a non-empty
    /// diff, but the assertion is harder to phrase generically; this is the easy half.)
    #[test]
    fn diff_is_nonempty_when_resource_sets_differ(
        base in any_graph(),
        target in any_graph()
    ) {
        let mut target = target;
        target.root = base.root.clone();
        if base.resources.keys().collect::<BTreeSet<_>>() != target.resources.keys().collect::<BTreeSet<_>>() {
            let changes = diff_graphs(&base, &target);
            prop_assert!(!changes.is_empty());
        }
    }
}

// =============================================================================
// Targeted unit tests (corner cases)
// =============================================================================

#[test]
fn diff_create_resource() {
    let base = empty_graph();
    let target = graph_with(vec![make_resource(
        "vpc",
        vec![("cidr", Value::String("10.0.0.0/8".into()))],
    )]);
    let changes = diff_graphs(&base, &target);
    assert_eq!(changes.len(), 1);
    assert!(matches!(changes[0], TypedChange::Create { .. }));
}

#[test]
fn diff_delete_resource() {
    let base = graph_with(vec![make_resource("vpc", vec![])]);
    let target = empty_graph();
    let changes = diff_graphs(&base, &target);
    assert_eq!(changes.len(), 1);
    assert!(matches!(changes[0], TypedChange::Delete { .. }));
}

#[test]
fn diff_update_resource_attribute() {
    let base = graph_with(vec![make_resource("rds", vec![("size", Value::Int(100))])]);
    let target = graph_with(vec![make_resource("rds", vec![("size", Value::Int(200))])]);
    let changes = diff_graphs(&base, &target);
    assert_eq!(changes.len(), 1);
    let change = &changes[0];
    let TypedChange::Update { attr_diff, .. } = change else {
        panic!("expected Update, got {change:?}");
    };
    assert_eq!(attr_diff.len(), 1);
    let v = attr_diff.get(&AttrPath::new(["size"])).unwrap();
    assert!(matches!(
        v,
        ValueDiff::Changed {
            before: Value::Int(100),
            after: Value::Int(200)
        }
    ));
}

#[test]
fn apply_create_then_update_composes() {
    let g0 = empty_graph();
    let create = TypedChange::Create {
        resource: make_resource("rds", vec![("size", Value::Int(100))]),
    };
    let update = TypedChange::Update {
        resource_id: ResourceId::new("rds"),
        before: make_resource("rds", vec![("size", Value::Int(100))]),
        after: make_resource("rds", vec![("size", Value::Int(200))]),
        attr_diff: BTreeMap::new(),
    };
    let g1 = apply_changes(&g0, &[create, update]);
    let final_rds = g1.resources.get(&ResourceId::new("rds")).unwrap();
    assert_eq!(
        final_rds.attrs.get(&AttrPath::new(["size"])),
        Some(&Value::Int(200))
    );
}

#[test]
fn apply_delete_removes_resource() {
    let g0 = graph_with(vec![make_resource("rds", vec![])]);
    let delete = TypedChange::Delete {
        resource_id: ResourceId::new("rds"),
        before: make_resource("rds", vec![]),
    };
    let g1 = apply_changes(&g0, &[delete]);
    assert!(g1.resources.is_empty());
}

#[test]
fn diff_attrs_handles_added_removed_changed() {
    let mut before = BTreeMap::new();
    before.insert(AttrPath::new(["a"]), Value::Int(1));
    before.insert(AttrPath::new(["b"]), Value::Int(2));

    let mut after = BTreeMap::new();
    after.insert(AttrPath::new(["a"]), Value::Int(99)); // changed
    after.insert(AttrPath::new(["c"]), Value::Int(3));  // added
    // "b" removed.

    let diff = diff_attrs(&before, &after);
    assert_eq!(diff.len(), 3);
    assert!(matches!(
        diff.get(&AttrPath::new(["a"])).unwrap(),
        ValueDiff::Changed { .. }
    ));
    assert!(matches!(
        diff.get(&AttrPath::new(["b"])).unwrap(),
        ValueDiff::Removed(_)
    ));
    assert!(matches!(
        diff.get(&AttrPath::new(["c"])).unwrap(),
        ValueDiff::Added(_)
    ));
}
