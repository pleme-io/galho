//! galho-controller — the `engenho_controllers::Controller` impl for galho.
//!
//! On each tick:
//!
//! 1. Iterate every galho known to the embedded `Runtime`.
//! 2. For each galho whose current phase has an `Automatic` Sync, attempt the
//!    available forward morphism. If preconditions are satisfied, the morphism
//!    fires and the phase advances; if not, the galho is skipped (counted in the
//!    ReconcileReport).
//! 3. For non-Automatic Syncs (OperatorApproval / ExternalSignal / TimeBased /
//!    AttestationGated), the controller does NOT fire — the corresponding signal
//!    lands via a webhook ingress (M4.5) or operator-driven CLI invocation.
//!
//! Shares the `Runtime` logic with `galho-cli` so CLI-driven moves and
//! controller-driven moves go through identical preconditions + flag-update logic.
//! This guarantees CLI and controller stay in lockstep — the same galho in the same
//! phase reaches the same next phase regardless of who fired the morphism.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]
#![allow(clippy::missing_errors_doc, clippy::module_name_repetitions)]

use std::sync::Arc;

use async_trait::async_trait;
use engenho_controllers::{Controller, ControllerError, ReconcileReport};
use galho_cli::Runtime;
use galho_types::{KnowledgeBase, MorphismId, Phase, SignalSource, SyncKind};

/// The galho controller. Wraps an `Arc<Runtime>` so multiple consumers (the controller's
/// tick loop, a webhook handler at M4.5, an MCP server at M5+) can share state.
pub struct GalhoController {
    runtime: Arc<Runtime>,
    kb: KnowledgeBase,
}

impl GalhoController {
    #[must_use]
    pub fn new(runtime: Arc<Runtime>) -> Self {
        Self {
            runtime,
            kb: KnowledgeBase::default(),
        }
    }

    #[must_use]
    pub fn runtime(&self) -> &Arc<Runtime> {
        &self.runtime
    }

    /// Webhook ingress: deliver an external signal (GitHub PR-merge, carve-gate
    /// pass, Jira transition) to a galho parked at an `ExternalSignal` sync.
    /// Thin pass-through to the shared `Runtime::deliver_signal` so CLI-driven
    /// and controller-driven signal handling go through identical typed logic.
    pub async fn deliver_signal(
        &self,
        galho: &str,
        signal: SignalSource,
    ) -> Result<galho_cli::SignalOutcome, ControllerError> {
        let outcome = self
            .runtime
            .deliver_signal(galho, signal)
            .await
            .map_err(internal_error)?;
        tracing::info!(
            galho = %outcome.galho,
            from = %outcome.from_phase,
            to = %outcome.to_phase,
            morphism = %outcome.morphism,
            "controller delivered external signal"
        );
        Ok(outcome)
    }
}

#[async_trait]
impl Controller for GalhoController {
    fn name(&self) -> &'static str {
        "galho"
    }

    async fn tick(&self) -> Result<ReconcileReport, ControllerError> {
        let names = self.runtime.list_galhos().await;
        let mut report = ReconcileReport {
            objects_examined: names.len(),
            ..Default::default()
        };

        for name in &names {
            // Re-read current phase each iteration (the previous galho's tick could
            // have transitively affected this one in M5+ multi-galho scenarios).
            let Ok(status) = self.runtime.status(name).await else {
                report.objects_skipped += 1;
                continue;
            };
            let phase: Phase = status.phase.0;
            // Skip terminal phases — no work to do.
            if phase.is_terminal() {
                report.objects_skipped += 1;
                continue;
            }
            // Skip non-Automatic syncs — those advance via webhook or operator action.
            let Some(sync_cfg) = self.kb.sync_for(phase) else {
                report.objects_skipped += 1;
                continue;
            };
            if !matches!(sync_cfg.kind, SyncKind::Automatic) {
                report.objects_skipped += 1;
                continue;
            }

            // Find the available forward morphism. If multiple, the controller picks the
            // first; multi-target morphisms (RevertApply / Escalate / Abandon) aren't
            // among the forward-arc set, so this is deterministic.
            let forward = self.kb.forward_morphisms_from(phase);
            let Some(&morphism) = forward.first() else {
                report.objects_skipped += 1;
                continue;
            };

            match self.runtime.fire_morphism(name, morphism, None).await {
                Ok(new_phase) => {
                    tracing::info!(
                        galho = %name,
                        from = %phase,
                        to = %new_phase,
                        morphism = %morphism,
                        "controller advanced galho"
                    );
                    report.objects_changed += 1;
                }
                Err(_) => {
                    // Preconditions not satisfied OR missing operator signal — skip
                    // until next tick when the world may have advanced.
                    report.objects_skipped += 1;
                }
            }
        }

        if report.objects_changed > 0 {
            report.note = Some(format!(
                "advanced {} galho(s) of {} examined",
                report.objects_changed, report.objects_examined
            ));
        }
        Ok(report)
    }
}

/// Re-export of the morphism id type for downstream consumers that drive the controller
/// directly (e.g. webhook handlers picking a specific morphism rather than going through
/// the controller's tick loop).
pub use galho_types::{MorphismId as ControllerMorphismId, Phase as ControllerPhase};

/// Build a `ControllerError::Internal` from any error-shaped value. Convenience for
/// upstream consumers wrapping galho operations.
#[must_use]
pub fn internal_error(msg: impl std::fmt::Display) -> ControllerError {
    ControllerError::Internal(msg.to_string())
}

// Silence unused-import warning until M4.5 introduces a webhook handler that uses MorphismId directly.
#[allow(dead_code)]
fn _morphism_id_marker(_: MorphismId) {}
