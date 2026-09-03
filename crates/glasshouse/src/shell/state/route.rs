use super::*;

/// One observed routing identity for the route-evidence table — Phase 47,
/// map lines 1762 and 1764. Built by `shell::build_route_evidence_table`
/// from `crate::routing::evidence::EvidenceLedger::observed_identities`'s own
/// [`crate::routing::evidence::ObservedIdentity`] — this module holds plain
/// data rather than importing `crate::routing::evidence`'s own types
/// directly, the same split [`KnowledgeSection`] keeps from `crate::memory`.
///
/// **Deliberately three columns, not line 1762's seven.** TTFC, effective
/// TTFC, TTFT, decode throughput and rounds-per-minute have no producer on
/// this gateway at all — see `crate::routing::evidence`'s own module header
/// — and this type has no fields for them, so there is nothing here a future
/// render could accidentally show as a fabricated zero. `context_state` is
/// already the string [`crate::routing::evidence::ContextState::as_str`]
/// produces (`"warm"`, `"cold"`, or `"unknown"`) — never blank, and never
/// upgraded to look like a measurement it is not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteEvidenceRow {
    pub provider: String,
    pub model: String,
    /// `None` means this identity's rows were recorded with no route.
    pub route: Option<String>,
    pub context_state: String,
    pub sample_count: usize,
    pub window_start_unix: i64,
    pub window_end_unix: i64,
}

/// The route-evidence table's own data: every distinct routing identity the
/// run loop already read from the evidence ledger. See
/// [`ShellState::open_route_evidence`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteEvidenceState {
    rows: Vec<RouteEvidenceRow>,
    /// Set when the run loop could not read the evidence ledger at all. The
    /// overlay still opens with an honest, empty table rather than refusing
    /// to show anything — the same contract
    /// [`ProjectOverviewState::memory_note`] and
    /// [`ProjectKnowledgeState::memory_note`] keep.
    note: Option<String>,
}

impl RouteEvidenceState {
    pub fn rows(&self) -> &[RouteEvidenceRow] {
        &self.rows
    }

    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }
}

/// One recorded disposable-routing decision, for the routing-decisions view.
/// Built by `shell::build_route_decision_table` from
/// `crate::evaluation::EvaluationObservations::recent_of_kind` — this module
/// holds plain data rather than importing `crate::evaluation`'s own types
/// directly, the same split [`RouteEvidenceRow`] keeps from
/// `crate::routing::evidence`.
///
/// # Why the rationale is text, and must stay text
///
/// The decision behind one of these rows is a
/// `crate::routing::disposable::DisposableChoice`, whose fields are private
/// and which nothing outside its own module can construct. That is an
/// enforced safety invariant rather than a style choice — its module header
/// records that a choice on a metered resource must not be reproducible from
/// a policy that withheld it — so a stored decision is deliberately **not**
/// turned back into one. The producer renders the rationale at the moment it
/// decides and stores the sentence; this row carries that sentence; the view
/// draws it. Nothing anywhere reconstructs the choice.
///
/// # Every field is what was recorded, and absent stays absent
///
/// `session_id` and `rationale` are `Option` because the ledger's columns are
/// nullable and a row written by a later producer may not fill them. The view
/// says so plainly rather than drawing an empty column that reads as a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteDecisionRow {
    /// When the decision was made, as the ledger recorded it.
    pub observed_at_unix: i64,
    /// The job the decision was for, in
    /// `crate::routing::disposable::JobKind`'s own spelling.
    pub job: String,
    /// The session the decision was made for. `None` means the row recorded
    /// none.
    pub session_id: Option<String>,
    /// The rendered rationale, exactly as it was stored. `None` means the row
    /// recorded none.
    pub rationale: Option<String>,
}

/// The routing-decisions view's own data: the decisions the run loop already
/// read from the evaluation ledger. See
/// [`ShellState::open_route_decisions`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteDecisionsState {
    rows: Vec<RouteDecisionRow>,
    /// Set when the run loop could not read the evaluation ledger at all.
    /// The overlay still opens with an honest, empty list — the same contract
    /// [`RouteEvidenceState::note`] keeps.
    note: Option<String>,
}

impl RouteDecisionsState {
    pub fn rows(&self) -> &[RouteDecisionRow] {
        &self.rows
    }

    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }
}

/// One observed free resource, with map line 1765's five concepts carried as
/// **five separate groups of fields** — Phase 47.
///
/// Built by `shell::build_route_health_table` from
/// `crate::provider::telemetry::GatewayHealthCache` and `GatewayQuotaCache`,
/// the two files a gateway process writes and any later process reads back.
/// This module holds plain data rather than importing those types directly,
/// the same split [`RouteEvidenceRow`] keeps from `crate::routing::evidence`.
///
/// # Why the fields are grouped and not summarised
///
/// Line 1765 asks for route health, immediate availability, cadence, quota
/// reset and failure-domain evidence *"as separate concepts"*. They are five
/// different questions with five different answers and five different ways of
/// being unknown, and collapsing them is not a simplification — it is a lost
/// distinction:
///
/// - a resource can be **healthy** (no failures) and **unavailable** (its
///   credential was refused);
/// - it can be **available now** and still have a **cadence** that will stop
///   it in one more request;
/// - a **cooldown** Glasshouse imposed and a **quota reset** the provider
///   stated are different clocks owned by different parties, and neither is
///   the other's estimate;
/// - **failure-domain evidence** is about a *pair* of resources and says
///   nothing about either one alone.
///
/// `crate::provider::resources::render_health` currently prints health,
/// availability and cadence as one `status` word on one line. That is the
/// shape this row exists not to reproduce.
///
/// # "unknown" is a real answer, and it is `None`
///
/// Three of the five concepts come from provider-stated headers that most
/// providers do not send. A `None` here is *"no response ever stated this"*,
/// never a zero and never a default — the same contract
/// `crate::provider::telemetry::RateLimitHeaders` keeps field by field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteHealthRow {
    /// The provider these observations belong to — also the *only* signal
    /// this build has for `failure_domain` below.
    pub provider: String,
    /// `crate::routing::CredentialId::label` — two names, never a secret.
    pub credential_label: String,
    pub model: String,

    // --- concept 1: route health -------------------------------------
    /// How many failures in a row this resource has had **since its last
    /// success**. A streak, not a total: any success resets it to zero (see
    /// `crate::routing::free::ResourceHealth::observe`), so the view must
    /// never present it as a count of everything that ever went wrong.
    pub consecutive_failures: u32,
    /// The provider refused the credential itself. A different fact from a
    /// failure streak, and it is kept separate because waiting does not fix
    /// it.
    pub credential_rejected: bool,

    // --- concept 2: immediate availability ---------------------------
    /// `crate::provider::telemetry::GatewayHealthReading::is_available`, as
    /// of the moment the run loop built this row. The producer's own
    /// decision, not a verdict this module re-derives from the two fields
    /// above — which would be a second spelling of the same rule.
    pub available_now: bool,

    // --- concept 3: cadence ------------------------------------------
    /// When Glasshouse's own bounded backoff stops pacing this resource.
    /// `None` means it is not pacing it. Pacing is a scheduling fact, never
    /// a verdict on the resource — `render_health`'s own wording, kept.
    pub cooling_down_until_unix: Option<i64>,
    /// The request ceiling the provider stated, if it stated one.
    pub stated_limit: Option<i64>,
    /// How long the stated ceiling's window is, in seconds, if the provider
    /// said. `stated_limit` per `stated_window_seconds` is the provider's own
    /// cadence; either half alone is not.
    pub stated_window_seconds: Option<i64>,

    // --- concept 4: quota reset --------------------------------------
    /// When the provider said the current window resets, as a unix second.
    /// `None` means no response ever carried a reset field — not "it never
    /// resets", and not "now".
    pub quota_resets_at_unix: Option<i64>,

    // --- concept 5: failure-domain evidence --------------------------
    /// `crate::routing::domain::FailureDomain`'s own vocabulary, and never
    /// `"independent"`: that state is one this build cannot earn, because
    /// nothing here does the temporal correlation it would need. Spelled by
    /// `shell::build_route_health_table` from the enum itself so there is
    /// exactly one spelling of these words in the process.
    pub failure_domain: String,
    /// How many *other* observed resources share this one's provider — the
    /// resources this one is known to fail together with. Zero does not mean
    /// isolated; it means nothing has been observed that shares its domain.
    pub failure_domain_peers: usize,
}

/// The route-health view's own data: every resource a local gateway has
/// observed, as the run loop read it. See [`ShellState::open_route_health`].
///
/// No `note` field, deliberately, unlike [`RouteEvidenceState`] — see
/// [`Action::OpenRouteHealth`] for why there is no read failure to report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteHealthState {
    rows: Vec<RouteHealthRow>,
}

impl RouteHealthState {
    pub fn rows(&self) -> &[RouteHealthRow] {
        &self.rows
    }
}

impl ShellState {
    /// Open the route-evidence table with rows the run loop already read
    /// from the evidence ledger. Reading `crate::routing::evidence` is file
    /// I/O this module deliberately does not hold — see
    /// [`Self::open_project_overview`] for the same split.
    ///
    /// Opens even when `note` is `Some`: a project whose evidence ledger
    /// could not be read still gets an honest, empty table rather than no
    /// view at all — see `shell::build_route_evidence_table`'s doc comment
    /// for why both failure paths reach this.
    pub fn open_route_evidence(
        &mut self,
        rows: Vec<RouteEvidenceRow>,
        note: Option<String>,
    ) -> Action {
        self.overlay = Some(Overlay::RouteEvidence);
        self.route_evidence = Some(RouteEvidenceState { rows, note });
        Action::Redraw
    }

    /// The route-evidence table's own data, or `None` when it is not open.
    pub fn route_evidence(&self) -> Option<&RouteEvidenceState> {
        self.route_evidence.as_ref()
    }

    /// Open the route-health view with rows the run loop already read from
    /// the two gateway telemetry caches. Reading
    /// `crate::provider::telemetry` is file I/O this module deliberately does
    /// not hold — see [`Self::open_route_evidence`] for the same split.
    ///
    /// Opens on an empty `rows` too: "no gateway exchange has been observed"
    /// is an honest answer and the one a fresh installation gives, so a view
    /// that refused to open would be hiding the most common true state.
    pub fn open_route_health(&mut self, rows: Vec<RouteHealthRow>) -> Action {
        self.overlay = Some(Overlay::RouteHealth);
        self.route_health = Some(RouteHealthState { rows });
        Action::Redraw
    }

    /// The route-health view's own data, or `None` when it is not open.
    pub fn route_health(&self) -> Option<&RouteHealthState> {
        self.route_health.as_ref()
    }

    /// Open the routing-decisions view with the decisions the run loop
    /// already read from the evaluation ledger. Reading `crate::evaluation`
    /// is file I/O this module deliberately does not hold — see
    /// [`Self::open_route_evidence`] for the same split.
    ///
    /// Opens on an empty `rows`, and opens when `note` is `Some`: a project
    /// that has never completed a turn under Glasshouse has recorded no
    /// decision, and that is the most common true state rather than a failure
    /// — so the view says it plainly instead of refusing to open.
    pub fn open_route_decisions(
        &mut self,
        rows: Vec<RouteDecisionRow>,
        note: Option<String>,
    ) -> Action {
        self.overlay = Some(Overlay::RouteDecisions);
        self.route_decisions = Some(RouteDecisionsState { rows, note });
        Action::Redraw
    }

    /// The routing-decisions view's own data, or `None` when it is not open.
    pub fn route_decisions(&self) -> Option<&RouteDecisionsState> {
        self.route_decisions.as_ref()
    }
}

impl ShellState {
    /// Answer one key while the route-evidence table is open — the same
    /// shape as [`Self::handle_project_overview_key`] and
    /// [`Self::handle_session_events_key`], for the same reason: nothing
    /// here is acted on, only shown.
    pub(super) fn handle_route_evidence_key(&mut self, key: KeyEvent, had_status: bool) -> Action {
        match key.code {
            KeyCode::Esc | KeyCode::Char('r') => self.close_overlay(),
            _ => self.handle_control_key(key, had_status),
        }
    }

    /// Answer one key while the route-health view is open — the same shape as
    /// [`Self::handle_route_evidence_key`], for the same reason: nothing here
    /// is acted on, only shown.
    pub(super) fn handle_route_health_key(&mut self, key: KeyEvent, had_status: bool) -> Action {
        match key.code {
            KeyCode::Esc | KeyCode::Char('h') => self.close_overlay(),
            _ => self.handle_control_key(key, had_status),
        }
    }

    /// Answer one key while the routing-decisions view is open — the same
    /// shape as [`Self::handle_route_evidence_key`], for the same reason:
    /// nothing here is acted on, only shown.
    pub(super) fn handle_route_decisions_key(&mut self, key: KeyEvent, had_status: bool) -> Action {
        match key.code {
            KeyCode::Esc | KeyCode::Char('d') => self.close_overlay(),
            _ => self.handle_control_key(key, had_status),
        }
    }
}
