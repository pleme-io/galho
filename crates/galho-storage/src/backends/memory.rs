//! In-memory `ObjectStore` backend — the canonical test backend and offline-dev fallback.
//!
//! All operations are O(1) hash-map lookups guarded by a single `tokio::sync::RwLock`.

use std::collections::HashMap;

use async_trait::async_trait;
use galho_types::Blake3Hash;
use tokio::sync::RwLock;

use crate::object_store::{Addressed, ObjectStore, StoreError};

#[derive(Debug, Default)]
struct Inner {
    objects: HashMap<Blake3Hash, Vec<u8>>,
    meta: HashMap<(Blake3Hash, String), Vec<u8>>,
    refs: HashMap<String, Blake3Hash>,
}

#[derive(Debug, Default)]
pub struct MemoryBackend {
    inner: RwLock<Inner>,
}

impl MemoryBackend {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl ObjectStore for MemoryBackend {
    async fn put_object(&self, bytes: &[u8]) -> Result<Addressed, StoreError> {
        let hash = Blake3Hash::digest(bytes);
        let mut g = self.inner.write().await;
        g.objects
            .entry(hash.clone())
            .or_insert_with(|| bytes.to_vec());
        Ok(Addressed::new(hash, bytes.to_vec()))
    }

    async fn get_object(&self, hash: &Blake3Hash) -> Result<Option<Vec<u8>>, StoreError> {
        let g = self.inner.read().await;
        Ok(g.objects.get(hash).cloned())
    }

    async fn delete_object(&self, hash: &Blake3Hash) -> Result<(), StoreError> {
        let mut g = self.inner.write().await;
        g.objects.remove(hash);
        Ok(())
    }

    async fn list_objects(&self) -> Result<Vec<Blake3Hash>, StoreError> {
        let g = self.inner.read().await;
        Ok(g.objects.keys().cloned().collect())
    }

    async fn put_meta(
        &self,
        hash: &Blake3Hash,
        kind: &str,
        bytes: &[u8],
    ) -> Result<(), StoreError> {
        let mut g = self.inner.write().await;
        g.meta
            .insert((hash.clone(), kind.into()), bytes.to_vec());
        Ok(())
    }

    async fn get_meta(
        &self,
        hash: &Blake3Hash,
        kind: &str,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        let g = self.inner.read().await;
        Ok(g.meta.get(&(hash.clone(), kind.into())).cloned())
    }

    async fn delete_meta(&self, hash: &Blake3Hash, kind: &str) -> Result<(), StoreError> {
        let mut g = self.inner.write().await;
        g.meta.remove(&(hash.clone(), kind.into()));
        Ok(())
    }

    async fn cas_ref(
        &self,
        name: &str,
        expected: Option<&Blake3Hash>,
        new: &Blake3Hash,
    ) -> Result<(), StoreError> {
        let mut g = self.inner.write().await;
        let current = g.refs.get(name).cloned();
        if current.as_ref() != expected {
            return Err(StoreError::CasFailed {
                ref_name: name.into(),
                expected: expected.cloned(),
                current,
            });
        }
        g.refs.insert(name.into(), new.clone());
        Ok(())
    }

    async fn cas_delete_ref(
        &self,
        name: &str,
        expected: Option<&Blake3Hash>,
    ) -> Result<(), StoreError> {
        let mut g = self.inner.write().await;
        let current = g.refs.get(name).cloned();
        if current.as_ref() != expected {
            return Err(StoreError::DeleteCasFailed {
                ref_name: name.into(),
                expected: expected.cloned(),
                current,
            });
        }
        g.refs.remove(name);
        Ok(())
    }

    async fn read_ref(&self, name: &str) -> Result<Option<Blake3Hash>, StoreError> {
        let g = self.inner.read().await;
        Ok(g.refs.get(name).cloned())
    }

    async fn delete_ref(&self, name: &str) -> Result<(), StoreError> {
        let mut g = self.inner.write().await;
        g.refs.remove(name);
        Ok(())
    }

    async fn list_refs(&self) -> Result<Vec<String>, StoreError> {
        let g = self.inner.read().await;
        Ok(g.refs.keys().cloned().collect())
    }
}
