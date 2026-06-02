//! Resource Graph IR — the canonical representation of an IaC system's state.
//!
//! Flat resource graph (§II.1): nodes are resources, edges are typed dependencies.
//! Hierarchy (modules, namespaces) is recoverable from address prefixes but is not a
//! first-class node type. Adapters with richer hierarchical concepts flatten on the
//! way in and restore on the way out.

use std::collections::{BTreeMap, BTreeSet};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::canonical::{tag, CanonicalBytes, CanonicalSink};
use crate::error::GalhoError;
use crate::value::Value;
use tameshi::hash::Blake3Hash;

// ----- IDs and addresses -----

/// Adapter-defined fully-qualified resource address. Examples:
/// - `terraform`: `module.network.aws_vpc.main`
/// - `crossplane`: `kubernetes.crossplane.io/v1.Composition/network-base`
/// - `helm`: `chart.auth-svc/release/prod-auth`
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct ResourceId(pub String);

impl ResourceId {
    #[must_use]
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ResourceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Adapter-defined resource kind. Examples: `aws_db_instance`, `kubernetes_manifest`,
/// `helm.release`, `crossplane.io/v1.RDSInstance`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct ResourceKind(pub String);

impl ResourceKind {
    #[must_use]
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
}

/// Dotted attribute path within a resource (e.g. `tags.0.value`, `network_interface.ip`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct AttrPath(pub Vec<String>);

impl AttrPath {
    #[must_use]
    pub fn new(parts: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self(parts.into_iter().map(Into::into).collect())
    }

    #[must_use]
    pub fn root() -> Self {
        Self(Vec::new())
    }

    #[must_use]
    pub fn rendered(&self) -> String {
        self.0.join(".")
    }
}

// ----- Resources -----

/// Per-resource status reflecting what cloud-API state actually represents. Per
/// `ApplySemantics`, adapters interpret status against their transactionality model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResourceStatus {
    /// A real apply landed. The payload is constructed only via
    /// [`AppliedStatus::new`], which rejects the all-zeros (forged/sentinel)
    /// hash — so a forged `Applied` status is unrepresentable. The internally-
    /// tagged enum + plain-struct payload keeps the on-wire JSON byte-identical
    /// to the previous struct-variant shape
    /// (`{"kind":"applied","generation":…,"hash":…,"applied_at":…}`).
    Applied(AppliedStatus),
    Pending,
    Failed {
        reason: String,
        #[serde(with = "time::serde::rfc3339")]
        last_attempted: OffsetDateTime,
        retry_eligible: bool,
    },
    Drifted {
        #[serde(with = "time::serde::rfc3339")]
        detected_at: OffsetDateTime,
        drift_kind: DriftKind,
    },
    Tombstoned {
        #[serde(with = "time::serde::rfc3339")]
        destroyed_at: OffsetDateTime,
    },
}

/// The payload of [`ResourceStatus::Applied`]. Fields are private; the only
/// constructor is [`AppliedStatus::new`], which rejects an all-zeros BLAKE3
/// hash. A real apply always produces a non-zero content hash, so the
/// forged/sentinel status is unrepresentable by construction — the same
/// discipline as `cofre`'s SecretRef and ishou's `Refined<T,B>`.
///
/// `Deserialize` is derived (operators restore persisted state), but the
/// smart-ctor invariant is the construction-side guard; on-disk bytes that
/// somehow carry a zero hash would round-trip — that case is closed at the
/// translate.rs producer where the value is born.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppliedStatus {
    generation: u64,
    hash: Blake3Hash,
    #[serde(with = "time::serde::rfc3339")]
    applied_at: OffsetDateTime,
}

impl AppliedStatus {
    /// Construct an `Applied` status. Rejects the all-zeros BLAKE3 hash
    /// (the forged/sentinel value) with [`GalhoError::ZeroAppliedHash`].
    pub fn new(
        generation: u64,
        hash: Blake3Hash,
        applied_at: OffsetDateTime,
    ) -> Result<Self, GalhoError> {
        if hash.0 == [0u8; 32] {
            return Err(GalhoError::ZeroAppliedHash);
        }
        Ok(Self {
            generation,
            hash,
            applied_at,
        })
    }

    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    #[must_use]
    pub fn hash(&self) -> &Blake3Hash {
        &self.hash
    }

    #[must_use]
    pub fn applied_at(&self) -> OffsetDateTime {
        self.applied_at
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriftKind {
    AttrValueChanged,
    ResourceVanished,
    UnexpectedResource,
    SchemaChanged,
}

/// Recording-only provenance metadata. Not hashed.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Provenance {
    pub imported: bool,
    pub authored_by: Option<String>,
    pub source_path: Option<String>,
}

/// A single resource node in the graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resource {
    pub id: ResourceId,
    pub kind: ResourceKind,
    pub attrs: BTreeMap<AttrPath, Value>,
    pub deps: BTreeSet<ResourceId>,
    pub status: ResourceStatus,
    #[serde(default)]
    pub provenance: Provenance,
}

// ----- Edges -----

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct DependencyEdge {
    pub from: ResourceId,
    pub to: ResourceId,
    pub kind: DepKind,
    pub attr_path: Option<AttrPath>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DepKind {
    Explicit,
    AttrReference,
    Implicit,
    CrossSystem,
}

// ----- The graph itself -----

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphRoot {
    pub iac_system: String,           // IaCSystemId
    pub schema_version: String,       // Adapter schema version
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourceGraph {
    pub root: GraphRoot,
    pub resources: BTreeMap<ResourceId, Resource>,
    pub edges: BTreeSet<DependencyEdge>,
}

// ----- CanonicalBytes impls -----

impl CanonicalBytes for ResourceId {
    fn canonical_bytes(&self, sink: &mut CanonicalSink) {
        sink.write_tagged_str(tag::STRING, &self.0);
    }
}

impl CanonicalBytes for ResourceKind {
    fn canonical_bytes(&self, sink: &mut CanonicalSink) {
        sink.write_tagged_str(tag::STRING, &self.0);
    }
}

impl CanonicalBytes for AttrPath {
    fn canonical_bytes(&self, sink: &mut CanonicalSink) {
        sink.write_tag(tag::ATTR_PATH);
        sink.write_u32_be(u32::try_from(self.0.len()).expect("path len fits"));
        for seg in &self.0 {
            sink.write_tagged_str(tag::STRING, seg);
        }
    }
}

impl CanonicalBytes for DepKind {
    fn canonical_bytes(&self, sink: &mut CanonicalSink) {
        let v = match self {
            Self::Explicit => 0x01,
            Self::AttrReference => 0x02,
            Self::Implicit => 0x03,
            Self::CrossSystem => 0x04,
        };
        sink.write_u8(v);
    }
}

impl CanonicalBytes for DependencyEdge {
    fn canonical_bytes(&self, sink: &mut CanonicalSink) {
        sink.write_tag(tag::DEPENDENCY);
        self.from.canonical_bytes(sink);
        self.to.canonical_bytes(sink);
        self.kind.canonical_bytes(sink);
        sink.write_option(&self.attr_path);
    }
}

impl CanonicalBytes for Resource {
    fn canonical_bytes(&self, sink: &mut CanonicalSink) {
        sink.write_tag(tag::RESOURCE);
        // Identity fields. Provenance is intentionally NOT hashed (recording-only).
        // Status is hashed because it represents real cloud-side state at this DAG node.
        self.id.canonical_bytes(sink);
        self.kind.canonical_bytes(sink);
        sink.write_sorted_map(
            &self.attrs,
            |s, k| k.canonical_bytes(s),
            |s, v| v.canonical_bytes(s),
        );
        sink.write_tag(tag::LIST);
        sink.write_u32_be(u32::try_from(self.deps.len()).expect("deps len fits"));
        for d in &self.deps {
            d.canonical_bytes(sink);
        }
        // ResourceStatus: we emit a stable discriminant + the operationally-load-bearing
        // fields. Timestamps participate (the moment of apply is part of identity per
        // §II.7). Float status fields would canonicalize if present.
        match &self.status {
            ResourceStatus::Applied(applied) => {
                sink.write_u8(0x01);
                sink.write_raw(&applied.generation().to_be_bytes());
                sink.write_len_prefixed(applied.hash().0.as_slice());
                sink.write_tagged_str(tag::STRING, &applied.applied_at().to_string());
            }
            ResourceStatus::Pending => sink.write_u8(0x02),
            ResourceStatus::Failed {
                reason,
                last_attempted,
                retry_eligible,
            } => {
                sink.write_u8(0x03);
                sink.write_tagged_str(tag::STRING, reason);
                sink.write_tagged_str(tag::STRING, &last_attempted.to_string());
                sink.write_u8(u8::from(*retry_eligible));
            }
            ResourceStatus::Drifted {
                detected_at,
                drift_kind,
            } => {
                sink.write_u8(0x04);
                sink.write_tagged_str(tag::STRING, &detected_at.to_string());
                let dk = match drift_kind {
                    DriftKind::AttrValueChanged => 0x01,
                    DriftKind::ResourceVanished => 0x02,
                    DriftKind::UnexpectedResource => 0x03,
                    DriftKind::SchemaChanged => 0x04,
                };
                sink.write_u8(dk);
            }
            ResourceStatus::Tombstoned { destroyed_at } => {
                sink.write_u8(0x05);
                sink.write_tagged_str(tag::STRING, &destroyed_at.to_string());
            }
        }
    }
}

impl CanonicalBytes for GraphRoot {
    fn canonical_bytes(&self, sink: &mut CanonicalSink) {
        sink.write_tagged_str(tag::STRING, &self.iac_system);
        sink.write_tagged_str(tag::STRING, &self.schema_version);
    }
}

impl CanonicalBytes for ResourceGraph {
    fn canonical_bytes(&self, sink: &mut CanonicalSink) {
        sink.write_tag(tag::RESOURCE_GRAPH);
        self.root.canonical_bytes(sink);
        sink.write_sorted_map(
            &self.resources,
            |s, k| k.canonical_bytes(s),
            |s, v| v.canonical_bytes(s),
        );
        sink.write_tag(tag::LIST);
        sink.write_u32_be(u32::try_from(self.edges.len()).expect("edges len fits"));
        for e in &self.edges {
            e.canonical_bytes(sink);
        }
    }
}

