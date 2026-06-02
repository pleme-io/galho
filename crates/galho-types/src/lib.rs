//! galho-types — canonical Resource Graph IR + content-addressed state + three-way merge algebra.
//!
//! See `pleme-io/theory/GALHO.md` for the full destination spec. This crate hosts:
//!
//! - The canonical Resource Graph IR (§II.1): `ResourceGraph`, `Resource`, `Value`, `DependencyEdge`.
//! - The [`CanonicalBytes`] trait (§II.7): IR-side emission of canonical bytes, consumed by
//!   `tameshi::canonicalize::Canonicalizer` for BLAKE3 hashing. galho is the canonical
//!   second consumer of tameshi's `Canonicalizer` trait.
//! - The [`IaCSystem`] trait (§III): typed adapter contract for plugging IaC backends
//!   (terraform / crossplane / helm / pulumi / ansible / nix) into the algebra.
//! - `TypedState<S>` (§II.2), `Plan<S>` (§II.6), `TypedConflict` (§II.5).
//!
//! The crate is `no_std`-curious but currently `std`-only because tameshi pulls `std`.

#![forbid(unsafe_code)]
#![deny(missing_debug_implementations)]
#![warn(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions, clippy::missing_errors_doc)]

pub mod canonical;
pub mod compliance;
pub mod conflict;
pub mod config;
pub mod derive;
pub mod error;
pub mod iac_system;
pub mod ir;
pub mod knowledge;
pub mod merge;
pub mod morphism;
pub mod outcome;
pub mod phase;
pub mod plan;
pub mod state;
pub mod stack_lock;
pub mod sync;
pub mod value;

#[cfg(feature = "testing")]
pub mod testing;

pub use canonical::{CanonicalBytes, CanonicalSink, content_hash};
pub use derive::{apply_changes, apply_plan, diff_attrs, diff_graphs, diff_states};
pub use conflict::{
    BranchSide, CrossSystemConflict, CrossSystemMismatch, SemanticConflict,
    SemanticConflictKind, StructuralConflict, StructuralConflictKind, TypedConflict,
};
pub use error::GalhoError;
pub use iac_system::{ApplySemantics, IaCSystem, IaCSystemId, StateLocation};
pub use ir::{
    AppliedStatus, AttrPath, DependencyEdge, DepKind, DriftKind, GraphRoot, Provenance, Resource,
    ResourceGraph, ResourceId, ResourceKind, ResourceStatus,
};
pub use merge::{MergeResult, StateMergeResult, merge_states, three_way_merge};
pub use plan::{Plan, ReplacementReason, TypedChange, ValueDiff};
pub use state::{AdapterState, Passaporte, StateMeta, TypedState, TypedStateDecodeError};
pub use value::{CrossSystemContract, CrossSystemRef, SecretRef, Value};

// ===== §IV-bis: Phase / Morphism / Sync / StackLock / KnowledgeBase =====
pub use config::{DriftDetection, GalhoAcl, GalhoConfig, RebasePolicy, WritePolicy};
pub use knowledge::{check_query_consistency, KnowledgeBase, KnowledgeBaseError};
pub use morphism::{
    morphism_for, Abandon, ApplyToPreview, DriftReconcile, Escalate, MorphismContext,
    MorphismRequirement, PhaseMorphism, Plan as PlanMorphism, Promote, Recover, RecordApproval,
    Resume, RevertApply, SealDone, Verify,
};
pub use phase::{
    is_known_transition, transition_table, transitions_from, transitions_to, MorphismId, Phase,
    PhaseClass, PhaseTransition,
};
pub use compliance::{
    ComplianceBaseline, ComplianceReport, ComplianceRule, ComplianceStatus, ComplianceViolation,
};
pub use outcome::{
    EmitError, LogOutcomeEmitter, MemoryOutcomeEmitter, NoopOutcomeEmitter, OutcomeEmitter,
    OutcomeEvent, OutcomeEventType,
};
pub use stack_lock::{
    ApplyTtlBounds, BoundedApplyTtl, Bounds, LockError, LockOutcome, Refined, RefinedError,
    StackLock, StackLockError, StackRoot,
};
pub use sync::{
    AttestationProof, ComplianceRegime, SignalSource, Sync, SyncConfig, SyncKind, SyncReceipt,
    TimeoutAction,
};

// Re-export tameshi's `Blake3Hash` so consumers depend on one canonical hash type.
pub use tameshi::hash::Blake3Hash;
