# Capability evidence — phase 9d

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

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
