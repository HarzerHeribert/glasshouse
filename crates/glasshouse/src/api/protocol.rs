//! The wire shape of the control API — Phase 42.
//!
//! One connection carries exactly one [`Request`] and exactly one
//! [`Response`], each a single line of JSON. A protocol this small has no
//! framing to get wrong: a caller writes one line, reads one line, and closes
//! the connection. Nothing here is transport-specific — [`super::unix`] is
//! the module that knows this travels over a Unix domain socket, and
//! `super::mcp` is the one that knows the same requests arrive as MCP tool
//! calls over stdio. Both answer through the same handlers; neither adds a
//! verb this file does not name.

use glasshouse::events::MessageOrigin;
use glasshouse::guardrails::{
    AssumptionState, ChangeFactors, EvidenceSource, GuardrailOverride, GuardrailResponse, Origin,
    PromotionKind, Uncertainty,
};
use glasshouse::memory::snapshot::SnapshotBudget;
use serde::{Deserialize, Serialize};

/// `pub(super)` so `super::mcp`'s search tool defaults exactly as a bare
/// [`Request::QueryMemory`] does, rather than carrying a copy of this number
/// that could drift from the door it is talking to.
pub(super) fn default_memory_limit() -> usize {
    20
}

fn default_events_limit() -> usize {
    200
}

/// How many assumptions [`Request::ListAssumptions`] returns when the caller
/// does not say. `pub(super)` for `super::mcp`'s listing tool, for
/// [`default_memory_limit`]'s reason.
pub(super) fn default_assumptions_limit() -> usize {
    50
}

/// Matches `cli.rs`'s own `default_value` for `glasshouse route --moment`, so
/// a bare [`Request::RecommendRoute`] and a bare `glasshouse route` ask the
/// router the same question — the agreement capability map line 1681 is only
/// worth anything if it holds by default rather than only when a caller
/// remembers to state the moment.
fn default_routing_moment() -> String {
    "session-start".to_owned()
}

/// How many ranked alternatives [`Request::RecommendRoute`] returns when the
/// caller does not say. Small on purpose: the answer is the destination and
/// the reasons behind it, and the near misses are context. A caller that
/// wants the whole ranked field asks for more, up to
/// `unix::MAX_ROUTE_ALTERNATIVES`.
fn default_route_alternatives() -> usize {
    5
}

/// How much of a session's terminal output [`Request::RecentOutput`] returns
/// when the caller does not say.
///
/// Several screenfuls: enough to answer "what is this worker doing right
/// now" without a caller having to know how much to ask for, and small
/// against the ceiling `unix::MAX_RECENT_OUTPUT_BYTES` allows a caller that
/// wants more. Stated once, here, because `cli::ApiCommand::Read`'s
/// `--max-bytes` is deliberately optional rather than carrying a copy of
/// this number that could drift from the door it is talking to. `pub(super)`
/// for the same reason, for `super::mcp`'s recent-output tool.
pub(super) fn default_recent_output_bytes() -> usize {
    8192
}

/// Read from [`SnapshotBudget::default`] rather than restated, so the door's
/// default snapshot is the same one every other caller of
/// `memory::snapshot::snapshot` gets and the two cannot drift apart.
fn default_snapshot_limit() -> usize {
    SnapshotBudget::default().per_section_limit
}

/// As [`default_snapshot_limit`], for the per-entry body cap.
fn default_snapshot_body_chars() -> usize {
    SnapshotBudget::default().max_body_chars
}

/// Who is speaking, on the two requests that write into a session.
///
/// The door cannot work this out for itself and must not guess. A connection
/// carries no clue about what is on the other end of it: `unix::authorize`
/// admits a peer whose uid equals this process's, so a person at a terminal
/// and an orchestrator acting for them are the **same principal** by
/// construction, arriving over the same socket with the same credentials.
/// Which of the two it is, is a fact only the caller holds — so the caller
/// states it, and this is the field it states it in.
///
/// # Machine by default, and that is a compatibility requirement
///
/// An absent origin means [`Self::Machine`]. Every caller written before this
/// field existed keeps exactly the meaning it had — an orchestrator speaking
/// the protocol straight into the door was recorded as a machine and still
/// is — and the delivery Glasshouse makes to an orchestrator on its own
/// initiative (`unix::pump_watches`) is unchanged for the same reason. New
/// vocabulary on a wire format only earns its place if silence keeps meaning
/// what it meant.
///
/// # This is an attribution boundary, not a security one
///
/// A caller that states an origin it is not is **out of scope**, deliberately
/// and without a defence, and no part of this type should be read as a claim
/// about who a peer is. There is nothing here to authenticate: anything that
/// can reach this socket can already send any bytes it likes under any origin
/// it likes, and it is the *same user* on both sides. What the field buys is
/// that the honest callers stop being indistinguishable — `api::client`,
/// which knows it is a person's command line, and `unix::pump_watches`, which
/// knows it is Glasshouse's own delivery, no longer write log rows that are
/// equal field for field.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestOrigin {
    /// A person, at a keyboard. What `glasshouse api send` and `glasshouse
    /// api interrupt` say about themselves, because a command line a person
    /// typed is the one case on this door where that is knowable.
    User,
    /// Glasshouse, or an orchestrator through it.
    #[default]
    Machine,
}

impl RequestOrigin {
    /// The event-log vocabulary this becomes once it is through the door.
    ///
    /// Two spellings rather than one on purpose. [`MessageOrigin`] is what
    /// the log records and `glasshouse::events` owns it; this is what a wire
    /// format promises to keep accepting. Collapsing them would make every
    /// future rename of an event-log variant a protocol break, which is the
    /// same reason `unix`'s own `message_origin_str` exists rather than a
    /// `Serialize` derive on the internal type.
    pub fn message_origin(self) -> MessageOrigin {
        match self {
            Self::User => MessageOrigin::UserKeystroke,
            Self::Machine => MessageOrigin::Machine,
        }
    }

    /// The assumption ledger's vocabulary for the same fact — Phase 21K.
    /// A machine on this door is the agent working in the session; the
    /// third value, `glasshouse`, is never a caller's to claim and has no
    /// spelling here.
    pub fn guardrail_origin(self) -> Origin {
        match self {
            Self::User => Origin::User,
            Self::Machine => Origin::Agent,
        }
    }
}

/// One control-API call.
///
/// Every variant is answered against the project the door was opened for —
/// see the door's own doc comment for why that is structural rather than a
/// field every request would otherwise have to repeat.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// Every session in this project, most recently active first.
    ListSessions,
    /// One session's lifecycle, plus what Glasshouse recorded about its
    /// current route.
    SessionState { session: String },
    /// Start a new session under an installed harness — capability map
    /// Phase 14, boxes 5, 6 and part of box 1.
    SpawnSession {
        /// A harness identifier, e.g. `claude-code`.
        harness: String,
        #[serde(default)]
        args: Vec<String>,
        /// How the spawned session is tagged — `worker`, `orchestrator`, or
        /// `normal`. Absent means `worker`: every session this door spawns
        /// is spawned *by* something other than a person (an orchestrator,
        /// a script — see this module's own doc comment), so a session with
        /// no role stated is a worker by default rather than
        /// indistinguishable from one a person started by hand.
        #[serde(default)]
        role: Option<String>,
        /// A natural-language task delivered to the session as its first
        /// message, the instant it is live — distinct from
        /// [`Request::SendMessage`], which addresses a session that already
        /// exists. Absent means the harness starts with nothing sent to it,
        /// same as before this field existed.
        #[serde(default)]
        task: Option<String>,
        /// A per-task guardrail override — Phase 21K line 1008: `force`,
        /// `skip` or `lower`. Recorded on the new session's assumption
        /// ledger the moment its record exists, so every later
        /// [`Request::Preflight`] for it answers under that override and
        /// names it. Absent means no override, which is what every spawn
        /// before this field existed meant.
        #[serde(default)]
        guardrail: Option<GuardrailOverride>,
        /// Where the spawned session is presented — Phase 17 lines 757 and
        /// 761. Absent means headless in this process, exactly as this verb
        /// always spawned. `cmux` asks for a new cmux workspace in the
        /// project root, running an ordinary Glasshouse launch: the answer
        /// then carries `presentation_ref` (the workspace) and `session` once
        /// the pane has recorded itself. When cmux is not available the
        /// spawn still succeeds — headless, here — and the answer says so in
        /// `presentation_note`; an unknown backend is refused by name.
        ///
        /// A session presented this way is recorded by the launch inside
        /// the pane, which takes no `role`: it is recorded as `normal`, and a
        /// `task` reaches it through cmux's own `send` rather than through
        /// this door's session runtime, which never holds it.
        #[serde(default)]
        presentation: Option<String>,
    },
    /// Send one line of text to a live session.
    ///
    /// `origin` says whose line it is — see [`RequestOrigin`], and note that
    /// it defaults to [`RequestOrigin::Machine`], so a caller that says
    /// nothing is speaking as Glasshouse exactly as this verb always did.
    /// `glasshouse api send` states [`RequestOrigin::User`], because a person
    /// ran it.
    SendMessage {
        session: String,
        text: String,
        #[serde(default)]
        origin: RequestOrigin,
    },
    /// Interrupt a live session.
    ///
    /// `origin` carries the same meaning and the same default it does on
    /// [`Request::SendMessage`]: an interrupt is an intervention too, and a
    /// person's `Ctrl-C` through `glasshouse api interrupt` is a different
    /// fact from an orchestrator deciding to stop a worker.
    ///
    /// Never refused by a mute (line 1717) or by a person holding the
    /// keyboard (line 1719) — see `session::api::SessionApi::interrupt` for
    /// why the one verb that *stops* a session is the one verb neither
    /// control may hold back.
    Interrupt {
        session: String,
        #[serde(default)]
        origin: RequestOrigin,
    },
    /// Stop delivering orchestrator-generated messages to one session, for a
    /// stated time — capability map line 1717.
    ///
    /// While a session is muted, [`Request::SendMessage`] carrying
    /// [`RequestOrigin::Machine`] is refused with the remaining time named.
    /// [`RequestOrigin::User`] is unaffected — the point of muting a worker
    /// is to work in it yourself — and [`Request::Interrupt`] is unaffected
    /// whoever sends it.
    ///
    /// `seconds` is required and must be non-zero: *temporarily* is the whole
    /// of what this verb offers, and a mute with no end would be a session
    /// quietly excluded from orchestration with nothing to say when it came
    /// back. It is capped at `unix::MAX_MUTE_SECONDS` server-side, so a
    /// caller may ask for less and cannot ask for more.
    ///
    /// # It does not survive a restart, deliberately
    ///
    /// The state lives in the `glasshouse api serve` process that owns the
    /// session's pseudo-terminal and nowhere else. That process is the only
    /// thing that can deliver a machine message to a session in the first
    /// place — a door that has just started is not running the session that
    /// was muted — so there is no interval in which a lost mute lets a
    /// message through that a persisted one would have stopped. Nothing is
    /// migrated and nothing is written to disk.
    MuteSession { session: String, seconds: u64 },
    /// Lift a mute before it expires — capability map line 1717.
    ///
    /// Answers `ok` whether or not the session was muted, and says which it
    /// was: unmuting a session nobody muted is the state the caller asked
    /// for, not a failure. A session that is not this project's is still
    /// refused as foreign.
    UnmuteSession { session: String },
    /// The tail of one live session's terminal output — capability map line
    /// 745, *"allow the user to enter any orchestrated worker while it is
    /// running."*
    ///
    /// The third of the three verbs that together are a person being *in* a
    /// running worker: [`Request::SendMessage`] puts words in,
    /// [`Request::Interrupt`] stops what is happening, and this is the half
    /// that shows what came back. Until it existed a client built from this
    /// door could type into a worker and could not see it.
    ///
    /// Answered through `session::api::SessionApi::recent_output`, the same
    /// project-scoped seam its two neighbours resolve through, and read-only
    /// in the strong sense [`Request::RecommendRoute`] is: it sends nothing
    /// to the session, signals nothing, spawns nothing, writes to no store
    /// and records no event.
    ///
    /// # A session with no live process is a refusal, not an empty string
    ///
    /// Glasshouse does not persist terminal output, so a session no process
    /// is running has none to give. `recent_output` refuses that with
    /// `ApiError::NotLive` rather than answering `""`, because — in its own
    /// words — *"returning an empty string would be a lie the caller has no
    /// way to detect"*, and this verb carries that distinction onto the
    /// wire rather than flattening it: a session nothing is running comes
    /// back as `status: error`, and a live session that has printed nothing
    /// yet comes back as `status: ok` with an empty `output`. They are
    /// different answers because they are different facts, and a caller
    /// deciding whether to wait or to restart a worker needs to tell them
    /// apart.
    ///
    /// # The bound
    ///
    /// `max_bytes` is capped server-side at `unix::MAX_RECENT_OUTPUT_BYTES`
    /// regardless of what is asked for, so a caller may lower the ceiling
    /// and cannot raise it — the same shape as [`Request::QueryMemory`]'s
    /// `limit`. It matters more here than anywhere else on this door: a
    /// session's scrollback is bounded by the *runtime*, at a size no caller
    /// chose, and this is the one verb whose response would otherwise grow
    /// with how long a worker has been talking.
    RecentOutput {
        session: String,
        #[serde(default = "default_recent_output_bytes")]
        max_bytes: usize,
    },
    /// Current resource capacity and quota telemetry for every model
    /// resource Glasshouse can describe — capability map line 1679.
    ///
    /// Read-only, like every other request this door answers: it never
    /// makes a network request of its own. See
    /// `glasshouse::provider::resources::capacity_json`, which this is
    /// answered with directly, for the exact shape.
    ResourceCapacity,
    /// The current routing-model selection and its health — capability map
    /// line 1680.
    ///
    /// Read-only, like every other request this door answers. Answered from
    /// `glasshouse::config::EffectiveConfig::routing_model` and
    /// `::routing_model_resolution` directly: the recorded choice and the
    /// layer it came from, and whether that choice actually resolves or has
    /// degraded to deterministic heuristics with a reason named in the
    /// type's own words. There is no live latency or health probe anywhere
    /// in this project — a degrade to heuristics *is* the health signal this
    /// line asks for; see those functions' own doc comments.
    RoutingModel,
    /// Where this work would be routed, and why — capability map line 1681,
    /// *"an inspectable routing recommendation without executing it."*
    ///
    /// Read-only, and more strongly so than the rest of this door: it starts
    /// no session, sends no text, takes no checkpoint, writes no routing
    /// observation, and mutates no store. The whole verb is
    /// `main.rs`'s own `route_recommendation` — the same function
    /// `glasshouse route` is, so the command and the door cannot disagree
    /// about where work would go (there is one ranking, not two) — rendered
    /// as JSON rather than as a report.
    ///
    /// `task` is the free-form description of the work, classified exactly
    /// as `glasshouse route --task` classifies it: by keyword, into
    /// `TaskRequirements`, never executed and never interpolated into a
    /// command. Absent or blank means the ranking weighs no task
    /// requirement, byte for byte the no-`--task` behaviour.
    ///
    /// `moment` is one of `session-start`, `task-boundary` or `mid-turn`,
    /// defaulting to `session-start` as the command does.
    ///
    /// `alternatives` bounds how many ranked runners-up and rejected
    /// candidates come back; it is capped server-side at
    /// `unix::MAX_ROUTE_ALTERNATIVES` regardless of what is asked for, so a
    /// caller may lower the ceiling and cannot raise it. Every other part of
    /// the response is bounded by construction: one destination, and one
    /// contribution per scoring term.
    ///
    /// There is deliberately no override here — no `to`, no `fresh`, no
    /// `now`. Those are a *user* telling the router where to go
    /// (`glasshouse route`'s own line 1602 flags), and this verb exists to
    /// ask it a question. Nothing else on this door speaks that vocabulary
    /// either: [`Request::SpawnSession`] names a harness, not a routing
    /// override.
    RecommendRoute {
        #[serde(default)]
        task: Option<String>,
        #[serde(default = "default_routing_moment")]
        moment: String,
        #[serde(default = "default_route_alternatives")]
        alternatives: usize,
    },
    /// This project's lifecycle events, in Glasshouse's own vocabulary
    /// rather than any one harness's — capability map line 701.
    ///
    /// Read-only, like every other request this door answers. Incremental:
    /// `after` is the log position the caller has already consumed — `0` for
    /// the start of the log, or a prior response's `head` — and only events
    /// strictly newer than it come back, oldest first. `limit` bounds how
    /// many events one call returns; it is capped server-side regardless of
    /// what is asked for, so a caller cannot pull an unbounded response by
    /// naming a large number.
    ///
    /// **Since Phase 21K the answer carries a second, independent stream**:
    /// `assumptions` — the guardrail ledger's notifications (a `refuted`
    /// transition, an exceeded budget; capability map line 1050), newer
    /// than `assumptions_after` and bounded by the same `limit`, with
    /// `assumptions_head` as the cursor to pass next time. Two cursors
    /// rather than one because the two ledgers number their rows
    /// independently, and folding one into the other's `seq` would be
    /// exactly the `lifecycle_events` widening the design ruling refuses.
    Events {
        #[serde(default)]
        after: i64,
        #[serde(default = "default_events_limit")]
        limit: usize,
        #[serde(default)]
        assumptions_after: i64,
    },
    /// Register interest in one worker session's completion events, to be
    /// delivered into an orchestrator session — capability map line 733.
    ///
    /// `session` is the worker to watch and `notify` is the session the
    /// completion notification is typed into. Both are resolved through
    /// `session::api::SessionApi`, so both must belong to this project, and
    /// `notify` must be live in *this* process's runtime — an orchestrator
    /// this door did not spawn has no terminal here to be woken through, and
    /// saying so at registration is the difference between an orchestrator
    /// that knows it is not being watched over and one that waits forever.
    ///
    /// Idempotent per `(session, notify)` pair: registering twice replaces
    /// the watch rather than adding a second one, because two watches over
    /// one pair would wake the orchestrator twice for one completion, which
    /// is exactly what line 739 forbids.
    ///
    /// The response carries `from`, the log position the watch starts at.
    /// Nothing already in the log is replayed: registering interest is a
    /// statement about what happens next.
    WatchWorker {
        /// The worker session to watch.
        session: String,
        /// The session a completion is delivered into.
        notify: String,
    },
    /// Search this project's durable memory — capability map line 1111's
    /// project-scoped `memory.search`, and Phase 21F lines 935/936.
    ///
    /// Project-scoped twice over: this door is opened for one already-resolved
    /// project and carries no field naming another (see `super`'s own doc
    /// comment), and the query underneath it —
    /// `memory::search::MemoryStore::search` — filters on
    /// `memories.project_id` in its own `WHERE` clause rather than trusting
    /// that.
    ///
    /// `limit` is capped at `unix::MAX_MEMORY_LIMIT` regardless of what is
    /// asked for — line 1115. A caller may lower the ceiling; it cannot raise
    /// it.
    QueryMemory {
        query: String,
        #[serde(default)]
        history: bool,
        #[serde(default = "default_memory_limit")]
        limit: usize,
    },
    /// One selected memory in full — capability map line 1112's
    /// project-scoped `memory.get`.
    ///
    /// The complement of [`Request::QueryMemory`], which ranks and returns
    /// many, and of [`Request::CurrentMemory`], whose bodies are cut to a
    /// budget: this returns exactly one memory with nothing elided — its
    /// whole body, its supersession, and every provenance field Phase 21B
    /// records, so an agent that found a memory through either of the other
    /// two verbs has somewhere to go for the rest of it.
    ///
    /// Answered through `MemoryStore::get`, which is the module's stated read
    /// boundary: a row bound to another project is an **error**, never an
    /// empty answer — see line 1114 and that method's own doc comment.
    GetMemory {
        /// A memory identifier, or an unambiguous leading part of one — the
        /// same prefix rule `glasshouse memory show` uses. There is no
        /// project component: an identifier names a row, and which project
        /// that row must belong to is this door's business, not the
        /// caller's.
        memory: String,
    },
    /// A concise snapshot of what this project currently knows — capability
    /// map line 1113's project-scoped `memory.current`.
    ///
    /// Answered from `memory::snapshot::snapshot` directly, so this door and
    /// the TUI's project overview cannot disagree about what "current" means:
    /// active memories only, grouped by kind, most recently updated first.
    ///
    /// Bounded on both axes and on every section independently — line 1115's
    /// *"concise results rather than dumping the complete memory database"*.
    /// `limit` caps the entries in any one section and `body_chars` caps each
    /// entry's body; both are capped again server-side
    /// (`unix::MAX_SNAPSHOT_SECTION_LIMIT`, `unix::MAX_SNAPSHOT_BODY_CHARS`),
    /// so a caller may lower either ceiling and cannot raise it. Nothing is
    /// dropped silently: a capped section reports how many entries it left
    /// out and a cut body says that it was cut.
    CurrentMemory {
        #[serde(default = "default_snapshot_limit")]
        limit: usize,
        #[serde(default = "default_snapshot_body_chars")]
        body_chars: usize,
    },
    /// Retrieve a checkpoint — capability map line 66, "retrieve a completed
    /// worker result or checkpoint." A worker has no other durable "result"
    /// format Glasshouse owns (Phase 19's checkpoints are the shipped
    /// mechanism; see the evidence ledger); this is the read half of
    /// [`Request::TakeCheckpoint`]'s write.
    GetCheckpoint {
        /// A checkpoint id or unambiguous prefix, or absent/`"latest"` for
        /// the project's most recent checkpoint — the same rule
        /// `glasshouse checkpoint show` uses.
        #[serde(default)]
        checkpoint: Option<String>,
        /// The rendered handoff document, rather than the terser bootstrap
        /// prompt — the same distinction `glasshouse checkpoint show
        /// --document` makes.
        #[serde(default)]
        document: bool,
    },
    /// Ask the guardrail about an intended change — Phase 21K lines
    /// 1004–1009, 1013, 1036, 1049, 1052, 1053.
    ///
    /// `change` is what the agent **states** about the change: files and
    /// subsystems touched, reversibility, blast radius, the flags for a
    /// migration, a destructive operation, a security or data-integrity
    /// impact, an unfamiliar integration, an architectural change or a broad
    /// refactor, the evidence class its premise rests on, and a coarse
    /// budget (with what has been spent, when re-evaluating). Nothing is
    /// read from the session to fill any of it in, and an unknown field —
    /// `reasoning`, `transcript` — is refused rather than ignored.
    ///
    /// Answers a risk class, **which factor** decided it, a verdict from the
    /// configured mode and the session's per-task override, at most three
    /// critical-assumption prompts, a page of guidance in the map's own
    /// words, and the seven explicit responses. Trivial never gates, and
    /// answers with no prompts at all.
    ///
    /// With a `session` — this project's, or refused — the preflight is also
    /// **recorded**: a gate row on the session's ledger says it fired and
    /// why; a budget found exceeded is a second row and the answer lists the
    /// session's open assumptions to re-evaluate (line 1039); and a
    /// substantial change takes a checkpoint through the same path
    /// [`Request::TakeCheckpoint`] uses, unless `guardrails.mode` is `off`
    /// (line 1036). Without a session the answer is the same and nothing is
    /// written.
    Preflight {
        #[serde(default)]
        session: Option<String>,
        #[serde(default)]
        change: ChangeFactors,
    },
    /// State one critical assumption — Phase 21K lines 998, 1014–1016.
    ///
    /// **Six fields, all required, and nothing else.** The claim (one
    /// sentence; refused over the ceiling rather than cut), the current
    /// evidence, its source class (`observed`, `user_requirement`,
    /// `repository`, `external`, `experiment`, `inference`), the uncertainty
    /// (`low`, `medium`, `high`), the affected scope (`affected`), and the cheapest
    /// useful verification step. There is no field for the reasoning that
    /// produced the claim and no column for one. Every field is treated as
    /// untrusted text.
    ///
    /// Recorded in the `proposed` state. `origin` defaults to the machine,
    /// as on every other verb here, and becomes the ledger's `agent`.
    RecordAssumption {
        #[serde(default)]
        session: Option<String>,
        claim: String,
        evidence: String,
        evidence_source: EvidenceSource,
        uncertainty: Uncertainty,
        affected: String,
        verification: String,
        #[serde(default)]
        origin: RequestOrigin,
    },
    /// Append a transition — Phase 21K lines 1018, 1019, 1041, 1051.
    ///
    /// `state` moves the assumption; absent, the current state is
    /// re-stated, which is how a `response` or a `note` is recorded without
    /// a move. `response` is one of the seven a preflight offers, and is the
    /// door *"accepting the chosen one as a transition note"*. A transition
    /// to `waived_by_user` is refused unless `origin` is `user` — the door
    /// cannot check that, but it can insist it be said.
    ///
    /// `record_failed_approach`, with `state: refuted`, writes one
    /// `failed_attempt` memory through the existing store, with provenance
    /// naming the assumption (line 1019); the transition's `subject` is the
    /// memory's id. Without the flag, a refutation writes no memory at all.
    UpdateAssumption {
        /// An assumption identifier, or an unambiguous leading part of one.
        assumption: String,
        #[serde(default)]
        state: Option<AssumptionState>,
        #[serde(default)]
        note: Option<String>,
        #[serde(default)]
        response: Option<GuardrailResponse>,
        #[serde(default)]
        record_failed_approach: bool,
        #[serde(default)]
        origin: RequestOrigin,
    },
    /// This project's assumptions with their current states, newest first —
    /// Phase 21K line 1048. With a `session`, that session's only, plus its
    /// session-level events (gates, overrides, budgets). `limit` is capped
    /// server-side at `unix::MAX_ASSUMPTIONS_LIMIT`.
    ListAssumptions {
        #[serde(default)]
        session: Option<String>,
        #[serde(default = "default_assumptions_limit")]
        limit: usize,
    },
    /// Promote a **supported** assumption into durable project memory —
    /// Phase 21K lines 1017, 1020 — as a `decision`, a `constraint` or a
    /// `finding`, and as nothing else. Any other state is refused: a task
    /// assumption stays apart from project decisions until it has been
    /// supported *and* somebody explicitly promoted it. Never automatic.
    PromoteAssumption {
        assumption: String,
        kind: PromotionKind,
        #[serde(default)]
        note: Option<String>,
        #[serde(default)]
        origin: RequestOrigin,
    },
    /// Take a checkpoint for a session.
    TakeCheckpoint {
        /// Named explicitly, or the project's most recently active session —
        /// the same rule `glasshouse checkpoint save` uses outside this door.
        #[serde(default)]
        session: Option<String>,
        objective: String,
        state: String,
        #[serde(default)]
        decisions: Vec<String>,
        #[serde(default)]
        failed_approaches: Vec<String>,
        #[serde(default)]
        files: Vec<String>,
        #[serde(default)]
        test_state: Option<String>,
        #[serde(default)]
        next_actions: Vec<String>,
    },
}

/// One control-API answer.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Ok { result: serde_json::Value },
    Error { message: String },
}

impl Response {
    pub fn ok(result: serde_json::Value) -> Self {
        Response::Ok { result }
    }

    pub fn err(message: impl std::fmt::Display) -> Self {
        Response::Error {
            message: message.to_string(),
        }
    }
}
