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
    /// Whether automatic response-profile injection may apply the
    /// configured `default`/`roles` layers at all — independent of automatic
    /// routing ([`super::RoutingConfig::model`]) and automatic memory
    /// extraction ([`super::UserConfig::memory_extraction`]). `None` means
    /// "never decided" and resolves to enabled, the same reasoning
    /// [`super::RoutingConfig::model`] documents.
    ///
    /// This gates only the file-configured `Role`, `Project` and
    /// `UserDefault` layers of [`EffectiveConfig::response_stack`] — an
    /// explicit `--response-preset`/`--response-role` on one invocation is a
    /// request made *now*, not automatic injection, and still applies. See
    /// `tests::disabling_injection_suppresses_configured_layers_but_not_an_explicit_request`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    enabled: Option<bool>,
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
        self.enabled.is_none()
            && self.default.is_unset()
            && self.roles.values().all(ResponseProfileEntry::is_unset)
    }

    /// This layer's recorded decision on automatic response-profile
    /// injection, or `None` for "never decided". See the field's own doc.
    pub fn enabled(&self) -> Option<bool> {
        self.enabled
    }

    pub fn set_enabled(&mut self, enabled: Option<bool>) -> &mut Self {
        self.enabled = enabled;
        self
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
    /// Whether automatic response-profile injection may apply the
    /// configured layers, reporting which layer decided it. Project first,
    /// then user, then [`super::Layer::Default`] (enabled), matching every
    /// ordinary lookup on this type. See [`ResponseConfig::enabled`] for
    /// exactly what this does and does not suppress.
    pub fn response_injection_enabled(&self) -> super::Layered<bool> {
        if let Some(value) = self.project.and_then(|p| p.response().enabled()) {
            return super::Layered::new(value, super::Layer::Project);
        }
        if let Some(value) = self.user.response().enabled() {
            return super::Layered::new(value, super::Layer::User);
        }
        super::Layered::new(true, super::Layer::Default)
    }

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
        let injection_enabled = self.response_injection_enabled().value;

        let role_entry = injection_enabled
            .then(|| {
                self.project
                    .and_then(|project| project.response().role(role))
                    .filter(|entry| !entry.is_unset())
                    .map(|entry| (entry, "this project's configuration"))
                    .or_else(|| {
                        self.user
                            .response()
                            .role(role)
                            .filter(|entry| !entry.is_unset())
                            .map(|entry| (entry, "the user configuration"))
                    })
            })
            .flatten();
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
            Some(project)
                if injection_enabled && !project.response().default_entry().is_unset() =>
            {
                project
                    .response()
                    .default_entry()
                    .to_layer("this project's configuration", &mut problems)
            }
            _ => ProfileLayer::empty(),
        };
        stack.set(PrecedenceLayer::Project, project_layer);

        // 5. User default.
        stack.set(
            PrecedenceLayer::UserDefault,
            if injection_enabled {
                self.user
                    .response()
                    .default_entry()
                    .to_layer("the user configuration", &mut problems)
            } else {
                ProfileLayer::empty()
            },
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Layer, Layered, ProjectConfig, UserConfig};
    use crate::profile::response::PrecedenceLayer;

    #[test]
    fn enabled_key_round_trips_and_absence_parses_to_never_decided() {
        let mut user = UserConfig::default();
        assert_eq!(user.response().enabled(), None);
        user.response_mut().set_enabled(Some(false));

        let text = toml::to_string_pretty(&user).unwrap();
        assert!(
            text.contains("enabled = false"),
            "an explicit disable must actually be written:\n{text}"
        );
        let loaded: UserConfig = toml::from_str(&text).unwrap();
        assert_eq!(loaded.response().enabled(), Some(false));

        // A file written before this field existed has no `enabled` key at
        // all, and must load as "never decided" — never as "configured off".
        let old_file = "version = 1\n";
        let from_old_file: UserConfig = toml::from_str(old_file).unwrap();
        assert_eq!(from_old_file.response().enabled(), None);
    }

    #[test]
    fn injection_enabled_layers_project_over_user_over_default() {
        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        assert_eq!(
            effective.response_injection_enabled(),
            Layered::new(true, Layer::Default),
            "nothing recorded anywhere must resolve to enabled"
        );

        let mut user = UserConfig::default();
        user.response_mut().set_enabled(Some(false));
        let effective = EffectiveConfig::new(&user, None);
        assert_eq!(
            effective.response_injection_enabled(),
            Layered::new(false, Layer::User)
        );

        let mut project = ProjectConfig::default();
        project.response_mut().set_enabled(Some(true));
        let effective = EffectiveConfig::new(&user, Some(&project));
        assert_eq!(
            effective.response_injection_enabled(),
            Layered::new(true, Layer::Project),
            "a project's explicit re-enable must win over the user's disable"
        );
    }

    /// The disable trio's response half: disabling injection must not move
    /// [`super::super::RoutingConfig::model`] or
    /// [`super::super::UserConfig::memory_extraction`], and setting either of
    /// those must not move this. Each reads its own field.
    #[test]
    fn the_three_automatic_behaviours_disable_independently() {
        use crate::config::RoutingModelChoice;

        for (routing_off, memory_off, response_off) in [
            (false, false, false),
            (true, false, false),
            (false, true, false),
            (false, false, true),
            (true, true, true),
        ] {
            let mut user = UserConfig::default();
            if routing_off {
                user.routing_mut()
                    .set_model(Some(RoutingModelChoice::Deterministic));
            }
            user.set_memory_extraction(Some(!memory_off));
            user.response_mut().set_enabled(Some(!response_off));

            let effective = EffectiveConfig::new(&user, None);

            assert_eq!(
                matches!(
                    effective.routing_model_resolution().value,
                    crate::config::RoutingModelResolution::Heuristics(
                        crate::config::RoutingFallback::DeterministicChosen
                    )
                ),
                routing_off,
                "routing state must depend only on the routing field, case {routing_off} {memory_off} {response_off}"
            );
            assert_eq!(
                effective.memory_extraction_enabled().value,
                !memory_off,
                "memory-extraction state must depend only on its own field, case {routing_off} {memory_off} {response_off}"
            );
            assert_eq!(
                effective.response_injection_enabled().value,
                !response_off,
                "response-injection state must depend only on its own field, case {routing_off} {memory_off} {response_off}"
            );
        }
    }

    /// Disabling injection suppresses the file-configured `Role`, `Project`
    /// and `UserDefault` layers, but an explicit request made on this one
    /// invocation — a task override, a session preset, or naming a role on
    /// the command line — is not automatic injection and still applies.
    #[test]
    fn disabling_injection_suppresses_configured_layers_but_not_an_explicit_request() {
        let mut user = UserConfig::default();
        user.response_mut()
            .default_entry_mut()
            .set_preset(Some("audit".to_owned()));
        user.response_mut().set_enabled(Some(false));
        let effective = EffectiveConfig::new(&user, None);

        // With nothing requested, a disabled injection applies nothing: every
        // axis reads from `HarnessDefault`.
        let resolution = effective.response_profile(&ResponseRequest::default());
        assert!(
            resolution.resolved().is_harness_default(),
            "a disabled injection with no explicit request must apply nothing"
        );

        // An explicit task-level preset still applies even while injection is
        // disabled — this is a request made now, not automatic injection.
        let request = ResponseRequest {
            session_preset: Some("concise-technical".to_owned()),
            ..ResponseRequest::default()
        };
        let resolution = effective.response_profile(&request);
        assert!(
            !resolution.resolved().is_harness_default(),
            "an explicit session preset must still apply while injection is disabled"
        );
        assert!(
            resolution
                .resolved()
                .axes()
                .iter()
                .all(|axis| axis.source == PrecedenceLayer::Session),
            "every axis must come from the explicit session preset, not a suppressed layer"
        );

        // The same suppression applies to a project's own `[response]`
        // table, not only the user's — the disable is resolved once and
        // gates every file-configured layer, not just `UserDefault`.
        let mut project = ProjectConfig::default();
        project
            .response_mut()
            .default_entry_mut()
            .set_preset(Some("brief".to_owned()));
        let effective = EffectiveConfig::new(&user, Some(&project));
        let resolution = effective.response_profile(&ResponseRequest::default());
        assert!(
            resolution.resolved().is_harness_default(),
            "a disabled injection must also suppress a configured project layer"
        );

        // Re-enabling injection restores the configured user default.
        let mut enabled_user = user.clone();
        enabled_user.response_mut().set_enabled(Some(true));
        let effective = EffectiveConfig::new(&enabled_user, None);
        let resolution = effective.response_profile(&ResponseRequest::default());
        assert!(
            !resolution.resolved().is_harness_default(),
            "re-enabling injection must restore the configured user default"
        );
    }
}
