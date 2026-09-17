//! [`JobKind`]: the kind of bounded internal work a resource choice used to
//! be made for. The choosing is gone (2026-09-16 ruling — Glasshouse never
//! decides which model is used), but an entitlement may still allow or deny
//! a job kind by name (`config::entitlement::ConfiguredJobKind`), and the
//! context-firewall reducer and memory extraction still tag their own calls
//! with one so the evidence ledger can say which job a call served.
// History: design-decisions.md, "Glasshouse never decides which model is used", GH-GLASSHOUSE-ROUTING-DELETE.

/// The kind of bounded internal work a call was made for.
///
/// Carried so a call can be recorded against the job that made it — Phase
/// 39's "record which resource performed important memory extraction or
/// classification for debugging" needs the pair, and a job kind is a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Classification,
    MemoryExtraction,
    Reranking,
    /// Glasshouse's own automated evaluation or test run.
    Evaluation,
    /// The context firewall's semantic reducer (Phase 57B, map line 1997) —
    /// a disposable job that selects which of the deterministic ladder's
    /// retained candidates a coding session actually needs.
    ContextReduction,
}

impl JobKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Classification => "classification",
            Self::MemoryExtraction => "memory extraction",
            Self::Reranking => "reranking",
            Self::Evaluation => "evaluation",
            Self::ContextReduction => "context-reduction",
        }
    }
}

impl std::fmt::Display for JobKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}
