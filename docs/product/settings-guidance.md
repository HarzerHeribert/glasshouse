# Settings guidance — the gateway, the subscriptions, and the save key

Status: **plan only.** Nothing implemented. Written 2026-09-09 from a reading of
`crates/glasshouse/src/shell/state/settings/{mod.rs,keys.rs}`,
`crates/glasshouse/src/shell/mod.rs`,
`crates/glasshouse/src/commands/subscriptions.rs`,
`crates/glasshouse/src/config/{profile.rs,entitlement.rs,loading.rs}` and
`crates/glasshouse/src/gateway/`, plus a live run of the shipped binary in a
real terminal and timings taken on this machine.

The user, verbatim:

> the settings really dont clearly show me how i want to configure my gateway
> and what is important and how to connect my existing subscriptions. should be
> a tips and tricks. then having to press w to save is bad and saving seems to
> take ages aswell.

Four complaints. **Only the last one is about presentation.** The first two are
capability gaps wearing a usability costume: the gateway and the subscriptions
are not badly explained in Settings, they are **absent from it**, and one of the
two has a control that actively destroys the hand-written configuration a user
had to write because Settings could not. This document separates the four, and
says which of them a tip could ever have fixed. None of the first three.

## What this extends, and what it depends on

`settings-picker.md` is approved and is the substrate: every setting becomes a
row in one flat searchable list, with kinds `Toggle · Text · Secret · Path ·
Choice · Action`, `unavailable_reason` carried from pane, and Enter as the one
edit key. **Nothing here contradicts it and nothing here restates it.** Where a
recommendation below only makes sense on top of that list, it says so.

| § | recommendation | depends on the picker? |
| --- | --- | --- |
| 2 | make gateway backend selectable at all (`Choice`, not free text) | **no** — a defect fix, land it first |
| 2 | show a profile's entitlement on its row | **no** |
| 2 | entitlement rows exist at all | yes (needs a row kind, not a 7th tab) |
| 3 | subscription rows with a `connect` / `check` Action | yes |
| 3 | never print "valid"; print what was measured | **no** — copy fix, applies today |
| 4 | commit-on-Enter instead of `w` | yes (Enter is the picker's edit key) |
| 4 | per-row layer instead of `w`/`W` | yes (the row is the unit) |
| 4 | take `Discovery::run` off the **save** path entirely | **no** — independent; the open path was fixed concurrently, this half was not |
| 5 | one-line "why" under the selected row | yes (the picker's second mockup already draws it) |
| 5 | the task-shaped empty state | yes |

The order that follows from that column: **the four "no" rows are their own
package and go first.** They are small, they are defects rather than
improvements, and three of the four are invisible to the picker's refactor.

## 1. What Settings holds today, and what it does not

Six sections — Harnesses, Integrations, Providers, LaunchProfiles, Routing,
Memory (`settings/mod.rs:308-315`), 34 bindings across 20 distinct letters, 8
mutually exclusive bottom panels, 2,759 lines. Those counts are
`settings-picker.md`'s and they still hold (`wc -l`: 1,291 + 1,468 = 2,759).

The part that matters here is not the count but the **overloading**. Read
`settings/keys.rs:373-525` as a table and the same letter means different things
depending on which of six tabs has focus:

| letter | in Providers | in LaunchProfiles | in Routing |
| --- | --- | --- | --- |
| `a` | add a provider | add a profile | — |
| `d` | delete the row | remove the profile | disposable routing |
| `m` | fetch models | — | routing model |
| `c` | credential | — | max cost |
| `f` | free-tier models | — | prefer free |
| `p` | — | (profile field) | pin |

The footer says, in full: `tab section  up/down move  space toggle  section keys
edit  w save  W project  r setup  esc close` (`view/chrome.rs:345-346`).
"section keys edit" is the whole documentation of those twenty letters. There is
no help overlay in the shell — `Overlay` has ten variants
(`shell/state/mod.rs:76+`) and none of them is help — and `?` is unbound.

That is the picker's problem and the picker solves it. **The gap this document
is about is what is not in those six sections at all.** Measured on this
machine, from the shipped binary:

| thing | how many exist | how many Settings shows |
| --- | --- | --- |
| harnesses | 8 | 8 |
| providers | 4 | 4 |
| launch profiles | 13 | 13 |
| **entitlements** | **7** (`glasshouse status`) | **0** |
| **subscription accounts** | **3, all `present`** (`glasshouse subscriptions status`) | **0** |
| **the gateway** | 1, ephemeral, per launch | **0** |
| the CLIProxyAPI broker binary | adopted or not | **0** |

`grep -c entitlement crates/glasshouse/src/shell/state/settings/mod.rs` returns
**0**. The word does not occur in the file.

So the user's sentence is literally accurate. Settings does not show how to
configure the gateway, and it does not show how to connect a subscription,
because it contains neither.

## 2. Journey A — "configure my gateway"

### The decision this section makes: stop calling it a gateway setting

**There is nothing about the gateway to configure, and the UI should say so
once, plainly, instead of implying a settings surface that does not and should
not exist.**

From `gateway/mod.rs`: the gateway binds `Ipv4Addr::LOCALHOST` only
(`GATEWAY_INTERFACE`), on `EPHEMERAL_PORT = 0` — the OS picks it — with a
freshly generated 32-byte per-instance token (`TOKEN_BYTES`), and it is started
by `start_if_required(profiles, ...)`, which returns `Ok(None)` and **binds no
listener at all** unless some launch profile's backend is
`BackendResource::GlasshouseGateway`. There is no port to choose, no address to
bind, no token to manage, no daemon to start or stop. `glasshouse gateway
--help` offers exactly one subcommand, `pairs`, and its own help text says it
"Reads nothing but the binary."

What the user actually means by "configure my gateway" is: *make this harness
run against my subscription, through Glasshouse, so Glasshouse can see and route
it.* That is **three settings, none of which is the gateway**, and today two of
the three cannot be expressed in the UI at all.

### Step 1 — an entitlement must exist. Today: impossible in the UI.

An entitlement is a `[entitlements.<name>]` table (`config/entitlement.rs:508+`):
`kind`, `vendor`, `subscription_broker`, `credential`, `provider`, plus the
allow/deny rules and the spend ceiling.

`grep -rn entitlements_mut crates/glasshouse` returns **seven call sites and
every one of them is a test** (`config/tests/part_b.rs`, `tests/entitlement_pool.rs`,
`tests/firewall_reducer.rs`, `tests/firewall_local_reducer.rs`). There is **no
production writer of an entitlement anywhere in Glasshouse** — not in the TUI,
not in `commands/entitlements.rs` (read-only), not in the onboarding wizard,
whose own module doc says "the provider step configures from a built-in template
and stops there (the gateway is Phase 9D)" (`onboarding/mod.rs:15`).

The only way to create one is to hand-write TOML. **This user already did.**
Their `~/Library/Application Support/glasshouse/config.toml` carries seven
`[entitlements.*]` tables and three `[profiles.subscription-*-pane]` entries with
`kind = "glasshouse-gateway"`. They did not choose the config file over the UI;
the UI had no way to say it.

**Where it should appear.** As rows in the picker, under an `entitlements/`
path — not a seventh tab:

```
entitlements/claude-max/kind              Choice   claude
entitlements/claude-max/vendor            Choice   claude
entitlements/claude-max/broker            Choice   cliproxyapi
entitlements/claude-max/connect           Action   →          connected
entitlements/new                          Action   →          add an account
```

`entitlements/new` is the row that closes the gap. It asks for a name, then
`kind` and `vendor` from the four and five variants that already exist as enums
(`EntitlementKind`, `EntitlementVendor`), and writes the table. Two `Choice`
editors and a `Text` editor — the kinds the picker already defines. The rules
(`allow_harnesses`, `deny_tiers`, `spend_ceiling_tokens`) stay TOML-only for now
and the row says so; a person adding their Claude plan does not need them, and
inventing UI for them before anyone has asked is exactly the speculative
capability the freeze forbids.

### Step 2 — the subscription must be logged in. Today: CLI only. See §3.

### Step 3 — a launch profile must select the gateway. Today: a defect.

This is the sharp one, and it needs no picker to fix.

`ProfileBackend` has three variants: `Native`, `DirectProvider { provider }`,
`GlasshouseGateway` (`config/profile.rs:22-28`). The Settings backend editor
parses **two** of them (`settings/keys.rs:1199-1218`):

```rust
let backend = if typed.is_empty() || typed.eq_ignore_ascii_case("native") {
    Some(ProfileBackend::Native)
} else if self.providers.iter().any(|row| row.name == typed) {
    Some(ProfileBackend::DirectProvider { provider: typed.clone() })
} else {
    None                       // -> "`{typed}` is not `native` or a configured provider name"
};
```

There is no arm producing `GlasshouseGateway`. Typing `gateway`, or
`glasshouse-gateway`, or anything else, is refused by name. **The Settings UI
cannot create a gateway-backed profile, and the error message it gives does not
mention that the gateway exists.**

Worse, and this is the defect: `start_edit_profile_backend`
(`settings/keys.rs:1036-1052`) pre-fills the editor from the current backend and
maps the gateway to the **empty string**:

```rust
ProfileBackend::GlasshouseGateway => String::new(),
```

and empty parses back as `Native`. Driven live on this machine — the shipped
release binary, real config, Launch Profiles, cursor on
`subscription-claude-pane`, `b`:

```
│Backend for `subscription-claude-pane`: `native` or a configured provider name: _    │
```

An empty field, on a profile whose backend is `glasshouse-gateway`, under a
prompt that does not name the gateway as an option. **Press Enter and the
profile becomes Native.** Its `entitlement = "claude-max"` then violates
`ProfileConfig::resolve`'s own rule and the profile stops loading with
`EntitlementRequiresGateway` (`config/profile.rs:142-145, 293-296`) — a config
the user hand-wrote, destroyed by opening the editor for it and pressing the
key that everywhere else in this UI means "keep what is there".

**Decision: the backend field is a `Choice`, never `Text`.** The choices are
enumerable and small: `native`, the gateway, and one per configured provider.
That removes the parse, removes the error string, removes the empty-buffer
downgrade, and makes the gateway visible to somebody who did not already know it
existed — which is the user's actual complaint. This is the fix to land first,
and it is worth a mutation of its own: replace the `Choice` with the old text
parse and a test that opens the backend editor on a gateway-backed profile and
presses Enter must fail on the resulting backend still being the gateway.

**Second decision: a profile row shows its entitlement.** The live Launch
Profiles table renders name, harness, backend, model, approval, enabled
(`view.rs:1897` maps the backend to the word `gateway`) — and truncates at the
right edge, so several rows read `enab`, `ena`, `enable`. It does **not** render
`entitlement` at all. `subscription-claude-pane` shows `gateway` and
`claude-sonnet-4-6` and gives no hint that `claude-max` is the account paying
for it. Under the picker each field is its own row and this is free:

```
profiles/subscription-claude-pane/harness       Choice   pane
profiles/subscription-claude-pane/backend       Choice   glasshouse gateway
profiles/subscription-claude-pane/entitlement   Choice   claude-max
profiles/subscription-claude-pane/model         Text     claude-sonnet-4-6
```

with the `entitlement` row's `unavailable_reason` reading *"only a
glasshouse-gateway backend can select an entitlement"* whenever the backend row
above it is not the gateway. That sentence already exists, word for word, as the
`EntitlementRequiresGateway` error text; showing it as a reason on a greyed row
is strictly better than raising it at load time.

### What the UI should say, once

One line, on the `backend` row's help, and nowhere else:

> **glasshouse gateway** — Glasshouse proxies this session's requests on
> loopback, so it can route, meter and fail over. Nothing to configure: the port
> and token are per launch. Pick the entitlement below to say which account pays.

## 3. Journey B — "connect my existing subscriptions"

### What exists

`glasshouse subscriptions` has four subcommands: `status`, `login <provider>
--entitlement <NAME>`, `logout`, `adopt-binary <PATH>`. OAuth happens inside a
CLIProxyAPI sidecar, one per entitlement, loopback only, and Glasshouse never
sees a token (`gateway/subscription_broker.rs:1-5`).

The steps, in the order they must actually happen:

1. **Adopt the broker binary.** `glasshouse subscriptions adopt-binary <PATH>`,
   or set `GLASSHOUSE_CLIPROXYAPI_BIN`. Without it, `login` fails with
   *"CLIProxyAPI is not adopted"* (`commands/subscriptions.rs:392`). Glasshouse
   ships no binary and downloads none — the user supplies a verified one, which
   is hashed and copied under a `sha256-…` version directory.
2. **Create the entitlement** — §2 step 1. `login` refuses an unknown name
   ("entitlement `{name}` is not configured"), refuses one not backed by
   `cliproxyapi`, and refuses a `kind`/`vendor` that disagrees with the provider
   you named (`validate_entitlement`, `validate_kind_vendor`,
   `commands/subscriptions.rs:125-175`).
3. **`glasshouse subscriptions login anthropic --entitlement claude-max`.** This
   spawns the broker with `-claude-login` / `-codex-login` / `-antigravity-login`
   and waits up to fifteen minutes for the browser flow.
4. **Point a launch profile at it** — §2 step 3.

Steps 1 and 3 are genuinely CLI: one takes a filesystem path to a binary the
user must obtain themselves, the other opens a browser and blocks. Neither
belongs inside a full-screen TUI overlay. **Decision: they stay CLI, and
Settings' job is to say so at the exact moment the user needs to know**, as
`Action` rows that print the command rather than run it:

```
subscriptions/broker            Action   adopted (sha256-1f0c…)
subscriptions/claude-max        Action   connected · credential present
subscriptions/gemini-subscription  Action  connected · credential present
subscriptions/chatgpt-subscription Action  connected · credential present
```

Selecting one shows, in the help line, the literal command to run in another
terminal. A row that is not connected shows the `login` line; a row whose broker
is not adopted shows the `adopt-binary` line and is the only row not greyed out,
because it is the one that has to happen first.

### The honesty problem, and it cost a real benchmark

`subscriptions status` reports **credential presence, not validity**.
`auth_present` (`commands/subscriptions.rs:214-230`) returns true when the
entitlement's auth directory contains **at least one file**. Its own unit test
says so out loud (`subscriptions.rs:503-512`):

```rust
fn status_checks_presence_without_reading_token_contents() {
    …
    fs::write(&token, b"not even valid json").unwrap();
    assert!(auth_present(&auth).unwrap());
}
```

An expired OAuth token is a file. It reads `present`. On 2026-09-08 that cost
this project a benchmark run: three accounts reported `present`, one of them was
expired, and the failure surfaced as a routing error mid-run instead of as a
red row before it.

This is right as a security boundary — reading the token to check it would put
account material back inside Glasshouse, which the whole broker design exists to
prevent — so **the fix is vocabulary and one extra Action, not a change to
`auth_present`.**

**Decision: the word "valid" never appears, and presence is never rendered as a
tick.** The row says what was actually measured:

```
subscriptions/claude-max     credential present · not checked since 2026-09-07
```

and carries a `check` Action beside it that does the only honest test available:
start the broker for that entitlement and make one minimal request through it.
That is a real network act, it takes seconds, and it is therefore an explicit
action a person asks for — never something a settings screen does on open. Its
result is stamped with a time and decays: `checked 3d ago` is not `checked`.

`glasshouse subscriptions status` gets the same treatment in its header, so the
CLI and the TUI cannot disagree about what `present` means.

## 4. Saving

### 4a. "takes ages" — found, measured, and it is not the write

The write is not slow. `UserConfig::save` is `write_atomic_toml`
(`config/loading.rs:511-521`) on a 3,558-byte file: temp file, sync, rename.
Milliseconds.

**The cost is `refresh_settings_after_save`**, which runs unconditionally after
every successful save (both save arms in the run loop) and calls
`build_settings`, whose first line is:

```rust
let discovery = Discovery::run(runtime.project());
```

`Discovery::run` (`integrations/mod.rs:349-357`) maps over
`IntegrationId::ALL` — **eleven integrations** — **serially**, and each one
resolves an executable on `PATH` and then **spawns it with `--version`** and
waits, with `DEFAULT_PROBE_TIMEOUT = Duration::from_secs(5)` each
(`integrations/version.rs:32`).

Measured on this machine, 2026-09-09, release binary:

```
$ for i in 1 2 3; do /usr/bin/time -p ./target/release/glasshouse doctor >/dev/null; done
real 0.93     real 0.92     real 0.91

$ /usr/bin/time -p ./target/release/glasshouse --version >/dev/null
real 0.00
```

Process start is free, so essentially the whole 0.92s is the discovery pass. Per
probe, timed individually:

```
claude 0.04   codex 0.02   opencode 0.23   cursor-agent 0.28
hermes 0.16   pane 0.00    cmux 0.00       ollama 0.00
```

≈0.73s of subprocess time, serial, plus resolution. **That is the "ages", and it
runs on the drawing thread.** The run loop's redraw is at the end of the action
arm — the code says so, *"NEVER `continue` here: the redraw is at the END of
this arm … Shipped once as a freeze."* — so between pressing `w` and any
repaint, nothing moves. The status line the code sets ("saved to user
configuration") is set *before* the refresh and painted *after* it, so the user
does not even see a confirmation until the freeze ends.

The **worst case is 55 seconds**: eleven probes at a 5s timeout each, which a
hung or network-mounted harness binary will produce.

### The half that has already landed, and the half that has not

`crates/glasshouse/src/shell/settings_open.rs` appeared in this tree while this
document was being written, and it fixes the **open** path: `Action::OpenSettings`
now calls `settings_open::request_settings`, which runs `build_settings` on its
own thread and delivers the rows through a channel, with a note on screen while
it waits. Its own module doc measures the same defect independently at
**1.05–1.49s** with six Node CLIs enabled, which corroborates the 0.92s above.
That package also carries a guard,
`the_open_settings_arm_does_not_build_settings_on_the_drawing_thread`.

**The save path is untouched by it, and so is the user's complaint.**
`refresh_settings_after_save` still calls `build_settings` synchronously, and it
is still reached from the `w` and `W` keystroke arms. The guard cannot catch
that: it slices `mod.rs` between `Action::OpenSettings =>` and
`Action::OpenProjectOverview =>` and scans only those lines. So after that
package, opening Settings is instant and **saving still freezes the terminal for
about a second, or up to 55 of them.**

**Decision 1 — the remaining half, and it is the one the user asked for: a save
must not run discovery at all.** Not off-thread; *not at all*. Discovery reads
`PATH` and the environment (`detect_one_with` resolves
`id.executable_candidates()`; `presence_without_executable_with` reads env
vars) — **it never reads Glasshouse configuration.** Verified by reading both.
So no save can change any discovery output, and re-running it after a save is
not a latency problem to be moved onto a thread, it is waste to be deleted.
Split `build_settings` in two: the config half re-runs after a save; the
discovery half is what `settings_open.rs` already owns, and a save does not
call it. The save then costs one atomic file write.

**Decision 2 — extend the existing guard to the save arms.** The scan that
already exists is the right shape and the wrong scope. Widen it from the
`OpenSettings` arm to every keystroke-reachable arm, or assert the property
where it actually lives: `refresh_settings_after_save` must not name
`build_settings`. This is a one-line change to a test that has already been
written and reviewed, which is cheaper than the second freeze it prevents.

Neither decision needs the picker.

### 4b. `w` — what should replace it

Today: `w` saves user-level, `W` sets `confirm_project_write` and, on `y`,
saves project-level (`settings/keys.rs:373-377`, `344-350`). **`Esc` closes
unconditionally and discards every staged edit with no warning**
(`settings/keys.rs:372`). And there is already a second, invisible class of
change: storing a credential writes to the OS keychain **immediately**, before
any save, and the status line then says *"stored `{provider}`'s credential for
{var} in {store} — save with `w` to record it"* (`shell/mod.rs`, `store_provider_credential`);
deleting one is confirmed with *"This cannot be undone by not saving."* So the
screen already mixes staged edits and irreversible ones with nothing on screen
distinguishing them.

**Decision: commit on Enter, per row. There is no save key.**

Under the picker, Enter already means "apply this row's editor". Make that
commit reach disk. Esc in an editor cancels the row; Esc on the list closes
Settings, and because nothing is pending there is nothing to lose — which also
removes today's silent-discard defect without adding a confirmation dialog.

*Why not autosave on every keystroke.* Credential values, base URLs and
comma-separated free-model lists are typed character by character. A debounced
autosave writes half a URL to a file another `glasshouse` process may load.
Commit-on-Enter is the same number of keystrokes for the user and never writes a
partial value.

*Why not save-on-close.* Esc means cancel everywhere else in this UI; making it
mean commit in one place is the kind of inconsistency that produces exactly the
mistake the save key was supposed to prevent. It also loses everything to a
killed pane.

*What replaces the safety `w` was providing.* An undo, which is what people
actually want when they reach for a save key. After each commit the status line
names what changed, where it landed, and the way back:

```
harnesses/opencode/enabled  off → on   ~/…/glasshouse/config.toml   u undo
```

`u` reverts the last committed row. That is one key with one meaning, against
today's two save keys, a confirm dialog, and a silent discard on Esc.

The irreversible class keeps its confirmation, unchanged: deleting a credential
from the OS store is not undone by an undo key and its dialog already says so.

### 4c. Making user-vs-project survive the loss of `w`/`W`

The distinction must not be a save mode, because a save mode makes it a property
of *the moment you pressed a key* rather than of the setting — which is why
`W` writes **every** staged edit to the project, when what a person almost
always wants is one of them there.

**Decision: the layer is a property of the row, shown on the row, changed on the
row.** This is nearly free: `Layer` (`config/loading.rs:836-847`) already exists
for exactly this purpose — its own doc says it is "surfaced so the Phase 2D
settings view can visibly distinguish a user-level default from a project-level
override" — every row already carries it (`HarnessRow::enabled_layer`,
`ProviderRow::layer`, `ProfileRow::layer`), and the live screen already prints
it: `enabled (user)`.

So:

```
harnesses/opencode/enabled     Toggle   on      user
routing/prefer_free            Toggle   on      this project
providers/groq/base_url        Text     https://api.groq.com/openai/v1   user
```

One key on the selected row cycles `user ⇄ this project`. Moving a row to the
project layer is the consent moment `write_project_config_with_consent`
(`config/loading.rs:817`) is named for — its consent is the caller's to obtain,
and this is a better place to obtain it than a blanket `W`, because it names the
one setting being shared. Confirm it once per session, with the fact that
matters: *"`.glasshouse/config.toml` is a file in this repository. Anyone who
clones it gets this value."*

The three-state reality survives too: a row reading `default` has no stored
value in either file, which is information today's `w` cannot express at all.

## 5. Tips

The user asked for "tips and tricks". Three shapes were available.

**Decision: a one-line explanation under the selected row, plus a task-shaped
empty state. Not a tips section.**

*Why not a tips section.* A section is a place tips go to be not read. It is
read once, on the day it is written, by someone who does not yet have the
problem it solves; by the time they do, they are three tabs away looking at the
row that is confusing them. It also decays silently — nothing in the gate can
tell that tip #7 now describes a key that was removed — and this repository has
a standing rule against machinery whose upkeep exceeds its saving. The user's
four complaints are the test: a tips section could not have fixed any of the
first three, because a tip cannot create an entitlement, cannot add a
`GlasshouseGateway` arm to a parser, and cannot stop the backend editor
downgrading a profile. It would have been documentation of the defects.

*Why not per-row help behind a key.* Help you must ask for is help you must
know to ask for. It is the same failure as `section keys edit`.

*What to do instead — the row explains itself, always.* `settings-picker.md`'s
second mockup already draws this: under the selected row, one dim line saying
what the setting does. Make it mandatory rather than decorative — **a row
without one is an incomplete row** — and make it say the invariant, not the
label restated:

```
▸ profiles/subscription-claude-pane/backend   glasshouse gateway
    Requests go through Glasshouse on loopback, so it can route, meter and fail
    over. Nothing to configure; pick the entitlement below to say who pays.

  profiles/subscription-claude-pane/entitlement   claude-max
    Which of your accounts is charged. Only a gateway backend can select one.
```

It costs one string per row kind, it lives beside the code it describes, and it
is the only shape of the three that is in front of the user's eyes at the moment
the question arises.

*And the empty state, which is the real "getting started".* The picker's search
box with no query is the most valuable line in Settings and it is currently
blank. Put four tasks there, phrased as the user phrases them, each one
selecting the rows that accomplish it:

```
search  ▌                                                   47 settings

  I want to…
    use my Claude / ChatGPT / Gemini subscription      3 steps · 1 done
    add an API key provider                            2 steps
    make a harness use a different model                1 step
    keep a setting in this repo instead of my account   1 step
```

`3 steps · 1 done` is computable from state that already exists — the
entitlement table, `auth_present`, and the profile's backend — so it is a
progress reading, not a brochure. That is the whole "tips and tricks" feature,
and it is four strings and a filter rather than a document.

## What this does not do

- **No new configuration.** Every row proposed here already exists in
  `config/` and is reachable only by hand-editing TOML. This adds no field, no
  file, and no schema version.
- **No gateway settings.** Port, token, bind address and lifetime stay
  automatic and stay out of the UI. Exposing them would be inventing a
  capability to make a screen look complete.
- **No OAuth in the TUI.** `adopt-binary` and `login` stay CLI; Settings names
  them at the moment they are needed and never runs them.
- **No token validity check that reads a token.** The `check` Action makes one
  real request; it never opens the broker's auth directory.
- **No tips section, no help overlay, no slash palette.** `shell-chrome.md`
  already defers the `?` question; nothing here reopens it.

## Order of work

1. **The four picker-independent fixes, as one package.** Backend as a `Choice`
   including the gateway; the entitlement shown on the profile row; discovery
   deleted from the save path and the existing guard widened to cover it;
   `present` never rendered as `valid`. This is where the user's complaint
   actually lives, and the first and third are defects — one destroys
   hand-written configuration, the other is the "ages", of which only the
   opening half has been fixed.
2. `settings-picker.md`, unchanged.
3. Entitlement and subscription rows, the per-row layer, commit-on-Enter, the
   per-row explanation and the empty state — all of which are row definitions
   on top of (2), not new machinery.
