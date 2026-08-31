# Capability evidence — phase 50

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 50 — tracked project knowledge as an optional feature, 7 of 7 (lines 1810–1816)

Package `GH-TRACKED-KNOWLEDGE`, 2026-08-31, Sonnet implementer at high effort.
Seven lines, seven closed, five mutations KILLED.

Contract: Given a project whose memory lives outside the repository by default,
when the user explicitly opts in with `glasshouse memory export --tracked`,
Glasshouse writes current decisions and constraints (findings only with
`--include-findings`) as deterministic, human-readable Markdown under
`.glasshouse/knowledge/`, labelled as a projection of the canonical store and
reviewable in ordinary Git workflows — while preserving that no session
history, credential or provider metadata ever reaches those files, and that
nothing exports on its own.

State: **COMPLETE** for 1810, 1811, 1812, 1813, 1814, 1815, 1816.

Production evidence:
- `crates/glasshouse/src/memory/export.rs` (new) — `Selection`, `Manifest`,
  `WrittenFile`, `TrackedKnowledge::write(&ProjectMemory, root, Selection,
  dry_run)`, a bounded secret redactor (`SECRET_PREFIXES`: `sk-`, `gh*_`,
  `xox*-`, `AKIA`, `ASIA`, … on token-shaped runs ≥ 20 chars → `[REDACTED]`),
  and a dependency-free epoch→ISO-8601 formatter. Reads **only**
  `ProjectMemory` — the module's surface is reachable through the memory store
  alone, so 1813 is structural.
- `crates/glasshouse/src/cli.rs` — `MemoryCommand::Export { tracked,
  include_findings, dry_run }`; `main.rs::memory_export_tracked` — **the only
  door that ever copies memory back into the tree, and it never opens on its
  own** (1811, 1813).
- Runtime memory stays under `Runtime::state_dir()` (`<data_dir>/projects/<id>`),
  outside the repository, by default and always (1810 — existing behaviour,
  evidenced).

Two decisions the worker made and the orchestrator accepts: **the per-file
"exported" timestamp is each memory's own `updated_at`, never the wall clock**
— the only way two exports of an unchanged store are byte-identical and one
memory change is a one-file diff (1816 by construction); the README carries no
per-run timestamp for the same reason. **The canonical-store name in the header
is the project id** — which *is* the state directory's own name — so no path
and no `Runtime` reference enters the export module.

Regression evidence (`tests/tracked_knowledge.rs`, 5 tests through the shipped
binary, one per line; `memory_snapshot` 8; `memory_project_scope` 1; `memory::`
lib 105):
- `runtime_memory_lives_outside_the_repository_by_default_and_nothing_is_exported_without_opting_in`
- `opting_in_exports_decisions_and_constraints_as_readable_files_and_not_findings_by_default`
- `two_exports_are_byte_identical_and_one_memory_change_is_one_file_diff`
- `no_session_history_credential_or_provider_metadata_reaches_the_files` — a
  planted `sk-`-shaped token in a memory body is redacted, no provider
  configuration appears, and the exporter's source names no session, event or
  checkpoint module (source scan)
- `the_export_says_it_is_a_projection_of_the_canonical_store`

Failure / isolation evidence — five mutations, five KILLED (one per line's
mechanism): the opt-in gate bypassed (`if !tracked` → `if false`), findings
exported by default, ordering made non-deterministic, redaction removed, README
dropped — each killed by the test named for the line, failure text quoted in
the report.

Gates: fmt, clippy `-D warnings` clean; targets above green; `blast-radius.sh`
exit 0 (the worker's first run mis-read `tail`'s exit code as a failure banner
and re-ran redirecting to a file — §68's cousin, recorded).

Limits, stated by the worker: the redactor is a bounded prefix+length scanner,
deliberately a second, narrower control rather than the extractor's own
detector (out of scope to read), and will not catch a secret with no
recognisable prefix; 1815 proves the README and header say *projection*, not
that a reader understands it; byte-identity is sampled over one short window,
though it holds by construction.
