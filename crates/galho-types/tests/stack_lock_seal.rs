//! Seal-proof tests for the sealed `StackLock` + bounded apply TTL.
//!
//! The lock's fields are private; the only constructors validate the four
//! representable-but-invalid states, and `Deserialize` routes through that same
//! validation. The TTL is a `Refined<Duration, ApplyTtlBounds>` so an
//! out-of-range lifetime is unconstructible.
//!
//! Tests live under tests/ → exempt from the format!() ratchet.

use galho_types::{BoundedApplyTtl, StackLock};
use time::Duration;

#[test]
fn bounded_ttl_rejects_below_min_and_above_max() {
    assert!(
        BoundedApplyTtl::try_new(Duration::seconds(1)).is_err(),
        "1s below the 1-minute floor must be rejected"
    );
    assert!(
        BoundedApplyTtl::try_new(Duration::days(31)).is_err(),
        "31d above the 30-day ceiling must be rejected"
    );
    assert!(
        BoundedApplyTtl::try_new(Duration::days(7)).is_ok(),
        "7d within bounds is accepted"
    );
}

#[test]
fn deserialize_rejects_empty_holders() {
    let payload = serde_json::json!({
        "stack_root": "abc",
        "holders": [],
        "acquired_at": "2026-01-01T00:00:00Z",
        "expires_at": "2026-01-02T00:00:00Z",
        "holder_quorum": 1
    });
    let res: Result<StackLock, _> = serde_json::from_value(payload);
    assert!(res.is_err(), "empty holders must fail to deserialize");
}

#[test]
fn deserialize_rejects_zero_quorum() {
    let payload = serde_json::json!({
        "stack_root": "abc",
        "holders": ["feature/a"],
        "acquired_at": "2026-01-01T00:00:00Z",
        "expires_at": "2026-01-02T00:00:00Z",
        "holder_quorum": 0
    });
    let res: Result<StackLock, _> = serde_json::from_value(payload);
    assert!(res.is_err(), "zero quorum must fail to deserialize");
}

#[test]
fn deserialize_rejects_quorum_exceeding_holders() {
    let payload = serde_json::json!({
        "stack_root": "abc",
        "holders": ["feature/a"],
        "acquired_at": "2026-01-01T00:00:00Z",
        "expires_at": "2026-01-02T00:00:00Z",
        "holder_quorum": 2
    });
    let res: Result<StackLock, _> = serde_json::from_value(payload);
    assert!(res.is_err(), "quorum > holders must fail to deserialize");
}

#[test]
fn deserialize_accepts_well_formed_lock_and_roundtrips() {
    // Acquire a real lock, serialize it, then deserialize it back unchanged.
    let lock = StackLock::acquire(
        galho_types::StackRoot::new("abc"),
        "feature/a",
        BoundedApplyTtl::try_new(Duration::days(7)).unwrap(),
    );
    let bytes = serde_json::to_vec(&lock).unwrap();
    let back: StackLock = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(back, lock, "well-formed lock round-trips byte-for-byte");
    assert_eq!(back.stack_root().as_str(), "abc");
    assert_eq!(back.holder_quorum(), 1);
}
