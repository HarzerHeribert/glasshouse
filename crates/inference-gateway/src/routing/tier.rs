//! The workload tier a caller states for a request. The gateway never
//! classifies; it accepts a tier the caller names, in its own words.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WorkloadTier {
    /// Tier 0: deterministic or trivial work that should not require an LLM
    /// when simple rules are sufficient (line 1396).
    Deterministic,
    /// Tier 1: lightweight classification, extraction, reranking,
    /// formatting, and simple factual codebase lookup (line 1397). A
    /// disposable, free, or local model is expected to be sufficient.
    Leaf,
    /// Tier 2: routine coding, bounded debugging, focused review, and small
    /// multi-file changes (line 1398). An ordinary interactive model.
    Standard,
    /// Tier 3: difficult debugging, architecture-sensitive changes, broad
    /// refactors, and work requiring strong reasoning or long-lived
    /// repository context (line 1399). The strongest configured model the
    /// session has, short of a Tier 4 need.
    Heavy,
    /// Tier 4: frontier work where failure cost or reasoning difficulty
    /// justifies the strongest available model or a warm premium session
    /// (line 1400).
    Frontier,
}

impl WorkloadTier {
    /// One step more capable, or unchanged at the top. Never a step down —
    /// there is no direction in which escalating a workload tier should make
    /// it cheaper.
    pub fn escalate(self) -> Self {
        match self {
            Self::Deterministic => Self::Leaf,
            Self::Leaf => Self::Standard,
            Self::Standard => Self::Heavy,
            Self::Heavy | Self::Frontier => Self::Frontier,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deterministic => "deterministic",
            Self::Leaf => "leaf",
            Self::Standard => "standard",
            Self::Heavy => "heavy",
            Self::Frontier => "frontier",
        }
    }
}

impl fmt::Display for WorkloadTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
