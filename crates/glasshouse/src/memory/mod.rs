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
//! `store`, [`search`] and [`snapshot`] are the storage and retrieval half
//! of Phases 20, 22, 23 and 26 — a table, its lifecycle, its full-text index,
//! and the operations an agent asks it questions through. None of them decides
//! that a conversation contained a durable fact; the admission guard
//! ([`admit`]) is deliberately narrow — see [`MemoryRefusal`] — rather than a
//! stand-in for that judgment.
//!
//! [`inject`] is Phase 27's consumer of that half: the step that chooses
//! which of those memories reach a session Glasshouse is routing a task to,
//! and labels them so an agent can never mistake a remembered sentence for
//! something the user just said. It decides nothing about relevance either —
//! it reuses [`search`]'s ranking rather than ranking again.
//!
//! [`extract`] is the other half, and Phase 21 is where the judgment lives: it
//! bounds and scrubs session activity, asks a model for structured memories,
//! and validates the reply against a contract. **It is also where the
//! credential control lives.** `subject` and `body` are free text and no
//! schema can stop a secret being put in one, so the producer is the only
//! place the guarantee can be made — see [`extract::credentials`] and the
//! doc comment on
//! `crate::session::store::tests::the_project_database_schema_has_nowhere_to_put_a_credential`,
//! which hands the control here by name.
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

pub mod export;
pub mod export_local;
pub mod extract;
pub mod inject;
mod policy;
pub mod search;
pub mod snapshot;
mod store;

pub use export::{ExportError, Manifest, Selection, TrackedKnowledge, WrittenFile};
pub use extract::{
    ExtractionModel, ExtractionOutcome, ExtractionTrigger, Extractor, ModelError,
    disposable::RoutedModel,
    model::{ConfiguredModel, ConfiguredModelError},
};
pub use policy::{MemoryRefusal, admit};
pub use store::{
    AuthorityChange, Classifier, Clock, ConflictResolver, DecisionProvenance, FileAssociation,
    MemoryAuthority, MemoryId, MemoryKind, MemoryRecord, MemoryStatus, MemoryStore,
    MemoryStoreError, NewMemory, ProjectMemory, ProjectPhase, ReviewReason, SourceEvents,
    normalize_observed_path,
};
