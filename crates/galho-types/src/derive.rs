//! Plan derivation algebra — pure functions over the Resource Graph IR.
//!
//! Two halves:
//!
//! - **Graph-level** ([`diff_graphs`], [`apply_changes`]): pure, deterministic, no I/O.
//!   Proptest-friendly. Composable.
//!
//! - **State-level** ([`diff_states`], [`apply_plan`]): wraps graph-level functions in
//!   `TypedState<S>` with new `StateMeta` (parent_hashes pushed, timestamps refreshed).
//!
//! The round-trip identity `apply_changes(g, diff_graphs(g, g')) == g'` holds for all
//! graph pairs and is verified by proptest. This makes `Plan<S>` a fully algebraic
//! typed value — composable, replayable, rebase-able.

use std::collections::{BTreeMap, BTreeSet};

use time::OffsetDateTime;

use crate::iac_system::IaCSystem;
use crate::ir::{AttrPath, ResourceGraph, ResourceId};
use crate::plan::{Plan, TypedChange, ValueDiff};
use crate::state::{Passaporte, StateMeta, TypedState};
use crate::value::Value;

/// Compute the typed changes that transform `base` into `target`. Pure function.
#[must_use]
pub fn diff_graphs(base: &ResourceGraph, target: &ResourceGraph) -> Vec<TypedChange> {
    let mut changes: Vec<TypedChange> = Vec::new();
    let all_ids: BTreeSet<&ResourceId> = base
        .resources
        .keys()
        .chain(target.resources.keys())
        .collect();

    for id in all_ids {
        let b = base.resources.get(id);
        let t = target.resources.get(id);
        match (b, t) {
            (None, Some(after)) => changes.push(TypedChange::Create {
                resource: after.clone(),
            }),
            (Some(before), None) => changes.push(TypedChange::Delete {
                resource_id: id.clone(),
                before: before.clone(),
            }),
            (Some(before), Some(after)) if before != after => {
                let attr_diff = diff_attrs(&before.attrs, &after.attrs);
                changes.push(TypedChange::Update {
                    resource_id: id.clone(),
                    before: before.clone(),
                    after: after.clone(),
                    attr_diff,
                });
            }
            _ => {} // identical or both absent
        }
    }

    changes
}

/// Per-attribute diff. Used inside `Update` `TypedChange` variants.
#[must_use]
pub fn diff_attrs(
    before: &BTreeMap<AttrPath, Value>,
    after: &BTreeMap<AttrPath, Value>,
) -> BTreeMap<AttrPath, ValueDiff> {
    let mut diff = BTreeMap::new();
    let all_paths: BTreeSet<&AttrPath> = before.keys().chain(after.keys()).collect();
    for p in all_paths {
        match (before.get(p), after.get(p)) {
            (None, Some(v)) => {
                diff.insert(p.clone(), ValueDiff::Added(v.clone()));
            }
            (Some(v), None) => {
                diff.insert(p.clone(), ValueDiff::Removed(v.clone()));
            }
            (Some(b), Some(a)) if b != a => {
                diff.insert(
                    p.clone(),
                    ValueDiff::Changed {
                        before: b.clone(),
                        after: a.clone(),
                    },
                );
            }
            _ => {}
        }
    }
    diff
}

/// Apply a sequence of typed changes to a graph, producing a new graph. Pure function.
///
/// Application semantics:
/// - `Create { resource }` inserts the resource (overwrites if already present — caller's
///   responsibility to ensure that won't happen for well-formed plans).
/// - `Update { resource_id, after, .. }` and `Replace { resource_id, after, .. }`
///   overwrite the existing resource with `after`. Both have identical IR-level effect;
///   the distinction is operationally meaningful (Replace surfaces a recreation in the
///   reconciler) but does not change the resulting graph.
/// - `Delete { resource_id, .. }` removes the resource.
///
/// Edges are NOT recomputed here — the caller is responsible for re-deriving edges
/// from the resulting resource set (or feeding the result back through the adapter's
/// `extract_cross_refs`).
#[must_use]
pub fn apply_changes(base: &ResourceGraph, changes: &[TypedChange]) -> ResourceGraph {
    let mut graph = base.clone();
    for change in changes {
        match change {
            TypedChange::Create { resource } => {
                graph.resources.insert(resource.id.clone(), resource.clone());
            }
            TypedChange::Update {
                resource_id, after, ..
            }
            | TypedChange::Replace {
                resource_id, after, ..
            } => {
                graph.resources.insert(resource_id.clone(), after.clone());
            }
            TypedChange::Delete { resource_id, .. } => {
                graph.resources.remove(resource_id);
            }
        }
    }
    // Edges: filter to those whose endpoints both still exist.
    graph.edges.retain(|e| {
        graph.resources.contains_key(&e.from) && graph.resources.contains_key(&e.to)
    });
    graph
}

/// State-level wrapper: compute a Plan that transforms `base` into `target`.
///
/// The Plan's `from_state` is `base.hash()`. The Plan's `created_by` is the actor.
/// The graph diff is computed via [`diff_graphs`].
#[must_use]
pub fn diff_states<S: IaCSystem>(
    base: &TypedState<S>,
    target: &TypedState<S>,
    actor: Passaporte,
) -> Plan<S> {
    let changes = diff_graphs(&base.graph, &target.graph);
    Plan::new(base.hash(), changes, actor)
}

/// State-level wrapper: apply a Plan to a starting state, producing the resulting state.
///
/// The new state's `StateMeta`:
/// - `parent_hashes = [from.hash()]` (single-parent; merges go through `three_way_merge`).
/// - `created_at = OffsetDateTime::now_utc()`.
/// - `created_by = plan.created_by`.
/// - `iac_system` and `galho_name` inherited from `from.meta`.
/// - `commit_message = None` (callers set explicitly if desired).
///
/// The new state's `adapter_state` is inherited as-is from `from.adapter_state`. Adapters
/// that need to mutate their bookkeeping post-apply (e.g. tfstate lineage/serial bumps)
/// will produce a follow-up state with the bumped `adapter_state` via their own logic.
#[must_use]
pub fn apply_plan<S: IaCSystem>(plan: &Plan<S>, from: &TypedState<S>) -> TypedState<S> {
    let graph = apply_changes(&from.graph, &plan.changes);
    let meta = StateMeta {
        created_at: OffsetDateTime::now_utc(),
        created_by: plan.created_by.clone(),
        parent_hashes: vec![from.hash()],
        iac_system: from.meta.iac_system.clone(),
        galho_name: from.meta.galho_name.clone(),
        commit_message: None,
    };
    TypedState::new(graph, from.adapter_state.clone(), meta)
}
