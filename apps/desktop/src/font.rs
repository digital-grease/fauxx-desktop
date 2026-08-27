// fauxx-desktop: Fauxx Desktop Companion
// Copyright (C) 2026 Digital Grease
//
// This program is free software: you can redistribute it and/or modify it
// under the terms of the GNU Affero General Public License as published by the
// Free Software Foundation, either version 3 of the License, or (at your
// option) any later version.
//
// This program is distributed in the hope that it will be useful, but WITHOUT
// ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
// FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more
// details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

//! The bundled UI typeface.
//!
//! # Why the font is embedded (#52)
//!
//! iced's default font is a *generic* family that the text stack resolves
//! through a font database. On a host where that resolution finds nothing, every
//! glyph in the UI renders blank while the layout still draws: a window full of
//! invisible text, with no way to read the instructions that would tell you how
//! to fix it. That is the failure reported in #52 on a minimal Gentoo install,
//! hitting both the shell installer and the AppImage.
//!
//! The mechanism is worth stating precisely, because the obvious guess is wrong.
//! iced's `Font::SANS_SERIF` does **not** consult fontconfig's `sans-serif`
//! alias. cosmic-text overrides whatever the database resolved and hardcodes the
//! literal family `"Open Sans"` after loading system fonts
//! (`cosmic-text/src/font/system.rs`). So the pre-fix app was asking for a family
//! literally named "Open Sans"; the reporter's host had no such family, and none
//! of the platform's common fallbacks (`"Noto Sans"`, `"DejaVu Sans"`,
//! `"FreeSans"`) either, so the last-resort sweep handed back the only face
//! present, iced's own icon font, in which every character is a missing glyph.
//!
//! Shipping our own face removes the host dependency entirely rather than
//! papering over one distribution: the bytes below are registered with the text
//! backend at startup and named as the application default, so the UI renders
//! identically whether or not the host has any fonts installed at all.
//!
//! # Why the family is renamed
//!
//! [`UI_FONT_FAMILY`] is a project-unique name, not the upstream one, and the
//! asset is a renamed derivative built by `packaging/fonts/build-ui-font.py`.
//!
//! This is not cosmetic. Selecting an embedded face is done by family name,
//! because that is the only selector iced 0.14 offers ([`iced::Font::with_name`]);
//! there is no API meaning "use the face I just registered". If the embedded face
//! kept its upstream name, every host that *also* has that family installed would
//! produce a two-candidate match whose tie-break is database insertion order, and
//! the app would render in the host's copy: a different version, possibly
//! subsetted, never the bytes we tested. On any distribution shipping the Noto
//! fonts that is the normal case. A unique family makes the match unambiguous.
//!
//! # Fallback for scripts this face does not cover
//!
//! Naming a specific family does **not** switch off fallback. cosmic-text builds
//! its candidate pool by filtering faces on style and stretch only, never on
//! family (`Attrs::matches`), and its final fallback stage sweeps every remaining
//! face in the database. The requested family therefore only decides what is
//! tried *first*. Text in a script this face does not cover still resolves
//! against the host's fonts exactly as it did before the bundling.
//!
//! That property is what makes one embedded face the right size of answer, so it
//! is worth restating as a rule: **the bundled face guarantees the app's own
//! chrome and generated persona data; user-typed text relies on host fallback.**
//! Guaranteeing arbitrary user-typed script on a fontless host would mean
//! embedding tens of megabytes of CJK and Indic coverage, to serve the
//! intersection of "the user types script X" and "the host has no font for X" --
//! and a user who can *type* a script has an input method for it, which does not
//! meaningfully exist on a host with no fonts for that script.
//!
//! # The face
//!
//! Noto Sans Regular 2.015, renamed, under the SIL Open Font License 1.1. The
//! font and its licence live in `apps/desktop/assets/fonts/`. Noto was chosen for
//! coverage: personas carry generated names and locations drawn from real census
//! distributions, so the UI has to render Latin Extended (including Vietnamese),
//! Greek and Cyrillic without falling back.
//!
//! Only the Regular weight is bundled, because the UI sets no font weights
//! anywhere: hierarchy comes from size alone. Adding a bold face is a real change
//! to the design, not a packaging detail, so it is deliberately not pre-empted.

/// The bundled UI typeface, embedded in the binary.
///
/// Registered with iced at startup via `.font(..)`; see [`ui`] for the handle
/// that selects it.
pub const UI_FONT_BYTES: &[u8] = include_bytes!("../assets/fonts/FauxxUI-Regular.ttf");

/// The family name [`UI_FONT_BYTES`] registers itself under.
///
/// Deliberately project-unique so a same-named face on the host cannot win the
/// match and shadow the bytes we ship (see the module docs). Must stay in
/// lockstep with `FAMILY` in `packaging/fonts/build-ui-font.py`; the tests below
/// assert the asset actually declares it.
pub const UI_FONT_FAMILY: &str = "Fauxx UI";

/// The application's default font handle.
pub fn ui() -> iced::Font {
    iced::Font::with_name(UI_FONT_FAMILY)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Register the bundled bytes into a database the same way iced does, and
    /// return it with the ids the load produced.
    fn registered() -> (fontdb::Database, Vec<fontdb::ID>) {
        let mut db = fontdb::Database::new();
        let before: Vec<fontdb::ID> = db.faces().map(|f| f.id).collect();
        db.load_font_data(UI_FONT_BYTES.to_vec());
        let ids: Vec<fontdb::ID> = db
            .faces()
            .map(|f| f.id)
            .filter(|id| !before.contains(id))
            .collect();
        (db, ids)
    }

    fn query(db: &fontdb::Database, family: &str) -> Option<fontdb::ID> {
        db.query(&fontdb::Query {
            families: &[fontdb::Family::Name(family)],
            ..Default::default()
        })
    }

    /// The asset has to PARSE as a font that fontdb accepts, not merely look
    /// like one. iced discards the result of the font load and its font error
    /// type is uninhabited, so a rejected asset is reported to nobody: the app
    /// would simply be blank again, on exactly the hosts the bundling protects.
    /// This assertion is the only automated defence there is.
    #[test]
    fn bundled_font_is_accepted_by_the_real_font_database() {
        let (_db, ids) = registered();
        assert_eq!(
            ids.len(),
            1,
            "the bundled asset must register exactly one face, got {}",
            ids.len()
        );
    }

    /// `Font::with_name(UI_FONT_FAMILY)` is the app's entire font-selection
    /// mechanism. If the constant and the asset's family record ever drift, the
    /// query misses and iced falls through to the generic default, which is the
    /// #52 bug again.
    #[test]
    fn the_declared_family_resolves_to_the_bundled_face() {
        let (db, ids) = registered();
        let resolved = query(&db, UI_FONT_FAMILY);
        assert_eq!(
            resolved,
            ids.first().copied(),
            "UI_FONT_FAMILY {UI_FONT_FAMILY:?} must resolve to the embedded face"
        );
    }

    /// The point of renaming the face: a host copy of the UPSTREAM family must
    /// not be able to win the match. Simulate the common Linux case, a system
    /// "Noto Sans" already in the database, and assert our query is unambiguous.
    #[test]
    fn a_host_font_cannot_shadow_the_bundled_face() {
        let (mut db, ids) = registered();
        // Load whatever the host actually has, which on a developer machine or
        // CI runner very often includes the upstream family this derives from.
        db.load_system_fonts();
        assert_eq!(
            query(&db, UI_FONT_FAMILY),
            ids.first().copied(),
            "a system font shadowed the bundled face; UI_FONT_FAMILY is not unique enough"
        );
        assert!(
            !UI_FONT_FAMILY.eq_ignore_ascii_case("Noto Sans"),
            "the bundled family must not be the upstream name, or host copies will shadow it"
        );
    }

    /// The UI renders generated persona names and locations drawn from census
    /// distributions, so the bundled face has to cover more than ASCII, or we
    /// have merely moved the blank-glyph problem somewhere less obvious.
    ///
    /// Checked through the same parser the text stack uses, so a face that
    /// parses but lacks a usable character map fails here rather than at
    /// runtime. Anything outside this set is expected to come from host
    /// fallback; see the module docs for where that boundary sits and why.
    #[test]
    fn bundled_font_covers_the_text_the_app_can_produce_itself() {
        let face = match ttf_parser::Face::parse(UI_FONT_BYTES, 0) {
            Ok(face) => face,
            Err(e) => panic!("bundled font does not parse: {e}"),
        };
        // Sanity: if ASCII does not resolve the lookup itself is broken and the
        // assertions below would be meaningless.
        assert!(
            face.glyph_index('A').is_some(),
            "character map is broken: even 'A' does not resolve"
        );
        for (ch, why) in [
            ('\u{00E9}', "Latin-1 (e-acute), ordinary in generated names"),
            (
                '\u{1EC5}',
                "Vietnamese (e-circumflex-tilde), Latin Extended Additional",
            ),
            ('\u{0141}', "Polish (L-stroke), Latin Extended-A"),
            ('\u{03B1}', "Greek (alpha)"),
            ('\u{0416}', "Cyrillic (Zhe)"),
            ('\u{2014}', "em dash, used in the UI's own copy"),
        ] {
            assert!(
                face.glyph_index(ch).is_some(),
                "bundled face does not cover U+{:04X} - {why}; it would render as tofu",
                u32::from(ch)
            );
        }
    }

    /// OFL 1.1 requires the copyright notice and licence to travel with the font
    /// software. The renaming step rewrites the identity records and must leave
    /// these two alone; losing them would make redistribution non-compliant.
    #[test]
    fn the_renamed_asset_still_carries_its_copyright_and_licence() {
        let face = match ttf_parser::Face::parse(UI_FONT_BYTES, 0) {
            Ok(face) => face,
            Err(e) => panic!("bundled font does not parse: {e}"),
        };
        let mut copyright = false;
        let mut licence = false;
        for name in face.names() {
            let Some(value) = name.to_string() else {
                continue;
            };
            // name ID 0 is the copyright notice, 13 the licence description.
            if name.name_id == 0 && value.contains("Noto Project Authors") {
                copyright = true;
            }
            if name.name_id == 13 && value.contains("SIL Open Font License") {
                licence = true;
            }
        }
        assert!(
            copyright,
            "the upstream copyright notice was lost in renaming"
        );
        assert!(licence, "the OFL licence record was lost in renaming");
    }
}
