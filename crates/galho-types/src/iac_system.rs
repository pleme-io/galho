//! The `IaCSystem` trait — typed adapter contract.
//!
//! Every IaC backend (terraform / crossplane / helm / pulumi / ansible / nix) plugs
//! into galho's algebra by implementing [`IaCSystem`]. The algebra (merge, hash, DAG)
//! is implemented once in galho-types over `S: IaCSystem`; adapters translate between
//! the canonical IR and their native state.
//!
//! v0.1 ships only the trait surface and supporting types. The reference adapter
//! (`galho-terraform`) lands in M3 (per `theory/GALHO.md` §XII.2).

use serde::{Deserialize, Serialize};

/// Stable identifier for an IaC system (`"terraform"`, `"crossplane"`, `"helm"`, ...).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct IaCSystemId(pub String);

impl IaCSystemId {
    #[must_use]
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for IaCSystemId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What "apply" means for an IaC system. galho's reconciler interprets `ResourceStatus`
/// against the adapter's declared semantics (§III.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplySemantics {
    /// Apply either fully succeeds or has no state-visible effect. Examples: Helm
    /// `--atomic`, Nix activation, K8s admission-validated atomic updates.
    Transactional,
    /// Apply may partially succeed; failed resources remain in intermediate state.
    /// State updates to reflect what actually applied; status flags failures.
    /// Examples: Terraform, Ansible, raw cloud-API calls.
    PartialProgress,
    /// No deterministic terminal state; the underlying system continuously reconciles.
    /// State reflects current observed status. Examples: Crossplane, ArgoCD, FluxCD.
    ContinuousReconciliation,
}

/// Where a typed state lives. Adapters know how to read/write at this location.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct StateLocation {
    pub backend: String,       // "s3", "k8s", "local", "consul", "postgres"
    pub uri: String,           // backend-specific URI
}

impl StateLocation {
    #[must_use]
    pub fn local(path: impl Into<String>) -> Self {
        Self {
            backend: "local".into(),
            uri: path.into(),
        }
    }

    #[must_use]
    pub fn s3(uri: impl Into<String>) -> Self {
        Self {
            backend: "s3".into(),
            uri: uri.into(),
        }
    }
}

/// Typed adapter contract. v0.1 locks the marker surface (id / schema / semantics);
/// M3 adds the lifecycle methods (`read_state`, `write_state`, `diff`, `apply`, `drift`,
/// `import`, `extract_cross_refs`) under `async_trait`, mirroring `engenho-controllers::Controller`.
/// Each adapter is independently versioned and adopts the canonical IR via this trait.
pub trait IaCSystem: Send + Sync + 'static {
    fn id() -> IaCSystemId;
    fn schema_version() -> &'static str;
    fn apply_semantics() -> ApplySemantics;
}
