#!/usr/bin/env python3
"""Find the Cluster B shape: production code with no production caller.

WHY THIS EXISTS
---------------
Batch 51 closed eight capability lines. **Four of them were the same defect** —
a mechanism built, tested, mutation-proven, and never given a production caller,
so the behaviour the map describes could not occur in the shipped binary:

    1735  DegradeSink threaded through the gateway; main.rs never passed it
    925   `superseded` rejected --reason outright, so nothing recorded a why
    468   capability_probe existed; every call site was after #[cfg(test)]
    531   same shape, and ALSO missing a consumer — refused before dispatch

That was found four times by hand, by an orchestrator grepping on a hunch. This
script is that hunch made mechanical, and it is meant to be run *before*
planning a batch rather than after.

WHAT IT LOOKS FOR
-----------------
A `pub fn` defined before its file's first `#[cfg(test)]`, whose every call site
in `src/` falls *after* one. That is not proof of a dead mechanism — it is a
lead, and a good one.

HOW TO READ THE OUTPUT, AND WHAT IT IS NOT
------------------------------------------
**Most hits are not defects.** Three kinds of noise dominate:

  * plain accessors and setters (`set_budget`, `contributions`) — public API
    surface a consumer outside this crate may use;
  * functions reached only from `tests/` integration tests, which this script
    does not scan;
  * a symbol reached through a trait object or a re-export, which a textual
    scan cannot follow.

**So the output is a list of questions, not answers.** The one that matters is:
*does an open capability line depend on this symbol doing something?* Cross-
reference against `docs/product/evidence/` before believing any row. Validated
on two known cases when written: it independently rediscovered `lifecycle_for`
(the §35 shape a worker had found by hand) and `with_context_state` (line
1760's recorded blocker).

It also found a regression the same batch introduced: `MemoryStore::supersede`
lost its last production caller when 925 refactored it.

USAGE
  scripts/cluster-b.py            # top 45 candidates, most test-called first
"""
import re, pathlib, collections
SRC = pathlib.Path("crates/glasshouse/src")
files = sorted(SRC.rglob("*.rs"))
test_start, lines_of = {}, {}
for f in files:
    ls = f.read_text(errors="replace").split("\n")
    lines_of[f] = ls
    start = 10**9
    for i, l in enumerate(ls, 1):
        if l.strip().startswith("#[cfg(test)]"):
            start = i; break
    test_start[f] = start

defs = {}
defre = re.compile(r'^\s*pub(?:\([^)]*\))?\s+(?:const\s+|async\s+|unsafe\s+)*fn\s+([a-z_][a-z0-9_]*)')
for f in files:
    for i, l in enumerate(lines_of[f], 1):
        m = defre.match(l)
        if m and i < test_start[f]:
            defs.setdefault(m.group(1), (f, i))

callre = re.compile(r'\b([a-z_][a-z0-9_]*)\s*\(')
prod, test = collections.Counter(), collections.Counter()
for f in files:
    ts = test_start[f]
    for i, l in enumerate(lines_of[f], 1):
        for name in callre.findall(l):
            if name not in defs: continue
            d_f, d_i = defs[name]
            if f == d_f and i == d_i: continue
            (test if i >= ts else prod)[name] += 1

skip = {"new","default","fmt","from","clone","len","is_empty","as_str","value","get","open","run","build","id","name","path","to_string","into","iter","map","expect","unwrap"}
rows = [(test[n], n, f"{f.relative_to('crates/glasshouse')}:{i}")
        for n,(f,i) in defs.items()
        if n not in skip and prod[n]==0 and test[n]>0]
rows.sort(reverse=True)
print(f"{len(rows)} pub fn(s) defined in production with ZERO in-crate production call sites\n")
for t,n,loc in rows[:45]:
    print(f"  {t:3d} test call(s)  {n:44s} {loc}")
