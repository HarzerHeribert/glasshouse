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
