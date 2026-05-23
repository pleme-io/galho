//! Terraform tfstate v4 JSON schema — the on-wire format that magma + terraform
//! emit/consume. Sufficient surface for galho-terraform's round-trip + state-read
//! flows; full provider-quirk handling lands at M3.1.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Top-level tfstate v4 envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tfstate {
    pub version: u32,
    pub terraform_version: String,
    #[serde(default)]
    pub serial: u64,
    pub lineage: String,
    #[serde(default)]
    pub outputs: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub resources: Vec<TfstateResource>,
}

impl Tfstate {
    /// Construct an empty tfstate with a fresh lineage and zero serial.
    #[must_use]
    pub fn empty(lineage: impl Into<String>) -> Self {
        Self {
            version: 4,
            terraform_version: "1.10.0".into(),
            serial: 0,
            lineage: lineage.into(),
            outputs: BTreeMap::new(),
            resources: Vec::new(),
        }
    }

    /// Bump the serial — terraform's monotonic write counter. Called on every write.
    pub fn bump_serial(&mut self) {
        self.serial = self.serial.saturating_add(1);
    }

    /// Parse from JSON bytes.
    pub fn from_json_bytes(bytes: &[u8]) -> Result<Self, TfstateError> {
        serde_json::from_slice(bytes).map_err(|e| TfstateError::Parse(e.to_string()))
    }

    /// Serialize to JSON bytes (pretty-printed for human-readable on-disk format).
    pub fn to_json_bytes(&self) -> Result<Vec<u8>, TfstateError> {
        serde_json::to_vec_pretty(self).map_err(|e| TfstateError::Serialize(e.to_string()))
    }
}

/// One Terraform resource entry. A managed resource + its instances (for `count` /
/// `for_each` expansion).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TfstateResource {
    pub mode: String, // "managed" | "data"
    #[serde(rename = "type")]
    pub kind: String, // e.g. "aws_db_instance"
    pub name: String, // e.g. "main"
    pub provider: String,
    #[serde(default)]
    pub instances: Vec<TfstateInstance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
}

/// One instance of a Terraform resource. `count` / `for_each` resources have multiple;
/// most have exactly one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TfstateInstance {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_version: Option<u32>,
    #[serde(default)]
    pub attributes: serde_json::Value,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_key: Option<serde_json::Value>,
}

/// Errors emitted by tfstate I/O.
#[derive(Debug, Error)]
pub enum TfstateError {
    #[error("parse error: {0}")]
    Parse(String),

    #[error("serialize error: {0}")]
    Serialize(String),

    #[error("unsupported version: {got} (only v4 supported)")]
    UnsupportedVersion { got: u32 },

    #[error("invalid mode: '{0}' (expected 'managed' or 'data')")]
    InvalidMode(String),
}
