//! User-owned subscription broker lifecycle.
//!
//! Tokens stay inside CLIProxyAPI's per-account auth directories. This module
//! creates those directories, launches the broker's OAuth entry points, and
//! reports directory-entry presence without reading credential contents.

use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use glasshouse::cli::SubscriptionProvider;
use glasshouse::config::{self, EffectiveConfig, EntitlementKind, EntitlementVendor, UserConfig};
use glasshouse::{Runtime, RuntimePaths, shutdown};
use sha2::{Digest, Sha256};

const BROKER_BINARY_ENV: &str = "GLASSHOUSE_CLIPROXYAPI_BIN";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

pub(crate) fn status(runtime: &Runtime) -> Result<String> {
    let entitlements = configured_entitlements(runtime)?;
    let mut rows = Vec::new();
    for entitlement in entitlements {
        if entitlement.backing().subscription_broker()
            != Some(glasshouse::config::SubscriptionBroker::CliProxyApi)
        {
            continue;
        }
        let Some(provider) = provider_for_entitlement(entitlement.kind(), entitlement.vendor())
        else {
            continue;
        };
        validate_account_ancestors(runtime.paths(), entitlement.name())?;
        let state = if auth_present(
            &runtime
                .paths()
                .subscription_broker_auth_dir(entitlement.name()),
        )? {
            "present"
        } else {
            "absent"
        };
        rows.push(format!(
            "{}\t{}\t{state}",
            provider.as_str(),
            entitlement.name()
        ));
    }
    rows.sort();

    let mut out = String::from("provider\tentitlement\tstatus\n");
    for row in rows {
        out.push_str(&row);
        out.push('\n');
    }
    Ok(out)
}

pub(crate) fn login(
    runtime: &Runtime,
    provider: SubscriptionProvider,
    entitlement: &str,
) -> Result<String> {
    validate_entitlement(runtime, provider, entitlement)?;
    validate_account_ancestors(runtime.paths(), entitlement)?;
    let binary = resolve_broker_binary(runtime.paths(), std::env::var_os(BROKER_BINARY_ENV))?;
    let account = prepare_account(runtime.paths(), entitlement)?;
    let flag = login_flag(provider);

    run_login_process(
        &binary,
        &account,
        [
            OsStr::new("-config"),
            account.config.as_os_str(),
            OsStr::new(flag),
        ],
        LOGIN_TIMEOUT,
    )?;
    if !auth_present(&runtime.paths().subscription_broker_auth_dir(entitlement))? {
        bail!(
            "CLIProxyAPI login exited without creating authentication for entitlement `{entitlement}`"
        );
    }
    Ok(format!("{}\t{entitlement}\tpresent\n", provider.as_str()))
}

fn login_flag(provider: SubscriptionProvider) -> &'static str {
    match provider {
        SubscriptionProvider::Google => "-antigravity-login",
        SubscriptionProvider::Anthropic => "-claude-login",
        SubscriptionProvider::Openai => "-codex-login",
    }
}

pub(crate) fn logout(
    runtime: &Runtime,
    provider: SubscriptionProvider,
    entitlement: &str,
) -> Result<String> {
    validate_entitlement(runtime, provider, entitlement)?;
    validate_account_ancestors(runtime.paths(), entitlement)?;
    let dir = runtime.paths().subscription_broker_auth_dir(entitlement);
    remove_auth_dir(&dir)?;
    ensure_private_dir(&dir)?;
    Ok(format!("{}\t{entitlement}\tabsent\n", provider.as_str()))
}

pub(crate) fn adopt_binary(runtime: &Runtime, source: &Path) -> Result<PathBuf> {
    adopt(runtime.paths(), source)
}

fn configured_entitlements(
    runtime: &Runtime,
) -> Result<Vec<glasshouse::config::ResolvedEntitlement>> {
    let user = UserConfig::load(runtime.paths())?;
    let project = config::load_project_config(runtime.project())?;
    Ok(EffectiveConfig::new(&user, project.as_ref()).configured_entitlements()?)
}

fn validate_entitlement(
    runtime: &Runtime,
    provider: SubscriptionProvider,
    name: &str,
) -> Result<()> {
    let entitlements = configured_entitlements(runtime)?;
    let entitlement = entitlements
        .iter()
        .find(|entry| entry.name() == name)
        .with_context(|| format!("entitlement `{name}` is not configured"))?;
    if entitlement.backing().subscription_broker()
        != Some(glasshouse::config::SubscriptionBroker::CliProxyApi)
    {
        bail!("entitlement `{name}` is not backed by CLIProxyAPI");
    }
    validate_kind_vendor(provider, name, entitlement.kind(), entitlement.vendor())
}

fn validate_kind_vendor(
    provider: SubscriptionProvider,
    name: &str,
    kind: Option<EntitlementKind>,
    vendor: Option<EntitlementVendor>,
) -> Result<()> {
    let expected_kind = match provider {
        SubscriptionProvider::Google => EntitlementKind::Gemini,
        SubscriptionProvider::Anthropic => EntitlementKind::Claude,
        SubscriptionProvider::Openai => EntitlementKind::ChatGpt,
    };
    let expected_vendor = match provider {
        SubscriptionProvider::Google => EntitlementVendor::Google,
        SubscriptionProvider::Anthropic => EntitlementVendor::Claude,
        SubscriptionProvider::Openai => EntitlementVendor::OpenAi,
    };
    if let Some(actual) = kind
        && actual != expected_kind
    {
        bail!(
            "entitlement `{name}` has kind `{}`, not `{}` for {}",
            actual.as_str(),
            expected_kind.as_str(),
            provider.as_str()
        );
    }
    if let Some(actual) = vendor
        && actual != expected_vendor
    {
        bail!(
            "entitlement `{name}` has vendor `{}`, not `{}` for {}",
            actual.as_str(),
            expected_vendor.as_str(),
            provider.as_str()
        );
    }
    Ok(())
}

fn provider_for_entitlement(
    kind: Option<EntitlementKind>,
    vendor: Option<EntitlementVendor>,
) -> Option<SubscriptionProvider> {
    match (kind, vendor) {
        (Some(EntitlementKind::Claude), None | Some(EntitlementVendor::Claude))
        | (None, Some(EntitlementVendor::Claude)) => Some(SubscriptionProvider::Anthropic),
        (Some(EntitlementKind::ChatGpt), None | Some(EntitlementVendor::OpenAi))
        | (None, Some(EntitlementVendor::OpenAi)) => Some(SubscriptionProvider::Openai),
        (Some(EntitlementKind::Gemini), None | Some(EntitlementVendor::Google))
        | (None, Some(EntitlementVendor::Google)) => Some(SubscriptionProvider::Google),
        _ => None,
    }
}

struct AccountPaths {
    root: PathBuf,
    config: PathBuf,
}

fn prepare_account(paths: &RuntimePaths, entitlement: &str) -> Result<AccountPaths> {
    let root = paths.subscription_broker_entitlement_dir(entitlement);
    let auth = paths.subscription_broker_auth_dir(entitlement);
    let config = root.join("cliproxyapi.json");
    ensure_private_dir(&paths.subscription_brokers_dir())?;
    ensure_private_dir(&root)?;
    ensure_private_dir(&auth)?;
    let document = serde_json::to_vec(&serde_json::json!({ "auth-dir": auth }))?;
    atomic_write(&config, &document, private_file_permissions())?;
    Ok(AccountPaths { root, config })
}

fn auth_present(dir: &Path) -> Result<bool> {
    let metadata = match fs::symlink_metadata(dir) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err).with_context(|| format!("could not inspect `{dir:?}`")),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("subscription auth location `{dir:?}` is not a private directory");
    }
    for entry in fs::read_dir(dir).with_context(|| format!("could not inspect `{dir:?}`"))? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn validate_account_ancestors(paths: &RuntimePaths, entitlement: &str) -> Result<()> {
    for path in [
        paths.subscription_brokers_dir(),
        paths.subscription_broker_entitlement_dir(entitlement),
    ] {
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                bail!("subscription broker private directory `{path:?}` is not a real directory")
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("could not inspect `{path:?}`"));
            }
        }
    }
    Ok(())
}

fn remove_auth_dir(dir: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(dir) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err).with_context(|| format!("could not inspect `{dir:?}`")),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("subscription auth location `{dir:?}` is not a private directory");
    }
    let tombstone = dir.with_file_name(format!(
        ".auth-removed-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    fs::rename(dir, &tombstone)
        .with_context(|| format!("could not disconnect subscription at `{dir:?}`"))?;
    fs::remove_dir_all(&tombstone)
        .with_context(|| format!("could not remove disconnected subscription `{tombstone:?}`"))
}

fn run_login_process<'a>(
    binary: &Path,
    account: &AccountPaths,
    args: impl IntoIterator<Item = &'a OsStr>,
    timeout: Duration,
) -> Result<ExitStatus> {
    let mut command = Command::new(binary);
    command
        .args(args)
        .current_dir(&account.root)
        .env_clear()
        .stdin(Stdio::inherit())
        // CLIProxyAPI owns its OAuth UI. Glasshouse deliberately never reads,
        // copies, or reprints process output that could contain credentials.
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    for name in [
        "HOME",
        "PATH",
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "XDG_RUNTIME_DIR",
        "BROWSER",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("could not start CLIProxyAPI `{binary:?}`"))?;
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child
            .try_wait()
            .context("could not wait for CLIProxyAPI login")?
        {
            if status.success() {
                return Ok(status);
            }
            bail!("CLIProxyAPI login failed with {status}");
        }
        if shutdown::shutdown_requested() {
            let _ = child.kill();
            let _ = child.wait();
            bail!("CLIProxyAPI login cancelled");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "CLIProxyAPI login timed out after {} seconds",
                timeout.as_secs()
            );
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn tools_root(paths: &RuntimePaths) -> PathBuf {
    paths.data_dir().join("tools").join("cliproxyapi")
}

fn adopted_binary_name() -> &'static str {
    if cfg!(windows) {
        "cliproxyapi.exe"
    } else {
        "cliproxyapi"
    }
}

fn adopt(paths: &RuntimePaths, source: &Path) -> Result<PathBuf> {
    validate_executable(source)?;
    let mut input = File::open(source)
        .with_context(|| format!("could not open CLIProxyAPI executable `{source:?}`"))?;
    let mut bytes = Vec::new();
    input
        .read_to_end(&mut bytes)
        .with_context(|| format!("could not read CLIProxyAPI executable `{source:?}`"))?;
    let version = format!("sha256-{}", hex::encode(Sha256::digest(&bytes)));
    let root = tools_root(paths);
    let version_dir = root.join(&version);
    ensure_private_dir(&root)?;
    ensure_private_dir(&version_dir)?;
    let destination = version_dir.join(adopted_binary_name());
    if !destination.exists() {
        let temp = version_dir.join(format!(".adopt-{}-{}", std::process::id(), unique_suffix()));
        let permissions = fs::metadata(source)?.permissions();
        let result = (|| -> Result<()> {
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            output.write_all(&bytes)?;
            output.sync_all()?;
            fs::set_permissions(&temp, permissions)?;
            fs::rename(&temp, &destination)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result.with_context(|| format!("could not adopt CLIProxyAPI into `{version_dir:?}`"))?;
    }
    atomic_write(
        &root.join("current"),
        version.as_bytes(),
        private_file_permissions(),
    )?;
    Ok(destination)
}

fn resolve_broker_binary(paths: &RuntimePaths, override_path: Option<OsString>) -> Result<PathBuf> {
    if let Some(path) = override_path.filter(|value| !value.is_empty()) {
        let path = PathBuf::from(path);
        validate_executable(&path)?;
        return Ok(path);
    }
    let root = tools_root(paths);
    let version = fs::read_to_string(root.join("current")).with_context(|| {
        format!(
            "CLIProxyAPI is not adopted; run `glasshouse subscriptions adopt-binary <PATH>` or set {BROKER_BINARY_ENV}"
        )
    })?;
    if !version.strip_prefix("sha256-").is_some_and(|digest| {
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    }) {
        bail!("the adopted CLIProxyAPI version marker is invalid");
    }
    let path = root.join(version).join(adopted_binary_name());
    validate_executable(&path)?;
    Ok(path)
}

fn validate_executable(path: &Path) -> Result<()> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("CLIProxyAPI executable `{path:?}` is not readable"))?;
    if !metadata.is_file() {
        bail!("CLIProxyAPI executable `{path:?}` is not a regular file");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            bail!("CLIProxyAPI executable `{path:?}` is not executable");
        }
    }
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8], permissions: fs::Permissions) -> Result<()> {
    let parent = path
        .parent()
        .context("atomic destination has no parent directory")?;
    ensure_private_dir(parent)?;
    let temp = parent.join(format!(".write-{}-{}", std::process::id(), unique_suffix()));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::set_permissions(&temp, permissions)?;
        fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.with_context(|| format!("could not atomically write `{path:?}`"))
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

#[cfg(unix)]
fn private_file_permissions() -> fs::Permissions {
    use std::os::unix::fs::PermissionsExt;
    fs::Permissions::from_mode(0o600)
}

#[cfg(not(unix))]
fn private_file_permissions() -> fs::Permissions {
    let temp = std::env::temp_dir();
    fs::metadata(temp)
        .expect("the platform temporary directory has permissions")
        .permissions()
}

fn ensure_private_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
            .with_context(|| format!("could not create private directory `{path:?}`"))?;
        let metadata = fs::symlink_metadata(path)
            .with_context(|| format!("could not inspect private directory `{path:?}`"))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            bail!("subscription broker private directory `{path:?}` is not a real directory");
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("could not secure private directory `{path:?}`"))?;
    }
    #[cfg(not(unix))]
    fs::create_dir_all(path)
        .with_context(|| format!("could not create private directory `{path:?}`"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_paths_are_entitlement_separated_and_provider_independent() {
        let paths = RuntimePaths::new("/tmp/data", "/tmp/config");
        let a = paths.subscription_broker_auth_dir("personal");
        let b = paths.subscription_broker_auth_dir("work");
        let c = paths.subscription_broker_auth_dir("personal");
        assert_ne!(a, b);
        assert_eq!(a, c);
        assert!(a.ends_with("auth"));
        assert!(!a.to_string_lossy().contains("personal"));
    }

    #[test]
    fn status_checks_presence_without_reading_token_contents() {
        let temp = tempfile::tempdir().unwrap();
        let auth = temp.path().join("auth");
        ensure_private_dir(&auth).unwrap();
        assert!(!auth_present(&auth).unwrap());
        let token = auth.join("account.json");
        fs::write(&token, b"not even valid json").unwrap();
        assert!(auth_present(&auth).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn fake_binary_receives_exact_argv_and_its_output_is_not_captured() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let record = temp.path().join("argv");
        let fake = temp.path().join("cliproxyapi");
        fs::write(
            &fake,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf 'do-not-copy-this-token\\n'\n",
                record.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
        let paths = RuntimePaths::new(temp.path().join("data"), temp.path().join("config"));
        let account = prepare_account(&paths, "personal").unwrap();
        run_login_process(
            &fake,
            &account,
            [
                OsStr::new("-config"),
                account.config.as_os_str(),
                OsStr::new("-claude-login"),
            ],
            Duration::from_secs(2),
        )
        .unwrap();
        let args = fs::read_to_string(record).unwrap();
        assert_eq!(
            args,
            format!("-config\n{}\n-claude-login\n", account.config.display())
        );
        let config: serde_json::Value =
            serde_json::from_slice(&fs::read(&account.config).unwrap()).unwrap();
        assert_eq!(
            config["auth-dir"],
            account.root.join("auth").to_string_lossy().as_ref()
        );
    }

    #[cfg(unix)]
    #[test]
    fn adoption_is_content_addressed_atomic_and_preserves_executable_mode() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::write(&source, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o751)).unwrap();
        let paths = RuntimePaths::new(temp.path().join("data"), temp.path().join("config"));
        let adopted = adopt(&paths, &source).unwrap();
        assert_eq!(fs::read(&adopted).unwrap(), fs::read(&source).unwrap());
        assert_eq!(
            fs::metadata(&adopted).unwrap().permissions().mode() & 0o777,
            0o751
        );
        assert_eq!(resolve_broker_binary(&paths, None).unwrap(), adopted);
        assert_eq!(paths.cliproxyapi_executable(), adopted);
        assert!(
            adopted
                .parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("sha256-")
        );
    }

    #[cfg(unix)]
    #[test]
    fn adoption_refuses_a_non_executable_file() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::write(&source, b"not executable").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
        let paths = RuntimePaths::new(temp.path().join("data"), temp.path().join("config"));
        let error = adopt(&paths, &source).unwrap_err().to_string();
        assert!(error.contains("not executable"), "{error}");
    }

    #[test]
    fn provider_validation_rejects_a_cross_account_kind_or_vendor() {
        assert!(
            validate_kind_vendor(
                SubscriptionProvider::Anthropic,
                "wrong",
                Some(EntitlementKind::ChatGpt),
                Some(EntitlementVendor::OpenAi),
            )
            .is_err()
        );
        validate_kind_vendor(
            SubscriptionProvider::Google,
            "google",
            Some(EntitlementKind::Gemini),
            Some(EntitlementVendor::Google),
        )
        .unwrap();
    }

    #[test]
    fn every_provider_uses_cliproxyapis_exact_oauth_flag() {
        assert_eq!(
            login_flag(SubscriptionProvider::Google),
            "-antigravity-login"
        );
        assert_eq!(login_flag(SubscriptionProvider::Anthropic), "-claude-login");
        assert_eq!(login_flag(SubscriptionProvider::Openai), "-codex-login");
    }

    #[cfg(unix)]
    #[test]
    fn nonzero_and_timeout_are_bounded_without_child_output_in_errors() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::new(temp.path().join("data"), temp.path().join("config"));
        let account = prepare_account(&paths, "personal").unwrap();
        let failed = temp.path().join("failed");
        fs::write(
            &failed,
            b"#!/bin/sh\nprintf 'credential-shaped-output\\n'\nexit 7\n",
        )
        .unwrap();
        fs::set_permissions(&failed, fs::Permissions::from_mode(0o700)).unwrap();
        let error = run_login_process(
            &failed,
            &account,
            std::iter::empty(),
            Duration::from_secs(1),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("exit status: 7"), "{error}");
        assert!(!error.contains("credential-shaped-output"), "{error}");

        let hangs = temp.path().join("hangs");
        fs::write(&hangs, b"#!/bin/sh\nwhile :; do :; done\n").unwrap();
        fs::set_permissions(&hangs, fs::Permissions::from_mode(0o700)).unwrap();
        let started = Instant::now();
        let error = run_login_process(
            &hangs,
            &account,
            std::iter::empty(),
            Duration::from_millis(20),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("timed out"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
