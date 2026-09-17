//! What the gateway knows about the models it serves, for the one caller that
//! needs it: the subagent roster in the system block.
//!
//! **The figures are the gateway's, and pane only asks.** `inference-gateway
//! models --json` answers with the snapshot baked into that binary overlaid
//! by whatever the user's own Artificial Analysis key last fetched
//! (`inference_gateway::models`), so a session with no Glasshouse installed
//! anywhere still gets them. This module parses that document and keeps the
//! models this session can actually reach.
//!
//! **An absence is never an error.** No gateway, an older gateway that does
//! not know the subcommand, a document that will not parse -- each leaves the
//! roster unmeasured, and an unmeasured model is still offered to the model,
//! by name, without figures.
//!
//! `crate::glasshouse::intelligence` is the other reader of the same
//! measurements and stays as it is: the `/model` picker orders by what
//! Glasshouse cached, and this path does not disturb it.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::gateway::Gateway;

/// One model a session may delegate to.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RosterModel {
    /// The id `agent.run({model})` names -- the served spelling, not the
    /// published slug.
    pub id: String,
    /// The headline index, higher is better; `None` when nobody published one.
    pub intelligence: Option<f64>,
    pub coding: Option<f64>,
}

/// The `models` half of `inference-gateway models --json`.
#[derive(Debug, Clone, Default, Deserialize)]
struct Document {
    #[serde(default)]
    models: BTreeMap<String, Facts>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct Facts {
    #[serde(default)]
    intelligence: Option<f64>,
    #[serde(default)]
    coding: Option<f64>,
}

/// The name a model is looked up under: lower case, `.` and `_` as `-`.
///
/// The same one line as `inference_gateway::models::normalise` and
/// `glasshouse::routing::analysis::normalise`; these three processes share no
/// library, and a published slug (`gpt-5-6-sol`) has to find a served id
/// (`gpt-5.6-sol`).
#[must_use]
pub fn normalise(model: &str) -> String {
    model.trim().to_ascii_lowercase().replace(['.', '_'], "-")
}

/// Every model in `served`, carrying whatever the gateway has measured for
/// it. Strongest first, unmeasured last by name.
#[must_use]
pub fn roster(gateway: &Gateway, served: &[String]) -> Vec<RosterModel> {
    measure(served, &published(gateway))
}

/// [`roster`] with the figures already in hand -- the seam the tests drive,
/// and the only place the ordering is decided.
#[must_use]
pub fn measure(served: &[String], published: &BTreeMap<String, MeasuredFacts>) -> Vec<RosterModel> {
    let mut models: Vec<RosterModel> = served
        .iter()
        .map(|id| {
            let facts = published.get(&normalise(id));
            RosterModel {
                id: id.clone(),
                intelligence: facts.and_then(|facts| facts.intelligence),
                coding: facts.and_then(|facts| facts.coding),
            }
        })
        .collect();
    // Strongest first; a model nobody measured sorts after every measured
    // one rather than as a zero, because unmeasured is not weak.
    models.sort_by(|a, b| match (a.intelligence, b.intelligence) {
        (Some(x), Some(y)) => y
            .partial_cmp(&x)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.id.cmp(&b.id)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.id.cmp(&b.id),
    });
    models
}

/// The figures the roster reads.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeasuredFacts {
    pub intelligence: Option<f64>,
    pub coding: Option<f64>,
}

/// The gateway's measurements, keyed by [`normalise`]d name; empty whenever
/// the gateway cannot answer.
#[must_use]
pub fn published(gateway: &Gateway) -> BTreeMap<String, MeasuredFacts> {
    let Some(bytes) = gateway.run(&["models", "--json"], None) else {
        return BTreeMap::new();
    };
    let Ok(document) = serde_json::from_slice::<Document>(&bytes) else {
        return BTreeMap::new();
    };
    document
        .models
        .into_iter()
        .map(|(id, facts)| {
            (
                normalise(&id),
                MeasuredFacts {
                    intelligence: facts.intelligence,
                    coding: facts.coding,
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(intelligence: Option<f64>, coding: Option<f64>) -> MeasuredFacts {
        MeasuredFacts {
            intelligence,
            coding,
        }
    }

    #[test]
    fn a_served_model_carries_the_published_figures_for_its_normalised_name() {
        let published =
            BTreeMap::from([("gpt-5-6-sol".to_string(), facts(Some(47.1), Some(77.4)))]);
        let roster = measure(&["gpt-5.6-sol".to_string()], &published);
        assert_eq!(roster[0].intelligence, Some(47.1));
        assert_eq!(roster[0].coding, Some(77.4));
        assert_eq!(roster[0].id, "gpt-5.6-sol", "the served spelling is kept");
    }

    #[test]
    fn the_strongest_leads_and_an_unmeasured_model_is_listed_last_not_as_a_zero() {
        let published = BTreeMap::from([
            ("weak".to_string(), facts(Some(10.0), None)),
            ("strong".to_string(), facts(Some(50.0), None)),
        ]);
        let roster = measure(
            &[
                "weak".to_string(),
                "unmeasured".to_string(),
                "strong".to_string(),
            ],
            &published,
        );
        let order: Vec<&str> = roster.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(order, ["strong", "weak", "unmeasured"]);
        assert_eq!(roster[2].intelligence, None);
    }

    #[test]
    fn nothing_published_still_lists_every_served_model() {
        let roster = measure(&["a".to_string(), "b".to_string()], &BTreeMap::new());
        assert_eq!(roster.len(), 2);
        assert!(roster.iter().all(|model| model.intelligence.is_none()));
    }

    #[test]
    fn an_unreachable_gateway_publishes_nothing_rather_than_failing() {
        assert!(published(&Gateway::None).is_empty());
    }
}
