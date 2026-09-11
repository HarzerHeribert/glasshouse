// Splitting a pre-2026-09-11 Glasshouse configuration into the two files it
// is now.
//
// User ruling of that date: an account's plan, billing vendor, credential
// reference, subscription broker and provider are the **gateway's**, and
// `[entitlements.<name>]` in Glasshouse's own `config.toml` states only
// policy about an account the gateway already has.
//
// Dozens of integration fixtures here write one configuration string and
// spawn the binary against it. Rather than hand-split every one of them —
// and get a different answer in each — this does it once: the five account
// keys move into `[accounts.<name>]`, everything else stays, and an
// entitlement table left holding nothing is dropped.
//
// `include!`d rather than `mod`-declared: each integration test binary is
// its own crate and `tests/fixtures/` is not a target, so this is the one
// way a helper can be shared without turning it into product code.
//
// Deliberately line-based and deliberately narrow. It reads the fixture
// strings this repository writes — a `[table]` header per line, one
// `key = value` per line — and nothing cleverer. A fixture it cannot split
// comes out unchanged, which shows up as the loud refusal the loader gives
// rather than as a quiet wrong answer.

/// The five keys that move.
const ACCOUNT_KEYS: [&str; 5] = [
    "kind",
    "vendor",
    "credential",
    "subscription_broker",
    "provider",
];

/// `(config.toml, gateway.toml)` — `text` with the account keys removed, and
/// the `[accounts.<name>]` tables they became.
///
/// The gateway half is empty when `text` states no account key, so a caller
/// can write it unconditionally.
#[allow(dead_code)]
pub fn split_gateway_state(text: &str) -> (String, String) {
    let mut config = String::with_capacity(text.len());
    // Name to the account lines taken from its entitlement table, in the
    // order they were written.
    let mut accounts: Vec<(String, Vec<String>)> = Vec::new();
    // The `[entitlements.<name>]` header we are inside, and the lines kept
    // under it so far — buffered, because a header whose every key moved is
    // a header that goes with them.
    let mut pending: Option<(String, String, Vec<String>)> = None;

    let flush = |pending: &mut Option<(String, String, Vec<String>)>, config: &mut String| {
        if let Some((_, header, kept)) = pending.take()
            && kept.iter().any(|line| !line.trim().is_empty())
        {
            config.push_str(&header);
            for line in kept {
                config.push_str(&line);
            }
        }
    };

    for raw in text.split_inclusive('\n') {
        let trimmed = raw.trim();
        if trimmed.starts_with('[') {
            flush(&mut pending, &mut config);
            if let Some(name) = entitlement_table_name(trimmed) {
                pending = Some((name.to_owned(), raw.to_owned(), Vec::new()));
                continue;
            }
            config.push_str(raw);
            continue;
        }
        match &mut pending {
            Some((name, _, kept)) => match account_key(trimmed) {
                Some(_) => {
                    let entry = match accounts.iter_mut().find(|(n, _)| n == name) {
                        Some(entry) => entry,
                        None => {
                            accounts.push((name.clone(), Vec::new()));
                            accounts.last_mut().expect("just pushed")
                        }
                    };
                    entry.1.push(raw.to_owned());
                }
                None => kept.push(raw.to_owned()),
            },
            None => config.push_str(raw),
        }
    }
    flush(&mut pending, &mut config);

    let mut gateway = String::new();
    for (name, lines) in accounts {
        gateway.push_str(&format!("[accounts.{name}]\n"));
        for line in lines {
            gateway.push_str(&line);
        }
        gateway.push('\n');
    }
    (config, gateway)
}

/// `[entitlements.<name>]` — and never `[entitlements.<name>.something]`,
/// whose keys are a sub-table's and stay where they are.
fn entitlement_table_name(header: &str) -> Option<&str> {
    let inner = header.strip_prefix('[')?.strip_suffix(']')?;
    let name = inner.strip_prefix("entitlements.")?;
    if name.contains('.') || name.is_empty() {
        return None;
    }
    Some(name)
}

/// The account key a `key = value` line sets, if it sets one.
fn account_key(line: &str) -> Option<&'static str> {
    let key = line.split('=').next()?.trim();
    ACCOUNT_KEYS.into_iter().find(|candidate| *candidate == key)
}
