//! `glasshouse migrate-gateway-state` — moving account and broker state out
//! of Glasshouse's own store and into the gateway's.
//!
//! User ruling, 2026-09-11. Everything this command moves was Glasshouse's
//! before that date and is the gateway's after it: the five account keys of
//! each `[entitlements.<name>]` table, and the five directories under
//! Glasshouse's data directory that hold brokers, the broker's executable,
//! the model catalogue cache and the gateway's own quota and health caches.
//!
//! # Three rules, and each one is about not losing anything
//!
//! **Nothing is written until nothing can refuse.** The plan is built whole
//! first — every account compared against what `gateway.toml` already holds,
//! every destination directory checked — and a single refusal stops the run
//! before the first byte. A migration that wrote half of itself and then
//! stopped would leave a user with an account in two files and no way to
//! tell which one is in force.
//!
//! **A rewrite of a file a person wrote keeps what they wrote.** The
//! `config.toml` rewrite is `toml_edit`'s, so comments, key order and
//! spacing survive; only the moved keys go, and a table left with nothing in
//! it goes with them. A backup is taken first and its name says what it is.
//!
//! **A move is a rename, never a copy-and-delete.** `std::fs::rename` either
//! moves the directory or fails, so there is no window in which the state
//! exists twice or not at all. It cannot cross a filesystem — and it does
//! not have to: a data directory and a gateway data directory that live on
//! different volumes is a case this reports rather than one it silently
//! turns into a copy.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use glasshouse::Runtime;
use glasshouse::RuntimePaths;
use glasshouse::config::entitlement::LegacyEntitlementConfig;
use glasshouse::config::{AccountEntry, GATEWAY_OWNED_DIRECTORIES, GatewayCatalogue};

/// What one configuration layer's `[entitlements]` and `[providers]` tables
/// held before the ruling — read with a permissive deserializer, because the
/// rest of the file is none of this command's business and a file it could
/// not parse whole is one it could not migrate at all.
#[derive(Debug, Default, serde::Deserialize)]
struct LegacyLayer {
    #[serde(default)]
    entitlements: BTreeMap<String, LegacyEntitlementConfig>,
}

/// One planned change to one configuration file.
struct LayerPlan {
    path: PathBuf,
    text: String,
    /// Entitlement name to the account keys being removed from it.
    removals: BTreeMap<String, Vec<&'static str>>,
}

/// One planned directory move.
struct MovePlan {
    source: PathBuf,
    destination: PathBuf,
}

/// The whole plan, built before anything is written.
#[derive(Default)]
struct Plan {
    /// Accounts to add to `gateway.toml`, by name.
    accounts: BTreeMap<String, AccountEntry>,
    /// Accounts already in `gateway.toml` with exactly this content.
    identical: Vec<String>,
    /// Accounts already in `gateway.toml` with different content — the
    /// refusal.
    conflicts: Vec<String>,
    layers: Vec<LayerPlan>,
    moves: Vec<MovePlan>,
    /// Destinations that exist and are not empty — the other refusal.
    occupied: Vec<PathBuf>,
    /// Broker processes still running against the old layout.
    running: Vec<String>,
}

impl Plan {
    fn is_empty(&self) -> bool {
        self.accounts.is_empty() && self.moves.is_empty()
    }

    fn refusals(&self) -> bool {
        !self.conflicts.is_empty() || !self.occupied.is_empty()
    }
}

/// Run the migration, or describe it. Returns the report; `main.rs` prints
/// it, so the whole of this command is testable without a terminal.
pub(crate) fn migrate(runtime: &Runtime, dry_run: bool) -> Result<String> {
    let paths = runtime.paths();
    let gateway_config_path = paths.gateway_config_path().to_path_buf();
    let existing = GatewayCatalogue::load(&gateway_config_path)?;

    let mut plan = Plan::default();
    plan_layers(runtime, &existing, &mut plan)?;
    plan_moves(paths, &mut plan);
    plan.running = running_brokers(paths);

    let mut out = String::new();
    // "would " on a dry run and nothing otherwise, so a real run does not
    // print every line with a leading space.
    let verb = if dry_run { "would " } else { "" };

    if plan.refusals() {
        for name in &plan.conflicts {
            out.push_str(&format!(
                "refused: `{name}` is already an account in {} with different content; \
                 reconcile the two by hand, then run this again\n",
                gateway_config_path.display()
            ));
        }
        for path in &plan.occupied {
            out.push_str(&format!(
                "refused: `{}` already exists and is not empty; move or remove it, then run \
                 this again\n",
                path.display()
            ));
        }
        out.push_str(
            "nothing was changed: a migration that wrote half of itself would leave \
                      state in two places\n",
        );
        bail!("{out}");
    }

    for name in &plan.identical {
        out.push_str(&format!(
            "skipped: account `{name}` is already in {} with the same content\n",
            gateway_config_path.display()
        ));
    }

    if plan.is_empty() {
        out.push_str(&format!(
            "nothing to move: no `[entitlements.*]` table names an account key and no legacy \
             directory is left under {}\n",
            paths.data_dir().display()
        ));
        return Ok(out);
    }

    // ---- gateway.toml ---------------------------------------------------
    if !plan.accounts.is_empty() {
        let merged = merge_into_gateway(&gateway_config_path, &plan.accounts)?;
        for name in plan.accounts.keys() {
            out.push_str(&format!(
                "{verb}account `{name}` -> {}\n",
                gateway_config_path.display()
            ));
        }
        if !dry_run {
            write_gateway(&gateway_config_path, &merged)?;
        }
    }

    // ---- the configuration files ---------------------------------------
    let stamp = utc_stamp(now_unix());
    for layer in &plan.layers {
        let rewritten = rewrite_layer(&layer.text, &layer.removals)?;
        let backup = backup_path(&layer.path, &stamp);
        out.push_str(&format!(
            "{verb}back up {} -> {}\n",
            layer.path.display(),
            backup.display()
        ));
        for (name, keys) in &layer.removals {
            out.push_str(&format!(
                "{verb}remove {} from `[entitlements.{name}]` in {}\n",
                keys.join(", "),
                layer.path.display()
            ));
        }
        if !dry_run {
            std::fs::copy(&layer.path, &backup)
                .with_context(|| format!("could not back up `{}`", layer.path.display()))?;
            std::fs::write(&layer.path, &rewritten)
                .with_context(|| format!("could not rewrite `{}`", layer.path.display()))?;
        }
    }

    // ---- the directories -------------------------------------------------
    for one in &plan.moves {
        out.push_str(&format!(
            "{verb}move {} -> {}\n",
            one.source.display(),
            one.destination.display()
        ));
        if !dry_run {
            if let Some(parent) = one.destination.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("could not create `{}`", parent.display()))?;
            }
            // An empty destination left by an earlier partial state is
            // removed first: `rename` refuses a destination that exists on
            // some platforms whatever it holds, and this one was checked
            // empty when the plan was built.
            if one.destination.exists() {
                let _ = std::fs::remove_dir(&one.destination);
            }
            std::fs::rename(&one.source, &one.destination).with_context(|| {
                format!(
                    "could not move `{}` to `{}`. A rename cannot cross a filesystem; if these \
                     are on different volumes, copy the directory across by hand and remove the \
                     original",
                    one.source.display(),
                    one.destination.display()
                )
            })?;
        }
    }

    for line in &plan.running {
        out.push_str(&format!(
            "warning: {line} is still running against the old layout; restart that session so \
             it reads the gateway's directories\n"
        ));
    }

    if dry_run {
        out.push_str("dry run: nothing was changed\n");
    }
    Ok(out)
}

// -------------------------------------------------------------------------
// Planning
// -------------------------------------------------------------------------

/// Read both configuration layers in their **legacy** shape and plan what
/// comes out of each.
fn plan_layers(runtime: &Runtime, existing: &GatewayCatalogue, plan: &mut Plan) -> Result<()> {
    let mut files = vec![runtime.paths().user_config_file()];
    if let Ok(project) = glasshouse::config::project_config_path(runtime.project()) {
        files.push(project);
    }

    for path in files {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let layer: LegacyLayer = toml::from_str(&text)
            .with_context(|| format!("could not read `{}`", path.display()))?;
        let mut removals: BTreeMap<String, Vec<&'static str>> = BTreeMap::new();
        for (name, entry) in &layer.entitlements {
            if !entry.has_account_keys() {
                continue;
            }
            let account = entry.to_account();
            match existing.account(name) {
                Some(there) if same_account(there, &account)? => {
                    if !plan.identical.contains(name) {
                        plan.identical.push(name.clone());
                    }
                }
                Some(_) => {
                    if !plan.conflicts.contains(name) {
                        plan.conflicts.push(name.clone());
                    }
                }
                None => {
                    // Two layers naming one account is not a conflict: the
                    // project layer wins whole, exactly as it does when the
                    // two are resolved, so the later read replaces.
                    plan.accounts.insert(name.clone(), account);
                }
            }
            removals.insert(name.clone(), entry.account_keys_present());
        }
        if !removals.is_empty() {
            plan.layers.push(LayerPlan {
                path,
                text,
                removals,
            });
        }
    }
    Ok(())
}

/// Whether two accounts say exactly the same thing, compared through their
/// written form — the same bytes the gateway would read.
fn same_account(a: &AccountEntry, b: &AccountEntry) -> Result<bool> {
    Ok(toml::to_string(a)? == toml::to_string(b)?)
}

/// Which of the five directories exist under Glasshouse's data directory,
/// and whether each destination is free.
fn plan_moves(paths: &RuntimePaths, plan: &mut Plan) {
    for (from, to) in GATEWAY_OWNED_DIRECTORIES {
        let source = paths.data_dir().join(from);
        let destination = paths.gateway_data_dir().join(to);
        if source == destination || !source.is_dir() {
            continue;
        }
        if is_non_empty_dir(&destination) {
            plan.occupied.push(destination);
            continue;
        }
        plan.moves.push(MovePlan {
            source,
            destination,
        });
    }
}

fn is_non_empty_dir(path: &Path) -> bool {
    match std::fs::read_dir(path) {
        Ok(mut entries) => entries.next().is_some(),
        Err(_) => path.exists(),
    }
}

/// Broker processes still pointed at the old layout.
///
/// A warning and never a refusal: a running CLIProxyAPI holds its
/// configuration open and keeps serving from it, so moving the directory
/// under it neither corrupts anything nor is noticed — the session simply
/// goes on reading a path that no longer exists, which is exactly the thing
/// a person needs to be told rather than stopped for.
#[cfg(unix)]
fn running_brokers(paths: &RuntimePaths) -> Vec<String> {
    let old = paths.data_dir().join("subscription-brokers");
    let Ok(output) = std::process::Command::new("ps")
        .args(["-axo", "pid,command"])
        .output()
    else {
        return Vec::new();
    };
    let listing = String::from_utf8_lossy(&output.stdout);
    let old = old.to_string_lossy().to_string();
    listing
        .lines()
        .filter(|line| line.to_ascii_lowercase().contains("cliproxyapi"))
        .filter(|line| line.contains(&old))
        .map(|line| format!("`{}`", line.trim()))
        .collect()
}

#[cfg(not(unix))]
fn running_brokers(_paths: &RuntimePaths) -> Vec<String> {
    Vec::new()
}

// -------------------------------------------------------------------------
// Writing
// -------------------------------------------------------------------------

/// `gateway.toml` with `accounts` merged in, as text.
///
/// Format-preserving, like the configuration rewrite and for the same
/// reason: a user who has hand-written a `gateway.toml` with comments in it
/// keeps them.
fn merge_into_gateway(path: &Path, accounts: &BTreeMap<String, AccountEntry>) -> Result<String> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = existing
        .parse()
        .with_context(|| format!("could not read `{}` as TOML", path.display()))?;

    let table = doc
        .as_table_mut()
        .entry("accounts")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    let table = table
        .as_table_mut()
        .context("`accounts` in the gateway configuration is not a table")?;
    table.set_implicit(true);

    for (name, account) in accounts {
        table.insert(name, toml_edit::Item::Table(account_table(account)?));
    }
    Ok(doc.to_string())
}

/// One [`AccountEntry`] as a `toml_edit` table, with its credential written
/// the way the gateway's own documentation writes it —
/// `credential = { env = "VAR" }`, an inline table rather than a sub-table,
/// so a person opening the file sees the shape they were told to write.
fn account_table(account: &AccountEntry) -> Result<toml_edit::Table> {
    let rendered = toml::to_string(account).context("could not render an account")?;
    let parsed: toml_edit::DocumentMut = rendered
        .parse()
        .context("could not re-read a rendered account")?;
    let mut table = parsed.as_table().clone();
    if let Some(item) = table.get_mut("credential")
        && let Some(sub) = item.as_table()
    {
        let mut inline = sub.clone().into_inline_table();
        // Replacing a sub-table with a value loses the spacing on both sides
        // of the `=`, which would render `credential= { … }`. The gateway's
        // documentation writes `credential = { env = "VAR" }` and a file a
        // person opens should look like the file they were told to write.
        inline.decor_mut().set_prefix(" ");
        *item = toml_edit::Item::Value(toml_edit::Value::InlineTable(inline));
    }
    if let Some(mut key) = table.key_mut("credential") {
        key.leaf_decor_mut().set_suffix(" ");
    }
    table.set_implicit(false);
    Ok(table)
}

fn write_gateway(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create `{}`", parent.display()))?;
    }
    std::fs::write(path, text).with_context(|| format!("could not write `{}`", path.display()))
}

/// `text` with each named key removed from its `[entitlements.<name>]`
/// table, a table left empty removed, and `[entitlements]` itself removed
/// when nothing is left under it.
fn rewrite_layer(text: &str, removals: &BTreeMap<String, Vec<&'static str>>) -> Result<String> {
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .context("could not re-read the configuration")?;
    {
        let Some(entitlements) = doc
            .as_table_mut()
            .get_mut("entitlements")
            .and_then(toml_edit::Item::as_table_like_mut)
        else {
            return Ok(doc.to_string());
        };
        let mut emptied = Vec::new();
        for (name, keys) in removals {
            let Some(entry) = entitlements
                .get_mut(name)
                .and_then(toml_edit::Item::as_table_like_mut)
            else {
                continue;
            };
            for key in keys {
                entry.remove(key);
            }
            if entry.is_empty() {
                emptied.push(name.clone());
            }
        }
        for name in emptied {
            entitlements.remove(&name);
        }
        if entitlements.is_empty() {
            doc.as_table_mut().remove("entitlements");
        }
    }
    Ok(doc.to_string())
}

/// `<path>.before-gateway-store-<UTC stamp>`, beside the file it backs up.
fn backup_path(path: &Path, stamp: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".before-gateway-store-{stamp}"));
    path.with_file_name(name)
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

/// `YYYYMMDDTHHMMSSZ` from a Unix second.
///
/// Written out rather than depended on: one date conversion is cheaper than
/// a date crate in a workspace that has none, and a backup file's name is
/// the only thing in this binary that needs one.
fn utc_stamp(unix: i64) -> String {
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

/// Howard Hinnant's `civil_from_days`, the standard shift-to-March
/// derivation, valid for every date this will ever be handed.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rewrite keeps a comment, drops only the named keys, and removes
    /// a table the removal left empty — the three things a person who opens
    /// the file afterwards checks.
    #[test]
    fn the_rewrite_keeps_comments_and_drops_only_what_moved() {
        let text = "\
version = 1

# My own note about this account.
[entitlements.claude-a]
kind = \"claude\"
subscription_broker = \"cliproxyapi\"
native_harness = \"claude-code\"
deny_tiers = [\"frontier\"]

[entitlements.spare]
provider = \"openrouter\"
";
        let mut removals = BTreeMap::new();
        removals.insert("claude-a".to_owned(), vec!["kind", "subscription_broker"]);
        removals.insert("spare".to_owned(), vec!["provider"]);

        let out = rewrite_layer(text, &removals).expect("rewrites");
        assert!(
            out.contains("# My own note about this account."),
            "the comment survives:\n{out}"
        );
        assert!(!out.contains("kind ="), "{out}");
        assert!(!out.contains("subscription_broker"), "{out}");
        assert!(out.contains("native_harness = \"claude-code\""), "{out}");
        assert!(out.contains("deny_tiers"), "{out}");
        assert!(
            !out.contains("[entitlements.spare]"),
            "a table with nothing left in it goes:\n{out}"
        );
        assert!(out.starts_with("version = 1"), "{out}");
    }

    /// Removing the last entitlement removes `[entitlements]` with it,
    /// rather than leaving a header with nothing under it.
    #[test]
    fn an_entitlements_table_with_nothing_left_is_dropped() {
        let text = "version = 1\n\n[entitlements.only]\nprovider = \"openrouter\"\n";
        let mut removals = BTreeMap::new();
        removals.insert("only".to_owned(), vec!["provider"]);
        let out = rewrite_layer(text, &removals).expect("rewrites");
        assert!(!out.contains("entitlements"), "{out}");
        assert_eq!(out.trim(), "version = 1");
    }

    /// The merge writes `[accounts.<name>]` with the credential as an inline
    /// table, and leaves everything already in the file alone.
    #[test]
    fn the_merge_adds_accounts_and_keeps_what_was_there() {
        let scratch = tempfile::tempdir().expect("a scratch directory");
        let path = scratch.path().join("gateway.toml");
        std::fs::write(
            &path,
            "# a gateway I wrote myself\n[providers.alpha]\nbase_url = \"http://127.0.0.1:1\"\n",
        )
        .expect("written");

        let mut accounts = BTreeMap::new();
        let mut entry = AccountEntry::default();
        entry
            .set_kind(Some(glasshouse::config::EntitlementKind::ApiKey))
            .set_credential(Some(
                glasshouse::config::EntitlementCredential::environment("ALPHA_KEY"),
            ))
            .set_provider(Some("alpha".to_owned()));
        accounts.insert("alpha-key".to_owned(), entry);

        let merged = merge_into_gateway(&path, &accounts).expect("merges");
        assert!(merged.contains("# a gateway I wrote myself"), "{merged}");
        assert!(merged.contains("[providers.alpha]"), "{merged}");
        assert!(merged.contains("[accounts.alpha-key]"), "{merged}");
        assert!(
            merged.contains("credential = { env = \"ALPHA_KEY\" }"),
            "the shape the gateway documents:\n{merged}"
        );

        // And the gateway can read back what was written.
        let catalogue = GatewayCatalogue::from_toml(&merged).expect("the gateway parses it");
        assert!(catalogue.account("alpha-key").is_some());
        assert_eq!(
            catalogue.account("alpha-key").and_then(|a| a.provider()),
            Some("alpha")
        );
    }

    /// The backup's name says what it is and when, beside the file it backs
    /// up.
    #[test]
    fn the_backup_is_named_for_what_it_precedes() {
        let stamp = utc_stamp(1_789_000_000);
        assert_eq!(stamp, "20260910T002640Z");
        let backup = backup_path(Path::new("/x/config.toml"), &stamp);
        assert_eq!(
            backup,
            PathBuf::from("/x/config.toml.before-gateway-store-20260910T002640Z")
        );
    }

    /// The epoch and a leap day, so the derivation is checked at both ends
    /// rather than only where it happens to be used.
    #[test]
    fn the_date_derivation_is_right_at_the_epoch_and_on_a_leap_day() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // 2024-02-29.
        assert_eq!(utc_stamp(1_709_164_800), "20240229T000000Z");
    }
}
