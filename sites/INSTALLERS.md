# Installers and updates

The website's static asset source is `sites/public/`. Vite copies its contents
to the public URL root; the directory name `public` is not part of the URL.

| Source | Pages URL now | Custom-domain URL later | Status |
| --- | --- | --- | --- |
| `sites/public/install.sh` | `/glasshouse/install.sh` | `/install.sh` | shipped (macOS arm64, Linux x86_64/arm64) |
| `sites/public/install.ps1` | `/glasshouse/install.ps1` | `/install.ps1` | not yet |

`curl -fsSL https://harzerheribert.github.io/glasshouse/install.sh | sh`
installs the newest release (pre-releases included) or `$GLASSHOUSE_VERSION`;
`sh -s -- --pane-only` leaves `glasshouse` unlinked. Each run verifies the
release archive against the release's `SHA256SUMS`, unpacks into a fresh
`~/.local/lib/glasshouse/versions/<tag>`, repoints `current`, links the
binaries into `~/.local/bin`, and adopts the subscription broker the release
pins in `release/cliproxyapi.toml` after checking its SHA-256. It installs
no harness, touches no credential and edits no shell profile.

**Updates.** A release install of Pane checks GitHub at most once a day from
an interactive session (`PANE_DISABLE_AUTOUPDATE` turns it off), installs a
newer release the same way beside the running one, and says *Pane <tag>
installed · restart to update*. `pane update [--check]` does it on demand.
A build-tree binary or a developer's version directory never updates itself.

**Broker releases make a release.** `.github/workflows/broker-bump.yml` runs
daily: a new CLIProxyAPI release is pinned with upstream's own checksums
(`scripts/release/bump-broker.py`), committed to `main`, tagged with the next
pre-release (`scripts/release/next-tag.py`) and built by `release.yml`.
