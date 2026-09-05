//! `commands::context_firewall` -- moved verbatim from `main.rs` (Phase 59 decomposition).

use glasshouse::Runtime;
use glasshouse::config::{self, EffectiveConfig, ProjectConfig, UserConfig};

/// Handle `context-firewall hook` — the production caller map lines
/// 1980-1990 need. Reads one `PostToolUse` event on stdin, runs it through
/// [`glasshouse::firewall::process`], records telemetry, and writes the
/// hook response on stdout.
///
/// Fails open at every internal step: a stdin document this build cannot
/// parse, a raw-store write that fails, or a ledger that cannot be opened
/// all end in the same no-op response a passthrough result gets, never a
/// nonzero exit — `docs/product/evidence/phase-57.md`'s "fail open, never
/// empty" applies to the hook process itself, not only to the reduction.
// One argument over the threshold, and it is `--session`. Grouping the
// hook's flags into a struct to get under it would put a type between the
// CLI and this function whose only job is to be counted, which is the shape
// `install_context_firewall_hook` below already declined for the same reason.
#[allow(clippy::too_many_arguments)]
pub(crate) fn context_firewall_hook(
    runtime: &Runtime,
    passthrough_tokens: u64,
    min_semantic_tokens: u64,
    task: &str,
    tools: &[String],
    emit_updated_output: bool,
    mode: glasshouse::config::firewall::FirewallMode,
    session: Option<&str>,
) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use std::io::Read;

    let mut input = Vec::new();
    std::io::stdin()
        .read_to_end(&mut input)
        .context("could not read the PostToolUse event from stdin")?;

    let event = match glasshouse::firewall::adapter::parse_event(&input) {
        Ok(event) => event,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "context firewall: could not parse the PostToolUse event; answering with a \
                 no-op response"
            );
            return print_context_firewall_response(None);
        }
    };

    // Map line 1139's producer, and it runs **before** the reduction on
    // purpose: everything below can fail open, and this must not be able to
    // change what any of it decides. It reads the event and writes elsewhere;
    // nothing it does is visible to `process`, and `record_file_touches`
    // returns nothing for the caller to branch on.
    record_file_touches(runtime, session, &event);

    let normalized = glasshouse::firewall::adapter::normalize(&event);
    let config = glasshouse::firewall::FirewallConfig::new(passthrough_tokens, tools.to_vec());
    let store = glasshouse::firewall::RawStore::open(runtime.state_dir().join("context-firewall"));
    let now_unix = glasshouse::provider::cache::now_unix_seconds();

    // Phase 57B, map lines 1997-2003: resolved once, from configuration and
    // disposable routing, and handed to `process` as a trait object — the
    // core itself never touches `DisposableRouting`, a `JobKind`, or a
    // provider (see `firewall::mod`'s own header). A configuration this
    // build cannot read degrades to "no reducer" — the same fail-open
    // posture every other step of this hook already has.
    let user = UserConfig::load(runtime.paths()).ok();
    let project = config::load_project_config(runtime.project())
        .ok()
        .flatten();
    let aggressive_drops_uncertain = user.as_ref().is_some_and(|user| {
        EffectiveConfig::new(user, project.as_ref())
            .context_firewall_aggressive_drops_uncertain()
            .value
    });
    let active_reducer = match &user {
        Some(user) => disposable_reducer(runtime, user, project.as_ref(), &event.session_id),
        None => None,
    };
    let tool_query = glasshouse::firewall::adapter::tool_query(&event.tool_input);
    let file_paths = glasshouse::firewall::adapter::tool_input_paths(&event.tool_input);
    let semantic = glasshouse::firewall::SemanticContext {
        mode,
        reducer: active_reducer.as_deref(),
        task,
        tool_query: tool_query.as_deref(),
        file_paths: &file_paths,
        min_semantic_tokens,
        aggressive_drops_uncertain,
    };

    let outcome = glasshouse::firewall::process(
        &store,
        &config,
        &event.session_id,
        &event.tool_use_id,
        now_unix,
        &event.tool_name,
        normalized,
        &semantic,
    );

    record_context_firewall_telemetry(runtime, &outcome, now_unix);

    // Map line 1991's mode decision, enforced here rather than trusted to
    // whatever registered the command line: `shadow` never emits
    // `updatedToolOutput`, whatever `--emit-updated-output` says, because
    // shadow's whole point is a session that sees only originals while the
    // pipeline still runs in full for storage, telemetry and provenance.
    let effective_emit =
        emit_updated_output && mode != glasshouse::config::firewall::FirewallMode::Shadow;
    let updated_output = match &outcome {
        glasshouse::firewall::Outcome::Reduced { forwarded_text, .. } if effective_emit => {
            Some(forwarded_text.as_str())
        }
        _ => None,
    };
    print_context_firewall_response(updated_output)
}

/// Map line 1139's producer: one `file_touched` lifecycle event per distinct
/// path a **writing** tool named, for the Glasshouse session this hook was
/// registered for.
///
/// # Why the hook's response can never depend on this
///
/// It returns `()`. There is no error for the caller to see, no value for it
/// to branch on, and every failure below ends in a `tracing::warn!` and a
/// `return`. That is not caution about a rare case — the whole tool call is
/// downstream of this function, and a bookkeeping write that could fail a
/// user's `Edit` would be a far worse defect than never learning which file
/// it touched. `the_hook_response_is_identical_whether_or_not_recording_works`
/// is the proof rather than this paragraph.
///
/// # The four gates a path passes, in order
///
/// 1. **A session**, or nothing is recorded. See
///    `cli::ContextFirewallCommand::Hook`'s `--session` for why absent is a
///    supported state and why the payload's own `session_id` is not a
///    substitute.
/// 2. **A writing tool** — `firewall::eligibility::is_writing_tool`, which is
///    the block list read the other way round. `Read`, `Grep` and `Glob`
///    carry paths and are deliberately not recorded: *touched* means the
///    session changed the file.
/// 3. **Under the project root.** An absolute path inside the root is made
///    relative to it; a path outside it is **dropped and never stored**, which
///    is the isolation invariant rather than a tidiness rule — a memory must
///    not be able to name a file in another project, or in the user's home.
/// 4. **Normalisable**, through the one function `memory_files.path` already
///    goes through, so the two producers spell a path identically or the
///    association never matches.
///
/// Distinct paths only: `MultiEdit` names the same file once per edit, and
/// sixty rows saying one file was edited is sixty times the storage for the
/// same fact.
pub(crate) fn record_file_touches(
    runtime: &Runtime,
    session: Option<&str>,
    event: &glasshouse::firewall::adapter::PostToolUseEvent,
) {
    use glasshouse::events::{EventBus, EventLog, LifecycleEvent};
    use glasshouse::session::SessionId;

    let Some(session) = session else {
        tracing::debug!(
            "context firewall: no --session on this hook's command line, so which files \
             the session edits is not recorded; relaunch the session to register a hook \
             that carries one"
        );
        return;
    };
    if !glasshouse::firewall::eligibility::is_writing_tool(&event.tool_name) {
        return;
    }

    let root = runtime.project().root();
    let mut paths: Vec<String> = Vec::new();
    for raw in glasshouse::firewall::adapter::tool_input_paths(&event.tool_input) {
        let Some(path) = project_relative_path(root, &raw) else {
            continue;
        };
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    if paths.is_empty() {
        return;
    }

    let log = match EventLog::open(runtime) {
        Ok(log) => log,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "context firewall: the project event log is unavailable; which files this \
                 tool call edited is not recorded"
            );
            return;
        }
    };
    let session = SessionId::new(session);
    // A bus with no history, purely to mint a `RecordedEvent` with the
    // timestamp its own clock takes — the same shape `LifecycleRecorder`
    // uses, and the only constructor there is. Nothing subscribes to it: this
    // process is a hook, it lives for one tool call, and the durable log is
    // the only consumer that matters.
    let bus = EventBus::with_history(0);
    for path in paths {
        let recorded = bus.publish(&session, LifecycleEvent::FileTouched { path });
        // Per path rather than per batch: one unwritable row must not take
        // the others with it, and there is no transaction here to be
        // half-applied — the log is append-only by trigger.
        //
        // `None` for the observation: this is not a translated harness
        // report. Claude Code said a tool ran; Glasshouse decided that means
        // a file changed, and attributing that reading to the harness would
        // be a claim it never made.
        if let Err(err) = log.append(&recorded, None) {
            tracing::warn!(
                error = %err,
                "context firewall: could not record an edited file"
            );
        }
    }
}

/// `raw` as a path under `root`, in `memory_files.path`'s spelling, or
/// `None`.
///
/// Claude Code hands the hook an **absolute** path, and on Windows it hands
/// one with `\` separators. So: fold the separators first — before any
/// prefix test, because `C:\proj\src\a.rs` does not start with
/// `C:/proj/src` until it has been folded — then strip the root, then put
/// what is left through
/// [`glasshouse::memory::store::normalize_observed_path`], which is the
/// function the other writer of this column uses and the only definition of
/// the spelling.
///
/// Both sides are reduced to one spelling before any prefix test, and the
/// separator fold is only half of that: on Windows the root is
/// `fs::canonicalize`'s output and therefore **verbatim** (`\\?\C:\proj`),
/// while a tool input or a shell argument is not, so the two would fail to
/// match for the same reason `\` and `/` did. See
/// [`folded_ordinary_spelling`].
///
/// `None` for a path outside the root, and that is the isolation invariant:
/// nothing outside the project is stored, not even to be filtered out later.
/// A relative path is accepted as already being relative to the root, which
/// is what a relative path in a tool input means.
///
/// `pub(crate)` for `commands::sessions::claimed_path`, which needs the same
/// answer for the same reason: `file_claims.path` and `memory_files.path`
/// hold the same spelling, and a second implementation of "inside this
/// project, spelled this way" is how the two would come to disagree.
pub(crate) fn project_relative_path(root: &std::path::Path, raw: &str) -> Option<String> {
    let folded = folded_ordinary_spelling(raw);
    let root_folded = folded_ordinary_spelling(&root.display().to_string());
    let root_folded = root_folded.trim_end_matches('/');

    let relative = if let Some(rest) = folded.strip_prefix(root_folded) {
        // The prefix must end at a separator, or `/proj-other/a.rs` would
        // pass as a file inside `/proj`.
        match rest.strip_prefix('/') {
            Some(rest) => rest,
            // The path *is* the root: a directory, not a file anything
            // edited.
            None if rest.is_empty() => return None,
            None => return None,
        }
    } else if folded.starts_with('/') || folded.chars().nth(1) == Some(':') {
        // Absolute, and not under this project. Dropped here rather than
        // normalised, because `normalize_observed_path` would refuse it too
        // and this says why in one place.
        return None;
    } else {
        folded.as_str()
    };

    glasshouse::memory::normalize_observed_path(relative)
}

/// `path` with `\` folded to `/` and any Windows verbatim prefix reduced to
/// the ordinary spelling of the same file.
///
/// `\\?\C:\proj` and `C:\proj` name one directory, and `\\?\UNC\srv\share` and
/// `\\srv\share` name one share; only the reduced spelling of each can be
/// compared with the other. Both spellings genuinely arrive at
/// [`project_relative_path`] — the project root is `fs::canonicalize`'s
/// output, which on Windows is always verbatim, while a hook's tool input or
/// a shell argument almost never is, and a caller that has canonicalized for
/// itself hands over the first form on both sides.
fn folded_ordinary_spelling(path: &str) -> String {
    let folded = path.replace('\\', "/");
    reduced_verbatim_prefix(&folded).unwrap_or(folded)
}

/// `folded` without its Windows verbatim prefix, or `None` when it carries
/// none — where "carries one" means `//?/` followed by something only
/// Windows produces.
///
/// That condition is the isolation half of the reduction, not a tidiness
/// check. `//?/` is an unusual but perfectly legal absolute path on Unix, so
/// an unconditional strip would reduce `//?/proj/a.rs` to the *relative*
/// `proj/a.rs`, which [`project_relative_path`] accepts as being inside
/// `/proj`. Requiring a drive letter or the `UNC/` marker is what keeps the
/// Windows repair from widening containment on every other platform.
fn reduced_verbatim_prefix(folded: &str) -> Option<String> {
    let rest = folded.strip_prefix("//?/")?;
    if is_drive_rooted(rest) {
        return Some(rest.to_owned());
    }
    // `\\?\UNC\srv\share` is the verbatim way of writing `\\srv\share`: the
    // marker stands in for the second leading separator.
    let marker = rest.get(..4)?;
    marker
        .eq_ignore_ascii_case("unc/")
        .then(|| format!("//{}", &rest[4..]))
}

/// `rest` begins with a drive letter and a colon — the same two-character
/// test [`glasshouse::memory::normalize_observed_path`] uses to recognise a
/// Windows-absolute path, applied here to what follows a verbatim prefix.
fn is_drive_rooted(rest: &str) -> bool {
    let mut chars = rest.chars();
    matches!(
        (chars.next(), chars.next()),
        (Some(drive), Some(':')) if drive.is_ascii_alphabetic()
    )
}

/// Write the `PostToolUse` hook response JSON to stdout — the protocol
/// channel here exactly as it is for `glasshouse mcp serve`.
fn print_context_firewall_response(updated_output: Option<&str>) -> anyhow::Result<()> {
    let response = glasshouse::firewall::adapter::hook_response(updated_output);
    println!("{}", serde_json::to_string(&response)?);
    Ok(())
}

/// Map line 1987: one telemetry row per reduced result and one per bypass —
/// never for a passthrough result, which line 1981 already defines as
/// carrying nothing beyond the harness's own original output.
fn record_context_firewall_telemetry(
    runtime: &Runtime,
    outcome: &glasshouse::firewall::Outcome,
    now_unix: i64,
) {
    use glasshouse::routing::evidence::{
        CONTEXT_FIREWALL_BYPASS_PURPOSE, CONTEXT_FIREWALL_REDUCTION_PURPOSE, EvidenceLedger,
        NewObservation,
    };

    let (purpose, route) = match outcome {
        glasshouse::firewall::Outcome::Passthrough { .. } => return,
        glasshouse::firewall::Outcome::Reduced { .. } => (CONTEXT_FIREWALL_REDUCTION_PURPOSE, None),
        glasshouse::firewall::Outcome::Bypass { reason, .. } => {
            (CONTEXT_FIREWALL_BYPASS_PURPOSE, Some(reason.as_str()))
        }
    };

    let ledger = match EvidenceLedger::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "routing evidence ledger unavailable; a context-firewall event is not recorded"
            );
            return;
        }
    };
    // `provider`/`model` have no real backend here — this is not a model
    // call — so a fixed, self-describing placeholder stands in, exactly as
    // `CORRELATION_PURPOSE`'s rows use the displaced route's identity for a
    // row that is "about" something rather than an exchange. No reader
    // filters on this pair, so it cannot be mistaken for real spend; the
    // `purpose` column is what keeps it out of every such reader, per its
    // own doc comment.
    let observation = NewObservation::new("glasshouse", "context-firewall")
        .with_harness(Some(
            glasshouse::integrations::IntegrationId::ClaudeCode.slug(),
        ))
        .with_purpose(Some(purpose))
        .with_route(route)
        .with_quota_context(Some(outcome.tool_name().to_owned()))
        .with_timing(Some(now_unix), Some(now_unix));
    if let Err(err) = ledger.record(observation, now_unix) {
        tracing::warn!(error = %err, "could not record a context-firewall event");
    }

    // Map line 1987's second half (the 1987 ruling in
    // `docs/product/evidence/phase-57.md`): a reducer call is a REAL model
    // call, so its own row carries the real provider/model identity and the
    // provider-reported token counts in the ledger's token columns —
    // distinct from the bookkeeping row above, which is not a model call
    // and therefore never carries tokens. Recorded whenever a call actually
    // completed with a parseable reply, applied or not (map line 1987: the
    // cost was real either way).
    if let glasshouse::firewall::Outcome::Reduced {
        semantic: Some(semantic),
        ..
    } = outcome
        && let Some(call) = &semantic.call
    {
        let call_observation = NewObservation::new(call.provider.clone(), call.model.clone())
            .with_harness(Some(
                glasshouse::integrations::IntegrationId::ClaudeCode.slug(),
            ))
            .with_purpose(Some(CONTEXT_FIREWALL_REDUCTION_PURPOSE))
            .with_route(call.route.clone())
            .with_quota_context(Some(outcome.tool_name().to_owned()))
            .with_timing(Some(now_unix), Some(now_unix))
            .with_tokens(
                call.input_tokens,
                call.output_tokens,
                call.cached_input_tokens,
            );
        if let Err(err) = ledger.record(call_observation, now_unix) {
            tracing::warn!(error = %err, "could not record a context-firewall reducer call");
        }
    }
}

/// Map line 2004's four granularities. `Whole` is the pre-existing
/// behaviour every earlier package relied on; the other three are this
/// package's own, and are reached only through this same subcommand — no
/// invented side channel.
pub(crate) enum ExpansionRequest {
    Whole,
    Candidate(usize),
    File(String),
    Range((usize, usize)),
}

/// What `context_firewall_show` decided, once the reference itself
/// resolved (or did not).
pub(crate) enum ExpansionOutcome {
    Content(String),
    /// The reference itself does not name a stored entry — the pre-existing
    /// refusal `Show`'s bare form already had.
    NotFound,
    /// The reference resolved, but the requested slice of it does not exist
    /// (an out-of-range candidate id, a file the result never names, or a
    /// reversed/out-of-bounds range). Kept distinct from `NotFound`: the
    /// expansion-request telemetry below already counted this reference as
    /// found, because it was — the refusal is about the *granularity*, not
    /// the reference.
    Refused(String),
}

/// `context-firewall show <id>`: expand a previously stored raw tool
/// result at the granularity `request` names, and record the map line 1988
/// expansion-request telemetry either way — a miss is still a request, and
/// still part of the recall signal. `request = Whole` reproduces map line
/// 1985's exact byte-identical round-trip; the other three variants are map
/// line 2004's.
pub(crate) fn context_firewall_show(
    runtime: &Runtime,
    id: &str,
    request: ExpansionRequest,
) -> anyhow::Result<ExpansionOutcome> {
    use anyhow::Context as _;

    let store = glasshouse::firewall::RawStore::open(runtime.state_dir().join("context-firewall"));
    let entry = store
        .read(id)
        .with_context(|| format!("could not read the context-firewall raw store for `{id}`"))?;
    record_context_firewall_expansion(runtime, entry.as_ref().map(|entry| entry.tool.as_str()));
    let Some(entry) = entry else {
        return Ok(ExpansionOutcome::NotFound);
    };

    Ok(match request {
        ExpansionRequest::Whole => ExpansionOutcome::Content(entry.content),
        ExpansionRequest::Candidate(candidate_id) => {
            // Recomputed rather than stored: `reduce` is a pure function of
            // `entry.content`, which is the exact original this entry has
            // held since it was written — the same id therefore always
            // names the same candidate, with nothing new to persist.
            let reduction = glasshouse::firewall::reduce::reduce(&entry.content);
            match reduction
                .candidates
                .iter()
                .find(|candidate| candidate.id == candidate_id)
            {
                Some(candidate) => ExpansionOutcome::Content(candidate.text.clone()),
                None => ExpansionOutcome::Refused(format!(
                    "`{id}` has no candidate `{candidate_id}` (0..{})",
                    reduction.candidates.len()
                )),
            }
        }
        ExpansionRequest::File(file) => {
            let matches: Vec<&str> = entry
                .content
                .lines()
                .filter(|line| line_names_file(line, &file))
                .collect();
            if matches.is_empty() {
                ExpansionOutcome::Refused(format!("`{id}` names no file `{file}`"))
            } else {
                ExpansionOutcome::Content(format!("{}\n", matches.join("\n")))
            }
        }
        ExpansionRequest::Range((start, end)) => {
            if start == 0 {
                ExpansionOutcome::Refused(
                    "line ranges are 1-indexed; `0` is not a line".to_string(),
                )
            } else if start > end {
                ExpansionOutcome::Refused(format!(
                    "range `{start}-{end}` is reversed; start must not exceed end"
                ))
            } else {
                let lines: Vec<&str> = entry.content.lines().collect();
                if end > lines.len() {
                    ExpansionOutcome::Refused(format!(
                        "range `{start}-{end}` is out of bounds; `{id}` has {} line{}",
                        lines.len(),
                        if lines.len() == 1 { "" } else { "s" }
                    ))
                } else {
                    ExpansionOutcome::Content(format!("{}\n", lines[start - 1..end].join("\n")))
                }
            }
        }
    })
}

/// Map line 2004's file granularity: a line naming `file` is either the
/// bare path on its own (Glob-shaped output) or a `path:...` prefix
/// (ripgrep-shaped search-hit output) — the two file-per-line shapes this
/// build's own eligible tools (Grep, Glob) actually produce. An exact
/// prefix only: a line about a different file that merely contains `file`
/// as a substring must never match.
fn line_names_file(line: &str, file: &str) -> bool {
    let trimmed = line.trim();
    trimmed == file || trimmed.starts_with(&format!("{file}:"))
}

/// Map line 2004's range granularity: `START-END`, 1-indexed and
/// inclusive. Malformed input (non-numeric, no separator) is refused with
/// the same clear-error posture as a reversed or out-of-bounds range —
/// `context_firewall_show` never sees anything but a validated pair.
pub(crate) fn parse_line_range(spec: &str) -> Result<(usize, usize), String> {
    let (start, end) = spec
        .split_once('-')
        .ok_or_else(|| format!("`{spec}` is not a `START-END` line range"))?;
    let start: usize = start
        .trim()
        .parse()
        .map_err(|_| format!("`{spec}` is not a `START-END` line range"))?;
    let end: usize = end
        .trim()
        .parse()
        .map_err(|_| format!("`{spec}` is not a `START-END` line range"))?;
    Ok((start, end))
}

/// `context-firewall show <id> --stats`: the entry's own recorded map line
/// 2005 comparison — original/forwarded token estimates and
/// retained/total candidate counts — never its content. This is the
/// "check for yourself" surface a savings claim needs, kept separate from
/// content expansion rather than folded into it, so a caller can always
/// tell which one it asked for.
pub(crate) fn context_firewall_show_stats(
    runtime: &Runtime,
    id: &str,
) -> anyhow::Result<Option<String>> {
    use anyhow::Context as _;
    use std::fmt::Write as _;

    let store = glasshouse::firewall::RawStore::open(runtime.state_dir().join("context-firewall"));
    let entry = store
        .read(id)
        .with_context(|| format!("could not read the context-firewall raw store for `{id}`"))?;
    record_context_firewall_expansion(runtime, entry.as_ref().map(|entry| entry.tool.as_str()));
    let Some(entry) = entry else {
        return Ok(None);
    };

    let mut out = String::new();
    let _ = writeln!(out, "tool: {}", entry.tool);
    let _ = writeln!(out, "original_tokens: {}", entry.original_token_estimate);
    match entry.forwarded_token_estimate {
        Some(tokens) => {
            let _ = writeln!(out, "forwarded_tokens: {tokens}");
        }
        None => {
            let _ = writeln!(
                out,
                "forwarded_tokens: unknown (recorded before map line 2005)"
            );
        }
    }
    match (entry.retained_candidates, entry.total_candidates) {
        (Some(retained), Some(total)) => {
            let _ = writeln!(out, "retained_candidates: {retained}");
            let _ = writeln!(out, "total_candidates: {total}");
        }
        _ => {
            let _ = writeln!(
                out,
                "retained_candidates/total_candidates: unknown (recorded before map line 2005)"
            );
        }
    }
    Ok(Some(out))
}

/// Map line 1988: track raw-expansion requests as their own telemetry
/// rows, independent of map line 1987's reduction/bypass rows — a recall
/// regression must be measurable before any savings claim from those rows
/// is believed.
fn record_context_firewall_expansion(runtime: &Runtime, found_tool: Option<&str>) {
    use glasshouse::routing::evidence::{
        CONTEXT_FIREWALL_EXPANSION_PURPOSE, EvidenceLedger, NewObservation,
    };

    let ledger = match EvidenceLedger::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "routing evidence ledger unavailable; a context-firewall expansion request is \
                 not recorded"
            );
            return;
        }
    };
    let now_unix = glasshouse::provider::cache::now_unix_seconds();
    let observation = NewObservation::new("glasshouse", "context-firewall")
        .with_purpose(Some(CONTEXT_FIREWALL_EXPANSION_PURPOSE))
        .with_route(Some(if found_tool.is_some() {
            "found"
        } else {
            "not-found"
        }))
        .with_quota_context(found_tool)
        .with_timing(Some(now_unix), Some(now_unix));
    if let Err(err) = ledger.record(observation, now_unix) {
        tracing::warn!(
            error = %err,
            "could not record a context-firewall expansion request"
        );
    }
}

/// Phase 57B's production caller (map lines 1997, 2002): resolve
/// `[context_firewall].reducer` (and its optional `reducer_model` pin) into
/// a real [`glasshouse::firewall::reducer::Reducer`], routed through
/// [`glasshouse::routing::disposable::DisposableRouting`] over the same
/// candidates [`disposable_candidates`] builds for every other disposable
/// job — never a firewall-private provider client (map line 1997).
///
/// `None` whenever there is nothing to route: no `reducer` configured (map
/// line 1992's guarantee — an absent reducer disables the whole semantic
/// stage), no configured candidate matches it, or
/// [`glasshouse::routing::disposable::DisposableRouting::choose`] found no
/// resource at all — including because an entitlement's `deny_job_kinds`
/// refuses [`glasshouse::routing::disposable::JobKind::ContextReduction`]
/// for every matching candidate, which is this line's own per-entitlement
/// job-kind rule applying unchanged.
fn disposable_reducer(
    runtime: &Runtime,
    user: &UserConfig,
    project: Option<&ProjectConfig>,
    session_id: &str,
) -> Option<Box<dyn glasshouse::firewall::reducer::Reducer>> {
    use glasshouse::provider::registry::Locality;
    use glasshouse::routing::disposable::{DisposableRouting, JobKind};
    use glasshouse::routing::free::{FreePool, FreePreferences};

    let effective = EffectiveConfig::new(user, project);
    let reducer_ref = effective.context_firewall_reducer().value?;

    // Phase 58, map lines 2028-2030: `local:<name>` selects an installed
    // out-of-process tool from `[context_firewall.local_reducers.<name>]`
    // instead of routing through `DisposableRouting` at all — a local tool
    // is local by construction, so `reducer_local_only` is satisfied without
    // being consulted here, and nothing below this branch (provider/model
    // candidates, free-resource routing, entitlement job-kind gating) applies
    // to it. design-decisions.md's *The local reducer seat*.
    if let Some(name) = reducer_ref.strip_prefix("local:") {
        return local_disposable_reducer(runtime, user, project, &effective, session_id, name);
    }

    let reducer_model_pin = effective.context_firewall_reducer_model().value;
    let local_only = effective.context_firewall_reducer_local_only().value;

    let secrets = glasshouse::secret::native::PreferNativeSecretStore::detect();
    let now_unix = glasshouse::provider::cache::now_unix_seconds();
    let telemetry = glasshouse::provider::resources::GatheredTelemetry::new().gather_gateway_quota(
        &glasshouse::provider::telemetry::GatewayQuotaCache::new(runtime.paths()),
    );
    // Map line 1519: priced spend against every provider's own configured
    // money budget, for `disposable_candidates`' own exclusion — the same
    // fail-soft gather `disposable_extraction_model` makes (8320).
    let telemetry = match glasshouse::routing::evidence::EvidenceLedger::open(runtime) {
        Ok(ledger) => {
            let prices = glasshouse::provider::pricing::PriceTable::load_from_dir(
                runtime.paths().config_dir(),
            );
            telemetry.gather_budget_spend(&ledger, &prices, &effective, now_unix)
        }
        Err(err) => {
            tracing::debug!(
                error = %err,
                "could not read the routing evidence ledger to count budget spend for the \
                 context-firewall reducer"
            );
            telemetry
        }
    };
    let candidates = crate::commands::routing_classification::disposable_candidates(
        user, project, &effective, &secrets, &telemetry, now_unix,
    );

    let mut filtered: Vec<_> = candidates
        .into_iter()
        .filter(|candidate| {
            candidate.provider() == reducer_ref
                || candidate
                    .entitlement()
                    .is_some_and(|entitlement| entitlement.name() == reducer_ref)
        })
        .filter(|candidate| {
            reducer_model_pin
                .as_deref()
                .is_none_or(|model| candidate.model() == model)
        })
        .collect();
    if local_only {
        filtered.retain(|candidate| candidate.locality() == Some(Locality::Local));
    }
    if filtered.is_empty() {
        return None;
    }

    let free_preferences = FreePreferences::new()
        .with_order(
            effective
                .free_resource_order()
                .value
                .iter()
                .map(|order| order.to_key())
                .collect(),
        )
        .with_disabled(
            effective
                .free_resource_disabled()
                .value
                .iter()
                .map(|disabled| disabled.to_key())
                .collect(),
        )
        .with_pin(
            effective
                .free_resource_pin()
                .value
                .as_ref()
                .map(|pin| pin.to_key()),
        );
    let reserve_override = glasshouse::routing::disposable::ReserveOverride::for_sessions(
        effective.reserve_override_sessions().value,
    )
    .deciding_for(session_id.to_string());
    // Map lines 1294 and 1610's production wiring, scoped exactly as the
    // override above: the sessions that declared, paired with the session
    // this decision is actually for. `DeclaredTaskProgress::applies` is what
    // makes those two facts one input, and it is false for every session
    // nobody declared — including when the set is empty, which is every user
    // who has never run `glasshouse task-progress`.
    let task_progress = glasshouse::routing::disposable::DeclaredTaskProgress::for_sessions(
        crate::commands::sessions::declared_task_progress_sessions(runtime),
    )
    .deciding_for(session_id.to_string());
    let routing = DisposableRouting::for_support_work(
        effective.prefer_free_routing().value,
        free_preferences,
    )
    .with_reserve_override(reserve_override)
    .with_task_progress(task_progress)
    .with_reserve_policy(
        effective
            .reserve_policies()
            .for_scope(glasshouse::routing::pressure::ReserveScope::Background),
    );

    let pool = FreePool::new();
    let choice = routing
        .choose(
            JobKind::ContextReduction,
            &filtered,
            &pool,
            std::time::Instant::now(),
            None,
        )
        .ok()?;

    match context_firewall_reducer_model(user, project, choice.provider(), choice.model()) {
        Ok(reducer) => Some(Box::new(reducer)),
        Err(err) => {
            tracing::warn!(error = %err, "the configured context-firewall reducer cannot be used");
            None
        }
    }
}

/// `[context_firewall].reducer = "local:<name>"` — Phase 58, map lines
/// 2028-2030. Resolves `[context_firewall.local_reducers.<name>]` (project
/// before user, matching every other reducer field's own layering) into a
/// [`glasshouse::firewall::reducer::LocalToolReducer`], or logs why not and
/// leaves the whole semantic stage disabled for this hook invocation — the
/// same fail-open posture [`disposable_reducer`]'s own `Err` arm already
/// has. The child's cwd is a scratch directory under this session's own
/// state, never the project root; its environment is scrubbed of every
/// entitlement's credential variable via
/// [`EffectiveConfig::foreign_entitlement_credential_vars`], called with
/// `None` because a subprocess Glasshouse did not write is not "serving"
/// any entitlement and has no business carrying any of their keys.
fn local_disposable_reducer(
    runtime: &Runtime,
    user: &UserConfig,
    project: Option<&ProjectConfig>,
    effective: &EffectiveConfig,
    session_id: &str,
    name: &str,
) -> Option<Box<dyn glasshouse::firewall::reducer::Reducer>> {
    use glasshouse::firewall::reducer::LocalToolReducer;

    let config = project
        .and_then(|p| p.context_firewall().local_reducer(name))
        .or_else(|| user.context_firewall().local_reducer(name));
    let Some(config) = config else {
        tracing::warn!(
            reducer = name,
            "the context-firewall reducer names a local tool this project has not configured"
        );
        return None;
    };

    let scratch_dir = runtime
        .session_dir(session_id)
        .join("context-firewall-reducer");
    let credential_vars = effective.foreign_entitlement_credential_vars(None);

    match LocalToolReducer::new(name, config, scratch_dir, credential_vars) {
        Ok(reducer) => Some(Box::new(reducer)),
        Err(err) => {
            tracing::warn!(error = %err, reducer = name, "the configured local reducer cannot be used");
            None
        }
    }
}

/// Build the [`glasshouse::firewall::reducer::ConfiguredReducer`]
/// `DisposableRouting` chose — [`classification_model`]'s exact shape,
/// restated for the reducer's own type, since both build a real client from
/// a provider name and a model name after routing has already decided them.
fn context_firewall_reducer_model(
    user: &UserConfig,
    project: Option<&ProjectConfig>,
    provider_name: &str,
    model_name: &str,
) -> Result<glasshouse::firewall::reducer::ConfiguredReducer, String> {
    use glasshouse::firewall::reducer::{ConfiguredReducer, ConfiguredReducerError};
    use glasshouse::secret::{SecretRef, SecretStore as _};

    let Some(provider_config) = project
        .and_then(|p| p.providers().get(provider_name))
        .or_else(|| user.providers().get(provider_name))
    else {
        return Err(format!(
            "the context-firewall reducer names `{provider_name}`, which this project has not \
             configured"
        ));
    };
    if !provider_config.enabled() {
        return Err(format!(
            "the context-firewall reducer names `{provider_name}`, which is disabled"
        ));
    }
    let provider = provider_config.to_provider(provider_name).map_err(|err| {
        format!("the context-firewall reducer's provider does not resolve: {err}")
    })?;

    let secrets = glasshouse::secret::native::PreferNativeSecretStore::detect();
    let credential = provider
        .credential_env
        .iter()
        .find_map(|var| secrets.resolve(&SecretRef::Environment { var: var.clone() }));

    ConfiguredReducer::new(&provider, model_name, credential).map_err(|err| match err {
        ConfiguredReducerError::UnsupportedProtocol { protocol, .. } => format!(
            "the context-firewall reducer speaks OpenAI chat completions, and \
             `{provider_name}` serves `{protocol}`; configure a provider that serves \
             openai-chat"
        ),
        other => format!("the context-firewall reducer cannot be used: {other}"),
    })
}

#[cfg(test)]
mod tests {
    use super::project_relative_path;
    use std::path::Path;

    /// A Windows project root is `fs::canonicalize`'s output and therefore
    /// verbatim; the paths that arrive to be tested against it are not. Every
    /// case here is lexical — the function takes a `&Path` and a `&str` and
    /// touches no filesystem — so a literal verbatim root proves the Windows
    /// behaviour on whichever platform the gate happens to run on, which is
    /// the point: gated behind `cfg(windows)` the regression would be
    /// invisible where it is actually caught.
    #[test]
    fn a_verbatim_root_accepts_every_ordinary_spelling_of_a_file_inside_it() {
        let root = Path::new(r"\\?\C:\proj");
        for raw in [
            r"C:\proj\src\a.rs",
            "C:/proj/src/a.rs",
            r"\\?\C:\proj\src\a.rs",
            "src/a.rs",
        ] {
            assert_eq!(
                project_relative_path(root, raw).as_deref(),
                Some("src/a.rs"),
                "`{raw}` names a file inside the project and must resolve repo-relative"
            );
        }
    }

    /// The refusal adjacent to every acceptance above. Reducing both sides to
    /// one spelling must not cost the rule that the prefix ends at a
    /// separator, or `C:\proj-other` passes as a file inside `C:\proj`.
    #[test]
    fn a_verbatim_root_still_refuses_what_is_outside_it() {
        let root = Path::new(r"\\?\C:\proj");
        for raw in [
            r"C:\proj-other\a.rs",
            "C:/proj-other/a.rs",
            r"\\?\C:\proj-other\a.rs",
            r"C:\elsewhere\a.rs",
            r"D:\proj\a.rs",
            // Inside by spelling, outside by meaning: `normalize_observed_path`
            // refuses the `..` rather than resolving it.
            r"C:\proj\..\other\a.rs",
        ] {
            assert_eq!(
                project_relative_path(root, raw),
                None,
                "`{raw}` is outside the project and nothing outside it may be stored"
            );
        }
    }

    /// `\\?\UNC\srv\share` is the verbatim spelling of `\\srv\share`, so a
    /// project on a share has the same two-spellings problem a drive does.
    #[test]
    fn a_verbatim_unc_root_accepts_the_ordinary_unc_spelling_and_refuses_another_share() {
        let root = Path::new(r"\\?\UNC\srv\share\proj");
        for raw in [
            r"\\srv\share\proj\src\a.rs",
            r"\\?\UNC\srv\share\proj\src\a.rs",
            "src/a.rs",
        ] {
            assert_eq!(
                project_relative_path(root, raw).as_deref(),
                Some("src/a.rs"),
                "`{raw}` names a file inside the share's project"
            );
        }
        for raw in [
            r"\\srv\other\proj\src\a.rs",
            r"\\srv\share\proj-other\src\a.rs",
            r"C:\proj\src\a.rs",
        ] {
            assert_eq!(
                project_relative_path(root, raw),
                None,
                "`{raw}` is not inside the share's project"
            );
        }
    }

    /// **The isolation case, and the whole reason the reduction is guarded.**
    ///
    /// `//?/` is an unusual but perfectly legal absolute path on Unix. An
    /// unconditional strip would reduce `//?/proj/a.rs` to the *relative*
    /// `proj/a.rs`, and a relative path is accepted as already root-relative
    /// — so a file outside the project would be stored as though it were
    /// inside it, on every non-Windows platform, for the sake of a Windows
    /// repair. Nothing after `//?/` here is drive- or UNC-shaped, so nothing
    /// is stripped and each path is refused as the absolute stranger it is.
    #[test]
    fn the_verbatim_reduction_never_fires_on_a_unix_shaped_path() {
        let root = Path::new("/proj");
        for raw in [
            "//?/proj/a.rs",
            "//?/proj/src/a.rs",
            "//?/UNC/proj/a.rs",
            "//?/a.rs",
        ] {
            assert_eq!(
                project_relative_path(root, raw),
                None,
                "`{raw}` is not inside /proj, and stripping `//?/` from it would say it was"
            );
        }
    }

    /// Every answer the function gave before the reduction, unchanged: an
    /// ordinary root, an ordinary path, and the three refusals.
    #[test]
    fn an_ordinary_root_answers_exactly_as_it_did() {
        let root = Path::new("/proj");
        for (raw, expected) in [
            ("/proj/src/a.rs", Some("src/a.rs")),
            ("src/a.rs", Some("src/a.rs")),
            ("./src/a.rs", Some("src/a.rs")),
            // The path *is* the root: a directory, not a file anything edited.
            ("/proj", None),
            ("/proj/", None),
            ("/proj-other/a.rs", None),
            ("/etc/passwd", None),
            ("../outside/a.rs", None),
        ] {
            assert_eq!(
                project_relative_path(root, raw).as_deref(),
                expected,
                "`{raw}` under /proj"
            );
        }
    }

    /// A verbatim root compared against itself is still the root, not a file.
    #[test]
    fn a_path_equal_to_the_verbatim_root_is_still_not_a_file() {
        let root = Path::new(r"\\?\C:\proj");
        for raw in [r"\\?\C:\proj", r"C:\proj", r"C:\proj\", "C:/proj"] {
            assert_eq!(
                project_relative_path(root, raw),
                None,
                "`{raw}` is the project root itself"
            );
        }
    }
}
