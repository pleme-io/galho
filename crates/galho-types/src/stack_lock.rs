//! Stack-aware lock primitive — addresses Atlantis's per-directory-lock deadlock for
//! stacked PRs touching the same Terragrunt module.
//!
//! Empirical context: Akeyless's `akeyless-environments` is a Terragrunt monorepo;
//! stacked PRs sliced by carve commonly touch overlapping modules. A per-directory
//! lock (Atlantis-style) serializes the whole stack against itself. The right
//! grain is the **stack root** — galhos in the same stack join a single lock; the
//! lock releases only when ALL galhos reach Merged OR the stack is abandoned.

use std::collections::BTreeSet;
use std::marker::PhantomData;

use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};

// ============================================================================
// Refined<T, B> — a value statically known to satisfy bounds B.
//
// Local copy of the ishou `Refined<T, Bounds>` shape; the third fleet consumer
// (after ishou-tokens' BoundedFontSize and any other) is the trigger to promote
// this to a shared `pleme-refined` crate. Until then it lives here, gated behind
// a `try_new` smart-constructor so an out-of-bounds value is unconstructible.
// ============================================================================

/// Statically-checked bounds over a value type `T`.
pub trait Bounds<T> {
    /// Validate `value`; return `Err` (with a human phrase) when out of bounds.
    fn validate(value: &T) -> Result<(), RefinedError>;
}

/// A `T` proven to satisfy bounds `B`. The only constructor is [`Refined::try_new`],
/// so an in-bounds invariant holds for every value of this type. Serializes as the
/// bare `T` (`#[serde(transparent)]`); deserialization re-runs the bounds check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Refined<T, B: Bounds<T>> {
    value: T,
    #[serde(skip)]
    _bounds: PhantomData<B>,
}

impl<T, B: Bounds<T>> Refined<T, B> {
    /// Construct, validating against `B`. Returns `Err(RefinedError)` if out of bounds.
    pub fn try_new(value: T) -> Result<Self, RefinedError> {
        B::validate(&value)?;
        Ok(Self {
            value,
            _bounds: PhantomData,
        })
    }

    /// Borrow the inner value (always in-bounds).
    pub fn get(&self) -> &T {
        &self.value
    }

    /// Consume into the inner value.
    pub fn into_inner(self) -> T {
        self.value
    }
}

impl<'de, T, B> Deserialize<'de> for Refined<T, B>
where
    T: Deserialize<'de>,
    B: Bounds<T>,
{
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = T::deserialize(d)?;
        Self::try_new(value).map_err(serde::de::Error::custom)
    }
}

/// Error raised when a [`Refined`] value falls outside its declared bounds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefinedError {
    message: String,
}

impl RefinedError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for RefinedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RefinedError {}

/// Bounds for a stack-lock apply TTL: at least 1 minute, at most 30 days.
/// Below the minimum the lock is effectively a no-op (immediate expiry races);
/// above the maximum a forgotten lock wedges a stack indefinitely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApplyTtlBounds;

impl ApplyTtlBounds {
    /// Minimum permitted TTL (1 minute).
    pub const MIN: Duration = Duration::minutes(1);
    /// Maximum permitted TTL (30 days).
    pub const MAX: Duration = Duration::days(30);
}

impl Bounds<Duration> for ApplyTtlBounds {
    fn validate(value: &Duration) -> Result<(), RefinedError> {
        if *value < Self::MIN {
            return Err(RefinedError::new(
                "stack-lock TTL below minimum (1 minute)",
            ));
        }
        if *value > Self::MAX {
            return Err(RefinedError::new(
                "stack-lock TTL above maximum (30 days)",
            ));
        }
        Ok(())
    }
}

/// A stack-lock TTL proven to lie within [`ApplyTtlBounds`].
pub type BoundedApplyTtl = Refined<Duration, ApplyTtlBounds>;

/// A git SHA acting as the stack root identifier. By convention, the merge-base of
/// every galho in the stack against its target branch.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct StackRoot(pub String);

impl StackRoot {
    #[must_use]
    pub fn new(sha: impl Into<String>) -> Self {
        Self(sha.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for StackRoot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Held lock state for a stack. Acquired when the first galho enters
/// `Planned`; released when ALL holders reach a terminal phase OR the stack is
/// explicitly abandoned.
///
/// Fields are **private**: the only constructors are [`StackLock::acquire`]
/// (fresh lock) and [`StackLock::try_from_parts`] (restore / validated build).
/// `Deserialize` routes through `try_from_parts`, so an invalid lock (empty
/// holders, zero quorum, quorum > holders, inverted lifetime) is unconstructible
/// — the bad state is sealed out at construction, not merely detected later.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StackLock {
    stack_root: StackRoot,
    holders: BTreeSet<String>, // galho names
    #[serde(with = "time::serde::rfc3339")]
    acquired_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    expires_at: OffsetDateTime,
    holder_quorum: u8, // require N holders' release votes to release
}

/// Wire-shape mirror of [`StackLock`] used only to decode into the validated
/// type via [`StackLock::try_from_parts`]. Field names + serde attrs match the
/// sealed struct exactly so on-disk bytes round-trip unchanged.
#[derive(Deserialize)]
struct StackLockWire {
    stack_root: StackRoot,
    holders: BTreeSet<String>,
    #[serde(with = "time::serde::rfc3339")]
    acquired_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    expires_at: OffsetDateTime,
    holder_quorum: u8,
}

impl<'de> Deserialize<'de> for StackLock {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let w = StackLockWire::deserialize(d)?;
        Self::try_from_parts(w.stack_root, w.holders, w.acquired_at, w.expires_at, w.holder_quorum)
            .map_err(serde::de::Error::custom)
    }
}

/// Validation failures for a [`StackLock`] built via [`StackLock::try_from_parts`]
/// (or restored from disk via `Deserialize`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StackLockError {
    /// A lock must have at least one holder.
    EmptyHolders,
    /// Quorum of zero can never be reached.
    ZeroQuorum,
    /// Quorum exceeds the number of holders — unreachable release.
    QuorumExceedsHolders { quorum: u8, holders: usize },
    /// `expires_at` precedes `acquired_at` — the lock is born expired.
    InvertedLifetime,
}

impl std::fmt::Display for StackLockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyHolders => f.write_str("stack lock has no holders"),
            Self::ZeroQuorum => f.write_str("stack lock quorum is zero"),
            Self::QuorumExceedsHolders { quorum, holders } => write!(
                f,
                "stack lock quorum {quorum} exceeds holder count {holders}"
            ),
            Self::InvertedLifetime => {
                f.write_str("stack lock expires_at precedes acquired_at")
            }
        }
    }
}

impl std::error::Error for StackLockError {}

impl StackLock {
    /// Acquire a fresh lock with the first holder. The TTL is a [`BoundedApplyTtl`]
    /// (proven in [`ApplyTtlBounds`]), so the lifetime is always sane by construction.
    #[must_use]
    pub fn acquire(
        stack_root: StackRoot,
        first_holder: impl Into<String>,
        ttl: BoundedApplyTtl,
    ) -> Self {
        let now = OffsetDateTime::now_utc();
        let mut holders = BTreeSet::new();
        holders.insert(first_holder.into());
        Self {
            stack_root,
            holders,
            acquired_at: now,
            expires_at: now + *ttl.get(),
            holder_quorum: 1,
        }
    }

    /// Build a lock from validated parts (restore path + Deserialize). Rejects
    /// the four representable-but-invalid states.
    pub fn try_from_parts(
        stack_root: StackRoot,
        holders: BTreeSet<String>,
        acquired_at: OffsetDateTime,
        expires_at: OffsetDateTime,
        holder_quorum: u8,
    ) -> Result<Self, StackLockError> {
        if holders.is_empty() {
            return Err(StackLockError::EmptyHolders);
        }
        if holder_quorum == 0 {
            return Err(StackLockError::ZeroQuorum);
        }
        if usize::from(holder_quorum) > holders.len() {
            return Err(StackLockError::QuorumExceedsHolders {
                quorum: holder_quorum,
                holders: holders.len(),
            });
        }
        if expires_at < acquired_at {
            return Err(StackLockError::InvertedLifetime);
        }
        Ok(Self {
            stack_root,
            holders,
            acquired_at,
            expires_at,
            holder_quorum,
        })
    }

    /// The stack root this lock guards.
    #[must_use]
    pub fn stack_root(&self) -> &StackRoot {
        &self.stack_root
    }

    /// The release quorum (number of holder release-votes required).
    #[must_use]
    pub fn holder_quorum(&self) -> u8 {
        self.holder_quorum
    }

    /// When the lock was acquired.
    #[must_use]
    pub fn acquired_at(&self) -> OffsetDateTime {
        self.acquired_at
    }

    /// When the lock expires.
    #[must_use]
    pub fn expires_at(&self) -> OffsetDateTime {
        self.expires_at
    }

    /// Add a new holder to the lock. Returns `false` if the galho is already a holder.
    pub fn join(&mut self, galho: impl Into<String>) -> bool {
        self.holders.insert(galho.into())
    }

    /// Remove a holder. Returns `true` if the lock is now empty (releasable).
    pub fn release_holder(&mut self, galho: &str) -> bool {
        self.holders.remove(galho);
        self.holders.is_empty()
    }

    /// Extend the TTL (operator override OR controller renewal).
    pub fn extend(&mut self, additional: time::Duration) {
        self.expires_at += additional;
    }

    /// Has this lock expired?
    #[must_use]
    pub fn is_expired(&self, now: OffsetDateTime) -> bool {
        now > self.expires_at
    }

    /// Number of holders currently in the stack.
    #[must_use]
    pub fn holder_count(&self) -> usize {
        self.holders.len()
    }

    /// Does this galho currently hold the lock?
    #[must_use]
    pub fn holds(&self, galho: &str) -> bool {
        self.holders.contains(galho)
    }
}

/// Result of attempting to acquire/join a stack lock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LockOutcome {
    Acquired { stack_root: StackRoot, holder_count: usize },
    Joined { stack_root: StackRoot, holder_count: usize },
    Conflict { stack_root: StackRoot, message: String },
    Expired { stack_root: StackRoot },
}

/// Errors emitted by stack-lock operations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LockError {
    AlreadyHeld { stack_root: StackRoot, current_holders: usize },
    NotHeld { stack_root: StackRoot },
    NotHolder { stack_root: StackRoot, galho: String },
    Expired { stack_root: StackRoot },
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyHeld { stack_root, current_holders } => write!(
                f,
                "stack lock {stack_root} already held by {current_holders} holder(s)"
            ),
            Self::NotHeld { stack_root } => write!(f, "stack lock {stack_root} not held"),
            Self::NotHolder { stack_root, galho } => {
                write!(f, "galho '{galho}' is not a holder of stack lock {stack_root}")
            }
            Self::Expired { stack_root } => write!(f, "stack lock {stack_root} expired"),
        }
    }
}

impl std::error::Error for LockError {}
