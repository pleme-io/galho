//! §XIX golden-attractor POLICY forcing-function test.
//!
//! The capstone over comfort + flow_tier: proves the attractor policy actually
//! *favors golden paths* and *always converges* —
//!
//! 1. **terminals recommend nothing** — `Done`/`Destroyed` are absorbing;
//! 2. **strict progress + termination** — every recommended move lands exactly
//!    one step closer to the goal, so iterating `recommend` reaches the goal in
//!    `distance_to` steps (≤ μ=7) with no loop;
//! 3. **golden preference** — when a golden move makes progress, the policy
//!    takes it; it crosses a ceiling only when no golden progress move exists
//!    (the unavoidable `#2` apply on the path to `Done`);
//! 4. **clean abandon is golden from the pre-apply berths** — abandoning a
//!    `Declared`/`Planned` change never touches cloud.

use galho_types::attractor::{Goal, distance_to, recommend};
use galho_types::phase::Phase;

const GOALS: [Goal; 2] = [Goal::Accept, Goal::Abandon];

/// Absorbing terminals recommend no move, for either goal.
#[test]
fn terminals_recommend_nothing() {
    for g in GOALS {
        assert_eq!(recommend(Phase::Done, g), None, "Done is absorbing");
        assert_eq!(recommend(Phase::Destroyed, g), None, "Destroyed is absorbing");
    }
}

/// Every recommended move strictly decreases distance-to-goal by exactly one, so
/// the policy lies on a shortest path and cannot stall or loop.
#[test]
fn recommend_makes_strict_progress() {
    for g in GOALS {
        let target = g.terminal();
        for p in Phase::all() {
            match distance_to(*p, target) {
                None | Some(0) => assert_eq!(
                    recommend(*p, g),
                    None,
                    "{p:?} at/unable-to-reach {target:?} must recommend nothing"
                ),
                Some(d) => {
                    let mv = recommend(*p, g)
                        .unwrap_or_else(|| panic!("{p:?} (dist {d} to {target:?}) recommends nothing"));
                    let d_next = distance_to(mv.to, target)
                        .unwrap_or_else(|| panic!("{p:?} recommended a dead-end {:?}", mv.to));
                    assert_eq!(d_next + 1, d, "{p:?} --{:?}--> {:?} is not a strict step toward {target:?}", mv.morphism, mv.to);
                }
            }
        }
    }
}

/// Iterating `recommend` from any phase reaches the goal terminal in exactly
/// `distance_to` steps — the attractor terminates (≤ μ=7).
#[test]
fn attractor_converges_to_the_goal() {
    for g in GOALS {
        let target = g.terminal();
        for p in Phase::all() {
            let Some(d) = distance_to(*p, target) else { continue };
            let mut cur = *p;
            let mut steps = 0usize;
            while cur != target {
                let mv = recommend(cur, g).expect("non-goal reachable phase recommends a move");
                cur = mv.to;
                steps += 1;
                assert!(steps <= 7, "{p:?} → {target:?} exceeded μ=7 (loop?)");
            }
            assert_eq!(steps, d, "{p:?} → {target:?} took {steps}, expected dist {d}");
        }
    }
}

/// Golden preference, both directions:
/// * Accept from `Declared` starts golden (`Plan`); the only non-golden step on
///   the accept path is the unavoidable `#2 ApplyToPreview` apply at `Planned`.
/// * Abandon from the pre-apply berths is always golden (metadata-only reap).
#[test]
fn policy_favors_golden_and_crosses_only_when_forced() {
    // Accept: Declared → Planned is golden (Plan).
    let m = recommend(Phase::Declared, Goal::Accept).expect("Declared can reach Done");
    assert!(m.is_golden(), "Accept from Declared should start on golden rails (Plan), got {m:?}");

    // Accept: from Planned the ONLY progress move toward Done is the #2 apply
    // crossing — there is no golden progress move, so the policy crosses.
    let m = recommend(Phase::Planned, Goal::Accept).expect("Planned can reach Done");
    assert!(!m.is_golden(), "Accept from Planned must cross the #2 apply ceiling, got {m:?}");

    // Abandon from the pre-apply berths is golden (nothing applied → metadata reap).
    for p in [Phase::Declared, Phase::Planned] {
        let m = recommend(p, Goal::Abandon).unwrap_or_else(|| panic!("{p:?} can reach Destroyed"));
        assert!(m.is_golden(), "Abandon from {p:?} should be golden (metadata-only), got {m:?}");
        assert_eq!(m.to, Phase::Destroyed, "Abandon from {p:?} goes straight to Destroyed");
    }
}

/// Recovery berths return on golden rails: Failed→Recover and RolledBack→Resume
/// are the policy's first move toward Accept, and both are golden.
#[test]
fn recovery_berths_rejoin_accept_on_golden_rails() {
    for p in [Phase::Failed, Phase::RolledBack] {
        let m = recommend(p, Goal::Accept).unwrap_or_else(|| panic!("{p:?} can reach Done"));
        assert!(m.is_golden(), "{p:?} should rejoin the accept path on golden rails, got {m:?}");
    }
}
