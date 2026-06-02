//! Knowledge graph mapping Phase → Morphisms → Syncs.
//!
//! "Full knowledge mapping" per the operator's directive: every relationship
//! between Phase, Morphism, and Sync is a typed, queryable, static graph. The
//! operator can ask "what morphisms are available from Verified?" and the
//! controller can ask "what syncs gate the Promote morphism?" — both are
//! `O(1)` lookups against the static knowledge tables.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::morphism::{morphism_for, MorphismRequirement, PhaseMorphism};
use crate::phase::{is_known_transition, transition_table, MorphismId, Phase, PhaseTransition};
use crate::sync::SyncConfig;

/// A static, queryable knowledge base over Phase / Morphism / Sync relations.
/// Built lazily; methods are pure functions over the static `phase::transition_table`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeBase {
    /// Per-phase Sync configurations. Defaults via `SyncConfig::default_for(phase)`;
    /// operator overrides land via shikumi-tiered config in consumers.
    pub syncs: BTreeMap<Phase, SyncConfig>,
}

impl Default for KnowledgeBase {
    fn default() -> Self {
        let mut syncs = BTreeMap::new();
        for &p in Phase::all() {
            syncs.insert(p, SyncConfig::default_for(p));
        }
        Self { syncs }
    }
}

impl KnowledgeBase {
    /// Constructor matching default impl. Useful when the operator wants to mutate
    /// before storing.
    #[must_use]
    pub fn prescribed() -> Self {
        Self::default()
    }

    /// Available transitions from `phase`. Static lookup.
    #[must_use]
    pub fn transitions_from(&self, phase: Phase) -> Vec<PhaseTransition> {
        crate::phase::transitions_from(phase)
    }

    /// Forward morphism ids (ids whose target phase advances the forward arc).
    /// "Forward" = the target phase has `PhaseClass::Forward` OR the target is `Done`.
    #[must_use]
    pub fn forward_morphisms_from(&self, phase: Phase) -> Vec<MorphismId> {
        self.transitions_from(phase)
            .into_iter()
            .filter(|t| {
                matches!(
                    t.to.class(),
                    crate::phase::PhaseClass::Forward | crate::phase::PhaseClass::Terminal
                ) && t.to != Phase::Destroyed
            })
            .map(|t| t.morphism)
            .collect()
    }

    /// Backward / rollback morphism ids from `phase`.
    #[must_use]
    pub fn backward_morphisms_from(&self, phase: Phase) -> Vec<MorphismId> {
        self.transitions_from(phase)
            .into_iter()
            .filter(|t| {
                matches!(
                    t.to.class(),
                    crate::phase::PhaseClass::Backward | crate::phase::PhaseClass::Failure
                ) || t.to == Phase::Destroyed
            })
            .map(|t| t.morphism)
            .collect()
    }

    /// Sync config for a given phase. Mutation of the returned reference is the
    /// caller's mechanism for operator overrides.
    #[must_use]
    pub fn sync_for(&self, phase: Phase) -> Option<&SyncConfig> {
        self.syncs.get(&phase)
    }

    /// Set a custom sync config for a phase. Used by `shikumi`-tiered config loading
    /// in consumers.
    pub fn set_sync(&mut self, phase: Phase, config: SyncConfig) {
        self.syncs.insert(phase, config);
    }

    /// Apply a typed forward morphism by id. Returns `Ok(next_phase)` on success,
    /// or `Err(Vec<MorphismRequirement>)` if preconditions aren't satisfied.
    ///
    /// The destination phase is looked up from the **transition table** (not the
    /// morphism's `to_phase()`), since morphisms like `RevertApply` / `Escalate` /
    /// `Abandon` have multiple destinations depending on the current phase.
    pub fn apply_morphism(
        &self,
        morphism: MorphismId,
        ctx: &crate::morphism::MorphismContext,
    ) -> Result<Phase, Vec<MorphismRequirement>> {
        let m: Box<dyn PhaseMorphism> = morphism_for(morphism).expect("known morphism");
        let missing = m.check_preconditions(ctx);
        if !missing.is_empty() {
            return Err(missing);
        }
        // Find the (from=current, morphism=this) transition in the static table.
        // A missing row is a typed hard error — never silently substitute the
        // morphism's nominal `to_phase()` (which masks wrong-target bugs for
        // multi-destination morphisms like Abandon / RevertApply / Escalate).
        match transition_table()
            .iter()
            .find(|t| t.from == ctx.current_phase && t.morphism == morphism)
            .map(|t| t.to)
        {
            Some(target) => Ok(target),
            None => Err(vec![MorphismRequirement::MissingTransitionRow {
                from: ctx.current_phase,
                morphism,
            }]),
        }
    }

    /// Validate that the whole knowledge graph is well-formed:
    /// - Every phase has at least one outgoing transition (except terminal Phases).
    /// - Every transition's `MorphismId` is materializable via `morphism_for`.
    /// - The transition table contains no duplicate `(from, to)` pairs with different morphisms.
    /// - Every phase has a registered sync config.
    pub fn validate(&self) -> Result<(), Vec<KnowledgeBaseError>> {
        let mut errors = Vec::new();

        // 1. Every non-terminal phase has at least one outgoing transition.
        for &phase in Phase::all() {
            if matches!(
                phase.class(),
                crate::phase::PhaseClass::Terminal
            ) {
                continue;
            }
            if self.transitions_from(phase).is_empty() {
                errors.push(KnowledgeBaseError::PhaseWithoutOutgoing(phase));
            }
        }

        // 2. Every morphism id in the table is materializable.
        for t in transition_table() {
            if morphism_for(t.morphism).is_none() {
                errors.push(KnowledgeBaseError::UnknownMorphism {
                    transition: *t,
                });
            }
        }

        // 3. No duplicate (from, to) pairs.
        let mut seen: BTreeMap<(Phase, Phase), MorphismId> = BTreeMap::new();
        for t in transition_table() {
            if let Some(prev) = seen.insert((t.from, t.to), t.morphism) {
                if prev != t.morphism {
                    errors.push(KnowledgeBaseError::DuplicateTransition {
                        from: t.from,
                        to: t.to,
                        existing: prev,
                        new: t.morphism,
                    });
                }
            }
        }

        // 4. Every phase has a registered Sync config.
        for &phase in Phase::all() {
            if !self.syncs.contains_key(&phase) {
                errors.push(KnowledgeBaseError::PhaseWithoutSync(phase));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Is `target` reachable from `start` via a sequence of known transitions?
    /// BFS-based. Used by operator UX ("can I advance to Done from here?").
    #[must_use]
    pub fn is_reachable(&self, start: Phase, target: Phase) -> bool {
        if start == target {
            return true;
        }
        let mut visited: std::collections::BTreeSet<Phase> = std::collections::BTreeSet::new();
        let mut queue: std::collections::VecDeque<Phase> = std::collections::VecDeque::new();
        queue.push_back(start);
        while let Some(p) = queue.pop_front() {
            if !visited.insert(p) {
                continue;
            }
            if p == target {
                return true;
            }
            for t in self.transitions_from(p) {
                if !visited.contains(&t.to) {
                    queue.push_back(t.to);
                }
            }
        }
        false
    }
}

/// Errors a knowledge base can surface during validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum KnowledgeBaseError {
    PhaseWithoutOutgoing(Phase),
    UnknownMorphism { transition: PhaseTransition },
    DuplicateTransition {
        from: Phase,
        to: Phase,
        existing: MorphismId,
        new: MorphismId,
    },
    PhaseWithoutSync(Phase),
}

/// Validate that `is_known_transition` agrees with `transitions_from` — sanity check
/// catching transitions added to the table but not exposed via the query helpers.
#[must_use]
pub fn check_query_consistency() -> Result<(), String> {
    for t in transition_table() {
        if !is_known_transition(t.from, t.to) {
            return Err(format!("transition table contains {t:?} but is_known_transition returns false"));
        }
        let from_list = crate::phase::transitions_from(t.from);
        if !from_list.iter().any(|x| x.to == t.to) {
            return Err(format!("transitions_from({}) doesn't include target {}", t.from, t.to));
        }
    }
    Ok(())
}
