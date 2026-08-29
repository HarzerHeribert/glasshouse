# Refusal register — every line refused, and whether its cause is ours

> **This is not an archive. It is the input to the next package.** Practice §83.
>
> A line refused at Phase −1 is a correct outcome and a recorded one. What went
> wrong for three batches is that nothing ever came back for them: the ledger
> filled with well-reasoned refusals and the next batch picked different lines.
>
> **The register exists so refusals can be read together rather than per phase**,
> because that is the only way the clusters are visible. Four separate phases
> each refusing "no production caller" looks like four dead ends; read together
> it was one hardcoded struct literal.

## How to use it

1. **Before writing the handoff**, add every line this batch refused, with the
   missing link named in current source and the `in-repo?` column filled.
2. **Before planning the next batch**, read the `in-repo = yes` rows together
   and ask which share a cause. Two or more sharing one is the next package.
3. **Delete a row when its line closes.** A stale register is worse than none —
   that is the failure mode this whole document is about.

## The column that decides everything

**`in-repo?`** — is the missing thing inside this repository?

- **yes** → packageable. Someone can write the code that supplies it.
- **no** → not work. A setter whose provider signal never arrives, or a
  capability whose decisive input is outside Glasshouse's process boundary, is
  blocked on reality. Packaging it manufactures work and invites a fabricated
  producer.

Do not let a `no` become a `yes` because the line looks close.

---

## Open refusals, as of batch 49

### Cluster A — a production caller passes an invented constant *(in-repo: YES — being attacked now by `GH-LEAD-PLACEHOLDERS`)*

| line | missing link | site |
|---|---|---|
| 1288 | `cheaper_adequate_resource_exists: false` hardcoded | `routing/disposable.rs:568` |
| 1290 | `user_override: false` hardcoded; no producer anywhere sets one | `routing/disposable.rs:568` |
| 1291 | blocked *by* 1288: with `cheaper_adequate…: false` the function falls through to `Allow`, so the imminent-reset branch is unobservable | `provider/quota.rs:2298` |
| 1294 | `task_nearly_complete: false` hardcoded | `routing/disposable.rs:568` |
| 1319 | `quota` carries the provider's `Retry-After`; `observe_exchange` is called 17 lines later without it; `session.rs:614` hardcodes `None` | `gateway/mod.rs:586-603` |

### Cluster B — a mechanism built, tested, and never installed in production *(in-repo: YES)*

| line | missing link |
|---|---|
| 1735 | `DegradeSink` threaded through the gateway; `main.rs` calls the plain constructor at both launch sites, so the sink is never `Some`. Blocked on an ownership question: `EventBus` does not exist yet when the gateway starts |
| 531 | `declare_token_priced` has zero non-test callers, so no token-priced allowance is ever created to track request pools separately *from* |
| 922 (half) | `MemoryStore::resolve_conflict` has zero non-test callers — Glasshouse can raise a conflict and cannot resolve one from the binary |
| 925 | **smaller than the ledger records.** `review_reason` is already persisted and `supersede`'s UPDATE leaves it intact; the recorded "needs a schema migration, Red tier" is wrong |

### Cluster C — an enum variant with no constructor *(in-repo: YES, but each needs a signal that may not exist)*

| line | missing link |
|---|---|
| 945 | `ReviewReason::ArchitectureDrift` — **no constructor anywhere, including tests**; reachable only by a human typing the string. Needs a "refactor starting" signal that does not exist |
| 946 | `ReviewReason::ProductionIncident` — identical shape, zero constructors |
| 1210 | `WindowCapacity::with_started_at` — one occurrence in the tree: its own definition |

### Cluster D — no consumer, because the consuming phase is at zero *(in-repo: yes, but correctly waiting)*

| line | missing link |
|---|---|
| 1239 | consumer belongs to routing phases still at zero |
| 1795 | no fallback-chain concept; routing-model selection is Phase 34C |
| 1796 | nothing maps a model to a tier ceiling. **Its recorded blocker is stale** — `WorkloadTier` ships at `routing/classify.rs:79` — but §14 applies: the blocker being gone is not the capability |
| 372 | **stale blocker**: `phase-9a.md:482` says `grep 'fn score\|Score'` is empty; it is not. Nothing selects among launch profiles, so still open |
| 1313 | latency aggregates have zero production readers; every candidate consumer is in another partition |

### Cluster E — the provider signal genuinely does not arrive *(in-repo: **NO** — do not package)*

| line | why it is not ours |
|---|---|
| 1205 | no header names an input-vs-output token split |
| 1206 | no `RATE_LIMIT_HEADERS` entry contains "cache" |
| 1208 | the one live account read answers `null` |
| 1213 | no concurrency header exists |
| 1215 | Groq's token ceiling arrives with **no window**; filing it per-minute would invent the period |
| 1216 | reader built and wired; no host has ever sent a window > 60s |
| 1317 | nothing states whether a 429 is provider-, model-, account- or pool-scoped |

### Cluster F — outside Glasshouse's process boundary *(in-repo: **NO** — a product boundary, not a gap)*

The decisive input is the user's source tree, the agent's plan, or the agent's
diff. Verified: nothing under `crates/glasshouse/src/` reads the user's tracked
source or runs their tests. Map line 932 declined this four times and
`memory/policy.rs:280-295` records the reason.

| lines |
|---|
| 919, 920, 921, 923, 943, 947, 951 |

### Cluster G — needs a schema migration this project refuses casually *(in-repo: yes; design first)*

| line | missing link |
|---|---|
| 327, 310 | a compaction lifecycle event needs a new `LIFECYCLE_EVENT_KINDS` value, and SQLite cannot widen a `CHECK` in place — `database.rs:830`'s house rule forbids the rebuild |
| 1316 | a new persisted outcome value, same constraint |
| 1325 | of four provenance values the line names, production can emit one |

---

## Standing refusals that are decisions, not blockers

- **1323** stays open by the user's own reasoning. Do not re-ask (§70).
- **828, 829** — a worker was asked to close them and declined, correctly: a
  keyword heuristic for "is this an obvious source-code fact" refuses real
  memories and admits fake ones. **Do not re-derive this.**
- **1681** — no recommendation producer exists to inspect without executing.
- **1661** — `max_router_latency_ms` is a configured ceiling, not a measurement.
- **1745, 1746** — no cmux-metadata path reaches project-scope validation, and
  there is no MCP surface. A grep for "cmux|mcp" hits doc comments and looks
  like a lead.
