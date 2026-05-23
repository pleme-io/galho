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

    #[error("decode error: {0}")]
    Decode(String),

    #[error("backend error: {0}")]
    Backend(String),
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
    async fn put_object(&self, bytes: &[u8]) -> Result<Blake3Hash, StoreError>;
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
    store.put_object(&sink.finish()).await
}
