//! The task catalogue: twelve commits of this repository, four per tier.
//!
//! The catalogue is a fixed table rather than a discovered one, because a
//! comparison whose task set can drift between runs compares two things that
//! were never the same question. Specification: `docs/product/pane/ruler.md`
//! §2, which is also where each statement's derivation is recorded.

use super::model::{Task, Tier};

/// The twelve tasks, in tier then index order.
///
/// Populated by `GH-PANE-61A-SCORE` from `ruler.md` §2.
pub static CATALOGUE: &[Task] = &[];

/// The task with this id, or `None`. Ids are case-sensitive: `L1`, not `l1`.
pub fn lookup(id: &str) -> Option<&'static Task> {
    CATALOGUE.iter().find(|task| task.id == id)
}

/// Every task in one tier, in catalogue order.
pub fn in_tier(tier: Tier) -> impl Iterator<Item = &'static Task> {
    CATALOGUE.iter().filter(move |task| task.tier == tier)
}
