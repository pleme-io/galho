//! The [`ObjectStore`] trait — pluggable backend for the content-addressed Merkle DAG.

use async_trait::async_trait;
use galho_types::{Blake3Hash, CanonicalBytes, CanonicalSink};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("CAS failed for ref '{ref_name}': expected {expected:?}, current {current:?}")]
    CasFailed {
        ref_name: String,
        expected: Option<Blake3Hash>,
        current: Option<Blake3Hash>,
    },

    #[error("delete-CAS failed for ref '{ref_name}': expected {expected:?}, current {current:?}")]
    DeleteCasFailed {
        ref_name: String,
        expected: Option<Blake3Hash>,
        current: Option<Blake3Hash>,
    },

    #[error("decode error: {0}")]
    Decode(String),

    #[error("backend error: {0}")]
    Backend(String),
}

/// A move-only witness that bytes were stored, carrying their content hash. Returned
/// by [`ObjectStore::put_object`] so callers cannot fabricate "this was stored" without
/// having actually stored it. The `hash` field is private; obtain it via [`Addressed::hash`]
/// / [`Addressed::into_hash`] — there is no way to construct an `Addressed` with a hash
/// that doesn't match its bytes outside the store itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Addressed {
    hash: Blake3Hash,
    /// The exact bytes that were stored (for callers that need to re-verify).
    pub bytes: Vec<u8>,
}

impl Addressed {
    /// Construct the witness. `pub(crate)` so only backend impls inside this crate
    /// can mint one — external callers receive `Addressed` from `put_object`, never build it.
    #[must_use]
    pub(crate) fn new(hash: Blake3Hash, bytes: Vec<u8>) -> Self {
        Self { hash, bytes }
    }

    /// The content hash of the stored bytes.
    #[must_use]
    pub fn hash(&self) -> &Blake3Hash {
        &self.hash
    }

    /// Borrow the stored bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Consume the witness into just its content hash.
    #[must_use]
    pub fn into_hash(self) -> Blake3Hash {
        self.hash
    }
}

/// Pluggable content-addressed object store. Async at the trait level so the M3
/// transition to S3 / Postgres backends adds no API churn.
///
/// **Identity contract.** `put_object(bytes)` MUST return `Blake3Hash::digest(bytes)`.
/// Two calls with identical bytes return the same hash and are idempotent at the
/// storage layer (re-storing is a no-op).
///
/// **Meta layer.** A small auxiliary blob keyed by `(hash, kind)`. Used for DAG-traversal
/// indices (`META_KIND_NODE` carries parent-hash + galho-name + timestamp). Meta blobs
/// are NOT part of the object's identity — they're recording metadata accessible without
/// fetching the full object.
///
/// **Ref layer.** Named pointers to object hashes, mutated via atomic CAS. galho heads
/// live under `refs/galhos/<name>`; conflicts under `refs/conflicts/<uuid>`.
#[async_trait]
pub trait ObjectStore: Send + Sync + std::fmt::Debug {
    /// Store bytes; returns an [`Addressed`] witness carrying the content hash.
    async fn put_object(&self, bytes: &[u8]) -> Result<Addressed, StoreError>;
    async fn get_object(&self, hash: &Blake3Hash) -> Result<Option<Vec<u8>>, StoreError>;
    async fn delete_object(&self, hash: &Blake3Hash) -> Result<(), StoreError>;
    async fn list_objects(&self) -> Result<Vec<Blake3Hash>, StoreError>;

    async fn put_meta(
        &self,
        hash: &Blake3Hash,
        kind: &str,
        bytes: &[u8],
    ) -> Result<(), StoreError>;
    async fn get_meta(
        &self,
        hash: &Blake3Hash,
        kind: &str,
    ) -> Result<Option<Vec<u8>>, StoreError>;
    async fn delete_meta(&self, hash: &Blake3Hash, kind: &str) -> Result<(), StoreError>;

    /// Atomic CAS on a named ref. `expected = None` means "must not exist."
    /// On mismatch, returns `Err(StoreError::CasFailed { current })` so the caller can retry
    /// against the observed current value.
    async fn cas_ref(
        &self,
        name: &str,
        expected: Option<&Blake3Hash>,
        new: &Blake3Hash,
    ) -> Result<(), StoreError>;

    /// Atomic compare-and-delete on a named ref. Deletes the ref only if its current
    /// value equals `expected` (`None` = "must not exist", a no-op success). On mismatch,
    /// returns `Err(StoreError::DeleteCasFailed { current })` so the caller doesn't
    /// blindly drop a ref another writer advanced. No default impl — each backend
    /// implements the atomic semantics native to its storage.
    async fn cas_delete_ref(
        &self,
        name: &str,
        expected: Option<&Blake3Hash>,
    ) -> Result<(), StoreError>;

    async fn read_ref(&self, name: &str) -> Result<Option<Blake3Hash>, StoreError>;
    async fn delete_ref(&self, name: &str) -> Result<(), StoreError>;
    async fn list_refs(&self) -> Result<Vec<String>, StoreError>;
}

/// Store a typed value (any `T: CanonicalBytes`) — computes canonical bytes,
/// then `put_object`s them. Returns the BLAKE3 content hash.
pub async fn put_typed<S, T>(store: &S, value: &T) -> Result<Blake3Hash, StoreError>
where
    S: ObjectStore + ?Sized,
    T: CanonicalBytes + ?Sized,
{
    let mut sink = CanonicalSink::new();
    value.canonical_bytes(&mut sink);
    Ok(store.put_object(&sink.finish()).await?.into_hash())
}
