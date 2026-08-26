# Capability evidence — phase 9f

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

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
