//! Typed `RenderState` — single source of truth across the four render surfaces.
//!
//! Load-bearing invariants:
//!   1. Classification table — terminal phase → Terminal regardless of deps;
//!      non-terminal + all deps satisfied → Ready; non-terminal + unmet → Blocked.
//!   2. Mermaid classDef name + DOT fillcolor + JSON `render_state` field
//!      all derive from the same RenderState method — surface consistency.
//!   3. Adding a new RenderState variant requires touching exactly one place
//!      (the enum + its impls); no renderer needs a separate branch.

use galho_cli::{DepGraph, GalhoStateSnapshot, RenderState};
use galho_types::Phase;

fn snap(name: &str, phase: Phase, deps: Vec<&str>, satisfied: Vec<&str>) -> GalhoStateSnapshot {
    GalhoStateSnapshot {
        name: name.into(),
        phase,
        depends_on: deps.into_iter().map(String::from).collect(),
        deps_satisfied: satisfied.into_iter().map(String::from).collect(),
    }
}

// ----- Classification truth-table -----

#[test]
fn terminal_phase_renders_terminal_regardless_of_deps() {
    // Done is terminal.
    let s = snap("a", Phase::Done, vec!["b"], vec![]); // unmet deps but terminal
    assert_eq!(s.render_state(), RenderState::Terminal);

    // Destroyed is terminal.
    let s = snap("a", Phase::Destroyed, vec!["b"], vec!["b"]); // satisfied + terminal
    assert_eq!(s.render_state(), RenderState::Terminal);
}

#[test]
fn non_terminal_all_deps_satisfied_renders_ready() {
    let s = snap("a", Phase::Verified, vec!["b", "c"], vec!["b", "c"]);
    assert_eq!(s.render_state(), RenderState::Ready);
}

#[test]
fn non_terminal_no_deps_renders_ready() {
    let s = snap("a", Phase::Declared, vec![], vec![]);
    assert_eq!(s.render_state(), RenderState::Ready);
}

#[test]
fn non_terminal_unmet_deps_renders_blocked() {
    let s = snap("a", Phase::Planned, vec!["b", "c"], vec!["b"]); // c unmet
    assert_eq!(s.render_state(), RenderState::Blocked);
}

// ----- Surface consistency: Mermaid + DOT + JSON all derive from RenderState -----

#[test]
fn mermaid_class_matches_render_state() {
    let g = DepGraph::new(vec![snap("a", Phase::Done, vec![], vec![])]);
    let mermaid = format!("{}", g.to_mermaid());
    // Terminal phase → :::terminal class
    assert!(
        mermaid.contains(":::terminal"),
        "expected :::terminal class for Done galho; got:\n{mermaid}",
    );
}

#[test]
fn dot_color_matches_render_state() {
    let g = DepGraph::new(vec![snap("a", Phase::Done, vec![], vec![])]);
    let dot = format!("{}", g.to_dot());
    // Terminal → grey fill
    assert!(
        dot.contains("fillcolor=\"#dddddd\""),
        "expected terminal grey fill; got:\n{dot}",
    );
}

#[test]
fn json_render_state_field_present_and_matches() {
    let g = DepGraph::new(vec![
        snap("a", Phase::Done, vec![], vec![]),
        snap("b", Phase::Verified, vec!["a"], vec!["a"]),
        snap("c", Phase::Planned, vec!["b"], vec![]),
    ]);
    let value = g.to_json_value();
    let nodes = value["nodes"].as_array().unwrap();
    let by_name: std::collections::HashMap<&str, &serde_json::Value> = nodes
        .iter()
        .map(|n| (n["name"].as_str().unwrap(), n))
        .collect();
    assert_eq!(by_name["a"]["render_state"], serde_json::json!("terminal"));
    assert_eq!(by_name["b"]["render_state"], serde_json::json!("ready"));
    assert_eq!(by_name["c"]["render_state"], serde_json::json!("blocked"));
}

#[test]
fn all_three_visual_surfaces_agree_for_same_snapshot() {
    // Take a galho that should classify as Blocked; verify mermaid class +
    // DOT fillcolor + JSON render_state all agree.
    let s = snap("x", Phase::Planned, vec!["upstream"], vec![]);
    assert_eq!(s.render_state(), RenderState::Blocked);

    let g = DepGraph::new(vec![s.clone()]);
    let mermaid = format!("{}", g.to_mermaid());
    let dot = format!("{}", g.to_dot());
    let json = g.to_json_value();

    assert!(mermaid.contains(":::blocked"));
    assert!(dot.contains(RenderState::Blocked.as_color_hex()));
    assert_eq!(
        json["nodes"][0]["render_state"],
        serde_json::json!("blocked"),
    );
}

// ----- Method-level invariants on RenderState itself -----

#[test]
fn class_names_are_distinct() {
    let names = [
        RenderState::Terminal.as_class_name(),
        RenderState::Ready.as_class_name(),
        RenderState::Blocked.as_class_name(),
    ];
    let unique: std::collections::HashSet<_> = names.iter().collect();
    assert_eq!(unique.len(), 3);
}

#[test]
fn colors_are_distinct() {
    let colors = [
        RenderState::Terminal.as_color_hex(),
        RenderState::Ready.as_color_hex(),
        RenderState::Blocked.as_color_hex(),
    ];
    let unique: std::collections::HashSet<_> = colors.iter().collect();
    assert_eq!(unique.len(), 3);
}

#[test]
fn text_marker_omits_for_no_deps() {
    assert_eq!(RenderState::Ready.as_text_marker(false, ""), "");
    assert_eq!(RenderState::Blocked.as_text_marker(false, ""), "");
    assert_eq!(RenderState::Terminal.as_text_marker(false, ""), "");
}

#[test]
fn text_marker_distinguishes_states_when_deps_present() {
    let ready = RenderState::Ready.as_text_marker(true, "upstream");
    let blocked = RenderState::Blocked.as_text_marker(true, "upstream");
    assert!(ready.contains("✓"));
    assert!(blocked.contains("⏸"));
    assert_ne!(ready, blocked);
}

#[test]
fn serializes_as_kebab_case() {
    let json = serde_json::to_string(&RenderState::Terminal).unwrap();
    assert_eq!(json, "\"terminal\"");
    let json = serde_json::to_string(&RenderState::Ready).unwrap();
    assert_eq!(json, "\"ready\"");
    let json = serde_json::to_string(&RenderState::Blocked).unwrap();
    assert_eq!(json, "\"blocked\"");
}
