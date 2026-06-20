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

//! Rendering the pairing QR.
//!
//! The pairing QR encodes a [`PairingPayload`]
//! string (base64url of a small JSON struct). The core renders it to text so
//! both clients can display it without pulling a windowing or image dependency
//! into the headless core: a unicode block form for terminals (the CLI prints
//! it directly) and an SVG string for the GUI (which can show or save it). The
//! phone scans the same payload either way.

use qrcode::render::svg;
use qrcode::render::unicode::Dense1x2;
use qrcode::{EcLevel, QrCode};

use crate::error::{CoreError, Result};
use crate::sync::wire::PairingPayload;

/// A rendered pairing QR in both text forms.
///
/// `payload` is the exact string encoded in the QR (so a caller can also show
/// it as fallback text); `unicode` is the terminal form; `svg` is the GUI form.
#[derive(Debug, Clone)]
pub struct PairingQr {
    /// The encoded pairing payload (base64url-of-JSON) the QR carries.
    pub payload: String,
    /// A unicode block rendering for terminals (two rows per text line).
    pub unicode: String,
    /// An SVG document string for the GUI to display or export.
    pub svg: String,
    /// The device public-key fingerprint, for printing under the QR so a user
    /// can eyeball it against the discovered peer.
    pub fingerprint: String,
}

/// Render a [`PairingPayload`] to a [`PairingQr`].
///
/// Uses medium error correction (a balance of density and scan robustness for
/// a payload of this size).
pub fn render(payload: &PairingPayload) -> Result<PairingQr> {
    let encoded = payload.encode()?;
    let code = QrCode::with_error_correction_level(encoded.as_bytes(), EcLevel::M)
        .map_err(|e| CoreError::Sync(format!("QR encoding failed: {e}")))?;

    let unicode = code
        .render::<Dense1x2>()
        .dark_color(Dense1x2::Dark)
        .light_color(Dense1x2::Light)
        .quiet_zone(true)
        .build();

    let svg = code
        .render::<svg::Color<'_>>()
        .min_dimensions(256, 256)
        .dark_color(svg::Color("#000000"))
        .light_color(svg::Color("#ffffff"))
        .build();

    let fingerprint = match payload.public_key_bytes() {
        Ok(pk) => super::wire::fingerprint(&pk),
        Err(_) => String::new(),
    };

    Ok(PairingQr {
        payload: encoded,
        unicode,
        svg,
        fingerprint,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::crypto::PUBLIC_KEY_LEN;
    use crate::sync::wire::PairingPayload;

    fn payload() -> PairingPayload {
        PairingPayload::new(
            "Desktop-QR".to_string(),
            &[3u8; PUBLIC_KEY_LEN],
            Some("desktop.local.".to_string()),
            45_999,
        )
    }

    #[test]
    fn renders_both_forms() -> Result<()> {
        let qr = render(&payload())?;
        // Unicode form has block glyphs and multiple lines.
        assert!(qr.unicode.contains('\n'));
        assert!(!qr.unicode.is_empty());
        // SVG form is a well-formed-ish document.
        assert!(qr.svg.contains("<svg"));
        assert!(qr.svg.contains("</svg>"));
        // The carried payload decodes back to the original.
        let back = PairingPayload::decode(&qr.payload)?;
        assert_eq!(back, payload());
        // Fingerprint is the grouped form.
        assert_eq!(qr.fingerprint.matches(':').count(), 3);
        Ok(())
    }

    #[test]
    fn qr_payload_decodes_to_same_public_key() -> Result<()> {
        let qr = render(&payload())?;
        let decoded = PairingPayload::decode(&qr.payload)?;
        assert_eq!(decoded.public_key_bytes()?, [3u8; PUBLIC_KEY_LEN]);
        Ok(())
    }
}
