//! §XIX golden-attractor POLICY — "from here, what is the best next move?"
//!
//! This is the decision the §XIX.9 attractor makes, expressed as a pure function
//! standing on the two abstractions already proven:
//!
//! * [`crate::flow_tier`] — which edges are golden (touch no cloud) vs cross a
//!   ceiling, and the distances.
//! * [`crate::comfort`] — how restable each destination berth is.
//!
//! The policy drives every change to a good terminal, **favoring golden edges**,
//! crossing a ceiling only when no golden move makes progress, and preferring
//! the more-comfortable destination on ties. It is pure + total + tested; the
//! M5 controller *calls* it (it does not re-implement the choice). That keeps
//! "the state machine favors traveling golden paths" a property of one tested
//! function, not scattered controller branches.

use crate::comfort::comfort_of;
use crate::flow_tier::{EdgeTier, edge_tier};
use crate::phase::{MorphismId, Phase, transition_table};
use std::collections::{BTreeSet, VecDeque};

/// Which good terminal the attractor is steering toward. The two — and only two
/// — absorbing outcomes (§VII.1): a change is either accepted or cleanly abandoned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Goal {
    /// Drive the change to `Done` (ACCEPTED).
    Accept,
    /// Drive the change to `Destroyed` (CLEANLY-ABANDONED).
    Abandon,
}

impl Goal {
    /// The good terminal this goal targets.
    #[must_use]
    pub const fn terminal(self) -> Phase {
        match self {
            Self::Accept => Phase::Done,
            Self::Abandon => Phase::Destroyed,
        }
    }
}

/// Min edges from `from` to `goal` over the full transition table. `Some(0)` iff
/// `from == goal`; `None` if `goal` is unreachable from `from`.
#[must_use]
pub fn distance_to(from: Phase, goal: Phase) -> Option<usize> {
    if from == goal {
        return Some(0);
    }
    let mut seen = BTreeSet::from([from]);
    let mut q = VecDeque::from([(from, 0usize)]);
    while let Some((cur, d)) = q.pop_front() {
        for t in transition_table().iter().filter(|t| t.from == cur) {
            if t.to == goal {
                return Some(d + 1);
            }
            if seen.insert(t.to) {
                q.push_back((t.to, d + 1));
            }
        }
    }
    None
}

/// A recommended attractor move toward a goal terminal: the morphism to apply,
/// the berth it lands in, and that edge's tier (so the caller knows whether a
/// §XIX.5 `CeilingCrossing` witness is required).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttractorMove {
    pub morphism: MorphismId,
    pub to: Phase,
    pub tier: EdgeTier,
}

impl AttractorMove {
    /// Does taking this move stay on golden rails (no cloud touch, no witness)?
    #[must_use]
    pub const fn is_golden(self) -> bool {
        self.tier.is_golden()
    }
}

/// The golden-attractor policy: from `from`, the single best move toward `goal`.
///
/// Among the moves that lie on a SHORTEST path to the goal (guaranteeing strict
/// progress + termination), prefer in order:
///   1. **GoldenPreserving** over `CeilingCrossing` — favor golden rails;
///   2. the **more-comfortable** destination berth (lower [`crate::comfort`]
///      worst-tier severity).
///
/// Returns `None` at the goal, at a terminal, or when the goal is unreachable.
/// Because every returned move strictly decreases `distance_to(_, goal)`,
/// iterating `recommend` reaches the goal in exactly `distance_to(from, goal)`
/// steps — the attractor always terminates.
#[must_use]
pub fn recommend(from: Phase, goal: Goal) -> Option<AttractorMove> {
    let target = goal.terminal();
    let d_from = distance_to(from, target)?;
    if d_from == 0 {
        return None;
    }
    transition_table()
        .iter()
        .filter(|t| t.from == from)
        .filter_map(|t| {
            // On a shortest path iff the destination is one step closer.
            let d_to = distance_to(t.to, target)?;
            if d_to + 1 != d_from {
                return None;
            }
            let tier = edge_tier(t.from, t.morphism).expect("a shipped edge has an EdgeTier");
            let comfort_severity = comfort_of(t.to).map_or(u8::MAX, |r| r.worst_tier().severity());
            // Sort key (smaller = preferred): golden first, then most comfortable.
            let key = (!tier.is_golden(), comfort_severity);
            Some((AttractorMove { morphism: t.morphism, to: t.to, tier }, key))
        })
        .min_by_key(|(_, key)| *key)
        .map(|(mv, _)| mv)
}
