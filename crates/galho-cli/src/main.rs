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

    /// Fire `RecordApproval` (AppliedPendingReview → ApprovedAwaitingMerge).
    Approve {
        /// Approver role (must match `OperatorApproval` sync's `roles`).
        #[arg(long)]
        role: String,
    },

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

    // Local-only mode for v0.1. M4 will add `--controller <endpoint>` for controller-attached mode.
    let rt = match cli.root {
        Some(p) => Runtime::with_local_fs(p).await?,
        None => Runtime::with_memory(),
    };

    match cli.command {
        Command::New => {
            let name = cli.galho.context("--galho is required (auto-detection from git lands at M2.5)")?;
            rt.new_galho(&name).await?;
            print_status(&rt, &name).await?;
        }
        Command::Status => {
            let name = cli.galho.context("--galho required")?;
            print_status(&rt, &name).await?;
        }
        Command::Plan => fire(&rt, cli.galho, MorphismId::Plan, None).await?,
        Command::Apply { stack_root } => fire(&rt, cli.galho, MorphismId::ApplyToPreview, Some(stack_root)).await?,
        Command::Approve { role } => fire(&rt, cli.galho, MorphismId::RecordApproval, Some(role)).await?,
        Command::Promote => fire(&rt, cli.galho, MorphismId::Promote, None).await?,
        Command::Verify => fire(&rt, cli.galho, MorphismId::Verify, None).await?,
        Command::Done => fire(&rt, cli.galho, MorphismId::SealDone, None).await?,
        Command::Rollback => fire(&rt, cli.galho, MorphismId::RevertApply, None).await?,
        Command::Resume => fire(&rt, cli.galho, MorphismId::Resume, None).await?,
        Command::Reconcile => fire(&rt, cli.galho, MorphismId::DriftReconcile, None).await?,
        Command::Recover => fire(&rt, cli.galho, MorphismId::Recover, None).await?,
        Command::Abandon => fire(&rt, cli.galho, MorphismId::Abandon, None).await?,
        Command::Escalate => fire(&rt, cli.galho, MorphismId::Escalate, None).await?,
        Command::Knowledge { query } => galho_cli::run_knowledge_query(query.into(), cli.output.into())?,
    }

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

