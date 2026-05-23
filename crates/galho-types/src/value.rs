//! IR-level typed values — the leaves of the Resource Graph.
//!
//! Mirrors the `Value` shape in `pleme-io/theory/GALHO.md` §II.1.

use std::collections::BTreeMap;
use serde::{Deserialize, Serialize};

use crate::canonical::{tag, CanonicalBytes, CanonicalSink};

/// IR value. The leaf type of every resource attribute.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Bytes(Vec<u8>),
    List(Vec<Value>),
    Map(BTreeMap<String, Value>),
    SecretRef(SecretRef),
    CrossSystemRef(CrossSystemRef),
}

/// Reference to a cofre-managed secret. Hashed by reference; the resolved value never
/// enters the canonical-bytes stream. In v0.1 we mirror the cofre shape locally; once
/// `cofre-types` is depended-on we'll re-export `cofre_types::SecretRef` and remove
/// this struct.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct SecretRef {
    pub backend: String,
    pub path: String,
    pub version: Option<String>,
}

impl SecretRef {
    #[must_use]
    pub fn new(backend: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            backend: backend.into(),
            path: path.into(),
            version: None,
        }
    }

    #[must_use]
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    /// Bytes that participate in canonical hashing. **Never** includes a resolved value.
    #[must_use]
    pub fn canonical_reference(&self) -> Vec<u8> {
        let mut sink = CanonicalSink::new();
        sink.write_tagged_str(tag::STRING, &self.backend);
        sink.write_tagged_str(tag::STRING, &self.path);
        sink.write_tagged_str(tag::STRING, self.version.as_deref().unwrap_or(""));
        sink.finish()
    }
}

/// Typed cross-system reference: a resource in one IaCSystem references a resource
/// in another. Drives the multi-IaC promotion DAG (§VI).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct CrossSystemRef {
    pub from_system: String,   // IaCSystemId
    pub from_resource: String, // ResourceId
    pub from_attr: String,     // AttrPath rendered as a dotted string
    pub to_system: String,
    pub to_resource: String,
    pub contract: CrossSystemContract,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CrossSystemContract {
    /// `from` reads `to`'s attribute as a value.
    ValueRead { ref_attr: String },
    /// `from` depends on `to` existing; no value flows but order matters.
    ExistenceDependency,
    /// `from` delegates lifecycle to `to` (e.g. galho-helm release lifecycle owned by FluxCD HelmRelease).
    LifecycleDelegation { owner: String },
}

// ----- CanonicalBytes impls -----

impl CanonicalBytes for Value {
    fn canonical_bytes(&self, sink: &mut CanonicalSink) {
        match self {
            Self::Null => sink.write_tag(tag::NULL),
            Self::Bool(b) => b.canonical_bytes(sink),
            Self::Int(i) => i.canonical_bytes(sink),
            Self::Float(f) => {
                sink.write_tag(tag::FLOAT);
                sink.write_f64_be(*f);
            }
            Self::String(s) => s.canonical_bytes(sink),
            Self::Bytes(b) => sink.write_tagged(tag::BYTES, b),
            Self::List(items) => items.canonical_bytes(sink),
            Self::Map(m) => sink.write_sorted_map(
                m,
                |s, k| s.write_tagged_str(tag::STRING, k),
                |s, v| v.canonical_bytes(s),
            ),
            Self::SecretRef(r) => r.canonical_bytes(sink),
            Self::CrossSystemRef(r) => r.canonical_bytes(sink),
        }
    }
}

impl CanonicalBytes for SecretRef {
    fn canonical_bytes(&self, sink: &mut CanonicalSink) {
        sink.write_tagged(tag::SECRET_REF, &self.canonical_reference());
    }
}

impl CanonicalBytes for CrossSystemRef {
    fn canonical_bytes(&self, sink: &mut CanonicalSink) {
        sink.write_tag(tag::CROSS_SYS_REF);
        let mut inner = CanonicalSink::new();
        inner.write_tagged_str(tag::STRING, &self.from_system);
        inner.write_tagged_str(tag::STRING, &self.from_resource);
        inner.write_tagged_str(tag::STRING, &self.from_attr);
        inner.write_tagged_str(tag::STRING, &self.to_system);
        inner.write_tagged_str(tag::STRING, &self.to_resource);
        match &self.contract {
            CrossSystemContract::ValueRead { ref_attr } => {
                inner.write_u8(0x01);
                inner.write_tagged_str(tag::STRING, ref_attr);
            }
            CrossSystemContract::ExistenceDependency => {
                inner.write_u8(0x02);
            }
            CrossSystemContract::LifecycleDelegation { owner } => {
                inner.write_u8(0x03);
                inner.write_tagged_str(tag::STRING, owner);
            }
        }
        sink.write_len_prefixed(inner.as_bytes());
    }
}
