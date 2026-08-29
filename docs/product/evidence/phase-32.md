# Capability evidence — phase 32

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 32 — the model-resource registry, and why most of it was already there

**Audit first.** Every kind of resource capacity the map's twelve lines ask
for already had a place to live before this batch: `BackendResource`
(`crates/glasshouse/src/profile/mod.rs`) distinguishes a harness's native
subscription from a direct provider from the local gateway, and
`provider::templates()` (`crates/glasshouse/src/provider/mod.rs`) already
enumerates every router, the two generic templates, and both local-inference
servers. What none of that carried was the phase's own fixed requirement:
**which quota shape each kind actually has, honest about the fact that they
are not the same shape.** A `BackendResource::DirectProvider { provider:
"ollama" }` and one naming `"openrouter"` were, and remain,
indistinguishable as types — both are "a direct provider" — and nothing
computed the difference that line 1185 asks for.

That is what `crates/glasshouse/src/provider/registry.rs` adds:
`Locality` (`Local`/`Remote`), `QuotaModel`
(`RollingWindowSubscription`/`MeteredBalance`/`Unmetered`/
`DelegatedToUpstream`), and `ResourceKind`, which enumerates every native
harness, every provider template, and the gateway, tagged with both. It is a
derived view over the two catalogs above, not a rewrite of either — the
packet's own instruction, and the honest shape once the audit showed what
was missing.

**The registry nothing consults is not a registry (practice §5), so this
one is wired into the one place a `BackendResource` is actually resolved for
a real session.** `profile::apply_direct_provider` and `profile::apply_gateway`
(`crates/glasshouse/src/profile/mod.rs`) each now push a `"resource kind"`
`MechanismNote` built from `ResourceKind::from_direct_provider` /
`ResourceKind::GlasshouseGateway`. Every launch's mechanism notes reach
`main.rs::mechanism_summary`, which is in the "opening a harness session"
log line for every real session — main.rs was not edited (it is Phase 49's
file this round); the existing consumer picks the new note up because it
already iterates every mechanism the overlay carries.

**One honest limit, stated rather than hidden.** `registry()` — the function
that lists every resource kind at once — has no production caller of its
own today; only the per-instance classification (`from_direct_provider` /
`GlasshouseGateway`) is exercised at launch. Nothing in the shipped binary
currently prints "here is everything Glasshouse can describe" to a user —
that would be a `glasshouse resource list`-shaped command or a `doctor`
integration, and both live in files this package does not own
(`cli.rs`/`main.rs`/`shell/**` are outside `YOURS`). The registry *type* and
its *classification* reach production; the registry's *enumeration entry
point* is proven only by its own tests. Recorded rather than papered over,
per practice §5's own distinction between "the type exists" and "the
shipped binary reads it."

State: **PARTIALLY VERIFIED** for line 1183 (the type and its per-instance
classification are production-reachable; the enumeration entry point is
not) — **COMPLETE** for lines 1184, 1185, 1186, 1187, 1188, 1189, 1190,
1191, 1192, 1193 and 1194 (see the per-line breakdown below).

Production evidence:
- `crates/glasshouse/src/provider/registry.rs::ResourceKind`, `Locality`,
  `QuotaModel`, `registry()` — the registry itself.
- `crates/glasshouse/src/profile/mod.rs::apply_direct_provider` — pushes a
  `"resource kind"` mechanism note via `ResourceKind::from_direct_provider`
  on every direct-provider launch.
- `crates/glasshouse/src/profile/mod.rs::apply_gateway` — same, via
  `ResourceKind::GlasshouseGateway`, on every gateway-backed launch.
- `crates/glasshouse/src/main.rs::mechanism_summary` (unedited; the existing
  reader) — every real launch's log line includes the new note, because it
  already renders every `MechanismNote` the overlay carries.

Regression evidence:
- `provider::registry::tests::*` (10 tests) — the classification itself:
  native vs. direct-provider vs. gateway have different `QuotaModel`s;
  Ollama and llama.cpp are `Local`/`Unmetered`; every router and both
  generic templates are `Remote`/`MeteredBalance`; a `localhost`-shaped
  base URL (LiteLLM's own template) does not by itself make a provider
  local inference; Claude Code, Codex and Antigravity are all describable
  as native subscriptions; `from_direct_provider` agrees with `registry()`'s
  own classification.
- `profile::tests::resolving_a_direct_provider_profile_records_whether_it_is_local_or_remote`
  — the production caller, proven at the launch path: an `"ollama"`-named
  provider's overlay carries `"local"`/`"unmetered"`; an
  `"openrouter"`-named provider's carries `"remote"`/`"metered balance"`.
- `profile::tests::resolving_a_gateway_backed_profile_assigns_the_session_a_provider_and_a_model`
  (extended) — a gateway-backed launch's overlay carries `"glasshouse
  gateway"`/`"delegated"`.

Failure/isolation evidence:
- `provider::registry::tests::a_label_never_contains_a_credential_shaped_string`
  — a resource-kind label is built from fixed phrases and the provider
  *name* only, never from anything resolved through `crate::secret`.
- The existing credential-absence sweeps over `overlay.mechanisms()`
  (`profile::tests::a_resolved_credential_never_reaches_a_rendering` and
  siblings) cover the new note for free: they iterate every mechanism, not
  a named subset, and all still pass with the note present.

Mutation evidence (practice §41, §35 — the call site, not only the
classification):
- `locality_of` mutated to return `Locality::Remote` unconditionally: `ok`
  before, `FAILED` at
  `provider::registry::tests::ollama_and_llama_cpp_are_local_and_unmetered`,
  `ok` after restore.
- The `"resource kind"` push deleted from `apply_direct_provider`: `ok`
  before, `FAILED` at
  `profile::tests::resolving_a_direct_provider_profile_records_whether_it_is_local_or_remote`,
  `ok` after restore.
- The `"resource kind"` push deleted from `apply_gateway`: `ok` before,
  `FAILED` at
  `profile::tests::resolving_a_gateway_backed_profile_assigns_the_session_a_provider_and_a_model`,
  `ok` after restore.

Platform/external evidence:
- `cargo test -p glasshouse --lib` (macOS, this worktree): 1113 passed, 0
  failed, run alone (practice §40).
- `cargo test -p glasshouse --test provider_discovery --test routing_policy`:
  11 + 18 passed, 0 failed.
- `cargo clippy -p glasshouse --all-targets -- -D warnings`: clean.
- `cargo fmt -p glasshouse -- --check`: clean (only the new file needed
  formatting; no other file touched by the formatter).
- `cargo doc -p glasshouse --no-deps`: clean.
- Not run: the Linux/Windows legs of the local gate, and the full
  `scripts/ci-local.sh` (forbidden file for this package; §40 — do not run
  it beside another cargo invocation regardless).
- `.agent-runtime/endpoint-authenticated-results.md` (2026-08-27, not
  re-probed): confirms Ollama as `"local, no credential"` — the same
  classification `locality_of` computes independently from
  `IntegrationKind::LocalInference` — and confirms that a *remote* provider's
  balance can be exhausted while the key still authenticates (NVIDIA, Nous,
  Cerebras, DeepSeek all `200` on `/models` and then refuse inference),
  which is exactly the state `QuotaModel::MeteredBalance` names honestly:
  a balance, not a boolean.

Missing evidence:
- No production caller for `registry()` itself (the full enumeration) — see
  the note above. A `glasshouse resource list`-shaped surface or a `doctor`
  integration would close it; both are outside this package's files.
- Quota *telemetry* (a rolling-window reset time, a spent balance, a
  request count) does not exist — `QuotaModel` names a shape, never a
  number. That is Phase 32B, and it is at zero.

---

## Per-line disposition

**1183 — Create a registry describing model resources available to
Glasshouse.**
CLOSED, with the enumeration-caller caveat above.
`provider::registry::{ResourceKind, Locality, QuotaModel, registry()}`.

**1184 — Represent native subscriptions separately from API-key or gateway
resources.**
Already satisfied at the type level before this batch —
`BackendResource::{Native, DirectProvider, GlasshouseGateway}` and
`ProfileClass` (`crates/glasshouse/src/profile/mod.rs`), proven by
`profile::tests::profile_class_matches_the_backend_kind`. This batch adds
the quota-level distinction the map's fixed requirement actually asks
for — `QuotaModel::RollingWindowSubscription` for native,
`MeteredBalance`/`Unmetered` for a direct provider,
`DelegatedToUpstream` for the gateway — proven by
`provider::registry::tests::a_native_subscription_and_a_direct_provider_have_different_quota_shapes`
and
`provider::registry::tests::the_gateway_is_a_third_kind_delegated_rather_than_flattened_into_either`.
CLOSED (already, and strengthened).

**1185 — Represent local inference resources separately from remote
resources.**
This was the real gap: `BackendResource::DirectProvider` carried a bare
provider name, and nothing distinguished Ollama from OpenRouter beyond the
string. Closed by `Locality`, computed in `locality_of` against
`IntegrationKind::LocalInference` (matched by `IntegrationId::slug`, not by
a base-URL heuristic — see
`provider::registry::tests::a_localhost_base_url_does_not_by_itself_make_a_provider_local_inference`,
which pins that a self-hosted LiteLLM proxy on `localhost` is still
classified `Remote`). Reaches production via the same mechanism note.
CLOSED.

**1186 — Allow the registry to describe Claude Code subscription capacity.**
**1187 — Allow the registry to describe Codex or ChatGPT-backed capacity.**
**1188 — Allow the registry to describe Google or Antigravity-backed
capacity.**
Already satisfied at the type level — `LaunchProfile::native(harness)` is
constructible for every `IntegrationId`, including `ClaudeCode`, `Codex`
and `Antigravity`, and this is the one profile every harness has "by
construction rather than by a configuration entry" (the type's own doc
comment). `registry()` now enumerates a `NativeSubscription` entry for
every `IntegrationKind::Harness` integration by construction — not by
naming these three specially — so any harness this project later adds a
native profile for is described the same way. Proven by
`provider::registry::tests::claude_code_codex_and_antigravity_are_native_subscriptions`.
CLOSED (already; the registry now also enumerates them explicitly).

**1189 — Allow the registry to describe OpenRouter-like gateways.**
**1190 — Allow the registry to describe other user-configured gateways such
as UnoRouter, AnyRouter, Kilo, or Nous.**
Already satisfied by `provider::templates()`'s `openrouter`, `unorouter`,
`anyrouter`, `kilo` and `nous` entries (Phase 9C/9D, `crates/glasshouse/src/provider/mod.rs`),
plus the two generic templates (`openai-compatible`, `anthropic-compatible`)
for a router this project has no built-in template for. `registry()` now
wraps every one of these as a `ResourceKind::DirectProvider`, all correctly
`Remote`/`MeteredBalance` —
`provider::registry::tests::every_router_and_generic_template_is_remote_and_metered`
and
`provider::registry::tests::openrouter_unorouter_anyrouter_kilo_and_nous_are_all_describable`.
CLOSED (already; the registry now names the quota shape too).

**1191 — Allow the registry to describe Ollama-backed local models.**
**1192 — Allow the registry to describe llama.cpp-backed local models.**
The `ollama` and `llama-cpp` provider templates already existed
(Phase 9D); what they lacked was ever being told apart from a remote
provider — see line 1185. Closed the same way, by the same mechanism:
`provider::registry::tests::ollama_and_llama_cpp_are_local_and_unmetered`.
CLOSED.

**1193 — Store secrets through environment references, OS keychain
integration, or provider-native authentication rather than plaintext
project memory.**
Already CLOSED, not touched by this package. `crate::secret::SecretRef`
(`Environment` / `OsCredential`) and `ProviderConfig::credential_env` /
`credential_store` (`crates/glasshouse/src/config/mod.rs`, read, not
edited) hold names and OS-keychain references only, never a value — Phase
9E. Independently, `memory::extract::credentials::screen` and `::scrub`
(`crates/glasshouse/src/memory/extract/credentials.rs`) fail-closed refuse
any memory whose text carries a recognized credential shape before it ever
reaches a row — the "source-scanning guard idiom" the packet asks Pass 2 to
find and cite rather than re-implement. Cited, not duplicated.

**1194 — Keep resource configuration outside durable project knowledge.**
Already CLOSED, not touched by this package. `crate::profile`'s own module
doc states and `harness::resolving_a_launch_profile_touches_no_files`
enforces that a launch profile "is configuration, not project memory" and
never touches the project's SQLite database. The database's own schema is
fully pinned by
`session::store::tests::the_project_database_schema_has_nowhere_to_put_a_credential`
(`crates/glasshouse/src/session/store.rs`, read, not edited) — the same
mechanism the packet's Pass 2 points at for 1193 doubles as the negative
proof for 1194: there is no column anywhere in `sessions`, `memories`,
`lifecycle_events` or `checkpoints` shaped like a provider template, a base
URL, a credential-variable name or a header, any more than there is one
shaped like a credential value. Resource configuration lives in
`crate::config`'s TOML files, structurally on the other side of the
boundary that test pins. Cited, not duplicated — no new test was written
for this line because `database.rs`, `config/mod.rs` and
`session/store.rs` are outside `YOURS`, and the existing guard already
proves the negative at the schema level, which is the "wide" render the
packet's Pass 2 (practice §17) asks for: a pinned enumeration of every
column, not a sample of rendered output that could be truncated.

---

## Phase 32 line 1183 — CLOSED 2026-08-29 (batch 47)

**This discharges the "enumeration-caller caveat" the per-line disposition
above recorded.** That entry already read *"CLOSED, with the enumeration-caller
caveat"*, and named what would discharge it: *"a `glasshouse resource list`-shaped
surface or a `doctor` integration would close it; both are outside this
package's files."* Both now exist. Found by the second-pass ledger sweep, which
correctly refused to call it closeable and left the ruling here.

Contract: Given a Glasshouse installation, when something asks what model
resources Glasshouse can describe, it gets the full enumeration — native
subscriptions, direct providers, local runtimes and the gateway — as a
registry of resource *kinds*, independent of what any one user has configured.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/provider/registry.rs:241` — `registry()`, building
  `ResourceKind::NativeSubscription` per harness, one entry per
  `provider::templates` entry, and `ResourceKind::GlasshouseGateway`.
- `crates/glasshouse/src/provider/resources.rs:436` and `:465` — the
  `glasshouse resources` report iterates the whole registry, in production.
- `crates/glasshouse/src/main.rs` — `status_report` reads it for
  `glasshouse status`.

The caveat's own words were "nothing in the shipped binary currently prints
'here is everything Glasshouse can describe'". Two things now do.

Regression evidence:
- `provider::resources::tests::capacity_json_carries_health_separately_from_capacity`
  and `::every_resource_in_the_report_names_its_telemetry_class` — the latter
  drives the report over the **whole** registry rather than a sample, and
  asserts `registry().len()`.
- `tests/provider_discovery.rs:781` — `glasshouse resources --no-harness`
  through the shipped binary.

Mutation, run by the orchestrator:

| mutation | vocabulary | result |
|---|---|---|
| `out.push(ResourceKind::GlasshouseGateway);` → deleted (`registry.rs:254`) | `skip-state-update` | **killed** against `--lib`; `capacity_json_carries_health_separately_from_capacity` FAILED at `resources.rs:1775` |

**A recorded limit, found by running the mutation twice rather than once.**
The same mutation **SURVIVED** `--test provider_discovery`, and its
`test result:` line confirms that was not a void verdict — 38 tests really
ran. So registry *membership* is watched at lib level and is **not** watched
by the binary-level `provider_discovery` suite: dropping a whole resource kind
changes `glasshouse resources`' output and no end-to-end test notices. That
does not block this line, which asks for the registry to exist and describe
resources, but it is the honest boundary of what the binary-level evidence
proves, and it is the kind of thing a future line about the *report's*
completeness would need to close.
