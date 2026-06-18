#!/usr/bin/env python3
# fauxx-desktop: Fauxx Desktop Companion
# Copyright (C) 2026 Digital Grease
#
# This program is free software: you can redistribute it and/or modify it
# under the terms of the GNU Affero General Public License (see LICENSE).

"""Vendor the category co-occurrence prior from the Fauxx Android app (the single
source of truth) into the desktop coherence linter (C5 #25).

The phone's ``ad_category_cooccurrence.json`` is a HAND-CURATED semantic-affinity
prior over the frozen 32-category ``CategoryPool`` taxonomy: an unordered list of
AFFILIATED pairs ``{a, b, w}`` (w in [0,1]; higher = the two interests cluster
together more). Its own provenance note records that NO public source publishes
measured co-occurrence over this taxonomy, so this prior is the best-available
data and is shared with the phone rather than re-invented on the desktop.

The desktop coherence linter uses it INVERSELY: a persona's interest pair that is
NOT affiliated in this prior (affinity below ``min_affinity``) is an UNCOMMON
combination the population sampler would rarely produce, so the linter surfaces a
non-fatal Warning for the author to notice. This script copies the affinities
verbatim (single source of truth) and stamps the desktop-side threshold + a
provenance line.

Usage (run from the repo root, with the Fauxx Android checkout at ../fauxx):
    python3 scripts/vendor-cooccurrence.py
"""

import json
import pathlib
import sys

REPO = pathlib.Path(__file__).resolve().parent.parent
ANDROID = REPO.parent / "fauxx" / "app" / "src" / "main" / "assets" / "ad_category_cooccurrence.json"
DEST = REPO / "crates" / "fauxx-core" / "src" / "studio" / "category_cooccurrence.json"

# A persona-interest pair with affinity at or above this is "affiliated" (an
# expected combination); below it (including absent = 0) is uncommon and warned.
# Set just under the weakest real affinity the phone ships, so any listed pair is
# treated as affiliated and only unaffiliated pairs warn.
MIN_AFFINITY = 0.30


def main() -> int:
    if not ANDROID.exists():
        print(f"error: Android source not found at {ANDROID}", file=sys.stderr)
        print("clone the Fauxx Android app at ../fauxx and retry", file=sys.stderr)
        return 1

    src = json.loads(ANDROID.read_text())
    affinities = src.get("affinities", [])
    if not affinities:
        print("error: no affinities in the Android source", file=sys.stderr)
        return 1

    # Copy verbatim (single source of truth), sorted for a stable diff.
    pairs = sorted(
        ({"a": e["a"], "b": e["b"], "w": e["w"]} for e in affinities),
        key=lambda e: (e["a"], e["b"]),
    )
    weakest = min(e["w"] for e in pairs)
    if MIN_AFFINITY >= weakest:
        print(
            f"error: MIN_AFFINITY {MIN_AFFINITY} >= weakest shipped affinity {weakest}; "
            "it would warn on a genuinely-affiliated pair",
            file=sys.stderr,
        )
        return 1

    out = {
        "_comment": (
            "VENDORED from the Fauxx Android app's ad_category_cooccurrence.json "
            "(single source of truth; re-sync with scripts/vendor-cooccurrence.py). "
            "A hand-curated semantic-affinity prior over the 32-category CategoryPool "
            "taxonomy (no measured co-occurrence data is published for it). Each entry "
            "is an UNORDERED pair of CategoryPool names plus an affinity `w` in [0,1] "
            "(higher = the interests cluster together). The coherence linter (C5 #25) "
            "warns when a persona's interest pair has affinity BELOW `min_affinity` "
            "(absent = 0): an uncommon combination the population sampler would rarely "
            "produce. Advisory, non-fatal."
        ),
        "min_affinity": MIN_AFFINITY,
        "affinities": pairs,
    }
    DEST.write_text(json.dumps(out, indent=2) + "\n")
    print(f"wrote {len(pairs)} affinity pairs to {DEST.relative_to(REPO)} (min_affinity={MIN_AFFINITY})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
