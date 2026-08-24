//! Provider signal detection: environment variables and config files that
//! hint a model/inference provider is already set up, without ever reading
//! or logging the secret material itself.
//!
//! The rule this module exists to enforce: **presence, never content.** A
//! [`ProviderSignals`] value can only ever answer "is `ANTHROPIC_API_KEY`
//! set?", never "what is it set to?" — the value is read from the process
//! environment exactly once, classified, and then discarded; it is never
//! copied into any field of this module's types. That is a structural
//! guarantee, not just a convention: there is no field anywhere in this
//! module capable of holding a secret value, so no `Debug`/log/report code
//! path can leak one by accident.

use std::path::PathBuf;

use crate::integrations::home_dir;

/// Environment variables Glasshouse knows to look for, grouped by what kind
/// of thing they typically hold. The grouping exists purely for reporting:
/// a "secret-like" variable is reported as present/absent only, while an
/// "endpoint-like" one names an endpoint (base URL / host), not a
/// credential — the name is safe to show, and it is still only the name that
/// is shown.
const SECRET_LIKE_ENV_VARS: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "OPENAI_API_KEY",
    "OPENROUTER_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "NVIDIA_API_KEY",
    "GROQ_API_KEY",
    "CEREBRAS_API_KEY",
];

const ENDPOINT_LIKE_ENV_VARS: &[&str] = &[
    "ANTHROPIC_BASE_URL",
    "OPENAI_BASE_URL",
    "OLLAMA_HOST",
    "LLAMA_CPP_SERVER_URL",
];

/// Config files whose mere *existence* is evidence a provider or harness has
/// been set up. Never opened, parsed, or written by this module — see the
/// module docs.
fn known_config_files(home: &std::path::Path) -> Vec<PathBuf> {
    vec![
        home.join(".claude.json"),
        home.join(".claude"),
        home.join(".codex"),
        home.join(".config").join("opencode"),
        home.join(".ollama"),
    ]
}

/// Presence-only evidence of provider/harness configuration.
///
/// Every field holds only variable *names* and file *paths* — never a
/// secret's value. See the module documentation for why that is a
/// structural property of this type, not just a habit followed by its
/// constructor.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct ProviderSignals {
    secret_vars_present: Vec<&'static str>,
    endpoint_vars_present: Vec<&'static str>,
    config_files_present: Vec<PathBuf>,
}

impl ProviderSignals {
    /// Names of secret-like environment variables (API keys, auth tokens)
    /// found set to a non-empty value. Report these as "set (value hidden)"
    /// — never fetch or display the value itself.
    pub fn secret_vars_present(&self) -> &[&'static str] {
        &self.secret_vars_present
    }

    /// Names of endpoint-like environment variables (base URLs, hosts) found
    /// set to a non-empty value. Unlike secrets, showing that these are set
    /// is informative and low-risk, but this type still only ever stores the
    /// *name*, never the URL/host value.
    pub fn endpoint_vars_present(&self) -> &[&'static str] {
        &self.endpoint_vars_present
    }

    /// Known provider/harness config files or directories that exist.
    pub fn config_files_present(&self) -> &[PathBuf] {
        &self.config_files_present
    }

    pub fn is_empty(&self) -> bool {
        self.secret_vars_present.is_empty()
            && self.endpoint_vars_present.is_empty()
            && self.config_files_present.is_empty()
    }
}

/// Manual `Debug` impl as a deliberate, defensive statement: even though no
/// field in this struct is capable of holding a secret *value* today, this
/// spells out exactly what gets printed so a future field addition cannot
/// silently start leaking one through a derived impl.
impl std::fmt::Debug for ProviderSignals {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderSignals")
            .field("secret_vars_present", &self.secret_vars_present)
            .field("endpoint_vars_present", &self.endpoint_vars_present)
            .field("config_files_present", &self.config_files_present)
            .finish()
    }
}

/// Run the full, non-destructive provider-signal pass against the real
/// process environment and filesystem.
pub fn detect() -> ProviderSignals {
    let env_pairs: Vec<(&'static str, String)> = SECRET_LIKE_ENV_VARS
        .iter()
        .chain(ENDPOINT_LIKE_ENV_VARS.iter())
        .map(|&name| (name, std::env::var(name).unwrap_or_default()))
        .collect();
    let (secret_vars_present, endpoint_vars_present) = classify_env_pairs(&env_pairs);

    let config_files_present = match home_dir() {
        Some(home) => known_config_files(&home)
            .into_iter()
            .filter(|p| p.exists())
            .collect(),
        None => Vec::new(),
    };

    ProviderSignals {
        secret_vars_present,
        endpoint_vars_present,
        config_files_present,
    }
}

/// Classify a set of `(name, value)` pairs into the secret-like and
/// endpoint-like names that are present (non-empty). Injectable by design —
/// callers pass in observed `(name, value)` pairs instead of this function
/// reading `std::env` itself — so tests can exercise classification without
/// mutating real process environment variables (which would race across
/// tests running in parallel).
///
/// An empty value is treated the same as "unset": a variable exported as
/// `FOO=` is not meaningfully configured.
fn classify_env_pairs(pairs: &[(&'static str, String)]) -> (Vec<&'static str>, Vec<&'static str>) {
    let mut secret = Vec::new();
    let mut endpoint = Vec::new();
    for &(name, ref value) in pairs {
        if value.is_empty() {
            continue;
        }
        if SECRET_LIKE_ENV_VARS.contains(&name) {
            secret.push(name);
        } else if ENDPOINT_LIKE_ENV_VARS.contains(&name) {
            endpoint.push(name);
        }
    }
    (secret, endpoint)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_values_are_treated_as_absent() {
        let pairs = vec![
            ("ANTHROPIC_API_KEY", String::new()),
            ("OLLAMA_HOST", String::new()),
        ];
        let (secret, endpoint) = classify_env_pairs(&pairs);
        assert!(secret.is_empty());
        assert!(endpoint.is_empty());
    }

    #[test]
    fn secret_and_endpoint_vars_are_classified_separately() {
        let pairs = vec![
            ("ANTHROPIC_API_KEY", "sk-super-secret-value".to_string()),
            ("OPENAI_API_KEY", "sk-another-secret".to_string()),
            ("OLLAMA_HOST", "http://127.0.0.1:11434".to_string()),
            ("ANTHROPIC_BASE_URL", "https://example.invalid".to_string()),
        ];
        let (secret, endpoint) = classify_env_pairs(&pairs);
        assert_eq!(secret, vec!["ANTHROPIC_API_KEY", "OPENAI_API_KEY"]);
        assert_eq!(endpoint, vec!["OLLAMA_HOST", "ANTHROPIC_BASE_URL"]);
    }

    #[test]
    fn unknown_variable_names_are_ignored() {
        let pairs = vec![("SOME_RANDOM_VAR", "value".to_string())];
        let (secret, endpoint) = classify_env_pairs(&pairs);
        assert!(secret.is_empty());
        assert!(endpoint.is_empty());
    }

    /// The one test this module exists to make pass: build signals from an
    /// env var carrying an actual secret-shaped value, and assert that value
    /// never appears in `Debug` output — only the variable *name* does.
    #[test]
    fn debug_output_never_contains_a_secret_value() {
        const SECRET_VALUE: &str = "sk-ant-totally-real-looking-secret-xyz123";
        let pairs = vec![("ANTHROPIC_API_KEY", SECRET_VALUE.to_string())];
        let (secret_vars_present, endpoint_vars_present) = classify_env_pairs(&pairs);
        let signals = ProviderSignals {
            secret_vars_present,
            endpoint_vars_present,
            config_files_present: Vec::new(),
        };

        let debug_output = format!("{signals:?}");
        assert!(!debug_output.contains(SECRET_VALUE));
        assert!(debug_output.contains("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn known_config_files_are_scoped_under_the_given_home() {
        let home = std::path::Path::new("/fake/home");
        let files = known_config_files(home);
        assert!(files.iter().all(|p| p.starts_with(home)));
        assert!(files.contains(&home.join(".claude.json")));
        assert!(files.contains(&home.join(".config").join("opencode")));
    }

    #[test]
    fn detect_runs_without_panicking() {
        // Exercises the real env/filesystem path end to end. Cannot assert
        // specific content since it depends on the machine running the test,
        // only that it completes and yields a well-formed value.
        let signals = detect();
        let _ = signals.is_empty();
    }
}
