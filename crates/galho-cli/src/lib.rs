//! galho-cli — library surface. The `main.rs` binary thin-wraps this library so
//! integration tests can drive the same logic without spawning a process.
//!
//! The library owns:
//!
//! - [`Runtime`] — handle wrapping an `ObjectStore` + a `KnowledgeBase` + an in-memory
//!   `MorphismContext` per galho name. Local-only for v0.1; controller-attached mode
//!   at M4.
//! - [`fire_morphism`](Runtime::fire_morphism) — fires a typed morphism, advances the
//!   galho's phase, updates context flags (e.g. `has_plan` after a `Plan` morphism).
//! - Knowledge queries — `Phases / Transitions / From / Sync / Reachable`.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]
#![allow(clippy::missing_errors_doc, clippy::module_name_repetitions)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use galho_storage::backends::{LocalFsBackend, MemoryBackend};
use galho_types::{
    morphism_for, transition_table, BoundedApplyTtl, KnowledgeBase, LogOutcomeEmitter,
    MorphismContext, MorphismId, OutcomeEmitter, OutcomeEvent, OutcomeEventType, Phase, PhaseClass,
    SignalSource, StackLock, StackRoot, SyncConfig, SyncKind,
};
use serde::Serialize;
use time::Duration;
use tokio::sync::RwLock;

/// Snapshot of a single galho's typed state — phase + declared deps + satisfied deps.
/// Returned by `Runtime::list_galhos_with_state` for CLI rendering.
///
/// Serializes to JSON for MCP / automation / web-UI consumers — the typed surface
/// is the algebra; format is a render concern. All field names are kebab-stable
/// so external consumers can rely on the shape.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GalhoStateSnapshot {
    pub name: String,
    pub phase: Phase,
    pub depends_on: Vec<String>,
    pub deps_satisfied: Vec<String>,
}

impl GalhoStateSnapshot {
    /// Convenience: are all declared deps satisfied?
    #[must_use]
    pub fn all_deps_satisfied(&self) -> bool {
        self.depends_on
            .iter()
            .all(|d| self.deps_satisfied.contains(d))
    }

    /// Convenience: unmet deps for this snapshot.
    #[must_use]
    pub fn unmet_deps(&self) -> Vec<&str> {
        self.depends_on
            .iter()
            .filter(|d| !self.deps_satisfied.contains(d))
            .map(String::as_str)
            .collect()
    }

    /// Typed render-state classification — the single source of truth that
    /// every render surface (Mermaid / DOT / JSON / text) consults. Computed
    /// once from (phase.is_terminal, all_deps_satisfied); each renderer reads
    /// `as_class_name` / `as_color_hex` / `as_marker` off the result rather
    /// than re-branching the underlying state.
    #[must_use]
    pub fn render_state(&self) -> RenderState {
        if self.phase.is_terminal() {
            RenderState::Terminal
        } else if self.all_deps_satisfied() {
            RenderState::Ready
        } else {
            RenderState::Blocked
        }
    }
}

/// Typed render-state classification used by every render surface. The single
/// source of truth for "what does this galho's state look like to a human?" —
/// extracted from three previously-inline branches in MermaidGraph::fmt /
/// DotGraph::fmt / print_galho_list per the org Prime Directive's three-site
/// rule.
///
/// Serializes as a kebab-case string ("terminal" / "ready" / "blocked") for
/// JSON consumers; renders to a Mermaid classDef name, DOT fillcolor hex, and
/// text marker via typed methods. Adding a fourth render surface means adding
/// one method here, not branching anywhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RenderState {
    /// Done or Destroyed — terminal phase. Greyed out.
    Terminal,
    /// Non-terminal, all declared deps satisfied. Green / proceed.
    Ready,
    /// Non-terminal, at least one declared dep unmet. Red / waiting.
    Blocked,
}

impl RenderState {
    /// Mermaid classDef name (must be the same identifier the classDef
    /// declarations at the bottom of the graph use).
    #[must_use]
    pub fn as_class_name(self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::Ready => "ready",
            Self::Blocked => "blocked",
        }
    }

    /// Hex color for DOT fillcolor + Mermaid classDef fill — single source of
    /// truth across both Display impls.
    #[must_use]
    pub fn as_color_hex(self) -> &'static str {
        match self {
            Self::Terminal => "#dddddd",
            Self::Ready => "#ddffdd",
            Self::Blocked => "#ffdddd",
        }
    }

    /// Mermaid classDef stroke color — paired with `as_color_hex` to render
    /// the box outline.
    #[must_use]
    pub fn as_stroke_hex(self) -> &'static str {
        match self {
            Self::Terminal => "#666666",
            Self::Ready => "#339933",
            Self::Blocked => "#993333",
        }
    }

    /// Operator-facing text marker rendered in `galho list`. Returns a typed
    /// `TextMarker<'_>` Display newtype — the typed AST renderer (★★ TYPED
    /// EMISSION surface 3); composes via `write!()` inside its Display impl,
    /// not `format!()`.
    #[must_use]
    pub fn text_marker<'a>(self, has_deps: bool, deps_label: &'a str) -> TextMarker<'a> {
        TextMarker {
            state: self,
            has_deps,
            deps_label,
        }
    }
}

/// Typed Display newtype for the "  deps: ✓ x,y" / "  deps: ⏸ unmet: x,y" /
/// "  (x,y)" marker rendered next to each galho in `galho list`. The Display
/// impl is the typed render surface (★★ TYPED EMISSION surface 1); no
/// `format!()` composition.
pub struct TextMarker<'a> {
    state: RenderState,
    has_deps: bool,
    deps_label: &'a str,
}

impl std::fmt::Display for TextMarker<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !self.has_deps {
            return Ok(());
        }
        match self.state {
            RenderState::Terminal => write!(f, "  ({})", self.deps_label),
            RenderState::Ready => write!(f, "  deps: ✓ {}", self.deps_label),
            RenderState::Blocked => write!(f, "  deps: ⏸ unmet: {}", self.deps_label),
        }
    }
}

/// Typed wrapper over a snapshot of the dependency graph at a moment in time.
/// Provides typed `Display` impls for Mermaid and DOT, plus typed JSON via
/// `to_json_value()` — all three are typed AST renderers per the org-wide ★★
/// TYPED EMISSION rule (no `format!()` for composition; only `writeln!` inside
/// `Display` impls + `serde_json::to_value` for JSON).
///
/// Composes downstream with MCP/web/PR-comment consumers without needing
/// duplicate format logic.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DepGraph {
    pub snapshots: Vec<GalhoStateSnapshot>,
}

/// Typed JSON edge — used by `DepGraph::to_json_value()`. Mirrors the typed edge
/// semantics that Mermaid/DOT renderers express via line style: `satisfied=true`
/// is the solid edge; `satisfied=false` is the dashed edge.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DepGraphJsonEdge {
    pub from: String,
    pub to: String,
    pub satisfied: bool,
}

// ----- Canonical-bytes impls: deterministic, content-addressable surface -----

impl galho_types::CanonicalBytes for GalhoStateSnapshot {
    fn canonical_bytes(&self, sink: &mut galho_types::CanonicalSink) {
        use galho_types::canonical::tag;
        // Tagged-string fields keep the canonical envelope discriminator.
        sink.write_tagged_str(tag::STRING, &self.name);
        sink.write_tagged_str(tag::STRING, self.phase.as_str());
        // Deps are sorted into a BTreeSet so two equivalent inputs (different
        // insertion order) produce byte-identical canonical streams.
        let sorted_deps: std::collections::BTreeSet<&String> = self.depends_on.iter().collect();
        sink.write_tag(tag::LIST);
        sink.write_u32_be(u32::try_from(sorted_deps.len()).expect("deps fit u32"));
        for d in &sorted_deps {
            sink.write_tagged_str(tag::STRING, d);
        }
        let sorted_sat: std::collections::BTreeSet<&String> = self.deps_satisfied.iter().collect();
        sink.write_tag(tag::LIST);
        sink.write_u32_be(u32::try_from(sorted_sat.len()).expect("satisfied fit u32"));
        for d in &sorted_sat {
            sink.write_tagged_str(tag::STRING, d);
        }
    }
}

impl galho_types::CanonicalBytes for DepGraph {
    fn canonical_bytes(&self, sink: &mut galho_types::CanonicalSink) {
        use galho_types::canonical::tag;
        // DepGraph::new() already sorts snapshots by name; defense-in-depth here
        // ensures the canonical stream is order-invariant even if a caller
        // mutates the public snapshots field directly.
        let mut snapshots: Vec<&GalhoStateSnapshot> = self.snapshots.iter().collect();
        snapshots.sort_by(|a, b| a.name.cmp(&b.name));
        sink.write_tag(tag::LIST);
        sink.write_u32_be(u32::try_from(snapshots.len()).expect("snapshots fit u32"));
        for s in &snapshots {
            s.canonical_bytes(sink);
        }
    }
}

impl DepGraph {
    #[must_use]
    pub fn new(mut snapshots: Vec<GalhoStateSnapshot>) -> Self {
        snapshots.sort_by(|a, b| a.name.cmp(&b.name));
        Self { snapshots }
    }

    /// Render as Mermaid `graph LR` syntax suitable for paste into a GitHub PR
    /// description (GitHub renders Mermaid in fenced ```` ```mermaid```` blocks).
    /// Each node carries its phase as a label; each declared dep becomes a typed
    /// directed edge (solid = satisfied, dashed = unmet).
    #[must_use]
    pub fn to_mermaid(&self) -> MermaidGraph<'_> {
        MermaidGraph(self)
    }

    /// Render as Graphviz DOT syntax. Same semantics as `to_mermaid`; different
    /// consumer (operator's graphviz workflow, IDE plugins).
    #[must_use]
    pub fn to_dot(&self) -> DotGraph<'_> {
        DotGraph(self)
    }

    /// Content-addressed signature of this graph — BLAKE3 over canonical bytes
    /// (sorted snapshots, each with sorted deps + sorted satisfied). Two graphs
    /// that compare equal modulo insertion order produce byte-identical hashes.
    /// Operator pastes this into a PR description to prove "the carve stack at
    /// review time was this exact shape" — cryptographically checkable.
    ///
    /// Determinism: proven by proptest (same input → same hash; shuffled-order
    /// input → same hash). The substrate's typed audit story extends to the
    /// operator's dep graph: hash a snapshot, attest later.
    #[must_use]
    pub fn content_hash(&self) -> galho_types::Blake3Hash {
        galho_types::canonical::content_hash(self)
    }

    /// Hex-encoded content hash, suitable for CLI output / PR descriptions.
    #[must_use]
    pub fn content_hash_hex(&self) -> String {
        self.content_hash().to_hex()
    }

    /// Render as a typed JSON object suitable for MCP / web / automation
    /// consumers. Edges carry the typed `satisfied` flag mirroring the
    /// mermaid/dot edge-style encoding; nodes include the typed `render_state`
    /// (terminal/ready/blocked) so JSON consumers see the same classification
    /// the visual renderers see — same algebra across all four surfaces.
    #[must_use]
    pub fn to_json_value(&self) -> serde_json::Value {
        let edges: Vec<DepGraphJsonEdge> = self
            .snapshots
            .iter()
            .flat_map(|s| {
                s.depends_on.iter().map(move |dep| DepGraphJsonEdge {
                    from: dep.clone(),
                    to: s.name.clone(),
                    satisfied: s.deps_satisfied.contains(dep),
                })
            })
            .collect();
        let nodes: Vec<serde_json::Value> = self
            .snapshots
            .iter()
            .map(|s| {
                let mut v = serde_json::to_value(s).expect("snapshot serialize");
                if let serde_json::Value::Object(ref mut obj) = v {
                    obj.insert(
                        "render_state".into(),
                        serde_json::to_value(s.render_state()).expect("render_state serialize"),
                    );
                }
                v
            })
            .collect();
        serde_json::json!({
            "nodes": nodes,
            "edges": edges,
        })
    }
}

/// Display newtype: writes Mermaid syntax for the wrapped DepGraph.
pub struct MermaidGraph<'a>(&'a DepGraph);

impl std::fmt::Display for MermaidGraph<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "graph LR")?;
        if self.0.snapshots.is_empty() {
            writeln!(f, "    %% (no galhos)")?;
            return Ok(());
        }
        for snap in &self.0.snapshots {
            let id = mermaid_id(&snap.name);
            writeln!(
                f,
                "    {id}[\"{name}<br/>{phase}\"]:::{phase_class}",
                name = snap.name,
                phase = snap.phase.as_str(),
                phase_class = snap.render_state().as_class_name(),
            )?;
        }
        for snap in &self.0.snapshots {
            let to_id = mermaid_id(&snap.name);
            for dep in &snap.depends_on {
                let from_id = mermaid_id(dep);
                let edge = if snap.deps_satisfied.contains(dep) {
                    "-->"
                } else {
                    "-.->"
                };
                writeln!(f, "    {from_id} {edge} {to_id}")?;
            }
        }
        // classDef declarations derive from RenderState — single source of truth.
        for state in [RenderState::Ready, RenderState::Blocked, RenderState::Terminal] {
            writeln!(
                f,
                "    classDef {name} fill:{fill},stroke:{stroke}",
                name = state.as_class_name(),
                fill = state.as_color_hex(),
                stroke = state.as_stroke_hex(),
            )?;
        }
        Ok(())
    }
}

/// Display newtype: writes Graphviz DOT syntax for the wrapped DepGraph.
pub struct DotGraph<'a>(&'a DepGraph);

impl std::fmt::Display for DotGraph<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "digraph galho {{")?;
        writeln!(f, "    rankdir=LR;")?;
        writeln!(f, "    node [shape=box, style=rounded];")?;
        if self.0.snapshots.is_empty() {
            writeln!(f, "    // (no galhos)")?;
            writeln!(f, "}}")?;
            return Ok(());
        }
        for snap in &self.0.snapshots {
            writeln!(
                f,
                "    \"{name}\" [label=\"{name}\\n{phase}\", fillcolor=\"{color}\", style=\"rounded,filled\"];",
                name = snap.name,
                phase = snap.phase.as_str(),
                color = snap.render_state().as_color_hex(),
            )?;
        }
        for snap in &self.0.snapshots {
            for dep in &snap.depends_on {
                let style = if snap.deps_satisfied.contains(dep) {
                    "solid"
                } else {
                    "dashed"
                };
                writeln!(f, "    \"{dep}\" -> \"{name}\" [style={style}];", name = snap.name)?;
            }
        }
        writeln!(f, "}}")
    }
}

/// Mermaid identifiers must be alphanumeric + underscore; sanitize a galho name.
fn mermaid_id(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// Detect whether adding `(new_galho, deps)` to the existing dep graph would
/// introduce a cycle. Returns `Some(path)` describing the cycle if found, `None`
/// if cycle-free. The returned path starts at `new_galho` and ends at `new_galho`
/// (i.e. the cycle closes).
///
/// Algorithm: DFS from each dep of `new_galho` through existing transitive deps;
/// if we ever revisit `new_galho`, the path is the cycle.
pub fn detect_dep_cycle(
    contexts: &BTreeMap<String, MorphismContext>,
    new_galho: &str,
    new_deps: &std::collections::BTreeSet<String>,
) -> Option<Vec<String>> {
    // Self-loop is the trivial direct cycle.
    if new_deps.contains(new_galho) {
        return Some(vec![new_galho.to_string(), new_galho.to_string()]);
    }
    // DFS from each declared dep; track visit path; if path revisits new_galho, cycle.
    for dep in new_deps {
        let mut path = vec![new_galho.to_string(), dep.clone()];
        let mut visited: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        if dfs_finds_cycle(contexts, new_galho, dep, &mut path, &mut visited) {
            return Some(path);
        }
    }
    None
}

fn dfs_finds_cycle(
    contexts: &BTreeMap<String, MorphismContext>,
    target: &str,
    current: &str,
    path: &mut Vec<String>,
    visited: &mut std::collections::BTreeSet<String>,
) -> bool {
    if !visited.insert(current.to_string()) {
        return false;
    }
    let Some(ctx) = contexts.get(current) else {
        return false;
    };
    for next in &ctx.depends_on {
        if next == target {
            path.push(next.clone());
            return true;
        }
        path.push(next.clone());
        if dfs_finds_cycle(contexts, target, next, path, visited) {
            return true;
        }
        path.pop();
    }
    false
}

/// Outcome of `Runtime::confirm_approval` — communicates whether the confirmation
/// reached quorum + how many more are needed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalOutcome {
    pub count: u8,
    pub quorum: u8,
    pub quorum_reached: bool,
    pub phase: Phase,
}

impl std::fmt::Display for ApprovalOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.quorum_reached {
            write!(
                f,
                "quorum reached at {} ({}-of-{})",
                self.phase, self.count, self.quorum
            )
        } else {
            write!(
                f,
                "approval recorded at {} ({}-of-{}; {} more needed)",
                self.phase,
                self.count,
                self.quorum,
                self.quorum - self.count
            )
        }
    }
}

/// Outcome of `Runtime::deliver_signal` — the typed result of an external
/// signal (e.g. a GitHub PR-merge webhook) advancing a galho past an
/// `ExternalSignal` sync. Reports the phase transition + the morphism fired.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SignalOutcome {
    pub galho: String,
    pub from_phase: Phase,
    pub to_phase: Phase,
    pub morphism: MorphismId,
}

impl std::fmt::Display for SignalOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "signal advanced {} {} → {} via {}",
            self.galho,
            self.from_phase.as_str(),
            self.to_phase.as_str(),
            self.morphism.as_str(),
        )
    }
}

/// Deploy-order gate report for a single galho — sibling of `carve gate`
/// (PR merge-order) at the IaC-deploy layer. `galho gate <name>` blocks a
/// galho's deploy when an upstream dependency has not reached `Verified`/`Done`,
/// and — under `--require-siblings-ready` — when a sibling galho (one sharing
/// a declared dep) is still `Declared`. A `Failed` self with deps met is NOT
/// blocked: gate sequences deploys, it does not adjudicate self-health.
///
/// Serializes to JSON for CI consumers; the typed `Display` is the text surface.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct GateReport {
    pub galho: String,
    pub phase: Phase,
    /// Declared deps not yet `Verified`/`Done`.
    pub unmet_deps: Vec<String>,
    /// Siblings still `Declared` (only populated under `--require-siblings-ready`).
    pub unready_siblings: Vec<String>,
}

impl GateReport {
    /// The gate blocks when any upstream dep is unmet, or when any sibling is
    /// unready (the latter only collected under `--require-siblings-ready`).
    #[must_use]
    pub fn is_blocked(&self) -> bool {
        !self.unmet_deps.is_empty() || !self.unready_siblings.is_empty()
    }
}

impl std::fmt::Display for GateReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_blocked() {
            write!(f, "gate BLOCKED for {} at {}", self.galho, self.phase.as_str())?;
            if !self.unmet_deps.is_empty() {
                write!(f, "; unmet upstream deps: {}", self.unmet_deps.join(", "))?;
            }
            if !self.unready_siblings.is_empty() {
                write!(f, "; unready siblings: {}", self.unready_siblings.join(", "))?;
            }
            Ok(())
        } else {
            write!(f, "gate OK for {} at {}", self.galho, self.phase.as_str())
        }
    }
}

/// One-line phase + sync summary returned by `Runtime::status`.
#[derive(Debug, Clone)]
pub struct StatusReport {
    pub name: String,
    pub phase: PhaseDisplay,
    pub forward: Vec<MorphismId>,
    pub backward: Vec<MorphismId>,
    pub sync_summary: Option<String>,
    pub stack_lock_root: Option<String>,
    pub stack_lock_holders: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct PhaseDisplay(pub Phase);

impl std::fmt::Display for PhaseDisplay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0.as_str())
    }
}

impl PhaseDisplay {
    #[must_use]
    pub fn class_str(&self) -> &'static str {
        match self.0.class() {
            PhaseClass::Forward => "forward",
            PhaseClass::Backward => "backward",
            PhaseClass::Terminal => "terminal",
            PhaseClass::Failure => "failure",
        }
    }
}

/// The CLI runtime. Wraps an object store + in-memory galho context map + an outcome
/// emitter that observes every state transition. The emitter is pluggable so consumers
/// pick the right destination — `LogOutcomeEmitter` for production CLI use,
/// `MemoryOutcomeEmitter` for tests, future `TameshiOutcomeEmitter` for fleet attestation.
pub struct Runtime {
    backend: RuntimeBackend,
    kb: KnowledgeBase,
    contexts: RwLock<BTreeMap<String, MorphismContext>>,
    locks: RwLock<BTreeMap<String, StackLock>>,
    emitter: Arc<dyn OutcomeEmitter>,
}

#[derive(Debug)]
pub enum RuntimeBackend {
    Memory(Arc<MemoryBackend>),
    LocalFs(Arc<LocalFsBackend>),
}

impl Runtime {
    /// In-memory runtime with `LogOutcomeEmitter`. Fastest path for production use.
    #[must_use]
    pub fn with_memory() -> Self {
        Self::new(
            RuntimeBackend::Memory(Arc::new(MemoryBackend::new())),
            Arc::new(LogOutcomeEmitter),
        )
    }

    /// Local-fs runtime with `LogOutcomeEmitter`.
    pub async fn with_local_fs(root: PathBuf) -> Result<Self> {
        Ok(Self::new(
            RuntimeBackend::LocalFs(Arc::new(LocalFsBackend::new(root))),
            Arc::new(LogOutcomeEmitter),
        ))
    }

    /// Construct with explicit emitter — for tests using `MemoryOutcomeEmitter` or for
    /// future controller deployments using `TameshiOutcomeEmitter`.
    #[must_use]
    pub fn with_emitter(backend: RuntimeBackend, emitter: Arc<dyn OutcomeEmitter>) -> Self {
        Self::new(backend, emitter)
    }

    fn new(backend: RuntimeBackend, emitter: Arc<dyn OutcomeEmitter>) -> Self {
        Self {
            backend,
            kb: KnowledgeBase::default(),
            contexts: RwLock::new(BTreeMap::new()),
            locks: RwLock::new(BTreeMap::new()),
            emitter,
        }
    }

    /// Access the emitter — primarily for tests that need to inspect what was emitted.
    #[must_use]
    pub fn emitter(&self) -> &Arc<dyn OutcomeEmitter> {
        &self.emitter
    }

    /// Internal: emit an event, swallowing errors per the emitter contract (audit
    /// failures never block business logic).
    fn observe(&self, event: OutcomeEvent) {
        if let Err(e) = self.emitter.emit(&event) {
            tracing::warn!("outcome emit failed: {e}");
        }
    }

    /// Confirm an `OperatorApproval` sync at the current phase. Validates the role is
    /// in the sync's allowed-roles list; records the confirmation in the galho's
    /// context; if the quorum is reached, sets `has_approval_quorum` so the next
    /// `fire_morphism(RecordApproval, ...)` can advance.
    ///
    /// Returns an `ApprovalOutcome` describing the count, quorum, and whether the
    /// quorum was reached by this confirmation.
    pub async fn confirm_approval(
        &self,
        galho: &str,
        role: &str,
    ) -> Result<ApprovalOutcome> {
        let mut ctxs = self.contexts.write().await;
        let ctx = ctxs
            .get_mut(galho)
            .ok_or_else(|| anyhow!("galho '{galho}' not found"))?;
        let current_phase = ctx.current_phase;

        let sync = self
            .kb
            .sync_for(current_phase)
            .ok_or_else(|| anyhow!("phase {current_phase:?} has no sync config"))?;

        let (allowed_roles, quorum) = match &sync.kind {
            SyncKind::OperatorApproval { roles, quorum } => (roles.clone(), *quorum),
            _ => {
                return Err(anyhow!(
                    "phase {current_phase:?} sync is not OperatorApproval"
                ));
            }
        };

        if !allowed_roles.iter().any(|r| r == role) {
            return Err(anyhow!(
                "role '{role}' not authorized for sync at phase {current_phase:?} (allowed: {allowed_roles:?})"
            ));
        }

        let confirmations = ctx
            .confirmations
            .entry(current_phase)
            .or_default();
        confirmations.insert(role.to_string());
        let count = confirmations.len() as u8;
        let quorum_reached = count >= quorum;
        if quorum_reached {
            ctx.has_approval_quorum = true;
        }
        drop(ctxs);

        // Emit a SyncConfirmed event regardless of quorum status — every confirmation
        // is a load-bearing audit entry.
        self.observe(
            OutcomeEvent::new(OutcomeEventType::SyncConfirmed, galho)
                .with_phase_transition(current_phase, current_phase)
                .with_sync(sync.kind.clone())
                .with_note(format!(
                    "role={role} count={count}/{quorum}{}",
                    if quorum_reached { " quorum-reached" } else { "" }
                )),
        );

        Ok(ApprovalOutcome {
            count,
            quorum,
            quorum_reached,
            phase: current_phase,
        })
    }

    /// Deliver an external signal (e.g. a GitHub PR-merge webhook, a carve-gate
    /// pass, a Jira transition) to a galho parked at an `ExternalSignal` sync.
    ///
    /// The galho's current phase MUST have an `ExternalSignal` sync, and the
    /// delivered `signal` MUST match the configured source by variant
    /// (`std::mem::discriminant` — a `GitHubPrMerge` signal satisfies a
    /// `GitHubPrMerge`-configured sync regardless of the placeholder
    /// repo/pr_number the fleet default seeds; operator overrides via shikumi
    /// tiered config tighten the match at M4.5). On match, fires the available
    /// forward morphism and returns a typed `SignalOutcome`.
    pub async fn deliver_signal(
        &self,
        galho: &str,
        signal: SignalSource,
    ) -> Result<SignalOutcome> {
        // Read current phase under the lock, then drop it before fire_morphism
        // (which takes its own write lock).
        let current_phase = {
            let ctxs = self.contexts.read().await;
            let ctx = ctxs
                .get(galho)
                .ok_or_else(|| anyhow!("galho '{galho}' not found"))?;
            ctx.current_phase
        };

        let sync = self
            .kb
            .sync_for(current_phase)
            .ok_or_else(|| anyhow!("phase {current_phase:?} has no sync config"))?;

        let configured = match &sync.kind {
            SyncKind::ExternalSignal { source } => source.clone(),
            _ => {
                return Err(anyhow!(
                    "phase {current_phase:?} sync is not ExternalSignal; cannot deliver signal"
                ));
            }
        };

        if std::mem::discriminant(&configured) != std::mem::discriminant(&signal) {
            return Err(anyhow!(
                "delivered signal {signal:?} does not match configured source {configured:?} at phase {current_phase:?}"
            ));
        }

        let forward = self.kb.forward_morphisms_from(current_phase);
        let morphism = *forward.first().ok_or_else(|| {
            anyhow!("no forward morphism available from phase {current_phase:?}")
        })?;

        // Emit a typed SyncConfirmed audit event for the signal delivery
        // (builders only — no ad-hoc note string).
        self.observe(
            OutcomeEvent::new(OutcomeEventType::SyncConfirmed, galho)
                .with_phase_transition(current_phase, current_phase)
                .with_sync(sync.kind.clone()),
        );

        let to_phase = self.fire_morphism(galho, morphism, None).await?;
        Ok(SignalOutcome {
            galho: galho.to_string(),
            from_phase: current_phase,
            to_phase,
            morphism,
        })
    }

    /// Materialize a fresh `MorphismContext` for a new galho. The galho enters in `Declared`.
    pub async fn new_galho(&self, name: &str) -> Result<()> {
        self.new_galho_with_deps(name, Vec::new()).await
    }

    /// Like `new_galho` but the galho declares typed dependencies. `Promote` is gated
    /// until every dep reaches `Verified` or `Done` — operationally enforces the
    /// stacked-PR dependency-ordering invariant carve creates at PR-stack time.
    ///
    /// Refuses to create a galho that would introduce a dependency cycle. The check
    /// walks the would-be dep graph (existing galhos + the new one); on cycle, returns
    /// a typed error naming the cycle path. This prevents the silent "deps never
    /// satisfy" failure mode where a → b → a would forever block both Promotes.
    pub async fn new_galho_with_deps(
        &self,
        name: &str,
        deps: impl IntoIterator<Item = String>,
    ) -> Result<()> {
        let deps: std::collections::BTreeSet<String> = deps.into_iter().collect();
        let mut ctxs = self.contexts.write().await;
        if ctxs.contains_key(name) {
            return Err(anyhow!("galho '{name}' already exists"));
        }
        if let Some(cycle) = detect_dep_cycle(&ctxs, name, &deps) {
            return Err(anyhow!(
                "dependency cycle: {}",
                cycle.join(" → ")
            ));
        }
        let mut ctx = MorphismContext::declared(name);
        ctx.depends_on = deps.clone();
        ctxs.insert(name.to_string(), ctx);
        // Mark this galho as satisfying its own deps for any galhos that already
        // depended on it (rare on creation, but defensive).
        Self::propagate_dep_satisfaction(&mut ctxs);
        drop(ctxs);
        self.observe(
            OutcomeEvent::new(OutcomeEventType::GalhoCreated, name)
                .with_phase_transition(Phase::Declared, Phase::Declared)
                .with_note(if deps.is_empty() {
                    String::new()
                } else {
                    format!("deps={}", deps.iter().cloned().collect::<Vec<_>>().join(","))
                }),
        );
        Ok(())
    }

    /// Recompute every galho's `deps_satisfied` set from the current snapshot of all
    /// galho phases. A dep is satisfied iff the dep-galho exists AND is in Verified
    /// or Done. Run after any phase advancement so downstream galhos see the change.
    fn propagate_dep_satisfaction(ctxs: &mut BTreeMap<String, MorphismContext>) {
        let snapshot: BTreeMap<String, Phase> = ctxs
            .iter()
            .map(|(n, c)| (n.clone(), c.current_phase))
            .collect();
        for ctx in ctxs.values_mut() {
            ctx.deps_satisfied.clear();
            for dep in &ctx.depends_on {
                if let Some(phase) = snapshot.get(dep) {
                    if matches!(phase, Phase::Verified | Phase::Done) {
                        ctx.deps_satisfied.insert(dep.clone());
                    }
                }
            }
        }
    }

    /// Fire a typed morphism. Returns the new phase or the typed precondition failure.
    pub async fn fire_morphism(
        &self,
        name: &str,
        morphism: MorphismId,
        extra: Option<String>,
    ) -> Result<Phase> {
        let _ = morphism_for(morphism).context("unknown morphism id")?;

        // Pre-action side effects + flag flips (so preconditions see fresh state).
        if let MorphismId::ApplyToPreview = morphism {
            let stack_root = extra
                .clone()
                .context("--stack-root required for `apply` (carve stack root SHA)")?;
            self.join_or_acquire_lock(name, &stack_root).await?;
            // Reflect lock acquisition into the context BEFORE precondition check.
            let mut ctxs = self.contexts.write().await;
            if let Some(ctx) = ctxs.get_mut(name) {
                ctx.stack_lock_held = true;
            }
        }
        // RecordApproval: the `has_approval_quorum` flag is driven by `confirm_approval`
        // against the typed OperatorApproval sync. The morphism's pre-action block here
        // intentionally does NOT auto-set the flag — quorum is gated by typed
        // confirmations, not by `fire_morphism` being called.
        //
        // Test fixtures that don't care about quorum may pre-set the flag via
        // `confirm_approval(galho, role)` matching a sync role.
        // Promote: simulate the GitHub merge event having landed.
        if let MorphismId::Promote = morphism {
            let mut ctxs = self.contexts.write().await;
            if let Some(ctx) = ctxs.get_mut(name) {
                ctx.has_merge_event = true;
            }
        }
        // Verify: simulate the verify receipt produced by smoke-tests.
        if let MorphismId::Verify = morphism {
            let mut ctxs = self.contexts.write().await;
            if let Some(ctx) = ctxs.get_mut(name) {
                ctx.has_verify_receipt = true;
            }
        }
        // SealDone: simulate Jira ticket resolvable (M2 wires real Jira sync).
        if let MorphismId::SealDone = morphism {
            let mut ctxs = self.contexts.write().await;
            if let Some(ctx) = ctxs.get_mut(name) {
                ctx.jira_ticket_resolvable = true;
            }
        }

        let mut ctxs = self.contexts.write().await;
        let ctx = ctxs
            .get_mut(name)
            .ok_or_else(|| anyhow!("galho '{name}' not found; run `galho new` first"))?;

        let next_phase = match self.kb.apply_morphism(morphism, ctx) {
            Ok(p) => p,
            Err(missing) => {
                // Emit a MorphismBlocked event so the auditor can see attempted-but-blocked
                // operator actions. Drop ctxs first so the observe() can take its own lock if needed.
                let blocked_phase = ctx.current_phase;
                let note = format!(
                    "missing: {}",
                    missing
                        .iter()
                        .map(|m| format!("{m:?}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                drop(ctxs);
                self.observe(
                    OutcomeEvent::new(OutcomeEventType::MorphismBlocked, name)
                        .with_phase_transition(blocked_phase, blocked_phase)
                        .with_morphism(morphism)
                        .with_note(note.clone()),
                );
                return Err(anyhow!(
                    "preconditions not satisfied for {}: {}",
                    morphism,
                    missing
                        .iter()
                        .map(|m| format!("{m:?}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        };

        let from_phase = ctx.current_phase;
        ctx.current_phase = next_phase;
        Self::update_flags_post_morphism(ctx, morphism, next_phase, extra);
        // Recompute every galho's dep-satisfaction set so downstream galhos see the
        // phase change (e.g. galho-a → Verified unblocks galho-b's Promote).
        Self::propagate_dep_satisfaction(&mut ctxs);
        drop(ctxs);

        // Emit the typed outcome event.
        let sync_kind = self.kb.sync_for(next_phase).map(|s| s.kind.clone());
        let mut event = OutcomeEvent::new(OutcomeEventType::MorphismFired, name)
            .with_phase_transition(from_phase, next_phase)
            .with_morphism(morphism);
        if let Some(k) = sync_kind {
            event = event.with_sync(k);
        }
        // Terminal phases get an additional GalhoDestroyed event.
        let was_destroyed = next_phase == Phase::Destroyed;
        self.observe(event);
        if was_destroyed {
            self.observe(
                OutcomeEvent::new(OutcomeEventType::GalhoDestroyed, name)
                    .with_phase_transition(from_phase, Phase::Destroyed)
                    .with_morphism(morphism),
            );
        }

        Ok(next_phase)
    }

    fn update_flags_post_morphism(
        ctx: &mut MorphismContext,
        morphism: MorphismId,
        next_phase: Phase,
        _extra: Option<String>,
    ) {
        match morphism {
            MorphismId::Plan => ctx.has_plan = true,
            MorphismId::ApplyToPreview => {
                ctx.has_apply_receipt = true;
                ctx.stack_lock_held = true;
            }
            MorphismId::RecordApproval => ctx.has_approval_quorum = true,
            MorphismId::Promote => ctx.has_merge_event = true,
            MorphismId::Verify => ctx.has_verify_receipt = true,
            MorphismId::SealDone => ctx.jira_ticket_resolvable = true,
            // RevertApply is a 2-step morphism: AppliedPendingReview/ApprovedAwaitingMerge
            // → RollingBack → RolledBack. We retain the apply receipt during RollingBack
            // (the reverse traversal depends on it); clear ONLY when we land in RolledBack.
            MorphismId::RevertApply => {
                if next_phase == Phase::RolledBack {
                    ctx.has_apply_receipt = false;
                    ctx.has_approval_quorum = false;
                    ctx.stack_lock_held = false;
                }
            }
            MorphismId::Abandon | MorphismId::Recover => {
                ctx.has_plan = false;
                ctx.has_apply_receipt = false;
                ctx.has_approval_quorum = false;
                ctx.has_merge_event = false;
                ctx.has_verify_receipt = false;
                ctx.jira_ticket_resolvable = false;
                ctx.stack_lock_held = false;
            }
            _ => {}
        }
    }

    async fn join_or_acquire_lock(&self, name: &str, stack_root: &str) -> Result<()> {
        let mut locks = self.locks.write().await;
        let event_type = match locks.get_mut(stack_root) {
            Some(lock) => {
                lock.join(name);
                OutcomeEventType::StackLockJoined
            }
            None => {
                let lock = StackLock::acquire(
                    StackRoot::new(stack_root),
                    name,
                    BoundedApplyTtl::try_new(Duration::days(7))
                        .expect("7d is within ApplyTtlBounds"),
                );
                locks.insert(stack_root.to_string(), lock);
                OutcomeEventType::StackLockAcquired
            }
        };
        drop(locks);
        self.observe(
            OutcomeEvent::new(event_type, name)
                .with_stack_root(StackRoot::new(stack_root)),
        );
        Ok(())
    }

    /// Build a status report — current phase + available morphisms + sync config + stack lock.
    pub async fn status(&self, name: &str) -> Result<StatusReport> {
        let ctxs = self.contexts.read().await;
        let ctx = ctxs
            .get(name)
            .ok_or_else(|| anyhow!("galho '{name}' not found"))?;

        let phase = ctx.current_phase;
        let forward = self.kb.forward_morphisms_from(phase);
        let backward = self.kb.backward_morphisms_from(phase);
        let sync_summary = self.kb.sync_for(phase).map(|s| summarize_sync(s).to_string());

        let locks = self.locks.read().await;
        let (lock_root, lock_holders) = locks
            .iter()
            .find(|(_, l)| l.holds(name))
            .map(|(root, l)| (Some(root.clone()), l.holder_count()))
            .unwrap_or((None, 0));

        Ok(StatusReport {
            name: name.to_string(),
            phase: PhaseDisplay(phase),
            forward,
            backward,
            sync_summary,
            stack_lock_root: lock_root,
            stack_lock_holders: lock_holders,
        })
    }

    /// Direct access to the backend (rare; used for advanced ops + tests).
    #[must_use]
    pub fn backend(&self) -> &RuntimeBackend {
        &self.backend
    }

    /// List every galho name currently known to this Runtime. Sorted lexicographically
    /// (BTreeMap iteration). Used by the controller's tick loop.
    pub async fn list_galhos(&self) -> Vec<String> {
        self.contexts.read().await.keys().cloned().collect()
    }

    /// Snapshot of every galho's (name, current_phase, declared deps, satisfied deps).
    /// Used by `galho list` + `galho deps` subcommands.
    pub async fn list_galhos_with_state(&self) -> Vec<GalhoStateSnapshot> {
        self.contexts
            .read()
            .await
            .iter()
            .map(|(name, ctx)| GalhoStateSnapshot {
                name: name.clone(),
                phase: ctx.current_phase,
                depends_on: ctx.depends_on.iter().cloned().collect(),
                deps_satisfied: ctx.deps_satisfied.iter().cloned().collect(),
            })
            .collect()
    }

    /// Persist every galho context + active stack lock to the runtime's `ObjectStore`,
    /// so a subsequent `restore_from` reconstitutes the same Runtime state.
    ///
    /// Storage layout:
    /// - One content-addressed object per context / lock (BLAKE3-hashed bytes).
    /// - One ref `runtime/contexts/<name>` per galho, exclusive — CAS-updated to point
    ///   at the current context hash.
    /// - One ref `runtime/locks/<root>` per active stack lock.
    ///
    /// Refs are exclusive per name (not meta-blob-indexed) so multiple checkpoints
    /// of the same name advance the ref deterministically — no stale-object shadowing.
    pub async fn checkpoint(&self) -> Result<()> {
        let backend: &dyn galho_storage::ObjectStore = match &self.backend {
            RuntimeBackend::Memory(s) => s.as_ref(),
            RuntimeBackend::LocalFs(s) => s.as_ref(),
        };

        let ctxs = self.contexts.read().await;
        for (name, ctx) in ctxs.iter() {
            let bytes = serde_json::to_vec(ctx)
                .with_context(|| format!("encode context for galho '{name}'"))?;
            let hash = backend
                .put_object(&bytes)
                .await
                .with_context(|| format!("put context for galho '{name}'"))?
                .into_hash();
            let ref_name = RuntimeRef::context(name).to_string();
            // CAS against current value to make concurrent writers fail-loud.
            let current = backend.read_ref(&ref_name).await?;
            backend
                .cas_ref(&ref_name, current.as_ref(), &hash)
                .await
                .with_context(|| format!("cas ref for galho '{name}'"))?;
        }

        let locks = self.locks.read().await;
        for (root, lock) in locks.iter() {
            let bytes = serde_json::to_vec(lock)
                .with_context(|| format!("encode lock for root '{root}'"))?;
            let hash = backend
                .put_object(&bytes)
                .await
                .with_context(|| format!("put lock for root '{root}'"))?
                .into_hash();
            // Encode root in the ref path via typed RuntimeRef builder — sanitizes
            // '/' → '_' internally, no ad-hoc format!() needed.
            let ref_name = RuntimeRef::lock(root).to_string();
            let current = backend.read_ref(&ref_name).await?;
            backend
                .cas_ref(&ref_name, current.as_ref(), &hash)
                .await
                .with_context(|| format!("cas ref for lock root '{root}'"))?;
        }
        Ok(())
    }

    /// Reconstitute a Runtime from a previously-checkpointed `ObjectStore`. The
    /// emitter is supplied at restore time so consumers can attach an audit chain,
    /// memory collector, or noop emitter independently of the persisted state.
    pub async fn restore_from(
        backend: RuntimeBackend,
        emitter: Arc<dyn OutcomeEmitter>,
    ) -> Result<Self> {
        let store: &dyn galho_storage::ObjectStore = match &backend {
            RuntimeBackend::Memory(s) => s.as_ref(),
            RuntimeBackend::LocalFs(s) => s.as_ref(),
        };

        let mut contexts: BTreeMap<String, galho_types::MorphismContext> = BTreeMap::new();
        let mut locks: BTreeMap<String, galho_types::StackLock> = BTreeMap::new();

        for ref_path in store.list_refs().await? {
            if let Some(name) = ref_path.strip_prefix(RuntimeRef::context_prefix()) {
                let Some(hash) = store.read_ref(&ref_path).await? else {
                    continue;
                };
                let Some(bytes) = store.get_object(&hash).await? else {
                    continue;
                };
                let ctx: galho_types::MorphismContext = serde_json::from_slice(&bytes)
                    .with_context(|| format!("decode context for galho '{name}'"))?;
                contexts.insert(name.to_string(), ctx);
                continue;
            }
            if let Some(root_sanitized) = ref_path.strip_prefix(RuntimeRef::lock_prefix()) {
                let Some(hash) = store.read_ref(&ref_path).await? else {
                    continue;
                };
                let Some(bytes) = store.get_object(&hash).await? else {
                    continue;
                };
                let lock: galho_types::StackLock = serde_json::from_slice(&bytes)
                    .with_context(|| format!("decode lock for root '{root_sanitized}'"))?;
                // The lock carries its own stack_root field — use that as the in-memory key.
                locks.insert(lock.stack_root().as_str().to_string(), lock);
            }
        }

        Ok(Self {
            backend,
            kb: KnowledgeBase::default(),
            contexts: RwLock::new(contexts),
            locks: RwLock::new(locks),
            emitter,
        })
    }
}

fn summarize_sync(s: &SyncConfig) -> SyncSummary<'_> {
    SyncSummary(s)
}

/// Typed ref-name builder for Runtime persistence. The persistence layer uses
/// two ref namespaces: `runtime/contexts/<galho>` for per-galho MorphismContext
/// state, `runtime/locks/<root>` for per-stack-root locks. Adding a third
/// namespace = adding a constructor here, NOT another ad-hoc `format!()`.
///
/// The `Display` impl is the typed render surface (★★ TYPED EMISSION surface
/// 1); call sites consume it via `to_string()` or `{}` formatting against the
/// ObjectStore's `&str` API.
///
/// Lock roots may contain `/` (the convention for SHA-prefixed branch refs);
/// `Self::lock` sanitizes those into `_` to stay inside the ref-path alphabet.
pub enum RuntimeRef<'a> {
    Context(&'a str),
    Lock(&'a str),
}

impl<'a> RuntimeRef<'a> {
    #[must_use]
    pub fn context(name: &'a str) -> Self {
        Self::Context(name)
    }

    #[must_use]
    pub fn lock(root: &'a str) -> Self {
        Self::Lock(root)
    }

    /// Stripped name space prefix for parsing — symmetric with the `Display`
    /// renderer; consumers use this to recognize a ref-path on restore.
    #[must_use]
    pub fn context_prefix() -> &'static str {
        "runtime/contexts/"
    }

    #[must_use]
    pub fn lock_prefix() -> &'static str {
        "runtime/locks/"
    }
}

impl std::fmt::Display for RuntimeRef<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Context(name) => write!(f, "{}{name}", Self::context_prefix()),
            Self::Lock(root) => {
                // Sanitize '/' → '_' so the ref stays inside the path alphabet.
                f.write_str(Self::lock_prefix())?;
                for ch in root.chars() {
                    if ch == '/' {
                        f.write_str("_")?;
                    } else {
                        write!(f, "{ch}")?;
                    }
                }
                Ok(())
            }
        }
    }
}

/// Typed Display newtype for `SyncConfig` summaries. Replaces the previous
/// `format!()`-composed `summarize_sync(&SyncConfig) -> String` with a typed
/// AST renderer (★★ TYPED EMISSION surface 1).
pub struct SyncSummary<'a>(&'a SyncConfig);

impl std::fmt::Display for SyncSummary<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0.kind {
            SyncKind::Automatic => write!(f, "automatic"),
            SyncKind::OperatorApproval { roles, quorum } => write!(
                f,
                "operator-approval {quorum}-of-{} ({})",
                roles.len(),
                roles.join(","),
            ),
            SyncKind::ExternalSignal { source } => write!(f, "external-signal {source:?}"),
            SyncKind::TimeBased { soak } => write!(f, "time-based soak={}s", soak.whole_seconds()),
            SyncKind::AttestationGated { regime, .. } => write!(f, "attestation-gated {regime:?}"),
        }
    }
}

// ----- Knowledge queries (CLI-callable; no Runtime needed) -----

#[derive(Debug, Clone)]
pub enum KnowledgeQuery {
    Phases,
    Transitions,
    From(String),
    Sync(String),
    Reachable { start: String, target: String },
}

#[derive(Debug, Clone, Copy)]
pub enum OutputFormat {
    Text,
    Json,
}

pub fn run_knowledge_query(q: KnowledgeQuery, fmt: OutputFormat) -> Result<()> {
    let kb = KnowledgeBase::default();
    match q {
        KnowledgeQuery::Phases => {
            let phases: Vec<&'static str> = Phase::all().iter().map(|p| p.as_str()).collect();
            emit(fmt, &phases)?;
        }
        KnowledgeQuery::Transitions => {
            #[derive(Serialize)]
            struct Row {
                from: String,
                to: String,
                morphism: String,
            }
            let rows: Vec<Row> = transition_table()
                .iter()
                .map(|t| Row {
                    from: t.from.as_str().into(),
                    to: t.to.as_str().into(),
                    morphism: t.morphism.as_str().into(),
                })
                .collect();
            emit(fmt, &rows)?;
        }
        KnowledgeQuery::From(phase) => {
            let p = parse_phase(&phase)?;
            let forward = kb.forward_morphisms_from(p);
            let backward = kb.backward_morphisms_from(p);
            #[derive(Serialize)]
            struct Available {
                phase: String,
                forward: Vec<String>,
                backward: Vec<String>,
            }
            emit(
                fmt,
                &Available {
                    phase: p.as_str().into(),
                    forward: forward.iter().map(|m| m.as_str().into()).collect(),
                    backward: backward.iter().map(|m| m.as_str().into()).collect(),
                },
            )?;
        }
        KnowledgeQuery::Sync(phase) => {
            let p = parse_phase(&phase)?;
            let s = kb.sync_for(p).context("phase has no sync config")?;
            emit(fmt, s)?;
        }
        KnowledgeQuery::Reachable { start, target } => {
            let s = parse_phase(&start)?;
            let t = parse_phase(&target)?;
            #[derive(Serialize)]
            struct Reach {
                start: String,
                target: String,
                reachable: bool,
            }
            emit(
                fmt,
                &Reach {
                    start: s.as_str().into(),
                    target: t.as_str().into(),
                    reachable: kb.is_reachable(s, t),
                },
            )?;
        }
    }
    Ok(())
}

fn parse_phase(s: &str) -> Result<Phase> {
    Phase::all()
        .iter()
        .copied()
        .find(|p| p.as_str() == s)
        .ok_or_else(|| anyhow!("unknown phase: '{s}'"))
}

fn emit<T: Serialize>(fmt: OutputFormat, v: &T) -> Result<()> {
    match fmt {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(v)?),
        OutputFormat::Text => println!("{}", serde_yaml_ng::to_string(v)?),
    }
    Ok(())
}
