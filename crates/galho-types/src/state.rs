//! TypedState<S> — the content-addressed state value at each GalhoTree node.
//!
//! See `pleme-io/theory/GALHO.md` §II.2.

use std::marker::PhantomData;

use serde::{Deserialize, Serialize};
use tameshi::hash::Blake3Hash;
use time::OffsetDateTime;

use crate::canonical::{tag, CanonicalBytes, CanonicalSink, content_hash};
use crate::iac_system::IaCSystem;
use crate::ir::ResourceGraph;

/// A typed, content-addressable state instance. Generic over the `IaCSystem` so
/// `TypedState<Terraform>` and `TypedState<Crossplane>` are distinct types at compile time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypedState<S: IaCSystem> {
    pub graph: ResourceGraph,
    #[serde(default)]
    pub adapter_state: AdapterState,
    pub meta: StateMeta,
    #[serde(skip, default)]
    _marker: PhantomData<S>,
}

impl<S: IaCSystem> TypedState<S> {
    #[must_use]
    pub fn new(graph: ResourceGraph, adapter_state: AdapterState, meta: StateMeta) -> Self {
        Self {
            graph,
            adapter_state,
            meta,
            _marker: PhantomData,
        }
    }

    /// Compute the content hash of this state — the BLAKE3 of its canonical bytes.
    #[must_use]
    pub fn hash(&self) -> Blake3Hash {
        content_hash(self)
    }
}

/// Adapter-private bookkeeping that's part of state but not part of the canonical IR.
/// v0.1 represents it as opaque canonical bytes; each adapter knows how to (de)serialize.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AdapterState(pub Vec<u8>);

/// State metadata recorded at the moment a TypedState is materialized.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateMeta {
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    pub created_by: Passaporte,
    pub parent_hashes: Vec<Blake3Hash>,
    pub iac_system: String,
    pub galho_name: String,
    #[serde(default)]
    pub commit_message: Option<String>,
}

/// Saguão identity placeholder until cofre/saguão integration lands (M4+).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct Passaporte(pub String);

impl Passaporte {
    #[must_use]
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// ----- CanonicalBytes impls -----

impl<S: IaCSystem> CanonicalBytes for TypedState<S> {
    fn canonical_bytes(&self, sink: &mut CanonicalSink) {
        sink.write_tag(tag::TYPED_STATE);
        self.graph.canonical_bytes(sink);
        sink.write_tagged(tag::BYTES, &self.adapter_state.0);
        self.meta.canonical_bytes(sink);
    }
}

impl CanonicalBytes for StateMeta {
    fn canonical_bytes(&self, sink: &mut CanonicalSink) {
        sink.write_tag(tag::STATE_META);
        // `created_at` is recording-only metadata; it participates in the hash so two
        // states with identical graph+adapter+actor at different times are different
        // DAG nodes (matches git's commit-timestamp behavior).
        sink.write_tagged_str(tag::STRING, &self.created_at.to_string());
        self.created_by.canonical_bytes(sink);
        sink.write_tag(tag::LIST);
        let len = u32::try_from(self.parent_hashes.len()).expect("parent count fits");
        sink.write_u32_be(len);
        for h in &self.parent_hashes {
            sink.write_len_prefixed(h.0.as_slice());
        }
        sink.write_tagged_str(tag::STRING, &self.iac_system);
        sink.write_tagged_str(tag::STRING, &self.galho_name);
        sink.write_option(&self.commit_message);
    }
}

impl CanonicalBytes for Passaporte {
    fn canonical_bytes(&self, sink: &mut CanonicalSink) {
        sink.write_tagged(tag::PASSAPORTE, self.0.as_bytes());
    }
}
