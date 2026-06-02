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
///
/// Deserialization routes through [`TypedStateRepr`] and asserts that the
/// decoded `meta.iac_system` matches `S::id()` — closing the type-confusion
/// hole where bytes written for one IaC system could be decoded into the wrong
/// `TypedState<S>` (the `PhantomData<S>` carries no runtime tag of its own).
/// Serialization is unchanged, so on-disk bytes + content hashes are stable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(bound(deserialize = ""))]
#[serde(try_from = "TypedStateRepr")]
pub struct TypedState<S: IaCSystem> {
    pub graph: ResourceGraph,
    #[serde(default)]
    pub adapter_state: AdapterState,
    pub meta: StateMeta,
    #[serde(skip, default)]
    _marker: PhantomData<S>,
}

/// Untagged wire shape for [`TypedState`] decoding. Carries no `S` marker; the
/// `TryFrom` impl re-attaches the marker after asserting the system matches.
#[derive(Deserialize)]
struct TypedStateRepr {
    graph: ResourceGraph,
    #[serde(default)]
    adapter_state: AdapterState,
    meta: StateMeta,
}

impl<S: IaCSystem> TryFrom<TypedStateRepr> for TypedState<S> {
    type Error = TypedStateDecodeError;

    fn try_from(repr: TypedStateRepr) -> Result<Self, Self::Error> {
        let expected = S::id();
        if repr.meta.iac_system != expected.as_str() {
            return Err(TypedStateDecodeError::SystemMismatch {
                expected: expected.as_str().to_string(),
                found: repr.meta.iac_system,
            });
        }
        Ok(Self {
            graph: repr.graph,
            adapter_state: repr.adapter_state,
            meta: repr.meta,
            _marker: PhantomData,
        })
    }
}

/// Error decoding a [`TypedState`] from bytes written for a different IaC system.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TypedStateDecodeError {
    /// The decoded `meta.iac_system` does not match `S::id()` — wrong-system bytes.
    #[error("TypedState system mismatch: expected '{expected}', found '{found}'")]
    SystemMismatch { expected: String, found: String },
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
