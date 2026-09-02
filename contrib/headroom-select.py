#!/usr/bin/env python3
"""headroom-select — Glasshouse's local-reducer contract over Headroom.

Phase 58, map lines 2028-2030 (design-decisions.md's *The local reducer
seat*). This is an example `[context_firewall.local_reducers.<name>].command`
target: `pip install "headroom-ai[all]"`, then point `command` at this file's
path. Glasshouse itself never imports Headroom and ships no dependency on
it — this shim is the one place the two meet, and a user's own install of
Headroom is what makes it work, not anything Glasshouse's Cargo.toml names.

Speaks Glasshouse's own local-reducer contract on stdin/stdout — never
Headroom's own wire shape:

    stdin:  {"version": 1, "tool": <str>, "query": <str|null>,
             "candidates": [{"id": <int>, "text": <str>}, ...]}
    stdout: {"version": 1, "tool_version": <str>,
             "verdicts": [{"id": <int>,
                           "relevance": "relevant"|"uncertain"|"discard",
                           "reason": <str>}, ...]}

Headroom's compressors return compressed TEXT, not verdicts, and Glasshouse's
semantic stage is a selector by contract (map lines 1985, 1999): it rebuilds
the forwarded result from the ORIGINAL candidates by id and never forwards a
reducer's own generated text. So this shim never answers with compressed
text — it joins the candidates in id order, asks Headroom to compress that
joined text with the transform its own content router picks for `tool`'s
shape (its log, search and diff compressors; never the prose model, which
rewrites rather than selects), and turns the transform's own output back
into one verdict per candidate id: a candidate whose text survives verbatim
is "relevant", one entirely absent is "discard", and one that survives only
in rewritten form is "uncertain". Glasshouse's own `decide_keep_set` then
applies its bias to inclusion over these verdicts exactly as it does for a
model-backed reducer's.

The exact Headroom entry points below (`select_transform`, `compress`,
`headroom.__version__`) are named from the comparison in design-decisions.md
and should be checked against the installed Headroom version's own public
API before use — this file is a reference implementation of the contract,
not a tested integration (Headroom is not installed in this build's own
test environment).
"""

import json
import sys

import headroom
from headroom import compress
from headroom.routing import select_transform


def build_reply(request: dict) -> dict:
    candidates = request["candidates"]
    tool = request["tool"]

    joined = "\n".join(candidate["text"] for candidate in candidates)
    transform = select_transform(tool)
    compressed = compress(joined, transform=transform)

    verdicts = []
    for candidate in candidates:
        text = candidate["text"]
        if text in compressed:
            relevance, reason = "relevant", "verbatim in the compressed transform"
        elif any(
            line in compressed for line in text.splitlines() if line.strip()
        ):
            relevance, reason = "uncertain", "rewritten by the compressed transform"
        else:
            relevance, reason = "discard", "absent from the compressed transform"
        verdicts.append(
            {"id": candidate["id"], "relevance": relevance, "reason": reason}
        )

    return {
        "version": 1,
        "tool_version": headroom.__version__,
        "verdicts": verdicts,
    }


def main() -> int:
    request = json.loads(sys.stdin.read())
    reply = build_reply(request)
    print(json.dumps(reply))
    return 0


if __name__ == "__main__":
    sys.exit(main())
