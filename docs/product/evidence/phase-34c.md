# Capability evidence — phase 34C

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 34C — line 1443 only: what "resource diagnostics" means

**No box in this phase is closed.** Phase 34C (automatic routing-model selection)
is 0 of 13, and twelve of its lines are blocked for one reason a read-only recon
established this round and a proof worker then confirmed independently:
**nothing in the shipped binary ever calls a routing model.** `RoutingModelChoice`
can be configured, resolved and rendered; no code path asks it to classify
anything.

This entry exists for **line 1443** alone, because a worker did the right thing
and refused to decide it.

Contract: Given a configured routing model, when the user inspects Glasshouse's
resource diagnostics, Glasshouse names the routing model currently selected —
while never implying a model is in use when nothing calls one.

State: **NOT STARTED.**

### The question, and why a worker was right to escalate it

`GH-PROOF-ROUTER` was asked to confirm or refuse eleven lines a recon called
closable. It closed seven, refused three, and for 1443 **reported both readings
and picked neither**:

- **TUI reading** — the Settings overlay's `RoutingRow` (`shell/mod.rs:1668-1687`,
  rendered in `shell/view.rs`) already shows the resolved routing-model choice.
  On this reading the line is closed today.
- **CLI reading** — `resources_report` (`main.rs:2233-2380`) renders no routing
  information at all; a grep for "routing" in that function's body returns
  nothing. On this reading the line is open.

That is exactly the escalation practice §33 asks for: *"a worker may hand you a
judgement; take it, and say which way you went."*

### The orchestrator's ruling: the CLI reading. The line stays OPEN.

**Three reasons, in order of weight.**

1. **A settings screen is where you choose a value, not where you diagnose one.**
   The Settings overlay showing your current selection is Phase 2D's settings
   capability — *"does using a control do something real and durable?"* — and
   those boxes are already closed on it. Counting the same rendering twice, once
   as configuration and once as diagnostics, would make the ledger say a
   diagnostic surface exists when the only thing that exists is a config editor.
2. **The map distinguishes surfaces deliberately.** Phase 41's line 1661 —
   *"Show the currently selected routing model **and its recent latency**"* — is
   the project-overview surface, and it is separately open. A map that names the
   same fact for two different views is not being redundant; it is naming two
   views. `glasshouse resources` is the third, and it is the one called
   *resources*.
3. **The line's purpose is answering "why did routing behave that way".** That
   question is asked at a diagnostic surface, next to capacity, health and quota
   — not on the screen where the user just set the value themselves.

**Closing it is cheap and is not blocked**, which is worth saying so nobody
records this as architecture: `resources_report` already receives the
`EffectiveConfig` that resolves the choice. It is a rendering line and a test.
It was not done in this batch only because `main.rs` was owned by another
worker's un-integrated diff.

**One honesty constraint on whoever closes it.** Nothing calls the routing model
(`routing::classify::classify` has one production caller, the `glasshouse
classify` CLI diagnostic; nothing constructs a `TaskClassification` outside
`#[cfg(test)]`). So the diagnostic must name the model that **would** be selected
and must not imply one is in use. Rendering a "currently selected routing model"
beside live capacity numbers, with no signal that it classifies nothing, is the
spectacle Phase 47 exists to prevent.

Missing evidence:

- A routing-model line in `resources_report` and its `api/unix.rs` twin, with a
  test entering through the shipped binary — the precedent is
  `provider_discovery.rs::a_planted_gateway_reading_now_reaches_the_shipped_binarys_report`.
- Wording that distinguishes *configured* from *in use* while line 1425/1426
  remain open.
