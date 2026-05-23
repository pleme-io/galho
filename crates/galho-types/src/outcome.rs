//! Typed audit events emitted on every Runtime state transition.
//!
//! See `pleme-io/theory/GALHO.md` §X. Every morphism fire, sync confirmation, stack-lock
//! acquire/release, and conflict materialization produces an `OutcomeEvent` that flows
//! through a pluggable `OutcomeEmitter`. The default `LogOutcomeEmitter` writes via
//! `tracing::info`; the test-only `MemoryOutcomeEmitter` collects events for assertions;
//! the upcoming `TameshiOutcomeEmitter` (M5) chains into tameshi's BLAKE3+Ed25519 receipt
//! graph for full audit-chain attestation.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::phase::{MorphismId, Phase};
use crate::stack_lock::StackRoot;
use crate::state::Passaporte;
use crate::sync::{AttestationProof, SyncKind};

/// One typed audit event. The complete superset of "things worth recording" across
/// galho's lifecycle. Every variant carries enough context to be replayable + auditable
/// without consulting the live galho state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutcomeEvent {
    pub event_type: OutcomeEventType,
    pub galho_name: String,
    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
    pub actor: Passaporte,

    // Optional context — populated when applicable to the event type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_phase: Option<Phase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_phase: Option<Phase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub morphism: Option<MorphismId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_kind: Option<SyncKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack_lock_root: Option<StackRoot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attestation: Option<AttestationProof>,
}

/// Discriminant over what kind of audit event this is. The full superset; emitters can
/// filter / route per kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeEventType {
    GalhoCreated,
    MorphismFired,
    MorphismBlocked,
    SyncConfirmed,
    StackLockAcquired,
    StackLockJoined,
    StackLockReleased,
    ConflictMaterialized,
    ConflictResolved,
    DriftDetected,
    GalhoDestroyed,
}

impl OutcomeEventType {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::GalhoCreated => "galho_created",
            Self::MorphismFired => "morphism_fired",
            Self::MorphismBlocked => "morphism_blocked",
            Self::SyncConfirmed => "sync_confirmed",
            Self::StackLockAcquired => "stack_lock_acquired",
            Self::StackLockJoined => "stack_lock_joined",
            Self::StackLockReleased => "stack_lock_released",
            Self::ConflictMaterialized => "conflict_materialized",
            Self::ConflictResolved => "conflict_resolved",
            Self::DriftDetected => "drift_detected",
            Self::GalhoDestroyed => "galho_destroyed",
        }
    }
}

impl std::fmt::Display for OutcomeEventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Builder pattern for `OutcomeEvent` so call sites stay tight.
impl OutcomeEvent {
    #[must_use]
    pub fn new(event_type: OutcomeEventType, galho_name: impl Into<String>) -> Self {
        Self {
            event_type,
            galho_name: galho_name.into(),
            timestamp: OffsetDateTime::now_utc(),
            actor: Passaporte::new("system"),
            from_phase: None,
            to_phase: None,
            morphism: None,
            sync_kind: None,
            stack_lock_root: None,
            note: None,
            attestation: None,
        }
    }

    #[must_use]
    pub fn with_actor(mut self, actor: Passaporte) -> Self {
        self.actor = actor;
        self
    }

    #[must_use]
    pub fn with_phase_transition(mut self, from: Phase, to: Phase) -> Self {
        self.from_phase = Some(from);
        self.to_phase = Some(to);
        self
    }

    #[must_use]
    pub fn with_morphism(mut self, m: MorphismId) -> Self {
        self.morphism = Some(m);
        self
    }

    #[must_use]
    pub fn with_sync(mut self, k: SyncKind) -> Self {
        self.sync_kind = Some(k);
        self
    }

    #[must_use]
    pub fn with_stack_root(mut self, root: StackRoot) -> Self {
        self.stack_lock_root = Some(root);
        self
    }

    #[must_use]
    pub fn with_note(mut self, n: impl Into<String>) -> Self {
        self.note = Some(n.into());
        self
    }

    #[must_use]
    pub fn with_attestation(mut self, a: AttestationProof) -> Self {
        self.attestation = Some(a);
        self
    }
}

/// Errors an emitter can return. Emitter failures NEVER propagate to the morphism
/// pipeline — they're logged + counted but don't unwind the operator's action. This
/// matches tameshi's existing emitter contract (audit failures shouldn't block business
/// logic).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EmitError {
    StoreUnavailable { reason: String },
    SignatureFailed { reason: String },
    Internal { reason: String },
}

impl std::fmt::Display for EmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StoreUnavailable { reason } => write!(f, "outcome store unavailable: {reason}"),
            Self::SignatureFailed { reason } => write!(f, "outcome signature failed: {reason}"),
            Self::Internal { reason } => write!(f, "internal: {reason}"),
        }
    }
}

impl std::error::Error for EmitError {}

/// Pluggable emitter. Implementors:
///
/// - `LogOutcomeEmitter` — tracing::info per event. Fleet default.
/// - `NoopOutcomeEmitter` — drops every event. For tests + pure-library consumers.
/// - `MemoryOutcomeEmitter` — collects events in-memory. For test assertions.
/// - `TameshiOutcomeEmitter` (M5) — chains into tameshi's BLAKE3+Ed25519 receipt graph.
pub trait OutcomeEmitter: Send + Sync {
    fn emit(&self, event: &OutcomeEvent) -> Result<(), EmitError>;
}

/// Tracing-based emitter. Each event → `tracing::info!` at the `galho.outcome` target.
/// Production default; works with any tracing subscriber the consumer has installed.
#[derive(Debug, Default, Clone, Copy)]
pub struct LogOutcomeEmitter;

impl OutcomeEmitter for LogOutcomeEmitter {
    fn emit(&self, event: &OutcomeEvent) -> Result<(), EmitError> {
        tracing::info!(
            target: "galho.outcome",
            event_type = %event.event_type,
            galho = %event.galho_name,
            from = ?event.from_phase,
            to = ?event.to_phase,
            morphism = ?event.morphism,
            stack_root = ?event.stack_lock_root,
            note = ?event.note,
            "galho outcome"
        );
        Ok(())
    }
}

/// Drop-everything emitter. For tests + offline consumers that don't want noise.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopOutcomeEmitter;

impl OutcomeEmitter for NoopOutcomeEmitter {
    fn emit(&self, _event: &OutcomeEvent) -> Result<(), EmitError> {
        Ok(())
    }
}

/// In-memory event collector. Hands back the full event list on demand. For test
/// assertions about which events fired in what order.
#[derive(Debug, Default)]
pub struct MemoryOutcomeEmitter {
    events: Mutex<Vec<OutcomeEvent>>,
}

impl MemoryOutcomeEmitter {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn snapshot(&self) -> Vec<OutcomeEvent> {
        self.events.lock().expect("mutex poisoned").clone()
    }

    pub fn clear(&self) {
        self.events.lock().expect("mutex poisoned").clear();
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.events.lock().expect("mutex poisoned").len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Count events of a specific type. Convenience for test assertions.
    #[must_use]
    pub fn count_of(&self, kind: OutcomeEventType) -> usize {
        self.events
            .lock()
            .expect("mutex poisoned")
            .iter()
            .filter(|e| e.event_type == kind)
            .count()
    }
}

impl OutcomeEmitter for MemoryOutcomeEmitter {
    fn emit(&self, event: &OutcomeEvent) -> Result<(), EmitError> {
        self.events
            .lock()
            .map_err(|e| EmitError::Internal {
                reason: format!("mutex: {e}"),
            })?
            .push(event.clone());
        Ok(())
    }
}
