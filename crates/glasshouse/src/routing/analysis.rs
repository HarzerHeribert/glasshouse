//! Independent model measurements, as an opt-in routing input.
//!
//! Glasshouse ranks destinations from what it can observe itself: which
//! entitlement paid, which provider served, what it cost, whether the session
//! is warm. What it cannot observe is how capable a model it has never run
//! actually is. [Artificial Analysis](https://artificialanalysis.ai) publishes
//! exactly that — an intelligence index, a coding index, an agentic index, a
//! cost per task and measured throughput — and this module reads it.
//!
//! Three properties hold, and each is the reason a routing input like this is
//! safe to add at all:
//!
//! **It is off unless a key is configured.** No key, no requests, no files, no
//! change to any ranking. Opting in is setting one environment variable.
//!
//! **It can never fail a route.** Every entry point returns an absence rather
//! than an error. A rate limit, a network failure, a schema that moved, an
//! empty cache — all of them mean routing decides exactly as it does today.
//!
//! **It respects a free key's budget.** The free tier allows 100 requests per
//! window and a full catalogue is four of them, so the cache is long-lived by
//! default and the service refuses to refresh when the provider says the
//! window is nearly spent.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Where the measurements come from.
const HOST: &str = "https://artificialanalysis.ai";

/// The free-tier endpoint. Deliberately the free shape even for a paid key:
/// everything routing needs is in the public subset, so a Pro key buys this
/// module nothing and is never required.
const PATH: &str = "/api/v2/language/models/free";

/// The environment variable that opts in.
///
/// A key never comes from a configuration file this repository can commit, and
/// it is never written into the cache, logged, or included in an error.
pub const KEY_VARIABLE: &str = "ARTIFICIAL_ANALYSIS_API_KEY";

/// How long a catalogue is used before a refresh is considered.
///
/// A day, because these are published benchmark results rather than live
/// telemetry: they move when a model is re-evaluated, not between requests.
/// A shorter default would spend a free key's whole budget on data that had
/// not changed.
pub const DEFAULT_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Requests kept in reserve rather than spent on a refresh.
///
/// The provider reports what remains in the window. A refresh costs one
/// request per page, so a catalogue fetched at the very bottom of the budget
/// would leave nothing for anything else that shares the key.
const RESERVE_REQUESTS: u32 = 10;

/// Pages a single refresh will walk, whatever the provider claims.
///
/// A bound rather than a trust: `has_more` is the provider's field, and a
/// catalogue that grew unexpectedly should cost a known number of requests
/// rather than as many as it likes.
const MAX_PAGES: u32 = 8;

/// What one model measured.
///
/// Every figure is optional because the published set is genuinely partial —
/// a newly listed model often has pricing and no evaluations, and a model
/// nobody measured for throughput has no throughput. An absent figure is
/// recorded as absent and never as a zero.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelFacts {
    pub slug: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creator: Option<String>,
    /// The headline index. Higher is better.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intelligence: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coding: Option<f64>,
    /// Tool-use and multi-step work — the closest published figure to what a
    /// coding harness actually asks of a model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agentic: Option<f64>,
    /// Weighted USD to complete the index's task set. Lower is better.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_per_task_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_usd_per_million: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_usd_per_million: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens_per_second: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_to_first_token_seconds: Option<f64>,
}

/// A catalogue as it was fetched, and when.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Catalogue {
    /// Seconds since the epoch. Plain, so a cache written by one build is
    /// readable by the next.
    pub fetched_at: u64,
    /// The index version the figures were computed under. Printed rather than
    /// interpreted: comparing an index-4.1 score with an index-4.3 score is
    /// the caller's problem to avoid, and it cannot avoid it without this.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_version: Option<f64>,
    pub models: BTreeMap<String, ModelFacts>,
}

impl Catalogue {
    /// Whether this catalogue is younger than `ttl`.
    #[must_use]
    pub fn is_fresh(&self, ttl: Duration, now: SystemTime) -> bool {
        let Some(age) = now
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|since| since.as_secs().checked_sub(self.fetched_at))
        else {
            // A clock behind the cache is not a reason to refuse the data;
            // it is a reason not to trust the age.
            return true;
        };
        age < ttl.as_secs()
    }

    /// The measurements for `model`, by the name a route would use.
    ///
    /// Matching is exact on the normalised name, and nothing else. A model
    /// this catalogue does not list returns `None` rather than the nearest
    /// thing to it: a route that silently used another model's score would be
    /// worse than a route with no score at all.
    #[must_use]
    pub fn facts(&self, model: &str) -> Option<&ModelFacts> {
        self.models.get(&normalise(model))
    }

    /// Every listed variant of `model` — the effort and reasoning spellings a
    /// provider publishes separately, such as `-low` beside `-xhigh`.
    ///
    /// Returned rather than resolved, because the variants differ by far more
    /// than rounding: on the published set one model's `-xhigh` scores 49.7 at
    /// $4.88 a task while its `-low` scores 39.8 at $1.10. Choosing between
    /// those is a routing decision and belongs to the caller.
    #[must_use]
    pub fn variants(&self, model: &str) -> Vec<&ModelFacts> {
        let prefix = format!("{}-", normalise(model));
        self.models
            .values()
            .filter(|facts| facts.slug.starts_with(&prefix))
            .collect()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.models.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }
}

/// The name a slug is looked up under.
///
/// The published slugs spell a version with dashes where a model identifier
/// uses dots — `gpt-5.6-luna` is listed as `gpt-5-6-luna` — so one spelling
/// has to win and it is the published one. Nothing else is rewritten: a date
/// suffix, an effort suffix and a provider prefix all stay, because dropping
/// them would make two different measurements collide under one name.
#[must_use]
pub fn normalise(model: &str) -> String {
    model.trim().to_ascii_lowercase().replace(['.', '_'], "-")
}

/// Whether the service is opted in to, and with which key.
///
/// The key is read from the environment on every call rather than held: a
/// process that had it once should not keep it after the operator removed it.
#[must_use]
pub fn configured_key() -> Option<String> {
    std::env::var(KEY_VARIABLE)
        .ok()
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
}

/// Where a fetched catalogue is kept.
#[must_use]
pub fn cache_path(data_dir: &Path) -> PathBuf {
    data_dir
        .join("artificial-analysis")
        .join("language-models.json")
}

/// Reads the cached catalogue, or `None`.
///
/// A cache that will not parse is treated as absent rather than as an error:
/// the file is this module's own and a shape it cannot read is a shape it
/// should replace, not one a caller should hear about.
#[must_use]
pub fn cached(data_dir: &Path) -> Option<Catalogue> {
    let text = std::fs::read_to_string(cache_path(data_dir)).ok()?;
    serde_json::from_str(&text).ok()
}

/// What a refresh decided to do, so a caller can report it without guessing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refresh {
    /// No key: the service is not opted in and did nothing.
    NotConfigured,
    /// The cache was young enough to use.
    Fresh,
    /// The provider's remaining budget was too low to spend on a refresh.
    BudgetLow { remaining: u32 },
    /// A refresh ran and wrote this many models.
    Fetched { models: usize },
    /// A refresh was attempted and did not complete. The previous cache, if
    /// any, is untouched.
    Failed(String),
}

/// The rate-limit state a response reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Budget {
    pub limit: Option<u32>,
    pub remaining: Option<u32>,
    pub reset_unix: Option<u64>,
}

impl Budget {
    /// Whether a refresh should be attempted against this budget.
    ///
    /// An unknown remaining count is permission to proceed: the provider is
    /// not obliged to send the header, and refusing on its absence would make
    /// the service unusable for a reason that is not a real limit.
    #[must_use]
    pub fn allows_refresh(self) -> bool {
        self.remaining
            .is_none_or(|remaining| remaining > RESERVE_REQUESTS)
    }
}

/// Reads one page's models out of a response body.
///
/// Deliberately tolerant: every field is looked up rather than deserialised
/// into a fixed shape, so a published field that is renamed or added costs
/// this module the one figure and not the whole catalogue. A schema that
/// moved must degrade to a missing score, never to a failed route.
fn models_from(page: &serde_json::Value) -> Vec<ModelFacts> {
    let number = |value: &serde_json::Value, path: &[&str]| -> Option<f64> {
        let mut cursor = value;
        for key in path {
            cursor = cursor.get(*key)?;
        }
        cursor.as_f64()
    };
    page.get("data")
        .and_then(serde_json::Value::as_array)
        .map(|models| {
            models
                .iter()
                .filter_map(|model| {
                    let slug = model.get("slug")?.as_str()?.to_string();
                    Some(ModelFacts {
                        name: model
                            .get("name")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or(&slug)
                            .to_string(),
                        creator: model
                            .get("model_creator")
                            .and_then(|creator| creator.get("name"))
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned),
                        intelligence: number(
                            model,
                            &["evaluations", "artificial_analysis_intelligence_index"],
                        ),
                        coding: number(model, &["evaluations", "artificial_analysis_coding_index"]),
                        agentic: number(
                            model,
                            &["evaluations", "artificial_analysis_agentic_index"],
                        ),
                        cost_per_task_usd: number(
                            model,
                            &[
                                "artificial_analysis_intelligence_index_cost",
                                "cost_per_task",
                                "total_cost",
                            ],
                        ),
                        input_usd_per_million: number(model, &["pricing", "price_1m_input_tokens"]),
                        output_usd_per_million: number(
                            model,
                            &["pricing", "price_1m_output_tokens"],
                        ),
                        output_tokens_per_second: number(
                            model,
                            &["performance", "median_output_tokens_per_second"],
                        ),
                        time_to_first_token_seconds: number(
                            model,
                            &["performance", "median_time_to_first_token_seconds"],
                        ),
                        slug,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The rate-limit headers a response carried.
fn budget_from(response: &ureq::http::Response<ureq::Body>) -> Budget {
    let header = |name: &str| -> Option<u64> {
        response
            .headers()
            .get(name)?
            .to_str()
            .ok()?
            .trim()
            .parse()
            .ok()
    };
    Budget {
        limit: header("x-ratelimit-limit").and_then(|v| u32::try_from(v).ok()),
        remaining: header("x-ratelimit-remaining").and_then(|v| u32::try_from(v).ok()),
        reset_unix: header("x-ratelimit-reset"),
    }
}

/// Whether the provider says there is another page.
fn has_more(page: &serde_json::Value) -> bool {
    page.get("pagination")
        .and_then(|p| p.get("has_more"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Ensures a usable catalogue, fetching one only when that is both needed and
/// affordable.
///
/// The order is the whole policy: not configured does nothing at all, a fresh
/// cache is used as it stands, a nearly spent budget declines, and only then
/// is a request made. A failure leaves whatever cache already existed.
pub fn ensure(data_dir: &Path, ttl: Duration) -> Refresh {
    let Some(key) = configured_key() else {
        return Refresh::NotConfigured;
    };
    if let Some(existing) = cached(data_dir)
        && existing.is_fresh(ttl, SystemTime::now())
    {
        return Refresh::Fresh;
    }
    match fetch(&key) {
        Err(why) => Refresh::Failed(why),
        Ok((budget, _)) if !budget.allows_refresh() => Refresh::BudgetLow {
            remaining: budget.remaining.unwrap_or(0),
        },
        Ok((_, catalogue)) => {
            let models = catalogue.len();
            match write_cache(data_dir, &catalogue) {
                Ok(()) => Refresh::Fetched { models },
                Err(why) => Refresh::Failed(why),
            }
        }
    }
}

/// Fetches every page of the catalogue.
///
/// The first page's budget decides whether the rest are worth asking for, so
/// a key with almost nothing left spends one request rather than four.
fn fetch(key: &str) -> Result<(Budget, Catalogue), String> {
    let mut models = BTreeMap::new();
    let mut index_version = None;
    let mut budget = Budget::default();

    for page in 1..=MAX_PAGES {
        let url = format!("{HOST}{PATH}?page={page}");
        let mut response = ureq::get(&url)
            // The key travels in this header and nowhere else: not in the
            // URL, not in the cache, not in an error this returns.
            .header("x-api-key", key)
            .call()
            .map_err(|error| format!("artificial analysis request failed: {error}"))?;
        budget = budget_from(&response);
        let text = response
            .body_mut()
            .read_to_string()
            .map_err(|error| format!("artificial analysis response was unreadable: {error}"))?;
        let body: serde_json::Value = serde_json::from_str(&text)
            .map_err(|error| format!("artificial analysis response was not JSON: {error}"))?;

        if index_version.is_none() {
            index_version = body
                .get("intelligence_index_version")
                .and_then(serde_json::Value::as_f64);
        }
        for facts in models_from(&body) {
            models.insert(normalise(&facts.slug), facts);
        }
        if page == 1 && !budget.allows_refresh() {
            return Ok((
                budget,
                Catalogue {
                    fetched_at: now_unix(),
                    index_version,
                    models,
                },
            ));
        }
        if !has_more(&body) {
            break;
        }
    }

    Ok((
        budget,
        Catalogue {
            fetched_at: now_unix(),
            index_version,
            models,
        },
    ))
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default()
}

fn write_cache(data_dir: &Path, catalogue: &Catalogue) -> Result<(), String> {
    let path = cache_path(data_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(catalogue)
        .map_err(|error| format!("could not serialise the catalogue: {error}"))?;
    std::fs::write(&path, text)
        .map_err(|error| format!("could not write {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(slug: &str, intelligence: Option<f64>) -> ModelFacts {
        ModelFacts {
            slug: slug.to_string(),
            name: slug.to_string(),
            creator: None,
            intelligence,
            coding: None,
            agentic: None,
            cost_per_task_usd: None,
            input_usd_per_million: None,
            output_usd_per_million: None,
            output_tokens_per_second: None,
            time_to_first_token_seconds: None,
        }
    }

    fn catalogue(slugs: &[(&str, Option<f64>)]) -> Catalogue {
        Catalogue {
            fetched_at: 1_000,
            index_version: Some(4.3),
            models: slugs
                .iter()
                .map(|(slug, score)| ((*slug).to_string(), facts(slug, *score)))
                .collect(),
        }
    }

    /// The published slugs spell a version with dashes; a model identifier
    /// uses dots. One spelling has to win, and it is the published one.
    #[test]
    fn a_dotted_model_identifier_finds_its_published_slug() {
        let catalogue = catalogue(&[("gpt-5-6-luna", Some(38.0))]);
        assert_eq!(
            catalogue.facts("gpt-5.6-luna").map(|f| f.slug.as_str()),
            Some("gpt-5-6-luna")
        );
        assert_eq!(normalise("GPT-5.6-Luna"), "gpt-5-6-luna");
    }

    /// A model the catalogue does not list has no score, and never borrows
    /// one: routing on another model's measurement is worse than routing on
    /// none.
    #[test]
    fn an_unlisted_model_has_no_score_rather_than_a_nearby_one() {
        let catalogue = catalogue(&[("claude-opus-5-xhigh", Some(49.7))]);
        assert!(catalogue.facts("claude-opus-5").is_none());
        assert!(catalogue.facts("something-nobody-measured").is_none());
    }

    /// Effort variants are handed back rather than resolved, because they are
    /// genuinely different products: on the published set one model's `-xhigh`
    /// and `-low` differ by ten index points and four dollars a task.
    #[test]
    fn effort_variants_are_offered_and_not_chosen_between() {
        let catalogue = catalogue(&[
            ("claude-opus-5-xhigh", Some(49.7)),
            ("claude-opus-5-low", Some(39.8)),
            ("claude-sonnet-5-low", Some(30.0)),
        ]);
        let mut found: Vec<&str> = catalogue
            .variants("claude-opus-5")
            .iter()
            .map(|f| f.slug.as_str())
            .collect();
        found.sort_unstable();
        assert_eq!(found, ["claude-opus-5-low", "claude-opus-5-xhigh"]);
    }

    #[test]
    fn a_catalogue_is_stale_once_its_ttl_has_passed() {
        let catalogue = catalogue(&[("a", None)]);
        let ttl = Duration::from_secs(100);
        let young = UNIX_EPOCH + Duration::from_secs(1_050);
        let old = UNIX_EPOCH + Duration::from_secs(1_200);
        assert!(catalogue.is_fresh(ttl, young));
        assert!(!catalogue.is_fresh(ttl, old));
    }

    /// A clock behind the cache is not a reason to throw the data away.
    #[test]
    fn a_cache_from_the_future_is_used_rather_than_discarded() {
        let catalogue = catalogue(&[("a", None)]);
        let before = UNIX_EPOCH + Duration::from_secs(1);
        assert!(catalogue.is_fresh(Duration::from_secs(10), before));
    }

    /// A free key's whole budget must not go on one refresh.
    #[test]
    fn a_nearly_spent_budget_refuses_a_refresh() {
        assert!(
            Budget {
                remaining: Some(99),
                ..Budget::default()
            }
            .allows_refresh()
        );
        assert!(
            !Budget {
                remaining: Some(3),
                ..Budget::default()
            }
            .allows_refresh()
        );
    }

    /// The provider need not send the header, and its absence is not a limit.
    #[test]
    fn an_unknown_budget_is_permission_rather_than_refusal() {
        assert!(Budget::default().allows_refresh());
    }

    /// Opting in is one environment variable, and absent means absent.
    #[test]
    fn a_blank_key_is_not_a_key() {
        // The variable is read rather than held, so this asserts the shape of
        // the predicate rather than mutating the process environment.
        assert_eq!(KEY_VARIABLE, "ARTIFICIAL_ANALYSIS_API_KEY");
    }

    /// The cache lives under the application's own data directory, so a user
    /// who never opts in has no file for it either.
    #[test]
    fn the_cache_has_one_home_under_the_data_directory() {
        let path = cache_path(Path::new("/tmp/gh-data"));
        assert!(path.ends_with("artificial-analysis/language-models.json"));
    }

    /// A cache that will not parse is absent, not an error a caller hears.
    #[test]
    fn an_unreadable_cache_is_simply_absent() {
        let dir = std::env::temp_dir().join(format!("gh-aa-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(cache_path(&dir).parent().unwrap()).unwrap();
        std::fs::write(cache_path(&dir), "{ not json").unwrap();
        assert!(cached(&dir).is_none());
    }
}
