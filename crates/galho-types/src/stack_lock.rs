//! Stack-aware lock primitive — addresses Atlantis's per-directory-lock deadlock for
//! stacked PRs touching the same Terragrunt module.
//!
//! Empirical context: Akeyless's `akeyless-environments` is a Terragrunt monorepo;
//! stacked PRs sliced by carve commonly touch overlapping modules. A per-directory
//! lock (Atlantis-style) serializes the whole stack against itself. The right
//! grain is the **stack root** — galhos in the same stack join a single lock; the
//! lock releases only when ALL galhos reach Merged OR the stack is abandoned.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackLock {
    pub stack_root: StackRoot,
    pub holders: BTreeSet<String>, // galho names
    #[serde(with = "time::serde::rfc3339")]
    pub acquired_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    pub holder_quorum: u8, // require N holders' release votes to release
}

impl StackLock {
    /// Acquire a fresh lock with the first holder.
    #[must_use]
    pub fn acquire(
        stack_root: StackRoot,
        first_holder: impl Into<String>,
        ttl: time::Duration,
    ) -> Self {
        let now = OffsetDateTime::now_utc();
        let mut holders = BTreeSet::new();
        holders.insert(first_holder.into());
        Self {
            stack_root,
            holders,
            acquired_at: now,
            expires_at: now + ttl,
            holder_quorum: 1,
        }
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
