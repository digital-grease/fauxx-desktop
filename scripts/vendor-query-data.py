#!/usr/bin/env python3
"""Vendor the query-generation data from the Fauxx Android app into fauxx-core.

The desktop's search-decoy query generation (C6 H1) reuses the SAME query banks
and the SAME harmful-query safety blocklist as the Android app, so the two never
diverge into emitting different (or differently-unsafe) traffic. Rather than
hand-copy and risk drift, this script is the single source of truth: it pulls the
EN corpus from a local Fauxx Android checkout and writes the two vendored files
fauxx-core embeds at compile time. A test (`querybank::tests`) re-derives the
hash and fails if the committed files drift from what this script would produce,
so a stale vendor is caught in CI.

US-only by design (the desktop personas are US/ACS-PUMS), so only the English
(legacy top-level) banks are vendored, not the es/fr/ru locale subdirectories.

Usage:
    python3 scripts/vendor-query-data.py [path-to-fauxx-android-checkout]

Defaults to ../fauxx. Re-run after the Android corpus changes, then commit the
two updated files under crates/fauxx-core/src/querybank/.
"""
import json
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
DEST = REPO / "crates" / "fauxx-core" / "src" / "querybank"


def main() -> int:
    android = Path(sys.argv[1]) if len(sys.argv) > 1 else REPO.parent / "fauxx"
    assets = android / "app" / "src" / "main" / "assets"
    banks_dir = assets / "query_banks"
    harmful_src = assets / "harmful_queries.json"

    if not banks_dir.is_dir() or not harmful_src.is_file():
        print(f"error: Fauxx Android assets not found under {assets}", file=sys.stderr)
        print("       pass the path to a Fauxx Android checkout as arg 1.", file=sys.stderr)
        return 1

    # Combine the EN (legacy top-level) per-category bank files into one object
    # keyed by the SCREAMING_SNAKE CategoryPool name (the file stem, uppercased),
    # so fauxx-core can embed a single JSON. Skip the es/fr/ru subdirectories.
    banks: dict[str, list[str]] = {}
    for f in sorted(banks_dir.glob("*.json")):
        category = f.stem.upper()
        queries = json.loads(f.read_text(encoding="utf-8"))
        if not isinstance(queries, list):
            print(f"error: {f} is not a JSON array of strings", file=sys.stderr)
            return 1
        banks[category] = queries

    DEST.mkdir(parents=True, exist_ok=True)
    # Sorted keys + stable formatting so the output is deterministic (the drift
    # test compares the committed bytes against a fresh run).
    banks_out = DEST / "query_banks_en.json"
    banks_out.write_text(
        json.dumps(banks, ensure_ascii=False, indent=1, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    # The harmful-query blocklist is copied verbatim (it is the safety corpus;
    # its shape is class_a_terms / self_signal_terms / regex_patterns).
    harmful_out = DEST / "harmful_queries.json"
    harmful_out.write_text(harmful_src.read_text(encoding="utf-8"), encoding="utf-8")

    total = sum(len(v) for v in banks.values())
    print(f"vendored {len(banks)} category banks ({total} queries) -> {banks_out}")
    print(f"vendored harmful-query blocklist -> {harmful_out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
