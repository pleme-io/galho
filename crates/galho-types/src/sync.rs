//! `Sync` — typed confirmation point gating a phase transition.
//!
//! Every phase boundary in galho is a Sync: an explicit, signed, tameshi-attested
//! confirmation that the morphism's postconditions hold and the next phase may begin.
//! Five kinds (Automatic / OperatorApproval / ExternalSignal / TimeBased /
//! AttestationGated) cover every real-world gating need observed in the empirical
//! survey of Akeyless's apply-before-merge flow.
//!
//! Sync config is `serde`-shaped so consumers (galho-cli, galho-controller) wire it
//! through `shikumi::TieredConfig` for tiered defaults + hot-reload.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use time::Duration;

use crate::phase::Phase;

/// Alias to make `Sync` namespacing explicit at the public API. The underlying type is
/// `SyncConfig`; this alias maps to the user-facing concept "a sync at a phase boundary."
pub type Sync = SyncConfig;

/// The kind of confirmation required at a phase boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SyncKind {
    /// Controller fires the morphism immediately when the source phase is reached.
    /// Used for metadata-only transitions (e.g. `RecordApproval` after webhooks land).
    Automatic,

    /// N-of-M operator approval required. `roles` lists the named roles that count
    /// toward quorum.
    OperatorApproval { roles: Vec<String>, quorum: u8 },

    /// Wait for a typed signal from an external system before advancing.
    ExternalSignal { source: SignalSource },

    /// Soak period — controller holds at the current phase for at least `soak`
    /// before considering forward movement.
    TimeBased {
        #[serde(with = "duration_serde")]
        soak: Duration,
    },

    /// `kensa verify` against a compliance regime must pass before advancing.
    AttestationGated {
        regime: ComplianceRegime,
        query: String, // kensa query string
    },
}

/// External signal sources that gate `ExternalSignal` sync kinds.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum SignalSource {
    GitHubPrMerge { repo: String, pr_number: u64 },
    JiraTransition { ticket: String, target_status: String },
    CarveGate { stack_root_sha: String },
    GalhoGate { galho: String },
    AdapterSpecific { adapter: String, signal: String },
}

/// Compliance regimes available to `AttestationGated` sync kinds. Open to extension.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ComplianceRegime {
    PciDss4_0,
    SocII,
    FedRampModerate,
    FedRampHigh,
    Iso27001,
    Iso27701,
    Hipaa,
    Dora,
    Fips140_3,
    Custom(String),
}

/// Per-Sync configuration. Shikumi-friendly: derive Serialize/Deserialize, leave
/// the tiered loading to the consumer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncConfig {
    pub kind: SyncKind,
    #[serde(with = "duration_serde")]
    pub timeout: Duration,
    pub on_timeout: TimeoutAction,
    pub attestation_required: bool,
    pub audit_channel: String, // tameshi OutcomeChain channel name
}

impl SyncConfig {
    /// The fleet-prescribed default for a given phase. Operator overrides land via
    /// `shikumi::TieredConfig::extend` in the consumer.
    #[must_use]
    pub fn default_for(phase: Phase) -> Self {
        match phase {
            Phase::Declared => Self {
                kind: SyncKind::Automatic,
                timeout: Duration::seconds(60),
                on_timeout: TimeoutAction::HoldForOperator,
                attestation_required: false,
                audit_channel: "galho.declared".into(),
            },
            Phase::Planned => Self {
                kind: SyncKind::Automatic,
                timeout: Duration::minutes(5),
                on_timeout: TimeoutAction::HoldForOperator,
                attestation_required: true,
                audit_channel: "galho.planned".into(),
            },
            Phase::AppliedPendingReview => Self {
                // The compliance-load-bearing sync — operator approval required.
                kind: SyncKind::OperatorApproval {
                    roles: vec!["reviewer".into(), "security-officer".into()],
                    quorum: 1,
                },
                timeout: Duration::hours(24),
                on_timeout: TimeoutAction::HoldForOperator,
                attestation_required: true,
                audit_channel: "galho.applied_pending_review".into(),
            },
            Phase::ApprovedAwaitingMerge => Self {
                kind: SyncKind::ExternalSignal {
                    source: SignalSource::GitHubPrMerge {
                        repo: String::new(),  // operator overrides
                        pr_number: 0,
                    },
                },
                timeout: Duration::hours(72),
                on_timeout: TimeoutAction::RollBack,
                attestation_required: true,
                audit_channel: "galho.approved_awaiting_merge".into(),
            },
            Phase::Merged => Self {
                kind: SyncKind::Automatic,
                timeout: Duration::minutes(30),
                on_timeout: TimeoutAction::Escalate {
                    ladder: vec!["oncall".into(), "infra-lead".into()],
                },
                attestation_required: true,
                audit_channel: "galho.merged".into(),
            },
            Phase::Verified => Self {
                kind: SyncKind::AttestationGated {
                    regime: ComplianceRegime::SocII,
                    query: "outcome-chain --galho ${galho} --window 24h".into(),
                },
                timeout: Duration::hours(1),
                on_timeout: TimeoutAction::HoldForOperator,
                attestation_required: true,
                audit_channel: "galho.verified".into(),
            },
            Phase::Done | Phase::Destroyed => Self {
                kind: SyncKind::Automatic,
                timeout: Duration::minutes(5),
                on_timeout: TimeoutAction::HoldForOperator,
                attestation_required: true,
                audit_channel: "galho.terminal".into(),
            },
            Phase::RollingBack => Self {
                kind: SyncKind::Automatic,
                timeout: Duration::minutes(30),
                on_timeout: TimeoutAction::Escalate {
                    ladder: vec!["oncall".into()],
                },
                attestation_required: true,
                audit_channel: "galho.rolling_back".into(),
            },
            Phase::RolledBack => Self {
                kind: SyncKind::Automatic,
                timeout: Duration::minutes(5),
                on_timeout: TimeoutAction::HoldForOperator,
                attestation_required: true,
                audit_channel: "galho.rolled_back".into(),
            },
            Phase::Drifted => Self {
                kind: SyncKind::TimeBased {
                    soak: Duration::minutes(5),
                },
                timeout: Duration::hours(1),
                on_timeout: TimeoutAction::Escalate {
                    ladder: vec!["oncall".into()],
                },
                attestation_required: true,
                audit_channel: "galho.drifted".into(),
            },
            Phase::Failed => Self {
                kind: SyncKind::OperatorApproval {
                    roles: vec!["galho-admin".into()],
                    quorum: 1,
                },
                timeout: Duration::hours(168), // 7 days
                on_timeout: TimeoutAction::HoldForOperator,
                attestation_required: true,
                audit_channel: "galho.failed".into(),
            },
        }
    }
}

/// What the controller does when the Sync's timeout fires without a confirmation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TimeoutAction {
    /// Walk the escalation ladder (page each role in sequence).
    Escalate { ladder: Vec<String> },
    /// Fire the typed inverse morphism (`RevertApply` from preview, `Abandon` from earlier phases).
    RollBack,
    /// Pause indefinitely; controller stops attempting forward movement until operator confirms.
    HoldForOperator,
}

/// Receipt of a Sync confirmation. Persisted to OutcomeChain via tameshi.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncReceipt {
    pub galho_name: String,
    pub phase: Phase,
    pub kind: SyncKind,
    pub confirmed_at: time::OffsetDateTime,
    pub confirmed_by: Vec<String>,
    pub attestation: Option<AttestationProof>,
    pub metadata: BTreeMap<String, String>,
}

/// Cryptographic attestation produced by tameshi for a confirmed Sync.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttestationProof {
    pub blake3_hex: String,
    pub ed25519_signature_hex: String,
    pub signer_key_id: String,
    pub chain_position: u64,
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
