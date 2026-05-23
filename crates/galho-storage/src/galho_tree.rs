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

    /// Advance the head of a galho via atomic CAS. `expected = None` means "no head yet."
    pub async fn advance_head(
        &self,
        galho: &str,
        expected: Option<&Blake3Hash>,
        new: &Blake3Hash,
    ) -> Result<(), StoreError> {
        self.store
            .cas_ref(&Self::ref_name(galho), expected, new)
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
