//! Storage-layer invariants — proptest + targeted unit tests.
//!
//! Covers `MemoryBackend`, `LocalFsBackend`, and the higher-level `GalhoTree` ops.
//! These properties are load-bearing: a failure here means the audit chain's
//! content-addressing story is broken.

use std::sync::Arc;

use galho_storage::{
    backends::{LocalFsBackend, MemoryBackend},
    GalhoTree, NodeMeta, ObjectStore, StoreError,
};
use galho_types::Blake3Hash;
use proptest::prelude::*;
use time::OffsetDateTime;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn empty_meta(galho: &str) -> NodeMeta {
    NodeMeta {
        parent_hashes: Vec::new(),
        galho_name: galho.into(),
        iac_system: "test".into(),
        created_at: OffsetDateTime::now_utc(),
    }
}

fn child_meta(galho: &str, parents: Vec<Blake3Hash>) -> NodeMeta {
    NodeMeta {
        parent_hashes: parents,
        galho_name: galho.into(),
        iac_system: "test".into(),
        created_at: OffsetDateTime::now_utc(),
    }
}

// =============================================================================
// ObjectStore properties (MemoryBackend — fast, deterministic)
// =============================================================================

proptest! {
    /// S.1 — put-get round-trip on the memory backend.
    #[test]
    fn memory_put_get_roundtrip(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
        let rt = rt();
        rt.block_on(async {
            let store = MemoryBackend::new();
            let hash = store.put_object(&bytes).await.unwrap();
            prop_assert_eq!(hash.clone(), Blake3Hash::digest(&bytes));
            let got = store.get_object(&hash).await.unwrap();
            prop_assert_eq!(got, Some(bytes));
            Ok(())
        })?;
    }

    /// S.2 — put_object is idempotent: storing the same bytes twice returns the same hash
    /// and doesn't double-store.
    #[test]
    fn memory_put_object_idempotent(bytes in prop::collection::vec(any::<u8>(), 0..256)) {
        let rt = rt();
        rt.block_on(async {
            let store = MemoryBackend::new();
            let h1 = store.put_object(&bytes).await.unwrap();
            let h2 = store.put_object(&bytes).await.unwrap();
            prop_assert_eq!(h1.clone(), h2);
            let list = store.list_objects().await.unwrap();
            prop_assert_eq!(list.len(), 1);
            let _ = h1;
            Ok(())
        })?;
    }

    /// S.3 — CAS atomicity: cas_ref(None) succeeds on missing ref; second call fails.
    #[test]
    fn memory_cas_initial_then_conflict(bytes in prop::collection::vec(any::<u8>(), 1..64)) {
        let rt = rt();
        rt.block_on(async {
            let store = MemoryBackend::new();
            let h = store.put_object(&bytes).await.unwrap();
            // First cas: must succeed.
            store.cas_ref("galhos/test", None, &h).await.unwrap();
            // Second cas with wrong expected: must fail.
            let other_bytes = vec![0xff];
            let h_other = store.put_object(&other_bytes).await.unwrap();
            let err = store.cas_ref("galhos/test", None, &h_other).await;
            let is_cas_failed = matches!(err, Err(StoreError::CasFailed { .. }));
            prop_assert!(is_cas_failed);
            // CAS with matching expected: succeeds.
            store.cas_ref("galhos/test", Some(&h), &h_other).await.unwrap();
            let head = store.read_ref("galhos/test").await.unwrap();
            prop_assert_eq!(head, Some(h_other.clone()));
            let _ = h_other;
            Ok(())
        })?;
    }
}

// =============================================================================
// LocalFsBackend — same invariants on the on-disk backend
// =============================================================================

#[test]
fn local_fs_put_get_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let store = LocalFsBackend::new(dir.path());
    let rt = rt();
    rt.block_on(async {
        let bytes = b"hello galho".to_vec();
        let hash = store.put_object(&bytes).await.unwrap();
        assert_eq!(hash, Blake3Hash::digest(&bytes));
        let got = store.get_object(&hash).await.unwrap();
        assert_eq!(got, Some(bytes));
    });
}

#[test]
fn local_fs_idempotent_put() {
    let dir = tempfile::tempdir().unwrap();
    let store = LocalFsBackend::new(dir.path());
    let rt = rt();
    rt.block_on(async {
        let bytes = b"galho stable".to_vec();
        let h1 = store.put_object(&bytes).await.unwrap();
        let h2 = store.put_object(&bytes).await.unwrap();
        assert_eq!(h1, h2);
        let list = store.list_objects().await.unwrap();
        assert_eq!(list.len(), 1);
    });
}

#[test]
fn local_fs_cas_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let store = LocalFsBackend::new(dir.path());
    let rt = rt();
    rt.block_on(async {
        let h1 = store.put_object(b"v1").await.unwrap();
        let h2 = store.put_object(b"v2").await.unwrap();
        // Initial cas with None: succeeds.
        store
            .cas_ref("galhos/main", None, &h1)
            .await
            .unwrap();
        // Cas with wrong expected fails.
        assert!(matches!(
            store.cas_ref("galhos/main", None, &h2).await,
            Err(StoreError::CasFailed { .. })
        ));
        // Cas with correct expected succeeds.
        store
            .cas_ref("galhos/main", Some(&h1), &h2)
            .await
            .unwrap();
        let head = store.read_ref("galhos/main").await.unwrap();
        assert_eq!(head, Some(h2));
    });
}

#[test]
fn local_fs_list_refs_recursive() {
    let dir = tempfile::tempdir().unwrap();
    let store = LocalFsBackend::new(dir.path());
    let rt = rt();
    rt.block_on(async {
        let h = store.put_object(b"x").await.unwrap();
        store.cas_ref("galhos/main", None, &h).await.unwrap();
        store
            .cas_ref("galhos/feat/foo", None, &h)
            .await
            .unwrap();
        store.cas_ref("conflicts/c1", None, &h).await.unwrap();
        let mut refs = store.list_refs().await.unwrap();
        refs.sort();
        assert_eq!(
            refs,
            vec![
                "conflicts/c1".to_string(),
                "galhos/feat/foo".to_string(),
                "galhos/main".to_string()
            ]
        );
    });
}

// =============================================================================
// GalhoTree DAG operations
// =============================================================================

#[test]
fn galho_tree_insert_and_reachable() {
    let rt = rt();
    rt.block_on(async {
        let store = Arc::new(MemoryBackend::new());
        let tree = GalhoTree::new(store.clone());

        // Insert root: parent_hashes = [].
        let root_hash = tree
            .insert_node(b"root-state".as_slice(), &empty_meta("main"))
            .await
            .unwrap();

        // Insert child with root as parent.
        let child_hash = tree
            .insert_node(
                b"child-state".as_slice(),
                &child_meta("main", vec![root_hash.clone()]),
            )
            .await
            .unwrap();

        // Insert grandchild.
        let grandchild_hash = tree
            .insert_node(
                b"grandchild-state".as_slice(),
                &child_meta("main", vec![child_hash.clone()]),
            )
            .await
            .unwrap();

        // Reachable from grandchild should include all three.
        let reachable = tree.reachable_from(&grandchild_hash).await.unwrap();
        assert!(reachable.contains(&root_hash));
        assert!(reachable.contains(&child_hash));
        assert!(reachable.contains(&grandchild_hash));
        assert_eq!(reachable.len(), 3);
    });
}

#[test]
fn galho_tree_gc_preserves_reachable_deletes_orphans() {
    let rt = rt();
    rt.block_on(async {
        let store = Arc::new(MemoryBackend::new());
        let tree = GalhoTree::new(store.clone());

        // Build: root → child (reachable from "main"); orphan unrelated.
        let root_hash = tree
            .insert_node(b"root".as_slice(), &empty_meta("main"))
            .await
            .unwrap();
        let child_hash = tree
            .insert_node(
                b"child".as_slice(),
                &child_meta("main", vec![root_hash.clone()]),
            )
            .await
            .unwrap();
        let orphan_hash = tree
            .insert_node(b"orphan".as_slice(), &empty_meta("never-pointed-to"))
            .await
            .unwrap();

        // Advance "main" head to child. No head for "never-pointed-to" galho.
        tree.advance_head("main", None, &child_hash).await.unwrap();

        // GC: orphan should disappear; root + child retained.
        let report = tree.gc().await.unwrap();
        assert_eq!(report.deleted, 1);
        assert_eq!(report.retained, 2);

        assert!(store.get_object(&root_hash).await.unwrap().is_some());
        assert!(store.get_object(&child_hash).await.unwrap().is_some());
        assert!(store.get_object(&orphan_hash).await.unwrap().is_none());
    });
}

#[test]
fn galho_tree_advance_head_cas_semantics() {
    let rt = rt();
    rt.block_on(async {
        let store = Arc::new(MemoryBackend::new());
        let tree = GalhoTree::new(store.clone());

        let h1 = tree
            .insert_node(b"s1".as_slice(), &empty_meta("main"))
            .await
            .unwrap();
        let h2 = tree
            .insert_node(b"s2".as_slice(), &child_meta("main", vec![h1.clone()]))
            .await
            .unwrap();

        // First advance: expected=None.
        tree.advance_head("main", None, &h1).await.unwrap();
        // Stale expected → CAS fails.
        assert!(matches!(
            tree.advance_head("main", None, &h2).await,
            Err(StoreError::CasFailed { .. })
        ));
        // Fresh expected → CAS succeeds.
        tree.advance_head("main", Some(&h1), &h2).await.unwrap();
        let head = tree.head("main").await.unwrap();
        assert_eq!(head, Some(h2));
    });
}

#[test]
fn galho_tree_list_galhos() {
    let rt = rt();
    rt.block_on(async {
        let store = Arc::new(MemoryBackend::new());
        let tree = GalhoTree::new(store.clone());

        let h = tree
            .insert_node(b"x".as_slice(), &empty_meta("main"))
            .await
            .unwrap();
        tree.advance_head("main", None, &h).await.unwrap();
        tree.advance_head("feature/auth", None, &h).await.unwrap();

        let mut names = tree.list_galhos().await.unwrap();
        names.sort();
        assert_eq!(names, vec!["feature/auth".to_string(), "main".to_string()]);
    });
}

#[test]
fn galho_tree_gc_cycle_safe() {
    // The DAG is acyclic by construction (every parent is a prior commit) but the
    // traversal still needs to be cycle-safe in case of a malformed meta blob. This
    // verifies that the BFS terminates even when a node's parent_hashes (incorrectly)
    // points back to itself.
    let rt = rt();
    rt.block_on(async {
        let store = Arc::new(MemoryBackend::new());
        let tree = GalhoTree::new(store.clone());

        // Insert a node, then overwrite its meta to claim self-parent.
        let h = tree
            .insert_node(b"loopy".as_slice(), &empty_meta("main"))
            .await
            .unwrap();
        tree.advance_head("main", None, &h).await.unwrap();

        let bad_meta = child_meta("main", vec![h.clone()]);
        let bad_meta_bytes = serde_json::to_vec(&bad_meta).unwrap();
        store
            .put_meta(&h, galho_storage::META_KIND_NODE, &bad_meta_bytes)
            .await
            .unwrap();

        // Traversal must terminate.
        let reach = tree.reachable_from(&h).await.unwrap();
        assert_eq!(reach.len(), 1);
    });
}
