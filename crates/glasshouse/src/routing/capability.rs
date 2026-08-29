//! The capability registry — map line 1382: *"describe each harness and
//! model resource with a small set of capabilities used for routing."*
//!
//! # Why this is not a second [`HardCapability`]
//!
//! [`super::classify::HardCapability`] states what a *task* needs.
//! [`ResourceCapabilities`] describes what a *resource* can do. Merging the
//! two into one scale would let a router compare a task's tier against its
//! own tier and believe that proved something —
//! `super::classify`'s own doc comment on line 79 already refuses this for
//! the same reason. [`axis_for`] is the one comparison function that joins
//! them; nothing else in this module or [`super::session`] collapses the
//! two.
//!
//! # Why this is not a widening of `harness::Capabilities`
//!
//! Map line 1382 asks for "each harness **and model** resource". A harness
//! adapter has no business declaring a model's context window or its
//! price/speed class, so [`ResourceCapabilities`] is *built from*
//! [`crate::harness::Capabilities`] plus [`ResourceFacts`] — a model/resource
//! fact a harness adapter never sees — rather than being a bigger version of
//! the adapter-declared type.
//!
//! # Why every axis is a [`Declared<bool>`]
//!
//! `Unverified` is not absent. `harness::Capabilities`' own tests pin that an
//! unverified axis must never be scored as a `no`
//! (`an_unverified_capability_is_not_treated_as_present`), and this registry
//! carries the same rule forward: [`ResourceCapabilities::axis`] returns
//! *established present*, *established absent*, or *not established* —
//! never a bare bool.
//!
//! # 1390 — updatable without changing the core router
//!
//! [`super::session::capability_fit`] contains no `match` on a resource's
//! identity and no capability values of its own; it only asks
//! [`ResourceCapabilities::axis`] a question and applies a fixed scoring
//! formula. To add a resource, correct an axis, or add a new model-level
//! fact, construct or edit a [`ResourceFacts`] value — nothing in
//! `session.rs` changes. `Destination::with_resource_facts` (`super::session`)
//! is where a caller attaches one; the harness half comes from the adapter
//! [`crate::harness::adapter_for`] already returns.

use crate::harness::{Capabilities as HarnessCapabilities, Declared};

use super::classify::HardCapability;

/// The seven capabilities map lines 1383–1389 ask a router to describe a
/// resource with.
///
/// `subagents` is `harness::Capabilities`' own fifth field and not one of
/// the map's seven — it is carried through [`ResourceCapabilities`] nowhere,
/// because nothing here closes a box for it; see the packet for
/// `GH-ROUTING-CAPABILITY`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CapabilityAxis {
    /// Box 1383.
    CodeEdit,
    /// Box 1384.
    ShellToolUse,
    /// Box 1385.
    BrowserUse,
    /// Box 1386. A model/resource property — no `harness::Capabilities`
    /// field carries this.
    LargeContext,
    /// Box 1387. A model/resource property.
    FastCheapAnalysis,
    /// Box 1388. A model/resource property.
    RepositoryReview,
    /// Box 1389.
    Mcp,
}

impl CapabilityAxis {
    /// All seven, in the map's own order — box lines 1383–1389, and the
    /// direct evidence acceptance test 3 reads: every axis is representable
    /// and every axis appears in [`ResourceCapabilities::render`].
    pub const ALL: [Self; 7] = [
        Self::CodeEdit,
        Self::ShellToolUse,
        Self::BrowserUse,
        Self::LargeContext,
        Self::FastCheapAnalysis,
        Self::RepositoryReview,
        Self::Mcp,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::CodeEdit => "code-edit",
            Self::ShellToolUse => "shell/tool-use",
            Self::BrowserUse => "browser-use",
            Self::LargeContext => "large-context",
            Self::FastCheapAnalysis => "fast-cheap-analysis",
            Self::RepositoryReview => "repository-review",
            Self::Mcp => "MCP",
        }
    }
}

/// Ruling 1's comparison function: which axis a task's hard capability
/// requirement is judged against.
///
/// Total over [`HardCapability`], not over [`CapabilityAxis`] — there is no
/// map line yet asking for `large-context`, `fast-cheap-analysis` or `MCP`
/// as a *task* requirement, so three of the seven axes have no
/// `HardCapability` variant to be reached by. They still close their boxes
/// through [`ResourceCapabilities::render`] (acceptance test 3); they simply
/// have no production routing consumer yet, and inventing a `HardCapability`
/// variant nothing classifies to would be exactly the kind of mechanism
/// practice §35 warns about.
pub fn axis_for(requirement: HardCapability) -> CapabilityAxis {
    match requirement {
        HardCapability::RepositoryAccess => CapabilityAxis::CodeEdit,
        HardCapability::ShellExecution => CapabilityAxis::ShellToolUse,
        HardCapability::BrowserInteraction => CapabilityAxis::BrowserUse,
    }
}

/// Model/resource facts a harness adapter has no business declaring —
/// ruling 2.
///
/// This is 1390's answer for the three axes `harness::Capabilities` has no
/// field for, and it is also the override a caller reaches for when a
/// *specific* model is known to differ from what its harness declares in
/// general: [`ResourceCapabilities::describe`] prefers a `Verified` fact
/// here over the harness's own declaration, because model-specific evidence
/// is more specific than a harness-wide one.
///
/// The default is [`Self::UNVERIFIED`] — nothing about a resource's facts is
/// known until a caller attaches some, which is the honest starting point
/// every other `Declared` value in this codebase uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceFacts {
    pub code_edit: Declared<bool>,
    pub shell_tool_use: Declared<bool>,
    pub browser_use: Declared<bool>,
    pub large_context: Declared<bool>,
    pub fast_cheap_analysis: Declared<bool>,
    pub repository_review: Declared<bool>,
    pub mcp: Declared<bool>,
}

impl ResourceFacts {
    /// Nothing known — the starting point a caller with no model-level
    /// evidence attaches.
    pub const UNVERIFIED: Self = Self {
        code_edit: Declared::Unverified,
        shell_tool_use: Declared::Unverified,
        browser_use: Declared::Unverified,
        large_context: Declared::Unverified,
        fast_cheap_analysis: Declared::Unverified,
        repository_review: Declared::Unverified,
        mcp: Declared::Unverified,
    };
}

impl Default for ResourceFacts {
    fn default() -> Self {
        Self::UNVERIFIED
    }
}

/// A resource's capability across all seven routing axes — map line 1382,
/// and the type `super::session::capability_fit` reads.
///
/// Built by [`Self::describe`], never assembled field-by-field outside this
/// module, so the merge rule (model fact over harness declaration) is
/// applied exactly once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceCapabilities {
    code_edit: Declared<bool>,
    shell_tool_use: Declared<bool>,
    browser_use: Declared<bool>,
    large_context: Declared<bool>,
    fast_cheap_analysis: Declared<bool>,
    repository_review: Declared<bool>,
    mcp: Declared<bool>,
}

impl ResourceCapabilities {
    /// Ruling 2: build a resource's description from what its harness
    /// declares plus the model/resource facts a harness adapter cannot see.
    ///
    /// `facts` wins whenever it is `Verified` — a model-specific fact
    /// outranks a harness-wide one — and is the *only* source for the three
    /// axes `harness::Capabilities` has no field for.
    pub fn describe(harness: &HarnessCapabilities, facts: ResourceFacts) -> Self {
        Self {
            code_edit: prefer(facts.code_edit, harness.code_editing),
            shell_tool_use: prefer(facts.shell_tool_use, harness.shell_access),
            browser_use: prefer(facts.browser_use, harness.browser_use),
            large_context: facts.large_context,
            fast_cheap_analysis: facts.fast_cheap_analysis,
            repository_review: facts.repository_review,
            mcp: prefer(facts.mcp, harness.mcp),
        }
    }

    /// The tri-state ruling 3 requires: established present, established
    /// absent, or not established — never a bare bool.
    pub fn axis(&self, axis: CapabilityAxis) -> Declared<bool> {
        match axis {
            CapabilityAxis::CodeEdit => self.code_edit,
            CapabilityAxis::ShellToolUse => self.shell_tool_use,
            CapabilityAxis::BrowserUse => self.browser_use,
            CapabilityAxis::LargeContext => self.large_context,
            CapabilityAxis::FastCheapAnalysis => self.fast_cheap_analysis,
            CapabilityAxis::RepositoryReview => self.repository_review,
            CapabilityAxis::Mcp => self.mcp,
        }
    }

    /// One line per axis, all seven always present — the direct evidence for
    /// boxes 1383–1389 that acceptance test 3 reads: every axis is
    /// representable and named in a rendered description.
    pub fn render(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        for axis in CapabilityAxis::ALL {
            let state = match self.axis(axis) {
                Declared::Verified {
                    value: true,
                    evidence,
                } => format!("present ({evidence})"),
                Declared::Verified {
                    value: false,
                    evidence,
                } => format!("absent ({evidence})"),
                Declared::Unverified => "not established".to_owned(),
            };
            let _ = writeln!(out, "  {:<20} {state}", axis.name());
        }
        out
    }
}

/// Model-specific evidence outranks a harness-wide declaration — the more
/// specific fact wins whenever there is one.
fn prefer(specific: Declared<bool>, general: Declared<bool>) -> Declared<bool> {
    match specific {
        Declared::Verified { .. } => specific,
        Declared::Unverified => general,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verified(value: bool) -> Declared<bool> {
        Declared::verified(value, "test evidence")
    }

    #[test]
    fn every_axis_is_representable_and_named_in_the_rendered_description() {
        let resource = ResourceCapabilities::describe(
            &HarnessCapabilities::UNVERIFIED,
            ResourceFacts::UNVERIFIED,
        );
        let rendered = resource.render();
        for axis in CapabilityAxis::ALL {
            assert!(
                rendered.contains(axis.name()),
                "axis `{}` did not appear in the rendered registry description:\n{rendered}",
                axis.name()
            );
        }
    }

    #[test]
    fn a_model_fact_outranks_the_harness_wide_declaration() {
        let harness = HarnessCapabilities {
            browser_use: verified(true),
            ..HarnessCapabilities::UNVERIFIED
        };
        let facts = ResourceFacts {
            browser_use: verified(false),
            ..ResourceFacts::UNVERIFIED
        };
        let resource = ResourceCapabilities::describe(&harness, facts);
        assert_eq!(
            resource.axis(CapabilityAxis::BrowserUse),
            verified(false),
            "a model-specific fact must outrank the harness's own declaration"
        );
    }

    #[test]
    fn an_unestablished_fact_falls_back_to_the_harness_declaration() {
        let harness = HarnessCapabilities {
            shell_access: verified(true),
            ..HarnessCapabilities::UNVERIFIED
        };
        let resource = ResourceCapabilities::describe(&harness, ResourceFacts::UNVERIFIED);
        assert_eq!(
            resource.axis(CapabilityAxis::ShellToolUse),
            verified(true),
            "with no model-specific fact, the harness's own declaration must carry through"
        );
    }

    #[test]
    fn the_three_model_only_axes_have_no_harness_fallback() {
        let harness = HarnessCapabilities::UNVERIFIED;
        let facts = ResourceFacts {
            large_context: verified(true),
            ..ResourceFacts::UNVERIFIED
        };
        let resource = ResourceCapabilities::describe(&harness, facts);
        assert_eq!(resource.axis(CapabilityAxis::LargeContext), verified(true));
        assert_eq!(
            resource.axis(CapabilityAxis::FastCheapAnalysis),
            Declared::Unverified
        );
    }

    #[test]
    fn axis_for_covers_every_hard_capability() {
        assert_eq!(
            axis_for(HardCapability::RepositoryAccess),
            CapabilityAxis::CodeEdit
        );
        assert_eq!(
            axis_for(HardCapability::ShellExecution),
            CapabilityAxis::ShellToolUse
        );
        assert_eq!(
            axis_for(HardCapability::BrowserInteraction),
            CapabilityAxis::BrowserUse
        );
    }
}
