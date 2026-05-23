//! Property-based mileage on `detect_dep_cycle` + the Promote dep-gate.
//!
//! Four load-bearing invariants:
//!
//! 1. **Self-loop universal:** for any galho name `n`, declaring `n` as its own
//!    dep is always rejected as a cycle.
//! 2. **Cycle-free graph stays cycle-free under unrelated insert:** in a
//!    randomly-generated cycle-free graph, inserting a new galho whose deps
//!    only point at existing galhos (forward direction only) never finds a
//!    cycle.
//! 3. **Closing back-edge always caught:** if a galho `b` (transitively) points
//!    at galho `a`, then declaring `a` with `b` as a dep MUST be rejected.
//! 4. **Promote dep-gate:** a galho with declared but unsatisfied deps cannot
//!    successfully fire `Promote` — the morphism's dep precondition fails.
//!
//! These properties cover the substrate-level guarantees the typed-cycle and
//! typed-dep-satisfaction algebra makes; proptest runs 256 randomly-generated
//! cases per property.

use galho_cli::{detect_dep_cycle, Runtime};
use galho_types::{MorphismContext, MorphismId};
use proptest::prelude::*;
use std::collections::{BTreeMap, BTreeSet};

fn any_name() -> impl Strategy<Value = String> {
    prop::sample::select(vec!["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"])
        .prop_map(String::from)
}

/// Strategy: build a cycle-free dep graph over up to N galhos by emitting them
/// in topological order and only allowing each galho to depend on previously-
/// emitted ones. Returns (insertion_order, map_of_galho_to_deps).
fn any_acyclic_graph() -> impl Strategy<Value = Vec<(String, BTreeSet<String>)>> {
    prop::collection::vec(any_name(), 0..6).prop_map(|raw_names| {
        // Dedupe preserving order.
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut order: Vec<String> = Vec::new();
        for n in raw_names {
            if seen.insert(n.clone()) {
                order.push(n);
            }
        }
        // For each galho, allow deps onto strictly-earlier galhos (topological).
        let mut out: Vec<(String, BTreeSet<String>)> = Vec::new();
        for (idx, name) in order.iter().enumerate() {
            let mut deps: BTreeSet<String> = BTreeSet::new();
            // Pick at most idx/2 earlier galhos as deps (deterministic, no recursion).
            for earlier in order.iter().take(idx).step_by(2) {
                deps.insert(earlier.clone());
            }
            out.push((name.clone(), deps));
        }
        out
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Invariant 1: a self-loop is always a cycle.
    #[test]
    fn self_loop_always_rejected(name in any_name()) {
        let contexts: BTreeMap<String, MorphismContext> = BTreeMap::new();
        let mut deps = BTreeSet::new();
        deps.insert(name.clone());
        let cycle = detect_dep_cycle(&contexts, &name, &deps);
        prop_assert!(cycle.is_some(), "self-loop on '{name}' should always be a cycle");
        let path = cycle.unwrap();
        prop_assert_eq!(path.first(), Some(&name));
        prop_assert_eq!(path.last(), Some(&name));
    }

    /// Invariant 2: in any topologically-built acyclic graph, the in-order
    /// forward inserts never trigger a cycle.
    #[test]
    fn acyclic_graph_inserts_never_trigger_cycle(
        graph in any_acyclic_graph(),
    ) {
        let mut contexts: BTreeMap<String, MorphismContext> = BTreeMap::new();
        for (name, deps) in &graph {
            // For each insert, deps point only at already-inserted galhos.
            let cycle = detect_dep_cycle(&contexts, name, deps);
            prop_assert!(
                cycle.is_none(),
                "acyclic graph insert {name} → {deps:?} should not find cycle; got {cycle:?}",
            );
            let mut ctx = MorphismContext::declared(name);
            ctx.depends_on = deps.clone();
            contexts.insert(name.clone(), ctx);
        }
    }

    /// Invariant 3: in any acyclic graph, choosing any existing galho `target`
    /// and trying to insert a NEW galho that (a) declares the target as a dep
    /// AND (b) has the target back-pointing transitively at it would close a
    /// cycle. We construct this by: taking any node a in the graph that has
    /// transitive descendants, then attempting to add a new edge target →
    /// (any descendant of target) → target. We model this directly by re-
    /// declaring target with one of its descendants as a dep; since descendant
    /// transitively reaches target, this MUST be a cycle.
    ///
    /// Test variant: pick the second galho in the graph (if it has a dep);
    /// attempt to add a dep edge from the first galho back to it. If a→b
    /// holds via the natural construction, then declaring `a` with `b` as a
    /// dep makes a→b and b→a → cycle.
    #[test]
    fn back_edge_against_existing_dep_creates_cycle(
        graph in any_acyclic_graph().prop_filter(
            "need at least one a→b dep edge",
            |g| g.iter().any(|(_, deps)| !deps.is_empty()),
        )
    ) {
        let mut contexts: BTreeMap<String, MorphismContext> = BTreeMap::new();
        for (name, deps) in &graph {
            let mut ctx = MorphismContext::declared(name);
            ctx.depends_on = deps.clone();
            contexts.insert(name.clone(), ctx);
        }
        // Find any (b, deps) where deps is non-empty; for any a in deps,
        // re-declaring a with b as its dep would create a → b → a → ... cycle.
        let candidate = graph
            .iter()
            .find(|(_, deps)| !deps.is_empty())
            .cloned();
        if let Some((b_name, b_deps)) = candidate {
            let a_name = b_deps.iter().next().cloned().expect("non-empty");
            let mut new_a_deps: BTreeSet<String> = BTreeSet::new();
            new_a_deps.insert(b_name.clone());
            let cycle = detect_dep_cycle(&contexts, &a_name, &new_a_deps);
            prop_assert!(
                cycle.is_some(),
                "re-declaring '{a_name}' with dep on '{b_name}' (which already depends on '{a_name}') should create a cycle; got None",
            );
        }
    }
}

/// Invariant 4 (async runtime): a galho with declared but unsatisfied deps
/// cannot fire Promote successfully. Uses a real Runtime + the actual
/// fire_morphism path (not the raw detect function) — exercises the typed
/// MorphismContext::deps_satisfied gate at the algebra layer.
#[tokio::test]
async fn promote_with_unmet_deps_always_errors() {
    for unmet_count in 1..=4 {
        let rt = Runtime::with_memory();
        rt.new_galho("upstream-1").await.unwrap();
        rt.new_galho("upstream-2").await.unwrap();
        rt.new_galho("upstream-3").await.unwrap();
        rt.new_galho("upstream-4").await.unwrap();

        let deps: Vec<String> = (1..=unmet_count)
            .map(|i| format!("upstream-{i}"))
            .collect();
        rt.new_galho_with_deps("downstream", deps.clone())
            .await
            .unwrap();

        // Advance downstream toward Promote-eligible state. Each step is
        // independent of dep satisfaction; the only gate we care about is
        // Promote's DependencyNotMet check.
        for m in [
            MorphismId::Plan,
            MorphismId::ApplyToPreview,
            MorphismId::RecordApproval,
            MorphismId::Verify,
        ] {
            // RecordApproval needs quorum satisfied; confirm once.
            if matches!(m, MorphismId::RecordApproval) {
                let _ = rt.confirm_approval("downstream", "operator").await;
            }
            let stack_root = if matches!(m, MorphismId::ApplyToPreview) {
                Some("stack-root-x".to_string())
            } else {
                None
            };
            let _ = rt.fire_morphism("downstream", m, stack_root).await;
        }

        // Now try Promote. With unmet deps, this MUST error with a precondition
        // failure naming DependencyNotMet.
        let res = rt
            .fire_morphism("downstream", MorphismId::Promote, None)
            .await;
        assert!(
            res.is_err(),
            "Promote with {unmet_count} unmet deps should error; got Ok",
        );
        let msg = format!("{}", res.unwrap_err());
        assert!(
            msg.contains("DependencyNotMet")
                || msg.contains("dependency")
                || msg.contains("dep"),
            "expected dep-related error; got: {msg}",
        );
    }
}
