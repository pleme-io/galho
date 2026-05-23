//! Three-way merge over the canonical Resource Graph IR.
//!
//! Pure function: `(base, ours, theirs) -> MergeResult`. The 13-case matrix from
//! `pleme-io/theory/GALHO.md` §II.4, plus per-attribute-path refinement that converts
//! "both branches modified the resource" into a clean merge when the modified
//! attribute paths are disjoint.
//!
//! Two surfaces:
//!
//! - [`three_way_merge`] — graph-level, pure, system-agnostic.
//! - [`merge_states`] — state-level wrapper that produces a new `TypedState<S>` on
//!   clean merges, with `parent_hashes = [ours.hash, theirs.hash]` and refreshed meta.

use std::collections::{BTreeMap, BTreeSet};

use time::OffsetDateTime;

use crate::conflict::{
    BranchSide, StructuralConflict, StructuralConflictKind, TypedConflict,
};
use crate::iac_system::IaCSystem;
use crate::ir::{AttrPath, DependencyEdge, Resource, ResourceGraph, ResourceId};
use crate::state::{Passaporte, StateMeta, TypedState};
use crate::value::Value;

/// Result of a three-way merge over `ResourceGraph`.
#[derive(Debug, Clone, PartialEq)]
pub enum MergeResult {
    /// A clean merge produced this new graph.
    Clean(ResourceGraph),
    /// Typed conflicts the operator must resolve.
    Conflict(Vec<TypedConflict>),
}

/// Three-way merge over `ResourceGraph`. Pure function; no I/O; no adapter dispatch.
///
/// Precondition: `base.root == ours.root == theirs.root` (the merge is per-IaCSystem;
/// cross-system promotion is a separate concern at §VI). If roots disagree, returns a
/// single CrossSystem-class conflict with `OrderingCycle` as the mismatch kind (the
/// closest existing variant for "these aren't mergeable").
///
/// # Panics
///
/// Never panics. All error paths surface as `MergeResult::Conflict`.
#[must_use]
pub fn three_way_merge(
    base: &ResourceGraph,
    ours: &ResourceGraph,
    theirs: &ResourceGraph,
) -> MergeResult {
    if base.root != ours.root || base.root != theirs.root {
        return MergeResult::Conflict(vec![TypedConflict::CrossSystem(
            crate::conflict::CrossSystemConflict {
                from_system: ours.root.iac_system.clone(),
                from_resource: ResourceId::new(""),
                to_system: theirs.root.iac_system.clone(),
                to_resource: ResourceId::new(""),
                mismatch: crate::conflict::CrossSystemMismatch::OrderingCycle,
            },
        )]);
    }

    let mut ids: BTreeSet<ResourceId> = BTreeSet::new();
    ids.extend(base.resources.keys().cloned());
    ids.extend(ours.resources.keys().cloned());
    ids.extend(theirs.resources.keys().cloned());

    let mut merged_resources: BTreeMap<ResourceId, Resource> = BTreeMap::new();
    let mut conflicts: Vec<TypedConflict> = Vec::new();

    for id in &ids {
        let b = base.resources.get(id);
        let o = ours.resources.get(id);
        let t = theirs.resources.get(id);
        match merge_one_resource(b, o, t) {
            ResourceMergeOutcome::Take(r) => {
                merged_resources.insert(id.clone(), r);
            }
            ResourceMergeOutcome::Delete => {}
            ResourceMergeOutcome::Conflict(c) => conflicts.push(c),
        }
    }

    if !conflicts.is_empty() {
        return MergeResult::Conflict(conflicts);
    }

    // Edges: union of edges in (base ∪ ours ∪ theirs), filtered to those whose endpoints
    // both still exist in the merged graph.
    let mut merged_edges: BTreeSet<DependencyEdge> = BTreeSet::new();
    for src in [&base.edges, &ours.edges, &theirs.edges] {
        for e in src {
            if merged_resources.contains_key(&e.from) && merged_resources.contains_key(&e.to) {
                merged_edges.insert(e.clone());
            }
        }
    }

    MergeResult::Clean(ResourceGraph {
        root: base.root.clone(),
        resources: merged_resources,
        edges: merged_edges,
    })
}

enum ResourceMergeOutcome {
    Take(Resource),
    Delete,
    Conflict(TypedConflict),
}

fn merge_one_resource(
    base: Option<&Resource>,
    ours: Option<&Resource>,
    theirs: Option<&Resource>,
) -> ResourceMergeOutcome {
    use ResourceMergeOutcome::{Conflict, Delete, Take};

    // 13-case matrix from §II.4. Order matters — earlier arms are special-cases of later ones.
    match (base, ours, theirs) {
        (None, None, None) => Delete,

        // Unchanged: base == ours == theirs.
        (Some(b), Some(o), Some(t)) if b == o && b == t => Take(b.clone()),

        // Modified-ours-only.
        (Some(b), Some(o), Some(t)) if b == t && b != o => Take(o.clone()),

        // Modified-theirs-only.
        (Some(b), Some(o), Some(t)) if b == o && b != t => Take(t.clone()),

        // Modified-both-identical.
        (Some(b), Some(o), Some(t)) if b != o && o == t => Take(o.clone()),

        // Modified-both-diverged: per-attribute refinement (clean if disjoint paths).
        (Some(b), Some(o), Some(t)) => merge_diverged_resource(b, o, t),

        // Created-both-identical.
        (None, Some(o), Some(t)) if o == t => Take(o.clone()),

        // Created-both-diverged.
        (None, Some(o), Some(t)) => Conflict(TypedConflict::Structural(StructuralConflict {
            resource_id: o.id.clone(),
            kind: StructuralConflictKind::CreatedBothDiverged,
            base: None,
            ours: Some(o.clone()),
            theirs: Some(t.clone()),
            conflicting_paths: diverged_paths(&o.attrs, &t.attrs),
        })),

        // Created-ours-only / Created-theirs-only.
        (None, Some(o), None) => Take(o.clone()),
        (None, None, Some(t)) => Take(t.clone()),

        // Deleted-both.
        (Some(_), None, None) => Delete,

        // Deleted-ours-only-unchanged-theirs.
        (Some(b), None, Some(t)) if b == t => Delete,

        // Deleted-ours, modified-theirs.
        (Some(b), None, Some(t)) => Conflict(TypedConflict::Structural(StructuralConflict {
            resource_id: b.id.clone(),
            kind: StructuralConflictKind::DeletedVsModified {
                deleted_in: BranchSide::Ours,
                modified_in: BranchSide::Theirs,
            },
            base: Some(b.clone()),
            ours: None,
            theirs: Some(t.clone()),
            conflicting_paths: diverged_paths(&b.attrs, &t.attrs),
        })),

        // Deleted-theirs-only-unchanged-ours.
        (Some(b), Some(o), None) if b == o => Delete,

        // Deleted-theirs, modified-ours.
        (Some(b), Some(o), None) => Conflict(TypedConflict::Structural(StructuralConflict {
            resource_id: b.id.clone(),
            kind: StructuralConflictKind::DeletedVsModified {
                deleted_in: BranchSide::Theirs,
                modified_in: BranchSide::Ours,
            },
            base: Some(b.clone()),
            ours: Some(o.clone()),
            theirs: None,
            conflicting_paths: diverged_paths(&b.attrs, &o.attrs),
        })),
    }
}

fn merge_diverged_resource(
    base: &Resource,
    ours: &Resource,
    theirs: &Resource,
) -> ResourceMergeOutcome {
    use ResourceMergeOutcome::{Conflict, Take};

    // Walk every attribute path that appears in any of the three. Per-path resolution.
    let all_paths: BTreeSet<&AttrPath> = base
        .attrs
        .keys()
        .chain(ours.attrs.keys())
        .chain(theirs.attrs.keys())
        .collect();

    let mut merged_attrs: BTreeMap<AttrPath, Value> = BTreeMap::new();
    let mut conflicting: BTreeSet<AttrPath> = BTreeSet::new();

    for p in all_paths {
        let bv = base.attrs.get(p);
        let ov = ours.attrs.get(p);
        let tv = theirs.attrs.get(p);

        match (bv, ov, tv) {
            (Some(b), Some(o), Some(t)) if b == o && b == t => {
                merged_attrs.insert(p.clone(), b.clone());
            }
            (Some(b), Some(o), Some(t)) if b == t => {
                merged_attrs.insert(p.clone(), o.clone());
            }
            (Some(b), Some(o), Some(t)) if b == o => {
                let _ = b;
                merged_attrs.insert(p.clone(), t.clone());
            }
            (Some(_), Some(o), Some(t)) if o == t => {
                merged_attrs.insert(p.clone(), o.clone());
            }
            (Some(_), Some(_), Some(_)) => {
                conflicting.insert(p.clone());
            }
            (None, Some(o), Some(t)) if o == t => {
                merged_attrs.insert(p.clone(), o.clone());
            }
            (None, Some(_), Some(_)) => {
                conflicting.insert(p.clone());
            }
            (None, Some(o), None) => {
                merged_attrs.insert(p.clone(), o.clone());
            }
            (None, None, Some(t)) => {
                merged_attrs.insert(p.clone(), t.clone());
            }
            (Some(_), None, None) => { /* deleted-both at attr level */ }
            (Some(b), None, Some(t)) if b == t => { /* delete in ours, unchanged theirs */ }
            (Some(_), None, Some(_)) => {
                conflicting.insert(p.clone());
            }
            (Some(b), Some(o), None) if b == o => { /* delete in theirs, unchanged ours */ }
            (Some(_), Some(_), None) => {
                conflicting.insert(p.clone());
            }
            (None, None, None) => unreachable!("attr path appeared in union but all None"),
        }
    }

    if !conflicting.is_empty() {
        return Conflict(TypedConflict::Structural(StructuralConflict {
            resource_id: base.id.clone(),
            kind: StructuralConflictKind::ModifiedBothDiverged,
            base: Some(base.clone()),
            ours: Some(ours.clone()),
            theirs: Some(theirs.clone()),
            conflicting_paths: conflicting,
        }));
    }

    // Reconcile non-attribute fields. v0.1 policy: prefer ours' kind/status/provenance,
    // union deps. Kind divergence isn't surfaced as a conflict here; M3 will refine
    // this once we know how each adapter handles kind changes (terraform: forces
    // replacement; crossplane: usually a structural break).
    Take(Resource {
        id: base.id.clone(),
        kind: ours.kind.clone(),
        attrs: merged_attrs,
        deps: ours.deps.union(&theirs.deps).cloned().collect(),
        status: ours.status.clone(),
        provenance: ours.provenance.clone(),
    })
}

fn diverged_paths(
    a: &BTreeMap<AttrPath, Value>,
    b: &BTreeMap<AttrPath, Value>,
) -> BTreeSet<AttrPath> {
    let mut out = BTreeSet::new();
    let all: BTreeSet<&AttrPath> = a.keys().chain(b.keys()).collect();
    for p in all {
        if a.get(p) != b.get(p) {
            out.insert(p.clone());
        }
    }
    out
}

/// State-level result: clean merge produces a new `TypedState<S>`; conflicts surface
/// the typed conflict list unchanged from graph-level merge.
#[derive(Debug, Clone, PartialEq)]
pub enum StateMergeResult<S: IaCSystem> {
    Clean(TypedState<S>),
    Conflict(Vec<TypedConflict>),
}

/// State-level three-way merge. Wraps `three_way_merge` and produces a fresh
/// `TypedState<S>` on a clean merge, with:
///
/// - `parent_hashes = [ours.hash(), theirs.hash()]` (two parents — this IS the merge commit).
/// - `created_at = OffsetDateTime::now_utc()`.
/// - `created_by = actor` (typically the promoter).
/// - `iac_system` and `galho_name` inherited from `ours.meta`.
/// - `adapter_state` inherited from `ours.adapter_state` (the promoting branch wins on
///   adapter bookkeeping; M3 adapters will refine if their bookkeeping has merge semantics).
#[must_use]
pub fn merge_states<S: IaCSystem>(
    base: &TypedState<S>,
    ours: &TypedState<S>,
    theirs: &TypedState<S>,
    actor: Passaporte,
) -> StateMergeResult<S> {
    match three_way_merge(&base.graph, &ours.graph, &theirs.graph) {
        MergeResult::Conflict(c) => StateMergeResult::Conflict(c),
        MergeResult::Clean(graph) => {
            let meta = StateMeta {
                created_at: OffsetDateTime::now_utc(),
                created_by: actor,
                parent_hashes: vec![ours.hash(), theirs.hash()],
                iac_system: ours.meta.iac_system.clone(),
                galho_name: ours.meta.galho_name.clone(),
                commit_message: None,
            };
            StateMergeResult::Clean(TypedState::new(graph, ours.adapter_state.clone(), meta))
        }
    }
}
