//! The system block of one task: the prompt, its facts and manifest, and
//! the scouting preflight block (moved out of `session.rs` for the Phase 59
//! size ratchet, 2026-09-13; nothing here is new).

use super::*;

/// The system block, and it is [`prompt::render_system`]'s bytes and nothing
/// else -- `model-contract.md` §1: the preamble, one declaration per
/// registered tool, then the project's own instructions.
///
/// **The joining of the instruction documents is all this function decides.**
/// Map line 2448 fixes what is loaded, not how it is joined; everything from
/// the preamble outwards is `prompt`'s, whose own golden test pins it byte for
/// byte, so there is no second spelling of the contract here to drift from it.
pub(super) fn build_system_prompt(
    web: &crate::web::WebConfig,
    agents: &crate::config::AgentsConfig,
    helpers: &crate::config::HelpersConfig,
    decisions: &crate::config::DecisionsConfig,
    served: &[crate::models::RosterModel],
    profile: &Profile,
    interface: crate::abi::Interface,
    manifest: &crate::manifest::Manifest,
) -> String {
    // Configuration/grants remain session-scoped; guidance is read fresh.
    let instructions = crate::project::instructions::root(profile);
    // The Runtime block declares `web` only when `[web]` reaches something,
    // the same predicate the runtime binds it on (map 2658).
    let web = prompt::declarations::WebReach::from_config(web);
    // The models a subagent may be sent to: the gateway's figures, resolved
    // once at session start, and `[agents]` as it stands now -- a tier
    // assignment can change the posture mid-session, the served models
    // cannot change at all.
    let agents = prompt::declarations::AgentRoster {
        posture: prompt::declarations::AgentsPosture::from_config(agents),
        models: served.to_vec(),
    };
    let mut system = prompt::render_system_reaching(
        &instructions,
        &registry::ALL.iter().collect::<Vec<_>>(),
        &session_facts_with(profile, interface, manifest),
        crate::runtime::bindings::HostGlobals::Every,
        prompt::Reach {
            web: web.as_ref(),
            agents: Some(&agents),
            // The same predicate `system_manifest` writes its `Unavailable:`
            // line on, so the roster and its refusal cannot disagree.
            helpers: Some(helpers.model.is_some() && helpers.enabled),
            // The same predicate the runtime binds `decide` on, so an
            // unconfigured session is told of no global it does not have.
            decisions: decisions.model.is_some(),
        },
    );
    if profile.os_sandbox_bypassed() {
        system.push_str(
            "\n\nDANGER: Pane's OS process sandbox is disabled by an explicit CLI bypass. The surrounding container or VM is the only process boundary.",
        );
    }
    system.push_str("\n\n");
    system.push_str(&crate::project::orientation::collect(profile));
    system
}
/// [`build_system_prompt`] for a session that is already running: the same
/// block, from what the session holds.
pub(super) fn system_prompt_for(session: &Session<'_>) -> String {
    build_system_prompt(
        &session.config().web,
        &session.config().agents,
        &session.config().helpers,
        &session.config().decisions,
        &session.roster,
        session.profile,
        session.interface.get(),
        &session.manifest,
    )
}

/// The most files one preflight serves in full, and the most bytes one of
/// them may hold to be served at all.
///
/// **A file too large to serve whole is not served.** The section says *in
/// full*, and a truncated file under that heading is a claim the model cannot
/// check: it would read the first half as the whole of it.
pub(super) const PREFLIGHT_SERVE_FILES: usize = 6;
pub(super) const PREFLIGHT_SERVE_BYTES: u64 = 32 * 1024;

/// The acceptance lister: one toolless request that turns the task into the
/// items its completion is checked against (`acceptance.rs`). Runs once per
/// task before the first turn when `[helpers] acceptance_list` is on; a
/// request too short to need the repository gets none.
pub(super) fn acceptance_block(
    task: &str,
    session: &Session<'_>,
) -> Option<(
    String,
    Vec<crate::acceptance::Item>,
    crate::helpers::HelperRecord,
)> {
    let helpers = session.config().helpers.clone();
    if !helpers.enabled || !helpers.acceptance_list || !request_may_need_the_repository(task) {
        return None;
    }
    let model = helpers.model.as_deref()?;
    let effort = helpers.effort.for_helper("accept")?;
    let token = invoke::CancellationToken::new();
    session.interrupt.arm(token.clone());
    let record = crate::helpers::acceptance_list(
        task,
        crate::helpers::HelperRoute::new(model, effort),
        session.profile,
        session.glasshouse,
        session.id,
        &token,
    )?;
    if record.outcome.cancelled {
        session.interrupt.consumed();
    }
    output::acceptance_helper(&record);
    if !record.outcome.ok {
        session_println!(
            "acceptance: the lister did not answer ({})",
            record.outcome.text.lines().next().unwrap_or("").trim()
        );
        return None;
    }
    let items = crate::acceptance::parse(&record.outcome.text);
    if items.is_empty() {
        session_println!("acceptance: the lister named nothing verifiable");
        return None;
    }
    session_println!(
        "acceptance: {} item(s) derived from the request",
        items.len()
    );
    Some((crate::acceptance::render_list(&items), items, record))
}

/// The decision hold, applied right before a cell or direct frame would run
/// (`session.rs`'s `act_on`, at the `run_cell`/`run_direct_frame` site) --
/// moved out of `session.rs` for the Phase 59 size ratchet, 2026-09-16.
/// `Some(step)` means the caller returns it at once, without running
/// anything; `None` means continue exactly as `act_on` already does.
///
/// `cell::compile` here is a second **parse**, never a second run -- V8 has
/// not seen `source` yet, and `runtime.run_cell`/`run_direct_frame` parse it
/// again themselves (`runtime/cell.rs::compile` touches no isolate).
pub(super) fn apply_decision_hold(
    session: &Session<'_>,
    task_state: &mut TaskState,
    runtime: &Runtime,
    lowered: Option<&crate::abi::Lowered>,
    source: &str,
    calls: &[(&String, &String, &serde_json::Value)],
) -> Option<Step> {
    let effect = if let Some(lowered) = lowered {
        crate::decide::direct_frame_names_effect(&lowered.calls).map(|name| (name, None))
    } else {
        crate::runtime::cell::compile(source, runtime.next_cell())
            .ok()
            .and_then(|compiled| crate::decide::names_effect(&compiled.free_names))
            .map(|(name, offset)| {
                let (line, _) = crate::runtime::cell::line_and_column(source, offset);
                (name, Some(line))
            })
    };
    let decisions = &session.config().decisions;
    match crate::decide::hold_for(
        decisions.mode,
        task_state.intent.as_ref(),
        decisions.hold_above,
        effect.as_ref().map(|(name, line)| (name.as_str(), *line)),
        task_state.effect_holds > 0,
    ) {
        crate::decide::Hold::Run => {
            apply_drift_hold(session, task_state, runtime, effect.as_ref(), source, calls)
        }
        crate::decide::Hold::Overridden => {
            task_state.effect_overrides += 1;
            output::decisions(task_state.decisions_telemetry(&session.config().decisions));
            None
        }
        crate::decide::Hold::Shadow(_) => {
            task_state.would_hold += 1;
            output::decisions(task_state.decisions_telemetry(&session.config().decisions));
            None
        }
        crate::decide::Hold::Held(block) => {
            task_state.effect_holds += 1;
            output::decisions(task_state.decisions_telemetry(&session.config().decisions));
            // One `tool_result` per provider call this turn requested,
            // exactly as a refused turn answers above `act_on`'s own
            // ProtocolError case -- `calls` is the raw list scraped from the
            // assistant message either way, so this covers a native
            // `execute_cell` call and a lowered direct frame alike; a bare
            // markdown program made no call and gets none.
            let native_result = (!calls.is_empty()).then(|| Message {
                role: Role::User,
                content: calls
                    .iter()
                    .map(|(id, _, _)| Block::ToolResult {
                        tool_use_id: (*id).clone(),
                        content: block.clone(),
                        is_error: false,
                    })
                    .collect(),
                historical: None,
            });
            Some(Step {
                answer: Some(block.clone()),
                historical: Some(block),
                native_result,
                response: None,
                prose: false,
                record: None,
                rollback: None,
                view: CellView::default(),
            })
        }
    }
}

/// The drift question (2643), asked only after the intent hold above has
/// itself returned `Run` for this cell -- an effectful cell running while a
/// read-only intent is not confident enough to hold, or while the intent is
/// not read-only at all, still gets one chance to be checked against the
/// plan's own current step. `effect` is `None` for a pure cell, in which case
/// nothing is ever asked.
///
/// Unlike the intent question (asked once, before the first turn, and cached
/// on [`TaskState::intent`]), this question is asked fresh before every
/// candidate effectful cell, including the one re-issued after a hold --
/// [`crate::decide::drift_for`]'s own once rule (`already_held`) is what
/// keeps that second ask from holding the cell again, exactly as
/// [`crate::decide::hold_for`]'s `already_held` does for the intent hold.
fn apply_drift_hold(
    session: &Session<'_>,
    task_state: &mut TaskState,
    runtime: &Runtime,
    effect: Option<&(String, Option<u32>)>,
    source: &str,
    calls: &[(&String, &String, &serde_json::Value)],
) -> Option<Step> {
    effect?;
    let decisions = &session.config().decisions;
    let model = decisions.model.as_deref()?;
    if decisions.mode == crate::config::DecisionMode::Off {
        return None;
    }
    let plan = runtime.plan();
    let step = plan
        .iter()
        .find(|item| item.status == crate::runtime::outcome::PlanStatus::Active)?;
    let already_held = task_state.drift_holds > 0;
    task_state.drift_asked += 1;
    let answer = match crate::decide::drift_satisfied(model, &task_state.task, &step.text, source) {
        Ok(noul) => Some(noul),
        Err(_) => {
            task_state.drift_failed += 1;
            None
        }
    };
    match crate::decide::drift_for(
        decisions.mode,
        answer,
        decisions.drift_no_below,
        &step.text,
        already_held,
    ) {
        crate::decide::Drift::Run => None,
        crate::decide::Drift::Shadow(_) => {
            task_state.would_drift += 1;
            output::decisions(task_state.decisions_telemetry(&session.config().decisions));
            None
        }
        crate::decide::Drift::Held(block) => {
            task_state.drift_holds += 1;
            output::decisions(task_state.decisions_telemetry(&session.config().decisions));
            let native_result = (!calls.is_empty()).then(|| Message {
                role: Role::User,
                content: calls
                    .iter()
                    .map(|(id, _, _)| Block::ToolResult {
                        tool_use_id: (*id).clone(),
                        content: block.clone(),
                        is_error: false,
                    })
                    .collect(),
                historical: None,
            });
            Some(Step {
                answer: Some(block.clone()),
                historical: Some(block),
                native_result,
                response: None,
                prose: false,
                record: None,
                rollback: None,
                view: CellView::default(),
            })
        }
    }
}

/// The decision model's one request, asked once per task beside
/// [`preflight_block`] and [`append_acceptance`] -- before the first turn,
/// never inside it. Carries both the intent and complexity questions
/// (`docs/product/pane/decision-model.md`; F2 for the latter). Runs on its
/// own thread, joined here: the 2 s bound is already inside the request
/// (`decide::DECISION_TIMEOUT`), the same reason `helpers.rs::wait_for_helper`
/// moves a side errand off the caller's own thread. A failed or absent
/// decision leaves the task exactly as it is today; the error is recorded in
/// the notice, never surfaced as a task failure.
pub(super) fn task_decision(
    task: &str,
    session: &Session<'_>,
) -> (Option<crate::decide::TaskDecision>, u32) {
    let decisions = session.config().decisions.clone();
    let Some(model) = decisions.model else {
        return (None, 0);
    };
    if decisions.mode == crate::config::DecisionMode::Off {
        return (None, 0);
    }
    let request = task.to_string();
    let handle = std::thread::spawn(move || crate::decide::task_questions(&model, &request));
    match handle.join() {
        Ok(Ok(decision)) => {
            session_println!(
                "decision: intent {} ({:.2}), complexity {} ({:.2}), {} ms",
                decision.intent.choice,
                decision.intent.confidence,
                decision.complexity.choice,
                decision.complexity.confidence,
                decision.intent.latency_ms
            );
            (Some(decision), 0)
        }
        Ok(Err(error)) => {
            session_println!("decision: no answer ({error})");
            (None, 1)
        }
        Err(_) => {
            session_println!("decision: no answer (the request panicked)");
            (None, 1)
        }
    }
}

/// The acceptance list in the task's system block: derived once from the
/// request, shown beside the preflight, paid for as one helper call, and
/// returned for the task state to check when the model claims completion.
pub(super) fn append_acceptance(
    task: &str,
    session: &Session<'_>,
    transcript: &mut Transcript,
    budget: &mut TaskSpend,
) -> Vec<crate::acceptance::Item> {
    let Some((block, items, record)) = acceptance_block(task, session) else {
        return Vec::new();
    };
    transcript.conversation.system.push_str(&block);
    budget.add_helpers(std::slice::from_ref(&record));
    items
}

/// The stand-in gate: a request of fewer words than this gets no preflight.
pub(super) const PREFLIGHT_MIN_WORDS: usize = 4;

/// Whether this request plausibly needs the repository at all.
///
/// A request under [`PREFLIGHT_MIN_WORDS`] words — "hi", "thanks", "carry
/// on" — gets no preflight; everything else is decided by
/// [`crate::preflight::should_scout`] on the request's own signals.
pub(super) fn request_may_need_the_repository(task: &str) -> bool {
    task.split_whitespace().count() >= PREFLIGHT_MIN_WORDS
}

/// [`preflight_block`]'s result: the system-prompt block, if a scout ran and
/// answered, plus what the decision model's complexity answer did to that
/// choice -- `scout_signal` and `would_scout` feed straight into
/// `TaskState::with_decision`'s telemetry.
pub(super) struct PreflightOutcome {
    pub(super) block: Option<String>,
    /// Whether `preflight::SIGNAL_DECIDED_EXPLORATION` was one of the reasons
    /// this task actually ran a scout (`mode = on` only).
    pub(super) scout_signal: bool,
    /// Whether the complexity answer would have added that signal had
    /// `mode` been `on` (`mode = shadow` only; recorded, changes nothing).
    pub(super) would_scout: bool,
}

impl PreflightOutcome {
    const NONE: Self = Self {
        block: None,
        scout_signal: false,
        would_scout: false,
    };
}

/// One preflight block to append to this task's system prompt, or `None` when
/// no scout ran or none answered.
///
/// The invariant: **a failed preflight leaves the session exactly as it is
/// today**, and **the scout never attempts the task**: it is handed a
/// scouting brief built around the verbatim request and the manifest, and
/// answers with constraints, files, tests, capabilities and risks
/// (`smarter-cheaper-roadmap.md`, *Preflight Helper*). With
/// `preflight_scope = "auto"` it runs only when the request carries an
/// uncertainty signal, so a task that names existing files and available
/// tools pays nothing -- unless `decision` names `needs_exploration` at or
/// above `scout_above` while `[decisions] mode = "on"` (F2): that signal
/// can only add a reason to run the scout, never remove one, and `mode =
/// "shadow"` only records what would have happened (`would_scout`).
pub(super) fn preflight_block(
    task: &str,
    session: &Session<'_>,
    transcript: &mut Transcript,
    decision: Option<&crate::decide::TaskDecision>,
) -> PreflightOutcome {
    let helpers = session.config().helpers.clone();
    if !helpers.enabled || !helpers.preflight || !request_may_need_the_repository(task) {
        return PreflightOutcome::NONE;
    }
    let decisions = session.config().decisions.clone();
    let complexity = decision.map(|decision| &decision.complexity);
    let clears_scout_above = complexity.is_some_and(|complexity| {
        complexity.choice == crate::decide::NEEDS_EXPLORATION
            && complexity.confidence >= decisions.scout_above
    });
    let would_scout = decisions.mode == crate::config::DecisionMode::Shadow && clears_scout_above;
    let decided_for_scout = if decisions.mode == crate::config::DecisionMode::On {
        complexity
    } else {
        None
    };
    let checks_configured = crate::verification::load(session.profile)
        .map(|config| !config.checks.is_empty())
        .unwrap_or(false);
    let scouting_decision = crate::preflight::should_scout(
        task,
        &session.manifest,
        helpers.preflight_scope,
        checks_configured,
        decided_for_scout,
        decisions.scout_above,
    );
    let scout_signal = matches!(
        &scouting_decision,
        crate::preflight::Decision::Run(signals)
            if signals.contains(&crate::preflight::SIGNAL_DECIDED_EXPLORATION)
    );
    session_println!(
        "preflight: {}",
        crate::preflight::signals_summary(&scouting_decision)
    );
    if matches!(scouting_decision, crate::preflight::Decision::Skip(_)) {
        return PreflightOutcome {
            block: None,
            scout_signal,
            would_scout,
        };
    }
    let none = PreflightOutcome {
        block: None,
        scout_signal,
        would_scout,
    };
    let Some(model) = helpers.model.as_deref() else {
        return none;
    };
    let Some(effort) = helpers.effort.for_helper("find") else {
        return none;
    };
    let token = invoke::CancellationToken::new();
    session.interrupt.arm(token.clone());
    let brief = crate::preflight::scouting_brief(task, &session.manifest);
    // Ranking and judging (2644, 2645): `decisions.model` set and `mode`
    // not `off` is the same gate the intent/complexity question already
    // uses above. `apply` carries `mode = on` versus `shadow` -- shadow
    // still asks and counts, it just never reorders what the Scout is
    // served or writes a line into what it returned.
    let decisions_active = decisions.mode != crate::config::DecisionMode::Off;
    let decisions_apply = decisions.mode == crate::config::DecisionMode::On;
    let decision_model = decisions.model.clone();
    // The pool ranked is `prepare_scout`'s own term-matched walk, never a
    // separate directory walk: `helper_context.rs` does no model work by
    // its own invariant, so the ranking happens here, over what that walk
    // already found, rather than inside it.
    let scout_pool = (decisions_active && decision_model.is_some()).then(|| {
        crate::helper_context::prepare(
            crate::helper_context::HelperRole::Scout,
            task,
            session.profile,
            &token,
        )
    });
    let rank_candidates = scout_pool.as_ref().map(|prepared| {
        crate::helpers::scout_candidates_from_evidence(&prepared.evidence, session.profile, 40)
    });
    let rank = match (&rank_candidates, decision_model.as_deref()) {
        (Some(candidates), Some(decision_model)) => Some((
            candidates.as_slice(),
            crate::helpers::ScoutRankRoute {
                model: decision_model,
                floor: decisions.scout_relevance_below,
                apply: decisions_apply,
            },
        )),
        _ => None,
    };
    let judge = if decisions_active {
        decision_model
            .as_deref()
            .map(|decision_model| crate::helpers::HelperJudge {
                model: decision_model,
                floor: decisions.helper_no_below,
                apply: decisions_apply,
            })
    } else {
        None
    };
    let helper_context = crate::helpers::HelperContext {
        profile: session.profile,
        glasshouse: session.glasshouse,
        session: session.id,
        token: &token,
    };
    let Some(judged) = crate::helpers::preflight_judged(
        &brief,
        crate::helpers::HelperRoute::new(model, effort),
        helper_context,
        rank,
        judge,
        |record| {
            let Some(ui) = session.ui else {
                return;
            };
            // The real user turn is recorded below in the established rollout
            // order. This snapshot makes the submitted request and its Scout
            // visible immediately without adding a synthetic model turn.
            let mut visible = transcript.clone();
            visible
                .conversation
                .messages
                .push(Message::text(Role::User, task));
            visible.notebook.preflight = Some(record.clone());
            ui.publish(&visible, &ServedBy::default(), tui::Activity::Searching);
        },
    ) else {
        return none;
    };
    let record = judged.record;
    let ranking_note = judged
        .ranking
        .as_ref()
        .map(crate::helpers::ScoutRanking::note);
    if let Some(ranking) = &judged.ranking {
        output::helpers_ranking(ranking);
    }
    if let Some((noul, latency_ms)) = judged.judge {
        output::helpers_checked(noul, decisions.helper_no_below, latency_ms);
    }
    if record.outcome.cancelled {
        session.interrupt.consumed();
    }
    output::preflight(&record);
    // Keep the resolved Scout beside this request for every later task-frame.
    // The next task clears it before deciding whether another preflight runs.
    transcript.notebook.preflight = Some(record.clone());
    let block = record.outcome.ok.then(|| {
        let named = crate::preflight::spans(&record.outcome.text);
        let (served, unserved) = preflight_serving(session.profile, &named);
        let served: Vec<(String, String)> = served
            .into_iter()
            .map(|(path, _why, text)| (path, text))
            .collect();
        crate::preflight::render_serving(
            task,
            &record.outcome.text,
            &served,
            &unserved,
            ranking_note.as_deref(),
        )
    });
    PreflightOutcome {
        block,
        scout_signal,
        would_scout,
    }
}

/// A file the preflight serves whole: the path the scout named, why it named
/// it, and the file's complete text.
pub(super) type ServedFile = (String, String, String);

/// A file the scout named that the preflight did not serve, and the reason in
/// the person's words.
pub(super) type UnservedFile = (String, String);

/// The named files that can be served whole -- inside the grant, a regular
/// file, UTF-8, and small enough that *in full* is true of it -- and beside
/// them every named file that was **not** served, with its reason.
///
/// **The profile decides what may be served, not this function.** A scout
/// that named a path outside the grant has it refused here for the same
/// reason `read` would refuse it, and a preflight is not a way around a
/// grant.
///
/// **A file the scout named and the preflight did not serve is stated, never
/// dropped.** Not serving it is right — the section claims *in full*, and a
/// truncated file under that heading is a claim the model cannot check — but
/// silence turns a bounded offer into an invisible one: the model cannot
/// `read` a file it was never told about, so it re-derives what it was almost
/// given. Each reason names the bound so the model can act on it.
pub(super) fn preflight_serving(
    profile: &Profile,
    named: &[(String, String)],
) -> (Vec<ServedFile>, Vec<UnservedFile>) {
    let mut served = Vec::new();
    let mut unserved: Vec<(String, String)> = Vec::new();
    for (path, why) in named {
        if served.len() == PREFLIGHT_SERVE_FILES {
            unserved.push((
                path.clone(),
                format!("not served: this section serves at most {PREFLIGHT_SERVE_FILES} files"),
            ));
            continue;
        }
        let candidate = std::path::Path::new(path);
        let absolute = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            profile.root().join(candidate)
        };
        let Ok(granted) = profile.check(
            "preflight",
            crate::sandbox::profile::Access::Read,
            &absolute,
        ) else {
            unserved.push((
                path.clone(),
                "not served: outside this session's grant".into(),
            ));
            continue;
        };
        let Ok(metadata) = fs::metadata(&granted) else {
            unserved.push((path.clone(), "not served: no such file".into()));
            continue;
        };
        if !metadata.is_file() {
            unserved.push((path.clone(), "not served: not a regular file".into()));
            continue;
        }
        if metadata.len() > PREFLIGHT_SERVE_BYTES {
            unserved.push((
                path.clone(),
                format!(
                    "not served whole: {} bytes, over the {PREFLIGHT_SERVE_BYTES}-byte limit",
                    metadata.len()
                ),
            ));
            continue;
        }
        let Ok(text) = fs::read_to_string(&granted) else {
            unserved.push((path.clone(), "not served: not UTF-8 text".into()));
            continue;
        };
        served.push((path.clone(), why.clone(), text));
    }
    (served, unserved)
}

/// The compiled profile, as the model needs to read it.
///
/// **The invariant: this reports the profile that is actually in force, never
/// the one the configuration asked for.** It is built from `Profile`'s own
/// accessors for that reason — a settings document that failed to parse
/// grants nothing, and a model told otherwise would plan against grants it
/// does not have.
///
/// `pub` so `tests/session.rs`'s byte-equality test can build the same facts
/// the binary did rather than spelling them a second time — the same reason
/// that test calls [`prompt::render_system`] instead of quoting its output.
pub fn session_facts(profile: &Profile) -> prompt::SessionFacts {
    // The roots come from `Profile::writable_roots`, which is the same
    // answer `Profile::check` gives and the same one the manifest renders.
    // Listing only the write-`allow` rules said "nothing is writable" for an
    // ordinary session, on the line above the manifest naming the project
    // root as writable.
    let mut writable: Vec<String> = profile
        .writable_roots()
        .into_iter()
        .map(|root| root.display().to_string())
        .collect();
    writable.extend(
        profile
            .rules()
            .filter(|rule| rule.write() && rule.effect() == crate::sandbox::profile::Effect::Allow)
            .map(|rule| rule.written().to_string()),
    );
    writable.sort();
    writable.dedup();
    prompt::SessionFacts {
        root: profile.root().display().to_string(),
        writable,
        command_patterns: profile.command_pattern_count(),
        // Not `args.yolo`: the flag is a request, the profile is the grant.
        // A mutation that stopped `--yolo` reaching the compiler survived
        // while this read the flag, because the model was still told the
        // grant was open (2026-09-06).
        all_commands: profile.admits_every_command(),
        network: profile.grants_network(),
        interface: crate::abi::Interface::default(),
        manifest: None,
    }
}

/// [`session_facts`] for the interface this session declares and the
/// manifest it collected — the facts the binary actually renders.
pub fn session_facts_with(
    profile: &Profile,
    interface: crate::abi::Interface,
    manifest: &crate::manifest::Manifest,
) -> prompt::SessionFacts {
    let mut facts = session_facts(profile);
    facts.interface = interface;
    facts.manifest = Some(manifest.render());
    facts
}

/// The executables the manifest looks for on `PATH`, so the model knows
/// before acting which of the tools a task usually names are absent.
pub const MANIFEST_PROBE: [&str; 24] = [
    "bash", "sh", "python3", "python", "git", "cargo", "rustc", "gcc", "g++", "clang", "make",
    "cmake", "node", "npm", "rg", "fd", "jq", "gdb", "lldb", "valgrind", "pytest", "go", "java",
    "docker",
];

/// The effective capability and environment manifest for one session
/// (`smarter-cheaper-roadmap.md`, *Capability/environment manifest*): the
/// compiled profile's roots and policies, the probed executables, and the
/// capabilities this configuration cannot provide.
pub fn system_manifest(profile: &Profile, config: &PaneConfig) -> crate::manifest::Manifest {
    let mut manifest = crate::manifest::Manifest::collect(profile, &MANIFEST_PROBE);
    if !config.web.enabled {
        manifest
            .unavailable
            .push("web.fetch and web.search: the host web broker is disabled".into());
    } else if !config.web.search_configured() {
        manifest
            .unavailable
            .push("web.search: no search provider is configured".into());
    }
    if config.helpers.model.is_none() || !config.helpers.enabled {
        manifest
            .unavailable
            .push("helper.*: no helper model is configured".into());
    }
    if matches!(config.agents.mode, crate::config::AgentsMode::Off) {
        manifest
            .unavailable
            .push("agent.run: subagents are off in this configuration".into());
    }
    manifest
}
