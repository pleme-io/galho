//! Typed cycle detection in `Runtime::new_galho_with_deps` + the new
//! `list_galhos_with_state` snapshot surface.
//!
//! Load-bearing invariant: a galho declaring a dependency that would form a cycle
//! (direct or transitive) is rejected at insert time with a typed error naming
//! the cycle path. Silent "deps never satisfy" is impossible by construction.

use galho_cli::{detect_dep_cycle, GalhoStateSnapshot, Runtime};
use galho_types::{MorphismContext, Phase};
use std::collections::BTreeMap;

#[tokio::test]
async fn direct_self_loop_rejected() {
    let rt = Runtime::with_memory();
    let err = rt
        .new_galho_with_deps("a", ["a".to_string()])
        .await
        .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("dependency cycle"), "msg: {msg}");
    assert!(msg.contains("a → a"), "msg: {msg}");
}

#[tokio::test]
async fn transitive_cycle_rejected() {
    let rt = Runtime::with_memory();
    // a depends on b
    rt.new_galho_with_deps("a", ["b".to_string()]).await.unwrap();
    // b depends on c
    rt.new_galho_with_deps("b", ["c".to_string()]).await.unwrap();
    // c depends on a → would form a → b → c → a → a cycle
    let err = rt
        .new_galho_with_deps("c", ["a".to_string()])
        .await
        .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("dependency cycle"), "msg: {msg}");
}

#[tokio::test]
async fn linear_chain_accepted() {
    let rt = Runtime::with_memory();
    rt.new_galho("a").await.unwrap();
    rt.new_galho_with_deps("b", ["a".to_string()]).await.unwrap();
    rt.new_galho_with_deps("c", ["b".to_string()]).await.unwrap();
    rt.new_galho_with_deps("d", ["c".to_string()]).await.unwrap();
    // No cycle; all created.
    let names = rt.list_galhos().await;
    assert!(names.contains(&"d".into()));
}

#[tokio::test]
async fn diamond_dependency_accepted() {
    let rt = Runtime::with_memory();
    // a → b → d
    //   ↘ c ↗
    rt.new_galho("d").await.unwrap();
    rt.new_galho_with_deps("b", ["d".to_string()]).await.unwrap();
    rt.new_galho_with_deps("c", ["d".to_string()]).await.unwrap();
    rt.new_galho_with_deps("a", ["b".to_string(), "c".to_string()])
        .await
        .unwrap();
    // Diamonds aren't cycles — accept.
    let names = rt.list_galhos().await;
    assert_eq!(names.len(), 4);
}

#[test]
fn detect_dep_cycle_pure_function_self_loop() {
    let contexts: BTreeMap<String, MorphismContext> = BTreeMap::new();
    let mut deps = std::collections::BTreeSet::new();
    deps.insert("a".into());
    let cycle = detect_dep_cycle(&contexts, "a", &deps);
    assert_eq!(cycle, Some(vec!["a".into(), "a".into()]));
}

#[test]
fn detect_dep_cycle_pure_function_no_cycle() {
    let mut contexts: BTreeMap<String, MorphismContext> = BTreeMap::new();
    let mut a_ctx = MorphismContext::declared("a");
    a_ctx.depends_on.insert("b".into());
    contexts.insert("a".into(), a_ctx);
    let mut deps = std::collections::BTreeSet::new();
    deps.insert("a".into());
    // c depends on a; a depends on b; b is unknown. No cycle.
    assert_eq!(detect_dep_cycle(&contexts, "c", &deps), None);
}

#[tokio::test]
async fn list_galhos_with_state_returns_phase_and_deps() {
    let rt = Runtime::with_memory();
    rt.new_galho("upstream").await.unwrap();
    rt.new_galho_with_deps("downstream", ["upstream".to_string()])
        .await
        .unwrap();
    let mut snaps = rt.list_galhos_with_state().await;
    snaps.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(snaps.len(), 2);
    let down = snaps
        .iter()
        .find(|s| s.name == "downstream")
        .expect("downstream present");
    assert_eq!(down.depends_on, vec!["upstream".to_string()]);
    assert_eq!(down.phase, Phase::Declared);
}

#[test]
fn snapshot_all_deps_satisfied_helpers() {
    let satisfied = GalhoStateSnapshot {
        name: "a".into(),
        phase: Phase::Verified,
        depends_on: vec!["b".into(), "c".into()],
        deps_satisfied: vec!["b".into(), "c".into()],
    };
    assert!(satisfied.all_deps_satisfied());
    assert!(satisfied.unmet_deps().is_empty());

    let blocked = GalhoStateSnapshot {
        name: "a".into(),
        phase: Phase::Declared,
        depends_on: vec!["b".into(), "c".into()],
        deps_satisfied: vec!["b".into()],
    };
    assert!(!blocked.all_deps_satisfied());
    assert_eq!(blocked.unmet_deps(), vec!["c"]);
}
