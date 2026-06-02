//! Tests for the `AppliedStatus` smart-constructor + its serde wire shape.
//!
//! The forged all-zeros `Applied` hash is made unrepresentable by the
//! `AppliedStatus::new` smart-ctor. The on-wire JSON stays byte-identical to
//! the previous struct-variant shape (`{"kind":"applied",...}`) so persisted
//! state + content hashes are unaffected.
//!
//! Tests live under tests/ → exempt from the format!() ratchet.

use galho_types::{AppliedStatus, Blake3Hash, GalhoError, ResourceStatus};
use time::OffsetDateTime;

#[test]
fn applied_status_new_rejects_zero_hash() {
    let err = AppliedStatus::new(7, Blake3Hash::from([0u8; 32]), OffsetDateTime::UNIX_EPOCH)
        .expect_err("zero hash must be rejected");
    assert!(matches!(err, GalhoError::ZeroAppliedHash), "{err:?}");
}

#[test]
fn applied_status_new_accepts_real_hash() {
    let hash = Blake3Hash::digest(b"real-apply-content");
    assert_ne!(hash.0, [0u8; 32], "digest must be non-zero");
    let applied = AppliedStatus::new(7, hash.clone(), OffsetDateTime::UNIX_EPOCH)
        .expect("real hash accepted");
    assert_eq!(applied.generation(), 7);
    assert_eq!(applied.hash(), &hash);
    assert_eq!(applied.applied_at(), OffsetDateTime::UNIX_EPOCH);
}

#[test]
fn applied_status_serde_wire_shape_unchanged() {
    let hash = Blake3Hash::digest(b"x");
    let status = ResourceStatus::Applied(
        AppliedStatus::new(3, hash, OffsetDateTime::UNIX_EPOCH).unwrap(),
    );
    let json: serde_json::Value = serde_json::to_value(&status).unwrap();
    // Internally-tagged shape: {"kind":"applied","generation":3,"hash":"...","applied_at":"..."}
    assert_eq!(json["kind"], "applied");
    assert_eq!(json["generation"], 3);
    assert!(json["hash"].is_string(), "hash serializes as hex string: {json}");
    assert!(json["applied_at"].is_string(), "applied_at is rfc3339: {json}");

    // Round-trips back to an equal value.
    let back: ResourceStatus = serde_json::from_value(json).unwrap();
    assert_eq!(back, status);
}
