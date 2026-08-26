# Capability evidence — phase 45

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 45 — the crash report's race, and why the box was right anyway

The Linux leg of the local gate had been failing randomly for days across two
different pty tests (practice §34). Rate, measured under full workspace load in
the container: **8 failures in 17 runs before; 0 in 20 after.**

**The cause, measured rather than inferred.** A pty child's exit becomes
observable before its output does: the exit comes from `waitpid`, while the
output must cross the pseudo-terminal and be copied into a buffer by a
*different thread* which, beside ~900 siblings, does not always get a CPU slice
in time. Instrumenting the failing assertion to keep looking after it failed
caught the window directly — at the moment of the empty read the output had not
ended, and the bytes arrived **1.1ms to 2.2ms later**.

**The orchestrator's hypothesis was wrong and the worker killed it with data.**
The packet proposed that Linux discards unread buffered output when the last
slave descriptor closes. It does not: 200 trials at each of three delays, child
reaped before the first read, **600 trials, zero bytes lost**, `EIO` every time
*after* the data. Linux hands the reader everything that was written and then
reports end-of-file.

**So the box stands and the defect was smaller than it looked.** Glasshouse
never lost a crashed harness's output on Linux; `crash_report` *reported* it as
absent when asked inside the window. The fix is `OutputEnd`, a `Mutex<bool>`
plus a `Condvar`, so `crash_report` waits to be **woken** by the reader rather
than sleeping and looking again — bounded at 250ms, deliberately the same bound
`session::attach` already allows its own pump, because on Windows no
end-of-file ever arrives while the pty is open and nothing else would end the
wait.

Known limit, recorded rather than hidden:
- A **different**, rarer failure survives:
  `a_direct_provider_profile_reaches_a_real_child_and_only_that_child`, once in
  37 runs, with the child killed by `SIGABRT`. That is a child that died, not
  output that had not arrived, and the drain fix does nothing for it. Ruled out
  with evidence: the `EIO` hypothesis (600 trials), a non-blocking master fd
  (portable-pty never sets `O_NONBLOCK`), `malloc` between `fork` and `exec`
  (2400 spawns against 24 allocation-churning threads, 0 aborts), and
  mislabelling (`strsignal(6)` really is `SIGABRT`). A ranked list of where to
  look next is in the report.
