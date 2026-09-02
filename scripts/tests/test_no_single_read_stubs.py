#!/usr/bin/env python3
"""A tripwire against the single-`read` stub shape that flaked on Windows.

WHY THIS EXISTS
----------------
Five test files carried the same stub HTTP server: read one `read(&mut buf)`
into a 4 KiB buffer, answer, drop the stream. The gateway writes its relayed
request as a head then a streamed body (`gateway::ingress`'s
`SendBody::from_owned_reader`); when the stub's single read lands between
those two writes, unread bytes are still queued when it closes. Closing a
socket over unread received bytes is an *abortive* close -- the stack sends
RST instead of FIN -- so Winsock discards the response the stub had just
written and the gateway's own read of it fails, taking `ingress::serve`'s
`Outcome::Unreachable` arm and answering its own `502` instead of relaying
the scripted status. Unix hands buffered bytes back before reporting the
reset, which is why the identical stub was reliable there and only flaked on
the Windows ARM64 CI VM. `.agent-runtime/report-windows-flakes.md` section 3
has the full account and the census that found all five.

The fix is `read_whole_request`: read the head to the blank line, parse
`Content-Length`, `read_exact` the body. The census (and this check) matches
the literal shape `read(&mut buf)` -- the buffer every stub of this family
named `buf` -- rather than a general `.read(` pattern, because the suite
also has legitimate single `.read(&mut buffer)` calls on the *client* side
of a streaming response (`gateway_translate*.rs`), read in a polling loop
against a deadline rather than once-and-close; those do not share the defect
and must not be flagged.
"""
from __future__ import annotations

import sys
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
TESTS_DIR = REPO_ROOT / "crates" / "glasshouse" / "tests"

SINGLE_READ = "read(&mut buf)"
HELPER_CALL = "read_whole_request"

REASON = (
    "a stub that reads a fixed buffer once and answers may still hold unread "
    "request bytes when it closes; that is an abortive close, which Winsock "
    "reports as a reset and Unix does not, so the gateway's read of the "
    "already-written response fails and it answers its own 502 -- see "
    "report-windows-flakes.md section 3"
)


def offending_lines(text: str) -> list[tuple[int, str]]:
    """Lines carrying the single-read shape and not also calling the fix."""
    return [
        (lineno, line)
        for lineno, line in enumerate(text.splitlines(), start=1)
        if SINGLE_READ in line and HELPER_CALL not in line
    ]


def scan(tests_dir: Path) -> dict[Path, list[tuple[int, str]]]:
    findings: dict[Path, list[tuple[int, str]]] = {}
    for path in sorted(tests_dir.glob("*.rs")):
        hits = offending_lines(path.read_text(encoding="utf-8"))
        if hits:
            findings[path] = hits
    return findings


class SingleReadShapeTests(unittest.TestCase):
    def test_the_single_read_shape_is_flagged(self):
        hits = offending_lines(
            "let mut buf = [0u8; 4096];\n"
            "let _ = stream.read(&mut buf);\n"
            "let _ = stream.write_all(&scripted);\n"
        )
        self.assertEqual([lineno for lineno, _ in hits], [2])

    def test_a_call_to_the_helper_on_the_same_line_is_not_flagged(self):
        # Not a real shape any caller writes, but the packet's own escape
        # clause ("outside a line that also calls read_whole_request") is
        # asserted directly so it stays true if the helper's signature ever
        # changes to something that could share a line with this pattern.
        hits = offending_lines("read_whole_request(stream); // read(&mut buf) note\n")
        self.assertEqual(hits, [])

    def test_the_helper_bodys_own_read_exact_is_not_flagged(self):
        hits = offending_lines(
            "fn read_whole_request(stream: &mut TcpStream) {\n"
            "    let mut reader = BufReader::new(stream);\n"
            "    let mut body = vec![0u8; declared];\n"
            "    let _ = reader.read_exact(&mut body);\n"
            "}\n"
        )
        self.assertEqual(hits, [])

    def test_a_differently_named_buffer_is_not_flagged(self):
        # `client.read(&mut buffer)` -- the streaming-response read in
        # gateway_translate*.rs -- is a different shape (a polling loop
        # against a deadline, not a once-and-close stub) and must not
        # false-positive just because it also starts with `.read(&mut`.
        hits = offending_lines(
            'let read = client.read(&mut buffer).expect("the stream is open");\n'
        )
        self.assertEqual(hits, [])

    def test_reverting_the_fix_reintroduces_the_finding(self):
        fixed = (
            "read_whole_request(stream);\n"
            "\n"
            "let body = b\"ok\";\n"
        )
        reverted = (
            "let mut buf = [0u8; 4096];\n"
            "let _ = stream.read(&mut buf);\n"
            "\n"
            "let body = b\"ok\";\n"
        )
        self.assertEqual(offending_lines(fixed), [])
        self.assertNotEqual(offending_lines(reverted), [])

    def test_the_real_tree_this_packages_own_files_are_clean(self):
        """`routing_cost.rs` and `gateway_retry_after.rs` are this package's
        expected files and must carry no single-read stub. Sibling files
        (`v1_criteria_routing.rs`, `gateway_failure_taxonomy.rs`,
        `evaluation_producers.rs`) are `GH-WINDOWS-FLAKES`'s -- FORBIDDEN
        FILES here, never edited by this package -- so they are reported by
        `main()` if still unfixed, not asserted clean by this test."""
        findings = scan(TESTS_DIR)
        this_package = {
            TESTS_DIR / "routing_cost.rs",
            TESTS_DIR / "gateway_retry_after.rs",
        }
        flagged = set(findings) & this_package
        self.assertEqual(
            flagged,
            set(),
            f"GH-WINDOWS-STUB-DRAIN's own files still carry a single-read "
            f"stub: {sorted(p.name for p in flagged)}",
        )


def main() -> int:
    suite = unittest.TestLoader().loadTestsFromTestCase(SingleReadShapeTests)
    result = unittest.TextTestRunner(verbosity=1).run(suite)
    if not result.wasSuccessful():
        return 1

    findings = scan(TESTS_DIR)
    if findings:
        print(
            "\ntest_no_single_read_stubs: single-read stub(s) found in the "
            "tree (not necessarily this package's own -- see below):",
            file=sys.stderr,
        )
        for path, hits in findings.items():
            rel = path.relative_to(REPO_ROOT)
            for lineno, line in hits:
                print(f"{rel}:{lineno}: {REASON}", file=sys.stderr)
                print(f"    {line.strip()}", file=sys.stderr)
        this_package = {
            TESTS_DIR / "routing_cost.rs",
            TESTS_DIR / "gateway_retry_after.rs",
        }
        if set(findings) - this_package:
            print(
                "\nThe file(s) above outside routing_cost.rs and "
                "gateway_retry_after.rs are GH-WINDOWS-FLAKES's known, "
                "named exposure (report-windows-flakes.md section 8) -- "
                "not a defect in this check -- and resolve when that "
                "package merges.",
                file=sys.stderr,
            )
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
