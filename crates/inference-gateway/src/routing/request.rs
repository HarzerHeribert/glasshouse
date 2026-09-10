//! The class of task a caller states for a request -- a word the gateway
//! records and never derives.

use std::fmt;

/// Line 1457's *task class*, derived from the classification's own signal
/// fields the way `TaskClassification::hard_capabilities` is — one place,
/// never a second field that could disagree with the signals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskClass {
    /// A question needing no repository.
    Question,
    /// Reading this repository without changing it.
    Investigation,
    /// Writing or changing code.
    CodeModification,
    /// Running something.
    ShellWork,
    /// Driving a browser.
    BrowserWork,
}

impl TaskClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Question => "question",
            Self::Investigation => "investigation",
            Self::CodeModification => "code modification",
            Self::ShellWork => "shell work",
            Self::BrowserWork => "browser work",
        }
    }

    /// The inverse of [`TaskClass::as_str`], for
    /// `routing_observations.task_class` (`crate::database` migration 23).
    ///
    /// `None` for anything this build does not recognise — and that is a
    /// deliberate difference from
    /// [`crate::routing::evidence::FailureClass::from_stored`], whose caller
    /// turns an unknown word into an error. See migration 23's own doc
    /// comment: a class is a bucketing input to an average, so a row this
    /// build cannot bucket is one more request of no class it counts, never
    /// a reason to fail the row.
    ///
    /// Every variant round-trips, pinned by
    /// `every_task_class_round_trips_through_its_stored_word`.
    pub fn from_stored(text: &str) -> Option<Self> {
        match text {
            "question" => Some(Self::Question),
            "investigation" => Some(Self::Investigation),
            "code modification" => Some(Self::CodeModification),
            "shell work" => Some(Self::ShellWork),
            "browser work" => Some(Self::BrowserWork),
            _ => None,
        }
    }

    /// Every variant, for a reader that must bucket by all of them and for
    /// the round-trip test. Ordered as declared.
    pub const ALL: [Self; 5] = [
        Self::Question,
        Self::Investigation,
        Self::CodeModification,
        Self::ShellWork,
        Self::BrowserWork,
    ];
}

impl fmt::Display for TaskClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
