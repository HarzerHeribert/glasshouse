//! Phase 9K's harness half: turning a Glasshouse response profile into the
//! closest safe thing one harness actually has, and recording which one that
//! was.
//!
//! [`mod@crate::profile::response`] is the domain model — five independent
//! axes, presets, roles and a six-layer precedence chain, and it knows nothing
//! about any harness. This module is the other side of line 602: *"Let the
//! HarnessAdapter translate a Glasshouse response profile into the closest
//! safe native harness configuration."*
//!
//! # The vocabulary stays inside the adapter
//!
//! Line 603 asks for Claude Code output styles and Codex personalities to be
//! treated as *adapter examples* rather than universal Glasshouse concepts, so
//! there is no `OutputStyle` type here and no enum of style names. What
//! crosses the seam is a [`NativeStyle`], whose `selection` is a plain string
//! in the harness's own words, carrying the evidence it was read from —
//! exactly the shape [`Declared`](super::Declared) already uses for every
//! other adapter fact.
//!
//! # What may never happen, expressed as a type that cannot say it
//!
//! Line 607: *"Never replace the complete native harness system prompt merely
//! to control verbosity, tone, or answer structure."* [`AppliedMechanism`] has
//! three variants — a native mechanism, an additive instruction, and nothing —
//! and **no variant that means "replaced"**. Nothing downstream can record a
//! replacement because nothing upstream can perform one: the only things
//! [`apply`] produces are a settings key the harness itself documents and
//! arguments an adapter declared as *additive*.
//!
//! Claude Code makes the distinction concrete and dangerous enough to be worth
//! stating: `claude --help` on 2.1.247 documents both `--system-prompt
//! <prompt>` ("System prompt to use for the session") and
//! `--append-system-prompt <prompt>` ("Append a system prompt to the default
//! system prompt"). The first is the line-607 violation; only the second is
//! declared by the adapter, and
//! `the_launch_never_replaces_a_native_system_prompt` fails on a build where
//! the first appears in a launch.
//!
//! # The gateway is not in this file
//!
//! Line 608: *"Do not make gateway-side system-prompt rewriting the default
//! way Glasshouse applies a response profile."* This module imports nothing
//! from [`mod@crate::gateway`] and has no path into it. The only two
//! mechanisms [`apply`] can choose are the harness's own; a gateway-side
//! transformation is a thing a **user configures on a provider** and
//! [`crate::config::response`] surfaces as backend metadata — line 609 — never
//! something Glasshouse reaches for on its own.

use std::ffi::OsString;

use crate::integrations::IntegrationId;
use crate::profile::response::{ResolvedProfile, ResponseProfile, floor_directive};

/// A harness's own communication-style mechanism, as that harness names it.
///
/// `selection` is deliberately the harness's vocabulary rather than
/// Glasshouse's: Claude Code says `Concise`, and calling it `concise` here to
/// match a Glasshouse axis would be the universal-concept line 603 forbids.
///
/// `evidence` is the same contract as [`Declared`](super::Declared)'s: a place
/// concrete enough to re-check. An adapter that cannot cite one declares no
/// native style at all rather than a plausible guess.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeStyle {
    /// How the harness's own documentation describes the mechanism.
    pub mechanism: &'static str,
    /// The harness's own name for the style being selected.
    pub selection: &'static str,
    /// The harness's own description of that style.
    ///
    /// Carried through verbatim rather than paraphrased, because it is the
    /// evidence for the choice: a reader who disagrees that `Concise` is the
    /// right style for a concise profile can only say so if they can see what
    /// the harness itself claims the style does.
    pub selection_description: &'static str,
    /// Where the mechanism and this selection were read from.
    pub evidence: &'static str,
    /// How the selection reaches the child process.
    pub delivery: NativeDelivery,
}

/// How a native style selection reaches one child process.
///
/// Two variants because two shapes exist in the harnesses read so far, not
/// because two is a design: a harness that selects a style some third way gets
/// a third variant, in this file, and nothing else changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeDelivery {
    /// One key in the single Glasshouse-owned settings document this harness
    /// is pointed at.
    ///
    /// `file_name` matters: a harness may read only *one* such document — see
    /// [`crate::session::HarnessSelection::install_session_document`], where a
    /// verified Claude Code observation makes that a hazard rather than a
    /// detail — so an adapter naming the same file its hooks use gets one
    /// merged document and one flag.
    SettingsKey {
        file_name: &'static str,
        /// The launch option that points the harness at that document, for
        /// the case where Glasshouse has to write it on its own — a harness
        /// whose hook document went somewhere else, or none at all.
        flag: &'static str,
        key: &'static str,
        value: &'static str,
    },
    /// Arguments appended to the launch.
    Arguments(&'static [&'static str]),
}

/// A harness's mechanism for adding an instruction *alongside* its own system
/// prompt.
///
/// The word that matters is *alongside*. An adapter may declare a mechanism
/// here only if the harness's own documentation says it appends; a mechanism
/// that replaces belongs nowhere in this file, and there is no variant of
/// [`AppliedMechanism`] that could record having used one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdditiveInjection {
    /// How the harness's own documentation describes the mechanism.
    pub mechanism: &'static str,
    /// Where that description was read from.
    pub evidence: &'static str,
    /// The flag the instruction follows, e.g. `--append-system-prompt`.
    pub flag: &'static str,
}

/// Which mechanism actually carried the response profile — line 604.
///
/// Three variants, and no fourth. See the module documentation for why the
/// absent fourth is the point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppliedMechanism {
    /// The harness's own communication-style mechanism.
    Native {
        mechanism: &'static str,
        selection: &'static str,
        selection_description: &'static str,
        evidence: &'static str,
    },
    /// An instruction added alongside the harness's own system prompt,
    /// because no native mechanism could represent the selected profile.
    Additive {
        mechanism: &'static str,
        evidence: &'static str,
    },
    /// Nothing was applied, and this says why.
    ///
    /// Two quite different situations reach here and the `reason` separates
    /// them: nothing above the harness default asked for a profile, or the
    /// harness declares no mechanism Glasshouse may safely use. The first is
    /// the ordinary state of an unconfigured Glasshouse and is not a
    /// shortcoming.
    NotApplied { reason: String },
}

impl AppliedMechanism {
    /// A one-word category, for a report column.
    pub fn category(&self) -> &'static str {
        match self {
            Self::Native { .. } => "native",
            Self::Additive { .. } => "additive",
            Self::NotApplied { .. } => "none",
        }
    }

    /// A sentence naming the mechanism. Never empty — an empty cell reads as
    /// missing data, and "nothing was applied" is a claim Glasshouse is making
    /// on purpose.
    pub fn describe(&self) -> String {
        match self {
            Self::Native {
                mechanism,
                selection,
                selection_description,
                ..
            } => format!("{mechanism}, set to `{selection}` — {selection_description}"),
            Self::Additive { mechanism, .. } => (*mechanism).to_owned(),
            Self::NotApplied { reason } => reason.clone(),
        }
    }

    pub fn evidence(&self) -> Option<&'static str> {
        match self {
            Self::Native { evidence, .. } | Self::Additive { evidence, .. } => Some(evidence),
            Self::NotApplied { .. } => None,
        }
    }

    pub fn was_applied(&self) -> bool {
        !matches!(self, Self::NotApplied { .. })
    }
}

/// One response profile, applied to one harness: what was done, and what it
/// puts on the launch.
///
/// The two halves are kept apart because they answer different questions, and
/// conflating them is the failure line 604 exists to prevent: `mechanism` is
/// what a user is *told* was applied, and `settings`/`args` are what actually
/// reaches the child. A build in which those two disagree — a record saying
/// `native` while the fallback ran — is exactly what
/// `the_applied_record_names_the_mechanism_that_actually_reached_the_child`
/// fails on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Application {
    mechanism: AppliedMechanism,
    settings_file: Option<&'static str>,
    settings_flag: Option<&'static str>,
    settings: Vec<(&'static str, String)>,
    args: Vec<OsString>,
    /// Anything a reader needs to know about how far application got.
    notes: Vec<String>,
}

impl Application {
    /// An application that does nothing, for a caller with no response
    /// profile to apply.
    ///
    /// `reason` is not decoration: every [`AppliedMechanism::NotApplied`] says
    /// why, so a report can never show an empty cell where a decision was
    /// made.
    pub fn none(reason: impl Into<String>) -> Self {
        Self::nothing(reason.into())
    }

    fn nothing(reason: String) -> Self {
        Self {
            mechanism: AppliedMechanism::NotApplied { reason },
            settings_file: None,
            settings_flag: None,
            settings: Vec::new(),
            args: Vec::new(),
            notes: Vec::new(),
        }
    }

    pub fn mechanism(&self) -> &AppliedMechanism {
        &self.mechanism
    }

    /// Which Glasshouse-owned settings document [`Application::settings`]
    /// belongs in, when there is one.
    pub fn settings_file(&self) -> Option<&'static str> {
        self.settings_file
    }

    /// The launch option that points the harness at that document, when
    /// Glasshouse has to write it on its own.
    pub fn settings_flag(&self) -> Option<&'static str> {
        self.settings_flag
    }

    /// Keys to merge into that document.
    pub fn settings(&self) -> &[(&'static str, String)] {
        &self.settings
    }

    /// Arguments to append to the launch.
    pub fn args(&self) -> &[OsString] {
        &self.args
    }

    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    /// Every piece of instruction *text* this application sends to the child.
    ///
    /// A settings key's value and an argument that follows a flag both count;
    /// a flag name does not. This is what
    /// `no_instruction_glasshouse_writes_omits_the_floor` reads, and it is why
    /// the floor is checkable rather than merely intended.
    pub fn instruction_text(&self) -> String {
        let mut text: Vec<String> = self
            .settings
            .iter()
            .map(|(_, value)| value.clone())
            .collect();
        // Skip the flags themselves: an adapter's declared flag is a name, and
        // the instruction is always the argument after it.
        let mut args = self.args.iter();
        while let Some(arg) = args.next() {
            if arg.to_string_lossy().starts_with('-') {
                if let Some(value) = args.next() {
                    text.push(value.to_string_lossy().into_owned());
                }
            } else {
                text.push(arg.to_string_lossy().into_owned());
            }
        }
        text.join("\n")
    }
}

/// Apply `resolved` to `adapter`, preferring the harness's own mechanism.
///
/// This is line 601 as a function. The order is fixed and each step has a
/// reason:
///
/// 1. **Nothing above the harness default asked for anything** — apply
///    nothing. An unconfigured Glasshouse must leave a harness exactly as it
///    found it, and [`ResolvedProfile::is_harness_default`] is what makes
///    "nobody asked" distinguishable from "somebody asked for the defaults".
/// 2. **A native mechanism that can represent this profile** — take it. The
///    adapter decides whether it can; a harness whose only styles would weaken
///    its coding instructions declares none for that profile and falls
///    through, which is the *"without weakening coding instructions"* half of
///    line 601 and is enforced inside the adapter that knows its own styles.
/// 3. **An additive instruction** — the profile's own sentences, appended
///    beside the harness's system prompt, never replacing it.
/// 4. **Nothing, and say why.**
///
/// # The floor rides along with whatever wins
///
/// A native style is the harness vendor's own wording and Glasshouse does not
/// get to edit it, so when an additive mechanism also exists the floor
/// sentence — [`floor_directive`] — is appended beside the native selection.
/// That is not belt-and-braces: the native selection expresses the five axes,
/// and [`crate::profile::response::REQUIRED_REPORTS`] is not one of the axes.
/// It is the thing no axis may reduce.
pub fn apply(adapter: &dyn super::HarnessAdapter, resolved: &ResolvedProfile) -> Application {
    let id = adapter.id();
    if resolved.is_harness_default() {
        return Application::nothing(format!(
            "nothing above the harness default asked for a response profile, so {}'s own \
             communication behaviour is untouched",
            id.display_name()
        ));
    }

    let profile = resolved.profile();
    let additive = adapter.additive_response_injection();

    if let Some(native) = adapter.native_response_style(&profile) {
        let mut application = Application {
            mechanism: AppliedMechanism::Native {
                mechanism: native.mechanism,
                selection: native.selection,
                selection_description: native.selection_description,
                evidence: native.evidence,
            },
            settings_file: None,
            settings_flag: None,
            settings: Vec::new(),
            args: Vec::new(),
            notes: Vec::new(),
        };
        match native.delivery {
            NativeDelivery::SettingsKey {
                file_name,
                flag,
                key,
                value,
            } => {
                application.settings_file = Some(file_name);
                application.settings_flag = Some(flag);
                application.settings.push((key, value.to_owned()));
            }
            NativeDelivery::Arguments(args) => {
                application
                    .args
                    .extend(args.iter().map(|arg| OsString::from(*arg)));
            }
        }
        match &additive {
            Some(injection) => {
                application.args.push(OsString::from(injection.flag));
                application.args.push(OsString::from(floor_directive()));
                application.notes.push(format!(
                    "the reports a response profile may never reduce are stated separately, \
                     through {}",
                    injection.mechanism
                ));
            }
            None => application.notes.push(format!(
                "{} declares no mechanism for adding an instruction beside its own system \
                 prompt, so the reports a response profile may never reduce rest on the \
                 harness's own coding instructions, which this mechanism preserves",
                id.display_name()
            )),
        }
        return application;
    }

    let Some(injection) = additive else {
        return Application::nothing(format!(
            "{} declares no native communication-style mechanism Glasshouse may use and no \
             mechanism for adding an instruction beside its own system prompt",
            id.display_name()
        ));
    };

    Application {
        mechanism: AppliedMechanism::Additive {
            mechanism: injection.mechanism,
            evidence: injection.evidence,
        },
        settings_file: None,
        settings_flag: None,
        settings: Vec::new(),
        args: vec![
            OsString::from(injection.flag),
            OsString::from(instruction_for(&profile)),
        ],
        notes: Vec::new(),
    }
}

/// The complete instruction an additive mechanism carries.
///
/// One function, so the text Glasshouse appends can only be built one way. It
/// always ends with [`floor_directive`], because
/// [`ResponseProfile::directives`] always does — see that method.
fn instruction_for(profile: &ResponseProfile) -> String {
    profile.instruction()
}

/// Whether `id` is a harness Glasshouse can apply a response profile to at
/// all, for a report that wants to say so before it has a profile in hand.
pub fn declares_any_mechanism(adapter: &dyn super::HarnessAdapter) -> bool {
    adapter.additive_response_injection().is_some()
        || crate::profile::response::presets()
            .iter()
            .any(|preset| adapter.native_response_style(&preset.profile).is_some())
}

/// The harness a report is describing, by name.
pub fn harness_name(id: IntegrationId) -> &'static str {
    id.display_name()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::response::{
        PrecedenceLayer, PrecedenceStack, ProfileLayer, REQUIRED_REPORTS, presets, resolve,
    };

    fn stack_for(preset_name: &str) -> PrecedenceStack {
        let mut stack = PrecedenceStack::empty();
        stack.set(
            PrecedenceLayer::UserDefault,
            ProfileLayer::from_preset(
                crate::profile::response::preset(preset_name).expect("a real preset"),
            ),
        );
        stack
    }

    #[test]
    fn an_unconfigured_profile_applies_nothing_to_any_harness() {
        let resolved = resolve(&PrecedenceStack::empty());
        for adapter in super::super::all() {
            let application = apply(adapter, &resolved);
            assert!(
                !application.mechanism().was_applied(),
                "{:?} was given a profile nobody asked for",
                adapter.id()
            );
            assert!(application.args().is_empty());
            assert!(application.settings().is_empty());
        }
    }

    #[test]
    fn no_instruction_glasshouse_writes_omits_the_floor() {
        // The invariant that matters most in this file. Whenever Glasshouse
        // writes instruction text of its own, that text carries the reports no
        // axis may reduce — whichever mechanism won, and whichever preset was
        // asked for.
        for preset in presets() {
            let resolved = resolve(&stack_for(preset.name));
            for adapter in super::super::all() {
                let application = apply(adapter, &resolved);
                let text = application.instruction_text();
                if text.is_empty() {
                    continue;
                }
                for required in REQUIRED_REPORTS {
                    assert!(
                        text.contains(required),
                        "{:?} under `{}` wrote instruction text without `{required}`:\n{text}",
                        adapter.id(),
                        preset.name
                    );
                }
            }
        }
    }

    #[test]
    fn the_applied_record_names_the_mechanism_that_actually_reached_the_child() {
        // §35's shape, one level down: a record saying `native` while nothing
        // native was delivered is the defect line 604 exists to prevent.
        for preset in presets() {
            let resolved = resolve(&stack_for(preset.name));
            for adapter in super::super::all() {
                let application = apply(adapter, &resolved);
                match application.mechanism() {
                    AppliedMechanism::Native { selection, .. } => {
                        let delivered = application
                            .settings()
                            .iter()
                            .any(|(_, value)| value == selection)
                            || application
                                .args()
                                .iter()
                                .any(|arg| arg.to_string_lossy() == *selection);
                        assert!(
                            delivered,
                            "{:?} recorded native `{selection}` and delivered none of it",
                            adapter.id()
                        );
                    }
                    AppliedMechanism::Additive { .. } => {
                        assert!(
                            !application.args().is_empty(),
                            "{:?} recorded an additive instruction and delivered nothing",
                            adapter.id()
                        );
                    }
                    AppliedMechanism::NotApplied { reason } => {
                        assert!(!reason.is_empty(), "a refusal must say why");
                        assert!(application.args().is_empty());
                        assert!(application.settings().is_empty());
                    }
                }
            }
        }
    }

    #[test]
    fn a_native_mechanism_is_preferred_over_an_additive_one() {
        // Claude Code declares both. Line 601 says the native one wins.
        let resolved = resolve(&stack_for("concise-technical"));
        let claude = super::super::adapter_for(IntegrationId::ClaudeCode).unwrap();
        assert!(
            claude.additive_response_injection().is_some(),
            "this test is vacuous unless the additive mechanism also exists"
        );
        let application = apply(claude, &resolved);
        assert_eq!(application.mechanism().category(), "native");
    }

    #[test]
    fn a_profile_no_native_style_represents_falls_through_to_the_additive_mechanism() {
        // `audit` asks for detailed narration and full evidence, which none of
        // Claude Code's communication-only built-in styles expresses. The
        // fall-through is what makes both branches of line 604 reachable.
        let resolved = resolve(&stack_for("audit"));
        let claude = super::super::adapter_for(IntegrationId::ClaudeCode).unwrap();
        let application = apply(claude, &resolved);
        assert_eq!(application.mechanism().category(), "additive");
        assert!(application.settings().is_empty());
    }

    #[test]
    fn the_launch_never_replaces_a_native_system_prompt() {
        // Line 607. `--system-prompt` replaces; `--append-system-prompt`
        // appends; Claude Code 2.1.247 documents both, and only one of them
        // may ever appear on a launch Glasshouse assembles.
        for preset in presets() {
            let resolved = resolve(&stack_for(preset.name));
            for adapter in super::super::all() {
                let application = apply(adapter, &resolved);
                for arg in application.args() {
                    let arg = arg.to_string_lossy();
                    assert_ne!(
                        arg,
                        "--system-prompt",
                        "{:?} would replace its own system prompt",
                        adapter.id()
                    );
                    assert_ne!(arg, "--system-prompt-file");
                }
            }
        }
    }

    #[test]
    fn an_adapter_that_declares_nothing_says_so_rather_than_inventing_a_mechanism() {
        // Six of seven harnesses have had no communication-style mechanism
        // read from them. The honest answer is a refusal that names the gap,
        // not a plausible flag.
        let resolved = resolve(&stack_for("concise-technical"));
        let mut refused = 0;
        for adapter in super::super::all() {
            if apply(adapter, &resolved).mechanism().was_applied() {
                continue;
            }
            refused += 1;
        }
        assert!(
            refused >= 5,
            "only {refused} adapters refused; a declaration was invented somewhere"
        );
    }
}
