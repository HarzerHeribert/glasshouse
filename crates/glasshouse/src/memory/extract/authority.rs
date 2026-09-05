//! How binding an extracted memory is allowed to become.
//!
//! Phase 21A's policy has three parts: the seven classes already exist in
//! the store ([`MemoryAuthority`]); classification only ever lowers
//! ([`conservative`]); and minting an invariant is a human act
//! ([`super::super::store::Classifier`] and `MemoryStore::set_authority`).
//!
//! An extractor may never mint an invariant, even from a certain report: the
//! only certainty it has access to is the model's own claim of it, and Phase
//! 21K treats confidence, repetition and eloquence as "presentation
//! characteristics rather than evidence." So [`EXTRACTOR_CEILING`] is
//! `Constraint` — still binding, but a class Phase 22's conflict machinery
//! may review. Reaching `Invariant` takes
//! [`super::super::store::Classifier::Reviewed`].
//!
//! [`conservative`] moves a memory toward `historical`, never toward
//! `invariant`: a memory stored weaker than it deserves is merely
//! under-surfaced context, but one stored stronger than it deserves directs
//! work unasked, and nobody notices because it looks deliberate.
// History: design-decisions.md, "Trims: memory export and extraction module docs", memory/extract/authority.rs module doc.

use super::super::store::MemoryAuthority;
use super::schema::{Confidence, Disposition};

/// The strongest class automatic extraction may assign.
///
/// See the module documentation: not a tuning parameter, a consequence of
/// Phase 21K's rule that model confidence is not evidence.
pub const EXTRACTOR_CEILING: MemoryAuthority = MemoryAuthority::Constraint;

/// Why a declared authority was lowered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lowering {
    /// Automatic extraction cannot produce an invariant.
    AutomaticExtraction,
    /// The model said it was not certain.
    StatedConfidence(Confidence),
    /// The thing remembered was not accepted, or was abandoned.
    StatedDisposition(Disposition),
}

impl Lowering {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AutomaticExtraction => {
                "automatic extraction may not assign an authority stronger than `constraint`; \
                 promote it explicitly if it is an invariant"
            }
            // Certainty imposes no ceiling, so this arm cannot reach a
            // `Classification::reasons`. It exists because `Lowering` carries
            // whatever confidence was stated, and a match that guessed at the
            // unreachable case is how an unreachable case stops being one.
            Self::StatedConfidence(Confidence::Certain) => {
                "stated certainty, which imposes no ceiling of its own"
            }
            Self::StatedConfidence(Confidence::Probable) => {
                "the model called this probable rather than certain"
            }
            Self::StatedConfidence(Confidence::Unsure) => {
                "the model was unsure, so this is a hypothesis until validated"
            }
            Self::StatedDisposition(Disposition::Accepted) => {
                "stated acceptance, which imposes no ceiling of its own"
            }
            Self::StatedDisposition(Disposition::Proposed) => {
                "this was proposed and not accepted, so it is an idea and never an instruction"
            }
            Self::StatedDisposition(Disposition::Abandoned) => {
                "this was abandoned, so it is historical context and must not direct work"
            }
        }
    }
}

impl std::fmt::Display for Lowering {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

/// What an extracted memory's authority became, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classification {
    /// What the model asked for.
    pub declared: MemoryAuthority,
    /// What will be stored. Never stronger than [`Self::declared`].
    pub stored: MemoryAuthority,
    /// Empty when nothing was lowered. Every rule that bound at the final
    /// strength, so a reader sees *which* of them was decisive rather than
    /// having to re-derive it.
    pub reasons: Vec<Lowering>,
}

impl Classification {
    pub fn was_lowered(&self) -> bool {
        self.stored != self.declared
    }
}

/// How binding a class is, strongest first.
///
/// Written out rather than taken from the derived `Ord` on
/// [`MemoryAuthority`]. A derived ordering is a contract nobody can see: it
/// changes when someone reorders the enum for readability, and it would
/// change this policy silently. `the_authority_ranking_matches_the_schema_
/// order` pins the two together instead.
fn rank(authority: MemoryAuthority) -> u8 {
    match authority {
        MemoryAuthority::Invariant => 0,
        MemoryAuthority::Constraint => 1,
        MemoryAuthority::Decision => 2,
        MemoryAuthority::Preference => 3,
        MemoryAuthority::Hypothesis => 4,
        MemoryAuthority::Idea => 5,
        MemoryAuthority::Historical => 6,
    }
}

/// The class at a given rank.
fn at_rank(rank: u8) -> MemoryAuthority {
    // Indexing `ALL` would tie this to the enum's declaration order, which
    // is exactly what `rank` refuses to do. A full match in the other
    // direction keeps the two independent and keeps both exhaustive.
    match rank {
        0 => MemoryAuthority::Invariant,
        1 => MemoryAuthority::Constraint,
        2 => MemoryAuthority::Decision,
        3 => MemoryAuthority::Preference,
        4 => MemoryAuthority::Hypothesis,
        5 => MemoryAuthority::Idea,
        _ => MemoryAuthority::Historical,
    }
}

/// The ceiling a stated confidence imposes, or `None` if it imposes none.
///
/// `None` rather than a sentinel class, and the first version of this
/// function got that wrong in a way worth recording: it returned
/// `Historical` — the weakest class — to mean "no opinion", reasoning that
/// returning the *strongest* would make a no-opinion rule binding. That is
/// backwards. Ceilings combine by taking the **weakest** of them, so the
/// identity element is the strongest class, and a weakest-class sentinel
/// dragged every single classification down to `historical`. Five unit tests
/// caught it at once. `Option` removes the question instead of answering it.
fn confidence_ceiling(confidence: Confidence) -> Option<MemoryAuthority> {
    match confidence {
        // Certainty imposes no ceiling of its own. `EXTRACTOR_CEILING` still
        // applies, which is why certainty cannot reach an invariant.
        Confidence::Certain => None,
        Confidence::Probable => Some(MemoryAuthority::Decision),
        Confidence::Unsure => Some(MemoryAuthority::Hypothesis),
    }
}

/// The ceiling a stated disposition imposes, or `None` if it imposes none.
fn disposition_ceiling(disposition: Disposition) -> Option<MemoryAuthority> {
    match disposition {
        Disposition::Accepted => None,
        Disposition::Proposed => Some(MemoryAuthority::Idea),
        Disposition::Abandoned => Some(MemoryAuthority::Historical),
    }
}

/// Decide the authority an extracted memory is stored under.
///
/// Never stronger than `declared`, and never stronger than
/// [`EXTRACTOR_CEILING`]. See the module documentation for the rules and why
/// the direction is one-way.
///
/// Ceilings combine by taking the **weakest** of them, and a rule with no
/// opinion contributes nothing — see `confidence_ceiling`, whose doc
/// comment records the way this was got wrong once.
pub fn conservative(
    declared: MemoryAuthority,
    confidence: Confidence,
    disposition: Disposition,
) -> Classification {
    let ceilings: Vec<(Lowering, MemoryAuthority)> = [
        (Lowering::AutomaticExtraction, Some(EXTRACTOR_CEILING)),
        (
            Lowering::StatedConfidence(confidence),
            confidence_ceiling(confidence),
        ),
        (
            Lowering::StatedDisposition(disposition),
            disposition_ceiling(disposition),
        ),
    ]
    .into_iter()
    .filter_map(|(reason, ceiling)| ceiling.map(|ceiling| (reason, ceiling)))
    .collect();

    let final_rank = ceilings
        .iter()
        .map(|(_, ceiling)| rank(*ceiling))
        .chain(std::iter::once(rank(declared)))
        .max()
        .unwrap_or_else(|| rank(declared));

    let stored = at_rank(final_rank);
    let reasons = if final_rank > rank(declared) {
        ceilings
            .iter()
            .filter(|(_, ceiling)| rank(*ceiling) == final_rank)
            .map(|(reason, _)| *reason)
            .collect()
    } else {
        Vec::new()
    };

    Classification {
        declared,
        stored,
        reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The property the whole module exists for, over every input there is.
    #[test]
    fn classification_never_strengthens_a_memory() {
        for declared in MemoryAuthority::ALL {
            for confidence in Confidence::ALL {
                for disposition in Disposition::ALL {
                    let result = conservative(*declared, *confidence, *disposition);
                    assert!(
                        rank(result.stored) >= rank(*declared),
                        "{declared} + {confidence} + {disposition} was strengthened to {}",
                        result.stored
                    );
                }
            }
        }
    }

    /// Phase 21A: *avoid promoting uncertain memories to invariants
    /// automatically*. There is no combination of inputs that reaches one.
    #[test]
    fn no_extraction_can_produce_an_invariant() {
        for declared in MemoryAuthority::ALL {
            for confidence in Confidence::ALL {
                for disposition in Disposition::ALL {
                    let result = conservative(*declared, *confidence, *disposition);
                    assert_ne!(
                        result.stored,
                        MemoryAuthority::Invariant,
                        "{declared} + {confidence} + {disposition} minted an invariant"
                    );
                }
            }
        }
    }

    #[test]
    fn a_declared_invariant_is_lowered_to_a_constraint_and_says_why() {
        let result = conservative(
            MemoryAuthority::Invariant,
            Confidence::Certain,
            Disposition::Accepted,
        );
        assert_eq!(result.stored, MemoryAuthority::Constraint);
        assert!(result.was_lowered());
        assert_eq!(result.reasons, vec![Lowering::AutomaticExtraction]);
    }

    /// Phase 21A: *distinguish an accepted decision from an idea that was
    /// merely discussed enthusiastically.*
    #[test]
    fn a_proposal_cannot_be_stored_as_a_decision_however_confident_the_model_is() {
        let result = conservative(
            MemoryAuthority::Decision,
            Confidence::Certain,
            Disposition::Proposed,
        );
        assert_eq!(result.stored, MemoryAuthority::Idea);
        assert!(!result.stored.is_binding());
        assert_eq!(
            result.reasons,
            vec![Lowering::StatedDisposition(Disposition::Proposed)]
        );
    }

    #[test]
    fn an_abandoned_approach_becomes_historical_context() {
        let result = conservative(
            MemoryAuthority::Constraint,
            Confidence::Certain,
            Disposition::Abandoned,
        );
        assert_eq!(result.stored, MemoryAuthority::Historical);
        assert!(!result.stored.is_binding());
    }

    #[test]
    fn an_unsure_claim_lands_no_stronger_than_a_hypothesis() {
        let result = conservative(
            MemoryAuthority::Constraint,
            Confidence::Unsure,
            Disposition::Accepted,
        );
        assert_eq!(result.stored, MemoryAuthority::Hypothesis);
    }

    /// An accepted, certain constraint is the strongest thing extraction can
    /// produce, and it produces it — the policy is conservative, not inert.
    #[test]
    fn a_certain_accepted_constraint_is_stored_as_a_constraint() {
        let result = conservative(
            MemoryAuthority::Constraint,
            Confidence::Certain,
            Disposition::Accepted,
        );
        assert_eq!(result.stored, MemoryAuthority::Constraint);
        assert!(result.stored.is_binding());
        assert!(!result.was_lowered());
        assert!(result.reasons.is_empty());
    }

    /// A weak declaration is left alone. Confidence never raises anything,
    /// which is the other half of the one-way rule.
    #[test]
    fn a_certain_idea_is_still_an_idea() {
        let result = conservative(
            MemoryAuthority::Idea,
            Confidence::Certain,
            Disposition::Accepted,
        );
        assert_eq!(result.stored, MemoryAuthority::Idea);
        assert!(!result.was_lowered());
    }

    /// `rank` is written out by hand so that reordering the enum cannot
    /// change the policy. This is what makes that safe: if the two ever
    /// disagree, the disagreement is a failing test rather than a silently
    /// different classification.
    #[test]
    fn the_authority_ranking_matches_the_schema_order() {
        let ranked: Vec<MemoryAuthority> = (0..7).map(at_rank).collect();
        assert_eq!(ranked.as_slice(), MemoryAuthority::ALL);

        for (index, authority) in MemoryAuthority::ALL.iter().enumerate() {
            assert_eq!(usize::from(rank(*authority)), index);
        }
    }

    /// The binding classes are exactly the strongest three, which is what
    /// makes "lower the rank" mean "make it less binding".
    #[test]
    fn ranking_and_bindingness_agree() {
        for authority in MemoryAuthority::ALL {
            assert_eq!(
                authority.is_binding(),
                rank(*authority) <= rank(MemoryAuthority::Decision),
                "{authority} disagrees between is_binding and rank"
            );
        }
    }
}
