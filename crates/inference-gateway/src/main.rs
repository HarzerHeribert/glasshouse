//! `inference-gateway` — the gateway as its own process.
//!
//! # The contract Pane holds this binary to
//!
//! ```text
//! inference-gateway serve [--listen 127.0.0.1:0] [--config <path>]
//! ```
//!
//! Long-running. It prints **exactly one line to stdout** when it is ready
//! to serve, and then nothing more on stdout ever:
//!
//! ```json
//! {"listening":"http://127.0.0.1:PORT","token":"<bearer>"}
//! ```
//!
//! The caller sets its client's base URL to `listening` and its bearer token
//! to `token`, then serves through it. This process exits `0` when **stdin
//! reaches EOF** or a termination signal arrives, and every diagnostic it
//! has goes to **stderr**. Those three facts — one line, that shape, stdin
//! as the shutdown channel — are the whole interprocess protocol, and they
//! are what [`serve`] is arranged around: stdout is written once and then
//! left alone, and the shutdown wait is a channel fed by both a stdin reader
//! and a signal handler so that neither can be missed while the other is
//! being waited on.
//!
//! stdin is the shutdown channel rather than a signal because a parent that
//! dies takes its child's stdin with it. A gateway that outlived the process
//! that spawned it would keep a loopback port and a set of resolved
//! credentials alive with nobody to answer for them.
//!
//! # What this binary may decide, and what it may not
//!
//! It may choose the provider, the account and the entitlement a request is
//! served by. It may **not** change the model or the effort the caller asked
//! for unless the caller's own fallback policy permits it — that decision
//! lives in `routing::interactive` and this file adds no second path to it.
//! There is no project scope here, no session memory and no harness
//! identity: a standalone gateway serves HTTP clients and does not know what
//! any of them is.

use std::io::{Read, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use serde::Serialize;

use inference_gateway::config::{self, GatewayConfig};
use inference_gateway::entitlement::{EntitlementKind, EntitlementVendor};
use inference_gateway::gateway::subscription_broker::RunningSubscriptionBroker;
use inference_gateway::gateway::{self, BackendDemand, null_sink};
use inference_gateway::pool::{self, Pool};
use inference_gateway::provider::cache::{
    ModelCache, ModelCatalogue, ModelEntry, now_unix_seconds,
};
use inference_gateway::secret::file::FileSecretStore;
use inference_gateway::secret::native::{PreferNativeSecretStore, Presence, SourceKind};
use inference_gateway::secret::{SecretRef, SecretStore};
use inference_gateway::subscription::connect as flow;

/// The gateway as its own process: one wire format in, many providers out.
#[derive(Debug, Parser)]
#[command(name = "inference-gateway", version, about, long_about = None)]
struct Cli {
    /// The configuration file to read, instead of the platform location.
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,
    /// Private state root for subscription brokers and cached catalogues,
    /// instead of the platform location.
    #[arg(long, global = true, value_name = "PATH")]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Serve until stdin reaches EOF, printing one ready line first.
    Serve {
        // Clap renders this doc comment as `--help` text, so it says what a
        // user needs and not what a reader of the code does: `serve`'s own
        // doc comment carries the reasoning and the link.
        /// Where to listen. Only an ephemeral loopback port can be bound.
        #[arg(long, value_name = "ADDR", default_value = "127.0.0.1:0")]
        listen: String,
    },
    /// What each configured account is and what it can serve.
    Entitlements {
        /// Print the versioned JSON document instead of prose.
        #[arg(long)]
        json: bool,
        /// Read a model catalogue for every connected subscription account
        /// that has none cached.
        #[arg(long)]
        refresh: bool,
    },
    /// Subscription accounts.
    Subscriptions {
        #[command(subcommand)]
        command: SubscriptionsCommand,
    },
    /// Provider API keys this gateway stores and resolves.
    Credentials {
        #[command(subcommand)]
        command: CredentialsCommand,
    },
    // Same rule as `Serve::listen`: this line is `--help` text. What a
    // standalone gateway can and cannot answer here is on [`routing_cost`].
    /// What routing has consumed, in the same JSON Lines a host emits.
    RoutingCost {
        /// How far back to look, in hours.
        #[arg(long, value_name = "N", default_value_t = 24)]
        hours: u32,
        /// One JSON object per observation, one per line.
        #[arg(long)]
        json: bool,
        /// Start the window at this Unix second instead of `--hours` ago.
        #[arg(long, value_name = "UNIX", conflicts_with = "hours", requires = "json")]
        since: Option<i64>,
        /// Keep only this session's rows.
        #[arg(long, value_name = "ID", requires = "json")]
        session: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum SubscriptionsCommand {
    /// Connect one configured account with the provider's OAuth flow.
    Connect {
        #[arg(value_enum)]
        provider: SubscriptionProvider,
        /// The `[accounts.<name>]` table to connect.
        #[arg(long, value_name = "NAME")]
        entitlement: String,
        /// Emit each progress step as one JSON object per line.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum CredentialsCommand {
    /// Where each provider's credential comes from. Names only, never a value.
    List {
        /// Print the versioned JSON document instead of prose.
        #[arg(long)]
        json: bool,
    },
    /// Store a provider's API key, read from stdin, in this gateway's credential file.
    Set {
        /// The provider, as `credentials list` names it.
        provider: String,
        /// Which of the provider's variables to file it under; its first by default.
        #[arg(long, value_name = "VAR")]
        variable: Option<String>,
        /// Print one JSON object instead of prose.
        #[arg(long)]
        json: bool,
    },
    /// Remove a provider's API key from this gateway's credential file.
    Remove {
        /// The provider, as `credentials list` names it.
        provider: String,
        /// Which of the provider's variables to remove; its first by default.
        #[arg(long, value_name = "VAR")]
        variable: Option<String>,
        /// Print one JSON object instead of prose.
        #[arg(long)]
        json: bool,
    },
}

/// The vendor login flows this binary can drive — the three
/// `subscription::connect` records an OAuth client for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum SubscriptionProvider {
    Anthropic,
    Openai,
    Google,
}

impl SubscriptionProvider {
    /// The spelling `subscription::connect::client_for` keys on, and the one
    /// `entitlements --json` reports as `connect_with`. One table, so a row
    /// that command offers to connect is a row this one accepts.
    fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::Openai => "openai",
            Self::Google => "google",
        }
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("inference-gateway: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match &cli.command {
        Command::Serve { listen } => {
            let config = load_config(&cli)?;
            serve(listen, &config, &data_dir(&cli)?)
        }
        Command::Entitlements { json, refresh } => {
            let config = load_config(&cli)?;
            entitlements(&config, &data_dir(&cli)?, *json, *refresh)
        }
        Command::Subscriptions {
            command:
                SubscriptionsCommand::Connect {
                    provider,
                    entitlement,
                    json,
                },
        } => {
            let config = load_config(&cli)?;
            connect(&config, &data_dir(&cli)?, *provider, entitlement, *json)
        }
        Command::Credentials { command } => {
            let config = load_config(&cli)?;
            let data_dir = data_dir(&cli)?;
            match command {
                CredentialsCommand::List { json } => credentials_list(&config, &data_dir, *json),
                CredentialsCommand::Set {
                    provider,
                    variable,
                    json,
                } => credentials_set(&config, &data_dir, provider, variable.as_deref(), *json),
                CredentialsCommand::Remove {
                    provider,
                    variable,
                    json,
                } => credentials_remove(&config, &data_dir, provider, variable.as_deref(), *json),
            }
        }
        // Reads no configuration and needs no data directory: it answers
        // from a ledger, and this process has none.
        Command::RoutingCost {
            hours,
            json,
            since,
            session,
        } => routing_cost(*hours, *json, *since, session.as_deref()),
    }
}

/// The configuration, with the one line saying where it came from on stderr.
///
/// stderr and never stdout: `serve`'s stdout carries exactly one line and it
/// is the ready line, and a note printed before it would break the contract
/// for every caller that reads one line and stops.
fn load_config(cli: &Cli) -> Result<GatewayConfig> {
    let loaded = config::load(cli.config.as_deref())?;
    eprintln!("{}", loaded.note());
    Ok(loaded.config)
}

fn data_dir(cli: &Cli) -> Result<PathBuf> {
    match &cli.data_dir {
        Some(dir) => Ok(dir.clone()),
        None => config::default_data_dir()
            .context("could not determine a per-user application-data directory; pass --data-dir"),
    }
}

/// Why [`serve`] stopped. Both arrive on one channel so that neither can be
/// missed while the other is being waited on.
enum Stop {
    StdinEof,
    Signal,
}

/// The ready line's exact shape.
///
/// A struct rather than `serde_json::json!` because struct-field
/// serialization emits keys in declaration order regardless of feature
/// flags, and this line's shape is a contract another program parses.
#[derive(Serialize)]
struct Ready<'a> {
    listening: &'a str,
    token: &'a str,
}

/// Bind, announce, and serve until stdin reaches EOF or a signal arrives.
///
/// `--listen` may name only an **ephemeral loopback** address. The gateway
/// binds `127.0.0.1:0` and the operating system chooses the port; that is
/// what lets several instances coexist, and it is not a parameter the
/// library exposes. A fixed port is therefore refused by name rather than
/// silently ignored — a caller told "listening on 41219" when it asked for
/// 8080 would have been lied to about the one fact it needs.
fn serve(listen: &str, config: &GatewayConfig, data_dir: &Path) -> Result<()> {
    let address: SocketAddr = listen
        .parse()
        .with_context(|| format!("`--listen {listen}` is not a socket address"))?;
    if !address.ip().is_loopback() || address.port() != 0 {
        bail!(
            "`--listen {address}` cannot be honoured: this gateway binds a loopback port the \
             operating system chooses, which is what lets two instances coexist. Pass \
             `127.0.0.1:0`, or omit the flag"
        );
    }

    let providers = config::providers(config);
    let secrets = secret_store(data_dir);
    eprintln!("credentials resolve through {}", secrets.describe());
    // Everything a rebuild needs, owned, so the supplier a deferred start
    // keeps can run again on a later request — see `gateway::UpstreamSlot`.
    let build = {
        let accounts = config.accounts.clone();
        let providers = providers.clone();
        let data_dir = data_dir.to_path_buf();
        move || {
            pool::pool_from_catalogue(
                &accounts,
                &providers,
                &secrets,
                &|entitlement| config::broker_paths(&data_dir, entitlement),
                // No free-tier marking: a standalone gateway is told nothing
                // about who pays, and `Cost::Metered` is that answer's
                // fail-closed default.
                &|_| false,
            )
        }
    };
    let gateway = match build() {
        Ok(Pool { upstream, notes }) => {
            for note in notes {
                eprintln!("{note}");
            }
            gateway::start_if_required_with_degrade_sink(
                &[BackendDemand::LocalGateway],
                || Ok(upstream),
                None,
                None,
                // Nobody is listening, said out loud. A hosted gateway
                // installs an emitter here; this one has no host and drops
                // what it observes.
                Some(null_sink()),
                None,
            )?
            .context("a gateway was required and none was started")?
        }
        // Nothing to forward to yet. Listen anyway: the one flow that stores
        // a credential — the client's own login control — needs the client
        // running, and the client waits for this ready line. Every request
        // is answered `503` with the refusal until a rebuild succeeds, and a
        // credential stored meanwhile is picked up without a restart.
        Err(refusal) => {
            let refusal = refusal.to_string();
            eprintln!("serving nothing yet: {refusal}");
            gateway::start_awaiting_upstream(
                refusal,
                move || {
                    build()
                        .map(|Pool { upstream, notes }| {
                            for note in notes {
                                eprintln!("{note}");
                            }
                            upstream
                        })
                        .map_err(|refusal| refusal.to_string())
                },
                Some(null_sink()),
            )?
        }
    };
    if let Some(provider) = gateway.serving_provider() {
        eprintln!(
            "serving {provider} over {}",
            gateway.served_protocols().join(", ")
        );
    }

    let listening = gateway.base_url();
    let line = serde_json::to_string(&Ready {
        listening: &listening,
        token: gateway.token().expose(),
    })?;
    let mut stdout = std::io::stdout();
    writeln!(stdout, "{line}")?;
    stdout.flush()?;

    let reason = wait_for_shutdown();
    eprintln!(
        "stopping: {}",
        match reason {
            Stop::StdinEof => "stdin reached EOF",
            Stop::Signal => "a termination signal arrived",
        }
    );
    // Explicit, because this is the whole shutdown: it stops the accept
    // loop, joins its thread, releases the port, and drops every backend —
    // which kills and reaps any subscription sidecar and removes its
    // ephemeral serving directory.
    drop(gateway);
    Ok(())
}

/// Block until stdin reaches EOF or a termination signal arrives.
///
/// The stdin read runs on its own thread and reports through a channel, so
/// the signal handler — which runs on `ctrlc`'s thread, not in signal
/// context — can report through the same one. Waiting on stdin directly
/// would make a signal wake nothing, and waiting on a flag would make EOF
/// cost a poll.
///
/// Bytes that arrive on stdin are read and discarded rather than buffered:
/// the channel is EOF, not data, and a caller that piped something in must
/// not be able to grow this process's memory by doing so.
fn wait_for_shutdown() -> Stop {
    let (sender, receiver) = std::sync::mpsc::channel::<Stop>();
    let signal_sender = sender.clone();
    if let Err(error) = ctrlc::set_handler(move || {
        let _ = signal_sender.send(Stop::Signal);
    }) {
        eprintln!(
            "could not install a termination handler ({error}); closing stdin remains the way \
             to stop this gateway"
        );
    }
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let mut discard = [0_u8; 1024];
        loop {
            match stdin.read(&mut discard) {
                Ok(0) => break,
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        let _ = sender.send(Stop::StdinEof);
    });
    // A disconnected channel means both reporters are gone, which can only
    // happen once stdin's thread has ended: treat it as the EOF it is.
    receiver.recv().unwrap_or(Stop::StdinEof)
}

/// `entitlements [--json] [--refresh]` — what each configured account is.
///
/// The JSON document is the one a host produces, key for key: a `version`,
/// and an `accounts` array whose entries carry `account`, `provider`,
/// `models`, `scope`, `selectable`, `unavailable_reason`, `authenticated`
/// and `connect_with`. Built with `serde_json::json!` for the same reason
/// the host builds it that way — the key order is the macro's, so the two
/// documents are the same bytes for the same catalogue.
///
/// Two fields answer differently here than in a host, and both are
/// deliberate. **`selectable` is always true** and `unavailable_reason` is
/// always null: a host pins a session to one entitlement for the life of a
/// launch, and a standalone gateway has no session to pin. **`scope` is
/// `unknown` until something has read a catalogue** — this process caches
/// what `--refresh` reads and reports `account-declared` or
/// `provider-declared` from that cache, and an empty list with `unknown`
/// claims nothing rather than claiming an account serves no model.
fn entitlements(config: &GatewayConfig, data_dir: &Path, json: bool, refresh: bool) -> Result<()> {
    let cache = ModelCache::at(config::model_cache_dir(data_dir));
    if refresh {
        refresh_catalogues(config, data_dir, &cache);
    }

    let mut accounts = Vec::new();
    for (name, entry) in &config.accounts {
        let provider = match entry.subscription_broker() {
            Some(broker) => Some(
                entry
                    .vendor()
                    .map(EntitlementVendor::as_str)
                    .unwrap_or_else(|| broker.as_str())
                    .to_owned(),
            ),
            None => entry.provider().map(str::to_owned),
        };
        let (cached, scope) = if entry.subscription_broker().is_some() {
            (cache.load(name), "account-declared")
        } else if let Some(provider_name) = entry.provider() {
            (cache.load(provider_name), "provider-declared")
        } else {
            (None, "unknown")
        };
        let (mut models, scope) = match cached {
            Some(catalogue) => (
                catalogue
                    .models()
                    .iter()
                    .map(|model| model.id().to_owned())
                    .collect::<Vec<_>>(),
                scope,
            ),
            None => (Vec::new(), "unknown"),
        };
        models.sort();
        models.dedup();
        // Whether this account can be used at all, and if not, which flow
        // would fix it. A subscription with no credential is the row a
        // person most wants to act on.
        let connect_with = entry
            .subscription_broker()
            .and_then(|_| subscription_provider_for(entry.kind(), entry.vendor()))
            .map(|provider| provider.as_str().to_owned());
        let authenticated = connect_with
            .as_ref()
            .map(|_| credential_present(&config::broker_auth_dir(data_dir, name)).unwrap_or(false));
        accounts.push(serde_json::json!({
            "account": name,
            "provider": provider,
            "models": models,
            "scope": scope,
            "selectable": true,
            "unavailable_reason": Option::<String>::None,
            "authenticated": authenticated,
            "connect_with": connect_with,
        }));
    }

    let mut stdout = std::io::stdout();
    if json {
        let document = serde_json::to_string(&serde_json::json!({
            "version": 1,
            "accounts": accounts,
        }))?;
        writeln!(stdout, "{document}")?;
        return Ok(());
    }
    if accounts.is_empty() {
        writeln!(stdout, "no accounts are configured")?;
        return Ok(());
    }
    for account in &accounts {
        writeln!(
            stdout,
            "{}\t{}\t{} model(s), {}",
            account["account"].as_str().unwrap_or_default(),
            account["provider"].as_str().unwrap_or("(none)"),
            account["models"].as_array().map(Vec::len).unwrap_or(0),
            account["scope"].as_str().unwrap_or_default(),
        )?;
    }
    Ok(())
}

/// Read a model catalogue for every connected subscription account that has
/// none cached.
///
/// **Missing, not stale** — the same rule a host applies: `--refresh` fills
/// a gap, it does not re-fetch what is already known, so asking twice costs
/// one sidecar start rather than two. Sequentially rather than in parallel:
/// a standalone catalogue is a handful of accounts, and starting several
/// CLIProxyAPI processes at once to save a second is not a trade worth the
/// moving parts.
///
/// Every failure is a line on stderr and never an error: one account whose
/// broker will not start must not stop the other three being reported.
fn refresh_catalogues(config: &GatewayConfig, data_dir: &Path, cache: &ModelCache) {
    for (name, entry) in &config.accounts {
        if entry.subscription_broker().is_none() || cache.load(name).is_some() {
            continue;
        }
        let paths = config::broker_paths(data_dir, name);
        if !credential_present(&paths.auth_dir).unwrap_or(false) {
            eprintln!("account `{name}`: not connected, so it has no catalogue to read");
            continue;
        }
        let broker = match RunningSubscriptionBroker::start(&paths, name) {
            Ok(broker) => broker,
            Err(error) => {
                eprintln!("account `{name}`: its broker would not start: {error}");
                continue;
            }
        };
        let base_url = format!("{}/v1", broker.base_url());
        let endpoint = format!("{base_url}/models");
        let document = match broker.model_catalogue_document() {
            Ok(document) => document,
            Err(error) => {
                eprintln!("account `{name}`: its model catalogue did not answer: {error}");
                continue;
            }
        };
        let models: Vec<ModelEntry> = pool::parse_model_catalogue(&document)
            .into_iter()
            .map(ModelEntry::new)
            .collect();
        if models.is_empty() {
            eprintln!("account `{name}`: its model catalogue named no model");
            continue;
        }
        let catalogue = ModelCatalogue::new(name, base_url, endpoint, now_unix_seconds(), models);
        if let Err(error) = cache.store(&catalogue) {
            eprintln!("account `{name}`: its catalogue could not be cached: {error}");
        }
    }
}

/// `subscriptions connect <provider> --entitlement <name> [--json]`.
///
/// Every line this writes is safe to show and to forward: an authorization
/// URL, a countdown, a success or a failure. Nothing else crosses, which is
/// what lets another program render this without ever holding a credential —
/// `flow::Progress` has no variant carrying a token, a code or a verifier,
/// and that is the type's whole job.
fn connect(
    config: &GatewayConfig,
    data_dir: &Path,
    provider: SubscriptionProvider,
    entitlement: &str,
    json: bool,
) -> Result<()> {
    let Some(entry) = config.accounts.get(entitlement) else {
        bail!("no `[accounts.{entitlement}]` table is configured, so there is nothing to connect");
    };
    // An account that states what it is must be connected with the flow that
    // matches: writing an Anthropic credential into a ChatGPT account would
    // produce a login that succeeded and a route that never worked.
    if let Some(expected) = subscription_provider_for(entry.kind(), entry.vendor())
        && expected != provider
    {
        bail!(
            "account `{entitlement}` is connected with `{}`, not `{}`",
            expected.as_str(),
            provider.as_str()
        );
    }

    let mut out = std::io::stdout();
    let mut emit = |progress: &flow::Progress| {
        let line = if json {
            serde_json::to_string(progress).unwrap_or_else(|_| "{}".to_owned())
        } else {
            match progress {
                flow::Progress::Opened { authorize_url } => {
                    format!("open this to continue:\n{authorize_url}")
                }
                flow::Progress::Waiting { seconds_remaining } => {
                    format!("waiting for the browser ({seconds_remaining}s left)")
                }
                flow::Progress::Connected { account } => format!(
                    "connected{}",
                    account
                        .as_deref()
                        .map(|account| format!(" as {account}"))
                        .unwrap_or_default()
                ),
                flow::Progress::Failed { reason } => format!("failed: {reason}"),
            }
        };
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    };

    let Some(client) = flow::client_for(provider.as_str()) else {
        let reason = format!("no OAuth client is recorded for `{}`", provider.as_str());
        emit(&flow::Progress::Failed {
            reason: reason.clone(),
        });
        bail!(reason);
    };
    // A client id is public — it travels in every authorize URL — so this is
    // configuration rather than a secret. It is overridable because it is
    // the field a vendor rotates, and a constant would need a release to fix.
    let variable = format!(
        "GLASSHOUSE_OAUTH_CLIENT_ID_{}",
        provider.as_str().to_ascii_uppercase()
    );
    let client_id = std::env::var(&variable)
        .ok()
        .map(|id| id.trim().to_owned())
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| client.client_id.to_owned());

    let pkce = flow::Pkce::generate()?;
    let state = flow::random_state()?;
    emit(&flow::Progress::Opened {
        authorize_url: flow::authorize_url(client, &client_id, &pkce.challenge, &state),
    });

    let mut last_reported = u64::MAX;
    let callback = flow::await_callback(client, &state, flow::AUTHORIZE_TIMEOUT, |progress| {
        // One line a second at most: a countdown printed twice a second
        // would be the whole of what a reader saw.
        if let flow::Progress::Waiting { seconds_remaining } = &progress {
            if *seconds_remaining == last_reported {
                return;
            }
            last_reported = *seconds_remaining;
        }
        emit(&progress);
    });
    let callback = match callback {
        Ok(callback) => callback,
        Err(error) => {
            let reason = error.to_string();
            emit(&flow::Progress::Failed {
                reason: reason.clone(),
            });
            bail!(reason);
        }
    };

    let auth_dir = config::broker_auth_dir(data_dir, entitlement);
    match flow::exchange_and_store(client, &client_id, &pkce, &callback.code, &auth_dir) {
        Ok(account) => {
            emit(&flow::Progress::Connected { account });
            Ok(())
        }
        Err(error) => {
            let reason = error.to_string();
            emit(&flow::Progress::Failed {
                reason: reason.clone(),
            });
            bail!(reason)
        }
    }
}

/// `routing-cost --json --since <unix>` — and the standalone reading of it.
///
/// **A standalone gateway keeps no ledger, so there are no rows and this
/// prints none.** That is not a stub: a host's `--json` is JSON Lines with
/// no wrapper and no summary, and its own empty-window case returns the
/// empty string and exits `0`. An empty window and an absent ledger produce
/// the same well-formed output, so a caller parsing line by line needs no
/// special case for either.
///
/// It is not reachable another way, either. What this process measures lives
/// in `gateway::usage`, which is private to the library and, more to the
/// point, lives in the memory of a **running `serve`** — a separate process
/// from this invocation, with no channel between them. Reaching it would
/// mean the gateway had grown a store or an IPC surface, and the first is
/// what the extraction removed.
fn routing_cost(hours: u32, json: bool, since: Option<i64>, session: Option<&str>) -> Result<()> {
    let window = match since {
        Some(since) => format!("since the Unix second {since}"),
        None => format!("over the last {hours} hour(s)"),
    };
    let filter = match session {
        Some(session) => format!(" for the session `{session}`"),
        None => String::new(),
    };
    eprintln!(
        "routing-cost: this gateway keeps no routing ledger, so it has no rows {window}{filter}. \
         A host that installs an observation sink records them on its own side of the process \
         boundary"
    );
    if !json {
        writeln!(
            std::io::stdout(),
            "no routing observations: this gateway keeps no ledger"
        )?;
    }
    Ok(())
}

/// The vendor login flow an account's `kind`/`vendor` selects, or `None` for
/// an account no subscription broker can connect.
///
/// One table, read by `entitlements --json`'s `connect_with` and by
/// `connect`'s own validation, so a row the first offers to connect is a row
/// the second accepts.
fn subscription_provider_for(
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

/// Whether `dir` holds at least one regular file, which is the whole of what
/// "connected" can honestly mean.
///
/// Presence, never validity: an expired OAuth token is still a file on disk,
/// so this proves only that a login once happened. Reading the token to say
/// more would put account material back inside this process, which is the
/// one thing the broker design exists to prevent.
///
/// Refuses a symlink or a non-directory rather than following it: the auth
/// directory is private state, and reporting through a symlink would be
/// reporting about a location the user did not choose.
/// The store every command resolves through: the gateway's own credential
/// file first, then the native store, then the process environment.
fn secret_store(data_dir: &Path) -> PreferNativeSecretStore {
    PreferNativeSecretStore::detect_with_file(config::credentials_path(data_dir))
}

/// One row per (provider, variable): where the credential comes from, never
/// what it is. `native_store` is the diagnostic for the one state a user
/// cannot otherwise see — an item that exists and is refused to this build.
fn credentials_list(config: &GatewayConfig, data_dir: &Path, json: bool) -> Result<()> {
    let store = secret_store(data_dir);
    let mut rows = Vec::new();
    for provider in config::providers(config) {
        for var in &provider.credential_env {
            let reference = SecretRef::Environment { var: var.clone() };
            let source = store.source_kind(&reference);
            let native_store = match store.native() {
                Ok(native) => match native.presence(&reference) {
                    Presence::Present => "present",
                    Presence::Absent => "absent",
                    Presence::Refused => "refused",
                },
                Err(_) => "unavailable",
            };
            rows.push((provider.name.clone(), var.clone(), source, native_store));
        }
    }
    let mut stdout = std::io::stdout();
    if json {
        let providers: Vec<serde_json::Value> = rows
            .iter()
            .map(|(provider, variable, source, native_store)| {
                serde_json::json!({
                    "provider": provider,
                    "variable": variable,
                    "source": source.map(SourceKind::as_str),
                    "native_store": native_store,
                })
            })
            .collect();
        writeln!(
            stdout,
            "{}",
            serde_json::json!({ "version": 1, "providers": providers })
        )?;
        return Ok(());
    }
    if rows.is_empty() {
        writeln!(
            stdout,
            "no configured provider declares a credential variable"
        )?;
        return Ok(());
    }
    for (provider, variable, source, native_store) in rows {
        let state = match source {
            Some(kind) => format!("stored in {}", kind.describe()),
            None if native_store == "refused" => {
                "a native-store item exists that this build may not read; store it again".to_owned()
            }
            None => "not set".to_owned(),
        };
        writeln!(stdout, "{provider}\t{variable}\t{state}")?;
    }
    Ok(())
}

/// The variable a provider's key is filed under: the one `--variable`
/// names, or the first the provider declares. A provider that declares
/// none takes no key, and one that is not configured or built in is named
/// as such rather than silently created.
fn credential_variable(
    config: &GatewayConfig,
    provider: &str,
    variable: Option<&str>,
) -> Result<String> {
    let providers = config::providers(config);
    let Some(entry) = providers.iter().find(|entry| entry.name == provider) else {
        bail!(
            "`{provider}` is neither a configured provider nor a built-in template; \
             `credentials list` names them"
        );
    };
    if entry.credential_env.is_empty() {
        bail!("`{provider}` declares no credential variable, so it takes no API key");
    }
    match variable {
        None => Ok(entry.credential_env[0].clone()),
        Some(var) if entry.credential_env.iter().any(|declared| declared == var) => {
            Ok(var.to_owned())
        }
        Some(var) => bail!(
            "`{provider}` reads {}, not `{var}`",
            entry.credential_env.join(" or ")
        ),
    }
}

/// Store the key on stdin under the provider's variable. **Stdin and never
/// an argument**: an argument is in every process listing and every shell
/// history. The value is held for this call and printed by nothing.
fn credentials_set(
    config: &GatewayConfig,
    data_dir: &Path,
    provider: &str,
    variable: Option<&str>,
    json: bool,
) -> Result<()> {
    let variable = credential_variable(config, provider, variable)?;
    let mut key = String::new();
    std::io::stdin()
        .read_to_string(&mut key)
        .context("reading the key from stdin")?;
    let key = key.trim_end_matches(['\r', '\n']);
    if key.is_empty() {
        bail!(
            "no key arrived on stdin; pipe it in: `printf %s \"$KEY\" | inference-gateway \
             credentials set {provider}`"
        );
    }
    if key.chars().any(char::is_control) {
        bail!("the key contains a line break or a control character; a key is one line");
    }
    let store = FileSecretStore::at(config::credentials_path(data_dir));
    store.store(&variable, key)?;
    let mut stdout = std::io::stdout();
    if json {
        writeln!(
            stdout,
            "{}",
            serde_json::json!({
                "provider": provider,
                "variable": variable,
                "stored_in": store.path().display().to_string(),
            })
        )?;
    } else {
        writeln!(
            stdout,
            "stored {variable} for {provider} in {}",
            store.path().display()
        )?;
    }
    Ok(())
}

fn credentials_remove(
    config: &GatewayConfig,
    data_dir: &Path,
    provider: &str,
    variable: Option<&str>,
    json: bool,
) -> Result<()> {
    let variable = credential_variable(config, provider, variable)?;
    let store = FileSecretStore::at(config::credentials_path(data_dir));
    let removed = store.remove(&variable)?;
    let mut stdout = std::io::stdout();
    if json {
        writeln!(
            stdout,
            "{}",
            serde_json::json!({ "provider": provider, "variable": variable, "removed": removed })
        )?;
    } else if removed {
        writeln!(stdout, "removed {variable} for {provider}")?;
    } else {
        writeln!(stdout, "nothing was stored for {provider} under {variable}")?;
    }
    Ok(())
}

fn credential_present(dir: &Path) -> Result<bool> {
    let metadata = match std::fs::symlink_metadata(dir) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).with_context(|| format!("could not inspect {dir:?}")),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("subscription auth location {dir:?} is not a private directory");
    }
    for entry in std::fs::read_dir(dir).with_context(|| format!("could not inspect {dir:?}"))? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ready line's shape, which is the whole interprocess contract:
    /// two keys, in this order, and the address is a loopback URL.
    #[test]
    fn the_ready_line_carries_exactly_the_two_keys_a_caller_reads() {
        let line = serde_json::to_string(&Ready {
            listening: "http://127.0.0.1:41219",
            token: "deadbeef",
        })
        .expect("a two-field struct serializes");
        assert_eq!(
            line,
            r#"{"listening":"http://127.0.0.1:41219","token":"deadbeef"}"#
        );
    }

    /// `--listen` is refused rather than ignored when it names something the
    /// library cannot bind, and the refusal names what was asked for.
    #[test]
    fn a_fixed_listen_port_is_refused_by_name() {
        let error = serve(
            "127.0.0.1:8080",
            &GatewayConfig::default(),
            Path::new("/nonexistent"),
        )
        .expect_err("a fixed port cannot be honoured");
        let rendered = error.to_string();
        assert!(rendered.contains("127.0.0.1:8080"), "{rendered}");
        assert!(rendered.contains("127.0.0.1:0"), "{rendered}");
    }

    /// A `kind`/`vendor` pair maps to one flow, and an account that states
    /// neither maps to none — so `connect_with` is never guessed.
    #[test]
    fn a_login_flow_is_selected_only_by_what_an_account_states() {
        assert_eq!(
            subscription_provider_for(Some(EntitlementKind::Claude), None),
            Some(SubscriptionProvider::Anthropic)
        );
        assert_eq!(
            subscription_provider_for(None, Some(EntitlementVendor::OpenAi)),
            Some(SubscriptionProvider::Openai)
        );
        assert_eq!(subscription_provider_for(None, None), None);
        assert_eq!(
            subscription_provider_for(Some(EntitlementKind::ApiKey), None),
            None
        );
    }

    /// This binary's own files are covered by the rule `lib.rs`'s header
    /// states — **nothing in this crate may name Glasshouse**.
    ///
    /// `gateway::tests::the_gateway_names_no_glasshouse_path` lists these
    /// three files too and pins its own length; this is the same two needles
    /// checked from the binary's own target, so the rule holds even for a
    /// `cargo test --bin` run that never builds the library's tests.
    #[test]
    fn the_binarys_own_files_name_no_glasshouse_path() {
        const FORBIDDEN: [&str; 2] = ["glasshouse::", "rusqlite"];
        for (name, source) in [
            ("main.rs", include_str!("main.rs")),
            ("pool.rs", include_str!("pool.rs")),
            ("config.rs", include_str!("config.rs")),
        ] {
            let production = source
                .split_once("#[cfg(test)]")
                .map_or(source, |(before, _)| before);
            for forbidden in FORBIDDEN {
                assert!(
                    !production.contains(forbidden),
                    "{name} names `{forbidden}` in production code"
                );
            }
        }
    }
}
