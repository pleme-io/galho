//! Error taxonomy for galho-types.

use thiserror::Error;

/// All errors emitted by galho-types operations. Adapter errors compose via
/// [`GalhoError::Adapter`] without forcing every consumer to depend on the adapter crate.
#[derive(Debug, Error)]
pub enum GalhoError {
    /// IR validation failure (cycle in dep graph, invalid resource id shape, NaN in float, etc.).
    #[error("IR validation: {0}")]
    Validation(String),

    /// Canonical-bytes emission disagreed with itself across two emissions of the same value.
    /// Indicates a bug in a `CanonicalBytes` impl (non-deterministic ordering, missing tag, etc.).
    #[error("canonical-bytes non-determinism for kind={kind}")]
    CanonicalNonDeterminism { kind: &'static str },

    /// Adapter-side error, opaque to galho-types.
    #[error("adapter error ({system}): {message}")]
    Adapter { system: String, message: String },

    /// State-location parse error.
    #[error("invalid state location: {0}")]
    InvalidStateLocation(String),
}

/// Shared adapter error taxonomy. One enum across every IaCSystem adapter so galho-controller
/// + audit chain don't need adapter-specific knowledge to classify failures.
#[derive(Debug, Error)]
pub enum AdapterError {
    /// I/O error talking to the adapter's underlying storage / API.
    #[error("io: {0}")]
    Io(String),

    /// State-location parse / resolve error (uri malformed, path not found, etc.).
    #[error("state location: {0}")]
    StateLocation(String),

    /// Serialization / deserialization error translating between canonical IR and adapter-native format.
    #[error("translation ({system}): {message}")]
    Translation { system: String, message: String },

    /// Adapter detected schema-version mismatch on the wire.
    #[error("schema mismatch ({system}): expected {expected}, got {actual}")]
    SchemaMismatch {
        system: String,
        expected: String,
        actual: String,
    },

    /// Adapter rejected a plan as un-applyable (e.g. cloud quota; provider validation).
    #[error("plan invalid ({system}): {message}")]
    PlanInvalid { system: String, message: String },

    /// Apply executed but produced an outcome the adapter classified as failure.
    #[error("apply failed ({system}): {message}")]
    ApplyFailed { system: String, message: String },

    /// Adapter doesn't yet implement this operation. M3 will replace many of these
    /// with real impls; adapters before they ship can declare partial readiness.
    #[error("not implemented ({system}): {operation}")]
    NotImplemented {
        system: String,
        operation: &'static str,
    },
}
