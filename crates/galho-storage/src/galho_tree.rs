//! High-level DAG operations layered over [`ObjectStore`].
//!
//! - Insert typed states with parent-hash bookkeeping.
//! - Atomic galho-head advancement via CAS.
//! - Parent-traversal reachability.
//! - Reference-counted GC sweep.

use std::collections::BTreeSet;
use std::sync::Arc;

use galho_types::{Blake3Hash, CanonicalBytes};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::object_store::{put_typed, ObjectStore, StoreError};

/// Auxiliary node metadata kept alongside each stored state object. Stored under
/// `META_KIND_NODE`. Owns the parent edges so GC traversal need not deserialize
/// the full TypedState (which would require knowing its `IaCSystem` type parameter).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeMeta {
    pub parent_hashes: Vec<Blake3Hash>,
    pub galho_name: String,
    pub iac_system: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// Meta kind tag for the parent-hash index. Stable; do not rename.
pub const META_KIND_NODE: &str = "node";

/// Outcome of a GC sweep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GcReport {
    pub deleted: usize,
    pub retained: usize,
}

/// A move-only witness that an `advance_ref` was refused because the new node's
/// lineage does not descend from the current head. Carries the conflicting
/// hashes so a caller can discharge the divergence with a merge node. `#[must_use]`
/// so a diverged advance can't be silently dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use = "a Divergence must be discharged via discharge_divergence or explicitly inspected"]
pub struct Divergence {
    /// The galho whose head diverged.
    galho: String,
    /// The head the caller expected the new node to descend from.
    expected_tip: Blake3Hash,
    /// The new node's hash, which did NOT list `expected_tip` among its parents.
    new_node: Blake3Hash,
}

impl Divergence {
    /// The galho name.
    #[must_use]
    pub fn galho(&self) -> &str {
        &self.galho
    }

    /// The head the new node failed to descend from.
    #[must_use]
    pub fn expected_tip(&self) -> &Blake3Hash {
        &self.expected_tip
    }

    /// The diverged new node.
    #[must_use]
    pub fn new_node(&self) -> &Blake3Hash {
        &self.new_node
    }
}

impl std::fmt::Display for Divergence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "galho '{}' diverged: new node {} does not descend from head {}",
            self.galho,
            self.new_node.to_hex(),
            self.expected_tip.to_hex(),
        )
    }
}

/// Result of [`GalhoTree::advance_ref`]. Either the head advanced, or the new node's
/// lineage did not descend from the expected tip (a [`Divergence`] the caller resolves).
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub enum AdvanceOutcome {
    /// The head advanced to the new node.
    Advanced,
    /// The new node's `parent_hashes` did not include `expected_tip` — the head is
    /// unchanged; resolve via [`GalhoTree::discharge_divergence`].
    Diverged(Divergence),
}

/// DAG operations layered over an `ObjectStore`. Generic in the store type so callers
/// pick the backend (memory / local-fs / S3 / Postgres) at composition time.
#[derive(Debug)]
pub struct GalhoTree<S: ObjectStore> {
    store: Arc<S>,
}

impl<S: ObjectStore> GalhoTree<S> {
    #[must_use]
    pub fn new(store: Arc<S>) -> Self {
        Self { store }
    }

    #[must_use]
    pub fn store(&self) -> &Arc<S> {
        &self.store
    }

    fn ref_name(galho: &str) -> String {
        format!("galhos/{galho}")
    }

    /// Insert a typed value into the DAG and record its `NodeMeta` for traversal.
    pub async fn insert_node<T>(
        &self,
        value: &T,
        meta: &NodeMeta,
    ) -> Result<Blake3Hash, StoreError>
    where
        T: CanonicalBytes + ?Sized,
    {
        let hash = put_typed(&*self.store, value).await?;
        let bytes = serde_json::to_vec(meta)
            .map_err(|e| StoreError::Decode(e.to_string()))?;
        self.store.put_meta(&hash, META_KIND_NODE, &bytes).await?;
        Ok(hash)
    }

    /// Fetch the `NodeMeta` for a hash, returning `None` if either the meta or the object is absent.
    pub async fn get_node_meta(
        &self,
        hash: &Blake3Hash,
    ) -> Result<Option<NodeMeta>, StoreError> {
        let bytes = self.store.get_meta(hash, META_KIND_NODE).await?;
        match bytes {
            None => Ok(None),
            Some(b) => serde_json::from_slice(&b)
                .map(Some)
                .map_err(|e| StoreError::Decode(e.to_string())),
        }
    }

    /// Current head hash for a galho name. Returns `None` if no head is registered yet.
    pub async fn head(&self, galho: &str) -> Result<Option<Blake3Hash>, StoreError> {
        self.store.read_ref(&Self::ref_name(galho)).await
    }

    /// Advance the head of a galho with a **lineage check**: the `new_node` may only
    /// become the head if it descends from `expected_tip` (i.e. `new_meta.parent_hashes`
    /// contains `expected_tip`). When `expected_tip = None` the galho has no head yet and
    /// any node may become the initial head. On a lineage miss the head is left unchanged
    /// and an [`AdvanceOutcome::Diverged`] is returned carrying the conflict.
    ///
    /// This replaces the old raw-CAS `advance_head`: a stale `expected_tip` is no longer
    /// just a CAS race — it surfaces as a typed `Diverged`, the precise signal a branch
    /// rebase / three-way merge consumes.
    pub async fn advance_ref(
        &self,
        galho: &str,
        expected_tip: Option<&Blake3Hash>,
        new_node: &Blake3Hash,
        new_meta: &NodeMeta,
    ) -> Result<AdvanceOutcome, StoreError> {
        if let Some(tip) = expected_tip {
            if !new_meta.parent_hashes.iter().any(|p| p == tip) {
                return Ok(AdvanceOutcome::Diverged(Divergence {
                    galho: galho.to_string(),
                    expected_tip: tip.clone(),
                    new_node: new_node.clone(),
                }));
            }
        }
        self.store
            .cas_ref(&Self::ref_name(galho), expected_tip, new_node)
            .await?;
        Ok(AdvanceOutcome::Advanced)
    }

    /// Discharge a [`Divergence`] by advancing the head to a merge node that descends
    /// from BOTH the diverged tips. The `merge_meta` MUST list both the divergence's
    /// `expected_tip` and `new_node` among its `parent_hashes`; otherwise the merge is
    /// rejected (the divergence is not actually resolved). On success the head moves to
    /// `merge_node`.
    pub async fn discharge_divergence(
        &self,
        divergence: Divergence,
        merge_node: &Blake3Hash,
        merge_meta: &NodeMeta,
    ) -> Result<(), StoreError> {
        let descends_from_tip = merge_meta
            .parent_hashes
            .iter()
            .any(|p| p == divergence.expected_tip());
        let descends_from_new = merge_meta
            .parent_hashes
            .iter()
            .any(|p| p == divergence.new_node());
        if !descends_from_tip || !descends_from_new {
            // Typed error WITHOUT format!() (typed-emission ratchet).
            let mut msg = String::from("merge node for galho '");
            msg.push_str(divergence.galho());
            msg.push_str("' does not descend from both diverged tips");
            return Err(StoreError::Backend(msg));
        }
        // Advance from the diverged head (expected_tip) to the merge node via CAS.
        self.store
            .cas_ref(
                &Self::ref_name(divergence.galho()),
                Some(divergence.expected_tip()),
                merge_node,
            )
            .await
    }

    /// List all galho names with registered heads.
    pub async fn list_galhos(&self) -> Result<Vec<String>, StoreError> {
        let refs = self.store.list_refs().await?;
        Ok(refs
            .into_iter()
            .filter_map(|r| r.strip_prefix("galhos/").map(str::to_string))
            .collect())
    }

    /// Set of state hashes reachable from `head` via `NodeMeta::parent_hashes`.
    /// A node without a `NodeMeta` is treated as a root (no parents); its hash is still
    /// included in the reachable set.
    pub async fn reachable_from(
        &self,
        head: &Blake3Hash,
    ) -> Result<BTreeSet<Blake3Hash>, StoreError> {
        let mut visited: BTreeSet<Blake3Hash> = BTreeSet::new();
        let mut stack: Vec<Blake3Hash> = vec![head.clone()];
        while let Some(h) = stack.pop() {
            if visited.contains(&h) {
                continue;
            }
            let meta = self.get_node_meta(&h).await?;
            visited.insert(h);
            if let Some(meta) = meta {
                for p in meta.parent_hashes {
                    if !visited.contains(&p) {
                        stack.push(p);
                    }
                }
            }
        }
        Ok(visited)
    }

    /// The live set: every state hash reachable from any galho head.
    pub async fn live_set(&self) -> Result<BTreeSet<Blake3Hash>, StoreError> {
        let names = self.list_galhos().await?;
        let mut live = BTreeSet::new();
        for n in names {
            if let Some(h) = self.head(&n).await? {
                live.extend(self.reachable_from(&h).await?);
            }
        }
        Ok(live)
    }

    /// Sweep: delete any object + its meta blob if it is not in the live set. Returns
    /// the (deleted, retained) counts.
    pub async fn gc(&self) -> Result<GcReport, StoreError> {
        let live = self.live_set().await?;
        let all = self.store.list_objects().await?;
        let mut deleted = 0;
        for h in &all {
            if !live.contains(h) {
                self.store.delete_object(h).await?;
                let _ = self.store.delete_meta(h, META_KIND_NODE).await;
                deleted += 1;
            }
        }
        Ok(GcReport {
            deleted,
            retained: live.len(),
        })
    }
}
