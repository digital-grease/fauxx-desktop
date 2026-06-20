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

//! Theme-aware shared widget styling.
//!
//! Every view used to hardcode light-mode RGB colors in its own `*_style`
//! closures, so the app could not honor a Light/Dark theme switch. This module
//! centralizes the chrome styling and derives every color from the ACTIVE
//! theme's [`extended palette`](iced::theme::palette::Extended), so the same
//! widgets render correctly under both [`iced::Theme::Light`] and
//! [`iced::Theme::Dark`] (the choice the user makes on the Settings screen).
//!
//! Views call these helpers from their `.style(...)` closures (which already
//! receive `&iced::Theme`) and use the [`Color`] accessors for inline text
//! coloring. Semantic colors (danger/success/warning) come from the palette's
//! own danger/success/warning roles, so "error red" and "ok green" stay
//! meaningful and legible in either theme.

use iced::widget::{container, text};
use iced::{Border, Color, Theme};

/// The standard corner radius used by panels and pills.
const RADIUS: f32 = 6.0;

/// A content panel: a subtly raised background, a themed border, and palette
/// text. Replaces the per-view hardcoded light `panel_style` copies.
pub fn panel(theme: &Theme) -> container::Style {
    let p = theme.extended_palette();
    container::Style {
        background: Some(p.background.weak.color.into()),
        text_color: Some(p.background.weak.text),
        border: Border {
            color: p.background.strong.color,
            width: 1.0,
            radius: RADIUS.into(),
        },
        ..container::Style::default()
    }
}

/// A slightly stronger panel, for nested or emphasized cards.
pub fn panel_strong(theme: &Theme) -> container::Style {
    let p = theme.extended_palette();
    container::Style {
        background: Some(p.background.strong.color.into()),
        text_color: Some(p.background.strong.text),
        border: Border {
            color: p.background.strong.color,
            width: 1.0,
            radius: RADIUS.into(),
        },
        ..container::Style::default()
    }
}

/// The non-fatal error banner: danger-tinted background, legible danger text.
pub fn error_banner(theme: &Theme) -> container::Style {
    let p = theme.extended_palette();
    container::Style {
        background: Some(p.danger.weak.color.into()),
        text_color: Some(p.danger.weak.text),
        border: Border {
            color: p.danger.strong.color,
            width: 1.0,
            radius: RADIUS.into(),
        },
        ..container::Style::default()
    }
}

/// A warning-tinted pill (e.g. a "due soon" indicator).
pub fn warning_pill(theme: &Theme) -> container::Style {
    let p = theme.extended_palette();
    container::Style {
        background: Some(p.warning.weak.color.into()),
        text_color: Some(p.warning.weak.text),
        border: Border {
            color: p.warning.strong.color,
            width: 1.0,
            radius: RADIUS.into(),
        },
        ..container::Style::default()
    }
}

// --- Inline text colors (for `text(...).style(...)` closures) ----------------

/// The default body text color for the current theme.
pub fn text_color(theme: &Theme) -> Color {
    theme.extended_palette().background.base.text
}

/// A dimmed/secondary text color (captions, hints) for the current theme.
pub fn muted_color(theme: &Theme) -> Color {
    theme.extended_palette().secondary.strong.color
}

/// The accent (primary) color for the current theme.
pub fn accent_color(theme: &Theme) -> Color {
    theme.extended_palette().primary.base.color
}

/// The semantic danger color (errors, overdue, paused) for the current theme.
pub fn danger_color(theme: &Theme) -> Color {
    theme.extended_palette().danger.base.color
}

/// The semantic success color (ok, reachable, on-track) for the current theme.
pub fn success_color(theme: &Theme) -> Color {
    theme.extended_palette().success.base.color
}

/// The semantic warning color (due-soon, caution) for the current theme.
pub fn warning_color(theme: &Theme) -> Color {
    theme.extended_palette().warning.base.color
}

/// A `text::Style` carrying just a color, for the common
/// `text(...).style(|t| style::text_in(color))` pattern.
pub fn text_in(color: Color) -> text::Style {
    text::Style { color: Some(color) }
}

/// Convenience: a muted-text style closure result.
pub fn muted_text(theme: &Theme) -> text::Style {
    text_in(muted_color(theme))
}
