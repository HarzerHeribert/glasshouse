//! Durable, project-scoped memory.
//!
//! > **Memory belongs to the project, not to the model.**
//!
//! Everything here lives in the one SQLite database bound to the active
//! project (see `crate::database`). There is no cross-project query, no
//! second store, and no path a caller can supply: a [`ProjectMemory`] is
//! opened from a [`crate::Runtime`], which resolves exactly one project.
//!
//! # What this module is, and is not
//!
//! It is the storage and retrieval half of Phases 20, 22, 23 and 26 — a table,
//! its lifecycle, its full-text index, and the operations an agent asks it
//! questions through. It is **not** the extractor: nothing here decides that a
//! conversation contained a durable fact. Phase 21 owns that judgment, and
//! this module's admission guard ([`admit`]) is deliberately narrow — see
//! [`MemoryRefusal`] — rather than a stand-in for it.
//!
//! # The two enum axes, and why they are two
//!
//! [`MemoryKind`] is *what sort of thing was remembered*. [`MemoryAuthority`]
//! is *how binding it is*. They overlap in spelling and not in meaning: a
//! [`MemoryKind::Finding`] can be an [`MemoryAuthority::Invariant`], and a
//! [`MemoryKind::Decision`] can have decayed to [`MemoryAuthority::Historical`].
//! [`MemoryStatus`] is a third, independent axis: where the memory sits in its
//! lifecycle. Migration 4's comment records why none of the three may be
//! folded into another.

mod policy;
pub mod search;
pub mod snapshot;
mod store;

pub use policy::{MemoryRefusal, admit};
pub use store::{
    Clock, ConflictResolver, MemoryAuthority, MemoryId, MemoryKind, MemoryRecord, MemoryStatus,
    MemoryStore, MemoryStoreError, NewMemory, ProjectMemory,
};
