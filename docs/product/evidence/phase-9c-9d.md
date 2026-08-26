# Capability evidence — phase 9c-9d

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

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
