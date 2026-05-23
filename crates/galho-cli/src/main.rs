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

use anyhow::{anyhow, Context, Result};
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

#[derive(Clone, Debug, clap::ValueEnum)]
enum GraphFormat {
    Mermaid,
    Dot,
}

#[derive(Subcommand)]
enum Command {
    /// Create a new galho in the `Declared` phase.
    New {
        /// Typed dependency on another galho. Repeat for multiple deps. Each named
        /// galho must reach `Verified` or `Done` before this galho's `Promote` fires.
        #[arg(long = "depends-on")]
        depends_on: Vec<String>,
    },

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

    /// List every known galho with its current phase. Defaults to all galhos; pass
    /// --terminal to include Done/Destroyed galhos in the output.
    List {
        /// Include terminal-phase galhos (Done, Destroyed) in the listing.
        #[arg(long)]
        terminal: bool,
    },

    /// Print the dependency graph for every known galho — declared deps, satisfied
    /// deps, unmet deps, current phase. Operator UX for "why is this galho's Promote
    /// still blocked?"
    Deps,

    /// Render the dependency graph as Mermaid or DOT syntax. Operator pastes the
    /// Mermaid output directly into a GitHub PR description (GitHub renders Mermaid
    /// natively in fenced ```mermaid blocks). DOT output for graphviz workflows
    /// and IDE plugins.
    Graph {
        /// Output format. Defaults to mermaid.
        #[arg(long, default_value = "mermaid")]
        format: GraphFormat,
    },

    /// Print the operator's typed audit chain from the --root store. Walks the
    /// hash-linked OutcomeChain in sequence order; verifies integrity end-to-end.
    /// With --baseline, verifies the chain against a typed compliance regime and
    /// reports any violations.
    Audit {
        /// Print the N most-recent entries. Default 20. Pass 0 for "all".
        #[arg(long, default_value_t = 20)]
        limit: usize,

        /// Verify the chain against a typed compliance regime baseline.
        /// One of: pci-dss-4-0, soc-ii, fed-ramp-moderate, fed-ramp-high, iso-27001,
        /// iso-27701, hipaa, dora, fips-140-3.
        #[arg(long)]
        baseline: Option<String>,
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

    // Auto-restore from --root if present (cross-session continuity); otherwise in-memory.
    // Knowledge subcommands don't need Runtime state; they query the static KnowledgeBase.
    if matches!(cli.command, Command::Knowledge { .. }) {
        if let Command::Knowledge { query } = cli.command {
            galho_cli::run_knowledge_query(query.into(), cli.output.into())?;
        }
        return Ok(());
    }

    // Audit subcommand reads the chain directly without constructing a Runtime.
    if let Command::Audit { limit, baseline } = &cli.command {
        let root = cli.root.clone().context("--root required for `audit`")?;
        return print_audit(root, *limit, baseline.clone()).await;
    }

    let (rt, persist) = make_runtime(cli.root.clone()).await?;

    match cli.command {
        Command::New { ref depends_on } => {
            let name = cli.galho.clone().context("--galho is required (auto-detection from git lands at M2.5)")?;
            if depends_on.is_empty() {
                rt.new_galho(&name).await?;
            } else {
                rt.new_galho_with_deps(&name, depends_on.clone()).await?;
            }
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
        Command::List { terminal } => {
            print_galho_list(&rt, terminal, &cli.output).await?;
        }
        Command::Deps => {
            print_dep_graph(&rt, &cli.output).await?;
        }
        Command::Graph { format } => {
            print_graph_typed(&rt, format, &cli.output).await?;
        }
        Command::Knowledge { .. } | Command::Audit { .. } => unreachable!("handled above"),
    }

    // Auto-checkpoint on exit when --root is in play.
    if persist {
        rt.checkpoint().await.context("auto-checkpoint failed")?;
    }
    Ok(())
}

/// Build a Runtime + report whether auto-checkpoint should fire on exit.
///
/// When `--root` is set, both the persistent state AND the audit chain share the
/// same on-disk store. Every Runtime transition emits an event that lands in the
/// hash-linked OutcomeChain — surviving across sessions for `galho audit` queries.
async fn make_runtime(root: Option<PathBuf>) -> Result<(Runtime, bool)> {
    use galho_cli::RuntimeBackend;
    use galho_storage::{backends::LocalFsBackend, ChainedOutcomeEmitter, OutcomeChain};
    use galho_types::OutcomeEmitter;
    use std::sync::Arc;

    match root {
        Some(p) => {
            let store = Arc::new(LocalFsBackend::new(p));
            let backend = RuntimeBackend::LocalFs(store.clone());
            // Restore the chain from the same store. New sessions pick up where prior
            // sessions stopped — verify_integrity holds across the whole accumulated chain.
            let chain = Arc::new(OutcomeChain::restore(store).await?);
            let emitter: Arc<dyn OutcomeEmitter> = Arc::new(ChainedOutcomeEmitter::new(chain));
            let rt = Runtime::restore_from(backend, emitter).await?;
            Ok((rt, true))
        }
        None => Ok((Runtime::with_memory(), false)),
    }
}

/// Print the operator's typed audit chain from `<root>`. Reads via `OutcomeChain::restore`
/// + `entries()`; verifies integrity end-to-end. With `baseline`, also runs the typed
/// compliance verifier and reports per-regime status + violations.
async fn print_audit(root: PathBuf, limit: usize, baseline: Option<String>) -> Result<()> {
    use galho_storage::{backends::LocalFsBackend, OutcomeChain};
    use std::sync::Arc;

    let store = Arc::new(LocalFsBackend::new(root));
    let chain = OutcomeChain::restore(store).await?;
    let entries = chain.entries().await?;
    let total = entries.len();

    let slice: Vec<_> = if limit == 0 {
        entries
    } else {
        entries.into_iter().skip(total.saturating_sub(limit)).collect()
    };

    println!("audit chain — {} total entries, showing {}", total, slice.len());
    for entry in &slice {
        let morphism = entry
            .event
            .morphism
            .as_ref()
            .map(|m| m.as_str())
            .unwrap_or("-");
        let from = entry
            .event
            .from_phase
            .as_ref()
            .map(|p| p.as_str())
            .unwrap_or("-");
        let to = entry
            .event
            .to_phase
            .as_ref()
            .map(|p| p.as_str())
            .unwrap_or("-");
        println!(
            "  [{:>4}] {} {} {} {} → {}",
            entry.sequence,
            entry.event.event_type,
            entry.event.galho_name,
            morphism,
            from,
            to,
        );
    }
    let ok = chain.verify_integrity().await?;
    println!(
        "integrity: {}",
        if ok { "OK" } else { "BROKEN" }
    );

    if let Some(baseline_str) = baseline {
        let regime = parse_regime(&baseline_str)?;
        let report = galho_storage::verify_regime(&chain, regime).await?;
        println!();
        println!("compliance baseline: {:?}", report.regime);
        println!("status: {}", report.status);
        println!("rules evaluated: {}", report.rules_evaluated.len());
        println!("violations: {}", report.violations.len());
        for v in &report.violations {
            let galho = v.galho_name.as_deref().unwrap_or("-");
            let seq = v
                .sequence
                .map(|s| s.to_string())
                .unwrap_or_else(|| "-".into());
            println!("  [{}] {} {} {}", seq, v.rule, galho, v.detail);
        }
        if !report.galho_summary.is_empty() {
            println!("per-galho violation counts:");
            for (name, count) in &report.galho_summary {
                println!("  {name}: {count}");
            }
        }
    }
    Ok(())
}

fn parse_regime(s: &str) -> Result<galho_types::ComplianceRegime> {
    use galho_types::ComplianceRegime;
    match s {
        "pci-dss-4-0" | "pci-dss-4.0" | "pci" => Ok(ComplianceRegime::PciDss4_0),
        "soc-ii" | "soc2" | "soc" => Ok(ComplianceRegime::SocII),
        "fed-ramp-moderate" | "fedramp-moderate" => Ok(ComplianceRegime::FedRampModerate),
        "fed-ramp-high" | "fedramp-high" => Ok(ComplianceRegime::FedRampHigh),
        "iso-27001" | "iso27001" => Ok(ComplianceRegime::Iso27001),
        "iso-27701" | "iso27701" => Ok(ComplianceRegime::Iso27701),
        "hipaa" => Ok(ComplianceRegime::Hipaa),
        "dora" => Ok(ComplianceRegime::Dora),
        "fips-140-3" | "fips140" => Ok(ComplianceRegime::Fips140_3),
        other => Err(anyhow!(
            "unknown compliance baseline '{other}' (expected one of: pci-dss-4-0, soc-ii, fed-ramp-moderate, fed-ramp-high, iso-27001, iso-27701, hipaa, dora, fips-140-3)"
        )),
    }
}

async fn confirm(rt: &Runtime, galho: Option<String>, role: String) -> Result<()> {
    let name = galho.context("--galho required")?;
    let outcome = rt.confirm_approval(&name, &role).await?;
    println!("{outcome}");
    print_status(rt, &name).await?;
    Ok(())
}

async fn print_galho_list(
    rt: &Runtime,
    include_terminal: bool,
    output: &OutputFormat,
) -> Result<()> {
    let mut galhos = rt.list_galhos_with_state().await;
    galhos.sort_by(|a, b| a.name.cmp(&b.name));
    let shown: Vec<_> = galhos
        .iter()
        .filter(|g| include_terminal || !g.phase.is_terminal())
        .cloned()
        .collect();
    if matches!(output, OutputFormat::Json) {
        let payload = serde_json::json!({
            "total": galhos.len(),
            "shown": shown.len(),
            "galhos": shown,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }
    println!("{} galhos total, showing {}", galhos.len(), shown.len());
    for g in &shown {
        let label = if g.all_deps_satisfied() {
            g.depends_on.join(",")
        } else {
            g.unmet_deps().join(",")
        };
        let deps_marker = g
            .render_state()
            .as_text_marker(!g.depends_on.is_empty(), &label);
        println!("  {} → {}{}", g.name, g.phase.as_str(), deps_marker);
    }
    Ok(())
}

async fn print_graph_typed(
    rt: &Runtime,
    format: GraphFormat,
    output: &OutputFormat,
) -> Result<()> {
    use galho_cli::DepGraph;
    let snaps = rt.list_galhos_with_state().await;
    let graph = DepGraph::new(snaps);
    // --output json overrides --format (the typed JSON surface is the algebra-faithful
    // representation; mermaid/dot are pretty-renderers for humans/PRs).
    if matches!(output, OutputFormat::Json) {
        println!("{}", serde_json::to_string_pretty(&graph.to_json_value())?);
        return Ok(());
    }
    match format {
        GraphFormat::Mermaid => print!("{}", graph.to_mermaid()),
        GraphFormat::Dot => print!("{}", graph.to_dot()),
    }
    Ok(())
}

async fn print_dep_graph(rt: &Runtime, output: &OutputFormat) -> Result<()> {
    let mut galhos = rt.list_galhos_with_state().await;
    galhos.sort_by(|a, b| a.name.cmp(&b.name));
    let total = galhos.len();
    let with_deps: Vec<_> = galhos.iter().filter(|g| !g.depends_on.is_empty()).collect();
    if matches!(output, OutputFormat::Json) {
        let payload = serde_json::json!({
            "total": total,
            "with_deps": with_deps.len(),
            "galhos": with_deps.iter().map(|g| (*g).clone()).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }
    println!(
        "dependency graph — {} galhos total, {} with declared deps",
        total,
        with_deps.len()
    );
    if with_deps.is_empty() {
        println!("(no declared dependencies)");
        return Ok(());
    }
    for g in &with_deps {
        let unmet: Vec<_> = g.unmet_deps().into_iter().collect();
        let status = if g.all_deps_satisfied() {
            "satisfied"
        } else {
            "blocked"
        };
        println!(
            "  {} [{}] depends_on: [{}] — {}",
            g.name,
            g.phase.as_str(),
            g.depends_on.join(", "),
            status,
        );
        if !unmet.is_empty() {
            println!("    unmet: {}", unmet.join(", "));
        }
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

