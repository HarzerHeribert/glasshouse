# Capability evidence — phase 2b

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

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
