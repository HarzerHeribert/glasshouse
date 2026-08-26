# Capability evidence — phase 9f-preflight

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

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
