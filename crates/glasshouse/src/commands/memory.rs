//! `commands::memory` -- moved verbatim from `main.rs` (Phase 59 decomposition).

use glasshouse::Runtime;
use glasshouse::checkpoint::WorkingTreeStatus;
use glasshouse::checkpoint::git::GitPosition;
use glasshouse::events::EventLog;
use glasshouse::session::ProjectSessions;

/// The one search this project's memory retrieval goes through — Phase 21F
/// line 929's grouping, and the shared core `memory_report` (the CLI's
/// `glasshouse memory search`) and `api::unix::query_memory` (the machine
/// door) both render from, so the two can never disagree about what a query
/// finds or how it is grouped.
///
/// `session` is the requesting session's id, when the caller has one in
/// scope — `GH-RETRIEVAL-ATTRIBUTION`'s gap 1. Both current callers pass
/// `None`: `memory_report`'s CLI command has no session to attribute a
/// person's own `memory search` to, and `query_memory`'s `Request::QueryMemory`
/// carries no session field to thread one from. Never guessed — see
/// [`glasshouse::evaluation::record_memory_retrieval`]'s own doc comment.
pub(crate) fn memory_search_grouped(
    runtime: &Runtime,
    query: &str,
    history: bool,
    limit: usize,
    session: Option<&str>,
) -> anyhow::Result<glasshouse::memory::search::RetrievalResult> {
    use glasshouse::memory::ProjectMemory;
    use glasshouse::memory::search::SearchScope;

    let scope = if history {
        SearchScope::Historical
    } else {
        SearchScope::Current
    };

    // The memory connection is opened, used and dropped before the evaluation
    // ledger opens its own. Two SQLite handles held over each other on one
    // file is practice §65's Windows hang, and there is no reason to hold both
    // here: the search is finished before the observation is written.
    let grouped = {
        let memory = ProjectMemory::open(runtime)?;
        memory.store().search_grouped(query, scope, limit)?
    };

    // Phase 51 lines 1822 and 1826: a retrieval is an ephemeral decision that
    // changes what the user gets and otherwise leaves no trace, so this is the
    // one place it becomes countable. One row per returned memory, carrying
    // `memory_id` and nothing of the memory itself; whether a memory was stale
    // is read later by joining `memories`, not judged here. This records and
    // never fails: bookkeeping does not get to break a search.
    //
    // Map line 1865: a search that returned nothing in either group records
    // one miss row instead — never both, and never neither.
    if grouped.invariants_and_constraints.is_empty() && grouped.other.is_empty() {
        glasshouse::evaluation::record_memory_retrieval_miss(
            runtime,
            glasshouse::evaluation::RetrievalScope::from_history_flag(history),
            glasshouse::evaluation::now_unix(),
        );
    } else {
        glasshouse::evaluation::record_memory_retrieval(
            runtime,
            glasshouse::evaluation::RetrievalScope::from_history_flag(history),
            grouped
                .invariants_and_constraints
                .iter()
                .chain(grouped.other.iter())
                .map(|record| record.id.as_str()),
            session,
            glasshouse::evaluation::now_unix(),
        );
    }

    Ok(grouped)
}

/// Render a memory search the way `session_report` renders sessions: the
/// provenance is part of the answer, because a memory a reader cannot trace
/// back to a session or a commit is one they have to take on trust.
///
/// The authority class is part of the answer for the same reason. Phase 21A's
/// fixed requirement is that retrieval preserve the distinction rather than
/// flatten every remembered statement into equally authoritative text, and
/// this is the one surface a person reaches.
pub(crate) fn memory_report(
    runtime: &Runtime,
    query: &str,
    history: bool,
    limit: usize,
) -> anyhow::Result<String> {
    let grouped = memory_search_grouped(runtime, query, history, limit, None)?;
    render_memory_report(&grouped, query, history)
}

/// `glasshouse memory search --path <p> [--for-edit]` — the CLI half of
/// capability map lines 1143, 1141 and 1142.
///
/// Answers from [`glasshouse::memory::MemoryStore::for_path`], the same
/// reader the socket door and the briefing's file section use, so the three
/// surfaces cannot disagree about what a file is associated with. Not
/// through [`memory_search_grouped`]: that records every retrieval as a
/// *search* in the evaluation ledger, and a path lookup runs no query.
///
/// `assoc=` is read per row (line 1139's second provenance), `freshness=` is
/// line 1142's commit-order label printed in rank order without reordering
/// or rescoring, and `for_edit` (line 1141) sorts constraints, decisions and
/// failed approaches ahead of features, findings and todos within each
/// authority rung; off, the order is byte-for-byte what `Lookup` gives.
///
/// History: design-decisions.md, "Trims: commands module docs", memory_path_report.
pub(crate) fn memory_path_report(
    runtime: &Runtime,
    path: &str,
    for_edit: bool,
    history: bool,
    limit: usize,
) -> anyhow::Result<String> {
    use glasshouse::checkpoint::git::{Freshness, last_change_commit};
    use glasshouse::memory::ProjectMemory;
    use glasshouse::memory::search::{RetrievalIntent, SearchScope};
    use std::fmt::Write as _;

    let scope = if history {
        SearchScope::Historical
    } else {
        SearchScope::Current
    };
    let intent = if for_edit {
        RetrievalIntent::CodeEdit
    } else {
        RetrievalIntent::Lookup
    };

    let project = ProjectMemory::open(runtime)?;
    let grouped = project.store().for_path(path, scope, limit, intent)?;
    drop(project);

    let mut out = String::new();
    if grouped.invariants_and_constraints.is_empty() && grouped.other.is_empty() {
        // Which of the two questions was asked, exactly as
        // `render_memory_report` distinguishes them: "nothing" after a
        // current-only lookup must not read as "this project remembers
        // nothing about this file".
        if history {
            writeln!(
                out,
                "No memories are associated with {path:?}, including history."
            )?;
        } else {
            writeln!(
                out,
                "No current memories are associated with {path:?}. Use --history to include \
                 superseded and resolved ones."
            )?;
        }
        return Ok(out);
    }

    let last_change = last_change_commit(runtime.project().root(), path);
    writeln!(
        out,
        "-- memories associated with {path} — advisory: the source at {path} is the evidence, \
         this is not --"
    )?;

    let write_row =
        |out: &mut String, record: &glasshouse::memory::MemoryRecord| -> anyhow::Result<()> {
            let association = grouped
                .association(&record.id)
                .map_or("unrecognised", |association| association.as_str());
            let freshness = Freshness::compare(
                runtime.project().root(),
                last_change.as_deref(),
                record.source_commit.as_deref(),
            );
            // Above the record rather than folded into it, so
            // `write_memory_record` stays the one renderer every memory surface
            // shares and this door adds a line instead of a dialect.
            writeln!(out, "   assoc={association} freshness={freshness}")?;
            write_memory_record(out, record)
        };

    if !grouped.invariants_and_constraints.is_empty() {
        writeln!(out, "-- current invariants & constraints --")?;
        for record in &grouped.invariants_and_constraints {
            write_row(&mut out, record)?;
        }
    }
    if !grouped.other.is_empty() {
        if !grouped.invariants_and_constraints.is_empty() {
            writeln!(out, "-- other results --")?;
        }
        for record in &grouped.other {
            write_row(&mut out, record)?;
        }
    }
    Ok(out)
}

/// `glasshouse memory search --explain` — map line 1094's other reader. Runs
/// the exact selection [`brief_launch_session`] and `api::unix::select_memory`
/// would run for `query` (`glasshouse::memory::inject::briefing_traced`, the
/// same seat, the same candidate limit, the same rerank), and prints the
/// [`glasshouse::memory::RetrievalTrace`] it built — never writes
/// `memory-retrieval.jsonl`, whatever `[memory] retrieval_diagnostics` says,
/// because `diagnostics: None` is passed regardless.
pub(crate) fn memory_search_explain(runtime: &Runtime, query: &str) -> anyhow::Result<String> {
    use glasshouse::memory::ProjectMemory;
    use glasshouse::memory::inject;
    use glasshouse::memory::rerank::explain_line;

    let model = glasshouse::memory::rerank::resolve_rerank_model(runtime);
    let project = ProjectMemory::open(runtime)?;
    let (_, trace) = inject::briefing_traced(
        &project.store(),
        query,
        &std::collections::HashSet::new(),
        model.as_deref(),
        None,
        // The same root the real briefing runs with, so `--explain`
        // describes the selection a session would actually get.
        Some(runtime.project().root()),
        // `None`: `--explain` describes what the search and selection would
        // pick, never whether line 1129 would withhold it — that decision is
        // the real door's alone.
        None,
    )?;
    drop(project);
    Ok(format!("{}\n", explain_line(&trace)))
}

/// Pure formatting half of [`memory_report`], separated so
/// `api::unix::query_memory` can render the identical text from a
/// [`glasshouse::memory::search::RetrievalResult`] it already has, without a
/// second trip through the database.
pub(crate) fn render_memory_report(
    grouped: &glasshouse::memory::search::RetrievalResult,
    query: &str,
    history: bool,
) -> anyhow::Result<String> {
    use std::fmt::Write as _;

    let mut out = String::new();
    if grouped.invariants_and_constraints.is_empty() && grouped.other.is_empty() {
        // Say which of the two questions was asked. "No memories" after a
        // default search would otherwise read as "this project remembers
        // nothing", when the history was simply not looked at.
        if history {
            writeln!(out, "No memories match {query:?}, including history.")?;
        } else {
            writeln!(
                out,
                "No current memories match {query:?}. Use --history to include \
                 superseded and resolved ones."
            )?;
        }
        return Ok(out);
    }

    // Phase 21F line 929: current invariants and constraints are printed as
    // their own group, ahead of and apart from everything else a search
    // matched, rather than left for a reader to tell apart from a rendered
    // string.
    if !grouped.invariants_and_constraints.is_empty() {
        writeln!(out, "-- current invariants & constraints --")?;
        for record in &grouped.invariants_and_constraints {
            write_memory_record(&mut out, record)?;
        }
    }
    if !grouped.other.is_empty() {
        if !grouped.invariants_and_constraints.is_empty() {
            writeln!(out, "-- other results --")?;
        }
        for record in &grouped.other {
            write_memory_record(&mut out, record)?;
        }
    }
    Ok(out)
}

/// `glasshouse memory retrievals`: how retrieval has been doing over a
/// window — map lines 1822 and 1826's own numbers, plus map line 1865's
/// miss count, giving [`glasshouse::evaluation::EvaluationObservations::stale_retrievals`]
/// its first production caller (practice §90; `phase-51.md`'s 1822/1826
/// re-open).
pub(crate) fn memory_retrievals_report(
    runtime: &Runtime,
    hours: u32,
    session: Option<&str>,
    limit: usize,
) -> anyhow::Result<String> {
    use glasshouse::evaluation::{EvaluationKind, EvaluationObservations};

    let ledger = EvaluationObservations::open(runtime)?;

    if let Some(session_id) = session {
        let rows = ledger.retrievals_for_session(session_id, limit)?;
        let project = glasshouse::memory::ProjectMemory::open(runtime)?;
        return Ok(render_session_retrievals(
            session_id,
            &rows,
            &project.store(),
        ));
    }

    let to = glasshouse::evaluation::now_unix();
    let from = to - i64::from(hours) * 3600;
    let counts = ledger.stale_retrievals(from, to)?;
    let missed = ledger.count(EvaluationKind::MemoryRetrievalMiss, from, to)?;
    let usefulness = ledger.usefulness(from, to)?;
    let prevented_repetition = ledger.prevented_repetition(from, to)?;
    let caused_complexity = ledger.caused_complexity(from, to)?;
    let revalidation_accuracy = ledger.revalidation_accuracy(from, to)?;
    let challenge_accuracy = ledger.challenge_accuracy(from, to)?;
    let false_positives_by_scope = ledger.false_positives_by_scope(from, to)?;
    Ok(render_memory_retrievals(
        runtime.project().id().as_str(),
        hours,
        &counts,
        missed,
        &usefulness,
        &prevented_repetition,
        &caused_complexity,
        &revalidation_accuracy,
        &challenge_accuracy,
        &false_positives_by_scope,
    ))
}

/// `glasshouse memory retrievals --session <id>`: which memories were
/// retrieved for one routed task — map line 1759. One line per row, newest
/// first, the memory's current kind and one-line summary when
/// [`glasshouse::memory::MemoryStore::get`] still finds it, and a marked line
/// rather than an error when it does not.
fn render_session_retrievals(
    session_id: &str,
    rows: &[glasshouse::evaluation::EvaluationObservation],
    store: &glasshouse::memory::MemoryStore<'_>,
) -> String {
    use std::fmt::Write as _;

    if rows.is_empty() {
        return format!("no memory was retrieved for session {session_id}\n");
    }

    let mut out = format!(
        "Memory retrievals for session {session_id} ({} row{})\n\n",
        rows.len(),
        if rows.len() == 1 { "" } else { "s" }
    );
    for row in rows {
        let scope = row.subject.as_deref().unwrap_or("(no scope)");
        let memory_id = row.memory_id.as_deref().unwrap_or("(no memory id)");
        let found = row
            .memory_id
            .as_deref()
            .map(glasshouse::memory::MemoryId::new)
            .and_then(|id| store.get(&id).ok().flatten());
        match found {
            Some(record) => {
                let _ = writeln!(
                    out,
                    "{}  {memory_id}  scope={scope}  {} {}",
                    row.observed_at,
                    record.kind,
                    crate::commands::shared::one_line(&record.body)
                );
            }
            None => {
                let _ = writeln!(
                    out,
                    "{memory_id}  scope={scope}  (memory no longer present)"
                );
            }
        }
    }
    out
}

/// Pure formatting half of [`memory_retrievals_report`].
///
/// **`stale` and `stale-under-history` are printed disjoint**, though
/// [`glasshouse::evaluation::StaleRetrievalCounts`] itself keeps
/// `stale_under_history` as a subset of `stale` by that struct's own
/// contract (`stale` counts every stale hit regardless of which scope asked
/// for it). This is the one place the distinction map line 1826 exists for
/// is rendered for a person: a superseded memory returned only because
/// `--history` explicitly asked for it is the tool doing what it was told,
/// not a defect, so it is printed once, under `stale-under-history`, and
/// subtracted out of `stale` rather than counted under both.
#[allow(clippy::too_many_arguments)]
fn render_memory_retrievals(
    project_id: &str,
    hours: u32,
    counts: &glasshouse::evaluation::StaleRetrievalCounts,
    missed: i64,
    usefulness: &glasshouse::evaluation::UsefulnessCounts,
    prevented_repetition: &glasshouse::evaluation::PreventedRepetitionCounts,
    caused_complexity: &glasshouse::evaluation::CausedComplexityCounts,
    revalidation_accuracy: &glasshouse::evaluation::RevalidationAccuracyCounts,
    challenge_accuracy: &glasshouse::evaluation::ChallengeAccuracyCounts,
    false_positives_by_scope: &[glasshouse::evaluation::FalsePositivesByScope],
) -> String {
    use std::fmt::Write as _;

    let stale_outside_history = counts.stale - counts.stale_under_history;
    let mut out = format!("Memory retrievals for project {project_id}, last {hours}h\n\n");
    let _ = writeln!(out, "  {:<20}{}", "returned", counts.retrievals);
    let _ = writeln!(out, "  {:<20}{}", "stale", stale_outside_history);
    let _ = writeln!(
        out,
        "  {:<20}{}",
        "stale-under-history", counts.stale_under_history
    );
    let _ = writeln!(out, "  {:<20}{}", "unresolved", counts.unresolved);
    let _ = writeln!(out, "  {:<20}{}", "missed", missed);

    let _ = write!(
        out,
        "\n{}",
        render_memory_quality(
            usefulness,
            prevented_repetition,
            caused_complexity,
            revalidation_accuracy,
            challenge_accuracy,
            false_positives_by_scope
        )
    );
    out
}

/// Memory quality for the same window `render_memory_retrievals` prints —
/// "Phase 51, the memory half of RC-B", user ruling 2026-09-02: an explicit
/// rating when given, a labelled `proxy` where the design decision defines
/// one, and `unknown`, always with its own denominator.
fn render_memory_quality(
    usefulness: &glasshouse::evaluation::UsefulnessCounts,
    prevented_repetition: &glasshouse::evaluation::PreventedRepetitionCounts,
    caused_complexity: &glasshouse::evaluation::CausedComplexityCounts,
    revalidation_accuracy: &glasshouse::evaluation::RevalidationAccuracyCounts,
    challenge_accuracy: &glasshouse::evaluation::ChallengeAccuracyCounts,
    false_positives_by_scope: &[glasshouse::evaluation::FalsePositivesByScope],
) -> String {
    use std::fmt::Write as _;

    let mut out = String::from("Memory quality\n\n");

    let rated = usefulness.explicit_useful + usefulness.explicit_not_useful;
    let _ = writeln!(out, "useful (1821):");
    let _ = writeln!(
        out,
        "  explicit useful {} / not-useful {} of {rated} rated",
        usefulness.explicit_useful, usefulness.explicit_not_useful
    );
    let _ = writeln!(
        out,
        "  proxy useful {} of {} retrieved-into-completed-turns",
        usefulness.proxy_useful, usefulness.proxy_denominator
    );
    let _ = writeln!(
        out,
        "  unknown {} of {} retrieved",
        usefulness.unknown, usefulness.retrieved
    );

    let _ = writeln!(out, "\nprevented-repetition (1831):");
    let _ = writeln!(
        out,
        "  explicit prevented-repetition {} of {} retrieved-failed-approach-memories",
        prevented_repetition.explicit, prevented_repetition.retrieved
    );
    let _ = writeln!(
        out,
        "  proxy prevented-repetition {} of {} retrieved-into-completed-turns",
        prevented_repetition.proxy, prevented_repetition.proxy_denominator
    );
    let _ = writeln!(
        out,
        "  unknown {} of {} retrieved-failed-approach-memories",
        prevented_repetition.unknown, prevented_repetition.retrieved
    );

    let _ = writeln!(out, "\ncaused-complexity (1823):");
    let _ = writeln!(
        out,
        "  explicit caused-complexity {} of {} retrieved-decision-memories",
        caused_complexity.explicit, caused_complexity.retrieved
    );
    let _ = writeln!(
        out,
        "  unknown {} of {} retrieved-decision-memories",
        caused_complexity.unknown, caused_complexity.retrieved
    );
    let _ = writeln!(out, "  no proxy: nothing observed bears on this");

    let _ = writeln!(out, "\nrevalidation-accuracy (1824):");
    let _ = writeln!(
        out,
        "  explicit revalidation-correct {} / revalidation-wrong {} of {} revalidations",
        revalidation_accuracy.correct,
        revalidation_accuracy.wrong,
        revalidation_accuracy.revalidations
    );
    let _ = writeln!(
        out,
        "  unknown {} of {} revalidations",
        revalidation_accuracy.unknown, revalidation_accuracy.revalidations
    );
    let _ = writeln!(out, "  no proxy: nothing observed bears on this");

    let _ = writeln!(out, "\nchallenge-accuracy (1825):");
    let _ = writeln!(
        out,
        "  explicit challenge-justified {} / challenge-unjustified {} of {} challenges",
        challenge_accuracy.justified, challenge_accuracy.unjustified, challenge_accuracy.challenges
    );
    let _ = writeln!(
        out,
        "  unknown {} of {} challenges",
        challenge_accuracy.unknown, challenge_accuracy.challenges
    );
    let _ = writeln!(out, "  no proxy: nothing observed bears on this");

    let _ = writeln!(out, "\nfalse positives by retrieval scope (939):");
    if false_positives_by_scope.is_empty() {
        let _ = writeln!(out, "  none recorded");
    } else {
        let find = |want: Option<&str>| {
            false_positives_by_scope
                .iter()
                .find(|row| row.scope.as_deref() == want)
        };
        for scope in ["current", "historical", "injection"] {
            let (not_useful, caused_complexity, retrieved) = find(Some(scope))
                .map(|row| (row.not_useful, row.caused_complexity, row.retrieved))
                .unwrap_or((0, 0, 0));
            let _ = writeln!(
                out,
                "  {scope}: not-useful {not_useful} / caused-complexity {caused_complexity} of {retrieved} retrieved"
            );
        }
        let (never_not_useful, never_caused_complexity) = find(None)
            .map(|row| (row.not_useful, row.caused_complexity))
            .unwrap_or((0, 0));
        let _ = writeln!(
            out,
            "  never retrieved: not-useful {never_not_useful} / caused-complexity {never_caused_complexity}"
        );
    }

    out
}

/// One memory, rendered the way [`memory_report`] prints every result.
fn write_memory_record(
    out: &mut String,
    record: &glasshouse::memory::MemoryRecord,
) -> anyhow::Result<()> {
    use std::fmt::Write as _;

    let subject = record.subject.as_deref().unwrap_or("(no subject)");
    // Phase 21A: retrieval must preserve the authority distinction rather
    // than flattening every memory into equally authoritative text. An
    // unclassified memory says so; it does not borrow a class.
    let authority = record.authority.map_or("unclassified", |a| a.as_str());
    // Phase 21B: *"treat a decision with missing rationale and missing
    // assumptions as lower-confidence."* The ranking already does it —
    // `memory::search::demote_thin_decisions` puts such a decision behind
    // a better-proven one of its own class. Saying so here is the other
    // half: a reader who cannot see *why* a decision sank has been given
    // a reordering and no reason for it.
    let confidence = if record.is_lower_confidence_decision() {
        "  lower-confidence"
    } else {
        ""
    };
    writeln!(
        out,
        "{}  {}  {authority}{confidence}  {subject}",
        record.kind, record.status
    )?;
    writeln!(out, "    {}", record.body)?;
    let provenance = provenance_lines(record);
    if !provenance.is_empty() {
        writeln!(out, "{provenance}")?;
    }
    // Phase 21F line 936: when this memory's authority means it may
    // constrain implementation, carry its validity and invalidation
    // conditions into the answer as well as its rationale — already printed
    // above, as `provenance_lines`'s "why" field.
    let constraint = constraint_lines(record);
    if !constraint.is_empty() {
        writeln!(out, "{constraint}")?;
    }
    // Phase 21F lines 937/938: a challenged memory must not read as settled.
    // Gated on `status`, not on `review_reason` alone, because a memory whose
    // review was resolved keeps its last `review_reason` on the record —
    // `MemoryStore::set_status` never clears it — so status is the only
    // field that says whether the challenge is still open.
    if record.status == glasshouse::memory::MemoryStatus::NeedsReview
        && let Some(reason) = record.review_reason
    {
        writeln!(
            out,
            "    challenged    {reason} — not returned as settled until resolved"
        )?;
    }
    // Map line 925: *"record why a decision was superseded so future agents do
    // not resurrect it without context."* `memory search --history` is where a
    // superseded memory is read at all, so it is where the context has to
    // arrive; printing the successor's identifier without the reason is
    // exactly the resurrection risk the line names.
    //
    // Gated on `superseded_reason` alone rather than on the status as well.
    // `MemoryStore::set_status` clears the column whenever a memory leaves
    // `Superseded`, so a reason present *is* a supersession in force — unlike
    // `review_reason` above, which survives its review being resolved and
    // therefore needs the status to disambiguate it.
    if let Some(reason) = &record.superseded_reason {
        writeln!(out, "    superseded    {reason}")?;
    }
    let session = record.source_session_id.as_deref().unwrap_or("unknown");
    let commit = record.source_commit.as_deref().unwrap_or("unknown");
    let events = record
        .source_events
        .map_or_else(|| "no event range".to_owned(), |events| events.to_string());
    writeln!(out, "    from session {session}, commit {commit}, {events}")?;
    // Phase 29: *every trigger names itself on the memory it produced.* On
    // its own line rather than appended above, because it answers a different
    // question from the three facts there — those say where this memory came
    // from, this says what made Glasshouse go and look.
    //
    // A memory with no recorded trigger prints **nothing** rather than
    // `unknown`, unlike its neighbours. Those three have been written for
    // every memory this build stores since Phase 20, so an `unknown` there
    // really does mean the producer did not know; a trigger is absent for
    // every memory recorded before the column existed, and a line reading
    // `trigger unknown` under all of them would be noise claiming to be a
    // finding.
    if let Some(trigger) = record.extraction_trigger.as_deref() {
        writeln!(out, "    trigger {trigger}")?;
    }
    Ok(())
}

/// Phase 21F line 936's conditional half: a memory's validity and
/// invalidation conditions are worth carrying only when its authority means
/// it may constrain implementation — an [`glasshouse::memory::MemoryAuthority::Invariant`],
/// a [`glasshouse::memory::MemoryAuthority::Constraint`], or an accepted
/// [`glasshouse::memory::MemoryAuthority::Decision`] (exactly
/// [`glasshouse::memory::MemoryAuthority::is_binding`]).
///
/// Explicit on `is_binding()` rather than "whichever fields happen to be
/// populated": an idea that recorded an invalidation condition anyway —
/// nothing in the schema stops it — must not read as though it could still
/// constrain anything.
fn constraint_lines(record: &glasshouse::memory::MemoryRecord) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    if !record
        .authority
        .is_some_and(glasshouse::memory::MemoryAuthority::is_binding)
    {
        return out;
    }
    if let Some(validity) = record.validity_conditions.as_deref() {
        let _ = writeln!(out, "    valid while  {validity}");
    }
    if let Some(invalidation) = record.invalidation_conditions.as_deref() {
        let _ = writeln!(out, "    invalid if   {invalidation}");
    }
    // The caller adds its own trailing newline, matching `provenance_lines`.
    out.pop();
    out
}

/// The Phase 21B provenance a search result carries, one labelled line per
/// field that has one.
///
/// Absent fields print nothing rather than `unknown`. There are nine of them
/// and a memory rarely has more than two; printing the absences would bury
/// the memory under a form. The one place absence *is* stated is the
/// `lower-confidence` marker beside the authority, which is where it changes
/// what the reader should do.
fn provenance_lines(record: &glasshouse::memory::MemoryRecord) -> String {
    use std::fmt::Write as _;

    let provenance = &record.provenance;
    let fields: [(&str, Option<&str>); 9] = [
        ("why", provenance.rationale.as_deref()),
        ("problem", provenance.problem.as_deref()),
        ("assumes", provenance.assumptions.as_deref()),
        ("scale", provenance.scale_assumptions.as_deref()),
        ("security", provenance.security_assumptions.as_deref()),
        ("compat", provenance.compatibility_assumptions.as_deref()),
        ("ops", provenance.operational_assumptions.as_deref()),
        ("evidence", provenance.evidence.as_deref()),
        ("quoted", provenance.source_excerpt.as_deref()),
    ];

    let mut out = String::new();
    if let Some(phase) = provenance.project_phase {
        let _ = writeln!(out, "    phase      {phase}");
    }
    for (label, value) in fields {
        if let Some(value) = value {
            let _ = writeln!(out, "    {label:<10} {value}");
        }
    }
    // The caller adds its own newline, so hand back a block without a
    // trailing blank line when there is nothing to say.
    out.pop();
    out
}

/// `glasshouse memory promote <id> <authority>` — Phase 21A's explicit
/// promotion. `Classifier::Reviewed`, because the person typing this is the
/// review the class requires.
pub(crate) fn memory_promote(
    runtime: &Runtime,
    id: &str,
    authority: &str,
) -> anyhow::Result<String> {
    use glasshouse::memory::{AuthorityChange, Classifier, MemoryAuthority, ProjectMemory};

    let wanted = match authority {
        "unclassified" | "none" => None,
        other => Some(MemoryAuthority::from_stored(other).ok_or_else(|| {
            anyhow::anyhow!(
                "`{other}` is not an authority class; use one of {} or `unclassified`",
                MemoryAuthority::ALL
                    .iter()
                    .map(|a| a.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?),
    };

    let memory = ProjectMemory::open(runtime)?;
    let store = memory.store();
    let resolved = store.resolve_id(id)?;
    let (record, change) = store.set_authority(&resolved, wanted, Classifier::Reviewed)?;

    Ok(match change {
        AuthorityChange::Changed => format!(
            "{} is now {}\n",
            record.id,
            record.authority.map_or("unclassified", |a| a.as_str())
        ),
        AuthorityChange::Unchanged => format!(
            "{} was already {}\n",
            record.id,
            record.authority.map_or("unclassified", |a| a.as_str())
        ),
    })
}

/// `glasshouse memory rate <id> <verdict> [--session <id>] [--note <text>]`
/// — "Phase 51, the memory half of RC-B", user ruling 2026-09-02: *"Both:
/// explicit rating when given, the labelled proxy otherwise."* This is the
/// explicit half — one new [`glasshouse::evaluation::EvaluationKind::MemoryRated`]
/// observation, never an edit of the retrieval it judges.
///
/// Project isolation the same way [`memory_challenge`] and
/// [`memory_resolve_conflict`] get it: [`glasshouse::memory::MemoryStore::resolve_id`]
/// refuses an id from another project by name before this ever opens the
/// evaluation ledger, so a rating can never be recorded against a memory
/// this project cannot see.
pub(crate) fn memory_rate(
    runtime: &Runtime,
    id: &str,
    verdict: glasshouse::evaluation::EvaluationOutcome,
    session: Option<&str>,
    note: Option<&str>,
) -> anyhow::Result<String> {
    use glasshouse::memory::ProjectMemory;

    let memory = ProjectMemory::open(runtime)?;
    let resolved = memory.store().resolve_id(id)?;

    glasshouse::evaluation::record_memory_rating(
        runtime,
        resolved.as_str(),
        verdict,
        session,
        note,
        glasshouse::evaluation::now_unix(),
    )?;

    Ok(format!(
        "{} rated {}\n",
        resolved.as_str(),
        verdict.as_str()
    ))
}

/// `glasshouse memory challenge <id> <reason>` — Phase 21F lines 937/938:
/// let the receiving agent say, explicitly, that current evidence
/// contradicts a memory, rather than silently distrusting it in a way
/// nothing records. Reuses Phase 21C's `mark_for_review` rather than
/// inventing a seventh reason: a challenge *is* "something changed that may
/// invalidate this" — the review mechanism already built for that.
///
/// The retrieval half of 937/938 is true the moment this returns:
/// `SearchScope::Current` only ever returns `Active` memories, so the
/// challenged memory drops out of every default search immediately and
/// stays reachable only as history.
///
/// 938's "before further automatic injection into the same task" has no
/// consumer here: Phase 27 (automatic injection) does not exist, so there
/// is nothing that injects a memory for this to gate. Closed on the
/// retrieval half only.
///
/// History: design-decisions.md, "Trims: commands module docs", memory_challenge.
pub(crate) fn memory_challenge(
    runtime: &Runtime,
    id: &str,
    reason: &str,
) -> anyhow::Result<String> {
    use glasshouse::memory::{ProjectMemory, ReviewReason};

    let parsed = ReviewReason::from_stored(reason).ok_or_else(|| {
        anyhow::anyhow!(
            "`{reason}` is not a review reason; use one of {}",
            ReviewReason::ALL
                .iter()
                .map(|r| r.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;

    let memory = ProjectMemory::open(runtime)?;
    let store = memory.store();
    let resolved = store.resolve_id(id)?;
    let record = store.mark_for_review(&resolved, parsed)?;

    Ok(format!(
        "{} is now {} ({}); it will not be returned as current until the challenge is \
         resolved. It remains searchable as history with --history.\n",
        record.id,
        glasshouse::memory::MemoryStatus::NeedsReview,
        parsed.as_str()
    ))
}

/// `glasshouse memory revalidate --list` — Phase 21G line 950's selection
/// half: the bounded queue of memories actually waiting for review, so
/// revalidation never becomes a sweep over the project's whole history.
/// Wires `MemoryStore::with_status`, which had no production caller before
/// this.
pub(crate) fn memory_revalidate_list(runtime: &Runtime, limit: usize) -> anyhow::Result<String> {
    use glasshouse::memory::{MemoryStatus, ProjectMemory};

    let memory = ProjectMemory::open(runtime)?;
    let store = memory.store();
    let waiting = store.with_status(MemoryStatus::NeedsReview, limit)?;

    if waiting.is_empty() {
        return Ok("no memory is waiting for review\n".to_owned());
    }

    let mut out = String::new();
    for record in &waiting {
        out.push_str(&format!(
            "{} {} ({})\n",
            record.id,
            record.subject.as_deref().unwrap_or(&record.body),
            record
                .review_reason
                .map_or("no reason recorded", |reason| reason.as_str())
        ));
    }
    Ok(out)
}

/// `glasshouse memory revalidate <id> <outcome>` — Phase 21G line 949: the
/// resolution `memory challenge` has always promised
/// (`main.rs::memory_challenge` prints *"it will not be returned as current
/// until the challenge is resolved"*) and this build has never shipped.
/// `<outcome>` is exactly the four words the line names.
///
/// Defaults to the reviewed actor: a person typing this command by hand is
/// the human review Phase 22's gate asks for. `--automatic` invokes the
/// automatic actor instead, purely so the refusal on a high-impact memory
/// (line 948) is reachable and testable — nothing in this build calls it
/// that way itself.
pub(crate) fn memory_revalidate(
    runtime: &Runtime,
    id: &str,
    outcome: &str,
    by: Option<&str>,
    reason: Option<&str>,
    automatic: bool,
) -> anyhow::Result<String> {
    use glasshouse::memory::{ConflictResolver, ProjectMemory, ReviewReason};

    let memory = ProjectMemory::open(runtime)?;
    let store = memory.store();
    let resolved = store.resolve_id(id)?;
    let actor = if automatic {
        ConflictResolver::Automatic
    } else {
        ConflictResolver::Reviewed
    };

    let record = match outcome {
        "reaffirmed" => {
            if by.is_some() || reason.is_some() {
                anyhow::bail!("`reaffirmed` takes neither --by nor --reason");
            }
            store.revalidate_reaffirmed(&resolved, actor)?
        }
        "needs-review" => {
            if by.is_some() {
                anyhow::bail!("`needs-review` does not take --by");
            }
            let reason = reason
                .ok_or_else(|| anyhow::anyhow!("`needs-review` requires --reason <REASON>"))?;
            let parsed_reason = ReviewReason::from_stored(reason).ok_or_else(|| {
                anyhow::anyhow!(
                    "`{reason}` is not a review reason; use one of {}",
                    ReviewReason::ALL
                        .iter()
                        .map(|r| r.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
            store.revalidate_needs_review(&resolved, parsed_reason, actor)?
        }
        "superseded" => {
            // Map line 925. `--reason` here is the operator's own sentence
            // about why this decision went, not `needs-review`'s six-value
            // vocabulary above — a different question with a different answer
            // type, which is why it is stored in its own column. Optional: a
            // supersession with nothing to say is still a supersession.
            let by = by.ok_or_else(|| anyhow::anyhow!("`superseded` requires --by <ID>"))?;
            let successor = store.resolve_id(by)?;
            store.revalidate_superseded(&resolved, &successor, reason, actor)?
        }
        "invalidated" => {
            if by.is_some() || reason.is_some() {
                anyhow::bail!("`invalidated` takes neither --by nor --reason");
            }
            store.revalidate_invalidated(&resolved, actor)?
        }
        other => anyhow::bail!(
            "`{other}` is not a revalidation outcome; use one of reaffirmed, needs-review, \
             superseded, invalidated"
        ),
    };

    // Map line 1824's own denominator, `GH-RETRIEVAL-ATTRIBUTION`: the store
    // mutation above is the real act and has already succeeded, so this row
    // records that a revalidation happened without being able to fail the
    // command that already did.
    glasshouse::evaluation::record_memory_revalidation(
        runtime,
        record.id.as_str(),
        outcome,
        glasshouse::evaluation::now_unix(),
    );

    Ok(format!("{} is now {}\n", record.id, record.status))
}

/// `glasshouse memory conflicts` — map line 922's surfacing half.
///
/// An ordinary `glasshouse memory search` can move two memories to
/// [`glasshouse::memory::MemoryStatus::Conflicted`]
/// (`memory::search::flag_contradictions` → `MemoryStore::mark_conflicted`),
/// which drops both out of every default search immediately —
/// `MemoryStatus::is_current` answers `false` for `Conflicted`, same as every
/// other non-`Active` status. Wires [`glasshouse::memory::MemoryStore::with_status`]
/// again, this time against `Conflicted` rather than `NeedsReview`: that
/// method already selects by the `status` column alone and never consulted
/// `is_current`, so listing a conflict needed no new store query, only a
/// second production call to the one that already exists.
pub(crate) fn memory_conflicts_list(runtime: &Runtime, limit: usize) -> anyhow::Result<String> {
    use glasshouse::memory::{MemoryStatus, ProjectMemory};

    let memory = ProjectMemory::open(runtime)?;
    let store = memory.store();
    let conflicted = store.with_status(MemoryStatus::Conflicted, limit)?;

    if conflicted.is_empty() {
        return Ok("no memory is conflicted\n".to_owned());
    }

    let mut out = String::new();
    for record in &conflicted {
        out.push_str(&format!(
            "{} {} ({})\n",
            record.id,
            record.subject.as_deref().unwrap_or(&record.body),
            record.authority.map_or("unclassified", |a| a.as_str())
        ));
    }
    Ok(out)
}

/// `glasshouse memory resolve <id> <outcome>` — map line 922's resolution
/// half: [`glasshouse::memory::MemoryStore::resolve_conflict`] is fully
/// implemented and tested and, before this, reachable only from `cargo test`.
///
/// Always calls it with [`glasshouse::memory::ConflictResolver::Reviewed`],
/// never `::Automatic`: a person typing this command by hand already is the
/// review Phase 22's gate asks for, and `::Automatic` would refuse every
/// binding-authority and every unclassified memory
/// (`MemoryStore::require_reviewed_for_high_impact`'s own documentation) —
/// the majority of them — which would make this command look broken rather
/// than working as designed. There is no `--automatic` flag here the way
/// `memory revalidate` has one: nothing in this build calls conflict
/// resolution automatically, so there is no refusal path this command needs
/// to make reachable.
pub(crate) fn memory_resolve_conflict(
    runtime: &Runtime,
    id: &str,
    outcome: &str,
) -> anyhow::Result<String> {
    use glasshouse::memory::{ConflictResolver, MemoryStatus, ProjectMemory};

    let outcome = match outcome {
        "active" => MemoryStatus::Active,
        "superseded" => MemoryStatus::Superseded,
        other => anyhow::bail!(
            "`{other}` is not a conflict outcome; use `active` to keep this memory as current \
             knowledge or `superseded` to record it as replaced"
        ),
    };

    let memory = ProjectMemory::open(runtime)?;
    let store = memory.store();
    let resolved = store.resolve_id(id)?;
    let record = store.resolve_conflict(&resolved, outcome, ConflictResolver::Reviewed)?;

    Ok(format!("{} is now {}\n", record.id, record.status))
}

/// `glasshouse memory commit` — map line 1148, *"allow a memory commit to be
/// triggered manually."* Calls [`run_extraction`] with
/// [`glasshouse::memory::ExtractionTrigger::Manual`], the same function the
/// `TurnEnded` and `PreCompact` arms of [`report_hook_with`] call, so the
/// event window, credential screen, duplicate check, bound, and the
/// working-tree and routing observations are identical by construction
/// rather than by two implementations agreeing. Deliberately not
/// [`memory_extract`], which evaluates the contract from a file without a
/// provider; this one asks the model the user configured.
///
/// Defaults to the most recently active session (`SessionStore::list`,
/// `last_activity_at DESC`); a project with no sessions is an error naming
/// the flag rather than a silent *stored 0*, which would be
/// indistinguishable from a model that looked and found nothing.
///
/// The session lookup is scoped so `ProjectSessions` closes before
/// [`run_extraction`] opens the event log and the memory store — one
/// database handle at a time, billed under Windows' mandatory locks
/// otherwise.
///
/// History: design-decisions.md, "Trims: commands module docs", memory_commit.
pub(crate) fn memory_commit(runtime: &Runtime, session: Option<&str>) -> anyhow::Result<String> {
    use std::fmt::Write as _;

    use glasshouse::memory::ExtractionTrigger;

    let id = {
        let sessions = ProjectSessions::open(runtime)?;
        let store = sessions.store();
        match session {
            Some(session) => store.resolve_id(session)?,
            None => store
                .list()?
                .into_iter()
                .next()
                .map(|record| record.id)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "this project has no sessions to commit; name one with --session"
                    )
                })?,
        }
    };

    let model = crate::commands::routing_classification::disposable_extraction_model(runtime, &id);
    let Some(outcome) = crate::commands::memory_extraction::run_extraction(
        runtime,
        &id,
        model,
        ExtractionTrigger::Manual,
    ) else {
        // `run_extraction` logged which of the two it was. Neither is a
        // failure of the command: nothing was stored, nothing was corrupted,
        // and the next commit will read the same activity.
        return Ok(format!(
            "memory commit for session {id} produced nothing;              see the log for why\n"
        ));
    };

    let mut out = String::new();
    writeln!(out, "trigger {}, model {}", outcome.trigger, outcome.model)?;
    writeln!(out, "session: {id}")?;
    if let Some(failure) = &outcome.failure {
        writeln!(out, "memory commit produced nothing: {failure}")?;
        return Ok(out);
    }
    writeln!(
        out,
        "stored {}, {} duplicate, {} speculative, {} rejected",
        outcome.stored(),
        outcome.duplicates,
        outcome.speculative,
        outcome.rejected.len()
    )?;
    for id in &outcome.recorded {
        writeln!(out, "    stored    {id}")?;
    }
    for rejection in &outcome.rejected {
        writeln!(out, "    rejected  {rejection}")?;
    }
    Ok(out)
}

pub(crate) fn memory_extract(
    runtime: &Runtime,
    session: &str,
    activity: Option<&std::path::Path>,
    from_events: bool,
    reply_from: &std::path::Path,
) -> anyhow::Result<String> {
    use std::fmt::Write as _;

    use anyhow::Context as _;
    use glasshouse::memory::extract::chunk::{ChunkLimits, SessionChunk};
    use glasshouse::memory::extract::lifecycle::{EVENT_WINDOW, chunk_for_session};
    use glasshouse::memory::{ExtractionTrigger, Extractor, ProjectMemory};

    let reply = std::fs::read_to_string(reply_from)
        .with_context(|| format!("read the model reply from {}", reply_from.display()))?;

    // Run from a person's own shell at a moment they chose, unlike the
    // post-turn hook path: the project's current commit is exactly "where the
    // project was when this was learned", and cheap to read — see
    // `GitPosition::detect`.
    let commit = GitPosition::detect(runtime.project().root()).map(|position| position.commit);

    // Two sources, and the difference between them is the provenance.
    //
    // A file of activity is text a person chose, and a memory extracted from
    // it can name the session but not which part of it — there is no event
    // range to name. The event log can: `chunk_for_session` narrows the range
    // to what actually reached the model, and every memory this run stores
    // carries it. That is Phase 21's *"store the originating session and
    // event references"* with a caller a person can actually run.
    let (chunk, source) = if from_events {
        let sessions = ProjectSessions::open(runtime)?;
        let id = sessions.store().resolve_id(session)?;
        let log = EventLog::open(runtime)?;
        let events = log.recent_for_session(&id, EVENT_WINDOW)?;
        let read = events.len();
        (
            chunk_for_session(&id, &events, commit.as_deref(), ChunkLimits::default()),
            format!("{read} recorded events for session {id}"),
        )
    } else {
        let activity = activity.expect("clap requires --activity unless --from-events");
        let activity_text = std::fs::read_to_string(activity)
            .with_context(|| format!("read session activity from {}", activity.display()))?;
        (
            SessionChunk::build(
                session,
                commit,
                activity_text.lines().map(str::to_owned),
                ChunkLimits::default(),
            ),
            format!("{}", activity.display()),
        )
    };

    // The same reading `run_extraction` takes, from the same producer, before
    // the model is asked. Here it is unambiguously cheap: this command is one
    // synchronous pass on the main thread, and the store below is the same
    // connection that is about to write the memories, so nothing opens a
    // second handle.
    let observed_files = WorkingTreeStatus::detect(runtime.project().root())
        .map(|status| status.changed_files)
        .unwrap_or_default();

    let memory = ProjectMemory::open(runtime)?;
    let store = memory.store();
    let model = crate::commands::memory_extraction::ReplyFromFile(reply);
    let outcome = Extractor::new(&store, &model).run(&chunk, ExtractionTrigger::Manual);

    // Observed, never referenced — see `record_observed_files` for the whole
    // argument. A clean tree, or an extraction that stored nothing, writes no
    // rows rather than an empty one. A failure is reported and does not fail
    // the command: the memories are already stored.
    if let Err(err) = store.record_observed_files(&outcome.recorded, &observed_files) {
        tracing::warn!(
            error = %err,
            "could not record which files were being worked on"
        );
    }

    let mut out = String::new();
    writeln!(out, "trigger {}, model {}", outcome.trigger, outcome.model)?;
    writeln!(out, "source: {source}")?;
    writeln!(
        out,
        "activity: {} entries, {} dropped, {} truncated, {} credentials redacted",
        chunk.entries().len(),
        outcome.activity_dropped,
        outcome.activity_truncated,
        outcome.redactions
    )?;
    if let Some(events) = chunk.source_events() {
        writeln!(out, "provenance: {events} of this project's log")?;
    }

    if let Some(failure) = &outcome.failure {
        writeln!(out, "extraction produced nothing: {failure}")?;
        return Ok(out);
    }

    writeln!(
        out,
        "stored {}, {} duplicate, {} speculative, {} rejected",
        outcome.stored(),
        outcome.duplicates,
        outcome.speculative,
        outcome.rejected.len()
    )?;
    for id in &outcome.recorded {
        writeln!(out, "    stored    {id}")?;
    }
    for (id, classification) in &outcome.lowered {
        // Name the rule that bound, not just the outcome: the point of
        // reporting a lowering at all is that a reader can see *why* the
        // model's declared class was not the stored one.
        let reasons = classification
            .reasons
            .iter()
            .map(|r| r.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        writeln!(
            out,
            "    lowered   {id}  {} -> {} ({reasons})",
            classification.declared.as_str(),
            classification.stored.as_str()
        )?;
    }
    for rejection in &outcome.rejected {
        writeln!(out, "    rejected  {rejection}")?;
    }
    Ok(out)
}

/// `glasshouse memory export --tracked` — Phase 50's tracked project
/// knowledge, and the only production caller of
/// [`glasshouse::memory::TrackedKnowledge::write`].
///
/// `tracked` gates writing outright: omitting `--tracked` prints an
/// explanation and writes nothing, so typing the subcommand alone is never
/// enough to put files in the tree. That is deliberately a second gate on top
/// of the subcommand existing at all — map lines 1810/1811 ask for an
/// explicit opt-in, not merely a discoverable one.
pub(crate) fn memory_export_tracked(
    runtime: &Runtime,
    tracked: bool,
    include_findings: bool,
    dry_run: bool,
) -> anyhow::Result<String> {
    use std::fmt::Write as _;

    use glasshouse::memory::{ProjectMemory, Selection, TrackedKnowledge};

    let mut out = String::new();
    if !tracked {
        writeln!(
            out,
            "tracked project knowledge is off by default; nothing was written. \
             Pass --tracked to opt in."
        )?;
        return Ok(out);
    }

    let memory = ProjectMemory::open(runtime)?;
    let selection = Selection { include_findings };
    let manifest = TrackedKnowledge::write(&memory, runtime.project().root(), selection, dry_run)?;

    if manifest.dry_run {
        writeln!(out, "dry run: nothing was written")?;
    }
    if manifest.written.is_empty() {
        writeln!(out, "no decisions or constraints to export yet")?;
    } else {
        for file in &manifest.written {
            writeln!(out, "{}  {}  {}", file.kind, file.id, file.path.display())?;
        }
    }
    writeln!(out, "{}", manifest.readme.display())?;

    if manifest.git_absent {
        writeln!(
            out,
            "note: {} has no .git directory; the files were still written",
            runtime.project().display_root().display()
        )?;
    }
    if manifest.gitignored {
        writeln!(
            out,
            "note: this project's .gitignore ignores .glasshouse/; the files were \
             still written, and Glasshouse does not edit .gitignore"
        )?;
    }

    Ok(out)
}

/// `glasshouse memory export-local` — map line 2040, Phase 58 item 6.
///
/// A sibling of [`memory_export_tracked`] above, not a variant of it:
/// [`MemoryCommand::Export`] projects tracked knowledge into
/// `.glasshouse/knowledge/`; this writes a gitignored harness file instead,
/// and is opt-in the same way — nothing here runs unless this subcommand is
/// typed.
///
/// `harness` defaults to
/// [`glasshouse::memory::export_local::LocalHarness::DEFAULT_SLUG`]
/// (`claude-code`), the only harness this build knows a native local
/// instruction file for.
pub(crate) fn memory_export_local(
    runtime: &Runtime,
    harness: Option<&str>,
    limit: usize,
    exclude: bool,
) -> anyhow::Result<String> {
    use glasshouse::memory::export_local::{self, LocalHarness};
    use glasshouse::memory::{MemoryKind, ProjectMemory};

    let harness_slug = harness.unwrap_or(LocalHarness::DEFAULT_SLUG);

    let memory = ProjectMemory::open(runtime)?;
    let store = memory.store();
    let mut records = store.binding(limit)?;
    records.extend(store.current_of_kind(MemoryKind::FailedAttempt, limit)?);

    let outcome = export_local::export(
        runtime.project().root(),
        harness_slug,
        &records,
        glasshouse::evaluation::now_unix(),
        exclude,
    )?;

    let exclude_note = match outcome.exclude {
        export_local::ExcludeAction::Added => "added to .git/info/exclude",
        export_local::ExcludeAction::AlreadyExcluded => "already gitignored",
        export_local::ExcludeAction::Skipped => "--no-exclude: left untouched",
        export_local::ExcludeAction::NotGitRepo => "no .git directory: nothing to exclude",
    };

    Ok(format!(
        "{harness_slug}: {} {} written to {} ({exclude_note})\n",
        outcome.exported,
        if outcome.block_present {
            "memories"
        } else {
            "memories (block removed)"
        },
        outcome.path.display(),
    ))
}
