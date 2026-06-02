//! Deploy-order gate tests — `galho gate` is the IaC-deploy sibling of
//! `carve gate` (PR merge-order). The gate blocks a galho's deploy when an
//! upstream dependency has not reached `Verified`/`Done`, and — under
//! `--require-siblings-ready` — when a sibling galho is still `Declared`.
//!
//! Tests live under tests/ → exempt from the format!() ratchet.

use galho_cli::{GateReport, Runtime};
use galho_types::{MorphismId, Phase};

/// Drive a galho all the way to `Verified` via the typed forward arc.
async fn drive_to_verified(rt: &Runtime, name: &str, role: &str) {
    rt.fire_morphism(name, MorphismId::Plan, None).await.unwrap();
    rt.fire_morphism(name, MorphismId::ApplyToPreview, Some("stack".into()))
        .await
        .unwrap();
    rt.confirm_approval(name, role).await.unwrap();
    rt.fire_morphism(name, MorphismId::RecordApproval, Some(role.into()))
        .await
        .unwrap();
    rt.fire_morphism(name, MorphismId::Promote, None).await.unwrap();
    rt.fire_morphism(name, MorphismId::Verify, None).await.unwrap();
}

/// Recompute a galho's gate report from the runtime snapshot, mirroring the
/// CLI's `run_gate` (upstream deps that are not Verified/Done are unmet).
async fn gate_for(rt: &Runtime, name: &str) -> GateReport {
    let snaps = rt.list_galhos_with_state().await;
    let me = snaps.iter().find(|g| g.name == name).expect("galho exists");
    GateReport {
        galho: name.to_string(),
        phase: me.phase,
        unmet_deps: me.unmet_deps().into_iter().map(str::to_string).collect(),
        unready_siblings: Vec::new(),
    }
}

#[tokio::test]
async fn gate_refuses_when_upstream_unready() {
    let rt = Runtime::with_memory();
    // base left in Declared; feat depends on base.
    rt.new_galho("base").await.unwrap();
    rt.new_galho_with_deps("feat", ["base".to_string()])
        .await
        .unwrap();

    let report = gate_for(&rt, "feat").await;
    assert!(
        report.is_blocked(),
        "feat must be blocked while base is unready: {report}"
    );
    assert_eq!(report.unmet_deps, vec!["base".to_string()]);
}

#[tokio::test]
async fn gate_passes_when_upstream_ready() {
    let rt = Runtime::with_memory();
    rt.new_galho("base").await.unwrap();
    rt.new_galho_with_deps("feat", ["base".to_string()])
        .await
        .unwrap();

    drive_to_verified(&rt, "base", "reviewer").await;

    // Sanity: base is now Verified.
    assert_eq!(rt.status("base").await.unwrap().phase.0, Phase::Verified);

    let report = gate_for(&rt, "feat").await;
    assert!(
        !report.is_blocked(),
        "feat must pass once base is Verified: {report}"
    );
    assert!(report.unmet_deps.is_empty());
}

#[test]
fn gate_binary_exits_nonzero_when_blocked() {
    use std::process::Command;
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_string_lossy().to_string();

    fn bin() -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_galho"));
        c.env("RUST_LOG", "warn");
        c
    }

    bin().args(["--root", &root, "--galho", "base", "new"]).output().unwrap();
    bin()
        .args(["--root", &root, "--galho", "feat", "new", "--depends-on", "base"])
        .output()
        .unwrap();

    let out = bin()
        .args(["--root", &root, "--galho", "feat", "gate"])
        .output()
        .expect("gate");
    assert!(
        !out.status.success(),
        "gate must exit non-zero while base is unready; stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn gate_blocks_unready_sibling_under_flag() {
    // Two galhos both depending on `base`; `sib` left Declared. With
    // --require-siblings-ready, gating `feat` reports `sib` as unready even
    // though feat's own dep is met.
    let report = GateReport {
        galho: "feat".into(),
        phase: Phase::Verified,
        unmet_deps: Vec::new(),
        unready_siblings: vec!["sib".into()],
    };
    assert!(report.is_blocked());
    let rendered = report.to_string();
    assert!(rendered.contains("unready siblings"), "{rendered}");
}
