//! Typed JSON output: third render surface for the snapshot algebra.
//!
//! Invariants:
//!   1. GalhoStateSnapshot round-trips through serde unchanged.
//!   2. DepGraph::to_json_value() emits {nodes, edges} where edges carry
//!      the typed `satisfied` flag mirroring mermaid/dot edge styling.
//!   3. The JSON shape stays kebab-stable so external consumers don't break.

use galho_cli::{DepGraph, DepGraphJsonEdge, GalhoStateSnapshot};
use galho_types::Phase;
use serde_json::json;

fn sample_graph() -> DepGraph {
    DepGraph::new(vec![
        GalhoStateSnapshot {
            name: "feat-a".into(),
            phase: Phase::Verified,
            depends_on: vec![],
            deps_satisfied: vec![],
        },
        GalhoStateSnapshot {
            name: "feat-b".into(),
            phase: Phase::Declared,
            depends_on: vec!["feat-a".into()],
            deps_satisfied: vec!["feat-a".into()],
        },
        GalhoStateSnapshot {
            name: "feat-c".into(),
            phase: Phase::Planned,
            depends_on: vec!["feat-a".into(), "feat-b".into()],
            deps_satisfied: vec!["feat-a".into()],
        },
    ])
}

#[test]
fn snapshot_serde_round_trip() {
    let original = GalhoStateSnapshot {
        name: "alpha".into(),
        phase: Phase::AppliedPendingReview,
        depends_on: vec!["upstream".into()],
        deps_satisfied: vec![],
    };
    let json = serde_json::to_string(&original).unwrap();
    let parsed: GalhoStateSnapshot = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, original);
}

#[test]
fn snapshot_json_field_names() {
    let snap = GalhoStateSnapshot {
        name: "a".into(),
        phase: Phase::Declared,
        depends_on: vec!["b".into()],
        deps_satisfied: vec![],
    };
    let value: serde_json::Value = serde_json::to_value(&snap).unwrap();
    let obj = value.as_object().unwrap();
    // External consumers depend on these keys; assert they exist.
    assert!(obj.contains_key("name"));
    assert!(obj.contains_key("phase"));
    assert!(obj.contains_key("depends_on"));
    assert!(obj.contains_key("deps_satisfied"));
}

#[test]
fn graph_to_json_has_nodes_and_edges() {
    let g = sample_graph();
    let value = g.to_json_value();
    let obj = value.as_object().unwrap();
    assert!(obj.contains_key("nodes"), "missing nodes key");
    assert!(obj.contains_key("edges"), "missing edges key");
    assert_eq!(obj["nodes"].as_array().unwrap().len(), 3);
    // 3 edges total: feat-a→feat-b, feat-a→feat-c, feat-b→feat-c
    assert_eq!(obj["edges"].as_array().unwrap().len(), 3);
}

#[test]
fn graph_json_edges_carry_satisfied_flag() {
    let g = sample_graph();
    let value = g.to_json_value();
    let edges = value["edges"].as_array().unwrap();
    // feat-a → feat-b: satisfied=true
    let ab = edges
        .iter()
        .find(|e| e["from"] == json!("feat-a") && e["to"] == json!("feat-b"))
        .expect("edge feat-a → feat-b");
    assert_eq!(ab["satisfied"], json!(true));
    // feat-b → feat-c: satisfied=false (feat-c hasn't satisfied feat-b yet)
    let bc = edges
        .iter()
        .find(|e| e["from"] == json!("feat-b") && e["to"] == json!("feat-c"))
        .expect("edge feat-b → feat-c");
    assert_eq!(bc["satisfied"], json!(false));
}

#[test]
fn graph_json_edge_count_matches_dep_declaration_count() {
    let g = sample_graph();
    let value = g.to_json_value();
    let total_deps: usize = g.snapshots.iter().map(|s| s.depends_on.len()).sum();
    assert_eq!(value["edges"].as_array().unwrap().len(), total_deps);
}

#[test]
fn empty_graph_has_empty_arrays_not_null() {
    let g = DepGraph::new(vec![]);
    let value = g.to_json_value();
    let obj = value.as_object().unwrap();
    assert_eq!(obj["nodes"].as_array().unwrap().len(), 0);
    assert_eq!(obj["edges"].as_array().unwrap().len(), 0);
}

#[test]
fn dep_graph_json_edge_serializes_as_typed_record() {
    let edge = DepGraphJsonEdge {
        from: "a".into(),
        to: "b".into(),
        satisfied: true,
    };
    let value = serde_json::to_value(&edge).unwrap();
    assert_eq!(value["from"], json!("a"));
    assert_eq!(value["to"], json!("b"));
    assert_eq!(value["satisfied"], json!(true));
}
