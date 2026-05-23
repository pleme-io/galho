//! galho — the operator CLI.
//!
//! Each subcommand maps to exactly one typed `PhaseMorphism` from galho-types §IV.10.
//! The CLI is a thin operator-UX layer; all algebra lives in galho-types and persistence
//! in galho-storage. v0.1 ships local-only mode using `MemoryBackend`; controller-attached
//! mode (talking to galho-controller on engenho) lands at M4.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]
#![allow(clippy::missing_errors_doc, clippy::module_name_repetitions)]

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use galho_cli::Runtime;
use galho_types::MorphismId;

#[derive(Parser)]
#[command(name = "galho", version, about = "branch-aware typed IaC state — operator CLI")]
struct Cli {
    /// Path to the local DAG root (for local-fs backend). Defaults to `.galho/`.
    #[arg(long, global = true)]
    root: Option<PathBuf>,

    /// Galho name. Defaults to the current git branch (when implemented; v0.1 requires --galho).
    #[arg(short, long, global = true)]
    galho: Option<String>,

    /// Output format.
    #[arg(short, long, global = true, default_value = "text")]
    output: OutputFormat,

    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Debug, clap::ValueEnum)]
enum OutputFormat {
    Text,
    Json,
}

#[derive(Subcommand)]
enum Command {
    /// Create a new galho in the `Declared` phase.
    New,

    /// Show current phase + available forward / backward morphisms + active Sync.
    Status,

    /// Fire the `Plan` morphism (Declared → Planned).
    Plan,

    /// Fire `ApplyToPreview` (Planned → AppliedPendingReview) — the compliance-load-bearing
    /// apply-before-merge step.
    Apply {
        /// Acquire / join the stack lock on this stack-root SHA.
        #[arg(long)]
        stack_root: String,
    },

    /// Record an OperatorApproval sync confirmation. When the typed quorum is reached,
    /// the next `approve` (or controller tick) advances the phase. Validates `role`
    /// against the active sync's declared roles.
    Confirm {
        /// Reviewer role (must appear in the active OperatorApproval sync's `roles` list).
        #[arg(long)]
        role: String,
    },

    /// Fire `RecordApproval` (AppliedPendingReview → ApprovedAwaitingMerge). Requires
    /// the typed quorum at the current phase to have been reached via prior `confirm`
    /// calls — failure surfaces `ApprovalQuorumMissing`.
    Approve {
        /// Approver role (must match `OperatorApproval` sync's `roles`).
        #[arg(long)]
        role: String,
    },

    /// Manually checkpoint the current Runtime state to the --root store. With --root,
    /// every CLI invocation already auto-checkpoints; this command is for forcing a
    /// checkpoint mid-session.
    Checkpoint,

    /// Fire `Promote` (ApprovedAwaitingMerge → Merged). Commit-only per ★★ GITOPS-NATIVE —
    /// never touches cloud directly.
    Promote,

    /// Fire `Verify` (Merged → Verified).
    Verify,

    /// Fire `SealDone` (Verified → Done).
    Done,

    /// Fire `RevertApply` (Applied/Approved → RollingBack → RolledBack).
    Rollback,

    /// Fire `Resume` (RolledBack → Planned).
    Resume,

    /// Fire `DriftReconcile` (Verified ↔ Drifted).
    Reconcile,

    /// Fire `Recover` (Failed → Declared).
    Recover,

    /// Fire `Abandon` (any non-terminal → Destroyed).
    Abandon,

    /// Fire `Escalate` (operational phase → Failed).
    Escalate,

    /// Inspect the knowledge graph: phases, transitions, syncs, reachability.
    Knowledge {
        #[command(subcommand)]
        query: KnowledgeQuery,
    },
}

#[derive(Subcommand)]
enum KnowledgeQuery {
    /// List all phases in declaration order.
    Phases,
    /// Show the transition table.
    Transitions,
    /// Show available morphisms from a given phase.
    From {
        phase: String,
    },
    /// Show the Sync config gating a given phase.
    Sync {
        phase: String,
    },
    /// Is `target` reachable from `start`?
    Reachable {
        start: String,
        target: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    // Auto-restore from --root if present (cross-session continuity); otherwise in-memory.
    // Knowledge subcommands don't need Runtime state; they query the static KnowledgeBase.
    if matches!(cli.command, Command::Knowledge { .. }) {
        if let Command::Knowledge { query } = cli.command {
            galho_cli::run_knowledge_query(query.into(), cli.output.into())?;
        }
        return Ok(());
    }

    let (rt, persist) = make_runtime(cli.root.clone()).await?;

    match cli.command {
        Command::New => {
            let name = cli.galho.clone().context("--galho is required (auto-detection from git lands at M2.5)")?;
            rt.new_galho(&name).await?;
            print_status(&rt, &name).await?;
        }
        Command::Status => {
            let name = cli.galho.clone().context("--galho required")?;
            print_status(&rt, &name).await?;
        }
        Command::Plan => fire(&rt, cli.galho.clone(), MorphismId::Plan, None).await?,
        Command::Apply { ref stack_root } => fire(&rt, cli.galho.clone(), MorphismId::ApplyToPreview, Some(stack_root.clone())).await?,
        Command::Confirm { ref role } => confirm(&rt, cli.galho.clone(), role.clone()).await?,
        Command::Approve { ref role } => fire(&rt, cli.galho.clone(), MorphismId::RecordApproval, Some(role.clone())).await?,
        Command::Promote => fire(&rt, cli.galho.clone(), MorphismId::Promote, None).await?,
        Command::Verify => fire(&rt, cli.galho.clone(), MorphismId::Verify, None).await?,
        Command::Done => fire(&rt, cli.galho.clone(), MorphismId::SealDone, None).await?,
        Command::Rollback => fire(&rt, cli.galho.clone(), MorphismId::RevertApply, None).await?,
        Command::Resume => fire(&rt, cli.galho.clone(), MorphismId::Resume, None).await?,
        Command::Reconcile => fire(&rt, cli.galho.clone(), MorphismId::DriftReconcile, None).await?,
        Command::Recover => fire(&rt, cli.galho.clone(), MorphismId::Recover, None).await?,
        Command::Abandon => fire(&rt, cli.galho.clone(), MorphismId::Abandon, None).await?,
        Command::Escalate => fire(&rt, cli.galho.clone(), MorphismId::Escalate, None).await?,
        Command::Checkpoint => {
            rt.checkpoint().await.context("checkpoint failed")?;
            println!("checkpoint OK");
        }
        Command::Knowledge { .. } => unreachable!("handled above"),
    }

    // Auto-checkpoint on exit when --root is in play.
    if persist {
        rt.checkpoint().await.context("auto-checkpoint failed")?;
    }
    Ok(())
}

/// Build a Runtime + report whether auto-checkpoint should fire on exit.
async fn make_runtime(root: Option<PathBuf>) -> Result<(Runtime, bool)> {
    use galho_cli::RuntimeBackend;
    use galho_storage::backends::LocalFsBackend;
    use galho_types::{LogOutcomeEmitter, OutcomeEmitter};
    use std::sync::Arc;

    match root {
        Some(p) => {
            let backend = RuntimeBackend::LocalFs(Arc::new(LocalFsBackend::new(p)));
            let emitter: Arc<dyn OutcomeEmitter> = Arc::new(LogOutcomeEmitter);
            let rt = Runtime::restore_from(backend, emitter).await?;
            Ok((rt, true))
        }
        None => Ok((Runtime::with_memory(), false)),
    }
}

async fn confirm(rt: &Runtime, galho: Option<String>, role: String) -> Result<()> {
    let name = galho.context("--galho required")?;
    let outcome = rt.confirm_approval(&name, &role).await?;
    println!("{outcome}");
    print_status(rt, &name).await?;
    Ok(())
}

async fn fire(
    rt: &Runtime,
    galho: Option<String>,
    morphism: MorphismId,
    extra: Option<String>,
) -> Result<()> {
    let name = galho.context("--galho required")?;
    rt.fire_morphism(&name, morphism, extra).await?;
    print_status(rt, &name).await?;
    Ok(())
}

async fn print_status(rt: &Runtime, name: &str) -> Result<()> {
    let s = rt.status(name).await?;
    println!("galho: {}", s.name);
    println!("  phase:    {} ({})", s.phase, s.phase.class_str());
    println!("  forward:  {}", join_morphisms(&s.forward));
    println!("  backward: {}", join_morphisms(&s.backward));
    if let Some(sync) = &s.sync_summary {
        println!("  sync:     {sync}");
    }
    if let Some(lock) = &s.stack_lock_root {
        println!("  stack:    {lock} ({} holders)", s.stack_lock_holders);
    }
    Ok(())
}

fn join_morphisms(ms: &[MorphismId]) -> String {
    if ms.is_empty() {
        "—".into()
    } else {
        ms.iter().map(|m| m.as_str()).collect::<Vec<_>>().join(", ")
    }
}

// Bridge clap's KnowledgeQuery to the library type.
impl From<KnowledgeQuery> for galho_cli::KnowledgeQuery {
    fn from(q: KnowledgeQuery) -> Self {
        match q {
            KnowledgeQuery::Phases => galho_cli::KnowledgeQuery::Phases,
            KnowledgeQuery::Transitions => galho_cli::KnowledgeQuery::Transitions,
            KnowledgeQuery::From { phase } => galho_cli::KnowledgeQuery::From(phase),
            KnowledgeQuery::Sync { phase } => galho_cli::KnowledgeQuery::Sync(phase),
            KnowledgeQuery::Reachable { start, target } => {
                galho_cli::KnowledgeQuery::Reachable { start, target }
            }
        }
    }
}

impl From<OutputFormat> for galho_cli::OutputFormat {
    fn from(f: OutputFormat) -> Self {
        match f {
            OutputFormat::Text => Self::Text,
            OutputFormat::Json => Self::Json,
        }
    }
}

