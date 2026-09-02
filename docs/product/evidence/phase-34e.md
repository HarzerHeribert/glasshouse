---

# Line 1464 — closed 2026-08-30

Package `GH-ROUTING-SPEND-DOOR`. Phase 34E was 9 open / 0 closed and had **no
ledger entry at all**; this file is its first.

The line: *"Measure routing-model token and request consumption separately from
coding-agent consumption."*

State: **COMPLETE**

## Why this was closeable now and was not last week

Two producers landed on 2026-08-30 and 1464 sits exactly on top of them.
`parse_usage` (`memory/extract/model.rs:495-501`) reads `prompt_tokens` /
`completion_tokens` / `prompt_tokens_details.cached_tokens` out of a reply
document Glasshouse already deserializes — **the first thing in this build that
counts tokens**. And `record_classification_observation` (`main.rs:3213-3232`)
stamps `purpose = "classification"` on the row a real classification call
produces.

What was missing was a **consumer**: nothing anywhere aggregated the ledger by
purpose. `glasshouse routing-cost [--hours N]` is that consumer.

## The hazard this line invites, and how it was actually avoided

**"Separately from coding-agent consumption" is the kind of qualifier that
passes vacuously**, which is precisely why 1455 and 1456 were un-ticked earlier
the same day. If coding-agent consumption were absent from the ledger, a report
"separating" routing consumption from it would be separating it from nothing.

It is not absent. The orchestrator verified before dispatch that
`routing_observations` has **three** production writers:

| writer | `purpose` | `harness` | tokens |
|---|---|---|---|
| classification — `main.rs:3229` | `"classification"` | NULL | **present** |
| extraction — `main.rs:3760` | NULL | NULL | **present** |
| gateway relay — `gateway/session.rs:358` (that file's `#[cfg(test)]` is `:739`) | NULL | **set** | **NULL by design** |

The gateway rows **are** the coding-agent exchanges, one per real relayed
exchange, and they carry genuine request counts. So the separation is over real
data on both sides.

**The packet as dispatched would have got this wrong**, and it was corrected
mid-flight: it said to group by `purpose` alone, which folds extraction's real
token counts together with the gateway's genuinely-uncounted rows. The worker
verified the correction against source itself before applying it and regrouped
on `(purpose, harness IS NOT NULL)`.

## The property that carries the line, and it is structural

A group whose every row has `NULL` tokens must report **"not counted"**, never
`0`. A reader who cannot tell *"nothing was consumed"* from *"nobody counted"*
has been handed a fabrication — the same failure `RetrievalResult.relevances`
is private to prevent.

This is not defended by Rust code that could drift. It falls out of SQL:
`SUM(x)` skips `NULL` inputs and returns `NULL` only when **every** input was
`NULL`. The aggregate is

    SELECT purpose,
           (harness IS NOT NULL) AS harness_recorded,
           COUNT(*) AS sample_count,
           SUM(input_tokens) AS input_tokens, ...
      FROM routing_observations
     WHERE project_id = ?1 AND observed_at >= ?2 AND observed_at <= ?3
     GROUP BY purpose, harness_recorded

`sample_count` is a real `COUNT(*)` and is always defined, so a group can report
**requests without tokens** — which is exactly the coding-agent group's honest
shape.

## Production

- `routing/evidence.rs :: EvidenceLedger::consumption_by_purpose`
- `routing/evidence.rs :: PurposeConsumption`, `row_to_purpose_consumption`
- `main.rs :: routing_cost_report`, `render_routing_cost`,
  `purpose_group_label`, `render_token_count`
- `cli.rs :: Command::RoutingCost`

The ledger handle is opened **only inside `routing_cost_report`** (practice
§65): an open SQLite handle on a path with nothing to do blocks a later writer
under Windows' mandatory `LockFileEx` while being invisible under POSIX
advisory locks.

## Regression — `tests/routing_cost.rs`, 6 tests, all through the real binary

`running 6 tests` … `test result: ok. 6 passed; 0 failed` (§68: a whole-target
run, not a name filter).

The two that carry the line:

- `the_classification_group_is_attributed_its_own_tokens_and_no_others` —
  asserts the exact counts land under `classification` and never under the
  coding-agent group.
- `a_group_with_no_counted_tokens_never_renders_a_digit_for_them` — the hazard
  test. Exact string equality on `"not counted"`, **plus** an assertion that the
  rendered value contains no ASCII digit at all.

`coding_agent_rows_and_other_unpurposed_rows_are_never_merged` pins the
mid-flight correction: an extraction-shaped row and a gateway-shaped row land in
different groups.

**The isolation test is worth naming.** Two projects sharing one `--data-dir`
already get **separate `glasshouse.db` files**, so a naive two-fixture test
would pass even with the `WHERE project_id = ?1` clause deleted — there would be
nothing in the same file to leak. The test therefore drops
`routing_observations_reject_foreign_project_insert`, plants a foreign-project
row **inside beta's own database file** under the same purpose, and recreates
the trigger. Only the SQL `WHERE` clause can keep the totals apart. That is the
same pattern `tests/memory_project_scope.rs::plant_foreign_memory` uses, and it
is the difference between an isolation test and an isolation-shaped test.

## Mutations

| change | result | killed by |
|---|---|---|
| `SUM(input_tokens)` → `COALESCE(SUM(input_tokens), 0)` | **KILLED** | `a_group_with_no_counted_tokens_never_renders_a_digit_for_them` (and `coding_agent_rows_and_other_unpurposed_rows_are_never_merged`) |
| `SUM(output_tokens)` → `COALESCE(SUM(output_tokens), 0)` | **KILLED** | re-run by the orchestrator at integration; the worker had flagged it as untested |

Observed on the first: `assertion left == right failed: a group with no counted
rows must say so, never a number: "    input tokens        : " was "0"`.

§80's checklist was applied to both: a real `test result:` line with real
counts, killing tests inside the named target, the mutated line on the path the
killing tests exercise, and a genuine assertion failure rather than a compile
break.

## Limits

- **Coding-agent token consumption is not counted anywhere**, and this line does
  not require it to be. `gateway::ingress` relays a body it is designed never to
  parse. The report says so in a fixed closing line, so a reader is never left
  to infer that "not counted" means zero.
- The `(no purpose or harness recorded)` group is extraction today. Nothing
  prevents a future producer from leaving both unset while being neither
  extraction nor coding-agent; this aggregate does not claim to identify a
  fourth kind of spend that does not exist yet.
- No currency figure is rendered anywhere. See 1465 below.

---

# Line 1465 — REFUSED, and it is not a near miss

*"Track routing-model spend separately from productive task spend."*

State: **NOT STARTED**, and blocked on a producer that does not exist.

`ObservedCost` has **no production producer**. Its only two assignments,
`routing/evidence.rs:1674` and `:1723`, both fall after that file's
`#[cfg(test)]` at `:1355`. `cost_micro_usd` is therefore `NULL` on every row
this build can write, `evidence.rs:65-67` names the four columns "not supplied",
and `provider/resources.rs:952-954` prints *"Glasshouse does not count spend
against this"* to the user.

**The distinction from 1464 is the whole point and must not be blurred.** 1464
says *token and request consumption*, which this build now measures. 1465 says
*spend*, which is a money figure, and no money figure exists anywhere in
Glasshouse. `routing-cost` therefore renders no price, no currency amount, and
no spend estimate — deliberately, and the packet forbade it explicitly.

This is Cluster M in `docs/process/refusal-register.md`.

---

# Line 1463 — open, untouched

*"Measure the number of routing decisions made per interactive hour."*

The ledger can count rows and knows `observed_at`, so decisions per *elapsed*
hour is available. **"Interactive hour" is the blocker**: nothing in this build
measures interactive time, and a proxy would be a fabricated denominator. Left
open rather than closed against a substituted quantity.


---

## From `GH-ROUTING-ECONOMICS` (2026-08-31)

The routing-model selector package closed this phase's lines 1463, 1465, 1466; the full entry — production sites, regression names, the 22 killed mutations and the four refusals with their producers — is in `phase-34c.md` under *Package GH-ROUTING-ECONOMICS*, because the mechanism (`DisposableRouting::choose_for_automatic_classification`) lives in that phase.


---

## From `GH-LAUNCH-CLASSIFIER` (2026-08-31)

The launch-path classifier package (router request schema, classification on the acting path) touched this phase's lines 1467, 1468, 1470, 1471 (closed). The full entry — production sites, regression names, the 23 killed mutations, the one honestly-survived one, and the missing producer for 1516/1517/1531 — is in `phase-34d.md`, *Phase 34D — router request schema* and *lines outside Phase 34D*, because the mechanism lives there.

---

## 1469 — censused 2026-09-02 (`GH-RECON-1469`, Sonnet high, read-only): open, and its package is named

*"Cache recent classification results for semantically identical task
starts when safe."* No producer exists for this line, and the mechanism that
looks like one is not: `ClassificationStickyCache` /
`StickyClassification::reuse_for` (`request.rs:1009-1038`, closing
1467/1468) reuses a classification across turns of the **same warm session**
and never compares task text. The repeat 1469 names is real on the shipped
path: `route --task X` then `launch --task X` asks the model twice (`route`
passes `sticky: None` by design, and a fresh `launch` fails `reuse_for`'s
session check), and `glasshouse classify` bypasses the sticky cache entirely
(`classify_with_routing_model`, `main.rs:7746`, from `:158`).

**Semantically identical, honestly:** no embeddings exist (Phase 52 is
Cluster Q for that reason), so identity is a normalised literal text match,
keyed by a **hash** — never the text, keeping Phase 51's *no query text
persisted* rule without extending it. **When safe:** never below
`Confidence::Low` (reuse `is_low_risk`'s own rule, `classify.rs:480`); same
`RoutingModelResolution` identity, with an `Automatic` answer tagged by the
model label that actually answered; same project (inherited from
`project_state_dir(project_id)`, as both existing sticky caches do); a TTL
read from `recorded_at_unix`, which `reuse_for` records and never reads; and
the existing `RoutingFingerprint` as one field of the record. **Where:**
neither observation ledger is honest — both are append-only logs with no
lookup key, and `evaluation_observations` forbids query text — so the home is
a third file-based cache in the two existing caches' exact shape
(`routing-classification-cache.json` under the project state dir). No
migration.

**Two rulings the recon left open, made here.** (1) The cache serves the
acting path (`launch`, `classify_for_routing`'s fall-through before the model
ask) and `glasshouse classify`; `route`'s report path keeps asking fresh, by
its own comment's design — a diagnostic that explains *what would happen
now* must not answer from yesterday. (2) *Recent* is a named constant beside
`STICKY_TURN_WINDOW_SECONDS` with its reasoning, not a config key; nothing
here earns a surface.

**Successor: `GH-CLASSIFICATION-CACHE`** (Amber, Sonnet high;
`routing/request.rs`, `main.rs`) — validated packet in `.agent-runtime`,
dispatched once the three-leg gate has the machine to itself. Every line
number the packet inherited was stale and the worker corrected all of them
(`classify_with_routing_model` `:7746`, `classify_for_routing` `:4464`,
callers `:158`, `:4528`, `:4272`, `:5061`).

---

## 1469 — CLOSED 2026-09-02 (`GH-CLASSIFICATION-CACHE`, Amber, Sonnet high): the census above, closed

`routing/request.rs`: `CachedClassification` (hash of the normalised text,
`RoutingFingerprint`, resolution tag, the existing `StoredClassification`,
`recorded_at_unix`), `is_reusable_for` with four gates, the normaliser (trim,
collapse whitespace, lowercase, SHA-256 — the file never holds the text),
`CLASSIFICATION_CACHE_WINDOW_SECONDS` beside the sticky window, and a new
`AnswerProvenance::ReusedFromCache` so a served answer says so. `main.rs`:
`ClassificationTextCache` in `ClassificationStickyCache`'s exact file shape
(`routing-classification-cache.json` under the project state dir), read in
`classify_for_routing` after the sticky-session reuse did not fire and before
the model ask, written after a real answer; `launch` passes it, `route`
passes `None`; `glasshouse classify` reads and writes it too. One
pre-existing test that launched the same text twice now sees the second
served from the cache, as it should. The worker's report was cut off by a
dropped connection and written again on a nudge; the work was intact.

**Phase 34E is complete.**

### Cache recent classification results for semantically identical task starts when safe. (line 1469)

Contract: Given a task start whose normalised text was classified in this project recently, under the same routing-model resolution, with a confidence above Low and an unchanged fingerprint, when `launch` (or `glasshouse classify`) needs its classification, Glasshouse answers from the cache and makes no model call — while preserving that `route`'s report path always asks fresh, that a Low-confidence, expired, differently-resolved or differently-fingerprinted entry is never served, and that the task text is stored only as a hash.

State: **COMPLETE** — ruled 2026-09-02. The cache answers a task start whose normalised text was classified in this project within the window, under the same pinned resolution, above Low confidence, with an unchanged fingerprint — and makes no model call, proven through the shipped binary with a provider that fails the test if asked; the three mutations are on the three gates the line's *when safe* names and all are KILLED. The two orchestrator rulings hold (`route` asks fresh; the window is a named constant), and the worker added a third, accepted here: an `Automatic` resolution neither reads nor writes the cache, because the model that would answer *now* cannot be known before asking, so a cached answer could stand in for a different model's — the recon's own warning, applied on the read side. The cache therefore serves pinned resolutions; that is the honest scope of *when safe* in this build.

Production evidence:
- `crates/glasshouse/src/routing/request.rs` — `normalised_task_key`
- `crates/glasshouse/src/routing/request.rs` — `CachedClassification::is_reusable_for`
- `crates/glasshouse/src/routing/request.rs` — `CachedClassification::new/classification/key/recorded_at_unix`
- `crates/glasshouse/src/routing/request.rs` — `AnswerProvenance::ReusedFromCache`
- `crates/glasshouse/src/main.rs` — `ClassificationTextCache::load/lookup/store`
- `crates/glasshouse/src/main.rs` — `classification_cache_resolution_tag`
- `crates/glasshouse/src/main.rs` — `classify_for_routing (text-cache lookup and write, Pinned|Automatic arm)`
- `crates/glasshouse/src/main.rs` — `Command::Classify handler (text-cache lookup and write)`

Regression evidence:
- `routing::request::tests::normalisation_collapses_whitespace_and_case_to_the_same_key`
- `routing::request::tests::normalisation_never_stores_the_task_text`
- `routing::request::tests::a_reusable_entry_passes_all_four_gates`
- `routing::request::tests::a_low_confidence_entry_is_never_reusable`
- `routing::request::tests::a_different_fingerprint_is_never_reusable`
- `routing::request::tests::a_different_resolution_tag_is_never_reusable`
- `routing::request::tests::an_entry_older_than_the_window_is_never_reusable`
- `tests::classification_cache_resolution_tag_names_the_pin_and_nothing_else (bin glasshouse)`
- `tests::a_classification_cache_round_trips_a_text_keyed_entry_and_bounds_its_size (bin glasshouse)`
- `tests::an_unreadable_classification_cache_file_reads_as_empty (bin glasshouse)`
- `launch_classification::identical_task_text_up_to_whitespace_and_case_is_served_from_the_text_cache`
- `launch_classification::a_low_confidence_answer_for_the_same_text_is_never_served_from_the_text_cache`
- `launch_classification::route_after_launch_with_the_same_task_always_asks_the_model`
- `launch_classification::the_text_cache_file_never_contains_the_task_text`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| let digest = Sha256::digest(normalised.as_bytes()); -> let digest = Sha256::digest(text.as_bytes()); | `compare-raw-text` | **killed** | `launch_classification::identical_task_text_up_to_whitespace_and_case_is_served_from_the_text_cache` |
| let age = now_unix.saturating_sub(self.recorded_at_unix); (0..=CLASSIFICATION_CACHE_WINDOW_SECONDS).contains(&age) -> let _ = self.recorded_at_unix; true | `drop-the-age-gate` | **killed** | `routing::request::tests::an_entry_older_than_the_window_is_never_reusable` |
| if classification.confidence() == Confidence::Low { return false; } block deleted | `serve-low-confidence` | **killed** | `routing::request::tests::a_low_confidence_entry_is_never_reusable` |

> compare-raw-text observed: assertion `left == right` failed: a repeat of the same normalised task text must not ask the routing model again: (model.requests().len() came back 2, expected 1)

> drop-the-age-gate observed: assertion failed: !cached.is_reusable_for(1_000 + CLASSIFICATION_CACHE_WINDOW_SECONDS + 1, &fingerprint(), "pinned:route-probe/router-model")

> serve-low-confidence observed: assertion failed: !cached.is_reusable_for(1_500, &fingerprint(), "pinned:route-probe/router-model")

Recorded scope limits — stated by the worker, not discovered later:
- Automatic-resolution classifications are written to the cache (their real model label is captured in the stored record) but never served back — classification_cache_resolution_tag returns None for RoutingModelResolution::Automatic by deliberate ruling, documented in this report and as a doc comment on that function. Only Pinned-resolution reuse is proven end to end.
- route's report path (main.rs:4272 area) is confirmed never wired to either cache, by design (map line 1469's contract and the packet's explicit ruling).
- Windows and Linux legs of the local gate were not run this session; only macOS. main.rs carries unrelated pre-existing cfg(unix/windows) code, so blast-radius.sh flags the whole file as platform-conditional even though this package adds no new cfg.

---

---
