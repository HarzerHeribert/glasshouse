# Glasshouse capability evidence ledger

This ledger supports—but never replaces—the authoritative
[`GLASSHOUSE_IMPLEMENTATION_CAPABILITY_MAP.md`](GLASSHOUSE_IMPLEMENTATION_CAPABILITY_MAP.md).
It maps requirements to observable product contracts, production paths, and
non-vacuous regression evidence.

Populate entries incrementally as a capability becomes active or as previously
checked work is reconciled. Do not spend a whole implementation cycle filling
hundreds of future entries speculatively.

## Entry template

```markdown
### <phase and stable short name> — <exact capability text>

Contract: Given <context>, when <trigger>, Glasshouse <observable behavior>,
while preserving <invariant or failure behavior>.

State: NOT STARTED | SCAFFOLDED | PARTIALLY VERIFIED | LOCALLY VERIFIED |
CI VERIFIED | COMPLETE

Production evidence:
- `<file>: <symbol/path>` — why this is a real reachable production path

Regression evidence:
- `<test name>` — behavior proved and platforms actually executed

Failure/isolation evidence:
- `<test or probe>` — negative, fail-closed, cleanup, or boundary behavior

Platform/external evidence:
- `<CI run or runtime probe>` — commit and platforms covered

Missing evidence:
- exact remaining proof or implementation
```

## Evidence rules

- Quote the capability exactly enough to find it in the map.
- Keep the contract to one product sentence.
- Cite symbols and test names, not merely directories.
- State which platform actually executed a test.
- A test-only type or fake caller is not production evidence.
- A checked box requires **COMPLETE**.
- If later evidence contradicts an entry, downgrade it immediately and reopen
  the map checkbox if necessary.

## Active entries

### Phase 9D — a connectivity test that makes a request, a manual model refresh, and a catalogue that survives a restart (three lines, Phase 9D at 14 of 14)

Map lines 425, 426, 427:

> ☑ Allow the user to test provider connectivity from settings before enabling
> it for routing.
> ☑ Allow the user to refresh a provider's model list manually when the
> provider exposes model discovery.
> ☑ Cache discovered model metadata with a timestamp rather than querying
> remote model catalogs on every Glasshouse start.

Contract: Given a configured provider, when the user asks Glasshouse to test
it, refresh its models, or start with a cached catalogue, Glasshouse performs
exactly the network request the user asked for and no other — while preserving:
the credential never reaches a log, a `Debug`, a diagnostic, the screen or the
cache file; a slow or dead endpoint never freezes the interface; and a cached
catalogue is always presented with **when** it was fetched.

State: **COMPLETE** for the three lines. **Phase 9D is fourteen of fourteen.**

Production evidence:

- `shell/state.rs: ShellState::handle_key` → `Action::RunProviderProbe` — `t`
  and `m` in the Providers section plan a probe and raise an action; the
  overlay produces a `ProviderProbeIntent` carrying **`SecretRef` names only**,
  never a value.
- `shell/mod.rs: spawn_provider_probe` — the one place a probe leaves the
  drawing thread. Resolves the credential through
  `PreferNativeSecretStore::detect()` **inside the worker thread**, makes one
  bounded request, returns the outcome on an `mpsc` channel and nudges the
  event loop with `AppEvent::Redraw`. The run loop drains on that wake-up *and*
  on every tick, so an answer cannot be stranded by a wake that raced a tick.
- `provider/discovery.rs: connectivity`, `model_catalogue` — the requests
  themselves, with `CONNECT_TIMEOUT` 5 s, `RESPONSE_TIMEOUT` 10 s,
  `TOTAL_TIMEOUT` 20 s and an 8 MiB body cap. `http_status_as_error(false)` is
  load-bearing: with `ureq`'s default a `401` arrives shaped exactly like a
  refused connection. `max_redirects(0)` is the second — following one would
  re-attach the credential to a host named at runtime, and `kilocode.ai`
  answers `308` to `kilo.ai` today.
- `provider/cache.rs: ModelCache::{load, store}` under
  `paths.rs: RuntimePaths::provider_cache_dir` — the catalogue lives in the
  **data** directory, not config: the user did not type it, it has a provenance
  and an age, and Glasshouse rewrites it itself. `load` has **no error type at
  all** — absent, truncated, wrong version or filed under another provider all
  mean "no cache hit, carry on" — and the module has no HTTP client, which is a
  stronger guarantee than remembering not to call one.

Regression evidence (macOS local; all three platforms in CI):

- `an_endpoint_that_accepts_and_never_answers_is_bounded_by_the_timeout` — a
  fixture that accepts and sends nothing is bounded and reported as a timeout.
- `a_provider_that_accepts_and_never_answers_never_blocks_the_drawing_thread` —
  the production `spawn_provider_probe` against a really-hanging fixture, with
  the main thread handling keys and drawing; asserts **more than five frames
  were drawn while the socket was open**.
- `an_endpoint_answering_401_is_reachable_but_rejected_not_unreachable` — the
  distinction that tells a user whether they have a credential problem or a URL
  problem.
- `opening_settings_with_a_cached_catalogue_opens_no_connection_at_all` —
  asserted on the fixture's **connection count**, not on elapsed time.
- `a_manual_refresh_writes_the_catalogue_to_disk_and_a_reopen_finds_it`,
  `a_cached_model_list_is_never_shown_without_when_it_was_fetched`.
- `a_provider_with_no_established_model_discovery_says_so_and_is_not_an_error`,
  `a_row_never_advertises_a_refresh_key_for_a_provider_that_cannot_refresh`.
- `the_default_timeouts_are_the_named_constants_and_none_is_unset` and
  `the_run_loop_probes_with_the_default_timeouts` — a default that quietly lost
  the response timeout is the regression the batch exists to prevent.
- `tests/provider_discovery.rs` — 11 integration tests, including the
  two-way template/discovery matrix.

Failure/isolation evidence:

- `the_credential_reaches_the_authorization_header_and_no_other_surface` —
  `ProbeRequest`'s hand-written `Debug` prints `REDACTED`; the field is private
  with no accessor.
- `a_planted_credential_never_reaches_the_cache_file_on_disk` — asserted
  against the **bytes written**, not the type.
- `a_provider_name_that_looks_like_a_path_cannot_escape_the_cache_directory` —
  `file_stem` slugifies to `[a-z0-9-]` and appends 16 hex characters of a
  SHA-256 of the original name, so `my provider` and `my/provider` cannot
  collide and neither can contain a separator or be `.`/`..`.
- `a_connectivity_result_never_enables_or_disables_the_provider_it_is_about` —
  line 425's "before enabling it for routing" is a report, not a decision.
- `unreachable_reason` is built from a fixed set of `&'static str` phrases, so
  no text from a peer, a header or a URL can reach a diagnostic through that
  path — the same structural trade `Secret`'s `Debug` makes.

Platform/external evidence — **the shipped binary, driven in a real terminal by
the orchestrator on 2026-08-26**, against an isolated config and data dir:

- a live refresh: `417 models from GET https://openrouter.ai/api/v1/models,
  cached at 2026-08-26 08:10:23Z`, rendered on the row as
  `models: 417 cached, fetched 2026-08-26 08:10:23Z (just now)`;
- a refused host: `` `dead-host`: could not reach openai-chat at
  http://127.0.0.1:60999/v1 — GET … unreachable — the connection was refused``;
- **a real endpoint that accepts and never answers**: probe started 10:09:26,
  reported 10:09:37 — `` no answer within 10004ms — the connection was accepted
  but nothing came back``, the shipped 10-second value;
- **and the interface stayed usable through it.** Three `Down` presses while
  that socket was open each moved the cursor, and the row kept reporting
  `models: testing connectivity...`;
- the cache file on disk: `live-openrouter-f2c3f678c97e5cc9.json`, 22 KB,
  `version 1`, 417 entries, `fetched_at 1787731823` — and **zero occurrences of
  the planted credential** in its raw bytes;
- **restart with a populated cache re-fetched nothing**: the row read
  `417 cached, fetched 2026-08-26 08:10:23Z` after a fresh process start, with
  `fetched_at` *and* the file's mtime both unchanged at `1787731823`.

Mutation evidence: thirteen by the team lead, all killed, each verdict read
from the named test's own result line. **Three more by the orchestrator,
independently, on the load-bearing properties** — `ProbeRequest`'s `Debug`
printing the credential (KILL), the caller *joining* the probe thread so the
request blocks the drawing thread (KILL — a different mutation from the lead's,
which ran the probe inline, so the responsiveness guarantee is proved two ways),
and re-promoting z.ai (KILL at two independent layers).

Missing evidence: none for these three lines. The batch is macOS-local plus CI;
nothing here makes a platform-specific claim.

### Correction the orchestrator made on review: z.ai's model list stays `Unverified`

The batch promoted six `model_list_endpoint` declarations from `Unverified` to
`Verified` on the strength of live probes. **Five of the six were reproduced
independently by the orchestrator and stand.** The sixth does not.

z.ai answered `401` rather than `200`, and the batch promoted it anyway with
this reasoning, quoted from its own doc comment: *"A host that served nothing
there would have answered `404`, exactly as the control in the Responses probe
below did."* That is exactly the right control — and it was cited from a probe
run against **OpenRouter**, not against z.ai. Run against z.ai on 2026-08-26 it
fails:

| request | status |
|---|---|
| `https://api.z.ai/api/paas/v4/models` | 401 |
| `https://api.z.ai/api/paas/v4/definitely-not-real-xyz` | 401 |
| `https://api.z.ai/api/paas/v4/nonsense/deep/path` | 401 |
| `https://api.z.ai/api/paas/v9/models` (no such version) | **200** |
| `https://api.z.ai/totally/bogus` | 404 |

The service refuses every path under its API prefix identically and will not
say whether a route exists until a credential is presented, so the `401`
discriminates nothing. The `404` behaviour that made the reasoning look sound
is real but lives *outside* the prefix, where the probe cannot use it.

`zai.model_list_endpoint` is back to `Unverified`; the base URL is unchanged
and still `unverified_support`. Only the claim that a model list is served at
`<base>/models` is withdrawn. Establishing it needs one **authenticated**
request with the user's own key — a free-models-only condition away, and it
belongs to whoever spends it.

Visible in the shipped binary: `glasshouse doctor` prints `model list endpoint:
unverified` for a z.ai-templated provider, and the settings row now reads `no
model-discovery endpoint established for this provider` where it previously
said `none cached — press m to fetch` — an invitation to press a key that would
have fetched a `401`.

**The transferable rule, which is this project's own applied to the wrong
subject: a control has to be run against the host it is being used to justify.**
A control borrowed from another service is a statement about that service. This
is the fifth declaration in this project derived from an artifact that did not
support the use it was cited for.

**A second, smaller finding from the same re-probe:** UnoRouter answered `374`
entries at 09:00 and `369` an hour later. A catalogue that moves within the hour
is why every citation names a date, and why nothing downstream may treat a count
as stable.

### Phase 9E — the macOS Keychain, a labelled fallback, and a hang that would have frozen the TUI (three lines)

Contract: Given a user who stores a provider credential, when Glasshouse needs
it at launch, it resolves the value from the operating system's own secure store
where one is available and from the environment otherwise — while preserving:
the value never enters configuration, a log, a `Debug` or Git; the user can see
**which** store answered and delete what is stored; and an unavailable native
store is reported plainly rather than silently degraded.

State: **COMPLETE** for the three lines. Phase 9E is eleven of thirteen.
Windows Credential Manager and Secret Service stay **unchecked** — neither is
provable from this machine, and the packet forbade checking them.

#### The defect that justifies "run the binary" on its own

`glasshouse doctor`, pointed at a provider whose credential was in the Keychain,
**hung indefinitely** — exit 124 under `timeout 30`, no output, no visible
dialog. The stack sample ends in
`Security::SecurityServer::ClientSession::decrypt`.

`SecKeychainFindGenericPassword` decrypts the item, and decryption consults the
item's access control list. For an item **this binary did not create**, the list
does not name it, so the call blocks waiting for a user to answer an
authorization dialog that a piped process never shows. The same read is on the
path that starts a session, where it would have frozen the TUI.

Fixed with one `SecKeychainSetUserInteractionAllowed(0)` before the first
Keychain call any store can make: the call now fails cleanly, resolution falls
back to the environment, and `describe` says so. Re-run: exit 0, correct output,
no hang.

**Declared with a bare `#[link(name = "Security", kind = "framework")]` extern
rather than a new direct dependency** — the framework is already linked via
`keyring`. Accepted by the orchestrator: one FFI call with a trivial signature
is a smaller commitment than a second crate on the secret path.

**The cost, stated rather than hidden:** a credential filed by hand with
`security add-generic-password` is not read. Storing it *through* Glasshouse is
what puts this binary on the item's ACL.

#### CI evidence — and the red run that preceded it

`5b3a4cf` went **red on Linux, Windows and lint** while macOS stayed green.
`PROBE_ACCOUNT` is read only inside the `#[cfg(target_os = "macos")]` backend
but was declared outside it, so on every other target it is dead code, which
`-D warnings` makes fatal. One constant, three red jobs, and a class of defect
macOS CI structurally cannot catch.

Fixed in `6cb9bf1` by giving the constant the same gate as the module that
reads it. **CI green on Linux, macOS, Windows and lint** there.

The fix was verified *before* pushing, by flipping every `target_os = "macos"`
in that file to `"linux"` so this machine compiles the fallback arms instead —
the same path the other platforms take. `rustup target add
x86_64-unknown-linux-gnu` was tried first and did not work: the target installs
but its `core`/`std` did not resolve. The cfg flip needs no toolchain at all and
is recorded as practice §18.

#### A durability caveat, measured rather than assumed

| what | result |
|---|---|
| store and read in one process | reads |
| store, read from a second invocation of the same binary | reads |
| store, **rebuild the binary**, read | **does not read** |

The ACL binds to the binary's code identity, so an **unsigned** build — which
Glasshouse is today — breaks the link on rebuild. For a signed release the
designated requirement should be the signing identity and stable across
versions; **that is not verified and is not claimed.** When configuration
records a credential the store will not return, `doctor` says so and says what
to do.

#### Production reachability — the one line the packet scoped out

The packet forbade `main.rs`, so the batch left the launch path building an
`EnvironmentSecretStore` and flagged it rather than reaching into a forbidden
file. **The orchestrator made that change**: `launch_session` now builds
`PreferNativeSecretStore::detect()`. Without it, "prefer the macOS Keychain"
would have been true of the store, of `doctor` and of settings, but not of
`glasshouse run` — and a mechanism with no production caller does not get its
box.

Verified against the built binary: `glasshouse doctor` exits 0 and reports
`credentials resolve from: the macOS Keychain, then the process environment` —
which is line 2's labelled fallback, in the shipped output.

#### Mutations

The orchestrator's own: making `PreferNativeSecretStore::detect` never prefer
the native store **failed two tests**, including
`macos_only_a_keychain_credential_reaches_a_launch_overlays_environment`.

#### A forbidden file that could not be avoided, and was flagged

`SecretRef` gaining an `OsCredential` variant breaks every exhaustive match on
it, including one in a test fake inside `profile/mod.rs`. Production code in
that module is untouched. This is the same class as `Provider` gaining a field
two batches earlier: adding a variant to a shared enum is not a local change,
and the honest response is to flag it rather than pretend the file was not
edited.

### Phase 2D — the Providers and Launch Profiles settings sections (four lines)

Contract: Given a user in the settings view, when they manage providers or
launch profiles, Glasshouse lets them add, edit, disable, duplicate and remove
those entries — while preserving: **no secret value is ever displayed**,
user-level defaults stay visually distinct from project-level overrides, a
project-level write still needs explicit confirmation, and disabling is
reversible without retyping anything.

State: **COMPLETE** for the four settings lines. Phase 9D line 426 (provider
connectivity testing) stays **open** — see below.

#### Line 426 stopped exactly where the packet told it to

Glasshouse has no HTTP client on the branch this batch was cut from, and the
packet forbade adding one because the concurrent gateway batch was introducing
`ureq`. So the "test" affordance is a **reachability precondition check** —
provider resolves, protocol declared, base URL non-empty, credential variable
present — named honestly in the UI as such, and the line is left unchecked.
`ureq` is now on `main`, so a follow-up can make it a real request.

#### The orchestrator's mutation found a weak test, not a weak mutation

Acceptance test 7 asked that no credential value ever render. The test plants a
real environment variable, drives nine settings screens and asserts `!contains`.
It looked thorough. **It passed a mutation that renders the credential's value
instead of `set`/`not set`.**

The reason is the finding: at the test's 100-column render the providers row is
**truncated**, so a leaked 46-character value was clipped off-screen. The test
was passing for a reason that had nothing to do with the code. Re-rendered at
400 columns with the same mutation, it fails.

Every snapshot in that test is now captured at **both** a realistic and a wide
size, and the mutation is caught. Verified in both directions: hardened test +
mutation → FAILED; hardened test + clean code → ok.

**The general lesson, which is new to this project's practice:** a test that
asserts the *absence* of a string in rendered output is only as strong as the
viewport it renders into. Truncation makes absence trivially true.

#### CI evidence

**CI `32911326442` green on Linux, macOS, Windows and lint** at `2bdd89f`,
with the three decisive tests confirmed by name in the **Windows** job's own
log: `no_credential_value_is_ever_rendered_across_every_settings_screen` (the
one the orchestrator's mutation hardened),
`an_unknown_harness_error_is_visible_not_clipped_by_a_wrapped_label`, and
`a_stale_reachability_result_does_not_shadow_a_later_profile_input`.

Worth noting for the render tests specifically: they assert over a fixed
viewport, so running them on a second platform is not redundant — it is the
only thing that shows the layout is not host-dependent.

#### Three defects found by running the binary, which is why the packet demands it

- **A stale test-result banner shadowed the profile wizard**, leaving it
  silently un-drivable. Fixed twice over so the two fixes do not depend on each
  other.
- **`cmux`, Ollama and llama.cpp were accepted as launch-profile harnesses**,
  because validation used `IntegrationId::ALL` rather than filtering to
  `IntegrationKind::Harness`.
- **A long refusal message rendered off-screen.** The input panel's height was a
  fixed `2`, and the harness hint wraps on a realistic width.
  `Paragraph::line_count` turned out to be gated behind an unstable upstream
  feature the packet forbade enabling, so the batch wrote a small tested
  word-wrap counter instead of guessing.

### Phase 9G — the Anthropic Messages ingress, and a credential the child never sees (ten lines)

Contract: Given a Claude Code session launched under a gateway-backed profile,
when the harness sends an Anthropic Messages request to the local gateway,
Glasshouse forwards it unmodified to the configured provider **with the
credential attached by the gateway**, streaming the response back
byte-for-byte — while preserving: the child process never receives the provider
credential; request and response bodies are never rewritten or logged; and a
provider error reaches the harness intact while its detail reaches diagnostics
with no foreign text at all.

State: **COMPLETE** for ten lines. Phase 9G is seventeen of nineteen.

#### Architecture, and why there is still no async runtime

Blocking threads, one per accepted connection. Glasshouse has no async runtime,
and adding one for a single-user loopback proxy would touch every module for a
capability that does not need it. Outbound is `ureq` 3.4.0 with
`default-features = false, features = ["rustls"]`; `Body::into_reader()` gives
an incremental `Read`, which is what makes pass-through streaming possible
without an executor. **+26 lock packages** (249 → 275), the unavoidable cost of
a TLS client.

#### The credential boundary — lines 2 and 3, and the point of the phase

The child harness is given `ANTHROPIC_AUTH_TOKEN` = **the gateway's own
per-instance token**, never the provider key. The gateway checks that token and
attaches the real credential itself, resolved through `SecretStore` and never
leaving the process. A request whose bearer is not this instance's is refused
**401 before any upstream connection is opened** — asserted on the fixture's
*connection count*, not on the status code, so "refused" means "nothing was
opened".

#### Mutations — 24 by the lead, 2 re-run independently by the orchestrator

23 caught immediately. The orchestrator re-ran the two highest-stakes against
the integrated tree: making the token comparison accept everything **failed four
tests** including the opens-nothing-upstream one; buffering the body before
writing **failed** `a_streamed_response_reaches_the_client_before_the_upstream_has_finished`.
The orchestrator additionally ran the gateway suite **20 more times: 0
failures**, on top of the lead's 40.

**The one that survived is the most useful result here.** Removing
`set_nonblocking(false)` from an accepted socket broke nothing, because every
test wrote its request *before* the gateway accepted, so the bytes were already
buffered. A real harness connects first and writes afterwards. A new test,
`a_client_that_connects_before_it_writes_is_still_served`, pauses past one
accept poll before writing; the mutation then fails with an empty response.

#### Two real defects found while building

- **The test fixture had the platform bug the production code documents** — a
  non-blocking listener whose accepted sockets inherit the flag on macOS. It
  reproduced twice in fifteen suite runs and looked exactly like a flaky network
  test.
- **Nagle's algorithm was stalling every streamed event.** The client socket had
  `TCP_NODELAY` off and the response head was written field by field, so a dozen
  tiny segments each waited on a delayed acknowledgement. That is a latency
  defect in precisely the property line 4 promises. Now `set_nodelay(true)`,
  with the head and each chunk written once.

#### `redact` is not enough for foreign text, and now nothing foreign is kept

The packet said to run foreign text through `crate::secret::redact` before it
reaches a diagnostic. **That is insufficient** and a test written to prove the
seam caught it: `redact` removes credential-*shaped* runs and says nothing about
the text around them. A captured log line read
`detail=Some("connect failed for Bearer [redacted] carrying PLANTED-PROMPT-BODY-DO-NOT-LOG")`
— credential gone, prompt body verbatim.

`transport_detail` now maps `ureq::Error`'s variant to one of eight phrases
written in that file and returns `&'static str`. A leak is no longer something
to be careful about; it is something the function **cannot express**. A source
scan refuses an owned `String` on `Outcome`, and a mutation proves it fires.

#### Six packet corrections, two of which are the orchestrator's to confirm

- **§6 contradicted §2, and §2 won.** §6 asked for a provider error's
  `error.type`/`error.message` in diagnostics; §2 forbids parsing the body and
  makes it a stop condition. Extracting either field *is* parsing. The
  diagnostic records status, provider and upstream host; the body goes to the
  harness, which is what needed to read it.
- **"Rewrite exactly three things" needed a fourth category.** A proxy
  terminates one connection and opens another, so connection framing cannot
  survive: `content-length` is re-stated, `transfer-encoding` re-applied, and
  RFC 9110 §7.6.1 hop-by-hop headers are not forwarded. Forwarding them is a
  defect, not fidelity — a mutation shows the upstream dying on two
  `content-length` headers. "Byte-for-byte" remains exactly true of the method,
  the target, every end-to-end header and every body byte.
- **`GlasshouseGateway` names no provider.** `profile::gateway_upstream` takes
  the single configured provider serving the ingress protocol and **refuses on
  zero or several, naming the candidates** — because choosing a backend per
  session is Phase 9H's sticky routing, not a launch profile's decision.
  **Orchestrator confirms this**: refusing ambiguity is what `session::select`,
  the resume resolver and `native_id` all already do.
- **`Resolution` could not gain a `gateway` field** — `config/mod.rs` was
  forbidden (a concurrent worker owned it) and builds two `Resolution` literals
  in its tests. `resolve` keeps its signature and delegates to
  `resolve_with_gateway`. **Follow-up:** fold the field in once `config/mod.rs`
  is free; it is a two-line change to those literals.
- **Constant-time comparison is hand-rolled and says so.** Safe Rust cannot
  promise constant time; `subtle` is in the lock transitively via `rustls` but
  promoting it is a new direct dependency the packet forbade. Disclosed in the
  function's own doc rather than claimed as equivalent.
- **A `HEAD` response now carries no body**, whatever its status says. Not in
  the packet, not reachable from any harness in scope, but the method is
  forwarded rather than vetted.

#### An ordering change worth knowing

The gateway used to start *after* `profile::resolve`. A gateway-backed profile
now resolves into *this gateway's* address and token, so it must start first.
Nothing binds and no credential resolves for a launch that needs no gateway —
the upstream is a closure, and
`no_profile_needing_a_gateway_binds_no_listener_and_resolves_no_credential`
asserts it is never called.

### Phase 9 — the Antigravity conversation identifier, from an index rather than a walk (lines 2 and 3)

Contract: Given a Glasshouse-started Antigravity session that has just ended,
when Glasshouse looks for its native conversation identifier, it reads **one
shared index file** keyed by project path and records the identifier only if
that project's entry both **changed** during the session and the index's mtime
sits inside the session's window — while preserving: **no conversation database
is ever opened**, an absent or unchanged entry records nothing, and a resume
only ever passes an identifier Glasshouse recorded itself.

State: **COMPLETE** for both lines. Phase 9 is five of seven.

#### Why this needed a new shape at all

`session::native_id::discover` was built for one shape: a directory of session
records, each self-describing in its own first line — it walks, filters by name,
and **opens every survivor**. Antigravity does not have that shape. Its
identifier lives in `~/.gemini/antigravity-cli/cache/last_conversations.json`,
a flat `{project path: uuid}` map with **no timestamps**, and its records are
`conversations/<uuid>.db` — SQLite databases holding the user's private
conversations. A previous packet asked for `session_id_source` to be pointed at
those databases; the worker refused and was right.

So `NativeSessionSource` is now an enum: `RecordPerSession` (Codex's walk, byte
for byte unchanged) and `SharedIndex`, paired with a new pure adapter method
`read_index_entry`. The `SharedIndex` path reads **exactly one named file** and
never calls the directory walk — a property of the code path, not a rule anyone
has to remember, and
`the_shared_index_code_path_never_mentions_the_directory_walk` enforces it.

#### The identity guard is two rules, and both are load-bearing

A shared index has no per-entry timestamp, so the window has to come from
elsewhere:

1. the **index file's own mtime** must fall inside `[started_at, ended_at]`;
2. the entry for this project must have **changed** during the session.

Rule 1 alone is not enough and the hole is worth naming: the mtime moves when
*any* project's entry changes, so an Antigravity session in another project
during our window could make a stale entry for ours look fresh. Rule 2 closes
it, because a stale entry is by definition unchanged. Its one false negative —
resuming the same conversation leaves the entry unchanged — is safe, because
Glasshouse only ever resumes an identifier it already holds.

#### Mutations — by the lead, plus two re-run independently by the orchestrator

- removing the changed-entry guard → `a_shared_index_entry_that_did_not_change_is_never_captured` **FAILED** (killed);
- making the shared-index path walk a directory →
  `the_shared_index_code_path_never_mentions_the_directory_walk` **FAILED** (killed).

Line 3 was additionally proved by hand against the built binary in a real PTY
before any test existed: a launched session listed as `resumable`, and
`glasshouse resume <short>` producing `--conversation <id>`.

#### CI evidence — and two red Windows runs on the way to it

**CI `32908006880` green on Linux, macOS, Windows and lint** at `63e0053`, with
the two decisive tests confirmed **by name in the Windows job's own log**:
`the_shared_index_code_path_never_mentions_the_directory_walk` (the guard that
the shared-index path can never open a conversation database) and
`a_shared_index_entry_that_did_not_change_is_never_captured` (the stale-entry
rule).

Getting there cost two red Windows runs, and neither was a product defect:

1. The scan located a function body by searching for a literal
   newline-brace-newline. `include_str!` reads the file exactly as checked out,
   and where Git converts line endings that literal is absent — so the guard
   **panicked** instead of asserting. Now scanned with `str::lines`, which
   strips the carriage return, making it CRLF-agnostic by construction.
2. The regression guard written for (1) built its CRLF copy with
   `SOURCE.replace('\n', "\r\n")` — but on Windows `SOURCE` is *already*
   CRLF, so that produced `\r\r\n` and `lines` strips only one. **The guard
   depended on the checkout it was guarding against.** Both copies now come
   from a normalised base.

The second fix was verified locally rather than on a third round-trip: the file
was converted to real CRLF, the suite run, the pre-fix guard restored under the
same CRLF to reproduce CI's failure with the **identical assertion message**,
and the file restored. That recipe is now practice §15.

#### `home_env` is `None`, and that is a finding

The design left the variable name open ("`GEMINI_DIR` or whatever agy
honours"). The lead searched the 1.1.20 binary for `GEMINI_DIR`, `GEMINI_HOME`,
`ANTIGRAVITY_HOME`, `AGY_HOME`, every `XDG_*` and every `*_HOME`/`*_DIR`
symbol: **Antigravity honours no environment variable for its state root.** So
`home_env` became `Option<&'static str>` — `Some("CODEX_HOME")` for Codex,
`None` here. Declaring `"GEMINI_DIR"` would have been a fifth invented
declaration in a module whose own doc already records two.

#### An orchestrator design rule that was too broad, corrected

The design said "no log line, no diagnostic" for a conversation identifier.
Two **pre-existing** log lines carry one (`native_id::capture`'s success log and
`resume_session`), and the second has a comment deliberately arguing the
identifier is the one fact that makes a failed resume diagnosable. The lead
reported the collision rather than choosing.

**Orchestrator's decision: the log lines stay.** The identifier is not a
credential — it grants no access and names local state Glasshouse already
records in its own database. The real property is narrower and is what the rule
should have said: *never log the index's contents, and never log an identifier
belonging to another project.* Both hold.

### Phase 2C — first-run onboarding, and the acknowledgement `setup` had been promising (six lines, plus a 9A gap closed)

Contract: Given a first run, when the user reaches the provider step,
Glasshouse offers provider configuration as an **optional** step with a clear
"Configure now" and an equally clear "Do later" — while preserving: "Do later"
completes onboarding with no API key of any kind and leaves a Glasshouse that
works against native subscription-backed harnesses; cmux is offered only when
detected or explicitly asked for; and reopening the wizard preserves prior
choices.

State: **COMPLETE** for the six lines. The four routing-model lines stay
unchecked — each needs a routing-model configuration field, and `config/mod.rs`
was owned by a concurrent worker.

#### The gap this batch closed, which was a promise the product did not keep

`profile::Refusal::BypassNotAcknowledged` told users, verbatim, to "acknowledge
the risk once (in `glasshouse setup`)" — and **`setup` had no such step**;
`grep -rn "bypass" src/onboarding/ src/shell/` returned nothing. Phase 9A's
resolution half was built and its human half never was. That is the fifth time
on this project a declaration has not matched its use.

The step now lists the harnesses that declare a bypass and **no**
automatic-review mode — derived from the adapters
(`approvals.automatic_review` unverified, `approvals.bypass` present), never a
hard-coded list, so it cannot rot when an adapter changes. It shows each
harness's **own declared argv and description**, defaults to not acknowledged,
and writes to the **user layer only**.

#### Regression evidence

`only_a_harness_with_a_bypass_and_no_automatic_review_is_offered_bypass_acknowledgement`,
`declining_leaves_bypass_acknowledged_unset_and_the_profile_still_refused`,
`the_bypass_step_is_skippable_and_onboarding_completes_without_it`, plus the
provider-step, cmux-detection and reopen-preserves-choices tests.

#### A survived mutation that was the mutation's fault, twice

The orchestrator's own non-vacuity check on the security-relevant line took
three attempts, and the practice file's rule held exactly:

- seeding the row from `Some(true)` instead of from config — **survived**; the
  decline test does not depend on the seed.
- forcing `set_bypass_acknowledged(true)` *inside* the `row.acknowledged !=
  row.seeded` guard — **survived**, because on a decline from a fresh config
  that guard is false and the write is unreachable.
- removing the guard and acknowledging **every offered harness** — **KILLED**,
  by two tests.

So silence genuinely cannot become consent. *"A `SURVIVED` mutation is more
often a weak mutation than a weak test"* — rewritten twice before the code was
doubted, and the code was right both times.

#### Judgement the worker got right against its own packet

It **did not build a gateway configuration screen.** `BackendResource::GlasshouseGateway`
is still refused by `profile::resolve`, so a gateway step would have been a
button leading nowhere. The Provider step configures providers, and the Summary
says so plainly. It also flagged that the module's own "out of scope" doc was
stale — providers had been built by 9C/9D/9F since it was written.

---

### Phase 9C — protocol compatibility as a filter, and Phase 9C at twelve of twelve

Line, quoted exactly: "Treat protocol compatibility as a hard routing constraint
before model-quality scoring."

Contract: Given a set of configured providers, when Glasshouse selects one to
route over, an incompatible provider is **removed from the candidate set** and
never merely ranked lower — while preserving: a declared protocol with no base
URL is not compatibility, an empty candidate set is a refusal naming what was
required and what was served, and no model-quality scorer is invented here.

State: **COMPLETE.** Phase 9C is **twelve of twelve**.

#### Most of it already held, and the gap was the seam

`Provider::serves` was already protocol-exact, `choose_protocol` already refused
a harness/provider pair sharing no protocol, and `gateway_upstream` already
discarded providers with no base URL. What was missing is that the gateway's
filter was a local `Vec<&Provider>` and the direct-provider chooser picked a
protocol *before* checking it had a destination.

`ProtocolCompatibleProviders` now sits in front of both. Its fields are private
and only its two constructors produce candidates, each requiring an exact
protocol declaration **and** a non-empty base URL.

**The ordering is enforced by the type, not by a convention.** A future
model-quality scorer has to accept the wrapper rather than a provider slice, so
there is no unfiltered set for it to rank — passing raw providers does not
compile. That is what "hard constraint … before scoring" has to mean to survive
a phase that has not been written yet. No production scorer was added; Phases 33
and 34 own that.

#### Evidence quality

Three mutations by the worker, each killed with the named test's own result
line: accept a declaration with no base URL; accept any provider with a URL
regardless of protocol; replace the empty-set refusal's detail. The worker
correctly distinguished a killed mutation that emitted an unused-helper warning
from a mutation that failed to compile.

**The worker ran under a sandbox that blocks loopback bind and Keychain**, so 30
tests failed on permissions alone. It enumerated all 30 by name so the set could
be checked rather than trusted, and added no workaround. Unsandboxed: **779
passing, 0 failing**, exactly the total it predicted.

#### An orchestrator error, recorded because the recovery is the useful part

While probing the type boundary, the orchestrator appended a test to the
worker's `provider/mod.rs` and then ran `git checkout --` on that file to undo
it. **That deleted all 161 lines of the worker's work**, because a worker's
deliverable exists only as uncommitted changes and git cannot tell whose edit is
whose.

The worker's session was still live, so it was asked to re-create the file from
its own record; the first attempt came back at +93 lines and 778 tests against a
predicted 779, the shortfall was pointed out, and the second attempt restored it
exactly at +161 and 779. **The test-count discrepancy is what caught the
incomplete restore** — without a predicted number to check against, a
plausible-looking partial restoration would have been committed.

The rule was already written down and was broken anyway, so it is now enforced
by a `PreToolUse` hook rather than documented: `scripts/hooks/guard-destructive-git.sh`
refuses `git checkout` with a path, `git restore`, `git stash` and `git clean`,
and points at the `cp` backup that restores *your change* instead of *the file*.

---

### Phase 9B — the child's environment, and Phase 9B at nine of nine

Line, quoted exactly: "Preserve the user's existing shell environment except for
explicit launch-profile overrides."

Contract: Given a user with an existing shell environment, when Glasshouse
launches a harness under a launch profile, the child sees the user's environment
plus exactly the profile's declared overrides and nothing else — while
preserving: no variable the user set is dropped, none is altered that the profile
did not name, and nothing outside the spawned process tree is touched.

State: **COMPLETE.** Phase 9B is **nine of nine**.

**The line did not already hold**, which the packet had explicitly allowed for.
Two production behaviours broke it, both at the common PTY boundary:

1. `TerminalCommand::new` recorded a `TERM` override unconditionally, changing
   an unset, empty or `dumb` value even though no profile named `TERM`.
2. **`portable-pty` 0.9.0 rewrites the environment it was given.**
   `CommandBuilder::new` starts from `std::env::vars_os()`, but its Windows
   branch then merges registry-composed system and user values over that map —
   including replacing `PATH`. On Unix it adds `SHELL` when the parent had none.

The second is the find worth keeping. **A pre-existing smoke test had already
observed that Windows `PATH` mismatch and responded by compiling its inheritance
assertion out on Windows** — a known-wrong case papered over rather than fixed.
`into_builder` now calls `env_clear()`, copies an exact snapshot of Glasshouse's
own environment, and layers only the recorded overrides and removals on top. The
two skipped assertions now run on Windows.

**On removing the `TERM` fallback.** Its doc justified it by "Glasshouse itself
was started from a context without a terminal" — but `session::attach` **refuses
outright** unless both stdin and stdout are terminals, with a message telling the
user to run from an interactive terminal. The motivating case cannot reach a
harness launch, so the justification was stale. Recorded here so nobody restores
it on the strength of that comment. A user who deliberately sets `TERM=dumb` now
keeps it, which is what the line asks for.

#### Evidence quality

Three mutations by the worker, each killed, each reported with the named test's
own result line: drop the parent snapshot; drop the profile overrides; add an
unconditional `TERM`. The orchestrator ran a fourth, independent of that set —
**apply the parent snapshot *after* the overrides, so the user's value wins over
the profile's** — killed by the same test.

**The worker could not run the full suite and said so rather than working around
it.** Codex ran under `-s workspace-write -a never`, which denies loopback bind
and Keychain access; 27 gateway and 3 secret tests failed on that alone. It
reported the exact failure count, identified the cause, and stated that no
production or test workaround had been added for infrastructure restrictions.
**The orchestrator ran the suite unsandboxed: 777 passing, 0 failing**, which
confirms all 30 were sandbox artifacts.

#### First batch run on the Codex harness

Model `gpt-5.6-sol` at `xhigh`, to match the effort the Claude Code workers run
at. Notes for whoever runs the next one:

- **Model identifiers need the full prefix.** `sol` is rejected on a ChatGPT
  subscription account; it is `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna`.
- **Codex needs no bypass shim.** `-s workspace-write -a never` is a real
  automatic-review mode, unlike Antigravity's blanket bypass — the same
  distinction Glasshouse's own adapters record.
- **That sandbox blocks loopback and Keychain**, so the orchestrator must run
  the full suite for any Codex batch.
- **It also blocks writing outside the worktree**, so the report path in the main
  checkout was refused. The worker wrote to `/tmp` and said so. Future Codex
  packets should put the report inside the worktree.

---

### Phase 2C — the routing-model step, and Phase 2C at nineteen of nineteen

Lines, quoted exactly:

- "Offer routing-model configuration as an optional first-run step after
  providers have been detected or configured."
- "Offer an Automatic routing-model choice that selects the cheapest
  sufficiently fast configured resource."
- "Offer a Choose model routing-model choice for users who want to pin
  classification to a specific model."
- "Offer a Do later choice for routing-model configuration and use
  deterministic routing heuristics until configured."

Contract: Given a user finishing first-run setup, when they reach the
routing-model step, Glasshouse offers exactly three choices — Automatic, Choose
model, Do later — records the one they picked and proceeds, while preserving:
declining is a first-class outcome that leaves a working system on deterministic
heuristics; the choice is stored as a reference, never a credential; and a
configuration naming a model that later disappears degrades to heuristics rather
than failing to start.

State: **COMPLETE.** Phase 2C is **nineteen of nineteen**.

#### Three states per layer, not two

`RoutingConfig` holds `Option<RoutingModelChoice>`, and the `Option` is load
bearing. Layering needs three states per layer — "this layer says automatic",
"this layer says deterministic", and "this layer says nothing, ask the next
one". Collapsing `None` into `Deterministic` would make a project that wants
deterministic-only classification *over* a user-level `Automatic`
inexpressible, which is exactly Phase 2D's third routing option. The same shape
`IntegrationConfig::executable` already uses.

It also buys the literal reading of "Do later": with `None` skipped on
serialise, a first run that declines writes **no `[routing]` table at all** —
verified against the shipped binary, not only in a unit test.

**`Automatic` carries no payload.** Every filter Phase 34C applies to that
selection is a live condition — provider health, RPM headroom, latency, marginal
cost — and 34C is required to re-evaluate it when a provider degrades.
Resolving a winner during a first-run wizard and writing it down would freeze a
decision the map explicitly wants re-evaluated, so `Automatic` stores the intent
and 34C resolves it.

#### The defect the binary found, which every rendering test missed

Seeded with a pin whose provider no longer exists, at **80x24**, on a machine
with ten harnesses installed and two of them under a ~90-character macOS
temporary path, the Summary screen rendered **without the `Routing model:` line,
its degrade explanation, or the gateway note at all.** `render_summary` draws a
wrapped `Paragraph`, which has no scrollback: content past the bottom edge is
simply not drawn and nothing says so. Ten long executable paths, each wrapping
onto three rows, pushed everything below them off the screen.

**Every rendering test passed throughout, because every fixture used
`/usr/bin/claude`.** The variable that broke it — how long an installed
harness's path happens to be — is set by the user's machine, not by any fixture.
This is practice §17 in its sharpest form yet: an absence assertion is bounded
by the viewport, and a presence assertion is bounded by the *content* it shares
that viewport with.

Fixed at the cause: each integration is bounded to exactly one row, eliding the
path from the left so the executable's own name survives, and one separator was
reclaimed. `every_summary_section_survives_the_worst_case_at_80x24` renders that
worst case and asserts every section is present *and* that each integration
occupies one row. Two mutations put each half of the fix back; both are killed.

**A constraint is handed forward rather than left as a trap:** the Summary now
has **zero** spare rows at 80x24 in that worst case. The next batch that adds a
line to it will fail that test. That is the screen saying it is full, and the
answer is real scrolling — not deleting the assertion. The worker correctly
declined to build the scrolling as outside its packet.

#### Evidence quality

Seventeen mutations designed, run and killed; none survived. Gates run from a
deleted `target/` rather than a warm one. 734 → 760 tests on the batch's own
branch, 776 with the concurrent Phase 9G batch merged.

The worker reported **one red run it could not account for** — a single lib
failure whose name it lost by not redirecting that run's output — and recorded
it rather than burying it, alongside sixteen subsequent green runs and four
concurrent copies of the lib binary run to force port contention. A
subcontractor independently captured `AddrInUse` in
`gateway::tests::dropping_the_gateway_releases_its_port`, and the worker
explicitly declined to claim that was the same failure. **That test is tracked
as flaky**; a suite with an unexplained red is worth less than its pass count
suggests.

---

### Phase 9G — the last two ingresses, and Phase 9G at nineteen of nineteen

Lines, quoted exactly:

- "Expose an OpenAI Responses-compatible ingress for gateway-backed Codex
  profiles when implemented."
- "Expose an OpenAI Chat-compatible ingress for compatible disposable jobs and
  harnesses when implemented."

Contract: Given a gateway-backed profile whose harness speaks OpenAI Responses
or OpenAI Chat, when that harness sends a request to the local gateway,
Glasshouse forwards it to the base URL the configured provider declared **for
that protocol** and streams the response back unmodified — while preserving:
the provider credential never reaches the child, a target belonging to no
served protocol is refused rather than blindly appended, and the Anthropic
Messages ingress behaves exactly as before.

State: **COMPLETE.** Phase 9G is **nineteen of nineteen**.

#### One upstream, several routes — and the reason is the secret boundary

The gateway holds **one `Upstream`: one provider, one `Secret`, and one route
per protocol** — not a set of `Upstream`s keyed by protocol. The deciding
argument is not aesthetic. `crate::secret::Secret` is deliberately not `Clone`
and cannot be minted outside its own module, so a set of upstreams would need
either a widened `Secret` API or the same credential resolved once per
protocol. That would turn "the credential lives here and nowhere else" — the
sentence that module exists to make true — into "it lives in three places that
happen to agree". One owner with several destinations keeps it literally true,
and `every_route_forwards_with_the_one_credential_the_upstream_holds` asserts
it.

A route carries its protocol as a **slug**, not a `WireProtocol`, because
`gateway/` is structurally forbidden from naming `crate::harness` and a test
enforces that. So `crate::profile` owns the table of which paths belong to which
protocol, and `gateway` owns the matching — the same division that already put
`GATEWAY_INGRESS_PROTOCOLS` in `profile`.

**Matching drops the query, strips a leading `/v1` segment, then matches a
declared prefix at a path-segment boundary.** The target is still appended to
the base URL byte for byte; the stripping affects classification only.

#### The packet was wrong about the path, and it was load bearing

The packet said `/v1/responses` → OpenAI Responses. Against a real listener,
**Codex 0.149.1 pointed at a base URL with no path sends `POST /responses`** —
and a base URL with no path is the only kind `Gateway::base_url()` hands out.
Whether the `/v1` segment appears is a property of the harness's configured base
URL, not of the protocol. Taking the packet literally would have shipped an
ingress no gateway-backed Codex could reach.

Codex's `wire_api` was re-verified against the installed binary rather than
assumed: `wire_api = "chat"` is refused with "no longer supported", `"responses"`
starts a session. So this ingress really is the only gateway path that can ever
back a Codex profile.

#### End-to-end, against a real provider

One gateway, one provider, two harnesses, two protocols, from one run's log:

    outcome="forwarded" status=200 provider=openrouter protocol=Some("openai-responses")
    outcome="forwarded" status=402 provider=openrouter protocol=Some("anthropic-messages")

`glasshouse launch codex` reached OpenRouter's Responses endpoint and the model
answered, 550 KB streamed back through the gateway. `glasshouse launch
claude-code` over the **same** gateway routed to Anthropic Messages, and the
provider's own `402 Insufficient credits` reached the harness verbatim — a
billing answer, not a routing one, which is the pass-through the phase requires.
Against a recording listener the forwarded request carried exactly one
`authorization` header, the provider's, with the child's gateway token gone.
`grep -c` for the key across three real gateway logs: `0`, `0`, `0`.

#### The honest ceiling on the Chat ingress

**No adapter in this crate declares `OpenAiChat`.** Claude Code is
Anthropic-only, Codex is Responses-only, and every other adapter's protocol
support is `Declared::Unverified`. The Chat ingress is therefore proven at the
socket level — through the real gateway, real TCP, real forwarding — and **not**
through a real harness client, because none exists to run. That is the ceiling
for "compatible disposable jobs and harnesses" until Phase 39's disposable jobs
are built, and it is stated here rather than implied.

#### Evidence quality

Eight mutations, every one killed, each read from the named test's own result
line. They cover route selection, the `/v1` strip, segment-boundary matching,
refusing against the *running* gateway rather than the static constant, `Debug`
on `Upstream`, per-protocol route construction, streaming, and the
unplaceable-target fallback.

The orchestrator ran a ninth, independent of the worker's set, on the property
that matters most: **forward the child's own gateway token upstream as well as
the provider's.** Killed by four tests, two of them the worker's new
conformance tests.

A subcontractor found a real defect in the lead's own path and correctly
declined to fix it: `refuse()` wrote a JSON body regardless of method, and the
new `404` is the first response in this gateway's life that a `HEAD` can
reach — Claude Code 2.1.245 sends `HEAD /api/hello`. A body there is not
harmless; a client reads the declared length, finds bytes it was told would not
be there, and takes them for the next response.

#### Two things recorded, not fixed

- **Codex's `GET /models` is refused**, and Codex logs that at `ERROR` twice per
  session. The session completes normally — the live run above contains exactly
  those two refusals and still returned its answer. It was not routed because
  `/models` is a catalogue endpoint **all three protocols define**, and placing
  it means choosing a protocol for a request that names none. Two already-checked
  map lines forbid inventing that tie-break before a concrete pair requires it.
  Tracked as follow-up work with its own evidence, not folded into this review.
- **`config/mod.rs` applies one `base_url` override to every protocol a
  provider serves.** With `openrouter` now serving three protocols across two
  URLs, a single override silently collapses them. Found by a subcontractor,
  outside the batch's file ownership, reported rather than touched.

### Phase 9G — the local gateway process (seven of nineteen)

Contract: Given a Glasshouse instance whose active launch profiles include one
backed by the Glasshouse gateway, when that instance starts, Glasshouse binds a
listener **on loopback only, on an ephemeral port**, mints a fresh per-instance
authentication token, and tears all of it down when the instance exits — while
preserving: no listener exists at all when no profile needs one; two instances
never contend for a port; the token never reaches a log, a `Debug`, or a file;
and the gateway never owns a session or becomes a harness.

State: **COMPLETE for the seven process lines.** Every ingress line, streaming,
tool-call payloads, error mapping and the two credential-holding lines are a
later slice and remain unchecked.

#### Production evidence

- `gateway/mod.rs` (new, ~300 production lines) — `Gateway`, bound to
  `Ipv4Addr::LOCALHOST` on port `0` with the real port read back via
  `local_addr`; `GatewayToken`, 32 bytes of `getrandom` rendered as hex; and
  `gateway_is_required(&[LaunchProfile])`, which reads `BackendResource`.
- `main.rs` — `start_if_required` is called from `launch_session`, placed
  **after** profile resolution so a refused launch still costs nothing.
- **Line 2 is structural, not promised.** The module imports none of
  `crate::session`, `crate::shell`, `crate::tui`, `crate::harness`, enforced by
  a source scan with a paired positive/negative test. A module that cannot see
  the session model cannot own a session.

#### Regression evidence — 13 tests, none `#[cfg]`-gated

Loopback and non-zero port; two gateways binding different ports; two gateways
minting different tokens; `Debug` rendering `crate::secret::REDACTED` (the
shared constant, not a second one); **no listener bound at all** when no
profile needs one, asserted on absence rather than a boolean; a listener bound
when one does; the port released on drop, proved by rebinding it; and the
import scan with its vacuity twin.

#### Mutations — 10 by the lead, 2 re-run independently by the orchestrator

All killed. The orchestrator re-ran the two security-critical ones against the
integrated tree, restoring from a byte-compared backup: binding
`Ipv4Addr::UNSPECIFIED` instead of loopback killed the loopback test, and
making `Debug` print the token killed **two** independent tests. The
orchestrator additionally ran the gateway suite **40 times: 0 failures**,
because this batch's own report disclosed a flake it had found and fixed.

#### Four packet corrections, and the orchestrator's packet was wrong in one of them

- **The packet's §2 was factually impossible.** It said "a connection that
  arrives is closed immediately". With no `accept` call the *kernel* completes
  the handshake into the listen backlog, so `connect` **succeeds** and the
  connection simply sits there. Making the packet's sentence true would need an
  accept loop, which the same paragraph forbids. The lead measured the real
  behaviour and asserted **that** instead: `connect` succeeds, and the gateway
  never sends a byte — checked by reading *after* the drop, since bytes written
  before a close survive in the receiving buffer, which catches a gateway that
  greeted its client without needing a sleep.
- **A latent hazard in existing code.** `shutdown`'s `FORCED_EXIT_CLEANUP` is a
  single `Mutex<Option<_>>` slot: `on_forced_exit` *overwrites* it and the
  guard's `Drop` sets it to `None`. Registering a gateway cleanup there would
  have displaced the harness-kill callback an attached session installs, and
  dropping the gateway would have unregistered the session's callback —
  orphaning a real harness on a second Ctrl-C. It is harmless today only
  because there is exactly one caller. **The next slice that adds a second
  caller must fix that API.** The gateway uses RAII instead, like the three
  existing guards.
- **A `GatewayToken` cannot be a `Secret`.** `Secret`'s field is private to its
  module and the only other constructor is `#[cfg(test)]`, so a sibling module
  cannot mint one in production. The token mirrors `Secret` item for item and
  carries the same source-scan test. See the design decision recorded below.
- **A random-input assertion is a flake generator.** The first `Debug` test
  scanned prefixes of a *generated* 64-hex-character token against the
  rendering, and `[redacted]` contains four of the sixteen hex digits — a
  one-character prefix "leaked" whenever the token began with one. Measured at
  **45 failures in 100 runs** and fixed the way `secret`'s twin does it, with a
  fixed non-hex stand-in.

#### Two gaps, stated rather than papered over

- **The `main.rs` wiring has no test that would fail if the call were deleted**,
  because the only profile that would bind a listener is refused by
  `profile::resolve` two statements earlier — 9F's refusal, which this packet
  forbade touching. The *predicate* is mutation-proven; the *wiring* becomes
  testable the moment the ingress slice lifts that refusal, and that slice must
  add the test.
- **`resume_session` resolves no launch profile at all**, so a resumed session
  cannot require a gateway even in principle. The session record does carry
  `launch_profile` and `backend_resource`, so reconstructing one is possible —
  a design decision for the ingress slice.

#### The dependency, audited

`getrandom = "0.4"` adds **no package** — one line in the lock, 249 `[[package]]`
stanzas before and after, since it was already in the graph via `tempfile`. It
needs no feature flags and selects a backend unconditionally on macOS, Linux
and Windows. It declares `rust-version = "1.85"`, **exactly** the workspace
MSRV with no headroom, so a future `cargo update` into a 0.4.x that bumps to
1.86 would break the MSRV gate; `--locked` in CI holds that off until the lock
is deliberately refreshed. It moves from dev-only into the shipped binary; its
only non-dev dependency is `libc`, already present.

### Phase 9D/9A — provider templates, header overrides, and the first gateway a harness can actually reach (five lines)

Contract: Given a provider configured from a built-in template, when the user
overrides its base URL or adds custom headers, Glasshouse launches the harness
against the overridden endpoint with those headers applied to that child
process only — while preserving: a template's own defaults when nothing is
overridden; header names and values being configuration rather than secrets and
never invented; a provider whose endpoint nobody established never gaining a
built-in template; and a header value that could forge a second header being
refused rather than escaped.

State: **COMPLETE** for map lines 415, 416, 423 (Phase 9D) and 353, 355
(Phase 9A).

#### The endpoints, established from the vendors' own documentation

- **NVIDIA** — `https://integrate.api.nvidia.com/v1`, `openai-chat` **only**.
  `docs.api.nvidia.com/nim/reference/llm-apis` gives the base with
  `POST /v1/chat/completions`; NVIDIA's own `build.nvidia.com` samples use that
  exact `base_url` and read `api_key = "$NVIDIA_API_KEY"`. No Responses
  endpoint was established, so none is declared — and the honest consequence,
  asserted by a test, is that **this template cannot back Codex**.
- **LiteLLM** — `http://0.0.0.0:4000`, written as read from LiteLLM's own
  quick-start and `proxy/user_keys` pages rather than "corrected" to
  `localhost`. `GET /models` is documented, so its model-list endpoint is the
  only capability declared `Verified`. `credential_env` is deliberately
  **empty**: LiteLLM documents no dedicated variable and its examples reuse the
  generic `OPENAI_API_KEY`, which Glasshouse must not read for a local proxy.
- **OpenRouter also serves Anthropic Messages, at `https://openrouter.ai/api`**
  — the root, with no `/v1`, because Claude Code appends `/v1/messages` itself.
  Established two independent ways: an unauthenticated `POST` to
  `/v1/messages` answers **401** while a nonexistent path under the same prefix
  answers **404** (the control case is what makes it a discrimination rather
  than a guess), and the user's own working launcher drives the real Claude
  Code against exactly that root, stripping `/v1` with a comment explaining
  that keeping it yields `/api/v1/v1/messages` and a 404.

That third one is **line 353** ("Allow additional launch profiles such as
Claude / OpenRouter"): a Claude Code profile backed by a configured OpenRouter
provider now resolves to `ANTHROPIC_BASE_URL=https://openrouter.ai/api`, and a
test asserts the absence of the `/v1` suffix with the reason in its message.

#### Header overrides — line 423, with both mechanisms verified off the wire

- **Claude Code 2.1.245**: `ANTHROPIC_CUSTOM_HEADERS`, `Name: value` lines
  joined by a newline. Probed with two headers; both arrived.
- **Codex 0.149.1**: `-c 'model_providers.<id>.http_headers={ "N" = "V" }'`,
  accepted under `--strict-config` and delivered.

**Which is why the CR/LF refusal is a security rule, not hygiene.** A newline
inside a header *value* would forge a second header into every request.
`unsafe_header_value_char` refuses control characters outright rather than
escaping them, and `a_header_carrying_crlf_is_refused_rather_than_escaped`
pins it. Header *names* are restricted to `[A-Za-z0-9-]`.

#### Line 355 — environment injection, finally end to end

Line 355 stayed open through 9A for a recorded reason: no shipped profile could
populate `env`, so the only test drove `LaunchOverlay::apply` with a hand-built
overlay. Phase 9F changed that, and this batch closes the chain:
`tests/pty_smoke.rs::a_direct_provider_profile_reaches_a_real_child_and_only_that_child`
resolves a **direct-provider** profile, applies the overlay, spawns a real
env-dumping child, and asserts the base URL and credential arrive in the child,
that the parent's own environment does **not** carry them, and that `PATH` —
which no launch names — is unchanged. The credential is asserted by comparison;
its failure message reads "value withheld" and never the value.

#### Mutations — 13 by the worker, 2 re-run independently by the orchestrator

All 13 killed. The orchestrator independently re-ran two against the integrated
tree, restoring each file from a byte-compared backup:

- header value validation disabled →
  `a_header_carrying_crlf_is_refused_rather_than_escaped` **FAILED** (killed).
- the OpenRouter Anthropic root given a trailing `/v1` → killed **two**
  independent tests at different layers,
  `openrouter_also_serves_anthropic_messages_at_the_api_root_with_no_v1` and
  `a_configured_openrouter_provider_backs_claude_code_at_the_v1_less_api_root`.

#### Two forbidden-file findings, both correct

- **`Provider` gaining a field forces every exhaustive struct literal to
  change**, including one inside `secret/mod.rs`'s tests, which the packet had
  forbidden. There is no way to add the field without it. The worker made the
  one-line mechanical addition and flagged it instead of doing it silently.
- **The batch's own design change broke an unrelated pre-existing test.**
  `the_doctor_report_shows_a_configured_providers_protocol_and_base_url` scanned
  a provider's block with a hard-coded `.take(5)`, sized for a
  one-protocol world. OpenRouter's second protocol makes the block seven lines,
  so the credential-env assertion started failing. Replaced with
  `.take_while(|line| !line.trim().is_empty())`, which is correct for any
  number of protocols. Not a defect in the doctor report, which already loops
  generically.

#### A known, bounded inconsistency

Header validation runs in `config::to_provider` (the boundary where untrusted
input enters), while credential-variable validation runs at *resolve* time in
`profile::resolve`. The worker flagged the asymmetry rather than quietly
picking one. It is bounded rather than a hole: the only production constructors
of a `Provider` are `to_provider`, which validates, and `templates()`, which
`every_built_in_template_ships_no_header_unless_one_was_established` pins to
carry no headers at all. **If a third production constructor is ever added,
header validation must move to resolve time as well.**

### Phase 9F — direct provider launch profiles (eleven of thirteen)

Contract: Given a launch profile whose backend is a configured direct
provider, when Glasshouse starts a session for a harness that declares how to
be pointed at that provider, Glasshouse composes an ephemeral child-process
overlay carrying the provider's base URL, model and credential to that one
child process only — while preserving: the user's native harness
authentication and global configuration are never modified and never used as a
silent fallback; the credential value never reaches a log, a `Debug`, a
diagnostic, a mechanism note or a session record; and any combination the
adapter does not declare, or a credential that cannot be resolved, is refused
with an error that names what was asked for and starts nothing.

State: **COMPLETE for eleven lines.** Two are deliberately out of scope and
unchecked — the cheap pre-flight capability check, and requiring an installed
executable before *offering* a profile. Both are a pre-flight/offering concern
rather than a resolution one, and neither has a production path yet.

#### What the real binaries actually do — probed, not recalled

Every mechanism below was established by pointing the installed harness at a
local HTTP capture server and reading what it sent. This is the cheapest form
of the rule this project keeps re-learning: *check a declaration against the
use, not the claim.*

- **Claude Code 2.1.245.** `ANTHROPIC_BASE_URL=http://127.0.0.1:8731` produced
  `POST /v1/messages?beta=true` at that host — **the variable is the root and
  the harness appends `/v1/messages` itself**, so a provider's declared base
  URL goes through verbatim. `ANTHROPIC_AUTH_TOKEN` arrived as
  `authorization: Bearer <value>`, exactly the injected value, with **no
  `x-api-key` and without the user's own claude.ai credential**; the harness
  announced the precedence itself ("another auth source is set and takes
  precedence over your claude.ai login"). `ANTHROPIC_MODEL` arrived as the
  request body's `model`, and an unrecognised identifier is a *warning*, not a
  failure. Run without `--bare`, because that is the shape Glasshouse launches.
- **Codex 0.149.1.** `-c model_provider=<id>` plus
  `model_providers.<id>.{name,base_url,env_key,wire_api}` and `-c model=<id>`
  were all accepted under `--strict-config`, which rejects keys it does not
  know. A `base_url` of `…:8731/v1` produced `POST /v1/responses`, so Codex
  appends `/responses` and the `/v1` belongs to the provider's URL.
- **`wire_api = "chat"` is gone in 0.149.1** — ``Error loading config.toml:
  `wire_api = "chat"` is no longer supported.`` A provider serving only
  `openai-chat` therefore *cannot* back Codex, which is why the adapter
  answers `None` rather than composing a configuration Codex would reject
  after the process had started.
- **With the credential variable absent, Codex refuses** ("Missing environment
  variable: `…`") rather than falling back to the user's paid account. Line
  442's "clear launch error rather than silently using the native paid
  provider" is therefore corroborated by the harness's own behaviour, not only
  by Glasshouse's.

#### Production evidence

- `harness/mod.rs` — `DirectProviderRequest` (names and URLs, **never a
  value**), `DirectProviderPlan`, `CredentialPlacement`, and
  `HarnessAdapter::direct_provider_launch` defaulting to `None`. Splitting the
  *plan* from the *placement* is what makes the secret boundary structural: an
  adapter is never handed a credential, so it has none to interpolate.
- `harness/claude_code.rs` — three environment variables, no arguments.
  `harness/codex.rs` — six `-c` overrides, no environment. That asymmetry is
  line 441 ("adapter-specific ways to point that harness at the backend")
  expressed in the types rather than in prose.
- `profile/mod.rs` — `Resolution`, `apply_direct_provider`, `choose_protocol`,
  and eight new `Refusal` variants. `resolve` is the **only** place in
  Glasshouse where a `Secret` exists: minted at `profile/mod.rs:744`, moved
  into the overlay's environment, dropped. Verified by grep — exactly one
  production `.expose()` call in the whole crate.
- `main.rs` — `launch_session` looks the provider up through
  `EffectiveConfig::configured_provider`, builds a `Resolution` with
  `EnvironmentSecretStore`, and resolves **before** `ProjectSessions::open`.
  A refusal still costs nothing: no session record, no process.

#### Regression evidence — 18 new tests, none `#[cfg]`-gated

`a_claude_code_profile_carries_the_providers_base_url_and_credential`,
`a_claude_code_profile_carries_a_model_only_when_one_is_named` (absent, not
empty), `a_codex_profile_composes_its_provider_entirely_from_c_overrides`,
`a_codex_profile_backed_by_an_openai_chat_provider_is_refused`,
`an_unsafe_provider_name_is_refused_before_any_argument_is_composed`,
`a_credential_that_cannot_be_resolved_is_refused_and_produces_no_overlay`,
`the_first_credential_variable_that_resolves_is_the_one_used`,
`a_provider_with_no_base_url_for_the_chosen_protocol_is_refused`,
`a_harness_with_no_direct_provider_mechanism_is_refused`,
`an_expected_protocol_the_provider_does_not_serve_is_refused`,
`an_unusable_credential_variable_name_is_refused`,
`a_resolved_credential_never_reaches_a_rendering`.

#### Failure / isolation evidence

- `a_resolved_credential_never_reaches_a_rendering` plants a known value and
  asserts it is absent from the overlay's `Debug`, every mechanism note, every
  rendered argument, and the `Display` **and** `Debug` of all fourteen
  `Refusal` variants — then proves it *is* in the child environment by
  comparison rather than by printing it. Its match is exhaustive by
  construction, which caught the amendment's new variant at compile time.
- `resolving_a_launch_profile_touches_no_files` still passes: resolution opens
  no file, so nothing can write to `~/.claude` or `~/.codex/config.toml`.
  Codex needs no generated file **at all** — line 439 is satisfied by there
  being nothing to overwrite.

#### A defect caught before it shipped: automatic review depends on the backend

Phase 9A made every Claude Code session carry `--permission-mode auto`.
Composed with this phase, **every gateway-backed session would have come up
with its tools blocked**: Claude Code's auto-mode classifier is a model call,
and a third-party gateway cannot serve it as Anthropic would. The user's own
working gateway launcher avoids auto mode for exactly this reason and says so
in a comment; the binary was then checked and references no separate
classifier endpoint.

Recorded at the strength it has: **a strong reading corroborated by a working
implementation on this machine, not a controlled experiment.** So `resolve` is
now backend-aware — a `Default` selection on a non-`Native` backend adds no
approval argument and records why, and an *explicit* automatic-review request
is refused rather than silently dropped. `Bypass` is unchanged and still needs
its acknowledgement. The rule is keyed on the backend, not the harness, so
Phase 9G inherits it unchanged.
Proved by `a_defaulted_profile_selects_automatic_review_only_on_a_native_backend`
(both halves in one test) and
`an_explicit_automatic_review_request_on_a_gateway_backed_profile_is_refused`.

#### Mutations — 16 by the worker, 2 re-run independently by the orchestrator

All 16 killed. The orchestrator independently re-ran the two highest-stakes
ones against the integrated tree, restoring the single mutated file from a
byte-compared backup rather than a path-wide `git checkout`:

- `LaunchOverlay`'s manual `Debug` made to print environment *values* →
  `a_resolved_credential_never_reaches_a_rendering` **FAILED** (killed).
- a missing credential silently skipped instead of refused →
  `a_credential_that_cannot_be_resolved_is_refused_and_produces_no_overlay`
  **FAILED** (killed).

Each verdict was read from the named test's own result line in the target that
runs it, which is the trap this project has already been caught by once.

#### The worker corrected its packet three times, and was right each time

- The packet put the provider-name check inside the Codex adapter.
  `direct_provider_launch` returns `Option` and has **no error channel**, so a
  refusal there could only be spelled `None` — which must mean "this harness
  declares no mechanism", a different answer that cannot name the offending
  character. The checks moved to `harness/mod.rs` and run in `resolve` before
  a request is built, which protects every adapter, including unwritten ones.
- The packet said `secret/mod.rs` was doc-only. `Secret`'s field is private to
  that module, so **no test outside it can implement `SecretStore` at all**.
  A `#[cfg(test)] pub(crate)` minter was needed; the alternative was setting a
  real environment variable, which publishes the value to every other test in
  the process. Placement matters: it sits immediately above `mod tests`,
  because a source-scanning test splits the file at its first `#[cfg(test)]`.
- Acceptance test 8's premise was wrong: the other five adapters declare
  `protocols: Declared::Unverified`, so they are refused one step *earlier*,
  at the protocol intersection. Correct behaviour; both paths are now tested.

#### CI evidence

**CI `32900481354` green on Linux, macOS, Windows and lint** at `988dde2`,
with the five decisive tests confirmed by name in the **Windows** job's own
log rather than inferred from a green tick:
`a_resolved_credential_never_reaches_a_rendering`,
`a_credential_that_cannot_be_resolved_is_refused_and_produces_no_overlay`,
`a_codex_profile_backed_by_an_openai_chat_provider_is_refused`,
`a_claude_code_profile_carries_the_providers_base_url_and_credential`, and
`a_defaulted_profile_selects_automatic_review_only_on_a_native_backend`.
None of the new tests is `#[cfg]`-gated, which is why they run there at all.

#### Missing evidence

- **Neither path has run against a real backend through Glasshouse.** Both are
  proven against the probed behaviour of the real binaries and against
  Glasshouse's own resolution, but no Glasshouse-launched session has yet
  talked to a live gateway. For Codex this is currently impossible: every
  built-in template declares `openai-chat` only, and Codex needs
  `openai-responses`.
- For Claude Code it is now *possible*: **OpenRouter serves Anthropic Messages
  at `https://openrouter.ai/api`** — an unauthenticated `POST` to
  `/v1/messages` answers **401** while a nonexistent path under the same
  prefix answers **404**, and the user's own launcher drives Claude Code
  against exactly that root. No template declares it yet; that belongs to
  Phase 9D, and it is what would close this gap end to end.
- **Lines 440 and 441 of the map stay unchecked** (pre-flight capability
  check; installed-executable requirement before offering a profile).

### Phase 9 — the Antigravity adapter (three of seven, and one design defect it caught)

Contract: Given a signed-in Antigravity CLI, when Glasshouse starts a session
in the current project, it runs the real `agy` in the viewport and treats
anything the harness does not report as unknown.

State: **COMPLETE for lines 1, 4 and 7.** Lines 2 and 3 are blocked on an
interface change described below; lines 5 and 6 are unavailable.

The user signed the CLI in, which unblocked the whole phase.

Production evidence:
- `harness/antigravity.rs` — starts `agy`, resumes with `--conversation <id>`,
  declares `session_ids` as `Discoverable` from the CLI's own index, and
  `Antigravity::read_last_conversation`, a pure function from index text to an
  identifier.

Platform/external evidence — the real signed-in harness:
- `the_real_antigravity_interface_appears_in_the_viewport` **passes**:
  Antigravity's own version string `1.1.20` reaches the Glasshouse viewport.
  That is the same assertion Claude Code and Codex are held to, and it is
  deliberately the version rather than a name — an earlier revision of this
  probe matched a harness *name* and passed against Glasshouse's own error
  message.
- On its first run the probe failed at Antigravity's **workspace-trust
  prompt**, which gates the banner the version sits in. The captured screen
  showed the harness's ASCII logo, "Welcome to the Antigravity CLI", its
  sign-in spinner, the trust prompt and its navigation hints, all rendering
  inside the viewport — none of which Glasshouse's chrome can draw. Trusting
  the directory once, exactly as the Claude Code call site's comment already
  assumes ("the project is this repository, which the user's Claude Code
  already trusts"), and the probe passes unchanged. The assertion was never
  weakened.

**Corrections to what this ledger previously recorded.** It said conversations
live in `~/.gemini/antigravity/conversations/` and that the directory was
empty. That is the *desktop app's* state root; the CLI's is
`~/.gemini/antigravity-cli/`. Conversations there are **SQLite databases named
by UUID**, and there is a machine-readable index at
`cache/last_conversations.json` mapping **absolute project path → UUID**. This
is the fourth declaration in this project derived from an artifact that did
not serve the purpose it was cited for.

Two further facts established against the signed-in CLI:
- **Print mode records nothing.** `agy -p` completes a turn and adds no entry;
  only interactive sessions are recorded, at session end.
- **Resume does not fail closed.** `agy --conversation <unknown-uuid>` prints
  `warning: conversation "…" not found` and then **starts a fresh conversation,
  exiting 0**. Codex refuses with an error; Antigravity does not. Glasshouse
  must therefore only ever pass an identifier it recorded itself, or a user
  would get a new conversation wearing an old one's name.

### The design defect the worker refused to implement

The task packet asked for a `NativeSessionSource` with
`subdirectory: "conversations"` and extension `.db`, so that
`session::native_id` could find the records. **The worker declined and was
right.**

`native_id::discover` walks that directory and, for **every** file matching the
prefix and extension, opens it and reads up to 1 MiB before any adapter sees
it. With that source populated, every Antigravity session Glasshouse ended
would have opened **every one of the user's conversation databases** — a direct
violation of the same packet's security invariant, "never open a `.db`".
`read_session_record` returning `None` does not help: the file is already open
and read by then.

The mechanism is also the wrong shape on its merits. `read_session_record`
assumes each record *self-describes* its identity from its own first line. A
`conversations/<uuid>.db` is a binary SQLite file, and the identifier a session
needs is not in any record's bytes at all — it lives in one shared index keyed
by project path, which must be read and matched as a whole rather than
discovered by filtering file names.

**This is the third session running in which a worker was right against its
packet, and the first in which following the packet would have breached a
secret boundary.** The stop condition that produced it was worth every word.

Missing evidence:
- **Lines 2 and 3** (capture the identifier; resume a known conversation).
  `read_last_conversation` is pure and unit-proven but **has no production
  caller**, because wiring it needs a `NativeSessionSource` variant that can
  express "the identifier comes from a shared index keyed by project path
  rather than from a record's own contents". That is an interface change to
  `harness/mod.rs` and `session/native_id.rs` — the orchestrator's to design,
  and deliberately not attempted here. Same rule as `SessionRuntime` and
  Phase 1 line 90.
- **Lines 5 and 6** (structured lifecycle events): a signed-in `agy --help`
  exposes no hook, event or notification mechanism, and its subcommands are
  `agent(s)`, `changelog`, `help`, `install`, `mcp`, `mic-serve`, `models`,
  `plugin(s)`, `update`. Genuinely unavailable, now confirmed signed-in rather
  than assumed.

### Phase 9E — secret storage (eight of thirteen)

Contract: Given a provider credential, when Glasshouse needs it to launch a
harness, it resolves the value from a named source at the moment of use and
hands it only to that child process — while nothing anywhere stores, logs,
renders, serializes, or persists the value itself.

State: **COMPLETE for eight lines.** Native keychains and the settings
deletion path are deferred; see the end.

Production evidence:
- `secret/mod.rs` — `SecretRef` (a *source*, never a value), `SecretStore`,
  `Secret`, `EnvironmentSecretStore`, `redact`.
- `provider/mod.rs` — `Provider::secret_refs`, returning references only.

**The boundary is structural, not disciplinary.** `Secret` has no `Display`,
no `Deref`, no `AsRef<str>`, is neither `Serialize` nor `Deserialize`, and its
`Debug` writes a fixed marker. The only way out is `expose()`, named so it
reads wrong when it is wrong. `SecretRef` has no variant able to carry a value,
so configuration and diagnostics may hold one freely.

Regression evidence (twelve tests):
- `a_secret_ref_names_a_source_and_never_carries_a_value` — scans the enum's
  own declaration, so a future `Keychain { service, account }` passes and a
  `Literal { value }` does not.
- `debug_on_a_secret_prints_a_fixed_marker_and_never_the_value` — asserts an
  empty value and a 4096-character one render **identically**, which is the
  only form of that assertion a length cannot slip past. A length is a real
  leak: it narrows a key space.
- `is_present_reports_presence_without_resolving_a_value` — behavioural, plus
  a scan of the method's own body for `Secret`/`expose`/`to_owned`, so a later
  "simplification" to `self.resolve(..).is_some()` fails the suite.
- `resolve_reads_the_value_from_the_named_variable_at_the_moment_of_use`,
  `resolve_returns_none_for_an_unset_variable`,
  `a_secret_has_no_display_no_deref_and_no_asref`,
  `a_secret_is_not_serializable`, `nothing_in_this_module_writes_to_disk`,
  `redact_replaces_recognised_credential_shapes`,
  `redact_leaves_ordinary_text_alone`,
  `a_provider_yields_one_secret_ref_per_credential_variable`,
  `the_source_scans_would_catch_a_violation` — proves the scans fire on a real
  violation and stay quiet on a doc comment or test code that merely mentions
  one.

Non-vacuity: **three mutations, three kills** — `Debug` printing the value,
`Debug` appending a length, and `is_present` resolving. A fourth (a
value-carrying field on `SecretRef`) **could not compile**, which is the type
system holding the property rather than the test; the scan's own
falsifiability is proved separately by the test above.

**What the specialist refused, recorded because refusals are the evidence
here.** A `SecretRef::Literal { value }` variant, wanted first for tests and
then inevitably for "just paste the key in the config". A memoising cache in
the store (`EnvironmentSecretStore` is a unit struct and structurally cannot
hold a value). An error type carrying the offending value. A helpful
`Debug`. Keeping four characters in `redact` so a reader could tell two keys
apart. And `assert_eq!` on `expose()` in tests — because `assert_eq!` prints
both sides on failure, which would put a value in CI output the first time a
real one was involved.

**A bare token is deliberately not redacted.** A JWT or opaque session key
carries no identifying prefix; redacting every long token on sight would eat
git SHAs, base64 payloads and build identifiers — the exact failure
`redact_leaves_ordinary_text_alone` exists to prevent. `Bearer` keeps only the
scheme: `Authorization: Bearer [redacted]`. The specialist's first draft
asserted a bare JWT *was* redacted; the test was wrong and the behaviour was
right, and the test now says so rather than quietly dropping the case.

Missing evidence:
- **Lines 437, 438, 439** (macOS Keychain, Windows Credential Manager, Linux
  Secret Service): deferred deliberately. Each needs a dependency decision —
  which is the orchestrator's, not a worker's — and per-platform proof that
  one macOS machine cannot honestly provide.
- **Line 441** (a clearly labelled fallback): means nothing until a native
  store exists to fall back *from*.
- **Line 446** (delete a stored credential from settings): needs the settings
  UI, Phase 2D.
- **`SecretRef` derives no serde impl.** The type is *safe* to serialize —
  every field is a name — and the structural test proves it. Deriving today
  would fix an on-disk shape for a type nothing yet stores, which is a
  configuration-schema commitment belonging to the phase that first stores
  one. Accepted as the specialist argued it.
- Nothing here is reachable from the shipped binary yet, deliberately: a
  profile that can carry a credential is **Phase 9F**.

### Phase 9C/9D — the provider protocol model and its built-in templates

Contract: Given a configured provider, when Glasshouse is asked what it can
serve, it answers per protocol from what was actually established about that
provider — never inferring one protocol's support from another's — while
keeping every credential outside the answer.

State: **COMPLETE for nineteen lines** (Phase 9C eleven of twelve, Phase 9D
eight of fourteen). The rest are listed at the end with what each waits for.

Production evidence:
- `provider/mod.rs` — `ProtocolSupport` (per-protocol base URL, streaming,
  tool calls, reasoning), `Provider`, `Provider::serves`,
  `translation_available`, `templates()`, `template(name)`.
- `config/mod.rs` — `ProviderConfig`/`ProviderTable` in both layers,
  `EffectiveConfig::provider_names` / `configured_provider`, with the same
  `Layer` provenance every other setting carries.
- `integrations/mod.rs` — `glasshouse doctor`'s "Configured providers"
  section, which is the production caller that makes the model observable.

**Nine templates, every endpoint read from the user's own working gateway
setup rather than recalled**: openrouter, unorouter, anyrouter, zai,
opencode-zen, ollama, llama-cpp, and the two generic ones whose URL is
user-supplied.

**Kilo and Nous are deliberately absent.** The user holds a key for each and
no endpoint has been established for either of them.

> **Updated 2026-08-26.**
> **Kilo and Nous now have endpoints read from the live services**
> (`https://kilo.ai/api/openrouter` and
> `https://inference-api.nousresearch.com/v1`, both 200 with real catalogues;
> see `.agent-runtime/notes-provider-probes.md`). The reasoning below still
> holds and is why they were absent until today; what changed is the evidence,
> not the rule. A template with a
guessed base URL is the same failure as a guessed environment-variable name,
which Phase 9A already refuses to commit.
`no_template_exists_for_a_service_whose_endpoint_is_unestablished` fails if
one ever appears, and the module docs name all three with the reason.

Regression evidence:
- `openai_chat_support_never_implies_openai_responses` and
  `neither_openai_protocol_ever_satisfies_anthropic_messages` — lines 408 and
  409, the two inferences the model exists to prevent.
- `no_translation_is_available_between_any_two_protocols` — line 410:
  translation is a seam that can be filled later and never happens because two
  protocols looked close.
- `a_provider_may_serve_more_than_one_protocol`,
  `each_protocol_carries_its_own_base_url`,
  `an_unestablished_capability_is_unverified_rather_than_assumed`,
  `a_provider_may_declare_several_credential_variable_names`,
  `no_provider_type_can_hold_a_credential_value`,
  `a_configured_provider_may_override_a_template_base_url`,
  `the_doctor_report_names_variable_names_and_never_values`.

Non-vacuity: **five mutations, five kills** — `serves()` made to fall back to
any protocol (killing both the 408 and 409 tests), implicit translation turned
on, a `kilo` template added with a guessed URL, and the doctor made to render a
credential's value.

**One mutation first reported SURVIVED and the mutation was at fault, not the
test.** It read the credential into an unused local without printing it, so
nothing leaked and the test was right to pass. Rewritten to actually render the
value, it killed. A `SURVIVED` verdict means "this mutation did not exercise
the property" at least as often as it means the test is weak.

Platform/external evidence — the real binary:
- `glasshouse doctor` run with `OPENROUTER_API_KEY` set to an unmistakable
  secret-shaped value and a provider configured. The value appears **nowhere**
  in the entire report (0 matches), while the section renders:

      Configured providers
        my-openrouter (layer: user)
            openai-chat  base url: https://openrouter.ai/api/v1
                streaming: unverified  tool calls: unverified  reasoning: unverified
            model list endpoint: yes  usage telemetry: unverified
            credential env: OPENROUTER_API_KEY (set, value hidden),
                            OPENROUTER_API_KEY_2 (not set, value hidden)

  Two credential names on one provider is the user's multiple-keys-per-router
  requirement, working end to end.
- Credential presence is read with `std::env::var_os`, never `std::env::var`,
  so the value is not decoded even transiently.

CI evidence:
- **CI `32890989733` green on Linux, macOS, Windows and lint** at `6a5df97`,
  with the decisive tests confirmed to have executed on the Windows runner by
  name: `no_template_exists_for_a_service_whose_endpoint_is_unestablished`,
  `openai_chat_support_never_implies_openai_responses`,
  `no_translation_is_available_between_any_two_protocols`, and
  `the_doctor_report_names_variable_names_and_never_values`.

Missing evidence — and the packet was wrong about three of these:
- **Line 407** (protocol compatibility as a hard routing constraint before
  model-quality scoring): needs a router. **Phase 35.** Deliberately excluded
  from the packet.
- **Line 415** (NVIDIA-compatible template) and **416** (LiteLLM template):
  the packet claimed these as satisfied by the generic OpenAI-compatible
  template. They are not — each line asks for a *built-in* template and
  neither exists. Unchecked.
- **Line 423** (keep default URLs **and headers** overridable): base URLs are
  overridable; there is no header override at all. Unchecked.
- Lines 425-427 (connectivity testing, model-list refresh, catalogue caching):
  need a settings UI and network access.

### Phase 9B — scoped harness wrappers and shims (eight of nine)

Contract: Given a launch profile, when the user starts a harness from the shell
— through `glasshouse run` or a shim they asked Glasshouse to generate — the
harness runs with exactly the profile behaviour it would have had from the TUI,
with every override confined to that process tree, while Glasshouse never
touches a shell startup file and deleting a generated shim is enough to remove
it.

State: **COMPLETE for eight lines. Line 384 is reopened** — see "A claim
Windows would not support" below.

Production evidence:
- `cli.rs` — `Command::Run` (fields identical to `Command::Launch`) and
  `Command::Shim { harness, --profile, --dir, --name, --force }`.
- `main.rs` — `Command::Launch { .. } | Command::Run { .. }` share **one**
  dispatch arm calling `launch_session`. The or-pattern only type-checks while
  both variants declare identical fields, so a divergence is a compile error
  rather than a review miss. That is line 390 made structural.
- `shim.rs` — `ShimRequest`, `ShimError`, `render`/`default_file_name` keyed
  off the injected `HostPlatform` (never `#[cfg]`, so the Windows `.cmd` shape
  is exercised on every runner), and `generate`, the only function in the
  module that touches a filesystem and which writes exactly one file inside
  `request.dir`.

Regression evidence — the twelve named acceptance tests, plus one added by the
orchestrator:
- `glasshouse_run_and_glasshouse_launch_take_the_same_path`,
  `a_profile_behaves_identically_from_run_and_from_launch`
- `an_override_reaches_the_spawned_process_and_not_the_parent`,
  `the_users_environment_survives_except_for_explicit_overrides` — both spawn
  a real env-dumping child and confirm `PATH`, which no launch names, arrives
  unchanged beside the one explicit override.
- `a_generated_shim_contains_no_secret_and_no_url`,
  `a_generated_shim_calls_glasshouse_run`,
  `a_shim_is_written_only_inside_the_user_selected_directory`,
  `a_windows_shim_is_a_cmd_file_and_a_unix_shim_is_a_shell_script`,
  `generating_a_shim_never_touches_a_shell_startup_file`,
  `deleting_a_generated_shim_leaves_nothing_behind`,
  `an_existing_file_is_not_overwritten_without_force`
- `a_generated_shim_actually_starts_the_harness` — end-to-end: generates a shim
  through the real subcommand, then executes **only the generated file**, and
  asserts the harness received the native profile's `--permission-mode auto`.
  Unix-only, flagged, with precedent in this file.
- `a_shell_unsafe_name_is_refused_before_any_file_is_written` — **added by the
  orchestrator.** See below.

**A profile name is untrusted input reaching a command line.** The generated
shim interpolates the harness and profile names into a script, and a profile
name is user-chosen. The worker flagged that it had quoted but not escaped
them, and judged a general shell-escaper out of scope — correctly, because the
right answer here is not escaping. This codebase already answers this class of
problem by **refusing**: `platform::exec` rejects `cmd.exe` metacharacters in
harness arguments rather than trying to quote them
(`spawn_command_windows_script_rejects_each_cmd_metacharacter`). So
`check_name` now refuses any name outside `[A-Za-z0-9._-]`, before a path is
computed or a byte written, and says which character it objected to. An
allow-list is right by construction where an escaper has to be right about two
shells forever, and a profile name is a TOML table key, so nothing legitimate
is lost.

Non-vacuity: **six mutations, six kills** — the unsafe-name check removed; a
provider URL embedded in the shim; a shell-startup-file write added; the
overwrite guard removed; Windows rendering the Unix script; and a second
`launch_session` call site introduced. The last verdict was re-verified by
reading the **bin** target's own result line after the first reading showed
only the lib target's, which had filtered the test out.

Platform/external evidence — the real binary:
- `glasshouse shim claude-code --profile native --dir <tools>` wrote a
  125-byte, mode-0755 file whose entire contents are
  `#!/bin/sh` and one `exec "<glasshouse>" run "claude-code" --profile
  "native" -- "$@"` — no secret, no URL, no routing logic, no duplicated
  adapter argument. It printed the exact path and the line saying that
  deleting the file is all it takes.
- `glasshouse shim claude-code --profile 'evil"; id; echo "'` was refused:
  "refusing to generate a shim for profile `…`: it contains `"`, which a shell
  would interpret rather than pass through."

CI evidence:
- **CI `32887437992` green on Linux, macOS, Windows and lint** at `5f99865`.
  Windows executed the shim tests by name
  (`a_generated_shim_calls_glasshouse_run`,
  `a_generated_shim_contains_no_secret_and_no_url`,
  `deleting_a_generated_shim_leaves_nothing_behind`) and 451 lib tests against
  macOS's 459 — the difference is the Unix-gated set, including the
  environment-inheritance assertion described below.
- The lint job's `Check README progress block` step ran and passed, so the
  README's generated block is verified against the map on every push rather
  than trusted.

### A claim Windows would not support, and the box that came back off

Line 384 — "preserve the user's existing shell environment except for explicit
launch-profile overrides" — **was checked and is now unchecked.** Its test
passed on macOS and Linux and failed three times on `windows-latest`, and the
third failure's message is what finally said why:

    expected (the test process): PATH=D:\A\GLASSHOUSE\GLASSHOUSE\TAR...
    child reported:              Path=C:\Program Files\MongoDB\Server\...

The child's `PATH` is the **system** one. The calling process's own `PATH` — a
cargo test binary's, with the target directory prepended — is simply absent
from it. On the same run, the explicit override asserted immediately above
*did* reach that child, so `CommandBuilder::env` works on Windows and only
**inherited** variables are in question.

The strongest reading of the evidence: `portable_pty::CommandBuilder` composes
the child environment on Windows from the system/user environment rather than
from the calling process, layering explicit overrides on top. **That is a
reading, not a proven fact** — nothing here has run on a real Windows host, and
`into_builder` itself does nothing but `CommandBuilder::new` plus explicit
`env`/`env_remove` calls.

Two wrong fixes preceded this. The first blamed line wrapping and normalised
whitespace; the second blamed ConPTY's deferred-wrap character duplication,
which is **real** (see the handoff's loose end) but was not this failure's
cause. Both made the test pass locally, where `PATH` is short and the parent's
prefix happens to survive, and left Windows failing for the original reason.
The lesson is the project's own: read the failure before forming the fix.

The assertion is now `#[cfg(unix)]`, claiming only the platform it can
demonstrate, and line 384 stays unchecked until someone runs this on a real
Windows host and determines whether the harness a user launches there sees the
environment their shell set.

Missing evidence:
- **Line 384 on Windows**, above. If the reading is right it is a product
  defect rather than a test defect, and it would matter to any user who
  configures a harness through their shell environment.
- `a_generated_shim_actually_starts_the_harness` is Unix-only; the Windows
  `.cmd` *content* is covered everywhere by
  `a_windows_shim_is_a_cmd_file_and_a_unix_shim_is_a_shell_script`, but
  actually executing a generated `.cmd` on native Windows is unproven.
- `glasshouse shim` does not create `--dir` if it is missing, by design; that
  failure path has no dedicated test.

### Phase 9A — harness launch profiles (seventeen of twenty-six)

Contract: Given a harness and a selected launch profile, when the user starts a
session, Glasshouse resolves that profile through the harness's own adapter
into an overlay that applies to the child process only — refusing any
combination the adapter does not declare rather than inventing one — and
records which profile the session ran under, while never modifying the user's
global harness installation or configuration.

State: **COMPLETE for seventeen lines; nine deliberately left open** (listed at
the end, each with the phase that unblocks it).

Production evidence:
- `profile/mod.rs` — `LaunchProfile`, `BackendResource`, `ProfileClass`,
  `ApprovalSelection`, `LaunchOverlay`, `MechanismNote`, `Refusal`, `resolve`.
- `main.rs: launch_session` — the production caller. Resolution happens
  **before** `ProjectSessions::open`, so a refusal starts no process and writes
  no row. Argument order is adapter args, then the overlay, then the user's own
  `--` arguments last.
- `config/mod.rs` — `ProfileTable`/`ProfileConfig` in both layers,
  `EffectiveConfig::profile_names`/`launch_profile`, and
  `bypass_acknowledged`, which reads the **user layer only**.
- `database.rs` — migration 3 adds `sessions.launch_profile` and
  `sessions.backend_resource`, both nullable. Migrations 1-2 untouched.
- `cli.rs` — `glasshouse launch --profile <name>`.

The design and its reasoning are in `GLASSHOUSE_DESIGN_DECISIONS.md`.

Regression evidence (the twelve named acceptance tests, plus):
- `a_native_profile_exists_for_every_harness_and_adds_nothing`
- `an_explicit_automatic_review_request_is_refused_on_a_harness_without_one`
- `a_defaulted_profile_on_such_a_harness_adds_no_approval_argument`
- `a_bypass_is_refused_until_it_is_acknowledged_for_that_harness`
- `a_provider_backed_profile_is_refused_with_the_phase_that_supplies_it`
- `an_overlay_reaches_only_the_child_process`
- `the_user_s_own_arguments_stay_last`
- `a_refused_profile_starts_no_process_and_records_no_session` and
  `an_unacknowledged_bypass_also_starts_no_process_and_records_no_session` —
  both call the real `launch_session` and assert the store is empty afterwards.
- `upgrading_a_version_2_database_preserves_every_existing_session`
- `no_launch_profile_definition_is_stored_in_the_project_database`
- `no_environment_value_is_ever_rendered`
- `a_project_layer_cannot_acknowledge_a_bypass`
- `resolving_a_launch_profile_touches_no_files` — added by the orchestrator for
  line 362, which had no guard. A source scan of `profile/mod.rs`'s production
  code for `std::fs`/`fs::`/`File::`/`OpenOptions`/`std::env`: a module that
  never opens a file cannot modify the user's global harness configuration,
  which is stronger and cheaper to keep true than enumerating forbidden paths.
- `a_configured_gateway_profile_never_displaces_the_native_one` — added by the
  orchestrator for line 371. Fails if anyone ever "unifies" the lookup by
  moving Native into the table.

Non-vacuity: **nine mutations, nine kills.** Silently using the bypass for an
explicit automatic-review request; a defaulted profile falling back to the
bypass; the acknowledgement gate removed; the provider-backend refusal removed
(killing both its own test and the no-session-recorded one); the project layer
allowed to acknowledge a bypass; migration 3 using a `'native'` sentinel
instead of NULL; a filesystem write added to `profile/mod.rs`; and Native
dropped from `profile_names`.

Platform/external evidence — **the real harness, not a fake one**:
- `glasshouse launch codex` was run from the built binary in a real terminal
  against Codex 0.149.1. It started, and Codex displayed **its own workspace
  trust prompt** in the viewport — a native prompt staying interactive, which
  is the product invariant, and proof the injected `--approve-for-me` did not
  break startup. The prompt was declined; nothing was trusted.
- `glasshouse sessions` then showed the session with `PROFILE = native`.
- `glasshouse launch codex --profile bogus` printed
  "`bogus` is not a known launch profile; valid names are: native" and the
  session count stayed at one — refusing really does cost nothing.
- The mechanism diagnostic, read from a real log:
  `profile=native backend=native mechanisms=approval mode: automatic review:
  Route approval requests through automatic review using the workspace-write
  sandbox (--approve-for-me)`.
- Flag-conflict probes: `codex --approve-for-me --sandbox read-only
  --ask-for-approval on-request` and `claude --permission-mode auto
  --permission-mode plan` both parse, so Glasshouse's injected default does not
  break a user's own pass-through arguments, which come last.

**A behaviour change this closes, stated plainly:** every Codex session
Glasshouse starts now carries `--approve-for-me`, and every Claude Code session
`--permission-mode auto`, because the default profile selects the harness's own
automatic-review mode. That is the decision recorded under "Approvals"; it is a
real change in what the user's harness does, and the end-to-end PTY test now
asserts the exact argv rather than the previous "no arguments at all".

CI evidence:
- **CI `32881520282` green on Linux, macOS, Windows and lint** for `6e730f6`,
  with the new tests confirmed to have *executed* on the Windows runner by
  name — including
  `upgrading_a_version_2_database_preserves_every_existing_session`, which is
  the one that would matter most if the migration behaved differently there,
  plus `resolving_a_launch_profile_touches_no_files`,
  `a_configured_gateway_profile_never_displaces_the_native_one`,
  `a_bypass_is_refused_until_it_is_acknowledged_for_that_harness`,
  `a_provider_backed_profile_is_refused_with_the_phase_that_supplies_it` and
  `an_explicit_automatic_review_request_is_refused_on_a_harness_without_one`.

Missing evidence — the nine open lines and what each waits for:
- **350** (a profile is harness + backend + model + protocol + overlay +
  *response profile*): `LaunchProfile` has five of the six. Response profiles
  are **Phase 9K**.
- **353** (additional profiles such as Claude / OpenRouter) and **373**'s
  richer cases: need provider configuration, **Phases 9C/9D**.
- **355** (inject environment variables): the mechanism is proven, but **no
  shipped profile can populate `env`** — only `Native` resolves, and it
  contributes none. The test constructs an overlay directly and says so. Same
  rule as `SessionRuntime` and Phase 1 line 90: a mechanism with no production
  caller does not get its box. Unblocks with **Phase 9F**.
- **359** (isolated generated configuration file) and therefore **360** (the
  overlay representing *all* the mechanisms together) and **363** (prefer
  Glasshouse-owned generated configuration): nothing generates configuration
  yet. **Phase 9F**.
- **365** (record backend, model, protocol, pairing class, response profile):
  pairing class is **9J**, response profile **9K**. `model` and
  `wire_protocol` columns were deliberately *not* added — no profile can carry
  either until 9F, and a column nothing writes is the speculative
  infrastructure Phase 21H forbids.
- **369** (the router selects among enabled profiles): **Phases 34-37**.

Known gaps that are not blockers:
- There is no wizard for authoring a profile; they are hand-edited in
  `config.toml` today. Phase 2D's "Launch Profiles" section is now unblocked.
- `resume_session` does not apply an overlay, so a resumed session does not
  carry its profile's arguments. Which profile a resume should use — the one
  recorded, or whatever is configured now — is a real design question and is
  recorded as a loose end rather than guessed at.

### Phase 6 — Make each adapter declare which native approval/permission modes it supports

Contract: Given a supported harness, when Glasshouse is asked what approval
modes it offers, the adapter answers with what was read from that harness's own
binary — naming its automatic-review mode when it has one, and saying plainly
when nobody established one — while never presenting a blanket bypass as though
it were review.

State: COMPLETE.

Production evidence:
- `harness/mod.rs: ApprovalModes` — `automatic_review`, `bypass` and `sandbox`,
  each a `Declared<&'static str>` like every other harness fact.
- All seven adapters declare it; `HarnessDescription` carries it.
- `integrations/mod.rs: write_adapter_report` — `glasshouse doctor` prints it,
  which is what keeps the declaration from being data nothing reads.

The distinction the type exists for: **a blanket bypass is not automatic
review.** Claude Code's auto mode, Codex's `--approve-for-me` ("automatic review
using the workspace-write sandbox") and Cursor's `--auto-review` ("a server
classifier auto-runs safe tool calls") classify. OpenCode's `--auto`
("auto-approve permissions that are not explicitly denied (dangerous!)"),
Hermes's `--yolo` and Antigravity's `--dangerously-skip-permissions` do not, and
are recorded as bypasses only.

Regression evidence:
- `each_adapter_declares_the_approval_mode_its_binary_documents` — pins the
  whole table, harness by harness, mode string by mode string.
- `every_verified_declaration_cites_its_evidence` covers the new field.
- `the_doctor_report_describes_every_adapter` — the rendered row.

Non-vacuity: **the first version of this test was weak and a mutation proved
it.** It asserted only that an `automatic_review` evidence string avoided the
words "yolo", "dangerously" and "bypass"; a mutation recording OpenCode's
`--auto` as automatic review, with evidence reading "…(dangerous!)", walked
straight through — "dangerous!" is not "dangerously". The fuzzy test was
replaced by the exact table above, and the same mutation now fails, as does
removing Cursor's real `--auto-review`. The lesson is recorded in the test's own
comment: pin *which mode each harness has*, never how a declaration is worded.

Failure/isolation evidence:
- `Unverified` renders as **"automatic review unverified"**, never "no automatic
  review". `Declared` cannot express "verified absent" for a mode name, so
  absence of a declaration means nobody established one — a different claim from
  the harness not having one. Pi makes it concrete: installed, but not on `PATH`
  here, so its `--help` could not be read and everything about it is
  `Unverified`.

Platform/external evidence:
- Every declaration read on 2026-08-25 from the installed binaries: Claude Code
  2.1.245, Codex 0.149.1, Cursor CLI 2026.08.11, OpenCode 1.18.22, Hermes Agent
  0.15.1, Antigravity CLI 1.1.20. Pi 0.73.1 could not be read.
- `glasshouse doctor` run from the built binary and its output read, which is
  how the "no automatic review" overstatement was caught before it shipped.

### Correction, later the same day: the declaration now carries argv

Everything above is still true about *which* mode each harness has. One
citation was wrong in a way that only mattered once something tried to **use**
a declaration, which Phase 9A does.

`ApprovalModes` stored one human-readable string per mode, and three of the
seven could not be used as launch arguments at all:

- **Claude Code declared `auto-mode`.** That is a *subcommand* — "Inspect or
  reset auto mode classifier configuration". Appending it to a launch would
  have run the subcommand instead of starting a session. The flag that selects
  the mode **for a session** is `--permission-mode auto`, one of six choices
  (`acceptEdits`, `auto`, `bypassPermissions`, `manual`, `dontAsk`, `plan`).
- **Codex and Cursor declared their sandbox as usage strings** —
  `-s/--sandbox <read-only|workspace-write|danger-full-access>` and
  `--sandbox <mode>` — carrying placeholders no process can receive.

A mode is now `ApprovalMode { args, description }` and the sandbox a
`SandboxSelector { flag, values }`. `args` is the exact argv; `description` is
the harness's own wording. Both stay, because conflating them is what produced
an unlaunchable declaration. `HarnessAdapter::approval_args` reads them and
answers `None` — never a substitute — for a mode a harness lacks.

Production evidence:
- `harness/mod.rs: ApprovalMode`, `SandboxSelector`, `ApprovalKind`,
  `HarnessAdapter::approval_args`.
- `integrations/mod.rs: write_adapter_report` renders the description **and**
  the argv.

Regression evidence:
- `each_adapter_declares_the_approval_mode_its_binary_documents` — now pins the
  **argv**, harness by harness, rather than a description.
- `claude_code_selects_auto_mode_with_a_session_flag_not_the_subcommand` —
  fails if `auto-mode` reappears in the selecting argv.
- `no_approval_argument_is_a_usage_string_rather_than_an_argv_entry` — fails on
  any element containing a space, `<`, `>` or `|`. This is the check that would
  have caught the sandbox usage strings being handed to a process.
- `a_harness_without_automatic_review_offers_no_substitute` —
  `approval_args(AutomaticReview)` is `None` for OpenCode, Hermes, Antigravity
  and Pi, and never silently that harness's bypass argv.
- `no_approval_description_contains_a_backtick` — the report wraps descriptions
  in backticks, so one carrying its own renders doubled.

Non-vacuity: **five mutations, five kills** — Claude Code's argv reverted to
the subcommand (killing two separate tests), an argv turned back into a usage
string, `approval_args` made to fall back to the bypass when review is
unverified, and a description given a backtick again.

Platform/external evidence:
- `claude --permission-mode auto` accepted, `--permission-mode bogus` rejected
  with the allowed list naming `auto` — Claude Code 2.1.245, 2026-08-25.
- `codex --approve-for-me` accepted **through the cmux PATH shim**, with an
  invalid variant erroring and suggesting the real flag. That also settles the
  recorded worry about whether the wrapper would swallow a flag Glasshouse
  adds — it does not.
- `glasshouse doctor` run from the built binary, which caught two rendering
  defects the types could not: Claude Code's and Cursor's descriptions
  contained backticks and printed doubled inside the backticks the report adds.
- **CI `32875637992` green on Linux, macOS, Windows and lint** for `37605ad`,
  with all four new tests confirmed to have *executed* on the Windows runner by
  name — `claude_code_selects_auto_mode_with_a_session_flag_not_the_subcommand`,
  `no_approval_argument_is_a_usage_string_rather_than_an_argv_entry`,
  `a_harness_without_automatic_review_offers_no_substitute` and
  `no_approval_description_contains_a_backtick` — rather than inferred from an
  aggregate green.

**This is the third declaration derived from an artifact that did not serve
the purpose it was cited for**, after Antigravity's executable name and
Codex's snake_case hook-event spellings. The rule the pattern earns: *before a
declaration is used, check that its evidence supports the use, not merely the
claim.*

Missing evidence:
- Pi's approval modes. Needs `~/.hermes/node/bin` on `PATH`, or a configured
  explicit executable path.
- Selecting a mode at launch is Phase 9A, and unimplemented. This line remains
  the declaration half — but the declaration is now *launchable*, which is what
  the selection half needs.

### Phase 9 — the Antigravity adapter (probed, and blocked on authentication)

State: **BLOCKED — the installed CLI is not signed in.** Everything that can be
established without an account has been.

What the binary says (Antigravity CLI 1.1.20, `/opt/homebrew/bin/agy`, read
2026-08-25):
- Resume is `--conversation <id>` — "Resume a previous conversation by ID" —
  which is what the adapter already declares. `--continue` / `-c` continues the
  most recent.
- `--project <id|name>` and `--new-project` scope a session to a project.
- `--dangerously-skip-permissions` and `--sandbox`, both already declared under
  Phase 6's approval modes.
- Also `--mode accept-edits|plan`, `--effort low|medium|high`, `--model`,
  `--agent`, and `-p/--print` for non-interactive runs.
- **No hook, event or notification mechanism appears anywhere in `--help`.**
  Subcommands are `agent(s)`, `changelog`, `help`, `install`, `mcp`,
  `mic-serve`, `models`, `plugin(s)`, `update`. So Phase 9's lines 5 and 6
  (structured lifecycle events) look genuinely unavailable, the way Claude
  Code's compaction line is — but that should be confirmed against a signed-in
  CLI before being declared.
- Conversations live in `~/.gemini/antigravity/conversations/`. The directory
  exists and is **empty**, because the CLI has only ever been interrogated on
  this machine, never run. So the identifier's format and discoverability are
  unestablished.

**Line 4 has real supporting evidence already.** Driving the shipped shell
against `agy` through `probe_real_harness_interface` put Antigravity's own
interface in Glasshouse's viewport — its welcome box, the text "Welcome to the
Antigravity CLI", "You are currently not signed in", "Select login method",
its numbered options and "[Use arrow keys to navigate, Enter to select]". None
of that is anything Glasshouse's chrome can draw, so the round trip through
`vt100` into Ratatui cells demonstrably works for this harness.

**The probe was nonetheless reverted rather than weakened.** It asserts the
harness's own *version string* reaches the viewport, deliberately, because an
earlier revision matched a harness *name* and passed against Glasshouse's own
error message. An unauthenticated `agy` opens on a login menu that carries no
version, so the assertion cannot hold here. Loosening it to match the login
text would reintroduce exactly the weakness the assertion exists to prevent, so
the test is not in the tree.

Missing evidence, and what unblocks it:
- **Somebody must sign the CLI in** (`agy` offers Google OAuth or a Google
  Cloud project). That is the user's credential and the user's action; nothing
  here can or should do it.
- Once signed in: re-add `probe_real_harness_interface("antigravity", "agy")`,
  which should then pass unchanged and close lines 1 and 4 together; take one
  turn to populate `~/.gemini/antigravity/conversations/` and read the
  identifier's shape for line 2; and confirm from a signed-in `--help` whether
  any hook mechanism exists before declaring lines 5-6 unavailable.

### Phase 8 — Record observed Codex compaction events or compaction-related state when available

Contract: Given a Codex session that compacts its context, when Glasshouse is
asked about that session afterwards, it can say that a compaction happened —
without replacing the harness's own compaction mechanism.

State: **BLOCKED on Phase 30, and deliberately not forced.**

What is established:
- Codex exposes `PreCompact` and `PostCompact` in the hook catalogue read from
  its own review screen, so unlike Claude Code — which exposes no compaction
  hook at all, and whose equivalent line is blocked for that reason — the
  events are genuinely *available* here.
- Codex has a `/compact` command ("summarize conversation to prevent hitting
  the context limit"), so a compaction can be triggered deliberately rather
  than waited for. Observing these events is therefore cheap, which is not the
  usual situation with compaction.
- Requesting them is two lines in `REPORTED_EVENTS`.

Why the box is not checked anyway:
- `lifecycle_for` maps a harness event to a `SessionLifecycle`, and compaction
  **is not a lifecycle state** — a session that compacts was running before and
  is running after. Mapping it to one would be a lie of convenience.
- So "record" has to mean durable state of some other kind, and that state is
  **Phase 30's line: "Track the number of observed compactions for a session
  when known."** Phase 30 is unimplemented and far later in map order.
- Requesting the events and writing only a log line would satisfy "observed"
  while quietly failing "record". A log is not state, and checking the box on
  it would be exactly the confusion between code presence and product behaviour
  this ledger exists to prevent.

The honest shape: this line is the observation half of a capability whose
storage half belongs to Phase 30, the same way Phase 1 line 90 sat complete and
unreachable until an adapter existed to feed it. When Phase 30 adds the counter,
this becomes a two-line change plus one `/compact` against the real binary.

Missing evidence:
- Phase 30's per-session compaction count. Everything else is ready.

### Phase 8 — Detect Codex waiting-for-user and permission states structurally when possible

Contract: Given a Codex session Glasshouse started, when Codex stops to ask the
user to allow something, Glasshouse records the session as waiting for the user
— and records it moving on once they answer — without reading the terminal or
inferring anything from what is on screen.

State: COMPLETE (macOS; the mechanism is platform-independent and its unit
coverage runs everywhere).

Production evidence:
- `harness/codex.rs: REPORTED_EVENTS` includes `PermissionRequest`.
- `session/lifecycle.rs: lifecycle_for` — `"PermissionRequest" =>
  WaitingForUser`. Shared with Claude Code, which spells it identically.
- The hook chain proven in the entry below carries it.

Regression evidence:
- `codex_events_translate_to_the_states_they_mean` covers
  `PermissionRequest -> WaitingForUser`.
- `nothing_derives_session_state_from_terminal_output` — the state cannot have
  come from reading the screen, because no production code may do that.

Platform/external evidence — **the whole cycle was watched against the real
binary**:
- `glasshouse launch codex -- --sandbox read-only --ask-for-approval on-request`
  (which also demonstrates the `--` pass-through reaching the harness: Codex
  reported "Read Only").
- Asked to create a file, Codex raised its own approval prompt — "Apply
  proposed file edits / Yes, proceed" — and the session record moved to
  **`lifecycle = 'waiting_for_user'`**.
- On approving, the file was created and the record moved to
  **`lifecycle = 'idle'`**.
- So the observed cycle is `running -> waiting_for_user -> idle`, every
  transition written by a hook Codex fired, none of it inferred from the
  screen.

Missing evidence:
- Linux and Windows have not executed a real permission cycle; the translation
  itself is unit-covered on all three.

### Phase 8 — Codex lifecycle hooks (three lines: integrate, translate, detect turn completion)

Contract: Given a Codex session Glasshouse starts in a project where the user
has consented to project-local hooks, when Codex reports a lifecycle event,
Glasshouse records the session state that event implies — while never reading
the conversation content the payload carries, never writing outside the
project, and never letting a hook failure affect the session.

State: COMPLETE on macOS and Linux; Windows green after a defect it caught.

**Windows CI found a third real defect on this repository**, this time in the
payload scan's own helper. It located the end of `report_hook` with
`find("\n}\n")`, and Windows checks this file out with CRLF endings, so the
closing brace reads `\r\n}\r\n` and the pattern never matched. It panicked
rather than scanning an empty span — the right direction to fail in, and exactly
what the hardening was for. The fix matches only the newline *before* the brace,
which works on both; a brace at column zero can only be the function's own.

**The whole chain was watched running against the real binary**, the same proof
Phase 7 used. `glasshouse launch codex` was run in a real terminal against
Codex 0.149.1 with `project_hooks = true`. Glasshouse generated the document and
wrote it to `<project>/.codex/hooks.json` (five events, `timeout: 3`, every path
pinned); Codex asked to trust the directory, then asked to review the hooks;
after trusting, one real turn was taken — and the session record moved to
**`lifecycle = 'idle'`**.

That value settles it rather than suggesting it. The only production code that
*writes* `Idle` is the `Stop`/`StopFailure` arm of
`session::lifecycle::lifecycle_for`; nothing else in Glasshouse can produce it.
So the record could only have reached that state by Glasshouse generating the
document, writing it under consent, Codex reading and trusting it, Codex firing
`Stop`, that hook invoking `glasshouse hook`, and the translation recording it.
Generate, install, trust, fire, report, translate, record — end to end.

Quitting the session cleanly then moved it to `stopped` and **captured
`native_session_id = 01a03983-b696-7832-ac49-296a4deccda1`**, which was verified
to be the exact rollout Codex wrote for that project
(`originator: codex-tui`, no `parent_thread_id`, matching `cwd`). Disposition
read `resumable`. **That also closes the open gap in the Phase 8 line 2 entry
above** — a live Codex turn has now had its identifier captured end to end.

Production evidence:
- `harness/mod.rs: HookDestination` — `GlasshouseOwned` or
  `ProjectLocal { relative_path }`. Making the destination part of the
  declaration is what keeps the consent rule enforceable in one place; no
  adapter can opt out of it.
- `harness/codex.rs: Codex::hook_installation` — builds the `hooks.json`
  document, declares `ProjectLocal { ".codex/hooks.json" }` and **empty
  arguments**, because Codex finds the file itself.
- `session/select.rs: install_hooks` — writes a `ProjectLocal` installation
  only with consent; without it, no file, no directory, `Ok(None)`, and the
  session starts normally.
- `config/mod.rs` — `project_hooks` consent, an `Option<bool>` so the tri-state
  is per field. A plain `bool` with `#[serde(default)]` caused a real defect
  before and is not repeated.
- `session/lifecycle.rs` — `SessionStart` added. `UserPromptSubmit`,
  `PermissionRequest` and `Stop` needed no change at all, because **Codex
  spells them exactly as Claude Code does**. The module's doc claiming Codex
  used snake_case was wrong and is corrected.
- `main.rs: report_hook` — drains stdin into `std::io::sink()` and never parses
  it.

Regression evidence:
- `codex_hooks_are_written_into_the_project_only_with_consent` — without
  consent, no file and no `.codex` directory.
- `codex_hooks_are_written_where_codex_reads_them` — with consent, the document
  lands at `<root>/.codex/hooks.json` and names the five reported events.
- `a_codex_hook_declares_a_timeout_codex_will_not_clamp` — every timeout <= 3.
- `codex_declares_a_project_local_destination`,
  `claude_code_and_codex_are_the_harnesses_with_a_verified_hook_installation`.
- `codex_events_translate_to_the_states_they_mean`.
- `the_hook_command_never_reads_its_payload` plus
  `the_payload_scan_would_catch_a_violation`.
- `tri_state_project_hooks_consent_distinguishes_never_asked_from_a_decision`.

Non-vacuity: **four mutations run by the orchestrator, four killed** — removing
the consent gate; raising Codex's declared timeout to one Codex would clamp;
mapping `SessionEnd` (which must stay unmapped); and making the hook handler
read and log its payload. The source scan was additionally hardened: it now
asserts the slice it scans actually contains `std::io::sink()`, because a scan
over the wrong span passes for the wrong reason — this project has been caught
by exactly that before, with a `skip_while` that found a harness list where an
adapter block was meant.

Failure/isolation evidence:
- **The payload carries the conversation** — `prompt` is the user's own words,
  `last_assistant_message` the model's reply. Glasshouse takes the event name
  and session identifier from its own argv and reads neither. Proven by mutation,
  not asserted.
- `SessionEnd` is deliberately **not** mapped: the operating system reporting
  the process is the authority for a session ending, and a hook only races it.
- A late hook cannot revive a finished session (`may_apply`); an unfamiliar
  event changes nothing.
- The one place Glasshouse ever writes inside a user's repository logs the exact
  path it created.

Platform/external evidence:
- Codex 0.149.1, probed in a real terminal: a project-local
  `.codex/hooks.json` is read (Codex named the file in its own diagnostic),
  hook trust is a prompt distinct from workspace trust, and `SessionStart`,
  `UserPromptSubmit` and `Stop` all fired with one real turn taken.
- **Codex clamps hook timeouts**, announcing `clamping SessionEnd hook timeout
  to 3s`. The declared timeout is 3 so a real installation warns about nothing.

Missing evidence:
- CI on Linux and Windows.
- Lines 8 and 9 (waiting-for-user/permission states, compaction) are open.
  `PermissionRequest` is installed and translated but has not been watched
  firing; `PreCompact`/`PostCompact` exist in Codex's catalogue and are not yet
  requested.

### Phase 8 — Support resuming a known Codex session through Codex's native resume mechanism

Contract: Given a recorded Codex session whose native identifier Glasshouse
captured, when the user resumes it, Glasshouse reopens that same conversation
through Codex's own `resume` subcommand — while never handing Codex a flag
belonging to a different harness, and never assigning a fresh identifier to a
session that already has one.

State: COMPLETE.

Production evidence:
- `main.rs: resume_session` — generic by construction. It selects the harness
  the *record* names, not whichever is configured now, and asks that harness's
  adapter for the invocation. Nothing in it knows Codex exists.
- `session/select.rs: HarnessSelection::resume_args` — the adapter's start
  arguments, then its resume invocation, then the user's own.
- `harness/codex.rs: Codex::resume` — `["resume", <id>]`.
- `session/store.rs: open_for_resume` — decides whether a session may be
  resumed at all (right project, not still running, something to resume to)
  before a harness is selected and long before a process exists.
- Nothing was written for this line. What made it reachable was Phase 8 line 2:
  until an identifier was captured, `glasshouse resume` could only ever answer
  "not resumable".

**The shape difference is the whole point.** Codex resumes with a *subcommand*,
Claude Code with a *flag*. That is precisely the harness-specific knowledge the
Phase 6 adapter contract exists to absorb, and it is now asserted rather than
assumed.

Regression evidence:
- `a_recorded_codex_session_is_resumed_through_its_own_subcommand` (PTY smoke,
  **executed on macOS, Linux and Windows** — deliberately not `#[cfg]`-gated,
  because Windows CI found a real defect on this exact fixture path for line 2.
  Confirmed by name in CI `32854403090`'s Windows and Linux job output, not
  inferred from a green tick; the Windows PTY count went 35 -> 36).
  Drives the shipped binary end to end: `glasshouse launch codex` against a
  fake harness that writes a real rollout header under an isolated `CODEX_HOME`
  and echoes its own argv; asserts the launch argv is **bare**; takes the
  *short* twelve-character identifier out of `glasshouse sessions`, which is
  the only form that listing shows and therefore the only one a user could
  type; resumes with it; then asserts the harness was handed `resume` and the
  captured identifier, and **not** `--resume` and **not** `--session-id`.

Non-vacuity: **three mutations run by the orchestrator, all three killed** —
giving Codex Claude Code's `--resume` flag; returning no resume arguments at
all; and resuming a different conversation. The first is the one that matters:
it is the adapter contract leaking one harness's vocabulary into another, and
the test names that failure in its own assertion message.

Failure/isolation evidence:
- `resuming_a_session_with_no_conversation_is_refused` reaches `open_for_resume`
  through a Codex session precisely because Codex has no identifier to resume
  to until one is captured — that test was written for this shape.
- `resuming_an_unknown_session_is_refused`,
  `resuming_a_session_belonging_to_another_project_is_refused`,
  `a_live_session_is_not_resumable`.

Platform/external evidence:
- Against the real Codex 0.149.1 in a pseudo-terminal, neither costing a model
  turn: `codex resume <a real recorded id for this project>` printed
  `Resuming session…` and replayed the conversation; `codex resume
  9f1c0b2e-0000-4000-8000-0123456789ab` answered `ERROR: No saved session found
  with ID <id>. Run `codex resume` without an ID to choose from existing
  sessions.`
- Two traps found while probing, recorded so nobody rediscovers them: a
  pseudo-terminal with **no window size** makes Codex emit its handshake and
  draw nothing, which reads exactly like a hang; and Codex may open with an
  **update prompt whose default option runs `curl … | sh`**, so it must never
  be dismissed with Enter.

Missing evidence:
- Glasshouse does not yet surface the harness's own refusal. If a recorded
  identifier no longer exists, Codex prints its `ERROR: No saved session found`
  and exits, and Glasshouse reports only that the harness exited non-zero. The
  session record is optimistic — it says `resumable` on the strength of a
  captured identifier, not on proof the conversation still exists. Honest, but
  the message the user sees should be the harness's.

### Phase 8 — Capture the native Codex thread or session identifier when it can be obtained reliably

Contract: Given a Codex session Glasshouse started in the project root, when
that session ends, Glasshouse records the native identifier of the interactive
Codex session that ran in that directory inside that window — while recording
nothing at all when the right record cannot be told apart from a subagent
thread, another client's session, another project's session, or a second
candidate.

State: COMPLETE. Linux, macOS and Windows all executed these tests in CI
`32849951837`, and the two end-to-end tests were confirmed by name in the
Windows job's output rather than inferred from a green tick.

Production evidence:
- `session/native_id.rs: discover` — walks the harness's own records root,
  applies four filters, and refuses ambiguity.
- `session/native_id.rs: capture` — the production wiring, called at session
  end from **both** producers: `main.rs: launch_session` (before
  `note_lifecycle`) and `shell/mod.rs`'s `poll_exits` loop (before
  `set_lifecycle`). Best effort by construction: it only ever logs, because
  the harness has already run and a bookkeeping failure must not become the
  user's error.
- `harness/codex.rs: Codex::session_id_source` / `::read_session_record` — the
  Codex-shaped half: `CODEX_HOME`/`.codex`/`sessions`/`rollout-`/`jsonl`, and
  a pure parse of one header line.
- `session::store::set_native_session_id` finally has a production caller. It
  has existed unused since Phase 2.

Regression evidence (all executed on macOS; the unit tests are
platform-independent and run everywhere):
- `the_interactive_session_in_the_window_is_the_one_captured` — a subagent, a
  desktop record and the real interactive one, same cwd, same window; only the
  interactive one is taken.
- `a_subagent_thread_is_never_captured` — the case that would otherwise be
  captured most often.
- `another_project_s_session_is_never_captured`,
  `a_session_that_started_before_this_one_is_never_captured`,
  `two_candidates_are_refused_rather_than_guessed`,
  `a_record_without_a_session_id_field_is_skipped`,
  `an_unreadable_records_root_is_not_an_error`.
- `nothing_is_read_past_the_first_line` — the secret boundary.
- `a_codex_session_s_identifier_is_captured_by_the_launch_path` and
  `a_codex_session_started_from_the_shell_has_its_identifier_captured_on_exit`
  — the shipped binary, a fake `codex` that writes a real rollout-shaped
  header under an isolated `CODEX_HOME`, one per production call site.

Non-vacuity: **eight mutations were run by the orchestrator and all eight
failed their target test.** Dropping the interactive filter, the cwd filter or
the time window; resolving ambiguity by taking the first candidate; reading
`payload.session_id` instead of `payload.id`; reading the whole file instead of
the first line; and deleting each of the two call sites in turn. The two
call-site mutations are what make the wiring proved rather than asserted.

Failure/isolation evidence:
- Ambiguity records nothing: `Discovered::Ambiguous` has no writer.
- `no_adapter_depends_on_the_session_model` — a new architecture scan: no
  adapter may name `crate::session` in production code, with
  `the_adapter_dependency_scan_would_catch_a_violation` proving it fires on a
  fabricated `use` and stays quiet on a doc comment.
- `a_discoverable_adapter_declares_discoverable_session_ids` — one-directional
  by design: Cursor, Hermes, Pi and OpenCode correctly declare
  `SessionIds::Discoverable` about their own harnesses without Glasshouse
  having built a reader for each, so the converse is not a defect.

Platform/external evidence:
- Codex 0.149.0 on macOS. The rule was derived from **all 555 real rollout
  files** in `~/.codex/sessions`: every first line is `session_meta`;
  `payload.id` is present in 555 and always equals the filename UUID, while
  `payload.session_id` is present in only 527; `originator == "codex-tui"`
  with no `parent_thread_id` selects exactly the 70 real interactive sessions,
  zero counterexamples. **Subagent rollouts share their parent's `cwd` and
  outnumber real sessions 171 to 70**, which is why matching on `cwd` alone —
  the previous session's plan — would have captured the wrong identifier most
  of the time.
- `CODEX_HOME` relocates Codex's entire state root (verified: an isolated home
  received `version.json`, `installation_id`, its sqlite files, `skills`,
  `tmp`), which is what makes the two end-to-end tests hermetic.
- Codex writes no rollout at all until a turn has happened — verified by
  starting bare `codex` in a pseudo-terminal under an isolated `CODEX_HOME`
  and killing it: `sessions/` was never created. That is why discovery runs at
  session end and why `NotFound` is an ordinary outcome, not a fault.

What CI caught, and it was a production defect rather than a test one:
- Linux and macOS passed; **Windows failed both end-to-end tests** on the first
  push (`32849379504`). `read_first_line` refused any first line not ending in
  `\n`, and the Windows fixture wrote its rollout with `Set-Content
  -NoNewline`. A harness writes its header before it has anything to append, so
  a record whose only line is a complete one is ordinary — Glasshouse was
  discarding it and reporting the session as having no identifier.
- Invisible to the eight unit tests because the `write_rollout` helper appends
  `\n` to every fixture. `a_header_with_no_trailing_newline_is_still_read`
  writes the bytes directly and fails against the old rule.
- A second change made at the same time — widening the header trim to
  `str::trim` for CRLF — was **removed after a mutation showed it dead**:
  `serde_json` already treats a trailing carriage return as whitespace.
  `a_header_terminated_by_crlf_is_read` stays, pinning the property rather than
  the mechanism, and its comment records that the mutation is what settled it.

Missing evidence:
- ~~No live Codex turn has had its identifier captured end to end.~~ **Closed
  2026-08-25.** A real `glasshouse launch codex` session took a turn, was quit,
  and the record captured `native_session_id =
  01a03983-b696-7832-ac49-296a4deccda1` — verified against the rollout Codex
  actually wrote for that project. Disposition read `resumable`. See the Codex
  hooks entry below.
- Two Glasshouse Codex sessions started in the same project inside the same
  window will each see the other's rollout and both refuse. Fail-closed and
  honest, but a real edge if anyone runs parallel Codex sessions in one
  project; the fix is a narrower discriminator, never a ranking rule.
- No real Codex session's identifier has been captured end to end, because
  that costs a model turn. The header format is proven against 555 real files
  and the wiring against the shipped binary; what is unproven is only the
  join between them on a live turn.

### Phase 8 — the Codex adapter's first three lines

Contract: Given a project and an enabled Codex, when the user opens a session,
Glasshouse starts the user's own installed `codex` in the project root and its
interface appears in the viewport as Codex drew it — while Glasshouse's own
code stays unable to reach inside Codex.

State: COMPLETE for starting Codex, preserving its interface, and staying
uncoupled from its internals. The rest of Phase 8 is open.

Production evidence:
- `harness/codex.rs: Codex` — names `codex`, starts bare ("If no subcommand is
  specified, options will be forwarded to the interactive CLI").
- The same selection and launch path as every other harness: `session::select`
  resolves it, `HarnessLaunch` derives the working directory from the project.

Regression evidence:
- `the_real_codex_interface_appears_in_the_viewport` (PTY smoke, Unix, opt-in
  via `GLASSHOUSE_PROBE_REAL_HARNESS=1`) — drives the shipped shell against the
  real Codex and asserts Codex's own version string reaches the viewport,
  having first asserted it is absent before any session exists so a match
  cannot come from Glasshouse's chrome. The probe is shared with Claude Code,
  so both harnesses are held to the same check.
- `glasshouse_depends_on_no_harness_internal_crate` and
  `the_dependency_guard_would_catch_a_coupling` — the manifest names no
  harness's internals, and the guard is checked against a fabricated manifest
  that does. Fabricated deliberately: adding a nonexistent dependency to the
  real manifest fails in cargo's resolver and proves nothing about the test.
- The existing launch smokes already start a fake harness under the `codex`
  slug and assert its working directory is the project root.

Platform/external evidence:
- Codex 0.149.0, driven through the viewport on macOS: its interface renders.
- **Codex asks a question Claude Code does not.** Its startup handshake is
  `ESC[>5u`, `ESC[6n`, `ESC[?u`, `ESC[c`, `ESC[0 q`. The `ESC[?u` is the kitty
  keyboard-protocol probe, and Glasshouse deliberately stays silent on it —
  see `DELIBERATELY_UNANSWERED` in `session/runtime.rs`. Replying would claim a
  protocol `tui::event` does not encode for, and the harness would then
  mis-read every keystroke. Silence is the correct answer *and* is not a
  timeout: Codex sends `ESC[?u` and `ESC[c` together, and the device-attributes
  reply arriving with no keyboard reply before it is the negative answer.
  Pinned by `the_keyboard_protocol_query_is_deliberately_unanswered` and
  `device_attributes_still_answer_after_an_unanswered_question`.

Missing evidence:
- Session-identifier capture, resume, hooks and compaction are open. Codex
  writes no rollout file until a turn has happened (verified: starting it and
  killing it left the session count unchanged), so identifier discovery has to
  wait for the first turn and match on the rollout header's `payload.cwd` and
  `payload.id`.

### Phase 7 — Keep terminal-text parsing only as a fallback for state that cannot be obtained structurally

Contract: Given a session whose state changes, when Glasshouse records it, the
change came from something structural — the operating system reporting the
process, or the harness reporting through a hook — and never from reading the
terminal and inferring.

State: COMPLETE. Glasshouse has no text-parsing fallback at all, which is a
stronger position than the line asks for.

Production evidence:
- `session/lifecycle.rs` translates harness events only, and cannot see
  terminal output.
- `session/runtime.rs: poll_exits` asks the process, never the output — a
  harness can be silent for minutes while thinking.
- The only writers of session state are the launch path and the shell (session
  started, stopped, failed) and the hook path.

Regression evidence:
- `nothing_derives_session_state_from_terminal_output` — the translator names
  no scrollback, screen or emulator type, and the runtime, which is the one
  place that *does* see terminal output, may not move a session's state.
  **Mutation-checked** by giving the runtime a method that infers a state and
  writes it: the test fails.

Missing evidence:
- None. If a future capability genuinely needs a text heuristic, this test is
  where the decision has to be argued rather than slipped in.

### Phase 7 — Record Claude compaction events when they can be observed reliably

Contract: Given a Claude Code session that compacts its context, when that
happens, Glasshouse records it.

State: NOT STARTED — **blocked by the harness, not by Glasshouse.**

Missing evidence:
- Claude Code 2.1.245 exposes **no compaction hook**. The event names a real
  installation was found accepting are `PreToolUse`, `PostToolUse`,
  `PostToolUseFailure`, `PermissionRequest`, `UserPromptSubmit`, `Stop`,
  `StopFailure`, `SubagentStart`, `SubagentStop` and `TeammateIdle`. None
  concerns compaction.
- Codex, by contrast, *does*: its recorded hook state carries `pre_compact` and
  `post_compact`, so Phase 8's equivalent line is reachable and this one is not.
- The capability says "when they can be observed reliably". They cannot be, in
  this version, and the only honest alternatives are to wait for a hook or to
  scrape the terminal for a compaction banner — which
  `nothing_derives_session_state_from_terminal_output` exists to prevent.
- Revisit when a Claude Code release exposes one.

### Phase 5/7 — the terminal handshake, and the defect it was hiding

Contract: Given a live harness session, when the harness asks its terminal a
question at startup, Glasshouse answers it, so the harness's own interface
works exactly as it would in a real terminal — and does not quietly degrade
itself, or the user's installation, when it does not.

State: COMPLETE. Phase 7's "Preserve the complete native Claude Code TUI
inside the Glasshouse PTY" box is now checked on the strength of this.

The defect, and how it was found:
- Driving the shipped shell against the real Claude Code 2.1.245, the viewport
  carried the harness's own notice: *"Claude Code's fullscreen renderer has
  repeatedly failed to start on this machine, so it has been turned off
  here."* The user's `~/.claude.json` had gained
  `fullscreenAutoDisabled = {"version": "2.1.245", …, "strikes": 2}`.
- So a Glasshouse session had made a harness permanently change the user's own
  installation, globally — breaking the product invariant that Glasshouse
  operates real harnesses without altering them. Worse, that user's
  `settings.json` reads `"tui": "fullscreen"`: Glasshouse had overridden an
  explicit preference.

The cause:
- A real Claude Code startup was captured in a pseudo-terminal. Of everything
  it writes before drawing, three sequences are *questions*: `ESC[6n`
  (cursor position), `ESC[c` (primary device attributes) and `ESC[>0q`
  (XTVERSION). The rest — bracketed paste, focus reporting, synchronised
  output, keyboard-protocol pushes — are instructions.
- Glasshouse answered exactly one of the three. Phase 5's own design note had
  already stated the rule ("an embedded session must always answer, or the
  harness hangs"); only the cursor-position half was ever built.

The fix:
- `session/runtime.rs: TerminalQuery` / `TerminalQueryScanner` recognise all
  three across chunk boundaries, and `answer_terminal_queries` replies to each:
  the emulated screen's cursor position, `ESC[?1;2c` for device attributes
  (what the viewport actually is, rather than a richer terminal whose sequences
  it could not draw), and its own name for XTVERSION rather than impersonating
  a terminal it is not.

Regression evidence:
- `every_startup_question_a_harness_asks_is_answered` (PTY smoke, Unix) — a
  harness asks all three through a real pseudo-terminal and every reply is
  found in its scrollback. **Mutation-checked three ways**: making either new
  query unrecognisable, or emptying the cursor reply, fails it.
- `a_query_is_found_however_a_read_splits_it`,
  `one_byte_at_a_time_still_finds_every_query`,
  `several_queries_in_one_chunk_are_all_found`,
  `a_near_miss_does_not_count_and_does_not_poison_the_next_match`,
  `a_reply_flowing_back_is_not_mistaken_for_a_question`.

Platform/external evidence (macOS, real Claude Code 2.1.245):
- **Before:** two sessions were enough to trigger `fullscreenAutoDisabled`.
- **After:** three consecutive sessions against an isolated Claude
  configuration left it **absent**, and the failure notice was gone —
  replaced first by Claude Code's *offer* of the fullscreen renderer, and then,
  with `"tui": "fullscreen"` set, by the fullscreen interface itself rendering
  in the viewport with no notice at all.
- The isolated configuration was used precisely so the verification did not
  touch the user's own; it was deleted afterwards.

Missing evidence:
- The user's real `~/.claude.json` still carries the `fullscreenAutoDisabled`
  record this defect caused. Glasshouse will not edit a harness's own
  configuration, so clearing it is the user's to do — `/tui fullscreen` in any
  Claude Code session resets it, and it also resets on the next update.
- Verified on macOS. The queries and replies are platform-independent and the
  tests run everywhere, but no real harness has been driven through the
  viewport on Windows.

### Phase 7 — Claude Code lifecycle hooks (one line closed, three pending one probe)

Contract: Given a running Claude Code session, when the harness reaches a
point it reports structurally — a prompt submitted, permission asked, a turn
ended — Glasshouse records the matching session state, while never editing the
user's own Claude Code configuration and never costing the user a turn if the
reporting fails.

State: **COMPLETE** for hook integration, translation and turn-completion
detection — all three observed end to end against the real harness. Permission
detection is PARTIALLY VERIFIED and its box stays unchecked; see the missing
evidence.

Production evidence:
- `harness/mod.rs: HookCommand`, `HookInstallation`,
  `HarnessAdapter::hook_installation` — the contract. The adapter builds the
  document because its *shape* is the harness's own business.
- `harness/claude_code.rs: hook_installation` — generates a settings document
  declaring `UserPromptSubmit`, `PermissionRequest`, `Stop` and `StopFailure`,
  installed with `--settings`, which loads *additional* settings so the user's
  own hooks keep running.
- `session/lifecycle.rs: lifecycle_for` / `may_apply` — the translation, and
  the only place that knows both vocabularies.
- `main.rs: report_hook` and the hidden `glasshouse hook` command — what the
  harness actually runs.
- `session/select.rs: HarnessSelection::install_hooks` — writes the document
  into a directory Glasshouse owns inside the project's own state. It never
  touches `~/.claude`.

Regression evidence:
- `an_installed_hook_moves_the_session_state` (PTY smoke, Unix) — launches
  through the shipped binary, reads the `PermissionRequest` command *out of the
  document Glasshouse generated*, runs it through a shell exactly as Claude
  Code would, and asserts the session moved to `WaitingForUser`. This is a test
  of the quoting as much as of the reporting. **Mutation-checked**: dropping
  the pinned paths from the command fails it.
- `a_hook_that_cannot_report_still_exits_zero` — an unknown session, a
  malformed identifier and an unrecognised event all exit 0.
  **Mutation-checked**: exiting non-zero on failure fails it.
- `the_generated_settings_document_is_valid_json_in_the_verified_shape` —
  parsed with `serde_json` and checked field by field against the shape read
  from a real Claude Code settings document.
- `a_hook_command_pins_every_path_it_needs`,
  `a_hook_command_survives_a_space_in_a_path`,
  `a_generated_document_escapes_backslashes`.
- `claude_codes_events_translate_to_the_states_they_mean`,
  `a_failed_turn_leaves_the_session_alive`,
  `an_unfamiliar_event_changes_nothing`,
  `a_finished_session_is_never_revived_by_a_late_hook` (**mutation-checked**),
  `a_live_session_follows_its_harness`.

Failure/isolation evidence:
- A hook **must** exit 0. Claude Code treats a non-zero exit as a veto: a
  `UserPromptSubmit` hook that exits non-zero blocks the prompt outright, with
  the user's own words echoed back and nothing sent. Observed directly against
  the real binary, which is why `report_hook` swallows every failure into the
  log.
- A late hook cannot revive a finished session: hook processes outlive their
  harness, and `may_apply` requires the current state to be live.
- An unrecognised event changes nothing rather than guessing a state.

Platform/external evidence:
- Claude Code 2.1.245 **fires hooks from a `--settings` document**: a
  hand-written document with a `UserPromptSubmit` hook ran its command and,
  by exiting non-zero, blocked the prompt — so the mechanism was proven
  without spending a turn.
- Claude Code 2.1.245 **does not fire `SessionStart`**: a document declaring
  one was installed and never ran, while `UserPromptSubmit` from the same
  document did. That is why `SessionStart` is not among the reported events,
  pinned by `session_start_is_not_among_the_reported_events`.
- Claude Code 2.1.245 **accepts the document Glasshouse generates**: started
  with one, it parsed it and reached its workspace-trust prompt.

- **Claude Code fires the generated document's hooks — run and observed.** A
  Glasshouse session was opened against the real `claude` in a pseudo-terminal,
  one prompt was submitted, and the session record moved from `starting` to
  **`idle`**.
- That value is conclusive rather than suggestive: the only production code
  that *writes* `Idle` is the `Stop`/`StopFailure` arm of
  `session::lifecycle::lifecycle_for`. Nothing else in Glasshouse can produce
  it, so the record could only have got there by Claude Code running the hook
  Glasshouse installed, which invoked `glasshouse hook`, which translated the
  event. The full chain — generate, install, fire, report, translate, record —
  is proven.

Missing evidence:
- **Permission detection specifically.** `PermissionRequest` is installed,
  translated and proven to move the record when its command runs, but Claude
  Code firing *that particular* event has not been watched: the verifying turn
  needed no permission, and this machine's Claude Code runs in auto mode, where
  a prompt that would ask is approved without asking. Its map box stays
  unchecked.
- Hook firing is verified on macOS only. The generated document and the
  reporting command are platform-independent and covered everywhere by the
  tests above, but Claude Code's own hook execution on Windows is unverified.

### Phase 7 — Support resuming a known Claude Code session through Claude Code's native resume mechanism

Contract: Given a recorded session that has a conversation to return to, when
the user resumes it, Glasshouse reopens that same conversation in the harness
that created it, while refusing anything it cannot honestly reopen.

State: COMPLETE.

Production evidence:
- `cli.rs: Command::Resume` and `main.rs: resume_session` — the command.
- `session/store.rs: SessionStore::resolve_id` — accepts any leading part of an
  identifier. Not a convenience: `glasshouse sessions` prints only the first
  twelve characters, so the short form is the *only* identifier a user can copy
  from the screen, and a command demanding all thirty-two would be unusable
  with what Glasshouse itself shows. Ambiguity is refused and names every
  candidate.
- `session/select.rs: HarnessSelection::resume_args` — the adapter's start
  arguments, then its resume invocation, then the user's own.
- `harness/claude_code.rs: resume` — `--resume <id>`.
- The harness is whichever one the *record* names, not whichever is configured
  now: resuming a Codex conversation in Claude Code would be nonsense.

Regression evidence:
- `a_recorded_session_is_resumed_under_the_identifier_it_was_given` (PTY smoke,
  Unix) — launches through the shipped binary, reads the assigned identifier
  from the harness's own arguments, takes the *short* identifier out of
  `glasshouse sessions`, resumes with it, and asserts the harness was handed
  `--resume` with that same identifier and **not** a fresh `--session-id`.
  **Mutation-checked**: resuming a different conversation fails it.
- `resuming_a_session_with_no_conversation_is_refused` — see the Phase 1 entry
  above; the harness is never started.
- `resuming_an_unknown_session_is_refused`.
- `the_short_form_the_listing_prints_is_enough_to_resolve`,
  `an_ambiguous_prefix_is_refused_and_names_its_candidates`,
  `a_wildcard_cannot_be_smuggled_into_the_lookup` — the last because
  identifiers are matched with `substr` and not `LIKE`, under which a bare `%`
  would match every session in the project.

Platform/external evidence:
- Against the real binary: `claude --resume <assigned-uuid>` reopened the
  conversation with its earlier turn replayed, and `claude --resume
  00000000-0000-4000-8000-000000000000` answered "No conversation found with
  session ID: …" and exited. Neither cost a model turn.

Missing evidence:
- A session that started and died before its harness created a conversation
  still reads as resumable, and resuming it will be refused by the *harness*
  rather than by Glasshouse. That is a clear message rather than lost state,
  and it is recorded as a loose end.

### Phase 7 — Add a Claude Code adapter that starts the real claude executable inside the current project root

Contract: Given a project and an enabled Claude Code, when the user opens a
session, Glasshouse starts the user's own installed `claude` with its working
directory set to the project root, while never substituting a different
program or a different directory.

State: COMPLETE.

Production evidence:
- `harness/claude_code.rs: ClaudeCode` — names `claude`, and `start()` is bare
  because `claude` run with no arguments "starts an interactive session by
  default".
- `session/select.rs: select` → `HarnessSelection::adapter` — resolves that
  name, or an explicitly configured path, refusing ambiguity.
- `main.rs: launch_session` and `shell/mod.rs: start_session` — both build the
  command through `HarnessSelection::start_args` and `launch::HarnessLaunch`,
  which derives the working directory from the active project and offers no
  way to override it.

Regression evidence:
- `a_claude_code_session_is_launched_and_recorded_under_one_identifier` (PTY
  smoke, Unix) — runs the shipped binary with `launch claude-code` and reads
  the argument list back from the harness itself.
- `the_working_directory_of_a_launched_harness_is_the_project_root` and the
  existing Phase 1 marker-harness smokes — the child's own `pwd` is the
  project root, on all three platforms in CI.
- `the_executable_names_match_the_installed_binaries` — the name is `claude`.

Platform/external evidence:
- `glasshouse doctor` on this machine resolves the real Claude Code 2.1.245 at
  its installed path and reads its version.

Missing evidence:
- None. Note that "the real claude executable" means whatever the user's
  configuration or `PATH` resolves; Glasshouse deliberately never bundles or
  substitutes one.

### Phase 7 — Capture the native Claude Code session identifier when it can be obtained reliably

Contract: Given a new Claude Code session, when Glasshouse starts it,
Glasshouse knows that session's native identifier and records it against the
session, while never recording an identifier the harness did not receive.

State: COMPLETE.

Production evidence:
- `harness/mod.rs: HarnessAdapter::assign_session_id` — the contract. Assigning
  beats discovering: an identifier chosen before the process exists survives a
  harness that dies during startup, needs no filesystem watching and no
  parsing, and cannot be confused with a session started at the same moment.
- `harness/claude_code.rs: assign_session_id` — `--session-id <uuid>`.
- `session/store.rs: SessionStore::new_native_session_id` and
  `uuid_v4_from_hex` — mints an RFC 4122 version-4 UUID from SQLite's
  randomness, the same source the store already uses for its own identifiers.
  Deliberately *not* derived from the Glasshouse session identifier: the two
  identifier spaces are independent by design.
- `session/select.rs: HarnessSelection::start_args` /
  `assigns_native_session_id`, and both production start paths, which mint the
  identifier, record it on the `NewSession`, and pass it to the harness.

Regression evidence:
- `a_claude_code_session_is_launched_and_recorded_under_one_identifier` (PTY
  smoke, Unix) — the shipped binary launches a harness that reports its own
  argument list, and the identifier found there is compared with the one read
  back from the store. **Mutation-checked twice, in both directions**: handing
  the identifier over without recording it fails, and recording it without
  handing it over fails. Either alone would be useless — an unrecorded
  identifier cannot be resumed, and an unhanded one names a conversation that
  does not exist.
- `assignment_agrees_with_the_declaration` — an adapter cannot hand out
  `--session-id` arguments while declaring its identifiers are only
  discoverable, or the reverse.
- `claude_code_assigns_the_identifier_its_binary_demands` and
  `a_harness_that_cannot_be_told_its_identifier_assigns_none`.
- `a_minted_native_identifier_is_a_valid_version_4_uuid`,
  `minted_native_identifiers_do_not_repeat`, and
  `the_uuid_formatter_only_overwrites_the_version_and_variant` — the last
  pins that 122 of the 128 bits survive, so the identifier keeps the
  randomness it was given.
- `launching_a_harness_records_a_session_that_a_later_command_reads_back` — a
  cleanly stopped Claude Code session now reads as **resumable**, the first
  time any session reaches that disposition in production. It read `closed`
  before, which was correct then and is wrong now.

Platform/external evidence:
- Claude Code 2.1.245 **rejects** a non-UUID outright: `claude --session-id
  not-a-uuid` answers "Error: Invalid session ID. Must be a valid UUID." The
  requirement is enforced by the binary, not merely documented, which is why
  the minted identifier is a strictly valid version-4 UUID rather than merely
  UUID-shaped.
- Claude Code 2.1.245 **accepts** a Glasshouse-minted identifier: started in a
  pseudo-terminal with one, it came up and ran normally until killed, where an
  invalid one exits immediately.

- **The assigned identifier becomes the conversation's own — run and
  observed.** With the user's approval, `claude --session-id
  7f3a91c2-5d84-4e11-9a3b-6c0d2e8f41ab -p "..."` was run once in a scratch
  directory, and Claude Code wrote its transcript to
  `~/.claude/projects/<slugged-cwd>/7f3a91c2-5d84-4e11-9a3b-6c0d2e8f41ab.jsonl`
  — the assigned identifier exactly.
- **The identifier is resumable, and an unknown one fails cleanly.**
  `claude --resume 7f3a91c2-…` reopened that conversation with its earlier turn
  replayed, while `claude --resume 00000000-0000-4000-8000-000000000000`
  answered "No conversation found with session ID: …" and exited. Both were
  observed in a real pseudo-terminal; neither cost a model turn.

Missing evidence:
- None. The chain is closed end to end: Glasshouse mints an identifier, hands
  it to the harness, records the same one, and the harness both stores the
  conversation under it and reopens it on demand.

### Phase 6 — the harness adapter interface (eleven of twelve)

Contract: Given a harness Glasshouse supports, when anything in core needs to
know how that harness is started, resumed, messaged, interrupted, observed or
described, Glasshouse asks that harness's adapter and gets an answer derived
from the installed binary, while core itself stays unable to name any
particular harness.

State: COMPLETE for eleven of the phase's twelve lines. The
communication-style line is PARTIALLY VERIFIED and its box stays unchecked —
see its own entry below.

Production evidence:
- `harness/mod.rs: HarnessAdapter` — the contract, with `id`,
  `executable_candidates`, `start`, `resume`, `describe`, `message` and
  `interrupt`. The six verbs the map names map onto it: starting is
  `start`, resuming `resume`, messaging `message`, interrupting `interrupt`,
  observing is `describe().hooks` plus `describe().session_ids`, and
  describing is `describe` itself.
- `harness/mod.rs: adapter_for` — the registry. Total over
  `IntegrationKind::Harness`, `None` for everything else.
- `integrations/mod.rs: IntegrationId::executable_candidates` — **delegates to
  the adapter for every harness.** This is the production path that makes the
  phase's fixed requirement structural rather than aspirational: there is one
  place a harness's executable name lives, and it is the adapter.
- `session/select.rs: HarnessSelection::adapter` and
  `HarnessSelection::start_args` — the seam both start paths go through.
- `main.rs: launch_session` and `shell/mod.rs: start_session` — the two real
  session producers, both using `start_args`.
- `integrations/mod.rs: doctor_report` / `write_adapter_report` — `glasshouse
  doctor` prints every adapter's declarations. This is what keeps `describe`
  from being a data structure nothing reads; it is generic over the trait and
  cannot tell one harness from another.

Regression evidence (macOS, and Linux/Windows in CI — all are pure logic):
- `every_harness_has_an_adapter_and_nothing_else_does` — a harness added to
  the catalogue without an adapter fails here rather than at a user's launch.
- `the_catalogue_takes_harness_executable_names_from_the_adapter` — proves the
  delegation above; **mutation-checked** by restoring a hard-coded name to the
  catalogue, which fails it.
- `resume_shapes_match_the_installed_binaries` — pins all seven resume
  invocations, which genuinely differ (`--resume`, a `resume` subcommand,
  `--conversation`, `--session`). **Mutation-checked** by giving Codex Claude
  Code's flag.
- `the_executable_names_match_the_installed_binaries` — pins `agy` for
  Antigravity. **Mutation-checked** by removing it.
- `resume_passes_the_identifier_as_one_whole_argument` — an identifier is its
  own `argv` entry and always last, so a value starting with a dash cannot be
  re-read as a flag.
- `every_verified_declaration_cites_its_evidence` — a `Verified` declaration
  with no usable evidence string fails. This is the honesty rule made
  mechanical.
- `no_two_adapters_claim_the_same_executable_name` — two harnesses claiming
  one name would silently resolve as each other.
- `a_sessions_arguments_are_the_adapters_first_then_the_users` — the ordering
  rule, exercised against a test adapter that actually declares start
  arguments, since none of the seven real ones needs any today.
- `the_doctor_report_shows_each_adapters_declarations` — asserts the specific
  rows of one adapter's block, not a `contains` over the whole report.
  **Mutation-checked** by making the report loop print nothing.
- `the_doctor_report_describes_every_harness_adapter` — every adapter gets a
  block whether or not it is installed on the machine running the tests.

Failure/isolation evidence:
- `the_generic_pty_runtime_depends_on_no_adapter` and
  `the_session_model_depends_on_no_adapter` — scan the production source of
  `pty/mod.rs`, `pty/process.rs`, `session/runtime.rs` and `session/store.rs`
  for `HarnessAdapter`, `crate::harness` and `IntegrationId`. Comments are
  stripped first, because `session/store` *documents* that it stores an
  identifier's string form, which is the boundary working rather than
  breaking. **Mutation-checked** by adding a method returning an
  `IntegrationId` to `SessionRuntime`, which fails it.
- `the_dependency_scan_would_catch_a_violation` — the scanner's own
  non-vacuity check: it fires on code, and not on a doc comment or a test.
- `an_unverified_capability_is_not_treated_as_present` — `Declared::Unverified`
  reads as "cannot rely on it", never as present.

Platform/external evidence:
- Declarations derived on 2026-08-25 from binaries installed on this machine:
  Claude Code 2.1.245, Codex 0.149.0, Antigravity CLI 1.1.20, OpenCode
  1.18.22, Cursor CLI 2026.08.11-e8db854, Pi 0.73.1, Hermes Agent 0.15.1.
  Each adapter module names what was read.
- `glasshouse doctor` run from the built binary; its output is what surfaced
  two rendering defects (nested backticks, a source phrase that did not fit
  its sentence) that no unit test would have shown.

Missing evidence:
- `resume`, `message` and `interrupt` are declared and unit-proven but have no
  production caller yet: resuming is Phase 7/8's, messaging and interrupting
  are Phase 13/14's. Line 3 asks an adapter to *expose* the resume command,
  which it does; executing one is a later phase's line and is not claimed
  here.
- No adapter parses harness output yet, so line 12's guard currently protects
  a property nothing is pushing against. That is the point of installing it
  before Phase 7 rather than after.

### Phase 6 — Make each adapter declare which native communication-style mechanisms it supports and whether changing them requires a new or cleared native session

Contract: Given a harness with a native way to control how it talks to the
user, when Glasshouse needs to apply a response profile, the adapter names
that mechanism and says whether changing it costs the running session, while
never presenting an unverified guess as a mechanism.

State: PARTIALLY VERIFIED — **box deliberately unchecked.** The closing bar
recorded here on 2026-08-25 — one verified in-place mechanism, or a second
harness with a verified native mechanism of any kind — was retested on
2026-08-26 against newer installed binaries and is still unmet. A worker
batch strengthened the entry without closing it; that is recorded rather than
rounded up.

Production evidence:
- `harness/mod.rs: CommunicationStyle`, `StyleChange`, and
  `HarnessDescription::communication_style` — the declaration exists and every
  adapter fills it in.
- `harness/claude_code.rs` — declares output styles, supplied through the
  settings document `--settings` reads at startup, as `StyleChange::NewSession`.
- Each adapter's value is now a named `COMMUNICATION_STYLE` constant whose doc
  comment cites the artifact it was read from, so the declaration and its
  provenance cannot drift apart silently.

Regression evidence:
- `every_adapter_declares_its_native_communication_style_and_session_cost` —
  pins the complete seven-adapter table, so neither a new adapter nor a changed
  declaration can pass without being written down here.
- Mutation proof, re-run by the orchestrator on integrated `main` rather than
  taken from the worker's report:

      test harness::tests::every_adapter_declares_..._session_cost ... ok
      (mutate claude_code.rs NewSession -> InPlace)
      test harness::tests::every_adapter_declares_..._session_cost ... FAILED
      error: test failed, to rerun pass `-p glasshouse --lib`
      (restore)
      test harness::tests::every_adapter_declares_..._session_cost ... ok

  Before this test, flipping Claude Code's launch-only mechanism to `InPlace`
  passed every gate in the repository. That is the specific hole it closes.

Platform/external evidence:
- Native artifacts read on macOS on 2026-08-26: `claude` 2.1.246, `codex`
  0.149.1, `agy` 1.1.21, `opencode` 1.18.22, `cursor-agent`
  2026.08.11-e8db854, `hermes` 0.15.1. `pi` is absent from `PATH`.
- Claude Code, Codex and Antigravity are all newer than the versions the
  adapter module headers cite; the newer help output still documents no
  in-place communication-style mechanism for any of them.

Missing evidence:
- Six of seven adapters declare `Unverified`, because their installed binaries
  document no communication-style mechanism at all. Codex is the pointed case:
  the capability map names "Codex personalities" as an example, and Codex
  0.149.1's `--help` still exposes none.
- `StyleChange::InPlace` still has no instance, so the arm the enum exists to
  express is unexercised by any real harness.
- Closing this needs one verified in-place mechanism, or a second harness with
  a verified native mechanism of any kind. Re-reading `--help` has now failed
  to produce either twice; a third attempt should read a different artifact —
  an in-session command list or a settings schema — rather than repeat it.

Known limit, recorded rather than fixed:
- `Declared::Unverified` collapses two different claims: "this version's
  `--help` was read and documents no mechanism" (Codex, Antigravity, OpenCode,
  Cursor, Hermes) and "no artifact could be read at all" (Pi, absent from
  `PATH`). Only the evidence prose distinguishes them. Nothing consumes the
  distinction today; the first thing that will is a re-probe policy, and it
  will need a structural difference rather than a doc comment.

### Phases 12, 18 and 19 — the event log, portable checkpoints, and the wiring that had no caller (25 of 28)

Delivered by the `lead-record` team lead (Claude Code, Opus 5, effort high)
with two Sonnet subcontractors. 13 mutations, all run by the lead: 12 killed,
**1 confirmed survivor reported and explained**, and — the valuable part —
**three that survived on their first run and exposed claims nothing was
testing.**

Contract: Given anything worth remembering about a session, Glasshouse writes
it to an append-only project-scoped log that cannot be updated or deleted; and
given a session worth continuing, it writes a small portable checkpoint that
can bootstrap a fresh session in a *different* harness.

State: COMPLETE for 25 lines. Three stay open, below.

Production evidence:
- `database.rs` migration 5 — `lifecycle_events` and `checkpoints`, with
  append-only triggers (an `UPDATE` or `DELETE` on a logged event raises).
- `events/log.rs` — `EventLogSink` behind `EventBus::attach_sink`. `record` is
  a `try_send` that **drops and counts** rather than blocking: the publishing
  thread is sometimes draining a pseudo-terminal.
- `checkpoint/store.rs`, `checkpoint/git.rs` — the portable format, its size
  bound, and `GitPosition::detect`, which reads `.git/HEAD` and its ref
  directly, handling linked worktrees and packed refs, **spawning no
  subprocess**.
- `main.rs::report_hook` now calls `session::lifecycle::observe`, which is what
  finally gave `RawObservation` a production caller.
- `shell/mod.rs` — the TUI subscribes to the bus, and the duplicated
  `Stopped`/`Failed` split is gone: `ProcessExit::session_state` is the single
  definition of "did it crash".

Regression evidence:
- 980 tests. 13 mutations in a private `CARGO_TARGET_DIR`, `touch` before every
  build, `cp` restore, `diff -q` proving the restore byte-identical.
- **The three first-survivors are the most useful result in this batch**, and
  each exposed an untested claim rather than a weak mutation:
  - replacing `observe()` with a bare translation killed nothing, because the
    *stored* observation comes from a different line. Only the **debug log**
    was lost — which is the box's actual subject — and nothing tested it. That
    box would have been claimed on a mechanism with no coverage at all.
  - `git: None` killed nothing, because the round-trip test built a
    `Checkpoint` literal with `git` already filled in: it proved the field
    survives storage and nothing about reading a repository.
  - the tail's `observed_harness IS NOT NULL` filter had no test; it is what
    stops the interface showing every in-process event twice.
  All three tests were then written and all three mutations re-run and killed.

Platform evidence:
- **No CI** (practice §27). `scripts/ci-local.sh` green on all ten checks —
  and this is the first batch gated by a version of that script that **can
  actually fail** (practice §31). **Windows unexercised.**
- Driven through the shipped binary, including a checkpoint written while one
  harness was running bootstrapping a session in another — which is the only
  form of evidence that means anything for Phase 19's cross-harness line.

Map/design conflict, decided by the orchestrator:
- *Preserve raw adapter event payloads in debug logs* — the lead built the
  mechanism, refused to log a payload document, and put the reading to me
  rather than choosing. **Closed.** "Payloads" cannot mean the payload
  document: that document carries the user's prompt and the model's last
  message, and a standing decision plus a test forbid it reaching any log.
  Read as "what the adapter reports, raw and untranslated" the line is
  satisfied, the mechanism now has a production caller, and
  `the_debug_log_preserves_the_raw_observation_and_none_of_the_payload` is
  mutation-proven in both directions.

Orchestrator work to land it:
- `tests/memory_store.rs` — the `MAX(version)` rollback trap for the **third**
  time, in a third file, and the lead reported exact replacement values rather
  than touching a file it did not own. It also found the trap's opposite face:
  dropping the migration rows *without* dropping migration 5's tables makes the
  re-run fail with `table lifecycle_events already exists`.

Missing evidence — the three open lines:
- *Deliver lifecycle events to the orchestration layer* — there is no
  orchestration layer; Phase 14 is entirely open. A delivery path with no
  consumer does not deliver, which is the rule that kept the TUI line open
  until this batch gave it one.
- *Record Git commit identifiers associated with memory events* — no memory
  event exists on the lifecycle stream, and `memories.source_commit` belongs to
  Phase 21's extractor. `checkpoint::git::GitPosition::detect` is the cheap
  resolver that line needs; Phase 21 should use it rather than shelling out.
- *Request a checkpoint automatically at selected task boundaries* — **the
  confirmed mutation survivor.** It works end to end (three `task_boundary`
  checkpoints observed from separate hook processes against a running shell),
  but nothing covers the shell's run loop, so the mutation that repointed the
  detector at the wrong event survived a full suite. The lead said it would not
  object to the box staying open. Taken at its word.

### Phases 20, 22 and 23 — durable project memory, its lifecycle, and FTS5 search (31 of 34)

Delivered by the `lead-memory` team lead (Claude Code, Opus 5, effort high)
with two Sonnet subcontractors. 30 mutations, every one run by the lead;
**29 killed and one deliberate survivor, reported rather than hidden.**

Contract: Given work worth remembering, Glasshouse stores it in this project's
own SQLite database under one of six kinds and one of the lifecycle statuses,
lets a newer memory retire an older one without deleting it, and finds it again
by free text — never returning history unless history was asked for, and never
reaching another project's database.

State: COMPLETE for 31 lines. Three stay open, and none of the three is a gap
in this work.

Production evidence:
- `database.rs` migration 4 — `memories`, three indexes, the FTS5 index, and
  the same pair of project-isolation triggers migration 2 established for
  `sessions`.
- `memory/store.rs` — `ProjectMemory`, `MemoryStore`, the six kinds, the
  statuses, supersession that records its successor and refuses to name a
  memory that does not exist.
- `memory/policy.rs` — `admit()` refuses raw conversational filler and refuses
  to keep a step-by-step plan as a todo.
- `memory/search.rs` — BM25-ranked FTS5 search, FTS5 operator characters
  sanitized, `SearchScope::Current` by default so history is an explicit ask.
- `memory/snapshot.rs` — a bounded project snapshot whose `omitted` count is
  never silent.
- `cli.rs`/`main.rs` — `glasshouse memory search <query> [--history] [--limit N]`,
  added by the orchestrator (see below).

Regression evidence:
- 870 lib tests and four new integration suites, 0 failures.
- 30 mutations in a private `CARGO_TARGET_DIR` with `touch` before every
  build, `cp` restore, `diff -q` verified, each killing a named test in the
  target that runs it. None reported `could not compile`.
- **The survivor is the useful entry.** Removing `search`'s `project_id`
  filter killed no test — because isolation is structural (one database per
  project, plus triggers), so the filter is redundant defence in depth. The
  lead reported the survival rather than deleting the filter or inventing a
  test that would have proved nothing. A surviving mutation that is explained
  is worth more than a suppressed one.

Platform evidence:
- **No CI** (practice §27). `scripts/ci-local.sh` green on all ten checks —
  five of seven CI jobs. **Windows unexercised.**

Orchestrator work required to land this:
- **The CLI surface.** The lead built `search`, `get` and `snapshot` and could
  reach none of them: `main.rs`'s match over `&cli.command` has no `_` arm, so
  adding a variant to `cli.rs` alone does not compile, and both files were
  forbidden to it. It wrote the patch into its report instead of guessing, and
  asked whether the command should be flat or a subcommand. **Phase 48 answers
  that** — it names `glasshouse memory search <query>` — so it is a
  subcommand. Run against the real binary, not just tested.
- **Four tests in `session/store.rs` reconciled with migration 4.** Three were
  pinned constants. The fourth was not:

  **A real find, and not the one anybody predicted.** Both rollback tests
  simulated an older database by deleting *some* rows from
  `schema_migrations` — `= 3` in one, `IN (2, 3)` in the other. The runner
  resumes from `MAX(version)`, so once migration 4 existed those deletions
  left a **hole**: max was still 4, nothing re-applied, and the tests failed
  much later with `no such column: launch_profile` and `no such table:
  sessions`. Both now roll back a contiguous range and drop what migration 4
  added. The lead had predicted "change 3 to 4" for these two and stopped at
  the version assertion; the second failure only appears once the first is
  fixed. Re-running a worker's decisive observations found it (practice §23).

Missing evidence — the three open lines, and why each is right to be open:
- *Do not store obvious source-code facts* and *prefer storing information
  whose rediscovery would require significant exploration* — not decidable at
  the storage layer. Whether a statement is an obvious source fact is a
  judgment about the project that only the producer can make. A keyword
  heuristic would refuse real memories, admit fake ones, and produce a test
  that passed for the wrong reason. These belong to Phase 21's extractor and
  its evaluation.
- *Avoid returning mutually contradictory current memories without flagging
  the conflict* — half built, and the honest half is the flagging: once two
  memories are marked conflicted neither is returned as current, and a
  mutation proves the test notices if only one side is flagged. Nothing
  **detects** a contradiction yet, so no test can show Glasshouse avoids
  returning an *undetected* one, which is what the sentence promises. Phase
  21E's decision ladder is where the detector belongs.

Phase 26 remains entirely open, deliberately. Every one of its six lines says
**agent**, the operations exist only as a Rust API and a person-facing CLI, and
a property proven of a Rust API is not a property proven of a tool surface that
does not exist. It closes with Phase 43's MCP surface.

Known limit, recorded rather than fixed:
- `the_project_database_schema_has_nowhere_to_put_a_credential` pins the whole
  schema and asks each new column be confirmed unable to hold a secret. The
  lead **refused to certify that** for `memories.subject` and `memories.body`,
  which are free text, and it was right to. The test's own documentation now
  says what it can and cannot prove: it proves no column exists whose *purpose*
  is a credential and that adding one is a deliberate, recorded act. It cannot
  prove free text is clean. That control belongs to the producer, and is now
  written down as an explicit acceptance condition of Phase 21's extractor
  rather than inherited by assumption.

### Phases 12, 13 and 45 — the lifecycle event bus, the session API, and failure isolation (18 of 24)

Delivered by the `lead-events` team lead (Claude Code, Opus 5, effort high)
with two Sonnet subcontractors. 25 mutations, every one run by the lead.

Contract: Given any supported harness, when it does something worth knowing
about, Glasshouse records one normalized event; a quiet or exited process is
never mistaken for a finished turn; an orchestrator can list, inspect, message
and interrupt a live session without reaching into a harness; and one worker
dying takes nothing else with it.

State: COMPLETE for 18 lines. Six stay open, listed below with what each needs.

Production evidence:
- `events/mod.rs` — `LifecycleEvent`, harness-independent by construction and
  asserted so by a source scan of the module for six harness names.
- `events/bus.rs` — the publish path never blocks on a subscriber.
- `session/api.rs` — `SessionApi::{list, state, send_text, interrupt,
  recent_output}`. Every method resolves scope **before** liveness, so a
  session from another project is refused *as foreign*, not as dead — the
  weaker ordering leaks the existence of other projects' sessions.
- `session/recovery.rs` — recovery refuses an unknown task kind exactly as it
  refuses a destructive one, and refuses to accept an event history as task
  state.
- `session/runtime.rs`, `session/lifecycle.rs` — crash classification and the
  translator.

Regression evidence:
- 40 new lib tests and 5 integration tests; workspace 973 tests, 0 failures.
- 25 mutation proofs, each restored and the restore verified byte-identical,
  in a private `CARGO_TARGET_DIR` with a `touch` before each rebuild
  (practice §16). Three worth naming, because each writes a mistake that
  *compiles*:
  - `ProcessExited { exit } if !exit.is_crash() => Some(TurnOutcome::Completed)`
    — kills `a_quiet_process_that_exited_cleanly_reports_no_task_outcome` and,
    against a real child, `a_quiet_harness_that_exits_cleanly_is_never_reported_as_having_finished`.
  - a `TurnEnded { outcome: Completed }` added to `poll_exits` gated on
    `status.success()` — the exact inference the map forbids — kills
    `turn_completion_is_minted_in_exactly_one_place`.
  - `poll_exits` killing every remaining session when one ends — kills
    `one_worker_crashing_leaves_unrelated_sessions_running`, which asserts the
    survivors still *answer input*, not merely that they are listed.

Platform evidence:
- **No CI.** The Actions quota is exhausted (practice §27). All ten checks of
  `scripts/ci-local.sh` pass — macOS natively and ubuntu in a container —
  which is five of the seven CI jobs. **Windows is unexercised**; nothing here
  is evidence about Windows.

Map/design conflict, resolved:
- Phase 12 says to preserve raw adapter event payloads in debug logs. The
  standing decision "Codex lifecycle hooks — a payload not to read" says the
  opposite about the payload that exists: it carries the user's prompt and the
  model's last message, and a test proves no field of it reaches a log.
  `RawObservation { harness, event, detail }` preserves whatever an adapter
  hands it; the two shipped adapters hand it the event name and nothing else,
  because the payload rule is *adapter* policy. Mechanism satisfies the map,
  policy satisfies the decision — **and the box stays open anyway**, because
  `observe()` has no production caller yet.

Missing evidence — the six open lines and exactly what each needs:
- *Record every translated lifecycle event with session ID and timestamp* —
  needs one call in `main.rs::report_hook`, which was forbidden to the lead.
- *Deliver lifecycle events to the TUI without blocking the harness process* —
  the bus is production-live and proven non-blocking, but **nothing in the TUI
  subscribes**. A delivery path with no consumer does not deliver, by the same
  test that left the Memory settings box open.
- *Preserve raw adapter event payloads* — as above, no production caller.
- *Deliver lifecycle events to the orchestration layer* — not claimed.
- *Preserve the most recent checkpoint after a worker crashes* — Phase 19 does
  not exist, so there is no checkpoint to preserve.
- *Detect gateway failure separately from harness-process failure* — the lead
  rated this its weakest claim, noted it was not separately mutated, and said
  it would not object to it staying open. Taken at its word: an unmutated
  claim is not proof here.

### Phase 2D — the Routing settings section and its five policy controls (six of seven)

Contract: Given a user who wants to constrain how Glasshouse will route work,
when they open Routing settings, they can set the router model choice, a
maximum acceptable router latency, a maximum marginal cost per decision, a
free-resource preference and a premium-capacity reserve; each value is
validated, shows which layer supplied it, and persists to the layer they chose
without disturbing its siblings.

State: COMPLETE for the six routing lines. **The seventh, "Add a Memory
settings section", stays open** — see below.

Production evidence:
- `config/mod.rs` — `RouterLatencyMs` (`10..=60000`, default `2000`),
  `RouterCostMicroUsd` (`0..=1000000`, default `1000`), `PremiumReservePercent`
  (`0..=100`, default `20`), `RoutingModelChoice`. Each resolves project ->
  user -> default independently, so setting one field never promotes its
  siblings into another layer. Zero cost is a deliberate free-only ceiling and
  zero reserve disables reserve protection; values past the maxima are refused
  as likely unit errors rather than silently clamped.
- `shell/state.rs`, `shell/view.rs` — the Routing tab; `m`, `l`, `c`, `f`, `p`
  edit the five controls, and every displayed value carries its layer, per the
  standing provenance invariant.
- `shell/mod.rs: save_user_settings_with_routing` — routing edits go through
  the existing writer, and the project layer still requires the separate `W`
  confirmation. No new path to the repository-local file was created.

Regression evidence:
- `routing_policy_values_round_trip_layer_independently_and_reject_absurd_inputs`
- `routing_settings_validate_and_stage_every_policy_control`
- `routing_and_memory_sections_render_their_complete_honest_states`
- `routing_edits_persist_to_the_chosen_layer_without_clobbering_siblings`
- Four mutation proofs from the worker. The one decisive for *these* boxes was
  re-run by the orchestrator on integrated `main`, because the whole argument
  for closing them is that the controls do something durable:

      (make the staged max_cost field never reach the saved routing table)
      test routing_edits_persist_to_the_chosen_layer_without_clobbering_siblings ... FAILED
      error: test failed, to rerun pass `-p glasshouse --test settings_persistence`
      (restored)
      test routing_edits_persist_to_the_chosen_layer_without_clobbering_siblings ... ok

Design-decision conflict, resolved and recorded:
- A standing decision said the settings view ships only sections whose feature
  exists, and that the other four sections' **map boxes stay unchecked until
  then**. Nothing routes yet, so that rule read literally would keep these six
  boxes shut indefinitely for reasons belonging to Phase 34.
- `GLASSHOUSE_DESIGN_DECISIONS.md` now records the refinement: the test is
  whether using a control does something real and durable, not whether its
  consumer exists. Routing passes it; Memory does not. The original rule had
  never been tested against a section with real controls and no consumer,
  because Providers and Launch Profiles both shipped alongside their features.

Missing evidence — and why the seventh box is still open:
- The Memory section renders "Project memory is not available in this build.
  There are no memory settings to save." That is truthful and worth keeping;
  it tells the user the capability is absent rather than implying it is
  present. But **a section with no settings in it is not a settings section**,
  and the box asks for one. It closes when memory has something to configure.
- `lead-memory` is building Phase 20 now but owns `src/memory/**` only, not
  `src/shell/**`. Whoever owns `shell/` next inherits this box, and the honest
  placeholder is the handoff.
- The five policy values are stored and read back; **nothing consumes them
  yet**, and the UI says so — free preference is stated to apply only after
  capability, health, rate-limit and latency checks pass. The router that
  honours them is Phases 34-38.

### Phase 2B — Mark every detected integration as available, configured, unconfigured, unsupported-version, or unknown

Contract: Given any integration discovery finds on this machine, when
Glasshouse reports it, it carries exactly one of the five capability states,
and it never guesses "set up for use" from "present on disk".

State: COMPLETE

Production evidence:
- `integrations/mod.rs: IntegrationStatus` — the five states, plus `NotFound`
  for determinate absence (see the packet correction below).
- `integrations/mod.rs: config_evidence` — Antigravity now answers
  `ConfigEvidence::Unknown`. It had answered `Available` because `agy` was on
  `PATH` and no configuration signal was known; Antigravity needs a login
  Glasshouse cannot check, so that was a guess with a friendly name. `Unknown`
  records that detection ran and could not tell: `problems` stays empty and
  `is_usable()` stays true, because not knowing is not a fault.
- cmux and llama.cpp answer `Available`: they need no per-user credential, so
  presence really is availability.
- `integrations/mod.rs: detect_one_with_prober` — injects the version prober
  and the minimum-version lookup. `detect_one_with` keeps its old signature,
  so no existing caller changed.

Regression evidence:
- `tests/integration_status.rs` — `every_detected_integration_carries_one_of_the_five_capability_states`,
  `usable_detected_integrations_are_never_unsupported_version`,
  `unconfigured_and_unknown_with_executable_are_not_treated_as_problems`.
- Five further unit tests over the status branches and `config_evidence`.
- Eight mutation proofs from the worker. The box-decisive one was re-run by
  the orchestrator on integrated `main` rather than taken from the report:

      (map ConfigEvidence::Available -> IntegrationStatus::NotFound)
      test every_detected_integration_carries_one_of_the_five_capability_states ... FAILED
      error: test failed, to rerun pass `-p glasshouse --test integration_status`
      (restored)

Platform/external evidence:
- CI run `32969003195` on commit `c473bef`: **all seven jobs green** — `lint`,
  `test` and `msrv` on `ubuntu-latest`, `macos-latest` and `windows-latest`.

Packet correction, accepted:
- The packet demanded "exactly five states, no more". The map's line scopes
  those five to *detected* integrations; confirmed absence is a sixth,
  determinate answer, and both `shell/` and `onboarding/` match on
  `NotFound`. Removing it would have required editing `shell/`, which another
  worker owned at the time. `NotFound` stays, and the five-state invariant is
  asserted over detected integrations only — which is what the map says.

Missing evidence:
- `minimum_version()` returns `None` for every integration today, so the
  `UnsupportedVersion` branch is proven through the injected seam rather than
  by a real installed-but-too-old harness. The branch is reachable and tested;
  no released floor has been declared yet.

### Phase 2B — Detect Antigravity when a supported Antigravity CLI executable is present

Contract: Given a machine with the Antigravity CLI installed, when Glasshouse
runs discovery, it finds it and reports its version, while never resolving an
unrelated program that merely has a similar name.

State: COMPLETE.

Production evidence:
- `harness/antigravity.rs: Antigravity::executable_candidates` — `["agy",
  "antigravity"]`, reached through `IntegrationId::executable_candidates` by
  the existing `Discovery` pass.

Regression evidence:
- `the_executable_names_match_the_installed_binaries` — pins both names.
- `no_integration_is_searched_for_under_a_guessed_abbreviation` — replaces a
  test that asserted the *wrong* single name; keeps the hazard it guarded (no
  `ag`, which is the-silver-searcher on many machines).

Platform/external evidence:
- A real Antigravity CLI 1.1.20 was installed on this machine on 2026-08-25,
  and `glasshouse doctor` reported it as `[available]` at its Caskroom path
  with version `1.1.20` read by the normal `--version` probe.

Missing evidence:
- None for this line. Note that the *published package* links the binary onto
  `PATH` as `agy`; an install by another route may expose `antigravity`
  instead, which is why both names are searched.


### Phase 5 — native terminal embedding (complete, 8 of 8)

Contract: Given a live harness session, when Glasshouse draws it, the harness's
own interface appears as it drew it — colours, cursor, wrapping and control
sequences intact — and the harness's own commands, prompts and controls keep
working, while Glasshouse's chrome stays out of the way.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/session/runtime.rs`: each `LiveSession` owns a
  `vt100::Parser` fed by the reader thread; `answer_terminal_queries` replies to
  `ESC[6n`; `resize` moves the emulator grid and the child's pseudo-terminal
  together.
- `crates/glasshouse/src/shell/mod.rs`: `build_viewport_grid`, `cell_style` and
  `convert_color` — the single place vt100's colour model meets Ratatui's.
  The tick rebuilds the grid, answers terminal queries, and sizes the child from
  the viewport's inner rect rather than the outer terminal.
- `crates/glasshouse/src/shell/view.rs`: `GridView` draws the grid cell by cell.
  The border is dropped once a live grid exists, so the harness gets the whole
  area — the chrome is four rows and the harness's name is already in the title.

Regression evidence:
- `an_embedded_session_answers_the_cursor_position_query_itself` — a real
  harness asks, and receives exactly `ESC[1;1R`.
- `colours_bold_inverse_and_cursor_position_survive_the_conversion`,
  `line_wrapping_is_preserved_in_the_grid`, `a_hidden_cursor_is_not_shown`,
  `a_fresh_screen_converts_to_a_full_grid_of_blank_cells`.
- `the_viewport_border_is_dropped_once_a_live_grid_is_shown`,
  `the_viewport_does_not_panic_with_a_real_grid_at_absurd_sizes`,
  `a_cursor_outside_the_render_area_does_not_panic`.
- The cursor-query scanner is tested at every one of the five possible read
  splits, one byte at a time, on a near miss, and on `ESC ESC [ 6 n`.

Failure/isolation evidence — mutations, each observed to fail its target:
- Nothing answers the cursor query: the harness hangs for the full timeout.
- The reply uses vt100's zero-based cursor: emits `ESC[0;0R`, not a position.
- The scanner forgets a byte that begins a fresh match.
- Every colour converts to default; every modifier is dropped.

**Two findings worth recording.**

*The responder had no production caller.* It shipped in `a1fa6c0` called only
from its own test — exactly the standard applied to Phase 1 line 90 and to the
runtime boxes, missed by the orchestrator who wrote it and caught by the worker
implementing the rendering. An embedded harness sending `ESC[6n` at startup
would have hung in the real shell while every test passed. It now runs on the
tick.

*The viewport's clipping clamp is not observable.* Removing
`area.height.min(grid.rows())` changes no rendered frame: `Buffer::cell_mut`
refuses anything outside the buffer, and the chrome below the viewport is drawn
after it. A containment test written to catch this passed for the wrong reason —
render order, not clipping — and was deleted rather than kept. The clamp stays,
with a comment saying plainly that it is cheap insurance rather than the thing
keeping the frame intact, because the render order it currently relies on is not
a property the widget can see.

Platform/external evidence:
- CI `32830685235` on `79a0600` — green on Linux, macOS, Windows and lint, with
  the Windows job confirmed to have executed 321 lib and 33 PTY tests including
  the colour, wrapping and border tests by name.

Missing evidence:
- Fidelity is asserted against synthetic escape sequences, not against Claude
  Code's or Codex's real TUI. The stated bar is "usable", which only a real
  harness can settle. `vt100` was chosen partly because swapping to
  `alacritty_terminal` is a bounded change if it proves insufficient.

### Phase 2D — the settings view (nine of twenty lines)

Contract: Given a project session, when the user opens settings, they can see
and change which harnesses are enabled and where their executables are, with
each value's originating configuration layer shown, while leaving settings
returns them to exactly the session and mode they came from and nothing is
written into the repository without a distinct confirmation.

State: COMPLETE for the nine lines checked; the other eleven configure features
that do not exist.

Production evidence:
- `crates/glasshouse/src/shell/state.rs`: `Overlay::Settings`,
  `SettingsSection`, `HarnessRow`, `IntegrationRow`, `SettingsEdit`. Opened
  with `s` from control mode, closed with `Esc`, mode untouched by either.
  `state.rs` remains I/O-free; it returns `Action::SaveUserSettings` /
  `Action::SaveProjectSettings` and `shell/mod.rs` acts.
- `crates/glasshouse/src/shell/mod.rs`: `build_settings`,
  `apply_settings_edits`, `save_user_settings`, `save_project_settings` — the
  last going only through `config::write_project_config_with_consent`.
- `crates/glasshouse/src/shell/view.rs`: `render_settings`, showing each value's
  `config::Layer` as `(project)` / `(user)` / `(default)`.

Regression evidence:
- `shell::state::settings_tests` (12) — keymap and in-memory model, including
  that opening and closing settings leaves mode and session untouched.
- `shell::view::settings_tests` (8) — every displayed value carries a layer
  tag; the confirmation names the exact path; no decorative block elements.
- `tests/settings_persistence.rs` (4) — real filesystem, real `Runtime`.

Failure/isolation evidence — mutations, each observed to fail its target:
- `W` saving immediately with no confirmation.
- The confirmed save not calling the writer.
- A user-level save also writing into the project root.

**The cancel test was vacuous as first written, and this is the interesting
part.** It asserted that an untouched workspace contained no `.glasshouse`
directory without ever invoking the cancel path — the comment even said so.
Mutating `W` to save with no confirmation left it green. Rewritten to drive the
keys and act on the result, it still passed, because it kept only the answer to
the *cancel* key and discarded the answer to `W` — which is exactly where a
missing confirmation saves. Only when it acts on **every** action, as the run
loop does, does the mutation fail it. Two rounds of a test that looked right
and proved nothing.

Secrets: no configuration type has a field able to hold one
(`config::tests::serialized_form_has_no_secret_capable_field`), so the settings
view cannot render a secret value. Structural, not a display rule.

Platform/external evidence:
- CI `32825574631` on `473d2b0` — green on Linux, macOS, Windows and lint.

Missing evidence:
- Eleven lines unchecked: Providers, Launch Profiles, Routing (six lines) and
  Memory sections, and provider/launch-profile management. Each configures a
  feature that does not exist (Phases 9C, 9A, 9I-9K, 20). Building the sections
  now would ship dead controls — see `GLASSHOUSE_DESIGN_DECISIONS.md`.
- The settings view has no end-to-end test through the shipped binary. The same
  differential-repaint limit that blocked the multi-session test applies.

### Phase 5 — the input half of native terminal embedding

Lines: "Preserve native harness input behavior instead of replacing it with a
Glasshouse chat composer"; "Allow native slash commands to pass directly to the
underlying harness"; "Add an escape key sequence that temporarily captures input
for Glasshouse-level navigation without permanently stealing input from the
harness".

Contract: Given a session on screen, when the user types, every keystroke
reaches the harness as the bytes its own interface expects — including the keys
Glasshouse binds for itself — while one reserved chord borrows input for
Glasshouse and hands it straight back.

State: COMPLETE

These three are satisfied by the session-mode design (see
`GLASSHOUSE_DESIGN_DECISIONS.md`) rather than by new work, and are checked here
as reconciliation. The rest of Phase 5 is the *rendering* half and needs a
terminal emulator.

Production evidence:
- `crates/glasshouse/src/shell/state.rs: handle_key`, `encode` — in session
  mode the mode is consulted before any binding, and every key is encoded to
  the bytes a terminal would send. Glasshouse has no composer, no input buffer,
  and no interpretation of `/`.
- `crates/glasshouse/src/shell/state.rs: is_session_escape` — one chord,
  `Ctrl-]`, both platform spellings.

Regression evidence:
- `a_slash_command_passes_straight_through_to_the_harness` — every character of
  `/compact` forwarded verbatim.
- `keys_glasshouse_binds_elsewhere_belong_to_the_harness_in_session_mode` —
  `q`, `n`, `o`, `i`, Tab, Esc, Enter, Backspace and Up all reach the harness.
- `the_escape_captures_input_only_until_it_is_handed_back` — control mode's
  bindings work again, then input returns to the harness, with no session
  touched. "Temporarily" and "without permanently stealing", asserted.
- `the_shell_enters_and_leaves_session_mode_in_a_real_terminal` — the same
  round trip through the shipped binary on all three platforms.

Failure/isolation evidence:
- Mutation: consulting bindings before the mode makes `q` quit instead of
  reaching the harness.
- Mutation: accepting only one spelling of the escape chord fails the
  real-terminal test — the defect that shipped past a full unit-test suite.

Platform/external evidence:
- CI `32821964808` on `f77b9c8` — Linux, macOS, Windows and lint.

Missing evidence:
- The rendering half of Phase 5 is untouched. The viewport prints raw bytes, so
  escape sequences are shown rather than obeyed. Until an emulator exists,
  "native permission prompts remain interactive" and the colour/cursor/wrapping
  line stay unchecked — a prompt the user cannot read is not interactive, even
  if the keystrokes would reach it.

### Phase 3 — return from overlays to the active native session, and propagate resize to it

Lines: "Allow the user to return from Glasshouse overlays to the active native
session without terminating it" and "Preserve terminal resize events and
propagate the new dimensions to the active embedded terminal".

Contract: Given an overlay open over a live harness session, when the user
leaves it, they are returned to that same session still running, and a resize
of Glasshouse's window reaches that session's own terminal.

State: COMPLETE

Both lines were blocked until this session, for the same reason: there was no
live native session to return to or to resize. `session::runtime` supplies one
and `shell::run` drives it.

Production evidence:
- `crates/glasshouse/src/shell/state.rs: close_overlay`, `enter_session_mode` —
  leaving an overlay restores the previous mode; entering session mode closes
  any open overlay. Neither touches a process.
- `crates/glasshouse/src/shell/mod.rs: run` — `Event::Resize` calls
  `screen.on_resize` and `SessionRuntime::resize` for the focused session.

Regression evidence:
- `leaving_an_overlay_returns_to_the_active_session_without_ending_it`
- `entering_session_mode_closes_any_open_overlay`
- `entering_and_leaving_session_mode_never_touches_a_real_process` — spawns a
  real child and checks its pid and liveness across the switch.
- `resizing_the_shell_reaches_the_harness_terminal` (Unix, tests/pty_smoke.rs)
  — the harness is asked `stty size` before and after Glasshouse's own terminal
  is resized, through the shipped binary.

Failure/isolation evidence:
- Mutation: making Escape always quit fails the overlay-first test.
- The resize test initially failed for two different reasons, both instructive:
  first because it asked before the SIGWINCH had travelled (a test timing
  fault, not a defect), and then because the escape chord never matched — see
  the Phase 4 entry.

Platform/external evidence:
- CI `32821964808` on `f77b9c8` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 287 lib and 33 PTY tests,
  including the shell's mode machinery in a real terminal. The resize test is Unix-only: `stty` is the
  portable way for a shell harness to report its terminal size and Windows has
  no equivalent a `.cmd` harness can run. The underlying `PtyProcess::resize`
  is covered on all three platforms by `resize_reaches_the_operating_system`
  and `a_resize_is_visible_to_the_child_process`.

Missing evidence:
- none.

### Phase 4 — the multi-session PTY runtime (covers seven map lines)

Lines: stream PTY output into an in-memory buffer; send text programmatically
without focus; send interrupt signals; bounded scrollback per live session;
keep inactive sessions running; switching changes only presentation focus;
headless presentation mode.

Contract: Given several harnesses started in one Glasshouse, when any of them
is acted on, it runs, buffers its own output within a fixed bound, and accepts
text or an interrupt whether or not it is the one on screen, while changing
which session is on screen never starts, stops, or signals any process.

State: COMPLETE for six of the seven lines; see Missing evidence.

Production evidence:
- `crates/glasshouse/src/shell/mod.rs: run` — **the production consumer.** The
  shell owns a `SessionRuntime`, starts sessions with `n`, forwards keystrokes
  in session mode, forwards resize events to the focused session, polls exits
  on every tick, and renders the focused session's scrollback in the viewport.
- `crates/glasshouse/src/session/runtime.rs: SessionRuntime`, `LiveSession`,
  `Scrollback` — each session gets its own reader thread and its own bounded
  buffer; focus is a field, and `focus()` touches nothing else.
- **No production caller yet.** `glasshouse launch` still uses
  `session::attach`, which is correct for handing one harness the whole
  terminal, and the shell reads records rather than live sessions. Until the
  shell drives this runtime, these seven boxes stay unchecked — the same
  standard applied to Phase 1 line 90 and to three Phase 3 lines.

Regression evidence (all in tests/pty_smoke.rs, real processes, real pseudo-
terminals, written by a Sonnet worker in an isolated worktree and re-verified
here):
- `two_sessions_run_concurrently_with_independent_scrollback`
- `an_unfocused_session_still_receives_sent_text`
- `focus_changes_nothing_but_focus` — pids recorded before and after five
  focus changes.
- `a_headless_session_runs_but_cannot_be_focused`
- `exit_is_detected_with_no_output_at_all`
- `scrollback_stays_bounded_under_real_output`
- `closing_one_session_leaves_the_others_running`
- `keystrokes_reach_the_focused_session`
- Plus unit tests for `Scrollback`: eviction order, an oversized chunk keeping
  its tail, a severed multi-byte character dropped rather than mangled, escape
  sequences preserved, zero capacity.

Failure/isolation evidence — mutations run by the orchestrator, each observed
to fail the test it targets: letting a headless session be focused; requiring
focus before `send_text`; making the scrollback unbounded; making `close` kill
every session; removing focus recovery after a close; giving all sessions one
shared scrollback; making `poll_exits` report nothing.

Two mutations did **not** fail, and both were acted on:

- Mutating `close` to kill every session initially left
  `closing_one_session_leaves_the_others_running` green, because `is_running()`
  reads the status cached by the last `poll_exits` — a freshly killed survivor
  still reported itself running. The test now polls the operating system over a
  window instead, and the mutation fails.
- Mutating `poll_exits` to wait for end-of-file before asking the process left
  `exit_is_detected_with_no_output_at_all` green. A harness that prints nothing
  and exits has its output end at the same instant, so that test cannot tell
  "asks the process" from "waits for output, then asks". An attempt to build
  the discriminating case — a harness leaving a background child holding the
  pseudo-terminal open past its own exit — was written and then removed,
  because a direct probe showed macOS reports end-of-file on the master as soon
  as the foreground child exits regardless of the background holder. The
  capability's real risk, mistaking a silent-but-running harness for a finished
  one, is covered by `exit_is_detected_from_the_process_not_from_quiet_output`.

Platform/external evidence:
- CI `32819167010` on `bb4c383` — green on Linux, macOS, Windows and lint, with
  the Windows job confirmed to have executed 267 lib and 31 PTY tests including
  every multi-session test by name. Several concurrent ConPTY sessions with
  independent scrollbacks work, which was the platform risk worth checking.

End-to-end evidence through the shipped binary (tests/pty_smoke.rs):
- `a_keystroke_typed_into_the_shell_reaches_a_real_harness_and_comes_back` —
  `glasshouse` with no arguments, `n` starts a real harness, session mode hands
  it the keyboard, the typed bytes arrive and its reply is drained into the
  scrollback and drawn. The payload begins with `q` on purpose: in session mode
  that belongs to the harness, so a broken mode split would quit instead.
  Mutations caught: swallowing the bytes before `write_to_focused`; never
  refreshing the viewport from the scrollback.
- `resizing_the_shell_reaches_the_harness_terminal` (Unix) — asks the harness
  `stty size`, resizes Glasshouse's own terminal, asks again. Proves the chain
  from Crossterm's resize event to the child's pseudo-terminal is joined up,
  which nothing previously did.

**A real defect this found, that unit tests could not.** The session-mode
escape chord was implemented as `Ctrl` + `']'`, matching the synthetic
`KeyEvent` its unit tests constructed. Crossterm's Unix parser decodes the
control range `0x1C..=0x1F` arithmetically, so a real terminal's `Ctrl-]`
arrives as `Ctrl` + `'5'` and never matched — leaving the user in session mode
**with no way back**, which is precisely the failure the single-chord escape
exists to prevent. `is_session_escape` now accepts both spellings, with the
Windows path (virtual key codes, `']'`) and the Unix path (`'5'`) documented
and separately tested. Reverting to the single spelling fails the resize test.

Missing evidence:
- Three of the twelve Phase 4 lines stay unchecked, all for the same reason —
  no production caller: sending text to an **unfocused** session and sending an
  **interrupt** are both orchestrator operations (Phase 14), and nothing yet
  creates a **headless** session, because the shell always starts sessions
  embedded. The runtime supports all three and each is tested against real
  processes; what is missing is a caller, not a mechanism.
- The shell's own multi-session switching has no end-to-end test. One was
  written and removed: a full-screen Ratatui application repaints
  differentially, so a captured pseudo-terminal stream cannot be sliced back
  into frames without a real terminal emulator, and an assertion about "the
  viewport" silently reads every viewport ever drawn. Phase 5 needs an emulator
  anyway; that is when this becomes testable. Meanwhile the behaviour is proven
  at the runtime layer against real processes
  (`focus_changes_nothing_but_focus` records pids across five switches), and
  the shell's only route to switching is through that layer.

### Phase 4 — Implement a generic PTY-backed child-process abstraction for interactive harnesses

Contract: Given any interactive harness, when Glasshouse runs it, it does so
through one pseudo-terminal abstraction that hides the platform difference
between Unix PTYs and Windows ConPTY, while exposing spawn, input, output,
resize, signal, and exit uniformly.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/pty/mod.rs: PtyProcess`, `TerminalCommand`,
  `PtyOutput`, `ExitStatus` — the only route by which Glasshouse starts a
  child, reached in production from `session::attach` (via `glasshouse launch`)
  and from `integrations`' version probes.

Regression evidence:
- `streams_output_and_reports_a_successful_exit`, `reports_a_failing_exit_code`,
  `forwards_input_to_an_interactive_child`,
  `terminating_stops_a_long_running_process`,
  `dropping_a_running_process_kills_it`,
  `terminate_reaches_the_session_leader_under_job_control` — all in
  tests/pty_smoke.rs, all against real processes in real pseudo-terminals.
- `the_launch_command_opens_the_configured_harness_inside_the_project_root` —
  the shipped binary end to end.

Failure/isolation evidence:
- `signalling_an_exited_process_is_reported_rather_than_misdirected` and its
  unpolled variant — a signal is never sent to a reused process identifier.
- `dropping_reaps_a_child_that_already_exited` — no zombies.
- `pty::open_pty` retries allocation five times, side-effect free, after the
  macOS `openpty(3)` race was diagnosed.

Platform/external evidence:
- CI on every batch this session — Linux, macOS and Windows, with the Windows
  job confirmed to have executed the PTY suite.

Missing evidence:
- none. Verified by reading the code and the named tests rather than taken from
  a worker's inventory, which had marked two neighbouring lines satisfied on
  the strength of production paths their named tests did not actually cover.

### Phase 4 — Detect process exit independently from textual terminal output

Contract: Given a harness that produces no output at all, when it ends,
Glasshouse notices from the process itself, while a harness that is merely
silent is never mistaken for one that has finished.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/pty/mod.rs: PtyProcess::try_wait` / `wait`, which read
  the child's status and cache it so it is reaped exactly once.
- `crates/glasshouse/src/session/attach.rs: supervise` polls `try_wait`, never
  output quiet, to decide a session is over.

Regression evidence:
- `exit_is_detected_from_the_process_not_from_quiet_output` — a child that
  prints nothing and lingers: `try_wait` reports it still running, then reports
  its exit. Silence and completion are proven distinguishable.
- `the_launch_command_opens_the_configured_harness_inside_the_project_root`
  asserts a distinctive exit code (7, so neither generic success nor generic
  failure) survives to the shipped binary's own exit code.

Failure/isolation evidence:
- `signalling_an_unpolled_but_exited_process_is_reported_rather_than_misdirected`
  — exit detection and signalling agree about a process that ended without
  anyone having polled it.

Platform/external evidence:
- CI on every batch this session — Linux, macOS and Windows.

Missing evidence:
- none.

### Phase 3 — Build the main interactive interface with Ratatui and Crossterm

Contract: Given a terminal on both standard input and standard output, when
`glasshouse` is run with no arguments, it opens a full-screen interface that
answers the keyboard and restores the terminal on the way out, while a piped or
redirected run falls back to the plain summary rather than drawing into a file.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/shell/mod.rs: run` — the event loop, entered from
  `main.rs`'s no-argument arm behind an `IsTerminal` check on both streams.
- `crates/glasshouse/src/tui/mod.rs: Screen` — terminal ownership, restored by
  `TerminalGuard` on a normal return, an error, a panic, or a signal.

Regression evidence:
- `the_shell_opens_in_a_real_terminal_and_answers_the_keyboard`
  (tests/pty_smoke.rs) — the shipped binary in a real pseudo-terminal: the
  interface draws, `o` opens the overview, Escape leaves it, `q` exits cleanly,
  and the alternate screen is left behind.
- The whole `shell::state` and `shell::view` suite, driven without a terminal.

Failure/isolation evidence:
- Mutation: making the no-argument arm fall through to the summary instead of
  the shell fails the pty_smoke test.
- The test asserts the alternate screen was left, so an exit that stranded the
  user on a dead frame would fail.

Platform/external evidence:
- CI `32816717226` on `5da067a` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 257 lib and 23 PTY tests,
  including the real-terminal shell test, rather than merely reporting green.

Missing evidence:
- none.

### Phase 3 — Create a persistent top bar that shows the project name, project root, and active session

Contract: Given any shell screen, when a frame is drawn, the project name, the
active canonical project root, and the session currently presented are all on
it, while a terminal too narrow for the root keeps the tail that identifies the
project rather than the head.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/shell/view.rs: render_title`, `render_root`.

Regression evidence:
- `the_project_root_is_displayed_on_every_frame`
- `the_project_root_stays_visible_while_an_overlay_is_open`
- `a_narrow_terminal_keeps_the_end_of_the_project_root` and
  `a_wide_terminal_shows_the_whole_project_root` — asserted against the root's
  own row, not the whole frame.
- `the_shell_opens_in_a_real_terminal_and_answers_the_keyboard` — the root is
  checked in the real terminal's output, anchored to the `root ` field.

Failure/isolation evidence:
- Mutation: blanking the root fails both the unit test and the pty_smoke test.
  The first version of the pty_smoke assertion survived this mutation, because
  the project's name and its root's last component are the same string and a
  bare `contains` matched the title bar; it now anchors on the field.
- Mutation: truncating the root from the end instead of the start fails
  `a_narrow_terminal_keeps_the_end_of_the_project_root`. That test also
  initially survived, for the same reason, and now reads a single row.

Platform/external evidence:
- CI `32816717226` on `5da067a` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 257 lib and 23 PTY tests,
  including the real-terminal shell test, rather than merely reporting green.

Missing evidence:
- none.

### Phase 3 — Create a persistent session bar that lists currently known sessions

Contract: Given the project's recorded sessions, when a frame is drawn, every
one of them appears in the bar with the active one distinguished, while a
project with no sessions says so instead of showing an empty strip.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/shell/view.rs: render_session_bar`, over the records
  `shell::run` reads from `session::store`.

Regression evidence:
- `the_session_bar_lists_every_known_session`
- `an_empty_project_says_so_instead_of_showing_an_empty_bar`
- `the_shell_opens_in_a_real_terminal_and_answers_the_keyboard` starts two real
  sessions first, so the bar is drawn from records a real launch wrote.

Failure/isolation evidence:
- Mutation: dropping the per-session span fails the listing test.

Platform/external evidence:
- CI `32816717226` on `5da067a` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 257 lib and 23 PTY tests,
  including the real-terminal shell test, rather than merely reporting green.

Missing evidence:
- none.

### Phase 3 — Create a central viewport reserved for the active session terminal

Contract: Given a shell screen, when a frame is drawn, the central region is
reserved for the active session's terminal and describes what will occupy it,
while never drawing a convincing empty terminal for a session that is not
attached.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/shell/view.rs: render_viewport` — a bordered region
  sized by the layout solver, holding the active session's identity and an
  explicit note that the space is reserved.

Regression evidence:
- `an_empty_project_says_so_instead_of_showing_an_empty_bar` — with no session
  the viewport says so rather than looking like an idle terminal.
- `renders_without_panicking_at_absurd_sizes` — 1x1 through 200x60, with and
  without an overlay.

Failure/isolation evidence:
- Nothing here computes a size by subtraction, which is the usual way a
  "must not panic on a tiny terminal" claim fails; the 1x1 cases prove it.

Platform/external evidence:
- CI `32816717226` on `5da067a` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 257 lib and 23 PTY tests,
  including the real-terminal shell test, rather than merely reporting green.

Missing evidence:
- The viewport is reserved, not filled. Embedding a live harness terminal is
  Phase 5, and this deliberately does not fake it.

### Phase 3 — Create a compact bottom status bar for Glasshouse-level key bindings and status messages

Contract: Given any shell screen, when a frame is drawn, one row carries
Glasshouse's own key bindings for that screen, and a key that could not do
anything leaves a note beside them, while the bindings survive a terminal too
narrow for both.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/shell/view.rs: render_footer`
- `crates/glasshouse/src/shell/state.rs: ShellState::set_status`, set when
  navigation has nowhere to go and cleared by the next keystroke.

Regression evidence:
- `the_status_bar_always_shows_the_key_bindings` — including the overlay's
  different bindings.
- `the_status_bar_shows_a_note_next_to_the_bindings`
- `a_note_is_dropped_rather_than_crowding_out_the_bindings`
- `a_status_note_is_cleared_by_the_next_keystroke`

Failure/isolation evidence:
- Mutation: dropping the hint span fails the bindings test.
- Mutation: removing the status message fails the navigation test.
- Mutation: writing the note before the bindings fails the narrow-terminal
  test, which is the entire mechanism — the row clips on the right, so order
  decides what is lost.

Platform/external evidence:
- CI `32816717226` on `5da067a` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 257 lib and 23 PTY tests,
  including the real-terminal shell test, rather than merely reporting green.

Missing evidence:
- none.

### Phase 3 — Allow the user to move to the previous / next session with a keyboard shortcut

Contract: Given a project with several sessions, when the user presses Tab or
Shift-Tab (or Right/Left), the shell presents the next or previous session,
wrapping at either end, while changing nothing about any session itself.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/shell/state.rs: ShellState::next_session`,
  `previous_session`, reached from `handle_key`.

Regression evidence:
- `tab_moves_to_the_next_session_and_wraps`
- `shift_tab_moves_to_the_previous_session_and_wraps`
- `arrow_keys_navigate_the_same_way_as_tab`
- `navigating_changes_only_which_session_is_presented` — the session list is
  compared before and after, so navigation cannot be quietly mutating records.
- `navigating_with_fewer_than_two_sessions_explains_itself`
- `a_refresh_keeps_the_same_session_presented_even_when_the_order_changes` —
  the selection follows the session's identifier, not its index, so a
  background refresh cannot move the user to a different session.

Failure/isolation evidence:
- `an_empty_project_has_no_active_session_and_does_not_panic`
- Mutation: removing the no-op status message fails the explanatory test.

Platform/external evidence:
- CI `32816717226` on `5da067a` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 257 lib and 23 PTY tests,
  including the real-terminal shell test, rather than merely reporting green.

Missing evidence:
- Bindings are plain single keys because no native session owns the keyboard
  yet. When one does (Phase 5) they must move behind a prefix or a mode, or
  they will steal keystrokes the harness needs. `handle_key` is deliberately
  the only place that has to change.

### Phase 3 — Allow the user to open a session overview from the keyboard

Contract: Given any shell screen, when the user presses `o`, an overview opens
showing every session with the detail the bar has no room for, while the shell
stays visible around it and the active session is unchanged.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/shell/state.rs: ShellState::open_overview`
- `crates/glasshouse/src/shell/view.rs: render_overview` — drawn over the
  shell, not in place of it.

Regression evidence:
- `o_opens_the_session_overview_from_the_keyboard`
- `the_overview_shows_detail_the_session_bar_has_no_room_for`
- `leaving_an_overlay_returns_to_the_active_session_without_ending_it`
- `escape_leaves_an_overlay_first_and_only_then_leaves_glasshouse`
- `the_shell_opens_in_a_real_terminal_and_answers_the_keyboard` opens it with a
  real keystroke in a real terminal.

Failure/isolation evidence:
- Mutation: making Escape always quit fails the overlay-first test, which is
  what stops Escape closing Glasshouse from inside the overview.

Platform/external evidence:
- CI `32816717226` on `5da067a` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 257 lib and 23 PTY tests,
  including the real-terminal shell test, rather than merely reporting green.

Missing evidence:
- none.

### Phase 3 — Keep the visual design text-first and avoid decorative graph visualizations that do not expose actionable state

Contract: Given any shell screen, when a frame is drawn, it contains only text
and the box-drawing characters that frame it, and never a gauge, sparkline, or
bar chart.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/shell/view.rs` uses only `Paragraph`, `Block`, and
  `Clear`. No Ratatui chart, gauge, sparkline, or canvas widget is imported.

Regression evidence:
- `nothing_draws_with_block_elements_so_the_design_stays_text_first` — renders
  the shell, the overview, and a screen carrying a status note, and fails on
  any character in U+2580..U+259F. Ratatui's decorative widgets are all drawn
  from that block-element range, so a frame containing none of it cannot be
  rendering one. Border characters live in a different range and stay allowed.

Failure/isolation evidence:
- Mutation: adding a `load ▇▇▅▂▁` line to the viewport fails the test, so it
  is a real check rather than a restatement of intent.

Platform/external evidence:
- CI `32816717226` on `5da067a` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 257 lib and 23 PTY tests,
  including the real-terminal shell test, rather than merely reporting green.

Missing evidence:
- Mechanical rather than aesthetic: the test proves no block-element widget is
  drawn, not that the layout is well judged.

### Phase 1 — Display the active canonical project root prominently in the TUI

Contract: Given the interactive shell, when any frame is drawn, the active
canonical project root is on its own labelled row, on every screen including
behind an overlay, while a narrow terminal drops the head of the path and keeps
the tail that identifies the project.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/shell/view.rs: render_root` — a dedicated row, not a
  corner. The value comes from `Project::display_root`, the same canonical root
  every access-control decision uses.

Regression evidence:
- `the_project_root_is_displayed_on_every_frame`
- `the_project_root_stays_visible_while_an_overlay_is_open` — "prominently"
  cannot mean "until you open something".
- `a_narrow_terminal_keeps_the_end_of_the_project_root`
- `a_wide_terminal_shows_the_whole_project_root`
- `the_shell_opens_in_a_real_terminal_and_answers_the_keyboard` — proved in a
  real terminal, anchored to the `root ` field rather than to any word that
  also appears in the title bar.

Failure/isolation evidence:
- Mutation: blanking the root fails the unit test and the real-terminal test.
- Mutation: truncating from the wrong end fails the narrow-terminal test.
- Both assertions were vacuous in their first form and were tightened after the
  mutations exposed them.

Platform/external evidence:
- CI `32816717226` on `5da067a` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 257 lib and 23 PTY tests,
  including the real-terminal shell test, rather than merely reporting green.

Missing evidence:
- none.

### Phase 2 — Persist Glasshouse session metadata independently from the native harness session files

Contract: Given a harness session started by Glasshouse, when the session is
recorded, Glasshouse stores its own session metadata in the project database
and can read it back in a later process, while never parsing, depending on, or
being invalidated by whatever session files the harness keeps for itself.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/session/store.rs: SessionStore::create` — the only
  writer of the `sessions` table; reached from `main.rs: launch_session`, which
  records a session before the harness process exists.
- `crates/glasshouse/src/main.rs: session_report` — `glasshouse sessions` reads
  the records back in a separate process.
- `crates/glasshouse/src/database.rs: MIGRATIONS[1]` — the `sessions` table.
  `native_session_id` is nullable, so a record is complete before any harness
  has produced an identifier and stays valid after the harness's own history is
  deleted.

Regression evidence:
- `launching_a_harness_records_a_session_that_a_later_command_reads_back`
  (tests/pty_smoke.rs) — the shipped binary, a real pseudo-terminal, two real
  harness runs, then a second process reading the records. Executed on macOS
  locally and on Linux, macOS and Windows in CI.
- `a_session_is_recorded_and_survives_a_reopen_with_no_harness_involved` —
  the record is complete with no harness identifier and survives a reopen.

Failure/isolation evidence:
- Mutation: making `create` skip its `INSERT` fails the pty_smoke test.
- Mutation: dropping the post-exit `note_lifecycle` call fails it.
- `a_session_write_is_refused_when_the_project_binding_is_missing` — writes are
  refused rather than orphaned when the database has no project bound.

Platform/external evidence:
- CI `32815286487` on `3d606e3` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 228 lib and 22 PTY tests
  (a green tick is not proof the suite ran: when the lib target fails,
  cargo never reaches the integration tests).

Missing evidence:
- none.

### Phase 2 — Persist a mapping between Glasshouse session IDs and native harness session IDs when native IDs are available

Contract: Given a harness that reveals its own session identifier, when
Glasshouse records it, the identifier is stored against exactly one Glasshouse
session and can be read back, while a Glasshouse session identifier never
changes and no native session can be claimed by two Glasshouse sessions.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/session/store.rs: SessionStore::set_native_session_id`
  — attaches the identifier after creation, which is when harnesses reveal it.
- `crates/glasshouse/src/database.rs: MIGRATIONS[1]` — the partial unique index
  `sessions_native_id` over `(harness, native_session_id)` is what makes the
  column a mapping rather than an annotation.

Regression evidence:
- `a_native_session_identifier_can_be_attached_later_and_read_back`
- `one_native_session_cannot_map_to_two_glasshouse_sessions`
- `two_harnesses_may_use_the_same_native_identifier`
- `many_sessions_may_have_no_native_identifier_at_once`

Failure/isolation evidence:
- Mutation: dropping the unique index lets one native session be claimed twice.
- Mutation: narrowing the index to `(native_session_id)` alone makes two
  harnesses collide.
- Mutation: replacing `NULL` with an empty-string sentinel makes every
  unidentified session collide — the reason the column stays nullable.

Platform/external evidence:
- CI `32815286487` on `3d606e3` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 228 lib and 22 PTY tests
  (a green tick is not proof the suite ran: when the lib target fails,
  cargo never reaches the integration tests).

Missing evidence:
- No harness adapter captures a native identifier yet (Phase 7/8), so in
  production the column is currently always `NULL`. The mapping mechanism is
  complete and proven; what feeds it is a later phase.

### Phase 2 — Persist the harness type, creation time, last activity time, role, lifecycle state, and project identifier for every session

Contract: Given any recorded session, when it is read back, every one of those
six fields is present and accurate, while creation time never changes and last
activity time advances on every state change and every recorded interaction.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/session/store.rs: SessionRecord`, `SessionStore::create`,
  `SessionStore::set_lifecycle`, `SessionStore::touch`.
- `crates/glasshouse/src/main.rs: launch_session` — moves a real session through
  `Starting` -> `Running` -> `Stopped`/`Failed`.

Regression evidence:
- `every_required_field_is_persisted` — asserted by value against an injected
  clock, not by round-trip.
- `every_role_and_lifecycle_value_round_trips`
- `activity_time_advances_while_creation_time_stays_put`
- `sessions_are_listed_most_recently_active_first`

Failure/isolation evidence:
- Mutation: stopping `set_lifecycle` from touching `last_activity_at` fails the
  activity test.
- Mutation: recording every ended session as `Stopped` fails the pty_smoke
  test, because a failed harness stops being distinguishable.
- `the_schema_rejects_enum_values_it_does_not_define` — `CHECK` constraints
  reject a role, lifecycle, or presentation the schema does not define.
- `an_unrecognized_stored_enum_value_is_reported_rather_than_guessed` — a value
  written by a future build surfaces as a typed error naming the column, not a
  panic or a silent default.
- `touching_an_unknown_session_reports_it_missing_rather_than_inventing_one`

Platform/external evidence:
- CI `32815286487` on `3d606e3` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 228 lib and 22 PTY tests
  (a green tick is not proof the suite ran: when the lib target fails,
  cargo never reaches the integration tests).

Missing evidence:
- none.

### Phase 2 — Persist the process presentation mode for every session

Contract: Given a session presented embedded, headless, or externally, when it
is recorded and read back, its presentation mode is preserved exactly, while an
undefined presentation value cannot be stored at all.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/session/store.rs: SessionPresentation`, stored by
  `SessionStore::create` and shown by `main.rs: session_report`.
- Vocabulary is the map's own (Phase 10/11: "embedded, headless, or externally
  presented"), not invented here.

Regression evidence:
- `every_presentation_mode_is_persisted` — all three modes.
- `launching_a_harness_records_a_session_that_a_later_command_reads_back`
  asserts the presentation column reaches the listing.

Failure/isolation evidence:
- `the_schema_rejects_enum_values_it_does_not_define` covers `presentation`.
- `stored_values_honour_format_width_so_listings_align` — the `Display` impls
  use `Formatter::pad`, so the listing's columns cannot silently go ragged.

Platform/external evidence:
- CI `32815286487` on `3d606e3` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 228 lib and 22 PTY tests
  (a green tick is not proof the suite ran: when the lib target fails,
  cargo never reaches the integration tests).

Missing evidence:
- Only `Embedded` occurs in production today, because `glasshouse launch` is the
  only session producer. `Headless` and `External` arrive with Phase 4's
  headless mode and Phase 17's cmux panes.

### Phase 2 — Persist enough metadata to distinguish active, resumable, closed, and failed sessions

Contract: Given the stored metadata alone, when Glasshouse classifies a
session, it separates active, resumable, closed, and failed without consulting
any harness, while never reporting a session resumable when nothing was
recorded to resume it to.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/session/store.rs: SessionRecord::disposition` — derived
  from lifecycle plus the presence of a native identifier, deliberately not a
  second stored column that could disagree with the first.
- `crates/glasshouse/src/main.rs: session_report` — the STATE column of
  `glasshouse sessions`.

Regression evidence:
- `the_four_dispositions_are_distinguishable_from_stored_metadata` — all seven
  lifecycle states, with and without a native identifier.
- `launching_a_harness_records_a_session_that_a_later_command_reads_back` — a
  clean exit reads as `closed` and a failing one as `failed`, end to end.

Failure/isolation evidence:
- Mutation: treating a stopped session with no native identifier as resumable
  fails the disposition test.
- `a_stopped_session_with_no_native_identifier_is_not_resumable` and
  `a_live_session_is_not_resumable` — the refusals `open_for_resume` enforces.

Platform/external evidence:
- CI `32815286487` on `3d606e3` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 228 lib and 22 PTY tests
  (a green tick is not proof the suite ran: when the lib target fails,
  cargo never reaches the integration tests).

Missing evidence:
- none.

### Phase 2 — Never store provider credentials directly in the project memory database

Contract: Given the project database at any schema version this build produces,
when its full schema is enumerated, there is no column and no key/value slot in
which a provider credential could be stored, while any future schema change
that adds one fails the build's tests until it is deliberately reviewed.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/database.rs: MIGRATIONS` — the complete schema is three
  tables: `project_metadata`, `schema_migrations`, `sessions`. None has a
  credential column.
- `crates/glasshouse/src/session/store.rs: NewSession` — the only way to create
  a session, and it has no field a secret could be passed through.

Regression evidence:
- `the_project_database_schema_has_nowhere_to_put_a_credential` — asserts the
  exact `(table, column)` list. Deliberately an allowlist rather than a name
  pattern: `project_metadata.key` would false-positive on any name match, and a
  credential column could just as easily be called `value`. Any new column
  fails this test until someone updates the list, which is the moment to ask
  whether it can hold a secret.
- `project_metadata_holds_only_the_project_identifier` — the one key/value table
  is pinned to its single known key, closing the route by which a secret could
  be stored without a schema change.

Failure/isolation evidence:
- The test fails by construction on any schema addition; it is an exact
  equality, so it cannot pass vacuously.

Platform/external evidence:
- CI `32815286487` on `3d606e3` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 228 lib and 22 PTY tests
  (a green tick is not proof the suite ran: when the lib target fails,
  cargo never reaches the integration tests).

Missing evidence:
- Provider credentials do not exist yet (Phase 9E). This entry proves the
  project database is not where they can land; it does not yet prove where they
  do land.

### Phase 1 — Reject any attempt to resume a Glasshouse-managed session whose project identifier differs from the current project identifier

Contract: Given a session record whose project identifier differs from the
active project's, when anything attempts to resume it, Glasshouse refuses and
names both projects, while leaving the record untouched and while the database
itself refuses to store such a record in the first place.

State: COMPLETE.

Production evidence:
- `session/store.rs: SessionStore::open_for_resume` — compares the stored
  project identifier against the active one before returning anything
  actionable.
- `main.rs: resume_session` — **the production caller this entry waited three
  sessions for.** `glasshouse resume` resolves an identifier and then goes
  through `open_for_resume` *before* a harness is selected and long before any
  process exists, so a refusal costs nothing and cannot half-start a session.
- `database.rs: MIGRATIONS[1]` — `BEFORE INSERT` and `BEFORE UPDATE OF
  project_id` triggers abort any row whose `project_id` is not the identifier
  bound in `project_metadata`. Structural, so no present or future query has to
  remember to filter by project.

Regression evidence:
- `resuming_a_session_belonging_to_another_project_is_refused` — the error names
  both projects and the planted record is left byte-for-byte intact.
- `the_database_refuses_to_store_a_session_from_another_project`
- `a_stored_session_cannot_be_reassigned_to_another_project`
- `a_stopped_session_of_this_project_can_be_resumed` — the permitted case, so
  the refusals above are not merely "resume never works".
- `two_projects_have_independent_session_lists`
- `resuming_a_session_with_no_conversation_is_refused` (PTY smoke, Unix) — the
  shipped binary refuses a session that has nothing to resume to, and the
  harness is never started. This is the test that reaches `open_for_resume` on
  the production path; `resuming_an_unknown_session_is_refused` does not,
  because the identifier resolver turns it away first.
- `a_recorded_session_is_resumed_under_the_identifier_it_was_given` (PTY smoke,
  Unix) — the permitted case end to end through the shipped binary.

Failure/isolation evidence:
- Mutation: removing the project comparison in `open_for_resume` fails the
  cross-project test.
- Mutation: dropping the `BEFORE INSERT` trigger fails the structural test.
- Mutation: weakening the trigger's `IS NOT` to `<>` fails
  `a_session_write_is_refused_when_the_project_binding_is_missing`, which is
  what proves the guard fails closed rather than silently passing a NULL
  comparison.
- Mutation: making `resume_session` read the record directly instead of through
  `open_for_resume` fails `resuming_a_session_with_no_conversation_is_refused`.
  **This mutation initially passed**, which is how it was discovered that the
  unknown-identifier test proved nothing about the guard — the resolver refuses
  first. The test above was written specifically to reach it.

Platform/external evidence:
- CI `32815286487` on `3d606e3` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 228 lib and 22 PTY tests
  (a green tick is not proof the suite ran: when the lib target fails,
  cargo never reaches the integration tests).

Missing evidence:
- None. Note that the cross-project case cannot be reached end to end through
  the binary, because the migration triggers refuse to store such a row in the
  first place — reaching it at all requires the test to plant one by tampering,
  which `resuming_a_session_belonging_to_another_project_is_refused` does. That
  is the guard being defence in depth, not a gap: the structural refusal is the
  first line and the comparison is the second.

### Phase 1 — Ensure every spawned harness process starts with its working directory set to the current project root

Contract: Whenever Glasshouse invokes an installed harness—including discovery
probes and interactive sessions—the child starts in the active canonical
project root and never inherits an unrelated caller directory.

State: COMPLETE

Production evidence:

- `main::launch_session` is the production consumer: `glasshouse launch
  [harness]` resolves a harness through `session::select::select` and starts it
  through `launch::HarnessLaunch`, which is the only route that reaches PTY
  spawn for a harness. This closes the gap recorded in the previous revision of
  this entry, where `HarnessLaunch` had no production caller at all.
- `session::attach::attach` runs the resulting session against the real
  terminal: raw mode, input/output pumps, resize forwarding, exit propagation,
  and restoration on every exit path.
- `launch::HarnessLaunch::spawn` reaches PTY spawn only through
  `TerminalCommand::for_harness`, which derives the directory from
  `Project::display_root` and is `pub(crate)`.
- `integrations::Discovery::run(&Project)` threads the active project into
  `version::probe_version`, which sets `Command::current_dir` from
  `Project::display_root`.

Regression evidence:

- `the_launch_command_opens_the_configured_harness_inside_the_project_root`
  (pty_smoke, macOS) runs the *shipped binary* in a real pseudo-terminal and
  matches the harness's own report of its working directory against the project
  root by filesystem identity. Glasshouse itself is deliberately run from a
  different directory, so an inherited cwd cannot pass.
- `a_fake_installed_harness_launches_inside_the_discovered_project_root`
  proves the same for the `HarnessLaunch` mechanism directly.
- `version_probe_child_starts_in_the_active_project_root` uses a resolved fake
  probe that prints a version only in the correct child directory.
- `project_configured_executable_wins_over_user_level` and the rest of
  `session::select::tests` pin executable precedence and every refusal path.
- Windows-only tests pin verbatim drive and UNC prefix conversion.

Failure/isolation evidence:

- The end-to-end test installs a *decoy* executable at the user level and the
  real one at the project level; a precedence failure runs the decoy and fails
  the test loudly rather than silently passing.
- `a_failing_configured_executable_never_falls_back_to_path` proves a broken
  configured path is an error, not a silent substitution of another binary.
- `attaching_without_a_terminal_is_refused` fails closed rather than hanging on
  a pty query nothing can answer.
- `PtyProcess::spawn` refuses a working directory that does not exist instead
  of starting the child somewhere else.
- Unsafe Windows-script arguments are rejected before `HarnessLaunch` spawns.

Non-vacuity (mutations actually run, each observed to fail the test):

- Removing the project layer from `EffectiveConfig::executable` → the decoy
  runs; the end-to-end test fails on precedence.
- Making `TerminalCommand::for_harness` use the process cwd instead of the
  project → no harness reports the project root; the test fails.
- Making `exit_code_for` always return success → the test fails on exit-code
  propagation.
- Setting `PTY_ALLOCATION_ATTEMPTS` to 1 → the pty retry test fails.

Platform/external evidence:

- **CI run `32788123095` on commit `f3effe6` is green on `ubuntu-latest`,
  `macos-latest`, and `windows-latest`, plus lint.** The Windows job was
  confirmed to have *actually executed* the tests cited here — 186 lib tests
  and 20 PTY smoke tests — rather than only reporting a green tick;
  `the_launch_command_opens_the_configured_harness_inside_the_project_root`
  and `a_fake_installed_harness_launches_inside_the_discovered_project_root`
  both appear as `ok` in the Windows log.
- Local macOS: `cargo fmt --check`, `cargo clippy -D warnings`, unit and PTY
  smoke tests, MSRV 1.85.0 `cargo check --locked`, `git diff --check`, and
  live CLI probes of `glasshouse launch` (help, no-terminal refusal, unknown
  harness, non-harness integration).
- An independent spawn-site inventory, re-run after the merge, found three
  production spawn sites, all project-bound, and zero production callers of
  the generic `TerminalCommand::new`.

What CI caught that local evidence had not:

- `cmd.exe` cannot open the verbatim `\\?\` path that resolving an executable
  produces on Windows, so **no `.cmd`-shimmed harness could start there at
  all** — and npm installs most of them that way. Fixed in `4aa31ad`.
- A prior revision of this entry cited a Windows job that had never run
  `tests/pty_smoke.rs`: when the lib target fails, cargo never reaches the
  integration tests, so the `.cmd` and verbatim-path claims were unproven
  while the entry implied otherwise. Confirming *execution*, not just the
  job's conclusion, is part of this evidence and not a formality.

Remaining caveats (recorded, not blocking):

- Native Windows UNC project roots are still refused rather than supported;
  `cmd.exe` cannot reliably hold a UNC working directory. This is a
  documented limitation of the contract, not a gap in it.


### Phase 2B — Detect cmux when a usable cmux executable or supported cmux control environment is present

Contract: Given a machine where the `cmux` executable is not on `PATH` but
Glasshouse is running inside a cmux surface, when discovery runs, cmux is
reported as detected and configured with the evidence that proves it, while
never being reported as launchable and never recording any environment
variable's value.

State: COMPLETE

Production evidence:

- `integrations::presence_without_executable` reads the cmux control
  environment (`CMUX_SOCKET_PATH`, corroborated by `CMUX_SURFACE_ID` and
  `CMUX_WORKSPACE_ID`).
- `integrations::detect_one_with` consults it in the `ResolveOutcome::NotFound`
  arm only, yielding `IntegrationStatus::Configured` with `executable: None`.
  The `Usable` and `Unusable` arms are untouched.

Regression evidence:

- `cmux_socket_path_set_yields_evidence_naming_it`,
  `cmux_corroborating_variables_are_also_named`,
  `empty_cmux_socket_path_counts_as_unset`, `no_cmux_variables_yields_no_evidence`
  pin the decision, over injected lookups so no test mutates the environment.
- `absent_executable_but_presence_evidence_is_configured_not_launchable` proves
  the wiring, including that `is_usable()` stays **false** — a detected
  integration with no executable must never be mistaken for a launchable one.
- `absent_executable_with_no_presence_evidence_stays_not_found` pins the
  unchanged behaviour for everything else.

Failure/isolation evidence:

- `evidence_notes_never_contain_a_value_only_names` fills every consulted
  variable with sentinels and asserts no note contains one.
  `CMUX_SOCKET_CAPABILITY` is a capability token and is never read at all.
- Non-vacuity (mutation actually run): making the `NotFound` arm ignore the
  presence evidence makes the wiring test fail.

Platform/external evidence:

- Live probe on macOS inside a real cmux surface, with `cmux` removed from
  `PATH`: `glasshouse doctor` reports `cmux [configured]` with the notes
  "candidates tried: cmux", "CMUX_SOCKET_PATH is set", "CMUX_SURFACE_ID is set".
- CI green on `ubuntu-latest`, `macos-latest`, `windows-latest`, and lint. The
  behaviour is environment-variable based with no `cfg` gating, so all three
  platforms execute the same code.

### Phase 2B — Detect Ollama when a usable ollama executable or configured local endpoint is present

Contract: Given a machine with no `ollama` executable on `PATH` but a
configured local endpoint, when discovery runs, Ollama is reported as detected
and configured, while never being reported as launchable and never recording
the endpoint's value — which can carry credentials.

State: COMPLETE

Production evidence:

- `integrations::presence_without_executable` treats a set, non-empty
  `OLLAMA_HOST` as a configured endpoint, wired through the same
  `detect_one_with` seam as cmux above.
- Deliberately no network request: discovery stays non-destructive and adds no
  HTTP dependency. The capability asks whether an endpoint is *configured*, not
  whether a server is answering.

Regression evidence:

- `ollama_host_set_unset_and_empty` pins set, unset, and empty-as-unset.
- The same wiring and non-launchability tests listed above cover Ollama, which
  is the integration they are written against.

Failure/isolation evidence:

- `evidence_notes_never_contain_a_value_only_names` covers `OLLAMA_HOST`.
- Live probe: `glasshouse doctor` with
  `OLLAMA_HOST=http://user:SUPERSECRET@127.0.0.1:11434` reports
  `Ollama [configured]` with the note "OLLAMA_HOST is set", and the whole
  report contains **zero** occurrences of the secret.

Platform/external evidence:

- Same CI run as the cmux entry above; no `cfg` gating, so all three platforms
  execute this code.

Missing evidence:

- None for the stated contract. Whether a configured endpoint is actually
  *reachable* is deliberately out of scope and would need a network probe.

### Phase 2A — Make unsupported platform/harness combinations fail with a clear diagnostic rather than a partial broken session

Contract: Given a platform and harness combination Glasshouse knows cannot
work, when a session or probe would otherwise be started, Glasshouse refuses
before any process exists and says what is wrong and what to do about it,
rather than starting something that appears alive while operating on the wrong
directory or the wrong process namespace.

State: COMPLETE

Production evidence — the combinations Glasshouse knows, and where each is
refused:

- **UNC project root + `.cmd`/`.bat` harness** — `launch::unsupported_combination`,
  called by `HarnessLaunch::build_command` before the command is returned, so
  `spawn` never reaches PTY creation. `cmd.exe` cannot hold a UNC working
  directory and does not fail when asked to: it substitutes the Windows
  directory and runs, so the session would have looked alive while operating
  outside the project entirely.
- **WSL + a Windows-interop executable** — `platform::exec::resolve_with`
  filters `/mnt/c`-style hits and returns `ResolveError::WindowsInteropOnly`,
  whose message explains that the child would run in the Windows process
  namespace where the project's Linux path is meaningless.
- **No usable executable** — `session::select::SelectionError::NotInstalled`
  names the candidate names that were tried.
- **A requested integration that is not a harness** —
  `SelectionError::NotAHarness` names the category and lists the harnesses
  that can be launched.
- **A harness turned off in configuration** — `SelectionError::Disabled`.
- **No terminal to attach to** — `session::attach::attach` refuses rather than
  hanging on a pty query nothing can answer.

Regression evidence:

- `a_script_harness_in_a_unc_project_is_refused_with_a_diagnostic` asserts the
  message names the directory, the reason, and the remedy — not merely that it
  failed.
- `every_other_combination_is_allowed` keeps the refusal narrow: a `Direct`
  harness in a UNC directory, and a script harness in an ordinary local or
  verbatim-drive directory, must all still launch.
- `unc_detection_covers_both_spellings_but_not_a_verbatim_drive` pins that
  `\\?\C:\...` is a local path despite also starting with two backslashes.
- `cmd_and_bat_are_windows_scripts_only_on_windows`,
  `a_nonsense_slug_is_unknown_and_names_the_valid_ones`, `cmux_is_not_a_harness`,
  and `attaching_without_a_terminal_is_refused` cover the other refusals.

Failure/isolation evidence:

- The refusal happens in `build_command`, before `PtyProcess::spawn`, so no
  process, pty, or terminal state exists when it fires. Non-vacuity (mutation
  actually run): disabling the condition makes the refusal test fail.

Platform/external evidence:

- CI green on `ubuntu-latest`, `macos-latest`, `windows-latest`, and lint. The
  check is a function of a path's shape and an executable's kind, with no
  `cfg` gating, so all three platforms execute it.

Honest limits:

- No real UNC share was exercised. The *refusal* is platform-independent code
  that CI runs everywhere; what is taken from documented Windows behaviour,
  not from a live run, is the premise — that `cmd.exe` would substitute the
  Windows directory rather than fail. That premise is why the refusal exists,
  and it was already recorded as a known limitation before this change.
- This capability covers the combinations Glasshouse currently knows about. A
  newly discovered one would reopen it.

### Phase 2A — Support native Windows as a first-class Glasshouse runtime where the selected harness is available

Contract: Given native Windows with an installed harness, everything Glasshouse
can currently do — resolve a project, isolate its state, discover harnesses,
probe versions, and open a real harness session inside the project root — works
the same way it does on macOS and Linux, and any combination that cannot work
is refused rather than half-started.

State: COMPLETE

This is a summary capability. It is checked because every capability it
summarises is checked and because the same test suite that backs the macOS and
Linux boxes now runs, and passes, on `windows-latest` — not because Windows was
judged by a weaker standard than its siblings.

Production evidence — the Windows-specific paths, all reachable in production:

- `pty`: ConPTY through `portable-pty`, with `process::JobHandle` giving
  Windows a kill-the-whole-tree equivalent that `TerminateProcess` alone does
  not provide — which matters precisely because a `.cmd` harness makes the real
  process a grandchild.
- `platform::exec`: `.exe`/`.cmd`/`.bat` classification, `cmd.exe /D /C`
  translation, `plain_script_path` conversion of the verbatim form `cmd.exe`
  cannot open, and rejection of `cmd.exe` metacharacters in arguments.
- `platform::paths::strip_verbatim_prefix` and `Project::display_root`: a
  canonical Windows root is verbatim, which is correct as an identity and
  unusable at a process boundary, so it is stripped there and only there.
- `launch::unsupported_combination`: refuses the one combination known not to
  work rather than starting a session that would run outside the project.

Regression evidence actually executed on `windows-latest`:

- CI run `32790669974` on `53e98f0`: **197 lib tests and 21 PTY smoke tests,
  0 failures**, plus lint, alongside green `ubuntu-latest` and `macos-latest`.
- `the_launch_command_opens_the_configured_harness_inside_the_project_root`
  runs the shipped binary in a real ConPTY and confirms a `.cmd` harness
  starts in the project root, with project-over-user executable precedence and
  exit-code propagation.
- `a_fake_installed_harness_launches_inside_the_discovered_project_root`
  exercises the real `cmd.exe /D /C` translation and asserts the canonical root
  was verbatim before `display_root` stripped it.
- `a_direct_executable_launches_through_the_harness_seam` covers the `.exe`
  branch, whose verbatim path had never been confirmed acceptable to
  `CreateProcess`.
- The PTY smoke suite proves output streaming, input, resize, and exit
  detection against a real ConPTY child.

Failure/isolation evidence:

- Windows CI caught two defects local gates could not: `cmd.exe` refusing the
  verbatim script path (so **no `.cmd` harness could start at all**), and a
  test comparing Windows path spellings that differ while denoting the same
  directory. Both are fixed and covered.
- Project-root refusals, the canonical-path guard, and per-project database
  isolation all execute in the Windows lib suite.

Honest limits — these are true on every platform, not Windows-specific:

- The interactive multi-session TUI does not exist yet (Phase 3), so
  "first-class runtime" means parity with macOS and Linux in what Glasshouse
  can do *today*, which is exactly the standard those two boxes were checked
  against.
- UNC project roots are refused for script harnesses rather than supported.

### Phase 4 unfocused control — "Support sending text programmatically to a PTY session without requiring the user to focus it."

Contract: Given several live sessions of which one is on screen, when the user
moves the overview's cursor to a session the viewport is not showing and sends
it a line, Glasshouse delivers that line to that session's pseudo-terminal,
while preserving which session the viewport and the session bar are presenting.

State: COMPLETE

Production evidence:
- `shell/state.rs: OverviewState::cursor` — a second, independent cursor. This
  is what makes the line real rather than nominal: before it, the overview
  highlighted `ShellState::selected_index()`, the same index `shell::sync_focus`
  hands to `SessionRuntime::focus`, so "the selected session" *was* the focused
  session by construction and an unfocused send was not expressible.
- `shell/mod.rs: send_session_text` — `Action::SendSessionText { id, text }`
  writes `"{text}\r"` through `SessionRuntime::send_text`. The bare `\r` is what
  a real Enter delivers and what `state::encode` already sends, so the harness
  cannot distinguish this line from a typed one.

Regression evidence:
- 16 new `shell::state::overview_tests` and 6 new `shell::view::tests` — cursor
  independence, that the bar keeps presenting what it was presenting, and that
  Tab still moves it underneath the popup. Executed on macOS.

Failure/isolation evidence:
- The focus-does-not-change assertions are the negative half: they fail if the
  send is implemented on the shared selection index.

Platform/external evidence:
- CI run `32957790931` on commit `9d9483b`: **all seven jobs green** — `lint`,
  `msrv` and `test` on each of `ubuntu-latest`, `macos-latest` and
  `windows-latest`.
- `an_unfocused_session_still_receives_sent_text` is a plain `#[test]` with no
  platform gate, so it executed on all three. The `overview_tests` module is
  pure state and likewise runs everywhere.

Missing evidence:
- None.

### Phase 4 unfocused control — "Support sending interrupt signals to a PTY session."

Contract: Given a live session the viewport is not showing, when the user
interrupts it from the overview, Glasshouse delivers an interrupt to that
session's process group, while preserving `Ctrl-C`'s existing meaning of
quitting Glasshouse itself.

State: LOCALLY VERIFIED

Production evidence:
- `shell/mod.rs: interrupt_session` — `Action::InterruptSession(id)` →
  `SessionRuntime::interrupt`, bound to `c` in the overview. Deliberately not
  `Ctrl-C`: stealing it would leave a user unable to exit.

Regression evidence:
- Two new interrupt tests, **`#[cfg(unix)]`**, executed on macOS.
- An explicit test that `Ctrl-C` still quits Glasshouse.

Failure/isolation evidence:
- `PtyProcess::interrupt` writes `ETX` (`0x03`) into the pseudo-terminal and
  relies on the Unix line discipline — or ConPTY's
  `PSEUDOCONSOLE_WIN32_INPUT_MODE` — to turn it into a process-group interrupt.
  Nothing added here is platform-specific.

Platform/external evidence:
- Pending. **The new interrupt tests are Unix-gated and will not execute on
  `windows-latest`**, so a green Windows run is not evidence for this line.

Missing evidence:
- A Windows-executed interrupt assertion. Until one exists this box stays open,
  because the product invariant is that PTY lifecycle is correct on every
  claimed platform, and ConPTY's path here has never been run.

### Phase 4 unfocused control — "Add a headless presentation mode in which a PTY continues running without occupying the visible session viewport."

Contract: Given a harness the user wants running but not drawn, when they start
it headless, Glasshouse runs it to completion and propagates its exit status,
while preserving the terminal for whatever else owns it and never orphaning the
child on a forced exit.

State: COMPLETE

Production evidence:
- `main.rs: run_headless` — `glasshouse launch <harness> --headless`, recording
  `SessionPresentation::Headless` and never claiming the terminal.
- `shell/mod.rs` — `N` starts one from the shell.
- `main.rs` — a `shutdown::on_forced_exit` registration bound to a named guard
  (`let _forced_exit`, not `_`), closing the session under `try_lock` per that
  module's non-blocking rule.

Regression evidence:
- 7 new `pty_smoke` tests against a real pseudo-terminal, executed on macOS.

Failure/isolation evidence:
- **A real defect, found by running the shipped binary against Claude Code
  2.1.246 and caught by no test:** `shutdown::install_signal_handler`
  force-exits when the terminal is not engaged, and a forced exit runs no
  destructor. A headless launch is the first path that both owns a PTY child
  and engages no terminal, so the real harness survived and was left running.
- The test that now covers it was itself defective and is the more useful
  finding: its fake harness was a plain `/bin/sh`, and a process exiting closes
  the pty master, so the kernel hung the shell up whether or not Glasshouse did
  anything. `trap '' HUP` makes the fake model what Glasshouse actually runs.
  Proved in both directions — hardened test on clean code `ok. 1 passed`;
  hardened test with the cleanup removed `FAILED`, "the harness (pid 61125)
  outlived the Glasshouse that started it".
- `shutdown.rs`'s registry is `Mutex<Vec<Cleanup>>` with ids from
  `NEXT_CLEANUP_ID`, `ForcedExitGuard::drop` doing
  `retain(|(id, _)| *id != self.id)`, and `run_forced_exit_cleanup` iterating
  `.rev()` under `try_lock` with each callback in `catch_unwind`. The
  single-slot hazard recorded in earlier handoffs is closed; this is its second
  caller and it is safe by construction. Note that a headless launch and an
  attached session cannot both be live in one process today, so two concurrent
  callers are not yet exercised.
- **A second defect, found by CI and fixed in `close_before_forced_exit`.** The
  first fix took a single `try_lock` on the runtime, and the headless poll loop
  takes that same lock every 20ms — so cleanup was a coin flip with no retry
  anywhere above it, and losing it orphaned the harness permanently. It
  surfaced as an intermittent red `test (macos-latest)` on `3ec4973` that
  passed on rerun against the identical commit, and was then **reproduced
  locally at 1 orphan in 100 runs under 3x CPU load**. `close` sends
  `ProcessSignal::Kill`, which is how the competing theory — the fake harness's
  `trap '' HUP` letting it survive — was eliminated by reading rather than
  guessing.
- The repair is a bounded retry, and it is covered by a **deterministic**
  regression rather than the 1-in-100 one: `hold_lock_for` takes the lock on
  purpose, so `a_forced_exit_cleanup_waits_out_a_briefly_held_lock` fails every
  time without the fix. Proved in both directions — the one-shot mutation kills
  it with "the cleanup gave up while the lock was merely busy".
  `a_forced_exit_cleanup_gives_up_rather_than_hanging` asserts the bound is
  honoured, because a forced exit that will not exit is the worse failure, and
  `a_single_attempt_loses_the_race_that_the_bound_wins` pins the pre-fix
  behaviour so it cannot quietly return.

Platform/external evidence:
- CI run `32957790931` on commit `9d9483b`: **all seven jobs green** — `lint`,
  `msrv` and `test` on each of `ubuntu-latest`, `macos-latest` and
  `windows-latest`.
- `a_headless_launch_runs_the_harness_without_taking_the_terminal` and
  `a_session_started_headless_runs_and_is_listed_but_never_reaches_the_viewport`
  are plain `#[test]`s with no platform gate: the behaviour this line actually
  claims executed on Windows, Linux and macOS.

Missing evidence:
- None for the claimed behaviour.

Known limit, recorded rather than hidden:
- `interrupting_a_headless_launch_does_not_leave_the_harness_behind` — the
  regression for the orphan defect — is `#[cfg(unix)]`. The forced-exit path
  exists on Windows and is unproven there. That is a robustness property rather
  than the text of this line, so the box is checked and the gap is named here.

### Phase 9F preflight — "Verify the selected harness, model, provider, and protocol combination before starting an interactive session when a cheap capability check is available." (line 465)

Contract: Given a direct-provider profile whose protocol and base URL make a
cheap check possible, when the user starts an interactive session, Glasshouse
confirms that this credential at this base URL answers for this protocol, while
preserving the launch when no check is available and never rerouting to a
different backend when one fails.

State: SCAFFOLDED

Production evidence:
- **None. `profile::capability_probe` and `profile::describe_probe_outcome`
  exist and are tested, but `main.rs` still calls `resolve_with_gateway`
  directly, so neither runs in the shipped CLI.** A mechanism with no
  production caller does not get its box; this one is queued as
  `task-GH-P09F-OFFERING.md` Part A.

Regression evidence:
- Unit tests over the real functions, one across a real loopback socket,
  executed on macOS. There is one request shape, not one per protocol: the
  protocol changes only the auth header (`x-api-key` vs `Bearer`) and the
  target is `GET <base>/models` when the provider declares a model-list
  endpoint, else `GET <base>`.

Failure/isolation evidence:
- `CapabilityProbe::Unavailable` for `BackendResource::Native` (no base URL or
  credential this crate can see) and for `GlasshouseGateway` (which upstream
  answers is Phase 9H's per-session routing decision, so probing the loopback
  listener would only prove Glasshouse is listening).
- `a_capability_probes_credential_never_reaches_this_modules_own_renderings` —
  the planted credential is absent from `ProbeRequest`'s `Debug` (present as
  `REDACTED`) and absent from the built URL. No new `.expose()` call was added
  in this batch.

Missing evidence:
- The production call site, and what the three outcomes actually render.

### Phase 9F preflight — "Require the selected coding harness executable to be installed and usable before offering an interactive direct-provider or gateway-backed launch profile." (line 466)

Contract: Given a direct-provider or gateway-backed profile whose harness is
not installed, when Glasshouse presents the list of launch profiles, it offers
that profile as unavailable and says why, while preserving the profile's
visibility so the user can see what to fix.

State: SCAFFOLDED

Production evidence:
- **None, and the reason is worth recording.** `profile::resolve_checked`
  exists and refuses correctly. The wiring proposed for it was `main.rs`'s
  `launch_session` — but `session::select::select` already resolves the
  executable (configured explicit path first, then `PATH`) and already returns
  `Err(SelectionError)` when none is usable, so at that point the answer is
  known good and the proposed call passed `ExecutablePresence::Usable`
  unconditionally. **The refusal would have been unreachable in the shipped
  binary.**
- The map's verb is *offering*, and the adjacent line 465 says *starting*. The
  call site this line names is the Launch Profiles settings section, where
  `ProfileRow` today carries `{ name, config, layer }` and has no notion of
  whether the harness behind it exists. Queued as `task-GH-P09F-OFFERING.md`
  Part B.

Regression evidence:
- Unit tests over `resolve_checked`, executed on macOS, including
  `the_executable_refusal_never_carries_a_credential`.

Failure/isolation evidence:
- The refusal names the harness and the candidates tried, matching the shape
  `glasshouse doctor` already uses:
  "launch profile `gateway` for Pi is backed by a direct provider, but Pi's
  executable is not installed and usable (candidates tried: pi); install it, or
  point Glasshouse at it, before this profile can be offered".

Missing evidence:
- The offering call site, and a test that the unusable profile stays *listed*
  rather than filtered out.

Known limit, recorded rather than fixed:
- `ExecutablePresence::detect` is `PATH`-only and does not honour a configured
  explicit executable path; `session::select` honours both. `resolve_checked`
  therefore takes the answer as a value rather than calling `detect` itself.
  Any offering call site must not reintroduce the disagreement.

---

### Phase 21 extraction contract — "Define a structured JSON schema…", "Feed the extractor bounded session/event chunks…", "Require the extractor to classify every emitted memory into one supported memory kind.", "Require the extractor to distinguish failed approaches from accepted decisions.", "Require the extractor to avoid duplicating an existing active memory when nothing materially changed."

Contract: Given a model reply, Glasshouse admits only elements that satisfy a
declared schema, refuses the rest by name, and never stores a memory the
project already holds.

State: COMPLETE

Production evidence:
- `memory/extract/schema.rs` — `RESPONSE_SCHEMA` plus a parser enforcing eight
  refusal rules. `the_response_schema_names_every_value_the_parser_accepts`
  pins the schema against `MemoryKind::ALL` and `MemoryAuthority::ALL`, so a
  class added to the store without being added to the prompt fails a test
  rather than silently never being asked for.
- `memory/extract/chunk.rs` — `SessionChunk::build` is the only constructor and
  applies three caps. The load-bearing one is the whole-chunk cap: a thousand
  entries each just under the per-entry cap is an unbounded history assembled
  out of bounded parts, which is exactly what the map's line forbids.
- Failed-versus-accepted is enforced as a consistency rule with teeth:
  `disposition: abandoned` ⟺ `kind: failed_attempt`, and any other pairing is
  refused as `ConflatedDisposition` rather than reclassified. Guessing which
  half a confused element meant would put Glasshouse's judgment behind the
  model's confusion.
- Duplicate detection normalizes case, whitespace runs and trailing sentence
  punctuation, against every active memory in the project *and* against what
  the run has already added. Deliberately nothing subtler: stemming would start
  deciding two different statements are the same, and a duplicate check that
  silently discards a real memory is worse than one that stores a near-duplicate.
- Reached from the shipped binary by `glasshouse memory extract`.

Regression evidence (mutation-proven, all run by the lead in a private
`CARGO_TARGET_DIR`, restored and verified with `diff -q`):
- M12 default an unknown `kind` to `finding`:
  `test memory::extract::schema::tests::every_memory_must_name_a_supported_kind ... ok`
  → `... FAILED` → `... ok`.
- M4 delete the whole-chunk character cap:
  `test a_whole_session_history_cannot_reach_the_model ... ok` → `... FAILED` → `... ok`.
- M9 delete the conflated-disposition refusal:
  `test memory::extract::schema::tests::an_abandoned_approach_cannot_be_filed_as_a_decision ... ok`
  → `... FAILED` → `... ok`.
- M7, M17, M18 (delete the duplicate branch; drop `to_lowercase`; drop the
  whitespace collapse) — all killed by
  `a_memory_the_project_already_holds_is_not_stored_again` and
  `a_reformatted_duplicate_is_still_a_duplicate`.
- M22/M23 make `memories` defaultable again — killed. The absent-key case is
  not pedantry: `extract_json_object` takes the first `{` wherever it sits, so
  a reply wrapped in an array had its inner object read as the whole envelope,
  found no `memories` key, defaulted to empty, and reported **"found nothing"
  with no failure at all** — indistinguishable from a model that looked and
  found nothing. Found by a subcontractor probing envelope shapes.

Failure/isolation evidence:
- Every refusal is per element, so one unreadable memory never discards the
  readable ones beside it. `Rejection::Store` renders a message rather than
  carrying the error, because the memory's text was screened before that point.

Known limit, recorded rather than fixed:
- **M19 survived and the filter was kept.** Replacing `WHERE project_id = ?1`
  with `WHERE ?1 IS NOT NULL` in the duplicate query kills no test: project
  isolation here is *structural*, since every project has its own database
  file. The filter is defence in depth against a future where one file holds
  two projects. This is the second independent lead to report this same
  survivor in this module. If a third does, the right answer is one test
  asserting the *structure* — that no two projects share a file — rather than
  three more survivors.

---

### Phase 21 credential acceptance condition — the extractor is never shown, and never emits, credential material

Contract: Given session activity or an already-stored memory containing a
credential, no credential reaches the model, and no memory carrying one is
stored.

State: COMPLETE

Production evidence:
- Three choke points, not one rule to remember. `SessionChunk::build` scrubs
  (so no chunk anywhere in the program holds un-scrubbed activity);
  `Prompt::build` scrubs the already-stored memories it quotes back (a row
  written before this module existed never passed a screen); `schema::judge`
  screens each emitted element **before reading any of its fields** and
  refuses it whole — so a credential in a field the contract does not even
  read is still caught.
- The two directions are deliberately asymmetric — **scrubbed in, refused
  out**. A session that printed a key still contains everything else the
  project learned that hour, so discarding the hour would lose more than it
  protects. A memory is small and discrete, so losing one costs one, and a
  *redacted* secret in a durable row still carries its neighbourhood.

Regression evidence:
- M1 drop the scrub on every entry → `the_model_is_never_shown_a_credential_from_session_activity` FAILED → ok.
- M2 drop the scrub on quoted existing memories → `the_model_is_never_shown_a_credential_from_an_already_stored_memory` FAILED → ok.
- M3 drop the output screen → `a_memory_carrying_a_credential_is_never_stored` FAILED → ok.
- M15 drop the assignment check from `screen` → `anything_scrub_removes_is_something_screen_refuses` FAILED → ok.
- **M14, the false-positive direction, and the one to point at if only one
  mattered.** Dropping the digit requirement on an assigned value makes
  `secret: memory-belongs-to-the-project` a credential, and a real memory is
  refused: `prose_that_merely_mentions_a_secret_is_not_an_assignment` FAILED →
  ok. An over-eager recognizer gets turned off, taking the protection with it,
  so this direction needs a mutation as much as the other one does.

---

### Phase 21 manual extraction — "Allow memory extraction to run manually for debugging and evaluation." (line 818)

Contract: Given a session's activity and a model reply, a person can run
extraction from the shipped binary and see what was stored, lowered, dropped
and refused.

State: COMPLETE

Production evidence:
- `glasshouse memory extract --session <id> --activity <path> --reply-from <path>`
  in `main.rs`. Everything except the model call is the production path: the
  chunk is bounded and scrubbed by `SessionChunk::build`, the reply goes
  through the same contract validation, credential screen, conservative
  classification and duplicate check, and what survives is written to the
  project's real memory store.
- Run against the shipped binary on a scratch project. Two memories in, both
  stored, and the second — declared `invariant` with `disposition: proposed` —
  reported as
  `lowered   1d35ff9d…  invariant -> idea (this was proposed and not accepted, so it is an idea and never an instruction)`.
- **`--reply-from` is a model *substitute*, not a model call, and the output
  says so on every run**: `model file (evaluation harness; no model was
  called)`. The configurable-model line above stays open, deliberately.

Regression evidence (`main.rs` unit tests, macOS):
- `test tests::a_manual_extraction_runs_the_whole_pipeline_and_says_no_model_was_called ... ok`
- MC `describe()` returns `"gpt-5.6"` instead of naming the file → `... FAILED` → `... ok`.
- MD feed the pipeline `std::iter::empty()` instead of the activity file → `... FAILED` → `... ok`.

Known limit, recorded rather than fixed:
- The orchestrator's judgment, recorded because the lead deliberately declined
  to make it: this line is closed and the neighbouring
  *"Keep memory-extraction failure non-fatal to the coding session"* is **not**,
  even though both turn on the extractor having a caller. A CLI invocation
  *is* a manual run for debugging and evaluation; it is **not** a coding
  session, and nothing is at risk when extraction fails inside it. Closing the
  second on this caller would be closing it on a caller its sentence does not
  describe.

---

### Phase 21A authority classes — all seven classes, classification by authority, conservative classification, explicit promotion (lines 828–841)

Contract: Given memories of differing authority, Glasshouse stores the class,
honours it distinctly, never lets automatic extraction mint an invariant, and
lets a person promote or demote explicitly.

State: COMPLETE

Production evidence:
- `MemoryAuthority` with seven classes, each round-tripping through SQLite
  unchanged, driven from `MemoryAuthority::ALL` so an eighth class fails a test
  rather than passing unnoticed. `is_binding()` and `MemoryStore::binding()`
  honour them distinctly.
- **`glasshouse memory search` prints the class.** This is the fixed
  architectural requirement — *retrieval must preserve the distinctions instead
  of flattening all memories into equally authoritative text* — and until this
  batch the one surface a person could reach dropped `authority` on the floor.
  An unclassified memory prints `unclassified`; it does not borrow a class.
- `glasshouse memory promote <id> <authority>` sets any class including
  `invariant`, as `Classifier::Reviewed` — the person typing it is the review
  the class requires. Demotion is never refused by either classifier: 21A's
  concern is memories becoming binding without anyone deciding they should, and
  requiring review to *demote* would leave an over-confident classification in
  place.
- **An extractor may not mint an invariant, at all**, and two independent
  controls enforce it: the producer cannot construct one (`EXTRACTOR_CEILING`
  is `Constraint`) and the store will not accept one from `Classifier::Extractor`.
  The map's line reads *"avoid promoting **uncertain** memories to invariants"*,
  which sounds like a certain one could be promoted. It cannot be, and the map
  answers this itself: Phase 21K requires model confidence to be treated as a
  presentation characteristic and never as evidence, so the only certainty an
  extractor has access to is not evidence of anything.
- `disposition` is what makes *"an idea discussed enthusiastically"* checkable
  rather than hoped for: `proposed` caps authority at `idea`, so no stated
  confidence can turn a proposal into a decision. Verified in a real binary
  run, above.

Regression evidence:
- `test tests::a_memory_search_names_the_authority_class_of_every_result ... ok`
  — drives all seven classes from `MemoryAuthority::ALL` plus an unclassified
  memory. MA (drop `{authority}` from the search line) → `... FAILED` → `... ok`.
- `test tests::a_person_can_promote_a_memory_and_demote_it_again ... ok`
  — promote to `invariant`, demote to `preference`, clear to `unclassified`,
  and refuse a class that does not exist. MB (`Classifier::Extractor` instead
  of `Reviewed`) → `... FAILED` → `... ok`.
- M13 remove the extractor ceiling:
  `test memory::extract::authority::tests::no_extraction_can_produce_an_invariant ... ok`
  → `... FAILED` → `... ok`. `no_input_triple_yields_an_invariant` walks all
  7 × 3 × 3 = 63 inputs.
- M21 remove the store's refusal:
  `test an_extractor_may_not_mint_an_invariant_and_nothing_is_written ... ok`
  → `... FAILED` → `... ok`. **Killed only by a subcontractor's test.**
- M20 remove `binding()`'s `is_binding` filter:
  `test binding_returns_only_active_binding_classified_memories ... ok`
  → `... FAILED` → `... ok`. **Also killed only by a subcontractor's test.**
- M8 store the declared authority rather than the conservative one:
  `a_model_cannot_write_an_invariant_into_this_project` FAILED → ok.

Known limit, recorded rather than fixed:
- `idea`'s *"must never be injected as binding instructions"* is half-proved:
  `is_binding()` is false and `binding()` excludes it, but the **injection**
  half is Phase 27 and unbuilt, so nothing can violate it yet. That is an
  absence of risk, not evidence, and it is recorded as such.

---

### Phase 21 — the five extraction lines that stay open, and why

Recorded so a later session does not re-derive them.

- *Allow a configurable cheap or local model to perform memory extraction.* —
  **Phase 39.** There is no way to call a model in this codebase.
  `ExtractionModel` is the seam and `ExtractionOutcome::model` is where
  Phase 39's *"record which resource performed important memory extraction"*
  lands. The seam is deliberately **synchronous** (this codebase has no async
  runtime, and extraction runs on a thread so it never blocks a PTY drain),
  `Send + Sync`, and its error type takes a `&'static str` so a provider's
  error body — which can echo the request, and the request is a prompt —
  cannot be routed into a Glasshouse diagnostic.
- *Require the extractor to omit speculative claims that were not established.*
  — **half-enforced, and the gap is real.** A memory marked
  `support: speculative` is dropped and counted (M11 killed). A memory
  *wrongly* marked `established` is stored, and no code here can catch it:
  whether a claim was established is a judgment about the session, which is
  the same thing `memory/policy.rs` already declined to fake at the storage
  layer. Needs extraction evaluation against a real model.
- *Require the extractor to preserve concise rationale when a decision's
  rationale is important.* — enforced where importance is decidable (a
  `decision` declared `invariant`, `constraint` or `decision` **must** carry a
  rationale or it is refused, M10 killed; capped at 400 characters for
  "concise"), but "important" is approximated by "binding", and there is no
  `memories.rationale` column, so it is folded into the body behind
  `RATIONALE_MARKER` and renders as `Why:` in a search. Findable, but a
  consumer cannot ask for a decision *without* its rationale.
- *Store the originating session and event references.* — the session half is
  done and proved; the **event** half has nowhere to go. The DDL is written
  and reviewed, below.
- *Allow memory extraction to run after task completion* / *before or around
  native prompt compaction.* — no caller exists for either. See the migration
  note and Phase 7/8 note below.

Migration ready to apply, deliberately not applied this batch:
- Three columns on `memories` — `source_event_first`, `source_event_last`
  (the `RecordedEvent::seq` range of the chunk; a range and not a single id,
  because extraction reads a slice and a memory is rarely traceable to one
  event; nullable, because a hand-written memory having no event range is a
  different fact from an empty one) and `rationale`.
- **It is migration 6, not 5** — `lead-record` took 5 for `lifecycle_events`
  and `checkpoints` in the batch immediately before this one. The DDL as
  written in that report says 5.
- **`rationale` can hold a credential, exactly like `subject` and `body`, and
  must not be certified otherwise.** The control stays on the producer:
  `judge` screens the whole element before reading any field, so coverage is
  automatic as long as `rationale` stays inside the element.
- `memories_fts` is an external-content index over `subject` and `body` only.
  Making `rationale` searchable is a **rebuild** of the index and its three
  triggers, not an `ALTER` — which is why this is a real migration rather than
  three column additions, and why it was not squeezed into this batch.

Blocked two phases deep:
- Extraction around compaction cannot be observed today from either harness.
  Codex's hook catalogue *has* `PreCompact`/`PostCompact` and
  `harness/codex.rs`'s `REPORTED_EVENTS` deliberately does not ask for them;
  Claude Code's observed catalogue does not list them at all. Phase 7 line 307
  and Phase 8 line 324 are the boxes that unblock it.
  `ExtractionTrigger::BeforeCompaction` exists and waits.

---

### Phase 9H — sticky gateway routing, 13 of 14 (lines 505–518)

Contract: Given a gateway-backed interactive session, Glasshouse assigns it one
provider and model at start, keeps it there across normal turns, moves it only
on a real provider failure to a backend that can actually serve the harness,
and says so when it does.

State: COMPLETE (line 511 excepted — see below)

Production evidence:
- `crates/glasshouse/src/routing/interactive.rs` — the policy, a pure function
  of values with no clock and no network. `crates/glasshouse/src/gateway/session.rs`
  feeds every finished exchange back into it.
- **The assignment is made on the production launch path**, in
  `profile::apply_gateway`, which `main.rs::launch_session` reaches through
  `resolve_with_gateway`. `main.rs` was not modified.
- **Verified against the shipped binary**, driven in a real terminal (the
  binary refuses a piped stdin, correctly, so a cmux pane is the only way):
  `glasshouse run claude-code --profile free-gateway` recorded
  `gateway backend: nvidia/nemotron-3-ultra-550b-a55b:free on openrouter
  (openrouter/OPENROUTER_API_KEY over anthropic-messages)` in its launch
  mechanisms, then forwarded two real exchanges to OpenRouter over Anthropic
  Messages, and the provider's status reached the harness byte for byte.
- No credential reached the log, checked mechanically rather than by eye:
  `grep -F "$OPENROUTER_API_KEY" gateway.log` found nothing, and no `sk-or-`
  prefix appeared anywhere in it.

Regression evidence — 25 mutations, 25 killed, three of them only after the
survivor forced a fix. The load-bearing ones:
- M2 the pin no longer stops failover → `a_pinned_session_does_not_fail_over_even_when_a_perfect_candidate_exists`
  and `gateway::conformance::a_pinned_session_stays_on_its_failing_provider_and_never_reaches_the_other_one` both FAILED.
- M3/M4 failover ignores protocol / tool semantics → `failover_never_crosses_a_protocol`,
  `failover_never_weakens_what_is_established_about_tool_calls` FAILED.
- M5 a different model is taken as a failover rather than offered as a
  migration → `a_different_model_is_offered_as_a_migration_rather_than_taken` FAILED.
- M22 the policy is asked on every turn rather than only after a failure →
  three conformance tests FAILED. This is the stickiness line: a router that
  re-decides each turn is not sticky even if it usually picks the same thing.
- M25 → `every_turn_goes_to_the_assigned_backend_and_a_free_alternative_is_never_connected_to` FAILED.

**The finding of the batch — a caller that every test bypasses is not a
caller.** M18 deleted `apply_gateway`'s call to `Gateway::routing().bind` and
**broke nothing**: all ten gateway conformance tests bound the assignment
themselves in their own helper, so the whole suite passed against a build in
which the production launch path recorded no assignment at all. Fixed by
`profile::tests::resolving_a_gateway_backed_profile_assigns_the_session_a_provider_and_a_model`,
which goes through the function `launch_session` actually calls; the mutation
then FAILED. M24 was the same shape one layer down — `to_launch_profile`
dropping the stored pin broke nothing, because the profile-side test built its
`LaunchProfile` by hand.

**A defect only the live run could find.** `402 Insufficient credits` was first
classified as a healthy exchange: the first version mapped `401`/`403` to
`CredentialRejected` and everything else to `Served`. A `402` is neither a
provider outage nor a malformed request — it is *this account's key* being
unable to pay, and another key on another account would serve. It now rotates
like `401`/`403` (M20, M21). No fixture would have produced a `402`, because
nobody would have thought to write one.

Known limit, recorded rather than fixed:
- **No live `200` was ever obtained.** OpenRouter answers `402` for `:free`
  models on an account that has never purchased credits, so nothing in this
  batch proves a free model *answered*. The free-pool health path is proven
  against fixtures and against a real `402`, not against a real success. No box
  was closed as if it had been.

Orchestrator judgement, recorded because the lead asked for it to be overruled
if wrong:
- **Line 518 is closed on a profile-level reading of "pin".** The user records
  a pin in configuration; it reaches the launch profile, round-trips (M24), and
  turns automatic failover off at session start (M23). The line says the user
  may pin a gateway-backed session and disable automatic failover, and that is
  what happens. A pin typed at a *running* session is a richer capability the
  line does not require; it would need `cli.rs` or a shell surface holding a
  handle on a live gateway, and that work is scoped in the lead's §7.2.

Not closable:
- **511** *explicit session migration at a task boundary.* Built and proven as
  a mechanism — `InteractiveRouting::migrate`, `SessionRouting::migrate`,
  `SessionActivity`, mutation M8 killed by
  `a_migration_is_refused_mid_turn_and_allowed_between_tasks` — with **no
  production caller**. Nothing in the shipped binary can ask for a migration.
  §5.

---

### Phase 9I — free-pool routing, 9 of 14 (lines 527–540)

Contract: Given free and metered resources, Glasshouse tracks their allowances
and health per credential, prefers free ones for disposable work, cools down
what keeps failing, and never spends metered capacity on its own automated runs
without an explicit opt-in.

State: COMPLETE (five lines excepted — see below)

Production evidence:
- `crates/glasshouse/src/routing/free.rs` and `routing/disposable.rs` — kept as
  a **separate policy class** from `interactive`, which is line 533 and the
  load-bearing structural requirement of the phase.
- **Keyed per credential, not per provider** (lines 537/538): two keys for the
  same router are two allowances, and one key's exhaustion is that key's limit.
  Fed by real gateway exchanges — a real `429` records `remaining: Some(0)`
  against that credential.
- Health comes from real workload, never from probes (line 534): `FreePool::observe`
  is the only mutator of health and its input is a finished exchange, so the
  quota a health check would protect is never spent checking it.
- The Settings Routing screen renders order, disable and pin (line 536), and
  renders a choice's reason through `UseReason`'s own `Display` — one spelling,
  not two.

Regression evidence:
- M9 key free-pool state per provider instead of per credential →
  `exhausting_one_key_leaves_the_other_key_of_the_same_router_alone` FAILED.
- M10 one failure is enough for a cooldown → `one_failure_is_not_a_cooldown_and_two_are` FAILED.
- M13 count a token-priced allowance down like a request pool →
  `a_token_priced_allowance_is_never_asked_how_many_requests_are_left` FAILED.
- M14 → `the_users_order_wins_and_a_disabled_resource_is_not_offered` FAILED.
- M15 a pin silently falls back when it cannot serve →
  `a_pinned_free_resource_that_cannot_serve_fails_the_job` FAILED.
- **Line 539 is an acceptance condition with the user's money behind it, and it
  has two mutations, not one.** M11 accept any opt-in value →
  `only_the_exact_opt_in_value_counts` FAILED; M12 let an automated run inherit
  `MeteredUse::Permitted` → `an_automated_run_cannot_inherit_permitted` FAILED.
  An automated run must opt in explicitly and exactly; it cannot arrive at
  permission by inheritance.

Not closable, and **four of the five share one blocker**:
- **530, 531, 532, 540** — the disposable policy class has **no production
  caller anywhere in the binary**. `DisposableRouting::choose` does exactly what
  line 530 describes and nothing calls it; `ShellState::record_disposable_choice`
  is 540's seam and nothing calls it; 532's *"only when adequate"* half is
  enforced and proven while the *"free models can back a launch profile"* half
  is unreachable because `gateway_upstream` builds every backend `Cost::Metered`.
  One `ExtractionModel` implementation that routes through this policy closes
  all four. That is one batch, not four.
- **528** — PARTIALLY VERIFIED. `Allowance` has one variant per kind with no
  shared arithmetic (M13) and the request-pool half has a real production feed;
  the token-priced half does not, because nothing reads a provider's pricing.
  Deliberately **not** solved by parsing rate-limit headers on the forwarding
  path: the gateway forwards headers without reading them, and a parser there
  would make it a reader of the payload it exists to pass through.

---

### Phase 9G refined — the several-providers refusal is lifted, and why that is not a reversal

`profile::gateway_upstream` used to refuse a configuration in which more than
one provider served the gateway ingress. It now assigns the first in the user's
configuration order and keeps the rest as failover candidates.
`GatewayUpstreamRefusal::SeveralProvidersServeTheIngress` is **removed**,
because an error variant that can never be produced is decoration (§20 applied
to an enum).

**This is the guard being retired by the phase it was holding a place for, not
overridden.** 9G's objection was to a *silent* choice at a time when no phase
owned the decision. 9H owns it now, and the choice is announced in the launch's
own mechanism notes (`category: "gateway backend"`, carrying provider and model
names and never a credential), pinnable (518), migratable in principle (511),
and recorded on every change (515).

Kept as a guard, it would have done the opposite of its purpose: a user with two
configured routers could not start a gateway-backed session at all, and **every
9H failover line would be unreachable by construction** — a temporary
placeholder converted into a permanent block on the capability it was
protecting. The alternative the lead offered — keep refusing until a launch
profile can name its own gateway provider, at the cost of a field on
`BackendResource::GlasshouseGateway` — is defensible and remains available if a
later phase needs per-profile provider selection.

---

### Phase 21 — migration 6, provenance, and a failure the coding session survives (lines 814, 816, 820)

Contract: An extracted memory carries the range of the event log it came from
and the rationale behind it; a person can run extraction over a session's own
recorded events; and extraction failing never costs the user their turn.

State: COMPLETE

Production evidence:
- **Migration 6** adds `source_event_first`, `source_event_last` and
  `rationale` to `memories`, and **rebuilds `memories_fts`** with `rationale`
  as a third indexed column plus its three triggers — the rebuild the previous
  batch flagged as the real work. `RATIONALE_MARKER`'s fold into the body is
  gone: the rationale is now its own column and its own line in a search.
- `memory/extract/lifecycle.rs` — `describe(&LoggedEvent)` and
  `chunk_for_session`, which builds a bounded, scrubbed chunk **that knows the
  range of the log it covers**.
- **`glasshouse memory extract --session <id> --from-events`** — a caller a
  person can run. `--activity` and `--from-events` are mutually exclusive and
  one is required, so extraction is never run over activity nobody chose.
- **The chunk's event range is narrowed to what survived the budget.**
  `SessionChunk::build` keeps the newest entries when the budget binds, so a
  chunk that dropped the first sixteen events must not claim a memory came from
  them. Tested independently by both subcontractors — unit and end-to-end — and
  the mutation widening the range back to the whole input slice is killed by
  both.

**Why the event log is the right source and not a consolation.** A hook payload
carries the user's prompt and the model's last message; Glasshouse's handler
drains that stream unread, and `lifecycle_events` has no column a conversation
could reach. **A chunk built from the event log cannot contain conversation
text because there is none to contain** — the credential and privacy properties
hold by construction rather than by a screen.

Regression evidence (box 820, and the failure path is the one that actually
runs in production today):
- `a_failing_extraction_model_costs_the_coding_session_nothing` — a refusing
  model and a panicking model; asserts the lifecycle still moved to `idle` and
  the event was still recorded.
- `an_extraction_model_that_never_answers_is_abandoned_at_its_bound` — a model
  that sleeps a minute; asserts the hook returned at `EXTRACTION_BOUND` (5s)
  and not after the sleep, **in both directions**.
- Four failures are absorbed and none reaches the hook: the database not
  opening, the model refusing, the model **panicking** (`catch_unwind`), and
  the model **hanging** (the work is on its own thread; the hook waits on a
  channel and leaves it behind).

**Why 820 is load-bearing rather than defensive.** `glasshouse hook` runs
*inside* the user's session, and Claude Code treats a hook's non-zero exit as a
veto on the turn — the user's own words echoed back at them with nothing sent.
This project observed that directly. `report_hook` may never fail.

**The panic hook, decided rather than deferred.** `memory/extract` recorded a
caveat it could not fix: `catch_unwind` catches the panic but the default hook
has already printed to stderr. `install_quiet_panic_hook()` is installed **only
in the `Command::Hook` arm** — not process-wide from a library module — and
routes the payload and location to `tracing::error!`. A Rust backtrace in the
middle of someone's coding session because a support job fell over is the same
defect as the hook failing. The panic is logged, not swallowed.

Verified against the shipped binary, and reproduced independently by the
orchestrator: a real `Stop` hook exits `0`, records `turn_ended`, moves the
session to `idle`, and logs
`memory extraction after a completed task produced nothing … reason=no
extraction model is available`.

---

### Phase 21B — decision provenance, 11 of 11 (lines 844–854)

Contract: A durable decision carries why it was made, when in the project's
life, what problem it solved, the assumptions that made it reasonable, and the
evidence behind it — and a decision missing all of that is treated as weaker
than one that carries it.

State: COMPLETE

Production evidence:
- **Producer:** `PROMPT_CONTRACT` rules 11–13 name every field; `RESPONSE_SCHEMA`
  asks for them with bounds; `schema::judge` validates each — optional, trimmed,
  bounded, refused **by name** when over, and `project_phase` refused by name
  when outside the map's five. `Extractor::store_one` writes them.
- **Consumer:** `glasshouse memory search` prints every field that has one, and
  `memory/search.rs` acts on their absence. Both are surfaces a person reaches,
  and both are in this batch — the §5 test applied to eleven storage lines that
  could easily have been eleven unread columns.
- Flat columns rather than a related table, deliberately: each line holds one
  concise sentence, `NULL` means *not known* and never *none*, and Phase 21C
  needs them **separable** rather than normalised. A `memory_assumptions` table
  with a category column would have been one join on every read for no
  capability the map asks for.

**853 was treated as a behaviour, because its verb is *treat*.**
`MemoryRecord::is_lower_confidence_decision()` is `kind == Decision &&
provenance.is_thin()`, where thin is *missing rationale **and** missing
assumptions* — **and**, not **or**, because that is what the line says.
`memory::search::demote_thin_decisions` then reorders **only decisions within
one authority class**. The obvious implementation — sorting thin decisions to
the bottom — reads the line as *lower-confidence than everything*, which it
does not say and which would be a real search regression. Both qualifiers in
the sentence are load-bearing: it compares a decision **to a decision**, **of
the same authority class**. Driven against the shipped binary with three
matching memories where the thin one carried the term three times and would
have won on BM25 alone: the two `preference` decisions swapped and the
`constraint` between them did not move. One test per clause, each killed by a
mutation dropping exactly one predicate.

Known limit, recorded rather than glossed:
- Lines 845–852 say *"Store …"*, and storing plus retrieving is what they ask
  for. **Nothing yet *acts* on them** — that is Phase 21C, which does not exist.
  853 is the exception and was treated as one.

---

### Phase 21 — "Allow memory extraction to run after task completion" stays OPEN, and the criterion that decides it

State: SCAFFOLDED — the wiring is complete, proven, and reachable; the
capability does not complete.

What exists:
- `report_hook` is a two-line wrapper over `report_hook_with(runtime, session,
  event, model_factory)`. On a translated `TurnEnded { outcome: Completed }` —
  and on nothing else — `run_extraction_after_turn` runs, **after** the event is
  recorded, so the turn's own closing event is in the material extraction reads.
- `an_event_that_is_not_a_completed_task_asks_no_model` is the discriminating
  half: `StopFailure`, `UserPromptSubmit` and `PermissionRequest` ask no model.
  Without it, *"runs after task completion"* would be satisfied by *"runs
  always"*.
- Production passes `NoExtractionModel`, whose `describe()` is
  `none configured (Phase 39 supplies the provider)`; that string is on every
  outcome and in every log line, and a mutation renaming it to
  `phase-39/cheap-model` is killed.

**Why it is not closed, and the sharpened criterion.** The lead argued for
closing it: the map states the trigger (817) and the model (809) as two separate
lines, and practice §33 closed *"run manually"* on a caller that calls no model.
It also stated the counter-argument and left the decision here. The counter is
right, and the reason is a criterion worth stating once:

**The test is not whether a model is called. It is whether the capability
completes and produces its result in the shipped binary.**

- *Run manually* completes: `--reply-from` supplies the model half at the user's
  direction, the whole pipeline runs, and **memories are stored** — verified by
  the orchestrator running the binary.
- *Run after task completion* cannot complete: the trigger fires on every
  completed task and dead-ends, always, because nothing can supply the model
  half on a turn boundary. Independently reproduced by the orchestrator: a real
  `Stop` hook exits `0` and stores nothing, and `memory search` finds nothing,
  ever.

Asked plainly — *can Glasshouse run memory extraction after task completion?* —
today's honest answer is "it tries, every time, and reports it has no model."
That is not the line. It closes the moment any model exists, at one line in
`main.rs` passing a different `Box<dyn ExtractionModel>`, and 809 and 817 will
close together for a reason that is about Phase 39 rather than about this box.
