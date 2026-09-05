//! Durable, project-scoped memory.
//!
//! > **Memory belongs to the project, not to the model.**
//!
//! Everything here lives in the one SQLite database bound to the active
//! project: no cross-project query, no second store, since a
//! [`ProjectMemory`] is opened from a [`crate::Runtime`], which resolves
//! exactly one project.
//!
//! `store`, [`search`] and [`snapshot`] are storage and retrieval; the
//! admission guard ([`admit`], see [`MemoryRefusal`]) is deliberately
//! narrow, not a stand-in for judging what is durable. [`inject`] chooses
//! which memories reach a routed session, reusing [`search`]'s ranking.
//! [`extract`] is where the judgment lives, and **where the credential
//! control lives**, since `subject` and `body` are free text no schema can
//! keep a secret out of; see [`extract::credentials`].
//!
//! [`MemoryKind`], [`MemoryAuthority`] and [`MemoryStatus`] are three
//! independent axes that overlap in spelling, not meaning — migration 4's
//! comment records why none may be folded into another.
// History: design-decisions.md, "Trims: memory export and extraction module docs", memory/mod.rs module doc.

pub mod export;
pub mod export_local;
pub mod extract;
pub mod inject;
mod policy;
pub mod rerank;
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
pub use rerank::{RerankOutcome, RetrievalTrace};
pub use store::{
    AuthorityChange, Classifier, Clock, ConflictResolver, DecisionProvenance, FileAssociation,
    MemoryAuthority, MemoryId, MemoryKind, MemoryRecord, MemoryStatus, MemoryStore,
    MemoryStoreError, NewMemory, ProjectMemory, ProjectPhase, ReviewReason, SourceEvents,
    normalize_observed_path,
};
