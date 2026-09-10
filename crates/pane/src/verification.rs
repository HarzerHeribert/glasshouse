//! Named, confined verification commands. Reuse is explicit and scoped to a
//! request and declared file inputs; a reused observation is never a fresh run.
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::sandbox::profile::{Access, Profile};
use crate::tools::invoke::{self, Args, CancellationToken, ToolContext};

const CONFIG_BYTES: u64 = 64 * 1024;
const FILE_BYTES: u64 = 1024 * 1024;
const SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;
const NODES: usize = 4096;
const OUTPUT_CHARS: usize = 8000;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct CheckConfig {
    #[serde(default)]
    pub checker: Vec<String>,
    #[serde(default)]
    pub checks: BTreeMap<String, CheckSpec>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CheckSpec {
    pub command: String,
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(default)]
    pub reuse: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    pub name: String,
    pub command: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub observed_at_ms: u64,
    pub executed: bool,
    pub reused: bool,
    pub reuse_scope: String,
}

#[derive(Default)]
pub struct Verification {
    successes: BTreeMap<String, (String, CheckResult)>,
}

fn bounded_read(profile: &Profile, path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let path = profile
        .check("checks", Access::Read, path)
        .map_err(|e| e.to_string())?;
    if !std::fs::metadata(&path)
        .map_err(|e| e.to_string())?
        .is_file()
    {
        return Err("verification input must be an ordinary file".into());
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let file = options.open(path).map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("verification input must be an ordinary file".into());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("verification input exceeds the bounded read limit".into());
    }
    Ok(bytes)
}

pub fn load(profile: &Profile) -> Result<CheckConfig, String> {
    let path = profile.root().join(".glasshouse/checks.toml");
    let admitted = profile
        .check("checks", Access::Read, &path)
        .map_err(|e| e.to_string())?;
    if !admitted.exists() {
        return Ok(CheckConfig::default());
    }
    let bytes = bounded_read(profile, &path, CONFIG_BYTES)?;
    let config: CheckConfig =
        toml::from_str(std::str::from_utf8(&bytes).map_err(|e| e.to_string())?)
            .map_err(|e| format!("checks.toml: {e}"))?;
    if config.checks.len() > 16 || config.checker.len() > 4 {
        return Err("checks.toml: at most 16 named checks and four checker checks".into());
    }
    for (name, spec) in &config.checks {
        if name.is_empty()
            || name.len() > 64
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            || spec.command.trim().is_empty()
            || spec.command.len() > 4096
            || spec.inputs.len() > 32
        {
            return Err("checks.toml: invalid name, command or input count".into());
        }
        if spec.reuse && spec.inputs.is_empty() {
            return Err(format!(
                "checks.toml: reusable check `{name}` requires explicit inputs"
            ));
        }
    }
    if config
        .checker
        .iter()
        .any(|name| !config.checks.contains_key(name))
    {
        return Err("checks.toml: checker names an undefined check".into());
    }
    Ok(config)
}

fn fingerprint(profile: &Profile, spec: &CheckSpec, token: &CancellationToken) -> Option<String> {
    if !spec.reuse || spec.inputs.is_empty() {
        return None;
    }
    let mut hash = Sha256::new();
    hash.update(serde_json::to_vec(spec).ok()?);
    hash.update(profile.root().as_os_str().as_encoded_bytes());
    let mut env: Vec<_> = std::env::vars_os().collect();
    env.sort();
    for (key, value) in env {
        hash.update(key.as_encoded_bytes());
        hash.update([0]);
        hash.update(value.as_encoded_bytes());
        hash.update([0]);
    }
    let mut stack: Vec<PathBuf> = spec.inputs.iter().map(|p| profile.root().join(p)).collect();
    let mut nodes = 0;
    let mut bytes = 0;
    while let Some(path) = stack.pop() {
        nodes += 1;
        if nodes > NODES || token.is_cancelled() {
            return None;
        }
        let admitted = profile.check("checks", Access::Read, &path).ok()?;
        let metadata = std::fs::symlink_metadata(&path).ok()?;
        // A dependency outside the declared tree or a special file cannot
        // support a trustworthy reuse key. Fresh execution remains available.
        if metadata.file_type().is_symlink() {
            return None;
        }
        hash.update(admitted.as_os_str().as_encoded_bytes());
        hash.update([0]);
        if metadata.is_dir() {
            let mut children = Vec::new();
            for entry in std::fs::read_dir(admitted).ok()? {
                if nodes + stack.len() + children.len() >= NODES {
                    return None;
                }
                children.push(entry.ok()?.path());
            }
            children.sort();
            stack.extend(children);
        } else if metadata.is_file() {
            let content = bounded_read(profile, &path, FILE_BYTES).ok()?;
            bytes += content.len();
            if bytes > SNAPSHOT_BYTES {
                return None;
            }
            hash.update(content.len().to_le_bytes());
            hash.update(content);
        } else {
            return None;
        }
    }
    Some(format!("{:x}", hash.finalize()))
}

fn output_tail(text: String) -> String {
    let count = text.chars().count();
    if count <= OUTPUT_CHARS {
        return text;
    }
    let tail: String = text.chars().skip(count - OUTPUT_CHARS).collect();
    format!(
        "[checks: {} characters omitted; original output tail]\n{tail}",
        count - OUTPUT_CHARS
    )
}

impl Verification {
    pub fn run(
        &mut self,
        name: &str,
        force: bool,
        ctx: &ToolContext<'_>,
        token: &CancellationToken,
    ) -> Result<CheckResult, String> {
        if token.is_cancelled() {
            return Err("verification cancelled before execution".into());
        }
        let config = load(ctx.profile)?;
        let spec = config.checks.get(name).ok_or_else(|| {
            format!(
                "No check `{name}` in .glasshouse/checks.toml; define its command before running it"
            )
        })?;
        // Recheck admission even for a cache hit; a result is no authority.
        ctx.profile
            .admits_command(&spec.command)
            .map_err(|e| e.to_string())?;
        let before = fingerprint(ctx.profile, spec, token);
        if !force
            && let Some(key) = &before
            && let Some((saved, result)) = self.successes.get(name)
            && key == saved
        {
            if token.is_cancelled() {
                return Err("verification cancelled before reuse".into());
            }
            let mut result = result.clone();
            result.executed = false;
            result.reused = true;
            return Ok(result);
        }
        self.successes.remove(name);
        let result = invoke::run_cancellable(
            ctx,
            token,
            "bash",
            &Args::new().with("command", &spec.command),
        )
        .map_err(|e| e.to_string())?;
        let after = fingerprint(ctx.profile, spec, token);
        if token.is_cancelled() {
            return Err("verification cancelled during execution".into());
        }
        let observation = CheckResult {
            name: name.into(),
            command: spec.command.clone(),
            stdout: output_tail(result.stdout),
            stderr: output_tail(result.stderr),
            exit_code: result.exit_code,
            observed_at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            executed: true,
            reused: false,
            reuse_scope: if before.is_some() && before == after {
                "declared input contents and process environment; external dependencies and nondeterminism require force=true".into()
            } else {
                "not reusable: no complete unchanged declared-input snapshot".into()
            },
        };
        if observation.exit_code == Some(0)
            && let Some(key) = before
            && Some(&key) == after.as_ref()
        {
            self.successes
                .insert(name.into(), (key, observation.clone()));
        }
        Ok(observation)
    }
}
