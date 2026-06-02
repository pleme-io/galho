//! Tests for the additive `Galho<P>` phantom-typestate facade (forward arc v1).
//!
//! The forward-arc methods exist only on the source phase's `Galho<From>`, so
//! the type system rejects firing a morphism from the wrong phase at compile
//! time. (The "can't call `.plan()` twice" property is a compile-error, asserted
//! by construction — it cannot be a runtime test.)
//!
//! Tests live under tests/ → exempt from the format!() ratchet.

use galho_types::typestate::{Galho, Verified};
use galho_types::{KnowledgeBase, MorphismContext, Phase};

#[test]
fn declared_plans_into_planned() {
    let kb = KnowledgeBase::default();
    let g = Galho::declared("galho-typed");
    assert_eq!(g.ctx().current_phase(), Phase::Declared);
    let planned = g.plan(&kb).expect("Declared -> Planned");
    assert_eq!(planned.ctx().current_phase(), Phase::Planned);
}

#[test]
fn full_forward_arc_threads_through_typestates() {
    let kb = KnowledgeBase::default();
    // Declared -> Planned -> AppliedPendingReview -> ApprovedAwaitingMerge
    //          -> Merged -> Verified -> Done.
    let planned = Galho::declared("g").plan(&kb).unwrap();

    // `KnowledgeBase::advance` advances the phase but does NOT set the runtime
    // flags (that bookkeeping is the cli Runtime's job). Seed every forward-arc
    // precondition once on the erased ctx, then thread the typestates through.
    let mut ctx = planned.into_ctx();
    ctx.has_plan = true;
    ctx.stack_lock_held = true;
    ctx.has_apply_receipt = true;
    ctx.has_approval_quorum = true;
    ctx.has_merge_event = true;
    ctx.has_verify_receipt = true;
    ctx.jira_ticket_resolvable = true;
    let planned = Galho::<galho_types::typestate::Planned>::assume(ctx).expect("still Planned");

    let applied = planned.apply_to_preview(&kb).unwrap();
    assert_eq!(applied.ctx().current_phase(), Phase::AppliedPendingReview);
    let approved = applied.record_approval(&kb).unwrap();
    let merged = approved.promote(&kb).unwrap();
    let verified = merged.verify(&kb).unwrap();
    assert_eq!(verified.ctx().current_phase(), Phase::Verified);
    let done = verified.seal_done(&kb).unwrap();
    assert_eq!(done.ctx().current_phase(), Phase::Done);
}

#[test]
fn assume_rejects_wrong_phase() {
    // A Declared ctx cannot be re-tagged as Verified.
    let ctx = MorphismContext::declared("g");
    let res = Galho::<Verified>::assume(ctx);
    assert!(res.is_err(), "assume must reject a phase mismatch");
}

#[test]
fn forward_transition_returns_self_on_precondition_failure() {
    let kb = KnowledgeBase::default();
    // Planned without has_plan/stack_lock seeded → apply_to_preview fails (advance
    // does not set runtime flags), returns self + the missing requirements.
    let planned = Galho::declared("g").plan(&kb).unwrap();
    match planned.apply_to_preview(&kb) {
        Ok(_) => panic!("expected precondition failure without stack lock"),
        Err((back, missing)) => {
            assert_eq!(back.ctx().current_phase(), Phase::Planned, "self unchanged");
            assert!(!missing.is_empty());
        }
    }
}
