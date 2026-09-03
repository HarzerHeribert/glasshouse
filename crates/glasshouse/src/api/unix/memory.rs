use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use glasshouse::Runtime;
use glasshouse::config::{self, EffectiveConfig, UserConfig};
use glasshouse::evaluation::{self, RetrievalScope};
use glasshouse::events::MessageOrigin;
use glasshouse::memory::inject::{self, BriefingOutcome, Injection};
use glasshouse::memory::{FileAssociation, MemoryId, ProjectMemory};
use glasshouse::session::SessionId;
use glasshouse::session::api::SessionApi;

use super::api_error;
use crate::api::protocol::Response;

/// Which of this project's memories each live session has already been sent
/// — capability map line 1135's *"already-aware hot session"*.
///
/// # Why this is in memory and not in the database
///
/// A hot session is a session this process holds a pseudo-terminal for. It
/// exists exactly as long as [`ServerContext`]'s own [`SessionRuntime`] does, and so
/// does the fact that it has already read a memory: a session that has been
/// restarted has read nothing, and a `glasshouse` process that is not this
/// one holds no such session at all (see `super`'s module doc comment). A
/// durable table would therefore record a claim that outlived the thing it
/// was about, and would have to be reconciled against a runtime it cannot
/// see.
///
/// Keyed by the session id's string rather than by [`SessionId`] so this map
/// needs nothing from the session type it does not already have.
pub(super) type Injected = Arc<Mutex<HashMap<String, HashSet<MemoryId>>>>;

/// The most memory identifiers remembered per session before this door stops
/// growing the set.
///
/// One delivery carries at most [`inject::MAX_INJECTED_MEMORIES`], so a
/// session has to be given more than fifty separate tasks, each selecting
/// entirely different memories, to reach this. Past it, the de-duplication
/// degrades toward re-injecting rather than toward unbounded growth in a
/// process that is meant to run for days.
const MAX_REMEMBERED_INJECTIONS: usize = 256;

/// The hard ceiling on how many memories [`Request::QueryMemory`] returns in
/// one call, regardless of the `limit` a caller asks for — capability map
/// line 1115's *"concise results rather than dumping the complete memory
/// database into agent context."*
///
/// Generous against the default of twenty and still far short of a project's
/// whole store, because the point of the bound is that the response size
/// stops depending on how much the project has accumulated. A caller that
/// wants more searches again with a narrower query, which is what a ranked
/// search is for.
const MAX_MEMORY_LIMIT: usize = 100;

/// The hard ceiling on entries in any one [`Request::CurrentMemory`] section,
/// regardless of the `limit` asked for — line 1115 again, per section rather
/// than per response, because `memory::snapshot::snapshot` budgets each
/// section independently.
const MAX_SNAPSHOT_SECTION_LIMIT: usize = 50;

/// The hard ceiling on characters in any one [`Request::CurrentMemory`]
/// entry's body, regardless of the `body_chars` asked for.
///
/// A snapshot is a summary an agent reads to orient itself; a memory it needs
/// in full is one identifier away through [`Request::GetMemory`], which is
/// where an untruncated body belongs. Without this pair of ceilings a caller
/// passing `usize::MAX` to both would receive exactly the complete dump this
/// line forbids.
const MAX_SNAPSHOT_BODY_CHARS: usize = 2000;

/// Take the injection ledger's lock, ignoring poisoning, for the reason
/// [`lock`] gives: a panicking handler must not permanently disable a
/// bookkeeping record that only ever makes deliveries quieter.
fn lock_injected(
    injected: &Injected,
) -> std::sync::MutexGuard<'_, HashMap<String, HashSet<MemoryId>>> {
    injected
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Choose the project memory a session about to be given `task` should have —
/// capability map lines 1125-1127 and 1131-1134.
///
/// The whole selection lives in
/// [`glasshouse::memory::inject::select_briefing`] (this door always supplies
/// `Some(task)`, so behaviour here is unchanged from when this called
/// [`inject::briefing`] directly — `GH-LAUNCH-BRIEFING` is the `None` caller,
/// for a launch with no task to query on), which is cross-platform and knows
/// nothing about this door; what belongs here is only the two things this
/// door owns: which project's memory is being read (the runtime this socket
/// was opened for — there is no request field naming a project, see
/// `super`'s module doc comment), and what this session has already been
/// sent.
///
/// # Never a reason to fail a delivery
///
/// Every failure path returns `None` and logs. A session that starts and
/// receives its task without memory is strictly better than one that does
/// not start, and a memory store that cannot be opened is not a reason to
/// refuse to talk to a worker. The error is logged rather than answered
/// with, and it never reaches the injected text: `database::DatabaseError`
/// names the project file's absolute path in every variant, and nothing this
/// module puts on a session's terminal is built from an error at all.
pub(super) fn select_memory(
    runtime: &Runtime,
    session: &SessionId,
    task: &str,
    injected: &Injected,
) -> Option<Injection> {
    let already = lock_injected(injected)
        .get(session.as_str())
        .cloned()
        .unwrap_or_default();

    let project = match ProjectMemory::open(runtime) {
        Ok(project) => project,
        Err(err) => {
            tracing::warn!(
                session = %session,
                error = %format!("{err:#}"),
                "could not open this project's memory to select context for a routed task"
            );
            return None;
        }
    };
    let rerank_model = glasshouse::memory::rerank::resolve_rerank_model(runtime);
    let diagnostics =
        memory_retrieval_diagnostics_enabled(runtime).then_some(inject::DiagnosticsRequest {
            runtime,
            session: Some(session.as_str()),
        });
    let outcome = match inject::select_briefing_traced(
        &project.store(),
        Some(task),
        &already,
        rerank_model.as_deref(),
        diagnostics,
        Some(runtime.project().root()),
    ) {
        Ok((outcome, _trace)) => Some(outcome),
        Err(err) => {
            tracing::warn!(
                session = %session,
                error = %err,
                "could not select project memory for a routed task"
            );
            None
        }
    };
    // The memory connection is dropped before the evaluation ledger opens —
    // practice §65, the same shape `memory_search_grouped` uses — so a miss
    // recorded below never holds both handles at once.
    drop(project);

    match outcome {
        Some(BriefingOutcome::Injected(injection)) => Some(injection),
        Some(BriefingOutcome::NothingMatched) => {
            // Map line 1865: this is the launch-time door, and unlike the
            // already-injected case below, nothing matched at all.
            evaluation::record_memory_retrieval_miss(
                runtime,
                RetrievalScope::Injection,
                evaluation::now_unix(),
            );
            None
        }
        Some(BriefingOutcome::NothingNew) | None => None,
    }
}

/// `[memory] retrieval_diagnostics`, resolved — map line 1094's gate on
/// whether [`select_memory`] writes `memory-retrieval.jsonl`. Mirrors
/// `main.rs::memory_retrieval_diagnostics_enabled`, which this door cannot
/// call (a separate binary crate); a configuration that cannot be read is
/// `false`, matching every other automatic-behaviour default's fail-safe
/// direction on this path.
fn memory_retrieval_diagnostics_enabled(runtime: &Runtime) -> bool {
    let Ok(user) = UserConfig::load(runtime.paths()) else {
        return false;
    };
    let Ok(project) = config::load_project_config(runtime.project()) else {
        return false;
    };
    EffectiveConfig::new(&user, project.as_ref())
        .memory_retrieval_diagnostics()
        .value
}

/// Deliver a selected briefing to `session`, and record what it carried.
///
/// # Line 1128: a message, not a write into the harness's own history
///
/// This goes through [`SessionApi::send_text`] — the same seam
/// `Request::SendMessage` uses — and touches no harness session file,
/// transcript or resume state. Glasshouse's memory arrives the way anything
/// else Glasshouse says arrives, which is what keeps it distinguishable from
/// the harness's own record of the conversation.
///
/// # Always [`MessageOrigin::Machine`], even under a person's own request
///
/// The briefing rides along with `Request::SendMessage`, which now carries an
/// origin — and this delivery deliberately ignores it. A person asking to
/// send a line did not write this text and has never seen it: it is selected
/// from the project's memory by [`select_memory`] and composed by
/// `memory::inject::briefing`. Stamping it with the requester's origin would
/// record Glasshouse's own words as the person's, which is the exact
/// confusion the origin exists to end. The person's line, sent immediately
/// after this one, carries their origin; this one is Glasshouse speaking and
/// says so.
///
/// # Injection failure is never a delivery failure
///
/// A refused or failed injection is logged and swallowed. The ledger is
/// updated only on a send that actually succeeded, so a memory that did not
/// arrive is not recorded as one the session already has.
pub(super) fn deliver_memory(
    runtime: &Runtime,
    api: &mut SessionApi<'_>,
    session: &SessionId,
    briefing: Option<Injection>,
    injected: &Injected,
) {
    let Some(briefing) = briefing else { return };
    if let Err(err) = api.send_text(session, briefing.text(), MessageOrigin::Machine) {
        tracing::warn!(
            session = %session,
            error = %api_error(err),
            "could not deliver this project's memory to a session; its task is being sent without it"
        );
        return;
    }

    // Map lines 1821 and 1831's proxy join, `GH-RETRIEVAL-ATTRIBUTION`: this
    // door already holds the `SessionId` it is briefing, so a successful
    // delivery is where a retrieval finally gets one. One row per memory
    // actually in this delivery — never for a memory `select_memory`'s own
    // `already` set suppressed, because that memory was not delivered here
    // and recording it would count a repeat as a fresh retrieval.
    evaluation::record_memory_retrieval(
        runtime,
        RetrievalScope::Injection,
        briefing.memories().iter().map(|id| id.as_str()),
        Some(session.as_str()),
        evaluation::now_unix(),
    );

    let mut ledger = lock_injected(injected);
    let seen = ledger.entry(session.as_str().to_owned()).or_default();
    if seen.len() < MAX_REMEMBERED_INJECTIONS {
        seen.extend(briefing.memories().iter().cloned());
    }
}

/// A memory-side failure this door may put on the wire, as a message.
///
/// `MemoryStoreError` is written for exactly this audience — it names the
/// memory, both project identifiers, and the operation that failed, and
/// nothing else. Everything else that can surface from opening the project's
/// memory is a `database::DatabaseError`, and **every one of its variants
/// names the database file's absolute path** (`crates/glasshouse/src/
/// database.rs`'s error enum). That path lies outside the project root and
/// outside what this door is scoped to, so it does not leave here — a caller
/// on the far end of a socket cannot repair the file anyway, and the class of
/// failure is the whole of what it can act on.
///
/// Down-cast rather than matched on a string, and `pub(crate)`-invisible
/// `DatabaseError` is deliberately not named here: this stays correct if a
/// new error type joins the anyhow chain, because anything that is not a
/// `MemoryStoreError` is treated as the unsafe case.
pub(super) fn memory_error_message(err: &anyhow::Error) -> String {
    match err.downcast_ref::<glasshouse::memory::MemoryStoreError>() {
        Some(store_error) => store_error.to_string(),
        None => "the project's memory database could not be opened".to_owned(),
    }
}

/// One selected memory in full — capability map line 1112's `memory.get`.
///
/// # The read boundary is the whole point (line 1114)
///
/// `memory` is an identifier or an unambiguous prefix of one, and it is the
/// only caller-supplied value on this path. It cannot name a project: there
/// is no project field on this door (see `super`'s module doc comment), the
/// store is opened from the [`Runtime`] this process was started for, and
/// `MemoryStore::get` compares the row's own `project_id` against the active
/// one before handing anything back.
///
/// A row belonging to another project comes back as
/// `MemoryStoreError::ForeignProject` — **an error, never an empty answer**.
/// That distinction is the requirement, not an implementation detail: an
/// agent reading `null` would conclude the memory does not exist, when what
/// actually happened is that this project's file holds a row it must not
/// read. `MemoryStore::resolve_id` is deliberately left unscoped for the same
/// reason — scoping the prefix lookup by project would turn a foreign row
/// back into a silent "not found", which is precisely the answer the store's
/// own doc comment refuses to give.
pub(super) fn get_memory(runtime: &Runtime, memory: &str) -> Response {
    use glasshouse::memory::{MemoryStoreError, ProjectMemory};

    let project = match ProjectMemory::open(runtime) {
        Ok(project) => project,
        Err(err) => return Response::err(memory_error_message(&err)),
    };
    let store = project.store();

    let id = match store.resolve_id(memory) {
        Ok(id) => id,
        Err(err) => return Response::err(err),
    };
    match store.get(&id) {
        Ok(Some(record)) => Response::ok(memory_full_json(&record)),
        // `resolve_id` found the row and `get` did not, which means it was
        // deleted between the two statements. `NotFound` rather than a null
        // result, for [`get_memory`]'s own reason.
        Ok(None) => Response::err(MemoryStoreError::NotFound { id }),
        Err(err) => Response::err(err),
    }
}

/// A concise snapshot of what this project currently knows — capability map
/// line 1113's `memory.current`.
///
/// Answered from `memory::snapshot::snapshot`, the same producer the TUI's
/// project overview reads (`shell::build_project_overview_memory`), so the
/// two cannot disagree about what "current" means. There is no second
/// snapshot implementation behind this door and there must not be one.
///
/// # Bounded on both axes, server-side (line 1115)
///
/// A caller's `limit` and `body_chars` are each `min`'d against a constant
/// here before they reach [`glasshouse::memory::snapshot::SnapshotBudget`],
/// so they may only ever *lower*
/// the ceiling. Passing `usize::MAX` to both — the executable form of
/// "dumping the complete memory database into agent context" — yields the
/// same bounded response as passing the ceiling itself.
///
/// # Sections, not a flattened dump
///
/// The response keeps `snapshot`'s own structure: one entry per
/// `MemoryKind`, present even when empty, each reporting how many entries it
/// left out. A section that hit its cap says so, and a body that was cut says
/// so, so a caller can tell "this project has nothing of that kind" from
/// "there is more of it than you asked for" without a second call.
pub(super) fn current_memory(runtime: &Runtime, limit: usize, body_chars: usize) -> Response {
    use glasshouse::memory::ProjectMemory;
    use glasshouse::memory::snapshot::{SnapshotBudget, snapshot};

    let project = match ProjectMemory::open(runtime) {
        Ok(project) => project,
        Err(err) => return Response::err(memory_error_message(&err)),
    };
    let store = project.store();

    let budget = SnapshotBudget::new(
        limit.min(MAX_SNAPSHOT_SECTION_LIMIT),
        body_chars.min(MAX_SNAPSHOT_BODY_CHARS),
    );
    match snapshot(&store, &budget) {
        Ok(snapshot) => Response::ok(serde_json::json!({
            "sections": snapshot
                .sections
                .iter()
                .map(snapshot_section_json)
                .collect::<Vec<_>>(),
            // The budget actually applied, not the one asked for, so a caller
            // that named a larger number learns what it got rather than
            // inferring it from a section that happens to be short.
            "budget": {
                "per_section_limit": budget.per_section_limit,
                "max_body_chars": budget.max_body_chars,
            },
        })),
        Err(err) => Response::err(err),
    }
}

/// One [`glasshouse::memory::snapshot::SnapshotSection`], as JSON.
fn snapshot_section_json(
    section: &glasshouse::memory::snapshot::SnapshotSection,
) -> serde_json::Value {
    use glasshouse::memory::MemoryKind;

    serde_json::json!({
        "kind": MemoryKind::as_str(section.kind),
        "entries": section.entries.iter().map(snapshot_entry_json).collect::<Vec<_>>(),
        "omitted": section.omitted,
    })
}

/// One snapshot entry, as JSON — line 1116's provenance, at the resolution a
/// snapshot has it.
///
/// A `SnapshotEntry` carries the two fields that *locate* a memory —
/// `source_session_id` and `source_commit` — and not the ten
/// `DecisionProvenance` fields that explain it, because the snapshot's job is
/// to be concise (line 1115). They are one [`Request::GetMemory`] away, which
/// is the division of labour those two lines describe between them: enough
/// provenance here to know where to look, all of it there.
fn snapshot_entry_json(entry: &glasshouse::memory::snapshot::SnapshotEntry) -> serde_json::Value {
    use glasshouse::memory::MemoryAuthority;

    serde_json::json!({
        "id": entry.id.as_str(),
        "subject": entry.subject,
        "body": entry.body,
        "body_truncated": entry.body_truncated,
        "authority": entry.authority.map(MemoryAuthority::as_str),
        "provenance": {
            "source_session_id": entry.source_session_id,
            "source_commit": entry.source_commit,
        },
    })
}

/// Search this project's durable memory — box 10, capability map line 1111's
/// project-scoped `memory.search`, and Phase 21F lines 935/936: the machine
/// door carries each result's authority, validity state, and — for a memory
/// that may constrain implementation — its rationale and invalidation
/// conditions, as structured fields rather than only inside a rendered
/// string.
///
/// # Project scope, and why there is no project argument
///
/// Line 1114. The scope is structural: this door is opened for one resolved
/// [`Runtime`] and no request field names a project (see `super`'s module doc
/// comment), and `MemoryStore::search` filters on `memories.project_id` in
/// its own `WHERE` clause underneath that rather than trusting it. The two
/// are independent, which is the point — see `memory::store`'s own
/// "Project isolation" section for why the read boundary is not redundant
/// with the trigger.
///
/// `invariants_and_constraints`/`other` is `main.rs`'s own
/// `memory_search_grouped` (line 929), the exact search
/// `glasshouse memory search` runs; `report` is `render_memory_report`'s
/// exact text over the same result, so this door and that command can never
/// disagree about what a query finds. One search, not two: the CLI's report
/// text is rendered from the already-fetched grouping rather than searched
/// for a second time.
///
/// # `path`, capability map line 1143
///
/// `path` present switches this to [`query_memory_for_path`] and `query` is
/// not consulted — see [`Request::QueryMemory`]'s own doc comment for why a
/// path lookup has no text to search. `path` absent is byte-for-byte what
/// this door has always answered.
pub(super) fn query_memory(
    runtime: &Runtime,
    query: &str,
    history: bool,
    limit: usize,
    path: Option<&str>,
) -> Response {
    // Line 1115. The cap is applied here rather than left to the caller's
    // `limit`, and it is a `min` rather than a rejection: a caller asking for
    // more than the door will give gets the door's answer, not an error, the
    // same shape [`project_events`] uses for `MAX_EVENTS_LIMIT`.
    let limit = limit.min(MAX_MEMORY_LIMIT);

    if let Some(path) = path {
        return query_memory_for_path(runtime, path, history, limit);
    }

    // `None`: `Request::QueryMemory` carries no session field to attribute
    // this search to — see `memory_search_grouped`'s own doc comment.
    let grouped = match crate::commands::memory::memory_search_grouped(
        runtime, query, history, limit, None,
    ) {
        Ok(grouped) => grouped,
        // Through [`memory_error_message`], not `Response::err(err)` directly:
        // this anyhow chain carries a `database::DatabaseError` when the
        // project's database cannot be opened, and every variant of that type
        // names the file's absolute path. See that function.
        Err(err) => return Response::err(memory_error_message(&err)),
    };
    let report = match crate::commands::memory::render_memory_report(&grouped, query, history) {
        Ok(report) => report,
        Err(err) => return Response::err(err),
    };

    Response::ok(serde_json::json!({
        "invariants_and_constraints": grouped
            .invariants_and_constraints
            .iter()
            .map(memory_result_json)
            .collect::<Vec<_>>(),
        "other": grouped.other.iter().map(memory_result_json).collect::<Vec<_>>(),
        "report": report,
    }))
}

/// [`query_memory`]'s `path` mode — capability map line 1143, *"the
/// rationale behind a file-related constraint"* — through
/// [`glasshouse::memory::MemoryStore::for_path`], migration 17's read door
/// rather than a text search.
///
/// # `association`, and why it is always `"observed"`
///
/// Every row carries an `association` field alongside the body and rationale
/// [`memory_result_json`] already puts there — line 1143 asks for the
/// rationale *behind a constraint*, and which relationship produced the row
/// is part of reading that rationale honestly. `for_path`'s own doc comment
/// says it does not narrow by [`FileAssociation`], and this build's only
/// writer, `MemoryStore::record_observed_files`, only ever stores
/// [`FileAssociation::Observed`] — so `association` is that constant on
/// every row today, not a per-row lookup this door invents. It reads
/// `"observed"` rather than `"referenced"` for the same reason
/// `memory::inject`'s line 1140 section does: the file changed during the
/// session that produced the memory, which this build can prove; that the
/// memory refers to the file is map line 1139's own claim, and 1139 is not
/// satisfied by anything shipped here.
///
/// No `report`: `render_memory_report`'s prose is written for a text search
/// and would misdescribe a path lookup as one, so this answers with `path`
/// naming what was looked up instead. `query` is not accepted here — see
/// [`query_memory`].
///
/// Opens the project's memory directly, the same shape [`get_memory`] and
/// [`current_memory`] use, rather than through `crate::commands::memory::memory_search_grouped`:
/// that helper is `main.rs`'s text-search core and records every retrieval
/// through it as a *search* (`evaluation::record_memory_retrieval`); a path
/// lookup runs no query and recording it as one would misreport what was
/// asked. `glasshouse memory search --path` is the same reader again, so the
/// CLI and this door cannot disagree about what a file is associated with.
///
/// One `git log` for the whole answer and at most two `merge-base` per row:
/// every row is about the same file, so the last-change commit is read once.
fn query_memory_for_path(runtime: &Runtime, path: &str, history: bool, limit: usize) -> Response {
    use glasshouse::checkpoint::git::{Freshness, last_change_commit};
    use glasshouse::memory::search::{RetrievalIntent, SearchScope};

    let project = match ProjectMemory::open(runtime) {
        Ok(project) => project,
        Err(err) => return Response::err(memory_error_message(&err)),
    };
    let scope = if history {
        SearchScope::Historical
    } else {
        SearchScope::Current
    };
    let grouped = match project
        .store()
        .for_path(path, scope, limit, RetrievalIntent::Lookup)
    {
        Ok(grouped) => grouped,
        Err(err) => return Response::err(err),
    };

    let root = runtime.project().root();
    let last_change = last_change_commit(root, path);
    let row = |record: &glasshouse::memory::MemoryRecord| {
        file_observed_memory_json(
            record,
            grouped.association(&record.id),
            Freshness::compare(
                root,
                last_change.as_deref(),
                record.source_commit.as_deref(),
            ),
        )
    };

    Response::ok(serde_json::json!({
        "invariants_and_constraints": grouped
            .invariants_and_constraints
            .iter()
            .map(&row)
            .collect::<Vec<_>>(),
        "other": grouped.other.iter().map(&row).collect::<Vec<_>>(),
        "path": path,
    }))
}

/// [`memory_result_json`] plus the three fields [`query_memory_for_path`]
/// adds — see that function for what each means.
///
/// `association` is `null` for a row whose stored provenance this build does
/// not recognise, which is
/// [`glasshouse::memory::search::RetrievalResult::association`]'s own answer
/// carried through rather than defaulted to the weaker of the two words this
/// build knows.
///
/// `advisory` is the constant `true`, and that is the point: it is not a
/// property of the row that could ever be `false`, it is map line 1142's
/// statement about this whole door, put where a machine reading one row
/// cannot miss it.
fn file_observed_memory_json(
    record: &glasshouse::memory::MemoryRecord,
    association: Option<FileAssociation>,
    freshness: glasshouse::checkpoint::git::Freshness,
) -> serde_json::Value {
    let serde_json::Value::Object(mut fields) = memory_result_json(record) else {
        // Unreachable: `memory_result_json` is a `json!({...})` object
        // literal, the same guarantee `memory_full_json` relies on above.
        return memory_result_json(record);
    };
    fields.insert(
        "association".to_owned(),
        match association {
            Some(association) => serde_json::json!(association.as_str()),
            None => serde_json::Value::Null,
        },
    );
    fields.insert("advisory".to_owned(), serde_json::json!(true));
    fields.insert(
        "freshness".to_owned(),
        serde_json::json!(freshness.as_str()),
    );
    serde_json::Value::Object(fields)
}

/// One memory, as JSON for the machine door — Phase 21F lines 935/936.
///
/// `may_constrain_implementation` is `MemoryAuthority::is_binding` made
/// explicit as its own field rather than left for a caller to infer from
/// which of `validity_conditions`/`invalidation_conditions` happen to be
/// non-null: those two are deliberately `null` for a memory that is not
/// binding even if the row somehow carries text in them, for the same
/// reason `main.rs`'s `constraint_lines` gates on it rather than on
/// presence.
///
/// `review` is `null` unless `status` is currently `needs_review`: 21C's
/// `review_reason` is not cleared when a review is resolved back to another
/// status (`MemoryStore::set_status` never touches it), so it is stale
/// information once the status has moved on, and only `status` says whether
/// a challenge is still open.
fn memory_result_json(record: &glasshouse::memory::MemoryRecord) -> serde_json::Value {
    use glasshouse::memory::{MemoryAuthority, MemoryStatus};

    let may_constrain_implementation = record.authority.is_some_and(MemoryAuthority::is_binding);
    let review = (record.status == MemoryStatus::NeedsReview)
        .then_some(record.review_reason)
        .flatten()
        .map(|reason| {
            serde_json::json!({
                "reason": reason.as_str(),
                "marked_at": record.review_marked_at,
            })
        });

    serde_json::json!({
        "id": record.id.as_str(),
        "kind": record.kind.as_str(),
        "authority": record.authority.map(MemoryAuthority::as_str),
        "status": record.status.as_str(),
        "current": record.is_current(),
        "subject": record.subject,
        "body": record.body,
        "may_constrain_implementation": may_constrain_implementation,
        "rationale": record.provenance.rationale,
        "validity_conditions": if may_constrain_implementation {
            record.validity_conditions.clone()
        } else {
            None
        },
        "invalidation_conditions": if may_constrain_implementation {
            record.invalidation_conditions.clone()
        } else {
            None
        },
        "provenance": provenance_json(record),
        "review": review,
        "last_validated_at": record.last_validated_at,
        "created_at": record.created_at,
    })
}

/// Everything that lets a caller trace one memory back to where it came from
/// — capability map line 1116, *"include provenance with machine-retrieved
/// memory so an agent can verify important claims against source or code."*
///
/// Deliberately the vocabulary `tests/memory_provenance.rs` already proves
/// round-trips, field for field and spelling for spelling — the two
/// *locating* fields `source_session_id` and `source_commit`, the event
/// slice, and all ten of Phase 21B's `DecisionProvenance` fields — rather
/// than a second provenance shape invented for this door. An agent that
/// wants to check a claim against code has `source_commit`; against the
/// conversation that produced it, `source_session_id` and `source_events`;
/// against the reasoning, `rationale`, `evidence` and `source_excerpt`.
///
/// Every field is `null` when absent and never `""` or `0` (§71): a decision
/// nobody recorded a security assumption for is a different fact from one
/// that recorded there was none, and `MemoryRecord`'s own doc comments say
/// so field by field.
///
/// `rationale` also appears at the top level of [`memory_result_json`], where
/// Phase 21F line 936 put it; it is repeated rather than moved so that this
/// change adds a field to the door's answer and removes none.
///
/// # Secrets
///
/// Nothing here is a credential by construction. `memory::store`'s module
/// documentation states there is no column for a token, a key, or a provider
/// secret; the screening is on the producer side, where
/// `memory::extract::schema::judge` inspects each emitted element **whole**
/// before any field is read. `source_excerpt` is the sharpest of these
/// because it is verbatim session text — and it is exactly as screened as
/// `body`, which this door has carried since Phase 21F. This is a `json!`
/// over named fields, never a `Debug` format of a struct, so the
/// `provider/discovery.rs::ProbeRequest` shape cannot reappear here.
fn provenance_json(record: &glasshouse::memory::MemoryRecord) -> serde_json::Value {
    use glasshouse::memory::ProjectPhase;

    let provenance = &record.provenance;
    serde_json::json!({
        "source_session_id": record.source_session_id,
        "source_commit": record.source_commit,
        "source_events": record.source_events.map(|events| {
            serde_json::json!({ "first": events.first, "last": events.last })
        }),
        "project_phase": provenance.project_phase.map(ProjectPhase::as_str),
        "problem": provenance.problem,
        "rationale": provenance.rationale,
        "assumptions": provenance.assumptions,
        "scale_assumptions": provenance.scale_assumptions,
        "security_assumptions": provenance.security_assumptions,
        "compatibility_assumptions": provenance.compatibility_assumptions,
        "operational_assumptions": provenance.operational_assumptions,
        "evidence": provenance.evidence,
        "source_excerpt": provenance.source_excerpt,
    })
}

/// One memory with nothing elided — [`Request::GetMemory`]'s answer, and
/// capability map line 1112's *"in full"*.
///
/// [`memory_result_json`] plus what only a single-memory lookup has room to
/// say: the supersession relationship and the reason for it (line 925), when
/// the row was last written, whether it is still open work, and — stated
/// rather than left to be assumed — that this body was **not** cut, which is
/// the one thing distinguishing it from the same memory seen through
/// [`Request::CurrentMemory`].
///
/// `validity_conditions` and `invalidation_conditions` stay gated on
/// `may_constrain_implementation`, exactly as in the search answer: "in full"
/// means nothing is elided, not that a non-binding memory starts being
/// presented as one that constrains implementation. That gate is Phase 21F
/// line 936's, and one verb relaxing it would make the door disagree with
/// itself about the same row.
fn memory_full_json(record: &glasshouse::memory::MemoryRecord) -> serde_json::Value {
    let serde_json::Value::Object(mut fields) = memory_result_json(record) else {
        // Unreachable: `memory_result_json` is a `json!({...})` object
        // literal. Answering with the search shape rather than unwrapping
        // keeps an impossible case off this door's panic path all the same —
        // a panic here would take the whole server down, not one request.
        return memory_result_json(record);
    };

    fields.insert(
        "superseded_by".to_owned(),
        serde_json::json!(record.superseded_by.as_ref().map(|id| id.as_str())),
    );
    fields.insert(
        "superseded_reason".to_owned(),
        serde_json::json!(record.superseded_reason),
    );
    fields.insert(
        "open_todo".to_owned(),
        serde_json::json!(record.is_open_todo()),
    );
    fields.insert("body_truncated".to_owned(), serde_json::json!(false));
    fields.insert(
        "updated_at".to_owned(),
        serde_json::json!(record.updated_at),
    );

    serde_json::Value::Object(fields)
}

/// The project's current binding memory, rendered for a checkpoint's
/// `Handoff::memory` — line 1641.
///
/// Identical to `main.rs`'s function of the same name, and duplicated for the
/// reason [`request_checkpoint`] itself is: opening the project's memory
/// database or reading its binding records must never fail a checkpoint, so
/// either failure degrades to an empty list rather than propagating.
pub(super) fn binding_memory_lines(runtime: &Runtime) -> Vec<String> {
    use glasshouse::memory::ProjectMemory;

    let Ok(memory) = ProjectMemory::open(runtime) else {
        return Vec::new();
    };
    let Ok(records) = memory.store().binding(20) else {
        return Vec::new();
    };
    records
        .into_iter()
        .map(|record| match record.subject {
            // Phase 20 allows an absent subject; rendering an empty one would
            // print a heading nobody wrote.
            Some(subject) => format!("{subject}: {}", record.body),
            None => record.body,
        })
        .collect()
}
