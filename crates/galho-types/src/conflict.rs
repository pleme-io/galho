//! Typed conflict surfacing: Structural / Semantic / CrossSystem.
//!
//! See `pleme-io/theory/GALHO.md` §II.5.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::canonical::{tag, CanonicalBytes, CanonicalSink};
use crate::ir::{AttrPath, Resource, ResourceId};

/// Top-level typed conflict. Each variant maps to one of the three conflict classes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "class", rename_all = "snake_case")]
pub enum TypedConflict {
    Structural(StructuralConflict),
    Semantic(SemanticConflict),
    CrossSystem(CrossSystemConflict),
}

// ----- Structural -----

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructuralConflict {
    pub resource_id: ResourceId,
    pub kind: StructuralConflictKind,
    pub base: Option<Resource>,
    pub ours: Option<Resource>,
    pub theirs: Option<Resource>,
    pub conflicting_paths: BTreeSet<AttrPath>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StructuralConflictKind {
    ModifiedBothDiverged,
    CreatedBothDiverged,
    DeletedVsModified {
        deleted_in: BranchSide,
        modified_in: BranchSide,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BranchSide {
    Ours,
    Theirs,
}

// ----- Semantic -----

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticConflict {
    pub kind: SemanticConflictKind,
    pub broken_resource: ResourceId,
    pub broken_attr_path: AttrPath,
    pub references: ResourceId,
    pub explanation: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticConflictKind {
    /// Resource references a now-deleted target.
    DanglingReference,
    /// Referenced resource changed kind/shape.
    TypeMismatch,
    /// Merge would introduce a dep cycle.
    CycleIntroduction,
    /// Adapter-reported quota violation (e.g. provider check).
    QuotaViolation,
}

// ----- CrossSystem -----

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrossSystemConflict {
    pub from_system: String,
    pub from_resource: ResourceId,
    pub to_system: String,
    pub to_resource: ResourceId,
    pub mismatch: CrossSystemMismatch,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrossSystemMismatch {
    /// `from_resource` references an out-of-date value.
    ReferenceStaleAtTarget,
    /// Declared cross-system type contract violated by actual emitted type.
    TypeContractViolation,
    /// Promotion DAG cycle.
    OrderingCycle,
}

// ----- CanonicalBytes impls -----

impl CanonicalBytes for TypedConflict {
    fn canonical_bytes(&self, sink: &mut CanonicalSink) {
        sink.write_tag(tag::TYPED_CONFLICT);
        match self {
            Self::Structural(c) => {
                sink.write_u8(0x01);
                c.canonical_bytes(sink);
            }
            Self::Semantic(c) => {
                sink.write_u8(0x02);
                c.canonical_bytes(sink);
            }
            Self::CrossSystem(c) => {
                sink.write_u8(0x03);
                c.canonical_bytes(sink);
            }
        }
    }
}

impl CanonicalBytes for StructuralConflict {
    fn canonical_bytes(&self, sink: &mut CanonicalSink) {
        self.resource_id.canonical_bytes(sink);
        match &self.kind {
            StructuralConflictKind::ModifiedBothDiverged => sink.write_u8(0x01),
            StructuralConflictKind::CreatedBothDiverged => sink.write_u8(0x02),
            StructuralConflictKind::DeletedVsModified {
                deleted_in,
                modified_in,
            } => {
                sink.write_u8(0x03);
                sink.write_u8(branch_side_byte(*deleted_in));
                sink.write_u8(branch_side_byte(*modified_in));
            }
        }
        sink.write_option(&self.base);
        sink.write_option(&self.ours);
        sink.write_option(&self.theirs);
        sink.write_tag(tag::LIST);
        sink.write_u32_be(u32::try_from(self.conflicting_paths.len()).expect("len fits"));
        for p in &self.conflicting_paths {
            p.canonical_bytes(sink);
        }
    }
}

impl CanonicalBytes for SemanticConflict {
    fn canonical_bytes(&self, sink: &mut CanonicalSink) {
        let kind_byte = match self.kind {
            SemanticConflictKind::DanglingReference => 0x01,
            SemanticConflictKind::TypeMismatch => 0x02,
            SemanticConflictKind::CycleIntroduction => 0x03,
            SemanticConflictKind::QuotaViolation => 0x04,
        };
        sink.write_u8(kind_byte);
        self.broken_resource.canonical_bytes(sink);
        self.broken_attr_path.canonical_bytes(sink);
        self.references.canonical_bytes(sink);
        sink.write_tagged_str(tag::STRING, &self.explanation);
    }
}

impl CanonicalBytes for CrossSystemConflict {
    fn canonical_bytes(&self, sink: &mut CanonicalSink) {
        sink.write_tagged_str(tag::STRING, &self.from_system);
        self.from_resource.canonical_bytes(sink);
        sink.write_tagged_str(tag::STRING, &self.to_system);
        self.to_resource.canonical_bytes(sink);
        let mismatch_byte = match self.mismatch {
            CrossSystemMismatch::ReferenceStaleAtTarget => 0x01,
            CrossSystemMismatch::TypeContractViolation => 0x02,
            CrossSystemMismatch::OrderingCycle => 0x03,
        };
        sink.write_u8(mismatch_byte);
    }
}

fn branch_side_byte(s: BranchSide) -> u8 {
    match s {
        BranchSide::Ours => 0x01,
        BranchSide::Theirs => 0x02,
    }
}
