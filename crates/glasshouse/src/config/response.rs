//! Phase 9K's configuration half: how a person records a response profile,
//! and what `glasshouse response` prints.
//!
//! [`mod@crate::profile::response`] is a pure domain model that imports no
//! configuration, and [`mod@crate::harness::response`] is the adapter-side
//! translation. This module is the caller both of those assume: it reads the
//! layered configuration, builds the six-layer
//! [`crate::profile::response::PrecedenceStack`] line 596
//! describes, asks [`crate::profile::response::resolve`], asks each
//! adapter what it would apply, and renders the answers.
//!
//! # Why the report lives here and not in `main.rs`
//!
//! The same reason [`mod@super::pairing`] gives, and it is §35: a caller only
//! the binary can reach is a caller no test enters through, and a capability
//! proven by tests that all set the world up themselves is proven against a
//! build whose production path could be deleted. [`report`] is what
//! `main.rs`'s `response` arm calls, in one line, and it is what
//! `tests/response_profiles.rs` calls too — so a mutation to the layering
//! below is a mutation to the path the shipped binary runs.
//!
//! # Project scope
//!
//! Line 597 requires a project's response-profile configuration to stay inside
//! that project. It does, structurally and for free:
//! [`ProjectConfig`](super::ProjectConfig) is read from
//! `<project root>/.glasshouse/config.toml` by
//! [`load_project_config`](super::load_project_config), which takes the
//! [`Project`](crate::Project) whose root Glasshouse resolved, and
//! [`EffectiveConfig`] holds exactly one of them. There is no path by which a
//! second project's file is opened, and
//! `a_projects_response_profile_does_not_reach_another_project` runs the
//! binary's own resolution in two project roots to show it.

use serde::{Deserialize, Serialize};

use crate::harness::response::{Application, apply};
use crate::integrations::IntegrationId;
use crate::profile::response::{
    AnswerFormat, Audience, Dimension, EvidenceDetail, Narration, PrecedenceLayer, PrecedenceStack,
    Preset, ProfileLayer, ResolvedProfile, Role, Verbosity, preset, preset_names, presets, resolve,
};
use crate::session::SessionRole;

use super::EffectiveConfig;

/// The five axes and an optional preset, as one configuration layer records
/// them.
///
/// Every field is optional and sets only what it names — the same rule
/// [`super::pairing::PairingModelOverride`] follows, and for the same reason:
/// a project that wants silent narration and nothing else should record one
/// key, not five.
///
/// A value this build does not understand is **ignored and reported**, never
/// refused and never silently turned into a default. That is the visible
/// degradation rule `RoutingConfig`'s stale free-resource pin already follows,
/// and it matters more here: a typo in `verbosity` that quietly became
/// `standard` would be a communication policy the user never chose and could
/// not see.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseProfileEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    verbosity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    audience: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    narration: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    evidence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    format: Option<String>,
}

impl ResponseProfileEntry {
    pub fn is_unset(&self) -> bool {
        self.preset.is_none()
            && self.verbosity.is_none()
            && self.audience.is_none()
            && self.narration.is_none()
            && self.evidence.is_none()
            && self.format.is_none()
    }

    pub fn preset(&self) -> Option<&str> {
        self.preset.as_deref()
    }

    pub fn set_preset(&mut self, name: Option<String>) -> &mut Self {
        self.preset = name;
        self
    }

    /// What this entry says about `dimension`, as it was written.
    pub fn axis(&self, dimension: Dimension) -> Option<&str> {
        match dimension {
            Dimension::Verbosity => self.verbosity.as_deref(),
            Dimension::Audience => self.audience.as_deref(),
            Dimension::Narration => self.narration.as_deref(),
            Dimension::Evidence => self.evidence.as_deref(),
            Dimension::Format => self.format.as_deref(),
        }
    }

    pub fn set_axis(&mut self, dimension: Dimension, value: Option<String>) -> &mut Self {
        match dimension {
            Dimension::Verbosity => self.verbosity = value,
            Dimension::Audience => self.audience = value,
            Dimension::Narration => self.narration = value,
            Dimension::Evidence => self.evidence = value,
            Dimension::Format => self.format = value,
        }
        self
    }

    /// This entry as a [`ProfileLayer`], with anything unreadable collected
    /// into `problems` rather than dropped.
    pub fn to_layer(&self, where_from: &str, problems: &mut Vec<String>) -> ProfileLayer {
        let mut layer = ProfileLayer::empty();

        if let Some(name) = &self.preset {
            match preset(name) {
                Some(found) => layer.preset = Some(found),
                None => problems.push(format!(
                    "{where_from} names preset `{name}`, which this build does not know; the \
                     presets are: {}",
                    preset_names()
                )),
            }
        }

        macro_rules! axis {
            ($field:ident, $ty:ident, $dimension:expr) => {
                if let Some(value) = &self.$field {
                    match $ty::from_slug(value) {
                        Some(parsed) => layer.$field = Some(parsed),
                        None => problems.push(format!(
                            "{where_from} sets {} to `{value}`, which this build does not know; \
                             the values are: {}",
                            $dimension,
                            $ty::ALL
                                .iter()
                                .map(|value| value.slug())
                                .collect::<Vec<_>>()
                                .join(", ")
                        )),
                    }
                }
            };
        }

        axis!(verbosity, Verbosity, Dimension::Verbosity);
        axis!(audience, Audience, Dimension::Audience);
        axis!(narration, Narration, Dimension::Narration);
        axis!(evidence, EvidenceDetail, Dimension::Evidence);
        axis!(format, AnswerFormat, Dimension::Format);

        layer
    }
}

/// The `[response]` table of one configuration layer.
///
/// `default` is that layer's own answer for every role; `roles` overrides it
/// per role — line 595. Both are optional, and a configuration file that was
/// never asked about response profiles carries no `[response]` table at all,
/// the same rule [`super::pairing::PairingConfig::is_unset`] follows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseConfig {
    #[serde(
        default,
        flatten,
        skip_serializing_if = "ResponseProfileEntry::is_unset"
    )]
    default: ResponseProfileEntry,
    #[serde(
        default,
        skip_serializing_if = "std::collections::BTreeMap::is_empty",
        rename = "roles"
    )]
    roles: std::collections::BTreeMap<String, ResponseProfileEntry>,
}

impl ResponseConfig {
    pub fn is_unset(&self) -> bool {
        self.default.is_unset() && self.roles.values().all(ResponseProfileEntry::is_unset)
    }

    pub fn default_entry(&self) -> &ResponseProfileEntry {
        &self.default
    }

    pub fn default_entry_mut(&mut self) -> &mut ResponseProfileEntry {
        &mut self.default
    }

    pub fn role(&self, role: Role) -> Option<&ResponseProfileEntry> {
        self.roles.get(role.slug())
    }

    pub fn role_entry(&mut self, role: Role) -> &mut ResponseProfileEntry {
        self.roles.entry(role.slug().to_owned()).or_default()
    }

    /// Role keys this build does not know, so they can be reported rather
    /// than silently ignored.
    pub fn unknown_roles(&self) -> Vec<&str> {
        self.roles
            .keys()
            .map(String::as_str)
            .filter(|slug| Role::from_slug(slug).is_none())
            .collect()
    }
}

/// The task-override and session layers, which come from the command line
/// rather than from a file.
///
/// A separate type from [`ResponseProfileEntry`] because these are not
/// configuration: nothing writes them to a file, and a layer whose only home
/// is one invocation should not be `Serialize`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResponseRequest {
    /// Which role this session is being opened in. `None` resolves to
    /// [`Role::Interactive`], which is line 595's *"ordinary
    /// interactive-session"*.
    pub role: Option<Role>,
    /// A preset named for one session — [`PrecedenceLayer::Session`].
    pub session_preset: Option<String>,
    /// Axes overridden for one task — [`PrecedenceLayer::TaskOverride`].
    pub task: ResponseProfileEntry,
}

impl ResponseRequest {
    /// The response-profile role a stored [`SessionRole`] implies.
    ///
    /// Two enumerations rather than one, on purpose. [`SessionRole`] is what a
    /// session **is** — a persisted tag on a row, with three values a schema
    /// `CHECK` constrains — and [`Role`] is which communication default
    /// applies, with the five values line 595 names. Making them one type
    /// would mean either a migration every time the map named a new
    /// communication role, or a communication vocabulary bounded by what a
    /// database column happens to allow.
    ///
    /// The mapping is total and deliberately loses nothing: a stored session
    /// has no reviewer or explainer tag today, so those two are reachable only
    /// from `--role`, and a `Normal` session is an ordinary interactive one.
    pub fn role_for(session: SessionRole) -> Role {
        match session {
            SessionRole::Orchestrator => Role::Orchestrator,
            SessionRole::Worker => Role::Worker,
            SessionRole::Normal => Role::Interactive,
        }
    }
}

/// One resolved response profile, everything it was resolved from, and what
/// each harness would do with it.
#[derive(Debug, Clone)]
pub struct ResponseResolution {
    role: Role,
    resolved: ResolvedProfile,
    problems: Vec<String>,
}

impl ResponseResolution {
    pub fn role(&self) -> Role {
        self.role
    }

    pub fn resolved(&self) -> &ResolvedProfile {
        &self.resolved
    }

    /// Configuration values this build could not read. Reported, never
    /// silently dropped.
    pub fn problems(&self) -> &[String] {
        &self.problems
    }

    /// What `harness` would actually apply — line 604.
    pub fn application(&self, harness: IntegrationId) -> Option<Application> {
        let adapter = crate::harness::adapter_for(harness)?;
        Some(apply(adapter, &self.resolved))
    }
}

impl EffectiveConfig<'_> {
    /// Line 596's six layers, filled in from configuration and `request`.
    ///
    /// The **only** place the stack is built, so `glasshouse response` and
    /// `glasshouse launch` cannot resolve differently. Six `set` calls, one
    /// per layer, and deleting any one of them is a mutation a test kills.
    ///
    /// [`PrecedenceLayer::HarnessDefault`] is deliberately left empty: the
    /// bottom of the chain is *the harness untouched*, and
    /// [`resolve`] reports an axis that
    /// reached it as having come from there. An unconfigured Glasshouse
    /// therefore applies nothing at all — see
    /// [`ResolvedProfile::is_harness_default`].
    pub fn response_stack(
        &self,
        request: &ResponseRequest,
    ) -> (PrecedenceStack, Role, Vec<String>) {
        let mut problems: Vec<String> = Vec::new();
        let role = request.role.unwrap_or(Role::Interactive);
        let mut stack = PrecedenceStack::empty();

        // 1. Task override — the axes named on this one invocation.
        stack.set(
            PrecedenceLayer::TaskOverride,
            request.task.to_layer("the task override", &mut problems),
        );

        // 2. Session — a preset named for this session.
        let session = match &request.session_preset {
            Some(name) => match preset(name) {
                Some(found) => ProfileLayer::from_preset(found),
                None => {
                    problems.push(format!(
                        "the session asked for preset `{name}`, which this build does not know; \
                         the presets are: {}",
                        preset_names()
                    ));
                    ProfileLayer::empty()
                }
            },
            None => ProfileLayer::empty(),
        };
        stack.set(PrecedenceLayer::Session, session);

        // 3. Role — line 595, and the layer whose exact conditions decide
        //    whether the three layers below it are reachable at all.
        //
        //    A configured `[response.roles.<role>]` table always applies to
        //    the role it names, project over user. The role's *built-in*
        //    default applies only when a person named the role, and that
        //    condition is load-bearing: a role layer that always supplied all
        //    five axes from the built-in table would make
        //    `PrecedenceLayer::Project`, `UserDefault` and `HarnessDefault`
        //    unreachable in the shipped binary — line 596's chain would still
        //    be there and its bottom half could never win. Asking for
        //    `--response-role worker` is a request for that role's defaults;
        //    not asking is not a request for anything.
        let role_entry = self
            .project
            .and_then(|project| project.response().role(role))
            .filter(|entry| !entry.is_unset())
            .map(|entry| (entry, "this project's configuration"))
            .or_else(|| {
                self.user
                    .response()
                    .role(role)
                    .filter(|entry| !entry.is_unset())
                    .map(|entry| (entry, "the user configuration"))
            });
        let role_layer = match (role_entry, request.role) {
            (Some((entry, where_from)), _) => {
                entry.to_layer(&format!("{where_from}'s `{role}` role"), &mut problems)
            }
            (None, Some(_)) => ProfileLayer::from_preset(
                preset(role.default_preset()).expect("a role's default names a real preset"),
            ),
            (None, None) => ProfileLayer::empty(),
        };
        stack.set(PrecedenceLayer::Role, role_layer);

        // 4. Project — this project's own `[response]` table, and no other
        //    project's. See the module documentation.
        let project_layer = match self.project {
            Some(project) if !project.response().default_entry().is_unset() => project
                .response()
                .default_entry()
                .to_layer("this project's configuration", &mut problems),
            _ => ProfileLayer::empty(),
        };
        stack.set(PrecedenceLayer::Project, project_layer);

        // 5. User default.
        stack.set(
            PrecedenceLayer::UserDefault,
            self.user
                .response()
                .default_entry()
                .to_layer("the user configuration", &mut problems),
        );

        // 6. Harness default — see this method's doc comment. Nothing to set.

        for unknown in self.user.response().unknown_roles().into_iter().chain(
            self.project
                .map(|project| project.response().unknown_roles())
                .unwrap_or_default(),
        ) {
            problems.push(format!(
                "a `[response.roles.{unknown}]` table names a role this build does not know; the \
                 roles are: {}",
                Role::names()
            ));
        }

        (stack, role, problems)
    }

    /// The resolved response profile for `request`.
    ///
    /// **The function that asks the policy.** `glasshouse response` calls it
    /// to print an answer and `main.rs`'s launch path calls it to apply one,
    /// so a resolved profile is never something only a report has seen.
    pub fn response_profile(&self, request: &ResponseRequest) -> ResponseResolution {
        let (stack, role, problems) = self.response_stack(request);
        ResponseResolution {
            role,
            resolved: resolve(&stack),
            problems,
        }
    }

    /// Every configured backend that declares a prompt transformation, with
    /// the layer it was configured in — line 609.
    fn prompt_transformations(&self) -> Vec<(String, String, &'static str)> {
        let mut found: Vec<(String, String, &'static str)> = Vec::new();
        for (name, provider) in self.user.providers().iter() {
            if let Some(transform) = provider.prompt_transform() {
                found.push((
                    name.to_owned(),
                    transform.to_owned(),
                    "the user configuration",
                ));
            }
        }
        if let Some(project) = self.project {
            for (name, provider) in project.providers().iter() {
                if let Some(transform) = provider.prompt_transform() {
                    found.retain(|(existing, _, _)| existing != name);
                    found.push((
                        name.to_owned(),
                        transform.to_owned(),
                        "this project's configuration",
                    ));
                }
            }
        }
        found.sort_by(|a, b| a.0.cmp(&b.0));
        found
    }
}

/// What `glasshouse response` prints.
///
/// The production caller of [`crate::profile::response::resolve`] and of
/// [`crate::harness::response::apply`], and the function `main.rs`'s
/// `response` arm calls.
pub fn report(effective: &EffectiveConfig<'_>, request: &ResponseRequest) -> String {
    use std::fmt::Write as _;

    let resolution = effective.response_profile(request);
    let mut out = String::new();

    let _ = writeln!(out, "Glasshouse response profile");
    let _ = writeln!(out, "===========================");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "A response profile governs how an answer reads, and nothing else. It cannot change \
         reasoning\ndepth, diligence, validation, permissions, safety or tool use, and it may \
         never use concision\nto reduce what is reported."
    );
    let _ = writeln!(out);

    if !resolution.problems().is_empty() {
        let _ = writeln!(out, "Configuration Glasshouse could not read");
        for problem in resolution.problems() {
            let _ = writeln!(out, "  {problem}");
        }
        let _ = writeln!(
            out,
            "  Each of these was ignored rather than guessed at, so nothing above was resolved \
             from it."
        );
        let _ = writeln!(out);
    }

    let _ = writeln!(out, "Resolved profile — role `{}`", resolution.role());
    for axis in resolution.resolved().axes() {
        let _ = writeln!(
            out,
            "  {:<10} {:<14} from the {}",
            axis.dimension.slug(),
            axis.value,
            axis.source
        );
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "Precedence, highest first");
    for layer in PrecedenceLayer::ALL {
        let claimed: Vec<&str> = resolution
            .resolved()
            .axes()
            .iter()
            .filter(|axis| axis.source == layer)
            .map(|axis| axis.dimension.slug())
            .collect();
        let _ = writeln!(
            out,
            "  {:<16} {}",
            layer.slug(),
            if claimed.is_empty() {
                "—".to_owned()
            } else {
                claimed.join(", ")
            }
        );
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "Always reported, whatever the axes say");
    let _ = writeln!(
        out,
        "  {}",
        resolution
            .resolved()
            .profile()
            .required_reports()
            .join(", ")
    );
    let _ = writeln!(
        out,
        "  Concision governs presentation only. No profile can reduce analysis, verification,\n  \
         diagnostics, error reporting or checkpoint completeness."
    );
    let _ = writeln!(out);

    let _ = writeln!(out, "What each harness would apply");
    for adapter in crate::harness::all() {
        let application = apply(adapter, resolution.resolved());
        let _ = writeln!(
            out,
            "  {} — {}",
            adapter.id().display_name(),
            application.mechanism().category()
        );
        let _ = writeln!(out, "      {}", application.mechanism().describe());
        if let Some(evidence) = application.mechanism().evidence() {
            let _ = writeln!(out, "      evidence: {evidence}");
        }
        for note in application.notes() {
            let _ = writeln!(out, "      note: {note}");
        }
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "Presets");
    for preset in presets() {
        let _ = writeln!(out, "  {:<18} {}", preset.name, preset.description);
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "Role defaults");
    for role in Role::ALL {
        let _ = writeln!(out, "  {:<14} {}", role.slug(), role.default_preset());
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "Backend prompt transformations");
    let transformations = effective.prompt_transformations();
    if transformations.is_empty() {
        let _ = writeln!(
            out,
            "  (none configured) — Glasshouse never rewrites a prompt at the gateway to apply a \
             response\n  profile. Record one with `prompt_transform` on a provider if your own \
             gateway does."
        );
    } else {
        for (name, transform, layer) in transformations {
            let _ = writeln!(out, "  provider `{name}` ({layer}): {transform}");
            let _ = writeln!(
                out,
                "      This is something your backend does, not something Glasshouse does. It \
                 may\n      interact with the harness's own instructions and with the profile \
                 above."
            );
        }
    }

    out
}

/// The five axes of a resolved profile as one line, for a launch log or a
/// session detail.
pub fn one_line(resolution: &ResponseResolution) -> String {
    let axes = resolution
        .resolved()
        .axes()
        .iter()
        .map(|axis| format!("{}={}", axis.dimension.slug(), axis.value))
        .collect::<Vec<_>>()
        .join(" ");
    format!("role={} {axes}", resolution.role())
}

/// Every preset, for a CLI that wants to list them.
pub fn known_presets() -> &'static [Preset] {
    presets()
}
