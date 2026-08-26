# Capability evidence — unfiled entries

Entries whose heading in `GLASSHOUSE_CAPABILITY_EVIDENCE.md` did not name a phase, so they were not guessed into a phase file (see `docs/product/evidence/README.md`).

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

### Migration 7 — `lifecycle_events` rebuilt, and `seq` proven durable

Contract: The event log admits `gateway_backend_changed`, and rebuilding the
table does not disturb the event identifiers that extracted memories point at.

State: COMPLETE (closes no box by itself; makes Phase 9H line 515 durable)

Production evidence:
- SQLite cannot add or drop a `CHECK`, so admitting a new `kind` is a full
  rebuild: rename, recreate, copy, drop, then recreate the index and all three
  triggers. `LIFECYCLE_EVENT_KINDS` goes from 10 entries to 11, and
  `LifecycleEvent::GatewayBackendChanged { provider, model, cause }` carries
  **names only** — never a credential.
- The two halves cannot be split:
  `every_lifecycle_event_kind_is_one_the_schema_accepts` asserts the enum and
  the schema constant are equal **in both directions**, so a variant without a
  `CHECK` value fails immediately rather than becoming a constraint violation on
  the event-writer thread where nobody is looking.

**The load-bearing evidence is the `seq` test.** `memories.source_event_first`
and `source_event_last` reference `lifecycle_events.seq`, which is
`INTEGER PRIMARY KEY AUTOINCREMENT`. A rebuild that renumbers `seq` would
silently re-point every extracted memory's provenance at the wrong events, and
**nothing would fail** — the data would simply be wrong. The regression asserts
a memory's event range still names the same events after the rebuild, and the
mutation that lets `seq` renumber makes it FAIL.

Orchestrator note: the worker needed three files outside its partition to prove
this end to end. It patched them locally, ran the suite green, **reverted them
to their exact committed byte content** (verified with an empty `git diff`), and
reported the patches. The orchestrator applied them at integration — including
one the packet never anticipated, in `session/store.rs`, which hard-codes every
table's column list and two expected schema versions and would have compiled
perfectly while failing three tests.

---
