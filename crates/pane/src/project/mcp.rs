//! Bounded parsing of the project's existing `.mcp.json` bytes.
use std::collections::BTreeMap;

use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Config {
    mcp_servers: BTreeMap<String, Server>,
}

/// Stdio configuration. No Debug implementation: argv and env may contain secrets.
#[derive(Deserialize)]
pub struct Server {
    #[serde(rename = "type", default)]
    transport: Option<String>,
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// Parse without returning source text or serde diagnostics to the model.
pub fn parse(raw: Option<&str>) -> Result<BTreeMap<String, Server>, &'static str> {
    let Some(raw) = raw else {
        return Ok(BTreeMap::new());
    };
    if raw.len() > 1024 * 1024 {
        return Err("MCP configuration exceeds 1 MiB");
    }
    let config: Config = serde_json::from_str(raw).map_err(|_| "invalid MCP configuration")?;
    if config.mcp_servers.len() > 16 {
        return Err("MCP configuration exceeds 16 servers");
    }
    let mut servers = BTreeMap::new();
    for (name, server) in config.mcp_servers {
        if !matches!(server.transport.as_deref(), None | Some("stdio")) {
            continue;
        }
        if name.is_empty()
            || name.len() > 128
            || name.contains("__")
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            || server
                .command
                .as_ref()
                .is_none_or(|s| s.is_empty() || s.len() > 4096 || s.contains('\0'))
            || server.args.len() > 64
            || server.env.len() > 64
            || server
                .args
                .iter()
                .any(|s| s.len() > 16 * 1024 || s.contains('\0'))
            || server.env.iter().any(|(k, v)| {
                k.is_empty()
                    || k.contains(['=', '\0'])
                    || k.len() > 256
                    || v.contains('\0')
                    || v.len() > 16 * 1024
            })
        {
            return Err("invalid stdio MCP server configuration");
        }
        servers.insert(name, server);
    }
    Ok(servers)
}
