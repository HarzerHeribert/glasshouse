#!/usr/bin/env python3
"""Bake the model-measurement snapshot the gateway ships.

Usage: scripts/bake-model-index.py [CATALOGUE.json|-]   (default: runs `glasshouse analysis`)

Reads an Artificial Analysis catalogue in the shape `glasshouse analysis`
prints -- {"fetched_at", "index_version", "models": {slug: facts}} -- and
rewrites crates/inference-gateway/data/model-index.json with the fields the
subagent roster and future cost routing need. Run it when a release is cut so
the published figures ship with the binary; a user's own key overlays this
copy at run time (`inference-gateway models --import`).
"""

import json
import subprocess
import sys
from datetime import date, timezone, datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "crates" / "inference-gateway" / "data" / "model-index.json"

# What the roster reads, plus the cost figures routing will read next. An
# absent figure stays absent: the published set is genuinely partial.
KEEP = (
    "name",
    "intelligence",
    "coding",
    "agentic",
    "cost_per_task_usd",
    "input_usd_per_million",
    "output_usd_per_million",
)


def source_bytes(argv):
    if not argv:
        return subprocess.run(
            ["glasshouse", "analysis"], check=True, capture_output=True
        ).stdout
    if argv[0] == "-":
        return sys.stdin.buffer.read()
    return Path(argv[0]).read_bytes()


def main(argv):
    catalogue = json.loads(source_bytes(argv))
    models = {}
    for slug, facts in sorted(catalogue.get("models", {}).items()):
        kept = {key: facts[key] for key in KEEP if facts.get(key) is not None}
        # A row with no figure at all measures nothing and only costs bytes.
        if len(kept) > 1 or (kept and "name" not in kept):
            models[slug] = kept
    captured = catalogue.get("fetched_at")
    document = {
        "source": "Artificial Analysis (artificialanalysis.ai)",
        "index_version": catalogue.get("index_version"),
        "captured": (
            datetime.fromtimestamp(captured, timezone.utc).date().isoformat()
            if isinstance(captured, (int, float))
            else date.today().isoformat()
        ),
        "models": models,
    }
    # One line per model: compact enough to ship, granular enough to diff.
    rows = ",\n".join(
        f"  {json.dumps(slug)}: {json.dumps(facts, sort_keys=True, separators=(',', ':'))}"
        for slug, facts in models.items()
    )
    head = {key: document[key] for key in ("source", "index_version", "captured")}
    body = ",\n".join(f" {json.dumps(k)}: {json.dumps(v)}" for k, v in head.items())
    OUT.write_text("{\n" + body + ',\n "models": {\n' + rows + "\n }\n}\n")
    print(f"{OUT.relative_to(ROOT)}: {len(models)} models")


if __name__ == "__main__":
    main(sys.argv[1:])
