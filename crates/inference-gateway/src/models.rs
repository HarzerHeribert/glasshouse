//! What the published measurements say about the models this gateway serves:
//! an intelligence index, a coding index, and the cost figures routing will
//! read next.
//!
//! **The gateway is where this belongs.** A model's measured capability is a
//! property of the model, not of a project, so it sits beside the catalogues
//! the gateway already caches rather than in a harness on top of it: a Pane
//! session with no Glasshouse installed anywhere still gets the figures, and
//! anything else that speaks to the gateway gets the same ones.
//!
//! **Two copies, and the fresher wins per figure.** [`BAKED`] ships with the
//! binary -- `scripts/bake-model-index.py` writes it from a catalogue when a
//! release is cut, which is what makes an install or an update carry current
//! numbers with no key and no network. A user who configures their own
//! Artificial Analysis key overlays that copy with what their key fetched
//! ([`import`] writes [`overlay_path`], `inference-gateway models --import`
//! is the command), and each figure the overlay carries wins over the baked
//! one. Until this binary fetches for itself, the accepted way to produce
//! that catalogue is any process holding the key --
//! `ARTIFICIAL_ANALYSIS_API_KEY=… glasshouse analysis --refresh && glasshouse
//! analysis | inference-gateway models --import -` is the one that exists
//! today, and it is a pipe rather than a dependency: nothing here knows what
//! wrote the file. Neither path ever fails a caller: an absent, unreadable or partial
//! file leaves the other copy standing.
//!
//! Nothing here reads a key, opens a socket or writes a log. The fetch that
//! needs the key happens in whatever process holds it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// The snapshot shipped with this binary, written by
/// `scripts/bake-model-index.py`.
pub const BAKED: &str = include_str!("../data/model-index.json");

/// What one model measured. Every figure is optional because the published
/// set is genuinely partial -- a newly listed model often has pricing and no
/// evaluations. An absent figure is recorded as absent, never as a zero.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelFacts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The headline index. Higher is better.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intelligence: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coding: Option<f64>,
    /// Tool use and multi-step work -- the closest published figure to what a
    /// coding harness actually asks of a model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agentic: Option<f64>,
    /// Weighted USD to complete the index's task set. Lower is better.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_per_task_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_usd_per_million: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_usd_per_million: Option<f64>,
    /// How much context the model accepts, in tokens -- the one limit a
    /// harness cannot choose and must not guess. A harness that does not know
    /// it compacts against a figure nobody supplied, so an absent figure is
    /// reported as absent and the harness says so.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window_tokens: Option<u64>,
    /// The most the model may produce in one response. The same rule: absent
    /// means the caller keeps its own documented fallback rather than
    /// inheriting a number invented here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
}

impl ModelFacts {
    /// `other`'s figures laid over these: a figure the fresher copy carries
    /// wins, one it lacks leaves the older figure standing.
    ///
    /// Per figure rather than per model because the two copies are partial in
    /// different places: an overlay fetched from the free endpoint may carry
    /// an intelligence index and no pricing, and replacing the whole entry
    /// would lose pricing the baked copy had.
    #[must_use]
    pub fn overlaid_with(&self, other: &Self) -> Self {
        Self {
            name: other.name.clone().or_else(|| self.name.clone()),
            intelligence: other.intelligence.or(self.intelligence),
            coding: other.coding.or(self.coding),
            agentic: other.agentic.or(self.agentic),
            cost_per_task_usd: other.cost_per_task_usd.or(self.cost_per_task_usd),
            input_usd_per_million: other.input_usd_per_million.or(self.input_usd_per_million),
            output_usd_per_million: other.output_usd_per_million.or(self.output_usd_per_million),
            context_window_tokens: other.context_window_tokens.or(self.context_window_tokens),
            max_output_tokens: other.max_output_tokens.or(self.max_output_tokens),
        }
    }

    /// Whether anything was measured at all.
    #[must_use]
    pub fn is_measured(&self) -> bool {
        self.intelligence.is_some() || self.coding.is_some() || self.agentic.is_some()
    }
}

/// A whole catalogue: where it came from, and one entry per model, keyed by
/// [`normalise`]d model name.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Measurements {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// The index version the figures were computed under. Printed rather than
    /// interpreted: comparing an index-4.1 score with an index-4.3 score is
    /// the caller's problem to avoid, and it cannot avoid it without this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_version: Option<f64>,
    /// The day the figures were captured, `YYYY-MM-DD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured: Option<String>,
    /// An overlay written by a fetching process carries this instead of
    /// `captured`; both are reported, neither is required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<u64>,
    #[serde(default)]
    pub models: BTreeMap<String, ModelFacts>,
}

impl Measurements {
    /// This catalogue with `overlay`'s entries laid over it, figure by figure
    /// ([`ModelFacts::overlaid_with`]). A model only the overlay knows is
    /// added; a model only this copy knows is kept.
    #[must_use]
    pub fn overlaid_with(&self, overlay: &Self) -> Self {
        let mut models = self.models.clone();
        for (id, fresh) in &overlay.models {
            let merged = models
                .get(id)
                .map_or_else(|| fresh.clone(), |baked| baked.overlaid_with(fresh));
            models.insert(id.clone(), merged);
        }
        Self {
            source: overlay.source.clone().or_else(|| self.source.clone()),
            index_version: overlay.index_version.or(self.index_version),
            captured: overlay.captured.clone().or_else(|| self.captured.clone()),
            fetched_at: overlay.fetched_at.or(self.fetched_at),
            models,
        }
    }
}

/// The name a model is looked up under: lower case, `.` and `_` as `-`.
///
/// Deliberately the same one line as `glasshouse::routing::analysis::normalise`
/// and `pane`'s copy: the three processes share no library, and a published
/// slug (`gpt-5-6-sol`) has to find a served id (`gpt-5.6-sol`).
#[must_use]
pub fn normalise(model: &str) -> String {
    model.trim().to_ascii_lowercase().replace(['.', '_'], "-")
}

/// The shipped snapshot, parsed once.
///
/// A file that will not parse is empty here and a failing test in this
/// module rather than a panic in a serving process: the file is generated
/// and checked in, so a broken one is caught before it ships.
#[must_use]
pub fn baked() -> &'static Measurements {
    static BAKED_ONCE: OnceLock<Measurements> = OnceLock::new();
    BAKED_ONCE.get_or_init(|| serde_json::from_str(BAKED).unwrap_or_default())
}

/// Where a user's own fetched catalogue is kept, beside the model catalogues
/// this gateway already caches.
#[must_use]
pub fn overlay_path(data_dir: &Path) -> PathBuf {
    data_dir
        .join("artificial-analysis")
        .join("language-models.json")
}

/// The overlay a user's key wrote, or `None`: absent, unreadable and
/// unparseable are the same answer, because each means the baked copy is
/// what this gateway knows.
#[must_use]
pub fn overlay(data_dir: &Path) -> Option<Measurements> {
    let bytes = std::fs::read(overlay_path(data_dir)).ok()?;
    serde_json::from_slice::<Measurements>(&bytes)
        .ok()
        .filter(|measurements| !measurements.models.is_empty())
}

/// What this gateway knows about every model it has a figure for: the baked
/// snapshot, overlaid by the user's own copy when they have one.
#[must_use]
pub fn measurements(data_dir: &Path) -> Measurements {
    match overlay(data_dir) {
        Some(fresh) => baked().overlaid_with(&fresh),
        None => baked().clone(),
    }
}

/// Writes `bytes` as this gateway's overlay, after checking it is a
/// catalogue with models in it. Returns how many models it holds.
///
/// The keys are normalised on the way in, so a catalogue keyed by published
/// slug and one keyed by served id both answer to the same lookup.
pub fn import(data_dir: &Path, bytes: &[u8]) -> Result<usize, String> {
    let parsed: Measurements = serde_json::from_slice(bytes)
        .map_err(|e| format!("not an Artificial Analysis catalogue: {e}"))?;
    if parsed.models.is_empty() {
        return Err("catalogue carries no models".to_string());
    }
    let normalised = Measurements {
        models: parsed
            .models
            .iter()
            .map(|(id, facts)| (normalise(id), facts.clone()))
            .collect(),
        ..parsed
    };
    let path = overlay_path(data_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let document = serde_json::to_string(&normalised).map_err(|e| e.to_string())?;
    std::fs::write(&path, document).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(normalised.models.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_snapshot_parses_and_carries_figures() {
        let baked = baked();
        assert!(
            baked.models.len() > 100,
            "the shipped snapshot has {} models; regenerate it with scripts/bake-model-index.py",
            baked.models.len()
        );
        let sol = baked
            .models
            .get("gpt-5-6-sol")
            .expect("a model the gateway serves is in the shipped snapshot");
        assert!(sol.intelligence.is_some() && sol.coding.is_some());
        assert!(baked.index_version.is_some() && baked.captured.is_some());
    }

    #[test]
    fn a_fresher_figure_wins_and_an_absent_one_leaves_the_baked_figure_standing() {
        let baked = Measurements {
            models: BTreeMap::from([(
                "m".to_string(),
                ModelFacts {
                    name: Some("M".into()),
                    intelligence: Some(10.0),
                    coding: Some(20.0),
                    input_usd_per_million: Some(1.0),
                    ..Default::default()
                },
            )]),
            ..Default::default()
        };
        let overlay = Measurements {
            models: BTreeMap::from([
                (
                    "m".to_string(),
                    ModelFacts {
                        intelligence: Some(11.5),
                        ..Default::default()
                    },
                ),
                (
                    "new".to_string(),
                    ModelFacts {
                        intelligence: Some(3.0),
                        ..Default::default()
                    },
                ),
            ]),
            ..Default::default()
        };
        let merged = baked.overlaid_with(&overlay);
        let m = &merged.models["m"];
        assert_eq!(m.intelligence, Some(11.5), "the fresher figure wins");
        assert_eq!(m.coding, Some(20.0), "a figure the overlay lacks stands");
        assert_eq!(m.input_usd_per_million, Some(1.0));
        assert!(merged.models.contains_key("new"), "a new model is added");
    }

    #[test]
    fn an_absent_or_broken_overlay_leaves_the_baked_copy_standing() {
        let dir = std::env::temp_dir().join(format!("gw-models-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(overlay(&dir).is_none(), "absent");
        assert_eq!(measurements(&dir).models.len(), baked().models.len());

        let path = overlay_path(&dir);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{ not json").unwrap();
        assert!(overlay(&dir).is_none(), "unparseable");
        std::fs::write(&path, br#"{"models":{}}"#).unwrap();
        assert!(overlay(&dir).is_none(), "empty");
        assert_eq!(measurements(&dir).models.len(), baked().models.len());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_imported_catalogue_is_normalised_and_read_back() {
        let dir = std::env::temp_dir().join(format!("gw-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let written = import(
            &dir,
            br#"{"fetched_at":1,"models":{"GPT_5.6-Sol":{"intelligence":99.0}}}"#,
        )
        .unwrap();
        assert_eq!(written, 1);
        let read = measurements(&dir);
        assert_eq!(read.models["gpt-5-6-sol"].intelligence, Some(99.0));
        assert!(
            read.models["gpt-5-6-sol"].coding.is_some(),
            "the baked coding figure survives an overlay that lacks one"
        );
        assert!(import(&dir, b"[]").is_err());
        assert!(import(&dir, br#"{"models":{}}"#).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_two_real_limits_round_trip_and_an_overlay_may_add_them_later() {
        // The figures a harness cannot choose: they travel in the same
        // document as the indices, so a catalogue that starts publishing them
        // reaches a session with no code change.
        let parsed: Measurements = serde_json::from_str(
            r#"{"models":{"m":{"intelligence":1.0,"context_window_tokens":400000,"max_output_tokens":128000}}}"#,
        )
        .unwrap();
        assert_eq!(parsed.models["m"].context_window_tokens, Some(400_000));
        assert_eq!(parsed.models["m"].max_output_tokens, Some(128_000));
        let json = serde_json::to_string(&parsed).unwrap();
        assert!(
            json.contains("context_window_tokens"),
            "served to a harness"
        );

        let baked = Measurements {
            models: BTreeMap::from([(
                "m".to_string(),
                ModelFacts {
                    intelligence: Some(1.0),
                    ..Default::default()
                },
            )]),
            ..Default::default()
        };
        let merged = baked.overlaid_with(&parsed);
        assert_eq!(
            merged.models["m"].context_window_tokens,
            Some(400_000),
            "an overlay that learns a window teaches the baked copy"
        );
        let quiet: Measurements =
            serde_json::from_str(r#"{"models":{"m":{"intelligence":1.0}}}"#).unwrap();
        assert_eq!(
            quiet.models["m"].context_window_tokens, None,
            "no catalogue we consume publishes it yet, and absent stays absent"
        );
    }
}
