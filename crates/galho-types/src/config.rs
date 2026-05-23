//! `GalhoConfig` — the operator-facing tiered config surface for a galho.
//!
//! Pure `serde` types so consumers wire shikumi independently (no shikumi dep in
//! galho-types). The tier flow:
//!
//! 1. **bare** — zero-opinion minimum (no Sync overrides; default apply-after-merge).
//! 2. **prescribed_default** — fleet-wide opinions (apply-before-merge default;
//!    per-phase SyncConfig::default_for(phase)).
//! 3. **extend(base)** — operator overlay from `~/.config/galho/galho.yaml` or
//!    `<repo>/.galho.yaml`.
//!
//! `discovered` (runtime auto-detect from the local repo / cluster) is consumer-side
//! and not modeled in galho-types.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::phase::Phase;
use crate::sync::SyncConfig;

/// Top-level galho config. The CR's spec is a structurally-equivalent typed value;
/// the file-level `galho.yaml` deserializes into this directly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GalhoConfig {
    /// Per-phase Sync overrides. Phases NOT mentioned use `SyncConfig::default_for`.
    #[serde(default)]
    pub phases: BTreeMap<Phase, SyncConfig>,

    /// Stack-lock TTL (lock auto-releases after this; controller may renew). Default 7 days.
    #[serde(with = "duration_serde", default = "default_stack_lock_ttl")]
    pub stack_lock_ttl: time::Duration,

    /// Default galho TTL (galho auto-destroyed after this; matches GALHO.md §IV.1 default 14d).
    #[serde(with = "duration_serde", default = "default_galho_ttl")]
    pub galho_ttl: time::Duration,

    /// Default write policy. Apply-before-merge is the fleet default per the operator
    /// directive + empirical Akeyless workflow + compliance requirements.
    #[serde(default = "default_write_policy")]
    pub write_policy: WritePolicy,

    /// Rebase policy (hybrid: auto if conflict-free; explicit if conflict).
    #[serde(default = "default_rebase_policy")]
    pub rebase_policy: RebasePolicy,

    /// Drift detection cadence + scope per the spec §IV.8.
    #[serde(default)]
    pub drift_detection: DriftDetection,

    /// Per-galho ACL (which roles can plan / apply / promote).
    #[serde(default)]
    pub acl: GalhoAcl,
}

impl GalhoConfig {
    /// Zero-opinion bare config. No phase overrides; everything via prescribed defaults
    /// at consumer load time.
    #[must_use]
    pub fn bare() -> Self {
        Self {
            phases: BTreeMap::new(),
            stack_lock_ttl: default_stack_lock_ttl(),
            galho_ttl: default_galho_ttl(),
            write_policy: default_write_policy(),
            rebase_policy: default_rebase_policy(),
            drift_detection: DriftDetection::default(),
            acl: GalhoAcl::default(),
        }
    }

    /// Fleet-prescribed default — apply-before-merge + per-phase SyncConfig::default_for.
    #[must_use]
    pub fn prescribed() -> Self {
        let mut cfg = Self::bare();
        for &p in Phase::all() {
            cfg.phases.insert(p, SyncConfig::default_for(p));
        }
        cfg
    }

    /// Merge an operator overlay onto a base config. Per-phase entries in `overlay`
    /// replace the base's; missing entries inherit.
    #[must_use]
    pub fn extend(mut self, overlay: GalhoConfig) -> Self {
        for (phase, sync) in overlay.phases {
            self.phases.insert(phase, sync);
        }
        // Top-level scalars: overlay wins.
        self.stack_lock_ttl = overlay.stack_lock_ttl;
        self.galho_ttl = overlay.galho_ttl;
        self.write_policy = overlay.write_policy;
        self.rebase_policy = overlay.rebase_policy;
        // Drift + ACL: overlay wins entirely (no per-field merge for v0.1).
        self.drift_detection = overlay.drift_detection;
        self.acl = overlay.acl;
        self
    }
}

/// When does the apply against cloud actually happen relative to merge?
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WritePolicy {
    /// Apply happens at the `ApplyToPreview` morphism (before reviewer approval).
    /// The Akeyless / compliance-regulated default.
    ApplyBeforeMerge,
    /// Apply happens at the `Promote` morphism (after PR merge). The GitOps-native
    /// default that respects ★★ GITOPS-NATIVE in pleme-io/CLAUDE.md.
    ApplyAfterMerge,
    /// Operator triggers apply explicitly via `galho apply`. No automatic apply.
    ManualOnly,
}

/// Rebase policy when galho.base falls behind base.current.head.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RebasePolicy {
    /// Auto-rebase always; risk of silent auto-resolutions on conflict.
    Always,
    /// Require explicit `galho rebase`. Safest, highest friction.
    ExplicitOnly,
    /// Auto if conflict-free; explicit if conflict. The locked v0.1 default.
    Hybrid,
}

/// Drift detection cadence + scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cadence", rename_all = "snake_case")]
pub enum DriftDetection {
    Off,
    OnDemand,
    Hourly,
    Daily,
    Continuous,
}

impl Default for DriftDetection {
    fn default() -> Self {
        Self::Off // feature galhos opt-in; root galho is configured explicitly
    }
}

/// Per-galho access control list. Roles are saguão crachá role identifiers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct GalhoAcl {
    pub plan_role: Option<String>,
    pub apply_role: Option<String>,
    pub promote_role: Option<String>,
    pub destroy_role: Option<String>,
}

fn default_stack_lock_ttl() -> time::Duration {
    time::Duration::days(7)
}

fn default_galho_ttl() -> time::Duration {
    time::Duration::days(14)
}

const fn default_write_policy() -> WritePolicy {
    WritePolicy::ApplyBeforeMerge
}

const fn default_rebase_policy() -> RebasePolicy {
    RebasePolicy::Hybrid
}

mod duration_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use time::Duration;

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        d.whole_seconds().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let secs = i64::deserialize(d)?;
        Ok(Duration::seconds(secs))
    }
}
