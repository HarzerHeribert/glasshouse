# Capability evidence — phase 9c

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 9C — protocol compatibility as a filter, and Phase 9C at twelve of twelve

Line, quoted exactly: "Treat protocol compatibility as a hard routing constraint
before model-quality scoring."

Contract: Given a set of configured providers, when Glasshouse selects one to
route over, an incompatible provider is **removed from the candidate set** and
never merely ranked lower — while preserving: a declared protocol with no base
URL is not compatibility, an empty candidate set is a refusal naming what was
required and what was served, and no model-quality scorer is invented here.

State: **COMPLETE.** Phase 9C is **twelve of twelve**.

#### Most of it already held, and the gap was the seam

`Provider::serves` was already protocol-exact, `choose_protocol` already refused
a harness/provider pair sharing no protocol, and `gateway_upstream` already
discarded providers with no base URL. What was missing is that the gateway's
filter was a local `Vec<&Provider>` and the direct-provider chooser picked a
protocol *before* checking it had a destination.

`ProtocolCompatibleProviders` now sits in front of both. Its fields are private
and only its two constructors produce candidates, each requiring an exact
protocol declaration **and** a non-empty base URL.

**The ordering is enforced by the type, not by a convention.** A future
model-quality scorer has to accept the wrapper rather than a provider slice, so
there is no unfiltered set for it to rank — passing raw providers does not
compile. That is what "hard constraint … before scoring" has to mean to survive
a phase that has not been written yet. No production scorer was added; Phases 33
and 34 own that.

#### Evidence quality

Three mutations by the worker, each killed with the named test's own result
line: accept a declaration with no base URL; accept any provider with a URL
regardless of protocol; replace the empty-set refusal's detail. The worker
correctly distinguished a killed mutation that emitted an unused-helper warning
from a mutation that failed to compile.

**The worker ran under a sandbox that blocks loopback bind and Keychain**, so 30
tests failed on permissions alone. It enumerated all 30 by name so the set could
be checked rather than trusted, and added no workaround. Unsandboxed: **779
passing, 0 failing**, exactly the total it predicted.

#### An orchestrator error, recorded because the recovery is the useful part

While probing the type boundary, the orchestrator appended a test to the
worker's `provider/mod.rs` and then ran `git checkout --` on that file to undo
it. **That deleted all 161 lines of the worker's work**, because a worker's
deliverable exists only as uncommitted changes and git cannot tell whose edit is
whose.

The worker's session was still live, so it was asked to re-create the file from
its own record; the first attempt came back at +93 lines and 778 tests against a
predicted 779, the shortfall was pointed out, and the second attempt restored it
exactly at +161 and 779. **The test-count discrepancy is what caught the
incomplete restore** — without a predicted number to check against, a
plausible-looking partial restoration would have been committed.

The rule was already written down and was broken anyway, so it is now enforced
by a `PreToolUse` hook rather than documented: `scripts/hooks/guard-destructive-git.sh`
refuses `git checkout` with a path, `git restore`, `git stash` and `git clean`,
and points at the `cp` backup that restores *your change* instead of *the file*.

---
