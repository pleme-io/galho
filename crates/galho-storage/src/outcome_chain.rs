//! Hash-linked typed audit chain for `OutcomeEvent` values.
//!
//! Each appended event becomes a [`ChainedOutcomeEntry`] carrying a sequence number,
//! the canonical-bytes BLAKE3 hash of the event, and the prior entry's hash. The chain
//! invariant — `entry.previous_hash == prior.entry_hash` for every adjacent pair — is
//! verifiable in O(n) via [`OutcomeChain::verify_integrity`].
//!
//! The chain is persisted to the surrounding `ObjectStore`:
//!
//! - Each [`ChainedOutcomeEntry`] is content-addressed (stored under its `entry_hash`).
//! - A monotonic `outcome_chain/head` ref points to the current chain head.
//! - A sequence index (`outcome_chain/index/<seq>`) maps `sequence` → `entry_hash` so
//!   callers can walk the chain by sequence number without traversing parent links.
//!
//! Consumes `tameshi::canonicalize::Canonicalizer` for byte-canonicalization
//! (galho-types' Canonicalizer second-consumer status now extends to chained receipts).
//! BLAKE3 hashing via tameshi's `Blake3Hash::digest`.

use std::sync::Arc;

use galho_types::{Blake3Hash, EmitError, OutcomeEmitter, OutcomeEvent};
use serde::{Deserialize, Serialize};
use tameshi::canonicalize::{CanonicalMode, Canonicalizer, RawCanonicalizer};
use tameshi::hash::Blake3Hash as TameshiHash;
use tokio::sync::RwLock;

use crate::object_store::{ObjectStore, StoreError};

/// One entry in the chained outcome log. The full event is embedded so receipts are
/// self-contained — replaying the chain doesn't require external lookup.
///
/// `entry_hash` is **derived**, not stored — it equals `Blake3Hash::digest(canonical_bytes(self))`.
/// This matches git / tameshi MerkleLedger / IPLD content-addressing semantics: the
/// hash IS the address. The `entry_hash()` method recomputes; the surrounding
/// `ObjectStore::put_object` produces the same hash because both pass through the same
/// canonical bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainedOutcomeEntry {
    pub sequence: u64,
    pub event: OutcomeEvent,
    pub previous_hash: Blake3Hash,
}

impl ChainedOutcomeEntry {
    /// Canonical-bytes BLAKE3 of this entry. Matches the `ObjectStore::put_object` hash
    /// produced when this entry is persisted (same bytes, same hash).
    #[must_use]
    pub fn entry_hash(&self) -> Blake3Hash {
        let bytes = serde_json::to_vec(self).expect("ChainedOutcomeEntry serializes");
        let canon = RawCanonicalizer
            .canonicalize(&bytes, CanonicalMode::Strict)
            .into_owned();
        TameshiHash::digest(&canon)
    }

    /// Compute the entry hash from raw on-disk bytes — used during chain restoration
    /// without paying a re-serialize cost.
    #[must_use]
    pub fn hash_from_bytes(bytes: &[u8]) -> Blake3Hash {
        let canon = RawCanonicalizer
            .canonicalize(bytes, CanonicalMode::Strict)
            .into_owned();
        TameshiHash::digest(&canon)
    }
}

/// On-disk meta-kind used for the sequence index.
pub const META_KIND_OUTCOME_INDEX: &str = "outcome_index";
/// The canonical chain-head ref name.
pub const REF_OUTCOME_HEAD: &str = "outcome_chain/head";

/// Compute the chain hash for an event at a given (sequence, previous) position. This
/// is the operation the chain performs on append; exposed for callers that want to
/// pre-compute a hash without taking the chain lock.
#[must_use]
pub fn compute_entry_hash(
    event: &OutcomeEvent,
    previous: &Blake3Hash,
    sequence: u64,
) -> Blake3Hash {
    let entry = ChainedOutcomeEntry {
        sequence,
        event: event.clone(),
        previous_hash: previous.clone(),
    };
    entry.entry_hash()
}

#[derive(Debug)]
struct ChainInner {
    head: Blake3Hash,
    sequence: u64,
}

impl Default for ChainInner {
    fn default() -> Self {
        Self {
            head: TameshiHash::from([0u8; 32]),
            sequence: 0,
        }
    }
}

/// A hash-linked outcome chain backed by an `ObjectStore`.
#[derive(Debug)]
pub struct OutcomeChain<S: ObjectStore> {
    store: Arc<S>,
    inner: RwLock<ChainInner>,
}

impl<S: ObjectStore> OutcomeChain<S> {
    /// Construct a fresh chain backed by `store`. Caller is responsible for ensuring
    /// the store is not already hosting a chain (or call `restore` to pick up an existing one).
    #[must_use]
    pub fn new(store: Arc<S>) -> Self {
        Self {
            store,
            inner: RwLock::new(ChainInner::default()),
        }
    }

    /// Restore chain state from `store`. Reads the head ref + counts forward through
    /// the index to recover sequence number. Idempotent.
    pub async fn restore(store: Arc<S>) -> Result<Self, StoreError> {
        let chain = Self::new(store.clone());
        if let Some(head) = store.read_ref(REF_OUTCOME_HEAD).await? {
            let bytes = store
                .get_object(&head)
                .await?
                .ok_or_else(|| StoreError::Backend("outcome head missing object".into()))?;
            let entry: ChainedOutcomeEntry = serde_json::from_slice(&bytes)
                .map_err(|e| StoreError::Decode(format!("outcome head: {e}")))?;
            let mut inner = chain.inner.write().await;
            inner.head = head;
            inner.sequence = entry.sequence + 1;
        }
        Ok(chain)
    }

    /// Direct store access — for diagnostics + tests.
    #[must_use]
    pub fn store(&self) -> &Arc<S> {
        &self.store
    }

    /// Append a new event to the chain. Returns the resulting [`ChainedOutcomeEntry`].
    /// Side effects: writes the entry as a content-addressed object (its hash IS the
    /// chain entry-hash); updates the head ref via CAS; writes the sequence-index meta.
    pub async fn append(&self, event: OutcomeEvent) -> Result<ChainedOutcomeEntry, StoreError> {
        let (sequence, previous) = {
            let inner = self.inner.read().await;
            (inner.sequence, inner.head.clone())
        };
        let entry = ChainedOutcomeEntry {
            sequence,
            event,
            previous_hash: previous.clone(),
        };
        let bytes = serde_json::to_vec(&entry)
            .map_err(|e| StoreError::Decode(format!("entry encode: {e}")))?;
        // Persist under content hash. The store's hash MUST equal entry.entry_hash()
        // because the store hashes the same bytes we just serialized.
        let stored = self.store.put_object(&bytes).await?;
        debug_assert_eq!(
            stored.hash(),
            &entry.entry_hash(),
            "store content-hash must match chain entry-hash"
        );
        let stored_hash = stored.into_hash();

        // Persist the sequence-index entry so `entry_at(sequence)` walks O(N) once
        // (acceptable for v0.1; a dedicated sequence backend trait lands at M5).
        let seq_key = format!("{sequence:020}");
        self.store
            .put_meta(&stored_hash, META_KIND_OUTCOME_INDEX, seq_key.as_bytes())
            .await?;

        // Advance the head ref via CAS so concurrent appends fail-loud instead of corrupting.
        let prior_head = if sequence == 0 { None } else { Some(&previous) };
        self.store
            .cas_ref(REF_OUTCOME_HEAD, prior_head, &stored_hash)
            .await?;

        // Update in-memory state.
        let mut inner = self.inner.write().await;
        inner.head = stored_hash;
        inner.sequence = sequence + 1;

        Ok(entry)
    }

    /// Current chain head hash. Equals `[0u8; 32]` for an empty chain.
    pub async fn head_hash(&self) -> Blake3Hash {
        self.inner.read().await.head.clone()
    }

    /// Current number of entries.
    pub async fn len(&self) -> u64 {
        self.inner.read().await.sequence
    }

    /// Convenience.
    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }

    /// Verify the chain integrity invariant: every entry's `previous_hash` equals the
    /// prior entry's `entry_hash`. O(n) — walks from sequence 0 to current head.
    pub async fn verify_integrity(&self) -> Result<bool, StoreError> {
        let total = self.len().await;
        let mut expected_previous = TameshiHash::from([0u8; 32]);
        for seq in 0..total {
            let Some(entry) = self.entry_at(seq).await? else {
                return Ok(false);
            };
            if entry.previous_hash != expected_previous {
                return Ok(false);
            }
            if entry.sequence != seq {
                return Ok(false);
            }
            expected_previous = entry.entry_hash();
        }
        Ok(true)
    }

    /// Read the entry at a specific sequence number. Returns `None` if absent.
    pub async fn entry_at(&self, sequence: u64) -> Result<Option<ChainedOutcomeEntry>, StoreError> {
        // Scan the index to find the hash for this sequence. (For v0.1; a dedicated
        // sequence→hash backend lookup is a later optimization.)
        for hash in self.store.list_objects().await? {
            let Some(meta) = self
                .store
                .get_meta(&hash, META_KIND_OUTCOME_INDEX)
                .await?
            else {
                continue;
            };
            let key = String::from_utf8(meta).unwrap_or_default();
            if key == format!("{sequence:020}") {
                let Some(bytes) = self.store.get_object(&hash).await? else {
                    continue;
                };
                let entry: ChainedOutcomeEntry = serde_json::from_slice(&bytes)
                    .map_err(|e| StoreError::Decode(format!("entry decode: {e}")))?;
                return Ok(Some(entry));
            }
        }
        Ok(None)
    }

    /// Collect every entry in sequence order. Useful for replay / display / kensa-style
    /// verification consumers.
    pub async fn entries(&self) -> Result<Vec<ChainedOutcomeEntry>, StoreError> {
        let total = self.len().await;
        let mut out = Vec::with_capacity(total as usize);
        for seq in 0..total {
            if let Some(e) = self.entry_at(seq).await? {
                out.push(e);
            }
        }
        Ok(out)
    }
}

/// `OutcomeEmitter` impl that appends every event to an [`OutcomeChain`].
///
/// Uses an internal tokio runtime handle to bridge the synchronous `OutcomeEmitter::emit`
/// to the async `OutcomeChain::append`. The emit contract (audit failures never block
/// business logic) is preserved — any error during chain append is converted to
/// `EmitError::StoreUnavailable` and the caller logs+counts but doesn't unwind.
pub struct ChainedOutcomeEmitter<S: ObjectStore + 'static> {
    chain: Arc<OutcomeChain<S>>,
    handle: tokio::runtime::Handle,
}

impl<S: ObjectStore + 'static> ChainedOutcomeEmitter<S> {
    /// Construct an emitter bound to `chain`. Captures the current tokio runtime handle
    /// so `emit()` can dispatch async appends.
    ///
    /// # Panics
    ///
    /// Panics if called outside a tokio runtime.
    #[must_use]
    pub fn new(chain: Arc<OutcomeChain<S>>) -> Self {
        Self {
            chain,
            handle: tokio::runtime::Handle::current(),
        }
    }

    /// Direct access to the wrapped chain — primarily for tests + diagnostics.
    #[must_use]
    pub fn chain(&self) -> &Arc<OutcomeChain<S>> {
        &self.chain
    }
}

impl<S: ObjectStore + 'static> std::fmt::Debug for ChainedOutcomeEmitter<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChainedOutcomeEmitter")
            .field("chain", &"OutcomeChain<S>")
            .finish()
    }
}

impl<S: ObjectStore + 'static> OutcomeEmitter for ChainedOutcomeEmitter<S> {
    fn emit(&self, event: &OutcomeEvent) -> Result<(), EmitError> {
        let chain = self.chain.clone();
        let event = event.clone();
        // Use block_on via Handle to bridge sync→async. This is the standard pattern
        // for async-emitter-from-sync-callsite; the alternative (full async OutcomeEmitter
        // trait) would require boxing futures everywhere which we explicitly avoid.
        let result = tokio::task::block_in_place(|| {
            self.handle.block_on(async move { chain.append(event).await })
        });
        result.map(|_| ()).map_err(|e| EmitError::StoreUnavailable {
            reason: e.to_string(),
        })
    }
}
