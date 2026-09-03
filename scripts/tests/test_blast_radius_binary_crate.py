"""Tests for scripts/blast-radius.sh's binary-crate module mapping.

WHY THIS FILE EXISTS
---------------------
2026-09-03: `GH-DECOMP-MAIN` moved 280 items out of `main.rs` into 21 files
under `src/commands/`, and `src/api/` already held seven more. Those modules
are declared by `main.rs`, not `lib.rs`, so they compile into the BINARY
crate -- and `cargo test --lib commands::hook` selects **zero** tests there.
A filter matching nothing is indistinguishable from a pass (practice §68), so
every change under `commands/` and `api/` got no coverage at all while the
gate printed green. `GH-CLAIMS-AF` hit it, noticed, and ran `--bin glasshouse`
by hand.

A src file under a top-level module that `main.rs` declares and `lib.rs` does
not must map to `--bin <pkg>`, in both the full-trace and the `--targeted`
mapping. A module declared in both belongs to the lib and keeps its `--lib`
filter, because the lib copy is the one `--lib` compiles.
"""
from __future__ import annotations

import os
import subprocess
import tempfile
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
SCRIPT = REPO_ROOT / "scripts" / "blast-radius.sh"


class BinaryCrateModuleTests(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        (self.tmp / "scripts").mkdir()
        (self.tmp / "scripts" / "blast-radius.sh").write_bytes(SCRIPT.read_bytes())
        os.chmod(self.tmp / "scripts" / "blast-radius.sh", 0o755)
        self.git("init", "-q")
        self.git("config", "user.email", "t@example.com")
        self.git("config", "user.name", "t")

    def git(self, *args):
        return subprocess.run(["git", *args], cwd=self.tmp,
                              capture_output=True, text=True)

    def commit_rs(self, name: str, body: str) -> None:
        p = self.tmp / name
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(body)
        self.git("add", "-A")
        self.git("commit", "-q", "-m", f"add {name}")

    def dry_run_since(self, since: str) -> str:
        r = subprocess.run(
            ["bash", str(self.tmp / "scripts" / "blast-radius.sh"),
             "--dry-run", "--list", "--since", since],
            cwd=self.tmp, capture_output=True, text=True,
        )
        return r.stdout + r.stderr

    @staticmethod
    def targeted_preview(out: str) -> str:
        marker = "--targeted preview"
        idx = out.index(marker)
        return out[idx:]

    def seed_crate(self) -> None:
        """A crate shaped like this one: a lib root and a bin root that each
        declare their own top-level modules."""
        self.commit_rs("crates/glasshouse/src/lib.rs",
                       "pub mod gateway;\npub mod session;\n")
        self.commit_rs("crates/glasshouse/src/main.rs",
                       "mod api;\nmod commands;\nfn main() {}\n")

    def test_a_binary_crate_module_maps_to_the_bin_not_a_dead_lib_filter(self):
        self.seed_crate()
        self.commit_rs("crates/glasshouse/src/commands/hook.rs",
                       "pub fn report() {}\n")
        out = self.dry_run_since("HEAD~1")
        targeted = self.targeted_preview(out)
        self.assertIn("--bin glasshouse", targeted)
        # The dead filters must be gone: neither the child's own path nor the
        # bare parent, because `--lib` compiles neither of them.
        self.assertNotIn("commands::hook", targeted)
        self.assertNotRegex(targeted, r"--lib.*(?<![:\w])commands(?![:\w])")

    def test_the_full_trace_also_routes_it_to_the_bin(self):
        self.seed_crate()
        self.commit_rs("crates/glasshouse/src/api/unix.rs",
                       "pub fn serve() {}\n")
        out = self.dry_run_since("HEAD~1")
        self.assertIn("glasshouse", out)
        self.assertNotIn("api::unix", out)

    def test_a_lib_module_keeps_its_lib_filter(self):
        self.seed_crate()
        self.commit_rs("crates/glasshouse/src/gateway/session.rs",
                       "pub fn a() {}\n")
        out = self.dry_run_since("HEAD~1")
        targeted = self.targeted_preview(out)
        self.assertIn("gateway::session", targeted)
        self.assertNotIn("--bin", targeted)

    def test_a_module_declared_in_both_roots_stays_with_the_lib(self):
        """`main.rs` may `mod` a name the lib also owns. The lib copy is what
        `--lib` compiles and runs, so the filter is still the right target."""
        self.commit_rs("crates/glasshouse/src/lib.rs", "pub mod shared;\n")
        self.commit_rs("crates/glasshouse/src/main.rs",
                       "mod shared;\nfn main() {}\n")
        self.commit_rs("crates/glasshouse/src/shared/thing.rs",
                       "pub fn a() {}\n")
        out = self.dry_run_since("HEAD~1")
        targeted = self.targeted_preview(out)
        self.assertIn("shared::thing", targeted)
        self.assertNotIn("--bin", targeted)

    def test_a_top_level_src_file_is_never_treated_as_binary_crate_code(self):
        """`binary_crate_pkg` only fires for a file *under* a module directory:
        `src/foo.rs` has no parent module to look up, and a crate with no
        `main.rs` at all must be unaffected."""
        self.commit_rs("crates/glasshouse/src/lib.rs", "pub mod foo;\n")
        self.commit_rs("crates/glasshouse/src/foo.rs", "pub fn a() {}\n")
        out = self.dry_run_since("HEAD~1")
        targeted = self.targeted_preview(out)
        self.assertIn("foo", targeted)
        self.assertNotIn("--bin", targeted)


if __name__ == "__main__":
    unittest.main()
