//! The task's running state: its spend, its capsule and progress guard, the
//! evidence gate and salvage, and the partial-effects sentence (moved out of
//! `session.rs` for the Phase 59 size ratchet, 2026-09-13; nothing here is new).

use super::*;

/// The task's cumulative token spend and executed-cell count, and where each
/// turn's figure came from. Spend is telemetry only; only the configured cell
/// count remains a control limit.
pub(super) struct TaskSpend {
    pub(super) parent_used: u64,
    pub(super) helpers: HelperTokens,
    pub(super) cells_used: u64,
    pub(super) reported: bool,
    pub(super) estimated: bool,
    pub(super) cells_cap: Option<u64>,
    /// The runtime's cumulative reduction ledger as last reported, so each
    /// frame's telemetry carries only what that frame added.
    pub(super) reductions_seen: crate::runtime::observation::ReductionStats,
}

impl TaskSpend {
    pub(super) fn new(cells_cap: Option<u64>) -> Self {
        Self {
            parent_used: 0,
            helpers: HelperTokens::default(),
            cells_used: 0,
            reported: false,
            estimated: false,
            cells_cap,
            reductions_seen: Default::default(),
        }
    }

    /// What the runtime's reduction ledger gained since the last frame.
    pub(super) fn reduction_delta(
        &mut self,
        current: crate::runtime::observation::ReductionStats,
    ) -> crate::runtime::observation::ReductionStats {
        let seen = std::mem::replace(&mut self.reductions_seen, current);
        crate::runtime::observation::ReductionStats {
            attempted: current.attempted.saturating_sub(seen.attempted),
            made: current.made.saturating_sub(seen.made),
            failed: current.failed.saturating_sub(seen.failed),
            cached: current.cached.saturating_sub(seen.cached),
            ruled: current.ruled.saturating_sub(seen.ruled),
            bytes_in: current.bytes_in.saturating_sub(seen.bytes_in),
            bytes_out: current.bytes_out.saturating_sub(seen.bytes_out),
        }
    }

    /// Adds one turn's cost: the gateway's own usage row when it reported
    /// one, else the Messages response's own `usage`, else `estimate`.
    ///
    /// **Which source was used is recorded, not averaged.** §6 reads a
    /// provider's figure "rather than estimated", and a total that quietly
    /// mixed a measurement with a heuristic would be a number the sidebar
    /// could not honestly label. The gateway's row is preferred over the
    /// response's own `usage` when both are present, because it is what
    /// `served_by` was built to make authoritative -- but the sidebar calls
    /// either one `reported`: a reader deciding whether to trust this figure
    /// only needs to know it did not come from `estimate_tokens`.
    pub(super) fn add(&mut self, served: &ServedBy, usage: Option<&wire::Usage>, estimate: u64) {
        match (served.input_tokens, served.output_tokens) {
            (None, None) => match usage {
                Some(usage) => {
                    self.parent_used = self.parent_used.saturating_add(usage.total_tokens());
                    self.reported = true;
                }
                None => {
                    self.parent_used = self.parent_used.saturating_add(estimate);
                    self.estimated = true;
                }
            },
            (input, output) => {
                self.parent_used = self
                    .parent_used
                    .saturating_add(input.unwrap_or(0))
                    .saturating_add(output.unwrap_or(0))
                    .saturating_add(
                        served
                            .cached_input_tokens
                            .or_else(|| usage.and_then(|row| row.cache_read_input_tokens))
                            .unwrap_or(0),
                    )
                    .saturating_add(
                        usage
                            .and_then(|row| row.cache_creation_input_tokens)
                            .unwrap_or(0),
                    );
                self.reported = true;
            }
        }
    }

    /// Add resolved helper records once, at the preflight or cell boundary
    /// that owns them. Rendering and rollout replay never call this method.
    pub(super) fn add_helpers(&mut self, records: &[crate::helpers::HelperRecord]) {
        for record in records {
            let usage = &record.usage;
            self.helpers.calls = self.helpers.calls.saturating_add(1);
            if usage.coverage_known {
                self.helpers.usage_known_calls = self.helpers.usage_known_calls.saturating_add(1);
            }
            self.helpers.used = self.helpers.used.saturating_add(usage.known_tokens());
            self.helpers.input_tokens =
                self.helpers.input_tokens.saturating_add(usage.input_tokens);
            self.helpers.output_tokens = self
                .helpers
                .output_tokens
                .saturating_add(usage.output_tokens);
            self.helpers.requests = self.helpers.requests.saturating_add(usage.requests);
            self.helpers.reported_requests = self
                .helpers
                .reported_requests
                .saturating_add(usage.reported_requests);
            self.helpers.cache_read_input_tokens = self
                .helpers
                .cache_read_input_tokens
                .saturating_add(usage.cache_read_input_tokens);
            self.helpers.cache_creation_input_tokens = self
                .helpers
                .cache_creation_input_tokens
                .saturating_add(usage.cache_creation_input_tokens);
            self.helpers.cache_read_reported_requests = self
                .helpers
                .cache_read_reported_requests
                .saturating_add(usage.cache_read_reported_requests);
            self.helpers.cache_creation_reported_requests = self
                .helpers
                .cache_creation_reported_requests
                .saturating_add(usage.cache_creation_reported_requests);
            let model_index = self
                .helpers
                .models
                .iter()
                .position(|model| model.model == usage.model)
                .unwrap_or_else(|| {
                    self.helpers.models.push(HelperModelTokens {
                        model: usage.model.clone(),
                        ..HelperModelTokens::default()
                    });
                    self.helpers.models.len() - 1
                });
            let model = &mut self.helpers.models[model_index];
            model.calls = model.calls.saturating_add(1);
            if usage.coverage_known {
                model.usage_known_calls = model.usage_known_calls.saturating_add(1);
            }
            model.used = model.used.saturating_add(usage.known_tokens());
            model.input_tokens = model.input_tokens.saturating_add(usage.input_tokens);
            model.output_tokens = model.output_tokens.saturating_add(usage.output_tokens);
            model.requests = model.requests.saturating_add(usage.requests);
            model.reported_requests = model
                .reported_requests
                .saturating_add(usage.reported_requests);
            model.cache_read_input_tokens = model
                .cache_read_input_tokens
                .saturating_add(usage.cache_read_input_tokens);
            model.cache_creation_input_tokens = model
                .cache_creation_input_tokens
                .saturating_add(usage.cache_creation_input_tokens);
            model.cache_read_reported_requests = model
                .cache_read_reported_requests
                .saturating_add(usage.cache_read_reported_requests);
            model.cache_creation_reported_requests = model
                .cache_creation_reported_requests
                .saturating_add(usage.cache_creation_reported_requests);
        }
    }

    pub(super) fn used(&self) -> u64 {
        self.parent_used.saturating_add(self.helpers.used)
    }

    pub(super) fn counted(&self) -> Option<Counted> {
        match (self.reported, self.estimated) {
            (true, true) => Some(Counted::Mixed),
            (true, false) => Some(Counted::Gateway),
            (false, true) => Some(Counted::Estimated),
            (false, false) => None,
        }
    }

    /// §6's own line, for the result block the model reads next.
    ///
    /// **The cap named here is the one the request actually carried.** It
    /// used to be `wire::MAX_TOKENS` whatever the turn asked for, and once
    /// per-model maxima landed the two parted company: a session on a model
    /// publishing 128,000 sent 128,000 and told the model it had 8,192, so
    /// the model would have cut its own work to a sixteenth of the room it
    /// had. `the_usage_line_names_the_max_tokens_actually_sent` pins the
    /// agreement rather than either figure.
    pub(super) fn line(&self, model: &str) -> Budget {
        Budget {
            turn_cap: u64::from(wire::max_tokens_for(model)),
            task_used: self.used(),
            // Kept in the wire-facing value for API compatibility. The
            // renderer deliberately ignores it: task spend has no cap.
            task_cap: 0,
            cells_used: self.cells_used,
            cells_cap: self.cells_cap,
        }
    }

    pub(super) fn tokens(&self) -> Option<TaskTokens> {
        Some(TaskTokens {
            used: self.used(),
            parent_used: self.parent_used,
            helpers: self.helpers.clone(),
            counted: self.counted()?,
        })
    }

    /// The cell limit buys exactly one final-answer turn. Token spend is not
    /// consulted here or anywhere else in the task loop.
    /// Whether a ceiling this person set has been reached. No ceiling is no
    /// limit: `None` never ends a task.
    pub(super) fn cell_limit_reached(&self) -> bool {
        self.cells_cap.is_some_and(|cap| self.cells_used >= cap)
    }
}

/// What one cell's observation added to the next turn's feedback.
pub(super) struct Observed {
    /// Lines every form of the feedback carries — a no-progress notice.
    pub(super) notices: Vec<String>,
    /// The `## Task` block for the live feedback only: state the next
    /// request replaces, never history.
    pub(super) capsule_block: Option<String>,
}

/// Everything the task loop learns across cells that a terminal return is
/// judged against — `smarter-cheaper-roadmap.md`'s *Structured task capsule*,
/// *Evidence-gated completion*, *No-progress guard*, *Verified checkpoint*
/// and *Cut-off salvage* rows.
///
/// The invariant: **every field is derived from the trajectory and the
/// tree, never from the model's narrative.** The capsule's facts cite cells,
/// the checkpoints hold tree digests, and the gate's findings come from the
/// filesystem the task changed.
pub(super) struct TaskState {
    pub(super) task: String,
    pub(super) capsule: crate::runtime::capsule::Capsule,
    pub(super) guard: crate::progress::Guard,
    pub(super) checkpoints: crate::progress::Checkpoints,
    pub(super) files: crate::completion::TaskFiles,
    pub(super) task_start: crate::changes::Snapshot,
    pub(super) last_verification_cell: Option<u64>,
    pub(super) last_mutation_cell: Option<u64>,
    pub(super) tree_digest: Option<String>,
    pub(super) deferred_findings: Option<Vec<String>>,
    pub(super) gate_deferrals: u32,
    pub(super) last_capsule_render: String,
    pub(super) previous_frame: Option<CellRecord>,
    pub(super) previous_failed: bool,
    pub(super) evidence_gate: bool,
    pub(super) completion_check: bool,
    pub(super) checker_ran: bool,
    /// The request-derived acceptance list (`acceptance.rs`), empty when no
    /// lister ran; its latest evaluation is what the result reports.
    pub(super) acceptance: Vec<crate::acceptance::Item>,
    pub(super) acceptance_verdicts: Vec<crate::acceptance::Verdict>,
    /// Cells in a row that changed nothing (`progress::Stall`).
    pub(super) stall: crate::progress::Stall,
    /// The decision model's answer to this task's one intent question, asked
    /// once before the first turn -- `None` when no model is configured, the
    /// mode is off, or the request failed (`decide-model.md`).
    pub(super) intent: Option<crate::decide::Intent>,
    /// The decision model's answer to the complexity question asked in the
    /// same request as [`Self::intent`] -- `None` under the same conditions
    /// (F2, map 2614/2615's paragraph).
    pub(super) complexity: Option<crate::decide::Complexity>,
    /// Whether `preflight::SIGNAL_DECIDED_EXPLORATION` was one of this task's
    /// `Decision::Run` signals -- only possible with `mode = on`.
    pub(super) scout_signal: bool,
    /// Whether `mode = shadow` recorded that the complexity answer would have
    /// added that signal, had `mode` been `on`. Never set alongside
    /// [`Self::scout_signal`]: exactly one mode is in force per task.
    pub(super) would_scout: bool,
    /// How many decision requests this task attempted and did not answer --
    /// the intent question (at most one, before the first turn) and the
    /// completion question (once per distinct diff claimed at the gate;
    /// 2616).
    pub(super) decision_failures: u32,
    /// Effectful cells or frames held this task (`mode = on`); the once rule
    /// is `effect_holds == 0`.
    pub(super) effect_holds: u32,
    /// Effectful cells or frames that ran after an earlier hold this task.
    pub(super) effect_overrides: u32,
    /// Effectful cells or frames `mode = shadow` would have held.
    pub(super) would_hold: u32,
    /// Effectful cells or frames held by the drift question this task
    /// (2643, `mode = on`); the once rule is `drift_holds == 0`.
    pub(super) drift_holds: u32,
    /// Effectful cells or frames `mode = shadow` would have held by drift.
    pub(super) would_drift: u32,
    /// Drift questions this task actually asked, including the re-ask of the
    /// cell issued again after a hold -- [`crate::decide::drift_for`]'s own
    /// once rule is what stops that second answer from holding again.
    pub(super) drift_asked: u32,
    /// Drift questions this task attempted and did not answer.
    pub(super) drift_failed: u32,
    /// The completion question's raw answer, keyed by the exact diff text it
    /// was asked about (2616): `None` when no model is configured, mode is
    /// off, or no request has answered yet. Re-asked whenever the diff at a
    /// later `gate` call differs from the cached key -- an identical second
    /// claim (nothing ran between the hold and the re-claim) reuses the
    /// answer and asks nothing; a changed diff (the model fixed something
    /// and claimed again) is a new question, never the stale answer to a
    /// tree that no longer exists.
    pub(super) completion_answer: Option<(String, crate::decide::CompletionAnswer)>,
    /// What the last `gate` call did with [`Self::completion_answer`] --
    /// `None` until the completion question has been asked.
    pub(super) completion_decision: Option<CompletionTelemetry>,
    /// This task's approval hints (F4, decision-model.md) that answered --
    /// synced from `approval::Gate::hint_counts` each cell, since the hint
    /// itself answers on a thread the approval seam owns, not this struct.
    pub(super) approval_hints: u32,
    /// This task's approval-hint requests that failed or timed out.
    pub(super) approval_hint_failures: u32,
    /// The mode proposal's own telemetry (2639, `mode_proposal.rs`): a
    /// `read_only` intent clearing `mode_above` in `execute`, unpinned. All
    /// four default false -- no model, `mode = off`, a non-`execute`
    /// session or a pin all leave this task's proposal unmade, the same
    /// fail-open shape [`Self::intent`] has.
    pub(super) mode_proposed: bool,
    pub(super) mode_applied: bool,
    pub(super) mode_would_apply: bool,
    pub(super) mode_pinned: bool,
}

/// One `gate` call's completion-question telemetry (2616): the cached wire
/// answer plus what that call did with it. Rebuilt on every call from
/// [`TaskState::completion_answer`] and that call's own other findings, so a
/// held candidate's second, identical claim reports the same outcome.
#[derive(Debug, Clone)]
pub(super) struct CompletionTelemetry {
    pub(super) noul: f64,
    pub(super) latency_ms: u64,
    pub(super) truncated: bool,
    pub(super) finding_added: bool,
    pub(super) checker_skipped: Option<String>,
    /// Which state the satisfaction question was asked over (2641/2642's
    /// addendum to 2616): `"diff"` or `"answer"`.
    pub(super) state: &'static str,
    /// The five diff-hygiene nouls (2641), `None` when `state` is `"answer"`.
    pub(super) hygiene: Option<serde_json::Value>,
    /// How many of the five hygiene questions were decisive and added a
    /// finding (2641).
    pub(super) hygiene_findings: u32,
    /// How many acceptance `judge` items this call's answer settled, `{yes,
    /// no, undecided}` (2642).
    pub(super) judged: serde_json::Value,
}

impl TaskState {
    pub(super) fn new(task: &str, profile: &Profile, config: &PaneConfig) -> Self {
        Self {
            task: task.to_string(),
            capsule: crate::runtime::capsule::Capsule::new(task),
            guard: crate::progress::Guard::new(crate::progress::DEFAULT_THRESHOLD),
            checkpoints: crate::progress::Checkpoints::default(),
            files: crate::completion::TaskFiles::default(),
            task_start: crate::changes::Snapshot::capture(profile),
            last_verification_cell: None,
            last_mutation_cell: None,
            tree_digest: None,
            deferred_findings: None,
            gate_deferrals: 0,
            last_capsule_render: String::new(),
            previous_frame: None,
            previous_failed: false,
            evidence_gate: config.limits.evidence_gate,
            completion_check: config.helpers.completion_check,
            checker_ran: false,
            acceptance: Vec::new(),
            acceptance_verdicts: Vec::new(),
            stall: crate::progress::Stall::default(),
            intent: None,
            complexity: None,
            scout_signal: false,
            would_scout: false,
            decision_failures: 0,
            effect_holds: 0,
            effect_overrides: 0,
            would_hold: 0,
            drift_holds: 0,
            would_drift: 0,
            drift_asked: 0,
            drift_failed: 0,
            completion_answer: None,
            completion_decision: None,
            approval_hints: 0,
            approval_hint_failures: 0,
            mode_proposed: false,
            mode_applied: false,
            mode_would_apply: false,
            mode_pinned: false,
        }
    }

    /// The request-derived acceptance list this task is checked against.
    pub(super) fn with_acceptance(mut self, items: Vec<crate::acceptance::Item>) -> Self {
        self.acceptance = items;
        self
    }

    /// The decision model's answer to this task's one request -- both the
    /// intent and complexity questions, asked before `TaskState` existed.
    /// `None` and `decision_failures` is `1` when the request was attempted
    /// and did not answer. `scout_signal` and `would_scout` come from
    /// `preflight_block`'s own use of [`Self::complexity`], computed before
    /// `TaskState` existed for the same reason.
    pub(super) fn with_decision(
        mut self,
        decision: Option<crate::decide::TaskDecision>,
        decision_failures: u32,
        scout_signal: bool,
        would_scout: bool,
    ) -> Self {
        self.intent = decision.as_ref().map(|decision| decision.intent.clone());
        self.complexity = decision.map(|decision| decision.complexity);
        self.decision_failures = decision_failures;
        self.scout_signal = scout_signal;
        self.would_scout = would_scout;
        self
    }

    /// This task's mode proposal (2639), computed in `run_task_inner`
    /// alongside [`Self::with_decision`] and before the profile is narrowed.
    pub(super) fn with_mode_proposal(mut self, proposal: super::mode_proposal::Proposal) -> Self {
        self.mode_proposed = proposal.proposed;
        self.mode_applied = proposal.applied;
        self.mode_would_apply = proposal.would_apply;
        self.mode_pinned = proposal.pinned;
        self
    }

    /// The `Telemetry.decisions` block: `docs/product/pane/decision-model.md`'s
    /// schema, `None` only when no decision model is configured at all.
    pub(super) fn decisions_telemetry(
        &self,
        config: &crate::config::DecisionsConfig,
    ) -> Option<serde_json::Value> {
        let model = config.model.as_deref()?;
        let asked = self.intent.is_some() || self.decision_failures > 0;
        let intent = self.intent.as_ref().map(|intent| {
            serde_json::json!({ "choice": intent.choice, "confidence": intent.confidence })
        });
        let complexity = self.complexity.as_ref().map(|complexity| {
            serde_json::json!({ "choice": complexity.choice, "confidence": complexity.confidence })
        });
        Some(serde_json::json!({
            "model": model,
            "mode": config.mode.as_str(),
            "asked": u32::from(asked),
            "answered": u32::from(self.intent.is_some()),
            "failed": self.decision_failures,
            "latency_ms_total": self.intent.as_ref().map_or(0, |intent| intent.latency_ms),
            "intent": intent,
            "complexity": complexity,
            "scout_signal": self.scout_signal,
            "would_scout": self.would_scout,
            "would_hold": self.would_hold,
            "holds": self.effect_holds,
            "overrides": self.effect_overrides,
            "drift": {
                "asked": self.drift_asked,
                "held": self.drift_holds,
                "would_hold": self.would_drift,
                "failed": self.drift_failed,
            },
            "approval_hints": self.approval_hints,
            "approval_hint_failures": self.approval_hint_failures,
            "mode_proposal": {
                "proposed": self.mode_proposed,
                "applied": self.mode_applied,
                "would_apply": self.mode_would_apply,
                "pinned": self.mode_pinned,
            },
            "completion": self.completion_decision.as_ref().map(|decision| serde_json::json!({
                "noul": decision.noul,
                "latency_ms": decision.latency_ms,
                "truncated": decision.truncated,
                "finding_added": decision.finding_added,
                "checker_skipped": decision.checker_skipped,
                "state": decision.state,
                "hygiene": decision.hygiene,
                "hygiene_findings": decision.hygiene_findings,
                "judged": decision.judged,
            })),
        }))
    }

    /// The `/cell` inspector's one line for this task, `None` only when no
    /// decision model is configured at all.
    pub(super) fn decision_line(&self, config: &crate::config::DecisionsConfig) -> Option<String> {
        config.model.as_ref()?;
        Some(crate::decide::summary_line(
            self.intent.as_ref(),
            self.effect_holds,
            self.effect_overrides,
        ))
    }

    /// Why the next parent request is being made, read from the last frame.
    pub(super) fn next_cause(&self) -> crate::abi::telemetry::RequestCause {
        crate::abi::telemetry::request_cause(self.previous_frame.as_ref(), self.previous_failed)
    }

    /// Folds one finished cell into the capsule, the checkpoints and the
    /// no-progress guard.
    pub(super) fn observe(
        &mut self,
        record: &CellRecord,
        error: Option<(&str, &str)>,
        plan: &[crate::runtime::outcome::PlanItem],
        snapshots: Option<(&crate::changes::Snapshot, &crate::changes::Snapshot)>,
    ) -> Observed {
        let mut progressed = false;
        if let Some((before, after)) = snapshots {
            let changed = before.changed_paths(after);
            if !changed.is_empty() {
                progressed = true;
                self.files.observe(&changed);
                self.last_mutation_cell = Some(record.cell);
                let digest = after.digest();
                self.checkpoints.note_mutation(record.cell, &digest);
                self.tree_digest = Some(digest);
            }
        }
        self.capsule
            .observe_cell(record, error, plan, self.tree_digest.as_deref());
        if self
            .capsule
            .facts()
            .last()
            .is_some_and(|fact| fact.evidence.cell == record.cell)
        {
            progressed = true;
        }
        if let Some(verification) = self.capsule.last_verification()
            && verification.cell == record.cell
        {
            progressed = true;
            self.last_verification_cell = Some(verification.cell);
            self.checkpoints.note_verification(
                verification.cell,
                self.tree_digest.as_deref().unwrap_or(""),
                verification.exit_code == Some(0),
            );
        }
        let mut notices = Vec::new();
        // Only a failing frame can repeat without progress: a denied or
        // thrown call, or a thrown cell. A successful frame ends the streak.
        let failed = error.is_some()
            || record
                .calls
                .iter()
                .any(|call| !matches!(call.ended, Ended::Ok));
        if failed {
            if let Some(notice) = self.guard.observe(crate::progress::fingerprint(
                record,
                error,
                self.tree_digest.as_deref(),
            )) {
                output::no_progress_notice();
                notices.push(notice);
            }
        } else {
            self.guard.reset();
        }
        // A stall is a run of cells that changed nothing: no tree change, no
        // new fact, no verification. It is a notice, never a stop.
        if let Some(notice) = self.stall.observe(progressed) {
            output::stall_notice();
            notices.push(notice);
        }
        let rendered = self.capsule.render();
        let capsule_block = (rendered != self.last_capsule_render).then(|| {
            self.last_capsule_render = rendered.clone();
            rendered
        });
        self.previous_frame = Some(record.clone());
        Observed {
            notices,
            capsule_block,
        }
    }

    /// The evidence gate on a terminal candidate: the deterministic
    /// final-state contract, then once per task the fresh independent
    /// checker. Findings hold the return once; the same findings a second
    /// time let the model finish with the completion recorded unverified.
    pub(super) fn gate(
        &mut self,
        candidate: &str,
        cell: u64,
        before: &crate::changes::Snapshot,
        after: &crate::changes::Snapshot,
        session: &Session<'_>,
    ) -> (Option<String>, Option<crate::helpers::HelperRecord>) {
        if !self.evidence_gate {
            output::completion(true, true, &[], 0);
            return (None, None);
        }
        let mut files = self.files.clone();
        let changed = before.changed_paths(after);
        files.observe(&changed);
        let last_mutation = if changed.is_empty() {
            self.last_mutation_cell
        } else {
            Some(cell)
        };
        let root = session.profile.root();
        let contract = match crate::completion::load_contract(root) {
            Ok(contract) => contract,
            Err(error) => {
                session_println!("completion: {error}");
                crate::completion::Contract::default()
            }
        };
        let findings = crate::completion::check(
            &contract,
            root,
            &files,
            self.last_verification_cell,
            last_mutation,
        );
        let mut findings = findings;
        let has_acceptance = !self.acceptance.is_empty();
        if has_acceptance {
            // Every item is decided against the tree or a command run
            // through the one kernel under the session's own profile.
            let ctx = ToolContext {
                profile: session.profile,
                glasshouse: session.glasshouse,
                session: session.id,
            };
            let mut runner = |command: &str| -> Result<(Option<i32>, String), String> {
                match invoke::run(&ctx, "bash", &Args::new().with("command", command)) {
                    Ok(result) => Ok((
                        result.exit_code,
                        format!("{}{}", result.stdout, result.stderr),
                    )),
                    Err(error) => Err(error.to_string()),
                }
            };
            self.acceptance_verdicts =
                crate::acceptance::evaluate(&self.acceptance, root, &mut runner);
            findings.extend(crate::acceptance::findings(&self.acceptance_verdicts));
        }

        // The completion question (2616, extended 2641/2642): does the diff
        // or the answer satisfy the request, does the diff show the
        // hygiene a reviewer would check, and does the decision model
        // settle any acceptance item this package cannot decide
        // mechanically -- `completion_answer` caches the wire answer keyed
        // by the exact diff or answer text it was asked about, so an
        // identical second claim (the hold-once case) asks nothing, but a
        // diff or answer that changed since the cached answer is a new
        // question, never the stale answer to a tree that no longer
        // exists. Only in `mode != off` with a model configured; `mode =
        // off` or no model stays byte-identical to before this question
        // existed.
        let diff = self.task_start.diff(after);
        let decisions_config = session.config().decisions.clone();
        if let Some(model) = decisions_config.model.clone()
            && decisions_config.mode != crate::config::DecisionMode::Off
        {
            // An empty diff (nothing changed) or a read-only intent means
            // there is no diff worth asking about -- the empty-diff defect
            // Phase 66's shadow calibration measured, a diff-shaped question
            // against a task that never touched a file.
            let is_read_only_intent = self
                .intent
                .as_ref()
                .is_some_and(|intent| intent.choice == crate::decide::READ_ONLY);
            let diff_is_empty = diff.as_ref().is_none_or(|diff| diff.is_empty());
            let use_answer_state = diff_is_empty || is_read_only_intent;
            let cache_key = if use_answer_state {
                candidate.to_string()
            } else {
                diff.clone().unwrap_or_default()
            };
            // Every acceptance item still `Status::Judge` after the
            // mechanical pass above -- always the full judge list, since
            // `evaluate` never remembers a previous decision.
            let judge_indices: Vec<usize> = self
                .acceptance_verdicts
                .iter()
                .enumerate()
                .filter(|(_, verdict)| verdict.status == crate::acceptance::Status::Judge)
                .map(|(index, _)| index)
                .collect();
            let judge_items: Vec<(String, String)> = judge_indices
                .iter()
                .map(|&index| {
                    let verdict = &self.acceptance_verdicts[index];
                    let text = match &verdict.item {
                        crate::acceptance::Item::Judge { text } => text.clone(),
                        _ => unreachable!("Status::Judge only ever applies to Item::Judge"),
                    };
                    (text, verdict.evidence.clone())
                })
                .collect();
            let stale = self
                .completion_answer
                .as_ref()
                .is_none_or(|(asked_about, _)| asked_about != &cache_key);
            if stale {
                let finding_sentences: Vec<String> = findings
                    .iter()
                    .map(|finding| finding.sentence.clone())
                    .collect();
                let state = if use_answer_state {
                    crate::decide::CompletionState::Answer { answer: candidate }
                } else {
                    crate::decide::CompletionState::Diff {
                        diff: &cache_key,
                        findings: &finding_sentences,
                    }
                };
                match crate::decide::completion_satisfied(&model, &self.task, state, &judge_items) {
                    Ok(answer) => self.completion_answer = Some((cache_key, answer)),
                    Err(_) => self.decision_failures += 1,
                }
            }
            if let Some((_, answer)) = self.completion_answer.clone() {
                let decisions_on = decisions_config.mode == crate::config::DecisionMode::On;
                let mut finding_added = false;
                if answer.noul <= decisions_config.completion_no_below && decisions_on {
                    findings.push(crate::completion::Finding {
                        kind: crate::completion::FindingKind::RequestNotSatisfied,
                        path: None,
                        sentence: format!(
                            "the decision model reads {} as not satisfying the request ({:.2})",
                            if use_answer_state {
                                "the answer"
                            } else {
                                "the diff"
                            },
                            answer.noul
                        ),
                    });
                    finding_added = true;
                }

                let mut hygiene_findings = 0u32;
                if let Some(hygiene) = &answer.hygiene {
                    let checks: [(bool, &str); 5] = [
                        (
                            hygiene.has_tests <= decisions_config.hygiene_no_below,
                            "no test for the changed behaviour — add one or say why.",
                        ),
                        (
                            hygiene.out_of_scope >= decisions_config.hygiene_yes_above,
                            "the diff changes files the request did not ask about — narrow it or say why.",
                        ),
                        (
                            hygiene.debug_leftovers >= decisions_config.hygiene_yes_above,
                            "the diff leaves debugging artefacts — remove the prints, commented-out code or TODOs.",
                        ),
                        (
                            hygiene.deletes_tests >= decisions_config.hygiene_yes_above,
                            "the diff deletes or disables tests — restore them or say why.",
                        ),
                        (
                            hygiene.changes_signature >= decisions_config.hygiene_yes_above,
                            "the diff changes a public function or type signature — confirm it is intended.",
                        ),
                    ];
                    for (decisive, sentence) in checks {
                        if decisive && decisions_on {
                            hygiene_findings += 1;
                            findings.push(crate::completion::Finding {
                                kind: crate::completion::FindingKind::HygieneIssue,
                                path: None,
                                sentence: sentence.to_string(),
                            });
                        }
                    }
                }

                let mut judged_yes = 0u32;
                let mut judged_no = 0u32;
                let mut judged_undecided = 0u32;
                for (&index, &noul) in judge_indices.iter().zip(answer.judge.iter()) {
                    if noul >= decisions_config.judge_yes_above {
                        judged_yes += 1;
                        if decisions_on {
                            self.acceptance_verdicts[index].status = crate::acceptance::Status::Met;
                            self.acceptance_verdicts[index].evidence = format!(
                                "the decision model reads this item as satisfied ({noul:.2})"
                            );
                        }
                    } else if noul <= decisions_config.judge_no_below {
                        judged_no += 1;
                        if decisions_on {
                            let text = match &self.acceptance_verdicts[index].item {
                                crate::acceptance::Item::Judge { text } => text.clone(),
                                _ => String::new(),
                            };
                            findings.push(crate::completion::Finding {
                                kind: crate::completion::FindingKind::JudgeNotSatisfied,
                                path: None,
                                sentence: format!(
                                    "the decision model reads the acceptance item as not satisfied ({noul:.2}): {text}"
                                ),
                            });
                        }
                    } else {
                        judged_undecided += 1;
                    }
                }

                let checker_skipped = if decisions_on
                    && answer.noul >= decisions_config.completion_yes_above
                    && findings.is_empty()
                    && judged_undecided == 0
                    && self.completion_check
                    && !self.checker_ran
                {
                    self.checker_ran = true;
                    Some(format!("decision {:.2}", answer.noul))
                } else {
                    None
                };

                self.completion_decision = Some(CompletionTelemetry {
                    noul: answer.noul,
                    latency_ms: answer.latency_ms,
                    truncated: answer.truncated,
                    finding_added,
                    checker_skipped,
                    state: if use_answer_state { "answer" } else { "diff" },
                    hygiene: answer.hygiene.map(|hygiene| {
                        serde_json::json!({
                            "has_tests": hygiene.has_tests,
                            "out_of_scope": hygiene.out_of_scope,
                            "debug_leftovers": hygiene.debug_leftovers,
                            "deletes_tests": hygiene.deletes_tests,
                            "changes_signature": hygiene.changes_signature,
                        })
                    }),
                    hygiene_findings,
                    judged: serde_json::json!({
                        "yes": judged_yes,
                        "no": judged_no,
                        "undecided": judged_undecided,
                    }),
                });
            }
            output::decisions(self.decisions_telemetry(&decisions_config));
        }
        if has_acceptance {
            output::acceptance(crate::acceptance::summary(
                &self.acceptance,
                &self.acceptance_verdicts,
            ));
        }

        let mut sentences: Vec<String> = findings
            .iter()
            .map(|finding| finding.sentence.clone())
            .collect();
        let mut checker = None;
        if self.completion_check && !self.checker_ran {
            let helpers = session.config().helpers.clone();
            if helpers.enabled
                && let Some(model) = helpers.model.as_deref()
                && let Some(effort) = helpers.effort.for_helper("check")
            {
                self.checker_ran = true;
                let diff = diff
                    .clone()
                    .unwrap_or_else(|| "(no observed changes)".to_string());
                let evidence = crate::completion::fresh_checker_evidence(
                    &self.task,
                    &diff,
                    &self.capsule.fact_lines(),
                    &findings,
                    &crate::acceptance::judge_texts(&self.acceptance),
                );
                // The checker is judged with the same one `noul` as the
                // preflight Scout's own result (2645), gated the same way
                // that judge is: a decision model configured and `mode` not
                // `off`. `apply` carries `mode = on` versus `shadow`.
                let decisions_active = decisions_config.model.is_some()
                    && decisions_config.mode != crate::config::DecisionMode::Off;
                let checker_judge = decisions_active.then(|| crate::helpers::HelperJudge {
                    model: decisions_config
                        .model
                        .as_deref()
                        .expect("decisions_active checked model.is_some()"),
                    floor: decisions_config.helper_no_below,
                    apply: decisions_config.mode == crate::config::DecisionMode::On,
                });
                let token = invoke::CancellationToken::new();
                let helper_context = crate::helpers::HelperContext {
                    profile: session.profile,
                    glasshouse: session.glasshouse,
                    session: session.id,
                    token: &token,
                };
                if let Some((record, judged)) = crate::helpers::check_completion_judged(
                    &evidence,
                    crate::helpers::HelperRoute::new(model, effort),
                    helper_context,
                    checker_judge,
                ) {
                    if let Some((noul, latency_ms)) = judged {
                        output::helpers_checked(noul, decisions_config.helper_no_below, latency_ms);
                    }
                    let verdict = record
                        .outcome
                        .text
                        .lines()
                        .next()
                        .unwrap_or("")
                        .trim()
                        .to_ascii_lowercase();
                    if record.outcome.ok && verdict.starts_with("does not hold") {
                        sentences.push(format!(
                            "Independent checker: {}",
                            crate::helper_context::bounded_string(&record.outcome.text, 600)
                        ));
                    }
                    checker = Some(record);
                }
            }
        }
        if sentences.is_empty() {
            output::completion(true, true, &[], self.gate_deferrals);
            return (None, checker);
        }
        if self.deferred_findings.as_ref() == Some(&sentences) {
            output::completion(true, false, &sentences, self.gate_deferrals);
            return (None, checker);
        }
        self.deferred_findings = Some(sentences.clone());
        self.gate_deferrals += 1;
        let listed: Vec<String> = sentences.iter().map(|s| format!("- {s}")).collect();
        let text = format!(
            "## Candidate completion (deferred)\n{candidate}\n\n## Final-state findings\n{}\n\n\
             Resolve each finding, or return the same final answer again to finish with the \
             completion recorded as unverified.\n\n{}",
            listed.join("\n"),
            self.capsule.render()
        );
        (Some(text), checker)
    }

    /// A prose answer is a completion claim, and the evidence gate covers it
    /// exactly as it covers a terminal `return`: the same candidate rule,
    /// held once, then recorded unverified. Measured 2026-09-13 (the first
    /// hybrid Terminal-Bench trial): gpt-5.6-sol ends a task with a
    /// structured return and then prose, so a gate on returns alone gated
    /// nothing in the field.
    pub(super) fn prose_completion(
        &mut self,
        candidate: &str,
        profile: &Profile,
        session: &Session<'_>,
    ) -> Step {
        let now = crate::changes::Snapshot::capture(profile);
        let cell = self.previous_frame.as_ref().map_or(0, |frame| frame.cell);
        let (gate, checker) = self.gate(candidate, cell, &now, &now, session);
        let helpers: Vec<_> = checker.into_iter().collect();
        if let Some(gate) = gate {
            return Step {
                answer: Some(gate.clone()),
                historical: Some(gate.clone()),
                native_result: None,
                response: None,
                prose: false,
                record: None,
                rollback: None,
                view: CellView {
                    output: Some(gate),
                    helpers,
                    ..CellView::default()
                },
            };
        }
        Step {
            answer: None,
            historical: None,
            native_result: None,
            response: None,
            prose: false,
            record: None,
            rollback: None,
            view: CellView {
                helpers,
                ..CellView::default()
            },
        }
    }

    /// A task that ended without completing keeps its established facts and
    /// its unfinished work in the capsule, without claiming completion.
    pub(super) fn salvage(&mut self, reason: &str) {
        self.capsule.salvage(reason);
        output::capsule(self.capsule.to_json());
    }
}

/// `model-contract.md` §5 applied to one assistant message: one `pane` block
/// blocks form one validated program; only explicit completion ends prose.
///
/// **Nothing in `assistant_text` reaches a shell.** The one thing extracted
/// from it is a program, and the only thing that ever receives a program is
/// [`Runtime::run_cell`]; every tool that program calls goes through
/// `tools::invoke` and the session's sandbox from inside the isolate.
/// The effectful calls a thrown cell completed before it threw, one line
/// each, or `None` when nothing with an effect ran.
///
/// The invariant: **only calls the trajectory records as `Ok` on a tool
/// that changes the world appear here.** A read that completed is not an
/// effect the model must know survived; an `edit` is.
pub(super) fn partial_effects(record: &CellRecord, threw: bool) -> Option<String> {
    if !threw {
        return None;
    }
    let lines: Vec<String> = record
        .calls
        .iter()
        .filter(|call| matches!(call.ended, Ended::Ok))
        .filter_map(|call| {
            let head = |text: &str| -> String {
                let mut head: String = text.chars().take(80).collect();
                if text.chars().count() > 80 {
                    head.push('…');
                }
                head
            };
            match call.tool.as_str() {
                "edit" | "write" => call
                    .args
                    .get("path")
                    .map(|path| format!("{} {}", call.tool, head(path))),
                "bash" => call
                    .args
                    .get("command")
                    .map(|command| format!("bash `{}`", head(command))),
                "checks.run" => call
                    .args
                    .get("name")
                    .map(|name| format!("checks.run {name}")),
                _ => None,
            }
        })
        .collect();
    if lines.is_empty() {
        return None;
    }
    Some(format!(
        "Completed before the throw, and their effects persist: {}. Calls after the \
         throw did not run.",
        lines.join("; ")
    ))
}
