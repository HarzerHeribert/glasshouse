# Capability evidence — phase 9d-9a

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

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
