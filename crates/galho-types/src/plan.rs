//! Plan<S> — typed value representing a proposed mutation to a TypedState.
//!
//! See `pleme-io/theory/GALHO.md` §II.6.

use std::collections::BTreeMap;
use std::marker::PhantomData;

use serde::{Deserialize, Serialize};
use tameshi::hash::Blake3Hash;
use time::OffsetDateTime;

use crate::canonical::{tag, CanonicalBytes, CanonicalSink};
use crate::iac_system::IaCSystem;
use crate::ir::{AttrPath, Resource, ResourceId};
use crate::state::Passaporte;
use crate::value::Value;

/// Typed plan over a given IaCSystem. A plan is a value: content-addressed, sliceable,
/// rebase-able. Generic over `S` so a `Plan<Terraform>` cannot be applied to a
/// `TypedState<Crossplane>` at compile time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan<S: IaCSystem> {
    pub from_state: Blake3Hash,
    pub changes: Vec<TypedChange>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    pub created_by: Passaporte,
    #[serde(skip, default)]
    _marker: PhantomData<S>,
}

impl<S: IaCSystem> Plan<S> {
    #[must_use]
    pub fn new(
        from_state: Blake3Hash,
        changes: Vec<TypedChange>,
        created_by: Passaporte,
    ) -> Self {
        Self {
            from_state,
            changes,
            created_at: OffsetDateTime::now_utc(),
            created_by,
            _marker: PhantomData,
        }
    }

    /// Slice the plan to only changes touching a specific resource.
    #[must_use]
    pub fn slice_resource(&self, id: &ResourceId) -> Vec<TypedChange> {
        self.changes
            .iter()
            .filter(|c| c.resource_id() == id)
            .cloned()
            .collect()
    }

    /// Number of changes in the plan.
    #[must_use]
    pub fn len(&self) -> usize {
        self.changes.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }
}

/// One typed change to a resource. The four kinds map directly to terraform's plan
/// taxonomy and generalize to other IaC systems (crossplane: managed-resource CR mutations;
/// helm: release upgrades; pulumi: stack diff entries).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TypedChange {
    Create {
        resource: Resource,
    },
    Update {
        resource_id: ResourceId,
        before: Resource,
        after: Resource,
        attr_diff: BTreeMap<AttrPath, ValueDiff>,
    },
    Replace {
        resource_id: ResourceId,
        before: Resource,
        after: Resource,
        reason: ReplacementReason,
    },
    Delete {
        resource_id: ResourceId,
        before: Resource,
    },
}

impl TypedChange {
    #[must_use]
    pub fn resource_id(&self) -> &ResourceId {
        match self {
            Self::Create { resource } => &resource.id,
            Self::Update { resource_id, .. }
            | Self::Replace { resource_id, .. }
            | Self::Delete { resource_id, .. } => resource_id,
        }
    }
}

/// Why a Replace was chosen over Update — adapter-specific signal surfaced to the
/// operator and to the audit chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReplacementReason {
    /// One or more attributes triggered replacement (e.g. terraform's `ForceNew`).
    AttrTriggersReplace { attrs: Vec<AttrPath> },
    /// Adapter-determined replacement (e.g. crossplane recreation due to schema mismatch).
    AdapterDetermined { description: String },
}

/// Diff of a single attribute value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ValueDiff {
    Added(Value),
    Removed(Value),
    Changed { before: Value, after: Value },
}

// ----- CanonicalBytes impls -----

impl<S: IaCSystem> CanonicalBytes for Plan<S> {
    fn canonical_bytes(&self, sink: &mut CanonicalSink) {
        sink.write_tag(tag::PLAN);
        sink.write_len_prefixed(self.from_state.0.as_slice());
        sink.write_tag(tag::LIST);
        let len = u32::try_from(self.changes.len()).expect("plan size fits");
        sink.write_u32_be(len);
        for c in &self.changes {
            c.canonical_bytes(sink);
        }
        sink.write_tagged_str(tag::STRING, &self.created_at.to_string());
        self.created_by.canonical_bytes(sink);
    }
}

impl CanonicalBytes for TypedChange {
    fn canonical_bytes(&self, sink: &mut CanonicalSink) {
        sink.write_tag(tag::TYPED_CHANGE);
        match self {
            Self::Create { resource } => {
                sink.write_u8(0x01);
                resource.canonical_bytes(sink);
            }
            Self::Update {
                resource_id,
                before,
                after,
                attr_diff,
            } => {
                sink.write_u8(0x02);
                resource_id.canonical_bytes(sink);
                before.canonical_bytes(sink);
                after.canonical_bytes(sink);
                sink.write_sorted_map(
                    attr_diff,
                    |s, k| k.canonical_bytes(s),
                    |s, v| v.canonical_bytes(s),
                );
            }
            Self::Replace {
                resource_id,
                before,
                after,
                reason,
            } => {
                sink.write_u8(0x03);
                resource_id.canonical_bytes(sink);
                before.canonical_bytes(sink);
                after.canonical_bytes(sink);
                reason.canonical_bytes(sink);
            }
            Self::Delete {
                resource_id,
                before,
            } => {
                sink.write_u8(0x04);
                resource_id.canonical_bytes(sink);
                before.canonical_bytes(sink);
            }
        }
    }
}

impl CanonicalBytes for ReplacementReason {
    fn canonical_bytes(&self, sink: &mut CanonicalSink) {
        match self {
            Self::AttrTriggersReplace { attrs } => {
                sink.write_u8(0x01);
                sink.write_tag(tag::LIST);
                sink.write_u32_be(u32::try_from(attrs.len()).expect("len fits"));
                for a in attrs {
                    a.canonical_bytes(sink);
                }
            }
            Self::AdapterDetermined { description } => {
                sink.write_u8(0x02);
                sink.write_tagged_str(tag::STRING, description);
            }
        }
    }
}

impl CanonicalBytes for ValueDiff {
    fn canonical_bytes(&self, sink: &mut CanonicalSink) {
        sink.write_tag(tag::VALUE_DIFF);
        match self {
            Self::Added(v) => {
                sink.write_u8(0x01);
                v.canonical_bytes(sink);
            }
            Self::Removed(v) => {
                sink.write_u8(0x02);
                v.canonical_bytes(sink);
            }
            Self::Changed { before, after } => {
                sink.write_u8(0x03);
                before.canonical_bytes(sink);
                after.canonical_bytes(sink);
            }
        }
    }
}
