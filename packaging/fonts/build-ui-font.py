#!/usr/bin/env python3
"""Build the bundled UI font asset from its upstream source.

WHY THIS EXISTS
---------------
The desktop GUI embeds a font so the interface renders on a host with no fonts
configured (#52). Selecting that embedded face is done by FAMILY NAME, because
that is the only selector iced 0.14 offers: `iced::Font::with_name(..)`. There is
no API to say "use the face I just registered".

That makes the family name load-bearing. If the embedded face keeps its upstream
name, every host that also has that family installed wins the match: fontdb's
query returns a candidate set containing both faces and the tie-break is
insertion order, so the app renders in the HOST's copy and the bytes we shipped
and tested are dead weight. On any Linux distribution shipping the Noto fonts,
that is the normal case rather than an edge case.

Renaming the face to a project-unique family removes the ambiguity: the query
matches exactly one face, and the app renders in the file it ships. Falling back
to host fonts for scripts this face does not cover is UNAFFECTED, because
cosmic-text's candidate filter ignores family entirely (`Attrs::matches` tests
style and stretch only), so the fallback pool is still the whole font database.

LICENSING
---------
Noto Sans is under the SIL Open Font License 1.1. Its copyright statement
declares NO Reserved Font Name, so OFL clause 3's restriction on renaming does
not apply and a Modified Version may carry a new family name. The copyright
(name ID 0) and licence (name IDs 13 and 14) records are preserved verbatim, and
the derivative status is recorded in LICENSE-NotoSans.txt beside the asset.

USAGE
-----
    python3 packaging/fonts/build-ui-font.py /path/to/NotoSans-Regular.ttf

Upstream source: Noto Sans Regular 2.015 from the Noto latin-greek-cyrillic
project (https://github.com/notofonts/latin-greek-cyrillic). The expected input
digest is pinned below so a substituted or corrupted source is refused rather
than silently baked into the release.
"""

import hashlib
import pathlib
import sys

from fontTools.ttLib import TTFont

# sha256 of the pristine upstream NotoSans-Regular.ttf this asset derives from.
UPSTREAM_SHA256 = "478c558ea716033cd60c03438f628dfa75694dcf6b5f6d505a2f05fd2b4f3823"

# The project-unique family the app selects with `iced::Font::with_name`. Must
# stay in lockstep with UI_FONT_FAMILY in apps/desktop/src/font.rs.
FAMILY = "Fauxx UI"
SUBFAMILY = "Regular"

OUT = pathlib.Path("apps/desktop/assets/fonts/FauxxUI-Regular.ttf")

# Name IDs that carry the family identity and must be rewritten together. Any
# one of them left behind would let a font-matching implementation resolve the
# old family and reintroduce the shadowing this rename exists to prevent.
FAMILY_NAME_ID = 1
SUBFAMILY_NAME_ID = 2
UNIQUE_ID_NAME_ID = 3
FULL_NAME_ID = 4
POSTSCRIPT_NAME_ID = 6
TYPOGRAPHIC_FAMILY_NAME_ID = 16
TYPOGRAPHIC_SUBFAMILY_NAME_ID = 17


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    source = pathlib.Path(sys.argv[1])
    raw = source.read_bytes()
    digest = hashlib.sha256(raw).hexdigest()
    if digest != UPSTREAM_SHA256:
        print(
            f"refusing to build: {source} has sha256 {digest},\n"
            f"but this script is pinned to the upstream {UPSTREAM_SHA256}.\n"
            "Update UPSTREAM_SHA256 deliberately if you are intentionally "
            "moving to a new upstream release.",
            file=sys.stderr,
        )
        return 1

    font = TTFont(source)
    name = font["name"]
    version = name.getDebugName(5) or ""

    for record in list(name.names):
        nid = record.nameID
        if nid in (FAMILY_NAME_ID, TYPOGRAPHIC_FAMILY_NAME_ID):
            name.setName(FAMILY, nid, record.platformID, record.platEncID, record.langID)
        elif nid in (SUBFAMILY_NAME_ID, TYPOGRAPHIC_SUBFAMILY_NAME_ID):
            name.setName(SUBFAMILY, nid, record.platformID, record.platEncID, record.langID)
        elif nid == FULL_NAME_ID:
            name.setName(f"{FAMILY} {SUBFAMILY}", nid, record.platformID, record.platEncID, record.langID)
        elif nid == POSTSCRIPT_NAME_ID:
            name.setName("FauxxUI-Regular", nid, record.platformID, record.platEncID, record.langID)
        elif nid == UNIQUE_ID_NAME_ID:
            # Must be globally unique; reusing the upstream string would collide
            # with the unmodified face in any font cache keyed on it.
            name.setName(
                f"{version};FauxxUI-Regular;derived-from-NotoSans-2.015",
                nid,
                record.platformID,
                record.platEncID,
                record.langID,
            )
        # name IDs 0 (copyright), 5 (version), 13 (licence), 14 (licence URL)
        # are deliberately left untouched: OFL requires the copyright and
        # licence to travel with the Font Software.

    OUT.parent.mkdir(parents=True, exist_ok=True)
    font.save(OUT)
    out_digest = hashlib.sha256(OUT.read_bytes()).hexdigest()
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes)")
    print(f"  family:        {FAMILY}")
    print(f"  derived from:  {source} ({UPSTREAM_SHA256})")
    print(f"  output sha256: {out_digest}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
