# Capability evidence — phase 9e

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 9E — the macOS Keychain, a labelled fallback, and a hang that would have frozen the TUI (three lines)

Contract: Given a user who stores a provider credential, when Glasshouse needs
it at launch, it resolves the value from the operating system's own secure store
where one is available and from the environment otherwise — while preserving:
the value never enters configuration, a log, a `Debug` or Git; the user can see
**which** store answered and delete what is stored; and an unavailable native
store is reported plainly rather than silently degraded.

State: **COMPLETE** for the three lines. Phase 9E is eleven of thirteen.
Windows Credential Manager and Secret Service stay **unchecked** — neither is
provable from this machine, and the packet forbade checking them.

#### The defect that justifies "run the binary" on its own

`glasshouse doctor`, pointed at a provider whose credential was in the Keychain,
**hung indefinitely** — exit 124 under `timeout 30`, no output, no visible
dialog. The stack sample ends in
`Security::SecurityServer::ClientSession::decrypt`.

`SecKeychainFindGenericPassword` decrypts the item, and decryption consults the
item's access control list. For an item **this binary did not create**, the list
does not name it, so the call blocks waiting for a user to answer an
authorization dialog that a piped process never shows. The same read is on the
path that starts a session, where it would have frozen the TUI.

Fixed with one `SecKeychainSetUserInteractionAllowed(0)` before the first
Keychain call any store can make: the call now fails cleanly, resolution falls
back to the environment, and `describe` says so. Re-run: exit 0, correct output,
no hang.

**Declared with a bare `#[link(name = "Security", kind = "framework")]` extern
rather than a new direct dependency** — the framework is already linked via
`keyring`. Accepted by the orchestrator: one FFI call with a trivial signature
is a smaller commitment than a second crate on the secret path.

**The cost, stated rather than hidden:** a credential filed by hand with
`security add-generic-password` is not read. Storing it *through* Glasshouse is
what puts this binary on the item's ACL.

#### CI evidence — and the red run that preceded it

`5b3a4cf` went **red on Linux, Windows and lint** while macOS stayed green.
`PROBE_ACCOUNT` is read only inside the `#[cfg(target_os = "macos")]` backend
but was declared outside it, so on every other target it is dead code, which
`-D warnings` makes fatal. One constant, three red jobs, and a class of defect
macOS CI structurally cannot catch.

Fixed in `6cb9bf1` by giving the constant the same gate as the module that
reads it. **CI green on Linux, macOS, Windows and lint** there.

The fix was verified *before* pushing, by flipping every `target_os = "macos"`
in that file to `"linux"` so this machine compiles the fallback arms instead —
the same path the other platforms take. `rustup target add
x86_64-unknown-linux-gnu` was tried first and did not work: the target installs
but its `core`/`std` did not resolve. The cfg flip needs no toolchain at all and
is recorded as practice §18.

#### A durability caveat, measured rather than assumed

| what | result |
|---|---|
| store and read in one process | reads |
| store, read from a second invocation of the same binary | reads |
| store, **rebuild the binary**, read | **does not read** |

The ACL binds to the binary's code identity, so an **unsigned** build — which
Glasshouse is today — breaks the link on rebuild. For a signed release the
designated requirement should be the signing identity and stable across
versions; **that is not verified and is not claimed.** When configuration
records a credential the store will not return, `doctor` says so and says what
to do.

#### Production reachability — the one line the packet scoped out

The packet forbade `main.rs`, so the batch left the launch path building an
`EnvironmentSecretStore` and flagged it rather than reaching into a forbidden
file. **The orchestrator made that change**: `launch_session` now builds
`PreferNativeSecretStore::detect()`. Without it, "prefer the macOS Keychain"
would have been true of the store, of `doctor` and of settings, but not of
`glasshouse run` — and a mechanism with no production caller does not get its
box.

Verified against the built binary: `glasshouse doctor` exits 0 and reports
`credentials resolve from: the macOS Keychain, then the process environment` —
which is line 2's labelled fallback, in the shipped output.

#### Mutations

The orchestrator's own: making `PreferNativeSecretStore::detect` never prefer
the native store **failed two tests**, including
`macos_only_a_keychain_credential_reaches_a_launch_overlays_environment`.

#### A forbidden file that could not be avoided, and was flagged

`SecretRef` gaining an `OsCredential` variant breaks every exhaustive match on
it, including one in a test fake inside `profile/mod.rs`. Production code in
that module is untouched. This is the same class as `Provider` gaining a field
two batches earlier: adding a variant to a shared enum is not a local change,
and the honest response is to flag it rather than pretend the file was not
edited.

### Phase 9E — secret storage (eight of thirteen)

Contract: Given a provider credential, when Glasshouse needs it to launch a
harness, it resolves the value from a named source at the moment of use and
hands it only to that child process — while nothing anywhere stores, logs,
renders, serializes, or persists the value itself.

State: **COMPLETE for eight lines.** Native keychains and the settings
deletion path are deferred; see the end.

Production evidence:
- `secret/mod.rs` — `SecretRef` (a *source*, never a value), `SecretStore`,
  `Secret`, `EnvironmentSecretStore`, `redact`.
- `provider/mod.rs` — `Provider::secret_refs`, returning references only.

**The boundary is structural, not disciplinary.** `Secret` has no `Display`,
no `Deref`, no `AsRef<str>`, is neither `Serialize` nor `Deserialize`, and its
`Debug` writes a fixed marker. The only way out is `expose()`, named so it
reads wrong when it is wrong. `SecretRef` has no variant able to carry a value,
so configuration and diagnostics may hold one freely.

Regression evidence (twelve tests):
- `a_secret_ref_names_a_source_and_never_carries_a_value` — scans the enum's
  own declaration, so a future `Keychain { service, account }` passes and a
  `Literal { value }` does not.
- `debug_on_a_secret_prints_a_fixed_marker_and_never_the_value` — asserts an
  empty value and a 4096-character one render **identically**, which is the
  only form of that assertion a length cannot slip past. A length is a real
  leak: it narrows a key space.
- `is_present_reports_presence_without_resolving_a_value` — behavioural, plus
  a scan of the method's own body for `Secret`/`expose`/`to_owned`, so a later
  "simplification" to `self.resolve(..).is_some()` fails the suite.
- `resolve_reads_the_value_from_the_named_variable_at_the_moment_of_use`,
  `resolve_returns_none_for_an_unset_variable`,
  `a_secret_has_no_display_no_deref_and_no_asref`,
  `a_secret_is_not_serializable`, `nothing_in_this_module_writes_to_disk`,
  `redact_replaces_recognised_credential_shapes`,
  `redact_leaves_ordinary_text_alone`,
  `a_provider_yields_one_secret_ref_per_credential_variable`,
  `the_source_scans_would_catch_a_violation` — proves the scans fire on a real
  violation and stay quiet on a doc comment or test code that merely mentions
  one.

Non-vacuity: **three mutations, three kills** — `Debug` printing the value,
`Debug` appending a length, and `is_present` resolving. A fourth (a
value-carrying field on `SecretRef`) **could not compile**, which is the type
system holding the property rather than the test; the scan's own
falsifiability is proved separately by the test above.

**What the specialist refused, recorded because refusals are the evidence
here.** A `SecretRef::Literal { value }` variant, wanted first for tests and
then inevitably for "just paste the key in the config". A memoising cache in
the store (`EnvironmentSecretStore` is a unit struct and structurally cannot
hold a value). An error type carrying the offending value. A helpful
`Debug`. Keeping four characters in `redact` so a reader could tell two keys
apart. And `assert_eq!` on `expose()` in tests — because `assert_eq!` prints
both sides on failure, which would put a value in CI output the first time a
real one was involved.

**A bare token is deliberately not redacted.** A JWT or opaque session key
carries no identifying prefix; redacting every long token on sight would eat
git SHAs, base64 payloads and build identifiers — the exact failure
`redact_leaves_ordinary_text_alone` exists to prevent. `Bearer` keeps only the
scheme: `Authorization: Bearer [redacted]`. The specialist's first draft
asserted a bare JWT *was* redacted; the test was wrong and the behaviour was
right, and the test now says so rather than quietly dropping the case.

Missing evidence:
- **Lines 437, 438, 439** (macOS Keychain, Windows Credential Manager, Linux
  Secret Service): deferred deliberately. Each needs a dependency decision —
  which is the orchestrator's, not a worker's — and per-platform proof that
  one macOS machine cannot honestly provide.
- **Line 441** (a clearly labelled fallback): means nothing until a native
  store exists to fall back *from*.
- **Line 446** (delete a stored credential from settings): needs the settings
  UI, Phase 2D.
- **`SecretRef` derives no serde impl.** The type is *safe* to serialize —
  every field is a name — and the structural test proves it. Deriving today
  would fix an on-disk shape for a type nothing yet stores, which is a
  configuration-schema commitment belonging to the phase that first stores
  one. Accepted as the specialist argued it.
- Nothing here is reachable from the shipped binary yet, deliberately: a
  profile that can carry a credential is **Phase 9F**.

---

### Phase 9E — Prefer Windows Credential Manager for user-entered provider secrets on Windows when available (line 441)

State: **LOCALLY VERIFIED** — orchestrator ruling, batch 51. **The map box is
NOT ticked.** This is a claim about Windows and nothing has executed on Windows.

Contract: Given a user-entered provider secret on Windows, when Glasshouse
stores or reads it, it prefers Windows Credential Manager over the process
environment and says which store it used — while never linking a store that
only pretends to secure anything, and falling back observably when the native
store is unavailable.

Production: `crates/glasshouse/Cargo.toml` enables `windows-native` under
`[target.'cfg(target_os = "windows")'.dependencies]`, mirroring the macOS
block; `crates/glasshouse/src/secret/native.rs` now covers macOS and Windows
through **one** `backend` module, with only `LABEL`, `NATIVE_FIRST_LABEL` and
`silence_authorization_dialogs` differing per platform, so `classify` stays the
single choke point between a `keyring::Error` and anything a user sees.

Regression: `crates/glasshouse/tests/secret_native.rs` (7 tests) and
`secret::native` lib tests (17). The macOS round trips were confirmed **not
skipping** — re-run with `--nocapture` and grepped for `SKIPPED`, no matches —
so they really executed against the login keychain, `security` CLI witness
included.

Mutations — four KILLED by the worker; `drop-the-windows-backend` re-run by the
orchestrator in the integrated tree, failing
`the_manifest_never_declares_keyring_without_a_real_backend` at
`secret_native.rs:127`. That one matters most from a macOS host: it proves the
manifest guard protects the *Windows* feature specifically. `use
keyring::macos::default_credential_builder` → `keyring::mock::...` also KILLED
four tests, proving the mock guard is not decorative.

**Why this is not COMPLETE, stated plainly.** No test in this package has run on
Windows. The worker said so itself and did not claim otherwise. It went further
than a `cfg` flip — the Windows arms **type-check against a real
`aarch64-pc-windows-msvc` target with keyring's Windows backend compiled** —
but type-checking is not execution.

**What closes it:** one `scripts/ci-local.sh --macos --linux --windows-vm` run
with these tests green on the VM, plus the worker's mutation `m1`
(`native: NativeSecretStore::detect()` → `Err(Unavailable::UnsupportedPlatform)`)
re-run **on Windows**. If m1 SURVIVES there, the Windows store is not being
reached and that is the finding, not a formality. That run is blocked only by
the unrelated Windows deadlock now being fixed.

---

### 441 — the Windows run happened, and the store would not open (2026-09-02)

The run the entry above asked for exists now: `ff57ddb`'s three-leg gate
(`phase-54a.md`, 1908) ran `tests/secret_native.rs` and the `secret::native`
lib tests on the ARM64 VM, green — **and five of them printed `SKIPPED: the
native secure store would not open in this session`** (three lib round-trips,
two integration round-trips: `a_credential_in_the_native_store_is_readable_there_and_invisible_to_the_environment`
and `deleting_a_credential_that_is_not_there_is_success`). The manifest guard
and the refusal-shape tests ran for real; the round-trips proved nothing on
Windows, exactly the case the entry said to treat as the finding rather than
a formality. So on the VM, under the runner's ssh session as the `glasshouse`
user, `NativeSecretStore::detect()` refuses to open Windows Credential
Manager. The test prints the fact and swallows the `keyring::Error` behind
it, so this ledger cannot yet say why — the honest candidates are the session
itself (an ssh logon without an interactive token or a loaded user profile is
the usual reason `CredWrite`/`CredRead` fail on a service-style session) or
the backend feature not behaving as its type-check promised.

**State unchanged: LOCALLY VERIFIED, box not ticked.** What closes it is now
one bounded Windows package, `GH-WINDOWS-SECRET-STORE` (Red — secrets on a
platform): on the VM, run the round-trips with the refusal's `keyring::Error`
printed rather than swallowed, name the cause, and either make the CI session
one in which the store opens (an interactive-logon task or the runner
invoking the target under the user's own logon) or record that the store
genuinely cannot be reached from a non-interactive session and what the
product does then; then re-run the round-trips and the entry's mutation `m1`
on Windows. The measurement harness the flakes worker left on the VM
(`C:\ci\flake.ps1`, `report-windows-flakes.md` §11) drives single targets
there.

---

### Phase 9E — Prefer a Secret Service-compatible keyring on Linux when available (line 442) — REFUSED, premise-invalid

State: **NOT STARTED.** Returned premise-invalid by the worker and accepted by
the orchestrator. **The blocker is not the headless runner.** It is that
`keyring` 3.6.3 cannot honour the words "when available".

Read from the resolved sources, not inferred:

- `keyring-3.6.3/src/secret_service.rs:144` and `:337` reach the bus with the
  plain `SecretService::connect(session_type)`.
- `dbus-secret-service-4.1.0/src/lib.rs:209` leaves `timeout: None` for that
  constructor. `connect_with_max_prompt_timeout` exists and **keyring never
  calls it**; `SsCredential`'s methods build their own connection internally,
  so a caller cannot reach it either.
- `dbus-secret-service-4.1.0/src/prompt.rs:42` —
  `let timeout = self.timeout.unwrap_or(ONE_YEAR_SECONDS);`

So an unanswered unlock prompt blocks the calling thread for **up to a year**,
and a locked collection is not an error on the read path: `secret_service.rs:363`
unlocks every locked match before reading, and `get_collection` (`:493`) unlocks
before writing. There is no Linux equivalent of
`SecKeychainSetUserInteractionAllowed(0)` in keyring 3.6.

**And a probe cannot see it coming**, which is what makes this disqualifying
rather than merely awkward. Probing an account nothing ever writes matches zero
items, so the probe returns `NoEntry` before anything needs unlocking:
`detect()` would report the store healthy, `describe()` would print "a Secret
Service keyring, then the process environment", and the **first real credential
read would freeze the TUI**. That is the silent-degradation shape this phase
exists to forbid, inverted.

The Linux dependency was therefore **deliberately not enabled**. Doing so would
link a store that can hang a launch, on a platform where nothing has executed
it, and would add `libdbus-1-dev` + `pkg-config` to every Linux build
(`libdbus-sys 0.2.7`'s `build.rs` panics without them) to gain a capability the
ledger cannot claim. `native.rs`'s design record now carries this reasoning
under "Which platforms, and why not the third".

**The one question to answer before 442 is re-attempted:** keyring 4.x
restructures into `keyring-core` plus per-store crates and its default `v1`
feature selects `zbus-secret-service-keyring-store`, which is pure Rust and
would remove the `libdbus-1-dev` requirement. **Whether it bounds the unlock
prompt is unverified.** The worker stopped rather than turn a bounded packet
into a dependency-upgrade evaluation, which was the right call.

#### 441 — Windows ran, and the store still did not open (batch 51, orchestrator)

`scripts/ci-local.sh --windows-vm` on `624205a`: **build PASS, test PASS, msrv
PASS, zero timeout notices.** The deadlock that blocked Windows evidence is
gone. And 441 still does not close, for a reason the run made visible:

    Running tests\secret_native.rs
    SKIPPED: the native secure store would not open in this session
    SKIPPED: the native secure store would not open in this session

**The Windows Credential Manager never opened.** The VM's CI session is a
**service session** — `tasklist` reports the CI processes in session `Services`,
0 — and Credential Manager is per interactive logon. Every test that needs a
real round trip skipped; what passed was the platform *gating*.

**This is why the box is still not ticked, and the distinction matters.**
`secret::native::tests::detect_offers_a_native_store_on_exactly_the_platforms_
with_a_backend` passes on Windows by asserting only that the refusal is not
`UnsupportedPlatform` — `StoreUnreachable` satisfies it. That is a correct test
of the gating and it is **not** evidence that Credential Manager works.

**A hazard worth naming for every future platform claim:** these tests skip
*loudly* — they print `SKIPPED` — which is what `agent-sdlc.md` asks for. But
cargo still counts them as passing, so the gate summary says
`PASS test (windows) / test` and a reader who stops there would conclude Windows
credential storage is proven. It is not. **Read the skips, not the summary.**

**What would close 441:** a Windows runner executing under an interactive logon
(autologon plus a scheduled task set to run only when a user is logged on, or an
equivalent), so Credential Manager exists to open. That is a CI-image change,
not a code change, and it is the only thing standing between this line and
COMPLETE — the production code, the mutations and the manifest guard are all in
place and green.

State remains **LOCALLY VERIFIED**.

---

## 441 — CLOSED 2026-09-02 (`GH-WINDOWS-SECRET-STORE`, Opus 5 high, Red): the store opens under a logon that has one, and the round-trips ran on Windows

### Phase 9E — Prefer Windows Credential Manager for user-entered provider secrets on Windows when available (line 441)

Contract: Given a user-entered provider secret on Windows, when Glasshouse stores or reads it, it prefers Windows Credential Manager over the process environment and says which store it used — while never linking a store that only pretends to secure anything, and falling back observably, with the platform's own reason, when the native store is unavailable.

State: **COMPLETE** — ruled 2026-09-02. The two entries above asked for one thing: the round-trips executing on Windows, and mutation `m1` re-run there. Both happened on the ARM64 VM. The refusal the first green Windows leg surfaced was the **session's**: Windows OpenSSH builds a public-key session's token with an S4U logon that carries no primary credentials, and Credential Manager is scoped to a logon session, so every `CredReadW`/`CredWriteW`/`CredDeleteW` from the runner's ssh session answered `1312 ERROR_NO_SUCH_LOGON_SESSION` — measured directly against `advapi32` with no Rust in the call. The product was reaching a real Credential Manager and being refused exactly as its fallback is designed to be; the runner was proving nothing. The runner now runs the unchanged `run-glasshouse-ci.cmd` through a scheduled task registered `-LogonType Interactive` under the same CI user (`scripts/dev/windows/run-in-session.ps1`, invoked by `scripts/dev/glasshouse-windows-ci`; no password, no new credential anywhere; it fails loudly with no interactive session rather than running in session 0 and skipping quietly; it never kills a run). Under that logon the two targets ran **ten times each with zero `SKIPPED`** (lib `secret::native` 19/19 ×10, `tests/secret_native.rs` 7/7 ×10; the five tests that skipped on `ff57ddb` each ran 10 of 10), and `a_credential_stored_in_the_native_store_resolves_and_deletes` has `cmdkey /list` as an outside witness — Credential Manager's own CLI saw the item appear and disappear.

Production evidence:
- `crates/glasshouse/src/secret/native.rs` — `Unavailable::StoreUnreachable(StoreRefusal)`: the refusal now carries `classify`'s fixed text plus the platform's own status, taken from the only two `keyring::Error` variants whose payload is a status code (`PlatformFailure`, `NoStorageAccess`) and never from store data; `Unavailable::reason()` renders it, and `integrations::doctor` and the Settings overlay already print `reason()` after *"native secure store: unavailable"* — so a user on a session without a credential store now reads `Windows ERROR_NO_SUCH_LOGON_SESSION` beside the fallback
- `crates/glasshouse/src/secret/native.rs` — `backend::probe`, `backend::platform_status`; `PreferNativeSecretStore::detect` unchanged in shape
- `scripts/dev/glasshouse-windows-ci`, `scripts/dev/windows/run-in-session.ps1` — the runner's interactive-logon path (process, not product; recorded because the platform claim rests on it)

Regression evidence:
- `secret_native::a_credential_in_the_native_store_is_readable_there_and_invisible_to_the_environment`, `secret_native::deleting_a_credential_that_is_not_there_is_success` — ran on the VM, 10/10, not skipped
- `secret::native::tests::a_credential_stored_in_the_native_store_resolves_and_deletes`, `the_native_store_is_preferred_over_the_environment`, `a_native_store_credential_reaches_a_launch_overlays_environment` — ran on the VM, 10/10, not skipped
- `secret::native::tests::an_unreachable_stores_reason_carries_the_platforms_own_status` (new), `a_store_error_never_carries_anything_the_store_returned` (extended to the carried status)

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| `PreferNativeSecretStore::detect`: `native: NativeSecretStore::detect(),` → `native: Err(Unavailable::UnsupportedPlatform),` — compiled and run **on the Windows ARM64 VM** under the interactive logon | `m1` (drop-the-native-store) | **killed** | `secret_native::a_credential_in_the_native_store_is_readable_there_and_invisible_to_the_environment`; also `secret::native::tests::the_native_store_is_preferred_over_the_environment`, `a_native_store_credential_reaches_a_launch_overlays_environment` |

> m1 observed (VM): `assertion left == right failed — left: None, right: Some("Windows Credential Manager")` (`tests/secret_native.rs:335`); `the native store must win` (`native.rs:1465`); `a direct-provider profile with a resolvable credential: CredentialUnavailable { … }` (`native.rs:1575`). `test result: FAILED. 17 passed; 2 failed` and `FAILED. 6 passed; 1 failed`. Restore verified by sha256, both sides `15dd73b0…6d81`; the VM re-synced to the restored tree. On the old session-0 runner all three of these tests SKIPPED, so `m1` would have SURVIVED — the finding the batch-51 entry said to expect.

Gates: macOS `secret::native` 19/19 and `secret_native` 7/7 with `--nocapture` and no `SKIPPED`; fmt, clippy clean; `blast-radius.sh --targeted` exit 0. The worker's full blast radius went red on the lib target's PTY-fixture family (*the fake harness never exited*, six tests) and was attributed to HEAD under a loaded machine with two runs (§34) — the gatekeeper-scan family this project has recorded before, not this package's.

Recorded scope limits — stated by the worker, not discovered later:
- proves Credential Manager for a logged-on user; a Windows service, a Remote Desktop session and a roaming profile are untested, and `1312` is exactly what the first of those would produce — the refusing path is now legible, and it is still the refusing path
- one VM, one Windows build (ARM64), one user profile, an empty credential store
- `describe()`'s three fixed labels did not widen (an `integrations` test pins them); the reason is on the next line, in `reason()`
- `run-in-session.ps1` has no test of its own; its evidence is that it ran the real `build` and `test` batches end to end and took the five skips to zero
- the report carried no ```glasshouse-facts block; every artifact above is quoted from its body

---

---

## 442 — LOCALLY VERIFIED 2026-09-05; the tick waits on the round trip against a real Secret Service

`GH-SECRET-SERVICE-BACKEND` (Opus xhigh, **Red**), report **`.agent-runtime/report-secret-service-backend.md`**; independent verifier `GH-SECRET-SERVICE-VERIFY` (Opus high), report **`.agent-runtime/report-secret-service-verify.md`**, **VERDICT: ACCEPT** — seven claims, six CONFIRMED against source and one CONFIRMED for the two mutations the packet required re-run. The user's decision of 2026-09-03 §2 approved the line; `.agent-runtime/answers/linux-keyring-buildhost.txt` (2026-09-05) accepted the build-host cost and a CI fixture.

### Prefer a Secret Service-compatible keyring on Linux when available. (line 442)

Contract: Given a Linux host, when Glasshouse resolves a credential that names the OS store, it uses a Secret Service-compatible keyring where one is reachable and unlocked, and otherwise refuses within a bounded time with a reason a user can act on — while preserving that a locked collection or an unanswered unlock prompt never blocks the calling thread, that no credential is ever written to a store that persists nothing, and that macOS and Windows behave byte-for-byte as they do today.

State: **LOCALLY VERIFIED** — production and regression evidence for every clause but one: the round trip against a live, unlocked Secret Service is not proven on any platform this build's gates reach. **`GH-SECRET-SERVICE-CI-FIXTURE`** (a `dbus-run-session` with an unlocked keyring daemon on the Linux leg and the ubuntu cells) owes it, and the box stays ☐ until then.

**The crate, ruled by the orchestrator 2026-09-05.** `dbus-secret-service` 4.1.0, `default-features = false`, `crypto-rust`, connected with `connect_with_max_prompt_timeout(EncryptionType::Dh, 0)` — **not** the `secret-service`/zbus route the earlier `secret-service-route` answer named. The worker judged both on the four properties the packet set and the verifier confirmed the deciding half on the crate taken: `Collection::is_locked` reads the `Locked` property without unlocking (`collection.rs:41`), and with a zero timeout `prompt.rs:43` returns `Err(Error::Prompt)` *before* `proxy.prompt()` is ever called — the Linux `SecKeychainSetUserInteractionAllowed(0)` this file's design record said did not exist. It exists; `keyring` 3.6.3 never calls it. The comparison's other half — that `secret-service` 5.2.0's blocking `delete` calls `ensure_unlocked` unconditionally and its prompt wait has no timeout at all — is recorded as the worker's reading of that crate's source, **UNCHECKED** by the verifier because the crate is not in this machine's registry. Every D-Bus reply carries a 2-second timeout (`proxy/mod.rs:17`); the probe is bounded by the connect plus a handful of those. `cargo tree --target x86_64-unknown-linux-gnu -e normal` adds 19 crates, all from this dependency, and **no executor, no `futures`, no zbus** — measured and re-measured. `libdbus-sys`'s `build.rs` panics without `pkg-config` finding `dbus-1`, so `libdbus-1-dev` + `pkg-config` are on the build host: the Linux container in `ci-local.sh` and the ubuntu cells in `ci-extended.yml` gained them in `367d344`; GitHub's Ubuntu images ship `dbus` and `pkg-config` but not the `-dev` headers (verifier, from `actions/runner-images`).

Production: `secret/native.rs :: backend` (Linux arm) — `probe` reads the bus, the service, then the default collection's `Locked` before touching an item; `get`/`exists`/`set`/`delete`; `refusal` → `Unavailable::StoreUnreachable(StoreRefusal)` with a classification per case (no session bus, no provider, collection locked, other) whose first sentence is the instruction the user's decision asks for; `LABEL`/`NATIVE_FIRST_LABEL` name the store honestly; `PROMPT_TIMEOUT_SECONDS = 0`. Manifest: `[target.'cfg(target_os = "linux")'.dependencies]` in `crates/glasshouse/Cargo.toml`, the comment that said *Linux is absent on purpose* rewritten to what is now true; `[workspace.dependencies]` gains the crate.

Regression (macOS + Linux, the Linux half run for real in `docker rust:1.98.0` with `libdbus-1-dev`): `secret::native::tests::a_locked_collection_is_refused_before_anything_can_prompt` (a fake that never answers), `::the_secret_service_backend_can_never_wait_for_an_unlock_prompt`, `secret_native::on_linux_a_keyring_that_cannot_be_reached_refuses_quickly_and_says_what_to_do`, `::on_a_platform_with_a_backend_the_only_honest_refusal_is_that_it_would_not_open` (now true of Linux too — the old *no backend* assertion is gone by design), the manifest scan unchanged and passing, and `secret_service_backend()`'s five-landmark source scan so a truncated slice fails loudly. `--lib secret` 51 (macOS) / 46 (Linux), `--lib secret::native` 23, `--test secret_native` 9; `msrv-check.sh` at 1.88 clean.

Mutations (verifier's line numbers, re-derived at `native.rs:1651` and `:1562`; the report's are stale by 18): `skip-locked-check` — **KILLED** by `a_locked_collection_is_refused_before_anything_can_prompt`, the fake's `proceed` reached; `unsupported-not-unreachable` — **KILLED** by two `secret_native` tests; `set-succeeds-without-writing` (the macOS arm, proving the round trips are not vacuous) — **KILLED** three ways; `prompt-timeout-nonzero` (`0` → `5`) — **KILLED** on both platforms. Two false KILLEDs from denied warnings were caught by reading the compiler and discarded (§80 case 4).

Limits: the live round trip (above); `cargo check --target x86_64-unknown-linux-gnu` cannot run on this host because `ring` needs a C cross-compiler — pre-existing, not this dependency — and was replaced by the native Linux run, which is stronger. **Debt, not a defect:** `backend::get` swallows a lock that arrives *after* `detect()` succeeded via `.ok()?`, so `resolve` returns `None` and `PreferNativeSecretStore` falls back to the environment — no hang, no leak, the same `Option` shape the macOS and Windows arms have.
