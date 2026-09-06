# Capability evidence — phase 9g

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

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

## 488 — UN-TICKED 2026-09-06: a dogfooding session showed Glasshouse's own provider credential reaching the harness child

Dogfooding session 586a0338b1a0 (2026-09-06, `docs/process/dogfooding.md`): with the machine's one support-work provider configured as `[providers.groq] credential_env = ["GROQ_API_KEY"]` and that variable exported in the launching shell (the way `credential_env` is meant to be supplied), a real Claude Code session launched by `glasshouse launch` ran `env | grep -c GROQ_API_KEY` and answered **1**. The harness child inherits every provider credential the launching shell holds; `launch.rs :: HarnessLaunch::build_command` replays only the overlay's own `EnvChange`s and nothing adds an `EnvChange::Remove` for a configured `credential_env` name. Line 488 says the credential stays inside the Glasshouse process or the secret store; it does not, so the box comes off until the fix lands.

Successor: `GH-LAUNCH-STRIPS-PROVIDER-CREDENTIALS` (Red — secrets): every configured provider's `credential_env` names are removed from the child's environment at launch, proven by a pty-fixture launch whose fake harness prints its environment, with an independent verifier. Re-tick 488 on that demonstration.
