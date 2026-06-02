//! §III.6 / §XIX.5 — the credential-safety layer for golden-path attraction.
//!
//! Golden-path attraction has two halves: *favor golden paths* (the
//! [`crate::attractor`] policy) and *cross a cloud ceiling only by explicit
//! typed intent* (this module). A move that mutates cloud must carry a
//! write-scoped credential; a plan/observe move needs only a read scope. The
//! split is a type, so "a write credential used on a golden plan-only move" or
//! "a read credential used to apply" is caught, not asserted.
//!
//! * [`CredScope`] — the §III.6 credential split (read-only vs time-boxed write).
//! * [`required_scope`] — the per-EDGE policy: a `CeilingCrossing(NonTransactionalIo)`
//!   edge requires [`RequiredScope::Write`]; everything else (golden edges and
//!   the read-only `ExternalObservation` crossings) requires only
//!   [`RequiredScope::ReadOnly`]. Keyed off [`crate::flow_tier::edge_tier`], so
//!   it can never disagree with the golden/ceiling classification.
//! * [`CeilingCrossing`] — the §XIX.5 move-only witness: the sole way to obtain
//!   one is [`CeilingCrossing::authorize`], which refuses golden edges (no
//!   witness needed), non-edges, and under-scoped credentials. A
//!   `CeilingCrossing`-tier morphism takes this witness by value at its
//!   destination, so an accidental cloud-touch cannot type-check.
//!
//! Refines §XIX.5's blanket "`CeilingCrossing` ⇒ `WriteApply`": that
//! over-privileges the two read-only `ExternalObservation` crossings (drift
//! detect / escalate). Least-privilege is keyed per-edge here.

use crate::flow_tier::{Ceiling, EdgeTier, edge_tier};
use crate::phase::{MorphismId, Phase};
use crate::stack_lock::BoundedApplyTtl;
use crate::value::SecretRef;

/// The minimum credential scope an edge requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RequiredScope {
    /// A live-cloud read (plan, drift observe) or no cloud at all (golden).
    ReadOnly,
    /// A live-cloud mutation (apply, destroy, reverse cutover).
    Write,
}

impl RequiredScope {
    /// Is `provided` strong enough to satisfy `self`? `Write` provides both;
    /// `ReadOnly` provides only reads.
    #[must_use]
    pub const fn satisfied_by(self, provided: RequiredScope) -> bool {
        matches!((self, provided), (Self::ReadOnly, _) | (Self::Write, RequiredScope::Write))
    }
}

/// §III.6 — the credential split. A morphism is *given* one of these; the edge it
/// drives *requires* a [`RequiredScope`] ([`required_scope`]). A `WriteApply`
/// credential is structurally time-boxed (the TTL is a [`BoundedApplyTtl`],
/// proven within `ApplyTtlBounds`), so an unbounded apply credential is
/// unrepresentable.
#[derive(Debug, Clone)]
pub enum CredScope {
    /// Read-only plan / observe. Cannot mutate cloud.
    ReadOnlyPlan { cofre_ref: SecretRef },
    /// Time-boxed write into a named environment.
    WriteApply { cofre_ref: SecretRef, env: String, ttl: BoundedApplyTtl },
}

impl CredScope {
    /// The scope this credential *provides*.
    #[must_use]
    pub const fn provides(&self) -> RequiredScope {
        match self {
            Self::ReadOnlyPlan { .. } => RequiredScope::ReadOnly,
            Self::WriteApply { .. } => RequiredScope::Write,
        }
    }

    /// The cofre reference resolved at materialization time (never the value).
    #[must_use]
    pub const fn cofre_ref(&self) -> &SecretRef {
        match self {
            Self::ReadOnlyPlan { cofre_ref } | Self::WriteApply { cofre_ref, .. } => cofre_ref,
        }
    }

    /// Does this credential satisfy what `edge` requires?
    #[must_use]
    pub fn satisfies_edge(&self, from: Phase, morphism: MorphismId) -> bool {
        required_scope(from, morphism).is_some_and(|req| req.satisfied_by(self.provides()))
    }
}

/// The per-edge required scope, keyed off [`edge_tier`]. `None` for non-edges.
///
/// `CeilingCrossing(NonTransactionalIo)` ⇒ `Write` (a real apply/destroy);
/// `CeilingCrossing(ExternalObservation)` ⇒ `ReadOnly` (a live read);
/// `GoldenPreserving` ⇒ `ReadOnly` (no cloud touch — the read floor).
#[must_use]
pub fn required_scope(from: Phase, morphism: MorphismId) -> Option<RequiredScope> {
    Some(match edge_tier(from, morphism)? {
        EdgeTier::CeilingCrossing(Ceiling::NonTransactionalIo) => RequiredScope::Write,
        EdgeTier::CeilingCrossing(Ceiling::ExternalObservation) | EdgeTier::GoldenPreserving => {
            RequiredScope::ReadOnly
        }
    })
}

/// Why a [`CeilingCrossing::authorize`] was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrossingRefused {
    /// `(from, morphism)` is not a shipped edge.
    NoSuchEdge(Phase, MorphismId),
    /// The edge is `GoldenPreserving` — it needs no witness (caller error).
    EdgeIsGolden(Phase, MorphismId),
    /// The credential is under-scoped for this edge (e.g. a read cred for an apply).
    InsufficientCred { required: RequiredScope, provided: RequiredScope },
}

/// §XIX.5 — a move-only witness of explicit, scoped intent to cross a cloud
/// ceiling. The sole constructor is [`CeilingCrossing::authorize`]; the private
/// `_seal` blocks downstream struct literals; it is **not** `Clone`, `Copy`,
/// `Default`, `Serialize`, or `Deserialize`. A `CeilingCrossing`-tier morphism
/// takes one by value at its destination, so an accidental cloud-touch (no
/// witness) cannot compile, and a witness cannot be re-used or forged.
#[derive(Debug)]
pub struct CeilingCrossing {
    ceiling: Ceiling,
    edge: (Phase, MorphismId),
    _seal: (),
}

impl CeilingCrossing {
    /// Authorize crossing the ceiling on `(from, morphism)` with `cred`. Refuses
    /// non-edges, golden edges (no witness needed), and under-scoped credentials.
    ///
    /// # Errors
    /// [`CrossingRefused`] when the edge is unknown, golden, or the credential is
    /// too weak for the edge's [`required_scope`].
    pub fn authorize(
        from: Phase,
        morphism: MorphismId,
        cred: &CredScope,
    ) -> Result<Self, CrossingRefused> {
        let tier = edge_tier(from, morphism).ok_or(CrossingRefused::NoSuchEdge(from, morphism))?;
        let EdgeTier::CeilingCrossing(ceiling) = tier else {
            return Err(CrossingRefused::EdgeIsGolden(from, morphism));
        };
        let required = required_scope(from, morphism).expect("a shipped edge has a required scope");
        let provided = cred.provides();
        if !required.satisfied_by(provided) {
            return Err(CrossingRefused::InsufficientCred { required, provided });
        }
        Ok(Self { ceiling, edge: (from, morphism), _seal: () })
    }

    /// The ceiling this witness authorizes crossing.
    #[must_use]
    pub const fn ceiling(&self) -> Ceiling {
        self.ceiling
    }

    /// The edge this witness authorizes.
    #[must_use]
    pub const fn edge(&self) -> (Phase, MorphismId) {
        self.edge
    }
}
