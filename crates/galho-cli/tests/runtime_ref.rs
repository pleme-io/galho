//! Typed `RuntimeRef` builder — third Display-newtype surface in galho-cli.
//!
//! Load-bearing invariants:
//!   1. context(name) renders as `runtime/contexts/<name>`.
//!   2. lock(root) renders as `runtime/locks/<root_with_slashes_sanitized>`.
//!   3. context_prefix() and lock_prefix() are the static strings used by
//!      restore_from to parse the inverse direction.
//!   4. Round-trip: render → strip_prefix(prefix) → original input (modulo
//!      slash-sanitization for lock roots).

use galho_cli::RuntimeRef;

#[test]
fn context_render_uses_canonical_prefix() {
    let r = RuntimeRef::context("feat-a").to_string();
    assert_eq!(r, "runtime/contexts/feat-a");
}

#[test]
fn lock_render_sanitizes_slashes() {
    let r = RuntimeRef::lock("refs/heads/feat-a").to_string();
    assert_eq!(r, "runtime/locks/refs_heads_feat-a");
}

#[test]
fn lock_render_handles_no_slashes() {
    let r = RuntimeRef::lock("plain-root").to_string();
    assert_eq!(r, "runtime/locks/plain-root");
}

#[test]
fn context_prefix_constant_matches_render() {
    let r = RuntimeRef::context("xyz").to_string();
    assert!(r.starts_with(RuntimeRef::context_prefix()));
    assert_eq!(
        r.strip_prefix(RuntimeRef::context_prefix()).unwrap(),
        "xyz",
    );
}

#[test]
fn lock_prefix_constant_matches_render() {
    let r = RuntimeRef::lock("root-x").to_string();
    assert!(r.starts_with(RuntimeRef::lock_prefix()));
    assert_eq!(
        r.strip_prefix(RuntimeRef::lock_prefix()).unwrap(),
        "root-x",
    );
}

#[test]
fn empty_name_renders_as_bare_prefix() {
    assert_eq!(
        RuntimeRef::context("").to_string(),
        "runtime/contexts/",
    );
    assert_eq!(
        RuntimeRef::lock("").to_string(),
        "runtime/locks/",
    );
}

#[test]
fn unicode_name_passes_through() {
    let r = RuntimeRef::context("feat-α").to_string();
    assert_eq!(r, "runtime/contexts/feat-α");
}
