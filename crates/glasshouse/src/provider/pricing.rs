//! Provider price metadata — capability map lines 1305 and 1306.
//!
//! [`PriceTable::price_for`] answers `None` for a provider/model pair this
//! table does not name, and that `None` must reach the routing explanation
//! as a stated unknown, never as a silent zero.
//! The file is untrusted user input. [`PriceTable::load_from_dir`] never
//! panics and never refuses to route: a missing file, an unreadable one, a
//! malformed document, or a single invalid entry all degrade to
//! [`PriceTable::empty`], routing with every price unknown — logged once via
//! [`tracing::warn!`] naming the path and the parse error, never the
//! document's contents.
// History: design-decisions.md, "Trims: gateway, profile and provider module docs", pricing.rs module doc.

use std::path::Path;

use serde::Deserialize;

/// File name of the user-owned price metadata document, resolved relative to
/// [`crate::paths::RuntimePaths::config_dir`].
pub const PRICING_FILE_NAME: &str = "pricing.toml";

/// A document larger than this is refused outright and never parsed. Price
/// metadata is a handful of numbers per model; anything past one megabyte is
/// not a price list.
const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;

/// Several orders of magnitude above any real per-million-token price — a
/// guard against a corrupt or adversarial value reaching arithmetic, not a
/// claim about what any provider actually charges.
const MAX_PRICE_PER_MILLION_USD: f64 = 1_000_000.0;

/// One provider/model's price, in US dollars per million tokens.
///
/// `cached_input_per_million_usd` is optional in the source file and stays
/// optional here: `None` means the file's author never stated a cached-input
/// rate, and is read by every consumer as *unknown*, never as *free* or as
/// `input_per_million_usd`'s value — map line 1300, unblocked once
/// `cache_read_ratio` (`routing::evidence::joins`) gave the signal a producer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPrice {
    pub input_per_million_usd: f64,
    pub output_per_million_usd: f64,
    pub cached_input_per_million_usd: Option<f64>,
}

/// The on-disk shape of `pricing.toml`: an array of tables so a model name
/// containing `/` (most of them do) never has to be a TOML key.
#[derive(Debug, Clone, Deserialize, Default)]
struct RawDocument {
    #[serde(default)]
    prices: Vec<RawEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawEntry {
    provider: String,
    model: String,
    input_per_million_usd: f64,
    output_per_million_usd: f64,
    /// Absent in the file means unknown, never zero — see [`ModelPrice`]'s
    /// own doc comment.
    #[serde(default)]
    cached_input_per_million_usd: Option<f64>,
}

/// Every price this build currently knows, keyed by exact provider and model
/// name.
///
/// [`PriceTable::default`] is empty, which is deliberate: it is what every
/// destination scored before this package, and what every destination with
/// no metadata file still sees after it. Construct one from disk with
/// [`PriceTable::load_from_dir`]; nothing in this crate constructs one any
/// other way, so a caller cannot mark a price known without it having come
/// from that file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PriceTable {
    entries: Vec<(String, String, ModelPrice)>,
}

impl PriceTable {
    /// No metadata at all — the state of this build before this package, and
    /// of every user who has not written `pricing.toml`.
    pub fn empty() -> Self {
        Self::default()
    }

    /// The known price for `provider`/`model`, or `None` when nothing in
    /// this table names that exact pair. `None` is the caller's cue to
    /// render "unknown", never a zero.
    pub fn price_for(&self, provider: &str, model: &str) -> Option<ModelPrice> {
        self.entries
            .iter()
            .find(|(p, m, _)| p == provider && m == model)
            .map(|(_, _, price)| *price)
    }

    /// Load `dir/pricing.toml`. Fail-soft: any failure at all — missing file,
    /// unreadable file, malformed document, an out-of-range number — answers
    /// [`PriceTable::empty`] rather than a partial table or a panic. A parse
    /// failure is logged once with the path and the error, never with the
    /// document's contents.
    pub fn load_from_dir(dir: &Path) -> Self {
        let path = dir.join(PRICING_FILE_NAME);
        match std::fs::read_to_string(&path) {
            Ok(contents) => Self::parse(&contents).unwrap_or_else(|error| {
                tracing::warn!(
                    path = %path.display(),
                    %error,
                    "ignoring malformed provider price metadata; routing continues with every \
                     price unknown"
                );
                Self::empty()
            }),
            // A missing or unreadable file is the normal case for every
            // existing user — not logged, and not distinguished from "empty
            // on purpose".
            Err(_) => Self::empty(),
        }
    }

    /// Parse a price-metadata document already read into memory. Private —
    /// every production path goes through [`Self::load_from_dir`]; this is
    /// exposed to this module's own tests only.
    fn parse(contents: &str) -> Result<Self, String> {
        if contents.len() > MAX_DOCUMENT_BYTES {
            return Err(format!(
                "document is {} bytes, over the {MAX_DOCUMENT_BYTES}-byte limit",
                contents.len()
            ));
        }
        let document: RawDocument = toml::from_str(contents).map_err(|error| error.to_string())?;
        let mut entries = Vec::with_capacity(document.prices.len());
        for entry in document.prices {
            let input = validate_price(entry.input_per_million_usd).ok_or_else(|| {
                format!(
                    "{}/{}: input_per_million_usd is not a valid price",
                    entry.provider, entry.model
                )
            })?;
            let output = validate_price(entry.output_per_million_usd).ok_or_else(|| {
                format!(
                    "{}/{}: output_per_million_usd is not a valid price",
                    entry.provider, entry.model
                )
            })?;
            let cached_input = entry
                .cached_input_per_million_usd
                .map(|value| {
                    validate_price(value).ok_or_else(|| {
                        format!(
                            "{}/{}: cached_input_per_million_usd is not a valid price",
                            entry.provider, entry.model
                        )
                    })
                })
                .transpose()?;
            entries.push((
                entry.provider,
                entry.model,
                ModelPrice {
                    input_per_million_usd: input,
                    output_per_million_usd: output,
                    cached_input_per_million_usd: cached_input,
                },
            ));
        }
        Ok(Self { entries })
    }
}

/// A price is admissible only if it is finite, non-negative, and below
/// [`MAX_PRICE_PER_MILLION_USD`]. Rejects `NaN`, infinities, negative values,
/// and absurd ones like `1e308` before they can reach a score.
fn validate_price(value: f64) -> Option<f64> {
    if value.is_finite() && (0.0..=MAX_PRICE_PER_MILLION_USD).contains(&value) {
        Some(value)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_table_answers_unknown_for_every_lookup() {
        let table = PriceTable::empty();
        assert_eq!(table.price_for("openrouter", "gpt-4"), None);
    }

    #[test]
    fn a_well_formed_document_parses_and_is_looked_up_by_exact_provider_and_model() {
        let table = PriceTable::parse(
            r#"
            [[prices]]
            provider = "openrouter"
            model = "anthropic/claude-opus-4"
            input_per_million_usd = 15.0
            output_per_million_usd = 75.0
            "#,
        )
        .expect("well-formed document must parse");
        let price = table
            .price_for("openrouter", "anthropic/claude-opus-4")
            .expect("the entry above must be found");
        assert_eq!(price.input_per_million_usd, 15.0);
        assert_eq!(price.output_per_million_usd, 75.0);
        assert_eq!(
            price.cached_input_per_million_usd, None,
            "an entry with no cached_input_per_million_usd must parse as unknown, not zero"
        );
        assert_eq!(table.price_for("openrouter", "a-different-model"), None);
        assert_eq!(
            table.price_for("a-different-provider", "anthropic/claude-opus-4"),
            None
        );
    }

    #[test]
    fn a_declared_cached_input_rate_parses_and_is_looked_up() {
        let table = PriceTable::parse(
            r#"
            [[prices]]
            provider = "openrouter"
            model = "anthropic/claude-opus-4"
            input_per_million_usd = 15.0
            output_per_million_usd = 75.0
            cached_input_per_million_usd = 1.5
            "#,
        )
        .expect("a declared cached-input rate must parse");
        let price = table
            .price_for("openrouter", "anthropic/claude-opus-4")
            .expect("the entry above must be found");
        assert_eq!(price.cached_input_per_million_usd, Some(1.5));
    }

    #[test]
    fn a_malformed_cached_input_rate_is_refused_naming_provider_and_model() {
        let result = PriceTable::parse(
            r#"
            [[prices]]
            provider = "openrouter"
            model = "m"
            input_per_million_usd = 1.0
            output_per_million_usd = 1.0
            cached_input_per_million_usd = -1.0
            "#,
        );
        let error = result.expect_err("a negative cached-input rate must not parse");
        assert!(
            error.contains("openrouter") && error.contains('m') && error.contains("cached_input"),
            "the error must name the provider, model and field: {error}"
        );
    }

    #[test]
    fn a_negative_price_is_refused() {
        let result = PriceTable::parse(
            r#"
            [[prices]]
            provider = "openrouter"
            model = "m"
            input_per_million_usd = -1.0
            output_per_million_usd = 1.0
            "#,
        );
        assert!(result.is_err(), "a negative price must not parse");
    }

    #[test]
    fn a_non_finite_price_is_refused() {
        // `inf` and `nan` are valid TOML float tokens — exactly the
        // adversarial input the packet's security section names.
        let result = PriceTable::parse(
            r#"
            [[prices]]
            provider = "openrouter"
            model = "m"
            input_per_million_usd = inf
            output_per_million_usd = 1.0
            "#,
        );
        assert!(result.is_err(), "a non-finite price must not parse");
    }

    #[test]
    fn an_absurdly_large_price_is_refused() {
        let result = PriceTable::parse(
            r#"
            [[prices]]
            provider = "openrouter"
            model = "m"
            input_per_million_usd = 1e308
            output_per_million_usd = 1.0
            "#,
        );
        assert!(result.is_err(), "an absurd price must not parse");
    }

    #[test]
    fn malformed_toml_is_refused_rather_than_panicking() {
        let result = PriceTable::parse("this is not [ valid toml");
        assert!(result.is_err());
    }

    #[test]
    fn an_oversized_document_is_refused_without_parsing() {
        let huge = "x".repeat(MAX_DOCUMENT_BYTES + 1);
        let result = PriceTable::parse(&huge);
        assert!(result.is_err());
    }

    #[test]
    fn loading_a_missing_file_answers_empty_rather_than_erroring() {
        let dir = std::env::temp_dir().join(format!(
            "glasshouse-pricing-test-missing-{}",
            std::process::id()
        ));
        // Deliberately do not create `dir` — the loader must tolerate a
        // directory (or file) that does not exist at all.
        let table = PriceTable::load_from_dir(&dir);
        assert_eq!(table, PriceTable::empty());
    }
}
