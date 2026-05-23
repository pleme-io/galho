//! Typed DepGraph renderer — Mermaid + DOT output via `Display` impls.
//!
//! Load-bearing invariants:
//!   1. Every galho appears as a node in both renderings.
//!   2. Every declared dep appears as a directed edge (dep → galho).
//!   3. Satisfied deps render with solid edges; unmet deps render dashed.
//!   4. Phase appears in every node label.
//!   5. Empty graph renders the typed empty-graph marker (`%%` for Mermaid,
//!      `//` for DOT).
//!   6. Non-alphanumeric characters in galho names sanitize to underscores
//!      in Mermaid IDs (Mermaid identifier rules).

use galho_cli::{DepGraph, GalhoStateSnapshot};
use galho_types::Phase;

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
            deps_satisfied: vec!["feat-a".into()], // satisfied
        },
        GalhoStateSnapshot {
            name: "feat-c".into(),
            phase: Phase::Planned,
            depends_on: vec!["feat-a".into(), "feat-b".into()],
            deps_satisfied: vec!["feat-a".into()], // feat-b unmet
        },
    ])
}

#[test]
fn mermaid_renders_all_nodes() {
    let g = sample_graph();
    let out = format!("{}", g.to_mermaid());
    assert!(out.starts_with("graph LR"));
    assert!(out.contains("feat_a"), "missing node feat_a");
    assert!(out.contains("feat_b"), "missing node feat_b");
    assert!(out.contains("feat_c"), "missing node feat_c");
}

#[test]
fn mermaid_renders_phase_labels() {
    let out = format!("{}", sample_graph().to_mermaid());
    assert!(out.contains("verified"), "missing phase verified");
    assert!(out.contains("declared"), "missing phase declared");
    assert!(out.contains("planned"), "missing phase planned");
}

#[test]
fn mermaid_satisfied_dep_is_solid_arrow() {
    let out = format!("{}", sample_graph().to_mermaid());
    // feat-a → feat-b is satisfied; expect solid arrow `-->`.
    assert!(
        out.contains("feat_a --> feat_b"),
        "expected solid edge feat_a → feat_b; got:\n{out}",
    );
}

#[test]
fn mermaid_unmet_dep_is_dashed_arrow() {
    let out = format!("{}", sample_graph().to_mermaid());
    // feat-b → feat-c is unmet (feat-c hasn't satisfied feat-b); expect dashed.
    assert!(
        out.contains("feat_b -.-> feat_c"),
        "expected dashed edge feat_b → feat_c; got:\n{out}",
    );
}

#[test]
fn mermaid_phase_class_assignment() {
    let out = format!("{}", sample_graph().to_mermaid());
    // feat-a is verified (terminal-ish but Verified isn't terminal; check is_terminal()).
    // feat-b's deps are satisfied → "ready"; feat-c blocked.
    assert!(out.contains(":::ready"), "expected at least one :::ready class");
    assert!(out.contains(":::blocked"), "expected at least one :::blocked class");
}

#[test]
fn dot_renders_all_nodes() {
    let out = format!("{}", sample_graph().to_dot());
    assert!(out.starts_with("digraph galho {"));
    assert!(out.contains("\"feat-a\""), "missing node feat-a");
    assert!(out.contains("\"feat-b\""), "missing node feat-b");
    assert!(out.contains("\"feat-c\""), "missing node feat-c");
    assert!(out.trim_end().ends_with('}'));
}

#[test]
fn dot_satisfied_edge_is_solid() {
    let out = format!("{}", sample_graph().to_dot());
    assert!(
        out.contains("\"feat-a\" -> \"feat-b\" [style=solid]"),
        "expected solid edge feat-a → feat-b; got:\n{out}",
    );
}

#[test]
fn dot_unmet_edge_is_dashed() {
    let out = format!("{}", sample_graph().to_dot());
    assert!(
        out.contains("\"feat-b\" -> \"feat-c\" [style=dashed]"),
        "expected dashed edge feat-b → feat-c; got:\n{out}",
    );
}

#[test]
fn empty_graph_renders_marker_mermaid() {
    let g = DepGraph::new(vec![]);
    let out = format!("{}", g.to_mermaid());
    assert!(out.contains("%% (no galhos)"));
}

#[test]
fn empty_graph_renders_marker_dot() {
    let g = DepGraph::new(vec![]);
    let out = format!("{}", g.to_dot());
    assert!(out.contains("// (no galhos)"));
}

#[test]
fn snapshots_are_sorted_by_name_on_construction() {
    let g = DepGraph::new(vec![
        GalhoStateSnapshot {
            name: "zeta".into(),
            phase: Phase::Declared,
            depends_on: vec![],
            deps_satisfied: vec![],
        },
        GalhoStateSnapshot {
            name: "alpha".into(),
            phase: Phase::Declared,
            depends_on: vec![],
            deps_satisfied: vec![],
        },
    ]);
    assert_eq!(g.snapshots[0].name, "alpha");
    assert_eq!(g.snapshots[1].name, "zeta");
}
