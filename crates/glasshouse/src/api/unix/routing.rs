use glasshouse::Runtime;
use glasshouse::config::{self, EffectiveConfig, UserConfig};

use crate::api::protocol::Response;

/// The hard ceiling on how many ranked alternatives and how many rejected
/// candidates [`Request::RecommendRoute`] returns, regardless of the
/// `alternatives` a caller asks for — ruling 4 of that verb's packet, and
/// the same shape as [`MAX_MEMORY_LIMIT`] above.
///
/// `Routed::considered` holds *every* eligible destination, and a project's
/// candidate set grows with its launch profiles and its session history, so
/// without this the response size would depend on how much the project has
/// accumulated. Generous against the default of five and far short of any
/// real project's whole field: a caller that wants a specific candidate's
/// score narrows the question, which is what naming a task is for.
const MAX_ROUTE_ALTERNATIVES: usize = 20;

/// Current resource capacity and quota telemetry — capability map line 1679.
///
/// Mirrors `main.rs`'s own `resources_report` for its non-probe path: reads
/// the user's configuration, folds in the persisted gateway-quota and
/// gateway-health caches [`crate::api`]'s door doc comment already promises
/// this project shares with every other process, and asks each installed
/// harness for its own status the same cheap, no-quota way `glasshouse
/// resources` does with no flags. Never makes a network request — this
/// request carries no provider name to probe, unlike the CLI's own `--probe`.
pub(super) fn resource_capacity(runtime: &Runtime) -> Response {
    let user = match UserConfig::load(runtime.paths()) {
        Ok(user) => user,
        Err(err) => return Response::err(err),
    };
    let project_config = match config::load_project_config(runtime.project()) {
        Ok(project_config) => project_config,
        Err(err) => return Response::err(err),
    };
    let effective = EffectiveConfig::new(&user, project_config.as_ref());
    let now_unix = glasshouse::provider::cache::now_unix_seconds();

    let telemetry = glasshouse::provider::resources::GatheredTelemetry::new()
        .gather_gateway_quota(&glasshouse::provider::telemetry::GatewayQuotaCache::new(
            runtime.paths(),
        ))
        .gather_gateway_health(&glasshouse::provider::telemetry::GatewayHealthCache::new(
            runtime.paths(),
        ))
        .gather_harness_status(now_unix);

    Response::ok(glasshouse::provider::resources::capacity_json(
        &effective, &telemetry, now_unix,
    ))
}

/// Current routing-model selection and its health — capability map line 1680.
///
/// `selection` is the recorded [`config::RoutingModelChoice`] together with
/// the layer it came from, reported the way every other layered value in
/// this project is reported (see [`describe_layer`]). `resolution` is what
/// will actually classify a request right now:
/// `EffectiveConfig::routing_model_resolution` already checks a pinned
/// choice against the providers configured this instant and degrades to
/// heuristics with a named [`config::RoutingFallback`] when one has gone
/// missing — this handler reports that computed state, keyed by the type's
/// own variant names, rather than re-deriving or re-wording it into prose of
/// its own. There is no live latency or health probe anywhere in this
/// project (see that function's own doc comment); a project that has
/// configured nothing gets [`config::RoutingFallback::NotConfigured`], the
/// honest default, never a fabricated pin.
pub(super) fn routing_model_status(runtime: &Runtime) -> Response {
    let user = match UserConfig::load(runtime.paths()) {
        Ok(user) => user,
        Err(err) => return Response::err(err),
    };
    let project_config = match config::load_project_config(runtime.project()) {
        Ok(project_config) => project_config,
        Err(err) => return Response::err(err),
    };
    let effective = EffectiveConfig::new(&user, project_config.as_ref());

    let selection = effective.routing_model();
    let resolution = effective.routing_model_resolution();

    Response::ok(serde_json::json!({
        "selection": routing_choice_json(&selection.value),
        "layer": describe_layer(resolution.layer),
        "resolution": routing_resolution_json(&resolution.value),
    }))
}

/// A recorded [`config::RoutingModelChoice`] as JSON. `provider`/`model` are
/// `null` for every choice but [`config::RoutingModelChoice::Pinned`] —
/// never an empty string, so an absent value cannot be mistaken for one that
/// was measured and happened to be empty (§71).
fn routing_choice_json(choice: &config::RoutingModelChoice) -> serde_json::Value {
    match choice {
        config::RoutingModelChoice::Deterministic => serde_json::json!({
            "choice": "deterministic",
            "provider": null,
            "model": null,
        }),
        config::RoutingModelChoice::Automatic => serde_json::json!({
            "choice": "automatic",
            "provider": null,
            "model": null,
        }),
        config::RoutingModelChoice::Pinned { provider, model } => serde_json::json!({
            "choice": "pinned",
            "provider": provider,
            "model": model,
        }),
    }
}

/// A computed [`config::RoutingModelResolution`] as JSON — what will
/// actually classify a request right now, distinct from the recorded
/// [`routing_choice_json`].
fn routing_resolution_json(resolution: &config::RoutingModelResolution) -> serde_json::Value {
    match resolution {
        config::RoutingModelResolution::Automatic => serde_json::json!({ "state": "automatic" }),
        config::RoutingModelResolution::Pinned { provider, model } => serde_json::json!({
            "state": "pinned",
            "provider": provider,
            "model": model,
        }),
        config::RoutingModelResolution::Heuristics(reason) => routing_fallback_json(reason),
    }
}

/// Why deterministic heuristics are answering instead of a model, keyed by
/// [`config::RoutingFallback`]'s own variant names rather than its
/// [`std::fmt::Display`] prose — a client matching on `reason` must be able
/// to tell the cases apart mechanically, not by parsing a sentence meant for
/// a person.
fn routing_fallback_json(reason: &config::RoutingFallback) -> serde_json::Value {
    match reason {
        config::RoutingFallback::NotConfigured => serde_json::json!({
            "state": "heuristics",
            "reason": "not_configured",
        }),
        config::RoutingFallback::DeterministicChosen => serde_json::json!({
            "state": "heuristics",
            "reason": "deterministic_chosen",
        }),
        config::RoutingFallback::ProviderNotConfigured { provider, model } => serde_json::json!({
            "state": "heuristics",
            "reason": "provider_not_configured",
            "provider": provider,
            "model": model,
        }),
    }
}

/// Where this project's work would be routed, and why — capability map line
/// 1681.
///
/// One ranking, not two: the decision is
/// `crate::commands::route::route_recommendation`, which is the whole of
/// `glasshouse route` as well. This handler classifies, scores and orders
/// nothing; it turns the answer into JSON — if the command and the door
/// could rank separately they could disagree about where work should go.
/// Without executing it, enforced rather than intended: nothing on this
/// path writes, takes the [`SessionRuntime`] lock, touches `SessionApi`,
/// records an event, or opens the evidence ledger. `tests/routing_api.rs`
/// asserts the negative over the shipped binary: the session list, the
/// event log and `routing_observations` are all unchanged across a call,
/// and the configured harness is never invoked.
/// `alternatives` is capped at [`MAX_ROUTE_ALTERNATIVES`] here rather than
/// left to the caller (a `min`, not a rejection); everything after a
/// malformed-config error is refused with a fixed sentence, because
/// `routing_destinations` opens the project's database and every
/// `database::DatabaseError` variant names that file's absolute path.
// History: design-decisions.md, "Trims: api, events, harness and config module docs, second packet", crates/glasshouse/src/api/unix/routing.rs `recommend_route`.
pub(super) fn recommend_route(
    runtime: &Runtime,
    task: Option<&str>,
    moment: &str,
    alternatives: usize,
) -> Response {
    let Some(moment) = crate::commands::route::routing_moment_from_str(moment) else {
        // The caller's own spelling is deliberately not quoted back: this
        // string arrived over a socket, and naming the three that exist is
        // the whole of what a client can act on.
        return Response::err(
            "that is not a routing moment; use `session-start`, `task-boundary` or `mid-turn`",
        );
    };

    let user = match UserConfig::load(runtime.paths()) {
        Ok(user) => user,
        Err(err) => return Response::err(err),
    };
    let project_config = match config::load_project_config(runtime.project()) {
        Ok(project_config) => project_config,
        Err(err) => return Response::err(err),
    };
    let effective = EffectiveConfig::new(&user, project_config.as_ref());

    // `None`/`false`/`false` are the three override arguments this verb does
    // not take — see [`Request::RecommendRoute`]'s own doc comment for why
    // asking a router a question and telling it an answer are different
    // requests. Structurally, that also makes `Routed::overrode` and
    // `Routed::override_refused` always `None` here, which is why neither
    // appears in the response.
    let recommendation = match crate::commands::route::route_recommendation(
        runtime, &effective, moment, None, false, false, task,
    ) {
        Ok(recommendation) => recommendation,
        Err(_) => return Response::err("this project's routing inputs could not be read"),
    };

    let bound = alternatives.min(MAX_ROUTE_ALTERNATIVES);
    match &recommendation {
        crate::commands::route::RouteRecommendation::Nowhere(reason) => {
            Response::ok(serde_json::json!({
                "routed": false,
                "moment": crate::commands::route::routing_moment_slug(moment),
                "reason": no_route_reason(reason),
                "report": crate::commands::route::render_route_recommendation(&recommendation),
            }))
        }
        crate::commands::route::RouteRecommendation::Ranked(ranked) => {
            let routed = &ranked.routed;
            // `considered` is best-first and its head is what the *ranking*
            // chose, which is `destination` itself — this verb takes no
            // override, so the two cannot come apart here. Skipping index 0
            // is `Routed::render_overview`'s own rule, kept identical so the
            // door and `glasshouse route` cannot list different runners-up.
            let alternatives = routed.considered().len().saturating_sub(1);
            let rejected = routed.rejected().len();
            Response::ok(serde_json::json!({
                "routed": true,
                // The wire spelling a caller sent, not `RoutingMoment`'s
                // own prose — see `crate::commands::route::routing_moment_slug`.
                "moment": crate::commands::route::routing_moment_slug(routed.moment()),
                // `false` is line 1592's boundary gate holding the work where
                // it is rather than a ranking having been taken — the same
                // distinction `Routed::render` prints in words.
                "re_decided": routed.re_decided(),
                "destination": route_destination_json(routed.chosen()),
                "score": routed.explanation().total(),
                "contributions": contributions_json(routed.explanation()),
                "alternatives": routed
                    .considered()
                    .iter()
                    .skip(1)
                    .take(bound)
                    .map(|(destination, explanation)| serde_json::json!({
                        "destination": route_destination_json(destination),
                        "score": explanation.total(),
                        "contributions": contributions_json(explanation),
                    }))
                    .collect::<Vec<_>>(),
                // Never silently dropped: a bounded listing that does not say
                // what it left out reads as a complete one.
                "alternatives_omitted": alternatives.saturating_sub(bound),
                "rejected": routed
                    .rejected()
                    .iter()
                    .take(bound)
                    .map(|(destination, constraint)| serde_json::json!({
                        "destination": route_destination_json(destination),
                        "constraint": constraint.as_str(),
                    }))
                    .collect::<Vec<_>>(),
                "rejected_omitted": rejected.saturating_sub(bound),
                // `Routed::render`, not `render_overview`: the overview's
                // alternatives block is as long as the candidate set, and a
                // response this handler bounds must not carry an unbounded
                // rendering of the same thing beside the bounded one. The
                // runners-up are above, structured and capped.
                "report": routed.render(),
                // What the ranking could not see, in `glasshouse route`'s own
                // words. This is part of the explanation rather than
                // decoration: a caller that cannot tell "provider health was
                // equal" from "provider health was never read" has been given
                // a number it will misread. Bounded by construction — at most
                // five lines, whatever the candidate set holds — which is why
                // it can travel beside a capped listing.
                "caveats": crate::commands::route::routing_caveats(
                    routed,
                    &ranked.destinations,
                    &ranked.refused_by_launch,
                    &ranked.health_observed,
                ),
            }))
        }
    }
}

/// Which of [`crate::commands::route::NoRoute`]'s two situations applies, keyed mechanically
/// rather than by its rendered sentence — the same reason
/// [`routing_fallback_json`] keys on variant names: a client telling the
/// cases apart must not have to parse prose written for a person.
fn no_route_reason(reason: &crate::commands::route::NoRoute) -> &'static str {
    match reason {
        crate::commands::route::NoRoute::NoDestination => "no_destination",
        crate::commands::route::NoRoute::MomentDoesNotRoute(_) => "moment_does_not_route",
    }
}

/// One routing candidate as JSON — enough to name it and to act on it, and
/// no more.
///
/// `id` is what a caller would pass to `glasshouse route --to`, and
/// `launch_profile` is a profile name. **No credential appears here**, not
/// even as a name: `Backend::credential` is a `CredentialId`, and while that
/// type carries only a variable name, a routing recommendation has no need
/// of it and the safest field is the one that is not on the wire. Nothing is
/// `Debug`-formatted, and no path of any kind is reachable from these
/// accessors.
fn route_destination_json(
    destination: &glasshouse::routing::session::Destination,
) -> serde_json::Value {
    serde_json::json!({
        "id": destination.id(),
        "harness": destination.harness().slug(),
        "launch_profile": destination.launch_profile(),
        "provider": destination.backend().provider(),
        "protocol": destination.backend().protocol(),
        // `null` when the launch profile names no model and the harness's own
        // default serves — `AssignedModel`'s own distinction, kept rather than
        // flattened into an empty string (§71).
        "model": destination.backend().model().name(),
        "fresh": destination.is_fresh(),
    })
}

/// A [`glasshouse::routing::RoutingExplanation`] as JSON — ruling 3 of this
/// verb's packet: a bare destination identifier is not inspectable, so the
/// contributions and their evidence strings travel with it.
///
/// Each entry is exactly one line of `RoutingExplanation::render`, in the
/// order the scoring policy pushed it, with the magnitude as a number rather
/// than a formatted `+0.400`. A `0.0` magnitude is a real contribution — an
/// informational term that says an input was weighed and added nothing —
/// and is kept for that reason.
fn contributions_json(
    explanation: &glasshouse::routing::RoutingExplanation,
) -> Vec<serde_json::Value> {
    explanation
        .contributions()
        .iter()
        .map(|contribution| {
            serde_json::json!({
                "name": contribution.name(),
                "magnitude": contribution.magnitude(),
                "evidence": contribution.evidence(),
            })
        })
        .collect()
}

/// Matches `provider::resources::describe_layer`'s own wire spelling for
/// [`config::Layer`] (`"project"` / `"user"` / `"default"`), duplicated
/// rather than imported because that one is private to its own module.
fn describe_layer(layer: config::Layer) -> &'static str {
    match layer {
        config::Layer::Project => "project",
        config::Layer::User => "user",
        config::Layer::Default => "default",
    }
}
