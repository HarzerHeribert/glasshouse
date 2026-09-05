# Future installer delivery

The website's static asset source is `sites/public/`. Vite copies its contents
to the public URL root; the directory name `public` is not part of the URL.

Planned paths:

| Source | Pages URL now | Custom-domain URL later |
| --- | --- | --- |
| `sites/public/install.sh` | `/glasshouse/install.sh` | `/install.sh` |
| `sites/public/install-pane.sh` | `/glasshouse/install-pane.sh` | `/install-pane.sh` |
| `sites/public/install.ps1` | `/glasshouse/install.ps1` | `/install.ps1` |
| `sites/public/install-pane.ps1` | `/glasshouse/install-pane.ps1` | `/install-pane.ps1` |

These are reserved plans, not shipped installers. Do not publish an install
command until matching release artifacts and verification exist.

The full suite should install Glasshouse and Pane. The Pane-only route should
install just Pane. Before implementation, establish release artifact names,
supported OS/architectures, checksums/signature verification, install directory,
version pinning, upgrade/rollback behavior, and whether shell scripts need
PowerShell counterparts. Existing external coding harnesses and credentials
must not be silently installed or modified. The site can link to verified
release assets without storing binaries in the site repository.
