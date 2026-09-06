#!/usr/bin/env python3
"""Refuse a change that carries a credential -- a local key-file value, or a
string shaped like a known provider or API key with a high-entropy tail.

Two sources of truth, checked in this order for every candidate line:

1. Local key values, read from .agent-runtime/provider-keys.env (and any
   *.env directly under .agent-runtime/) if it exists. Any value 12+
   characters found verbatim in a line is a `local-key NAME` finding. The
   value itself is never printed, logged, or written anywhere -- only the
   name of the environment variable it came from. Never allowlisted, never
   waived by the marker.

2. Shape rules: known provider key prefixes, a private-key block header, and
   a generic api_key/secret/token/password assignment. A shape match is a
   finding unless its tail clears an entropy bar (ENTROPY_BAR) and has no
   long sequential run (SEQUENTIAL_RUN_BAR) -- see those constants for the
   values and why -- OR the exact line is listed in the fingerprint
   allowlist (scripts/check-secrets-allow.txt, see ALLOWLIST_FILE).

A trailing `glasshouse:not-a-secret` marker (any comment syntax) on the same
line, or a fingerprint entry for that exact line, allows every rule except
local-key -- a real value is never allowable either way. `--fingerprint
<path>:<lineno>` prints the entry to paste into the allowlist for a new
fixture line.

No network, no dependency beyond the Python 3 standard library and git.
"""
from __future__ import annotations

import argparse
import hashlib
import math
import os
import re
import subprocess
import sys
from collections import Counter
from typing import Iterable, Iterator

MARKER = "glasshouse:not-a-secret"

# Bits of Shannon entropy per character a matched tail must clear to count as
# a finding. Low on purpose: its only remaining job is to pass ordinary
# low-entropy strings that happen to carry a recognized prefix or keyword --
# "sk-example", `token = "placeholder"` -- not to distinguish a real secret
# from a planted fixture. That distinction now belongs to the fingerprint
# allowlist (ALLOWLIST_FILE), which is exact rather than statistical: a real
# 36-character random token is flagged essentially always at this bar (a
# spot-check of 100 random ghp_-prefixed 36-char tokens and 100 random
# gsk_-prefixed 52-char tokens: 100/100 both), where a bar chosen only to
# admit fixtures by shape (this repo's highest-entropy planted fixture
# measured 4.725 bits/char) would have let a real same-length token pass
# undetected on a coin flip. See design-decisions for the earlier attempt.
ENTROPY_BAR = 3.5

TAIL_RE = re.compile(r"[A-Za-z0-9_\-]+")

# A prefix or keyword must not be immediately preceded by a letter, digit or
# underscore -- otherwise "sk-" (OpenAI) matches inside ordinary English
# hyphenated words ("task-based", "disk-backed", "whisk-"), and at the low
# entropy bar below (3.5 bits/char, chosen to catch a placeholder like
# "sk-example" that a higher bar would pass) an ordinary tail of English text
# clears it often enough to flood the tree with false positives. This one
# lookbehind is what makes a low bar usable at all.
NOT_WORD_BEFORE = r"(?<![A-Za-z0-9_])"

# (rule name, prefix pattern). Order matters only for readability -- every
# rule is tried against every line independently.
PREFIX_RULES = [
    ("groq", re.compile(NOT_WORD_BEFORE + r"gsk_")),
    ("anthropic", re.compile(NOT_WORD_BEFORE + r"sk-ant-api03-")),
    ("openai", re.compile(NOT_WORD_BEFORE + r"sk-proj-")),
    ("openai", re.compile(NOT_WORD_BEFORE + r"sk-")),
    ("google", re.compile(NOT_WORD_BEFORE + r"AIza")),
    ("huggingface", re.compile(NOT_WORD_BEFORE + r"hf_")),
    ("github", re.compile(NOT_WORD_BEFORE + r"gh[pousr]_|" + NOT_WORD_BEFORE + r"github_pat_")),
    ("gitlab", re.compile(NOT_WORD_BEFORE + r"glpat-")),
    ("slack", re.compile(NOT_WORD_BEFORE + r"xox[abp]-")),
    ("aws", re.compile(NOT_WORD_BEFORE + r"AKIA")),
]

PRIVATE_KEY_RE = re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----")

GENERIC_RE = re.compile(
    NOT_WORD_BEFORE + r"""(?i:(api[_-]?key|secret|token|password))\s*[:=]\s*["']?([A-Za-z0-9_\-]{12,})"""
)

DEFAULT_KEY_FILE = ".agent-runtime/provider-keys.env"

# Repo-relative-path-keyed allowlist of known fixture lines, resolved next to
# this script rather than relative to the caller's cwd, so it is found the
# same way whether invoked from a hook (cwd = the committing worktree root)
# or a test (cwd = wherever unittest runs from).
SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
ALLOWLIST_FILE = os.path.join(SCRIPT_DIR, "check-secrets-allow.txt")


def shannon_entropy(s: str) -> float:
    if not s:
        return 0.0
    counts = Counter(s)
    n = len(s)
    return -sum((c / n) * math.log2(c / n) for c in counts.values())


# Minimum length of a monotonic run (case-folded, each character exactly one
# code point after or before the last -- "abcdefg" or "9876") above which a
# tail is treated as a sequential pattern rather than randomness, regardless
# of its raw character entropy. Plain per-character Shannon entropy scores a
# string like "abcdefghijklmnopqrstuvwxyz0123" as high as true randomness --
# every character is distinct -- so this catches what entropy alone misses.
# A run this long in a genuinely random key is vanishingly unlikely.
SEQUENTIAL_RUN_BAR = 6


def max_sequential_run(s: str) -> int:
    folded = s.lower()
    if len(folded) < 2:
        return len(folded)
    best = 1
    run = 1
    for i in range(1, len(folded)):
        delta = ord(folded[i]) - ord(folded[i - 1])
        if delta in (1, -1):
            run += 1
        else:
            run = 1
        best = max(best, run)
    return best


def looks_random(s: str) -> bool:
    return shannon_entropy(s) > ENTROPY_BAR and max_sequential_run(s) < SEQUENTIAL_RUN_BAR


def line_has_marker(line: str) -> bool:
    return MARKER in line


def line_fingerprint(line: str) -> str:
    """sha256 of the exact line bytes, without the newline."""
    return hashlib.sha256(line.encode("utf-8")).hexdigest()


def load_allowlist(path: str) -> set[tuple[str, str]]:
    """Read `<repo-relative path>:<sha256 of the line>` entries. A missing
    file is fine -- it just means nothing is allowlisted yet."""
    entries: set[tuple[str, str]] = set()
    if not path or not os.path.isfile(path):
        return entries
    with open(path, "r", encoding="utf-8", errors="ignore") as fh:
        for raw in fh:
            line = raw.strip()
            if not line or line.startswith("#"):
                continue
            file_path, sep, fingerprint = line.rpartition(":")
            if sep and file_path and fingerprint:
                entries.add((file_path, fingerprint))
    return entries


def is_allowlisted(path: str, line: str, allowlist: set[tuple[str, str]]) -> bool:
    return (path, line_fingerprint(line)) in allowlist


def load_local_keys(key_file: str) -> dict[str, str]:
    """Read NAME=value pairs (12+ char values) from key_file and any *.env
    file directly under its parent directory. Returns {} if key_file does
    not exist -- CI has none, and that is fine."""
    values: dict[str, str] = {}
    if not key_file:
        return values
    paths = []
    if os.path.isfile(key_file):
        paths.append(key_file)
        parent = os.path.dirname(key_file) or "."
        if os.path.isdir(parent):
            for name in sorted(os.listdir(parent)):
                if name.endswith(".env"):
                    candidate = os.path.join(parent, name)
                    if os.path.isfile(candidate) and candidate != key_file:
                        paths.append(candidate)
    for path in paths:
        try:
            with open(path, "r", encoding="utf-8", errors="ignore") as fh:
                for raw in fh:
                    line = raw.strip()
                    if not line or line.startswith("#") or "=" not in line:
                        continue
                    name, _, value = line.partition("=")
                    name = name.strip()
                    value = value.strip().strip('"').strip("'")
                    if name and len(value) >= 12:
                        values[name] = value
        except OSError:
            continue
    return values


def is_binary(data: bytes) -> bool:
    return b"\x00" in data[:8192]


def scan_line(line: str, local_keys: dict[str, str]) -> list[str]:
    """Return the list of rule names that fire on this line, before the
    marker or the allowlist is applied."""
    findings: list[str] = []

    for name, value in local_keys.items():
        if value and value in line:
            findings.append(f"local-key {name}")

    if PRIVATE_KEY_RE.search(line):
        findings.append("private-key-block")

    for rule, prefix_re in PREFIX_RULES:
        for m in prefix_re.finditer(line):
            rest = line[m.end():]
            tail_m = TAIL_RE.match(rest)
            tail = tail_m.group(0) if tail_m else ""
            if looks_random(tail):
                findings.append(rule)

    for m in GENERIC_RE.finditer(line):
        candidate = m.group(2)
        if looks_random(candidate):
            findings.append("generic-assignment")

    return findings


def evaluate_line(
    path: str,
    line: str,
    local_keys: dict[str, str],
    allowlist: set[tuple[str, str]],
) -> list[str]:
    """scan_line, then let the marker or the fingerprint allowlist waive
    every non-local-key finding. local-key findings are never waived by
    either mechanism -- a real value is never allowable."""
    findings = scan_line(line, local_keys)
    if not findings:
        return findings
    # The allowlist itself is exempt from every shape rule: an entry that
    # named the fixture it waives would otherwise be a finding in its own
    # file, and the first commit that installed this guard was refused on
    # exactly that (2026-09-06). A local-key finding is never exempt anywhere.
    if path.replace(os.sep, "/").endswith("scripts/check-secrets-allow.txt"):
        return [f for f in findings if f.startswith("local-key")]
    if line_has_marker(line) or is_allowlisted(path, line, allowlist):
        return [f for f in findings if f.startswith("local-key")]
    return findings


class Finding:
    __slots__ = ("path", "line_no", "rule")

    def __init__(self, path: str, line_no: int, rule: str):
        self.path = path
        self.line_no = line_no
        self.rule = rule

    def __str__(self) -> str:
        return f"{self.path}:{self.line_no}: {self.rule}"


def scan_text(
    path: str,
    lines_with_numbers: Iterable[tuple[int, str]],
    local_keys: dict[str, str],
    allowlist: set[tuple[str, str]],
) -> list[Finding]:
    out: list[Finding] = []
    for line_no, line in lines_with_numbers:
        for rule in evaluate_line(path, line, local_keys, allowlist):
            out.append(Finding(path, line_no, rule))
    return out


def run_git(args: list[str]) -> str:
    result = subprocess.run(
        ["git"] + args,
        capture_output=True,
        text=True,
        errors="replace",
    )
    return result.stdout


def iter_added_lines_from_diff(diff_text: str) -> Iterator[tuple[str, int, str]]:
    """Parse unified diff text (as produced by `git diff -U0`), yielding
    (path, line_no, content) for every added line. Binary files are already
    excluded by git's own diff output."""
    path = None
    new_line_no = 0
    for raw in diff_text.splitlines():
        if raw.startswith("+++ "):
            p = raw[4:]
            if p.startswith("b/"):
                p = p[2:]
            path = None if p == "/dev/null" else p
            continue
        if raw.startswith("@@"):
            m = re.match(r"@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@", raw)
            if m:
                new_line_no = int(m.group(1))
            continue
        if path is None:
            continue
        if raw.startswith("+++") or raw.startswith("---"):
            continue
        if raw.startswith("+"):
            yield (path, new_line_no, raw[1:])
            new_line_no += 1
        elif raw.startswith("-"):
            continue
        else:
            new_line_no += 1


def scan_staged(local_keys: dict[str, str], allowlist: set[tuple[str, str]]) -> list[Finding]:
    diff_text = run_git(["diff", "--cached", "-U0", "--no-color"])
    out: list[Finding] = []
    for path, line_no, content in iter_added_lines_from_diff(diff_text):
        for rule in evaluate_line(path, content, local_keys, allowlist):
            out.append(Finding(path, line_no, rule))
    return out


def scan_range(
    range_spec: str, local_keys: dict[str, str], allowlist: set[tuple[str, str]]
) -> list[Finding]:
    diff_text = run_git(["diff", range_spec, "-U0", "--no-color"])
    out: list[Finding] = []
    for path, line_no, content in iter_added_lines_from_diff(diff_text):
        for rule in evaluate_line(path, content, local_keys, allowlist):
            out.append(Finding(path, line_no, rule))
    return out


def scan_tree(
    local_keys: dict[str, str], allowlist: set[tuple[str, str]], use_worktree: bool
) -> list[Finding]:
    out: list[Finding] = []
    if use_worktree:
        files = run_git(["ls-files"]).splitlines()
        for path in files:
            if not os.path.isfile(path):
                continue
            try:
                with open(path, "rb") as fh:
                    data = fh.read()
            except OSError:
                continue
            if is_binary(data):
                continue
            text = data.decode("utf-8", errors="replace")
            lines = [(i + 1, line) for i, line in enumerate(text.splitlines())]
            out.extend(scan_text(path, lines, local_keys, allowlist))
    else:
        files = run_git(["ls-tree", "-r", "--name-only", "HEAD"]).splitlines()
        for path in files:
            data = subprocess.run(
                ["git", "show", f"HEAD:{path}"],
                capture_output=True,
            ).stdout
            if is_binary(data):
                continue
            text = data.decode("utf-8", errors="replace")
            lines = [(i + 1, line) for i, line in enumerate(text.splitlines())]
            out.extend(scan_text(path, lines, local_keys, allowlist))
    return out


def scan_files(
    paths: list[str], local_keys: dict[str, str], allowlist: set[tuple[str, str]]
) -> list[Finding]:
    out: list[Finding] = []
    for path in paths:
        if not os.path.isfile(path):
            continue
        try:
            with open(path, "rb") as fh:
                data = fh.read()
        except OSError:
            continue
        if is_binary(data):
            continue
        text = data.decode("utf-8", errors="replace")
        lines = [(i + 1, line) for i, line in enumerate(text.splitlines())]
        out.extend(scan_text(path, lines, local_keys, allowlist))
    return out


def cmd_fingerprint(spec: str) -> int:
    path, _, lineno_s = spec.rpartition(":")
    if not path or not lineno_s.isdigit():
        print(f"--fingerprint wants PATH:LINE, got {spec!r}", file=sys.stderr)
        return 2
    lineno = int(lineno_s)
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as fh:
            lines = fh.read().splitlines()
    except OSError as exc:
        print(f"--fingerprint: cannot read {path}: {exc}", file=sys.stderr)
        return 2
    if not (1 <= lineno <= len(lines)):
        print(f"--fingerprint: {path} has no line {lineno}", file=sys.stderr)
        return 2
    print(f"{path}:{line_fingerprint(lines[lineno - 1])}")
    return 0


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--staged", action="store_true", help="scan added lines in the git index against HEAD")
    mode.add_argument("--range", metavar="A..B", help="scan added lines of every commit in the range")
    mode.add_argument("--tree", action="store_true", help="scan every tracked file (HEAD, or --worktree for the working tree)")
    mode.add_argument("--files", nargs="+", metavar="PATH", help="scan exactly these files")
    mode.add_argument("--fingerprint", metavar="PATH:LINE", help="print the allowlist entry for one line, to paste into check-secrets-allow.txt")
    parser.add_argument("--worktree", action="store_true", help="with --tree, scan the working tree instead of HEAD")
    parser.add_argument("--quiet", action="store_true", help="suppress the summary line on a clean scan")
    parser.add_argument("--key-file", default=None, help=f"local key file (default: {DEFAULT_KEY_FILE} or $GLASSHOUSE_KEY_FILE)")
    parser.add_argument("--allow-file", default=None, help=f"fingerprint allowlist (default: {ALLOWLIST_FILE})")
    args = parser.parse_args(argv)

    if args.fingerprint:
        return cmd_fingerprint(args.fingerprint)

    key_file = args.key_file or os.environ.get("GLASSHOUSE_KEY_FILE") or DEFAULT_KEY_FILE
    local_keys = load_local_keys(key_file)
    allow_file = args.allow_file or ALLOWLIST_FILE
    allowlist = load_allowlist(allow_file)

    if args.staged:
        findings = scan_staged(local_keys, allowlist)
    elif args.range:
        findings = scan_range(args.range, local_keys, allowlist)
    elif args.tree:
        findings = scan_tree(local_keys, allowlist, args.worktree)
    else:
        findings = scan_files(args.files, local_keys, allowlist)

    for f in findings:
        print(str(f))

    if findings or not args.quiet:
        print(f"{len(findings)} finding(s)")

    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
