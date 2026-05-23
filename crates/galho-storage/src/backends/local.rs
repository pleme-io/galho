//! Local filesystem `ObjectStore` backend — the canonical dev backend.
//!
//! Storage layout:
//! ```text
//! <root>/
//!   objects/<2-hex-prefix>/<rest>           # raw content-addressed bytes
//!   meta/<kind>/<2-hex-prefix>/<rest>       # auxiliary metadata (e.g. parent-hash index)
//!   refs/<ref-path>                          # newline-terminated hex hash; missing if absent
//! ```
//!
//! Atomic writes via write-tmp + rename. CAS implemented via O_EXCL-style
//! `OpenOptions::create_new(true)` on a temporary lockfile alongside each ref. The
//! lockfile approach is portable and survives concurrent processes; for production
//! S3+DynamoDB-style CAS is a future backend (M3).

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use galho_types::Blake3Hash;
use tokio::fs;
use tokio::io::AsyncWriteExt;

use crate::object_store::{ObjectStore, StoreError};

#[derive(Debug)]
pub struct LocalFsBackend {
    root: PathBuf,
}

impl LocalFsBackend {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn object_path(&self, hash: &Blake3Hash) -> PathBuf {
        let hex = hash.to_hex();
        let (prefix, rest) = hex.split_at(2);
        self.root.join("objects").join(prefix).join(rest)
    }

    fn meta_path(&self, hash: &Blake3Hash, kind: &str) -> PathBuf {
        let hex = hash.to_hex();
        let (prefix, rest) = hex.split_at(2);
        self.root.join("meta").join(kind).join(prefix).join(rest)
    }

    fn ref_path(&self, name: &str) -> PathBuf {
        self.root.join("refs").join(name)
    }

    async fn ensure_parent(path: &Path) -> Result<(), StoreError> {
        if let Some(p) = path.parent() {
            fs::create_dir_all(p).await?;
        }
        Ok(())
    }

    /// Atomic write via temp file + rename. Idempotent: if `path` already exists, skip.
    async fn write_atomic(path: &Path, bytes: &[u8], idempotent: bool) -> Result<(), StoreError> {
        if idempotent && fs::try_exists(path).await.unwrap_or(false) {
            return Ok(());
        }
        Self::ensure_parent(path).await?;
        let tmp = path.with_extension("tmp");
        let mut f = fs::File::create(&tmp).await?;
        f.write_all(bytes).await?;
        f.sync_all().await?;
        drop(f);
        fs::rename(&tmp, path).await?;
        Ok(())
    }
}

#[async_trait]
impl ObjectStore for LocalFsBackend {
    async fn put_object(&self, bytes: &[u8]) -> Result<Blake3Hash, StoreError> {
        let hash = Blake3Hash::digest(bytes);
        let path = self.object_path(&hash);
        Self::write_atomic(&path, bytes, true).await?;
        Ok(hash)
    }

    async fn get_object(&self, hash: &Blake3Hash) -> Result<Option<Vec<u8>>, StoreError> {
        let path = self.object_path(hash);
        match fs::read(&path).await {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(StoreError::Io(e)),
        }
    }

    async fn delete_object(&self, hash: &Blake3Hash) -> Result<(), StoreError> {
        let path = self.object_path(hash);
        match fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(StoreError::Io(e)),
        }
    }

    async fn list_objects(&self) -> Result<Vec<Blake3Hash>, StoreError> {
        let root = self.root.join("objects");
        let mut out = Vec::new();
        if !fs::try_exists(&root).await.unwrap_or(false) {
            return Ok(out);
        }
        let mut entries = fs::read_dir(&root).await?;
        while let Some(prefix_entry) = entries.next_entry().await? {
            if !prefix_entry.file_type().await?.is_dir() {
                continue;
            }
            let prefix = prefix_entry.file_name();
            let prefix = prefix.to_string_lossy().to_string();
            let mut subentries = fs::read_dir(prefix_entry.path()).await?;
            while let Some(file) = subentries.next_entry().await? {
                let rest = file.file_name();
                let rest = rest.to_string_lossy().to_string();
                let hex = format!("{prefix}{rest}");
                if let Ok(h) = Blake3Hash::from_hex(&hex) {
                    out.push(h);
                }
            }
        }
        Ok(out)
    }

    async fn put_meta(
        &self,
        hash: &Blake3Hash,
        kind: &str,
        bytes: &[u8],
    ) -> Result<(), StoreError> {
        let path = self.meta_path(hash, kind);
        Self::write_atomic(&path, bytes, false).await
    }

    async fn get_meta(
        &self,
        hash: &Blake3Hash,
        kind: &str,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        let path = self.meta_path(hash, kind);
        match fs::read(&path).await {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(StoreError::Io(e)),
        }
    }

    async fn delete_meta(&self, hash: &Blake3Hash, kind: &str) -> Result<(), StoreError> {
        let path = self.meta_path(hash, kind);
        match fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(StoreError::Io(e)),
        }
    }

    /// CAS via a lockfile sibling to the ref. Acquires by `create_new(true)`
    /// (POSIX `O_EXCL`); releases by `remove_file` after the rename. Spinwait-free
    /// because async readers don't block writers — they just see stale data until rename.
    async fn cas_ref(
        &self,
        name: &str,
        expected: Option<&Blake3Hash>,
        new: &Blake3Hash,
    ) -> Result<(), StoreError> {
        let path = self.ref_path(name);
        Self::ensure_parent(&path).await?;
        let lock = path.with_extension("lock");

        // Acquire lock via O_EXCL create. If lock exists, treat as contention.
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock)
            .await
        {
            Ok(f) => drop(f),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(StoreError::Backend(format!(
                    "ref '{name}' is currently locked"
                )));
            }
            Err(e) => return Err(StoreError::Io(e)),
        }

        // Read current value while holding the lock.
        let current = self.read_ref(name).await?;
        if current.as_ref() != expected {
            let _ = fs::remove_file(&lock).await;
            return Err(StoreError::CasFailed {
                ref_name: name.into(),
                expected: expected.cloned(),
                current,
            });
        }

        // Write new value via atomic rename.
        let tmp = path.with_extension("tmp");
        let hex = format!("{}\n", new.to_hex());
        fs::write(&tmp, hex.as_bytes()).await?;
        fs::rename(&tmp, &path).await?;
        let _ = fs::remove_file(&lock).await;
        Ok(())
    }

    async fn read_ref(&self, name: &str) -> Result<Option<Blake3Hash>, StoreError> {
        let path = self.ref_path(name);
        match fs::read_to_string(&path).await {
            Ok(s) => {
                let trimmed = s.trim();
                Blake3Hash::from_hex(trimmed)
                    .map(Some)
                    .map_err(|e| StoreError::Decode(e.to_string()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(StoreError::Io(e)),
        }
    }

    async fn delete_ref(&self, name: &str) -> Result<(), StoreError> {
        let path = self.ref_path(name);
        match fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(StoreError::Io(e)),
        }
    }

    async fn list_refs(&self) -> Result<Vec<String>, StoreError> {
        let root = self.root.join("refs");
        let mut out = Vec::new();
        if !fs::try_exists(&root).await.unwrap_or(false) {
            return Ok(out);
        }
        list_refs_recursive(&root, &root, &mut out).await?;
        Ok(out)
    }
}

fn list_refs_recursive<'a>(
    base: &'a Path,
    dir: &'a Path,
    out: &'a mut Vec<String>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), StoreError>> + Send + 'a>> {
    Box::pin(async move {
        let mut entries = fs::read_dir(dir).await?;
        while let Some(e) = entries.next_entry().await? {
            let ft = e.file_type().await?;
            let path = e.path();
            if ft.is_dir() {
                list_refs_recursive(base, &path, out).await?;
            } else if ft.is_file() {
                if let Some(ext) = path.extension() {
                    if ext == "tmp" || ext == "lock" {
                        continue;
                    }
                }
                if let Ok(rel) = path.strip_prefix(base) {
                    out.push(rel.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/"));
                }
            }
        }
        Ok(())
    })
}
