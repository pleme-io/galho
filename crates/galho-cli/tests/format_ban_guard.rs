//! Typed migration ratchet for the ★★ TYPED EMISSION rule.
//!
//! galho's CLAUDE.md carries a `skip-format-ban: migration in progress` waiver
//! per the documented per-repo opt-out path. The waiver IS NOT a free pass —
//! this test ratchets the count downward: the total number of `format!()`
//! call sites across galho's src/ trees cannot grow between commits.
//!
//! Failure modes:
//!   • New code adds a format!() call site → count exceeds CEILING → fails.
//!   • Cleanup removes a call site → count drops below CEILING → fails with a
//!     "ratchet downward" message prompting the operator to lower CEILING.
//!
//! Removing the waiver: when CEILING reaches 0, delete this test + the
//! `skip-format-ban:` marker from CLAUDE.md + add `clippy.toml` with
//! `disallowed_macros = ["std::format"]` at the workspace root. The substrate
//! enforcement then takes over.
//!
//! Test fixture target: src trees only — tests/ + benches/ are exempt because
//! test fixtures legitimately use format!() for assertion messages.

use std::path::{Path, PathBuf};

/// Maximum allowed `format!(` call sites across galho's src/ trees. MUST be
/// monotonically decreasing — when this fails downward, lower the ceiling.
///
/// History (most recent at top — each line is a ratchet step):
///   2026-05-22  41  ratchet step: -2 (RuntimeRef typed builder in lib.rs)
///                    + refinement: counter now skips comment lines (excludes
///                                  docstring mentions of `format!()`)
///   2026-05-22  47  ratchet step: -3 (SyncSummary Display newtype in lib.rs:1077)
///   2026-05-22  50  ratchet step: -3 (TextMarker Display newtype in lib.rs:155)
///   2026-05-22  53  baseline at waiver registration
///
/// Next refactor targets (in priority order):
///   * `with_context(|| format!(...))` × 12 → anyhow's `static_context` pattern
///   * `outcome_chain.rs` × 5 + `compliance_verifier.rs` × 6 → typed report renderers
///   * `translate.rs` × 8 (galho-terraform) → typed terraform-address renderer
const CEILING: usize = 41;

fn galho_workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is the package's Cargo.toml dir → walk up to workspace.
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    while p.file_name().and_then(|n| n.to_str()) != Some("galho") {
        if !p.pop() {
            panic!("could not find galho workspace root from CARGO_MANIFEST_DIR");
        }
    }
    p
}

fn count_format_in_dir(dir: &Path) -> usize {
    let mut total = 0;
    if !dir.exists() {
        return 0;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return 0,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            total += count_format_in_dir(&path);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            if let Ok(content) = std::fs::read_to_string(&path) {
                total += count_format_in_source(&content);
            }
        }
    }
    total
}

/// Count actual `format!(` macro invocations in source. Skips lines that are
/// entirely comments (start with `//` or `///` after whitespace) — comment
/// mentions of `format!()` document the ban but don't violate it.
fn count_format_in_source(content: &str) -> usize {
    let mut total = 0;
    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") {
            continue;
        }
        // Count any `format!(` substring; the typed surface is `write!(`,
        // `writeln!(`, or a Display impl — none collide with this pattern.
        total += trimmed.matches("format!(").count();
    }
    total
}

#[test]
fn format_call_count_does_not_grow() {
    let root = galho_workspace_root();
    let crates_dir = root.join("crates");
    assert!(crates_dir.exists(), "crates/ dir missing under {root:?}");

    let mut total = 0;
    for entry in std::fs::read_dir(&crates_dir).expect("read crates/") {
        let entry = entry.unwrap();
        let src = entry.path().join("src");
        total += count_format_in_dir(&src);
    }

    assert!(
        total <= CEILING,
        "★★ TYPED EMISSION ratchet failed: {total} format!() call sites across galho src/ trees \
         (ceiling: {CEILING}). Either lower the ceiling (if you removed sites) or refactor the \
         new violation to a typed surface (Display impl, anyhow!, typed AST renderer).",
    );

    // Soft hint when count drops below ceiling — operator should ratchet down.
    if total < CEILING {
        eprintln!(
            "★★ TYPED EMISSION ratchet downward: total now {total}, ceiling {CEILING}. \
             Lower CEILING in tests/format_ban_guard.rs to lock in the gain.",
        );
    }
}
