//! Smoke test: drive the actual `galho` binary against a shared local-fs root and
//! prove state survives between invocations. Each `cargo run` is a separate process —
//! cross-session continuity is the load-bearing claim.

use std::process::Command;

fn binary() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_galho"));
    c.env("RUST_LOG", "warn"); // hush tracing for cleaner test output
    c
}

#[test]
fn three_invocations_share_state_through_local_fs_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_string_lossy().to_string();

    // Invocation 1: create a galho.
    let out = binary()
        .args(["--root", &root, "--galho", "feature/persistent", "new"])
        .output()
        .expect("galho new");
    assert!(
        out.status.success(),
        "new failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("declared"), "stdout: {stdout}");

    // Invocation 2: fresh process picks up the persisted state, advances to Planned.
    let out = binary()
        .args(["--root", &root, "--galho", "feature/persistent", "plan"])
        .output()
        .expect("galho plan");
    assert!(out.status.success(), "plan failed: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("planned"), "stdout: {stdout}");

    // Invocation 3: ANOTHER fresh process, checks status of the persisted galho.
    let out = binary()
        .args(["--root", &root, "--galho", "feature/persistent", "status"])
        .output()
        .expect("galho status");
    assert!(out.status.success(), "status failed: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("planned"), "status stdout: {stdout}");
}

#[test]
fn confirm_subcommand_records_approval_against_typed_quorum() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_string_lossy().to_string();

    binary()
        .args(["--root", &root, "--galho", "g", "new"])
        .output()
        .unwrap();
    binary()
        .args(["--root", &root, "--galho", "g", "plan"])
        .output()
        .unwrap();
    binary()
        .args([
            "--root",
            &root,
            "--galho",
            "g",
            "apply",
            "--stack-root",
            "root-abc",
        ])
        .output()
        .unwrap();

    // Now at AppliedPendingReview. Pre-quorum confirm should announce "quorum reached"
    // since the default sync at this phase is { roles: [reviewer, security-officer], quorum: 1 }.
    let out = binary()
        .args([
            "--root", &root, "--galho", "g", "confirm", "--role", "reviewer",
        ])
        .output()
        .expect("confirm");
    assert!(out.status.success(), "confirm failed: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("quorum reached"), "stdout: {stdout}");

    // Now `approve` advances since quorum is met.
    let out = binary()
        .args([
            "--root", &root, "--galho", "g", "approve", "--role", "reviewer",
        ])
        .output()
        .expect("approve");
    assert!(out.status.success(), "approve failed: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("approved_awaiting_merge"), "stdout: {stdout}");
}

#[test]
fn knowledge_subcommand_lists_phases_without_runtime() {
    let out = binary()
        .args(["knowledge", "phases"])
        .output()
        .expect("knowledge phases");
    assert!(out.status.success(), "knowledge phases failed: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("declared"));
    assert!(stdout.contains("planned"));
    assert!(stdout.contains("done"));
}

#[test]
fn checkpoint_subcommand_succeeds() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_string_lossy().to_string();
    binary()
        .args(["--root", &root, "--galho", "g", "new"])
        .output()
        .unwrap();
    let out = binary()
        .args(["--root", &root, "checkpoint"])
        .output()
        .expect("checkpoint");
    assert!(out.status.success(), "checkpoint failed: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("checkpoint OK"));
}
