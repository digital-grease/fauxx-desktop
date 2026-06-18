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

//! The C3 privacy HUB: one screen with tabs over the privacy surfaces the core
//! already tracks: DSAR requests + statutory deadlines (#16), the email-alias
//! inventory (#17), per-site GPC honoring (#18), and the account-anchor map
//! (#19). All read-only here.
//!
//! Pure rendering of a [`PrivacySnapshot`] already loaded into state (the rows
//! are pre-formatted in [`crate::bg::load_privacy`], so this module only lays
//! them out). It issues no core calls: the toolbar/tab controls emit a
//! [`Message`] the update fn turns into a reload or a tab switch.

use iced::widget::{button, column, container, row, scrollable, text, Space};
use iced::{Color, Element, Length};

use crate::message::{
    AliasRow, AnchorRecommendationRow, AnchorRow, DsarRow, GpcRow, Message, PrivacySnapshot,
};
use crate::state::PrivacyTab;

/// Red flag for an overdue DSAR deadline.
const OVERDUE: Color = Color {
    r: 0.69,
    g: 0.0,
    b: 0.13,
    a: 1.0,
};
/// Muted grey for secondary text (on-track deadlines, "not sent", etc.).
const MUTED: Color = Color {
    r: 0.42,
    g: 0.42,
    b: 0.47,
    a: 1.0,
};

/// Render the privacy hub: toolbar, tab selector, and the active tab's section.
pub fn view(
    snapshot: Option<&PrivacySnapshot>,
    tab: PrivacyTab,
    busy: bool,
) -> Element<'_, Message> {
    let body: Element<'_, Message> = match snapshot {
        Some(snapshot) => loaded(snapshot, tab),
        None => text("Loading privacy data...").size(14).into(),
    };

    column![toolbar(busy), tab_selector(tab), body]
        .spacing(12)
        .height(Length::Fill)
        .into()
}

fn toolbar(busy: bool) -> Element<'static, Message> {
    let reload = button(text(if busy { "Working..." } else { "Reload" }))
        .on_press_maybe((!busy).then_some(Message::RefreshPrivacy))
        .padding(8);
    let back = button(text("< Back"))
        .on_press(Message::ClosePrivacy)
        .padding(8);

    row![
        text("Privacy").size(20),
        Space::new().width(Length::Fill),
        reload,
        back,
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center)
    .into()
}

/// The tab strip. The active tab's button is non-pressable, so it reads as the
/// current selection.
fn tab_selector(active: PrivacyTab) -> Element<'static, Message> {
    let mut tabs = row![].spacing(6);
    for tab in PrivacyTab::all() {
        let is_active = tab == active;
        let label = if is_active {
            format!("[{}]", tab.label())
        } else {
            tab.label().to_string()
        };
        let pressable = button(text(label).size(14))
            .padding(6)
            .on_press_maybe((!is_active).then_some(Message::SetPrivacyTab(tab)));
        tabs = tabs.push(pressable);
    }
    tabs.into()
}

fn loaded(snapshot: &PrivacySnapshot, tab: PrivacyTab) -> Element<'_, Message> {
    let section = match tab {
        PrivacyTab::Dsar => dsar_section(&snapshot.dsar),
        PrivacyTab::Aliases => alias_section(&snapshot.aliases),
        PrivacyTab::Gpc => gpc_section(&snapshot.gpc),
        PrivacyTab::Anchors => anchor_section(&snapshot.anchors, &snapshot.anchor_recommendations),
    };
    scrollable(
        container(section)
            .padding(12)
            .width(Length::Fill)
            .style(panel_style),
    )
    .height(Length::Fill)
    .into()
}

/// DSAR requests + statutory deadline state (#16).
fn dsar_section(rows: &[DsarRow]) -> Element<'_, Message> {
    if rows.is_empty() {
        return empty("No data-subject requests yet. Generate one with `fauxx-cli dsar`.");
    }
    let mut col = column![section_title(
        "Data-subject requests and statutory deadlines"
    )]
    .spacing(6);
    for r in rows {
        let deadline_color = if r.overdue { OVERDUE } else { MUTED };
        col = col.push(
            row![
                text(r.controller.as_str())
                    .size(14)
                    .width(Length::FillPortion(3)),
                text(r.kind.as_str()).size(13).width(Length::FillPortion(2)),
                text(r.status.as_str())
                    .size(13)
                    .width(Length::FillPortion(2)),
                text(r.deadline.as_str())
                    .size(13)
                    .color(deadline_color)
                    .width(Length::FillPortion(2)),
            ]
            .spacing(8),
        );
    }
    col.into()
}

/// Email-alias inventory (#17).
fn alias_section(rows: &[AliasRow]) -> Element<'_, Message> {
    if rows.is_empty() {
        return empty("No email aliases yet. Mint one with `fauxx-cli alias`.");
    }
    let mut col = column![section_title("Email aliases")].spacing(6);
    for a in rows {
        col = col.push(
            row![
                text(a.site.as_str()).size(14).width(Length::FillPortion(2)),
                text(a.address.as_str())
                    .size(13)
                    .width(Length::FillPortion(3)),
                text(a.kind.as_str()).size(12).width(Length::FillPortion(1)),
                text(a.status.as_str())
                    .size(12)
                    .color(MUTED)
                    .width(Length::FillPortion(1)),
            ]
            .spacing(8),
        );
    }
    col.into()
}

/// Per-site GPC honoring (#18).
fn gpc_section(rows: &[GpcRow]) -> Element<'_, Message> {
    if rows.is_empty() {
        return empty("No GPC observations yet. Check a site with `fauxx-cli gpc`.");
    }
    let mut col = column![section_title("Per-site Global Privacy Control honoring")].spacing(6);
    for g in rows {
        let (label, color) = if g.honored {
            ("honored", MUTED)
        } else {
            ("NOT honored", OVERDUE)
        };
        col = col.push(
            row![
                text(g.origin.as_str())
                    .size(14)
                    .width(Length::FillPortion(3)),
                text(label)
                    .size(13)
                    .color(color)
                    .width(Length::FillPortion(1)),
            ]
            .spacing(8),
        );
    }
    col.into()
}

/// The account-anchor map + prioritized partitioning recommendations (#19).
fn anchor_section<'a>(
    rows: &'a [AnchorRow],
    recs: &'a [AnchorRecommendationRow],
) -> Element<'a, Message> {
    if rows.is_empty() && recs.is_empty() {
        return empty("No account anchors yet. Record one with `fauxx-cli anchor`.");
    }
    let mut col = column![section_title("Account anchors (real identity touchpoints)")].spacing(6);
    for a in rows {
        let linkage = if a.linked {
            text("linked").size(12).color(OVERDUE)
        } else {
            text("isolated").size(12).color(MUTED)
        };
        col = col.push(
            row![
                text(a.label.as_str())
                    .size(14)
                    .width(Length::FillPortion(2)),
                text(a.site.as_str()).size(13).width(Length::FillPortion(2)),
                text(format!("{} signals", a.signals))
                    .size(12)
                    .color(MUTED)
                    .width(Length::FillPortion(1)),
                linkage.width(Length::FillPortion(1)),
            ]
            .spacing(8),
        );
    }

    // Prioritized partitioning recommendations (highest linkage first): the
    // analysis, not just the raw inventory. Each carries its score + rationale.
    if !recs.is_empty() {
        col = col.push(section_title(
            "Recommended partitioning (highest linkage first)",
        ));
        for r in recs {
            col = col.push(
                row![
                    text(r.label.as_str())
                        .size(13)
                        .width(Length::FillPortion(2)),
                    text(r.action.as_str())
                        .size(13)
                        .color(OVERDUE)
                        .width(Length::FillPortion(2)),
                    text(format!("score {}", r.score))
                        .size(12)
                        .color(MUTED)
                        .width(Length::FillPortion(1)),
                ]
                .spacing(8),
            );
            col = col.push(text(r.rationale.as_str()).size(11).color(MUTED));
        }
    }
    col.into()
}

/// A bold section heading.
fn section_title(label: &str) -> Element<'_, Message> {
    text(label.to_string()).size(15).into()
}

/// The empty-state line for a section with no data yet.
fn empty(message: &str) -> Element<'_, Message> {
    container(text(message.to_string()).size(13).color(MUTED))
        .padding(12)
        .width(Length::Fill)
        .into()
}

/// A light card background for the section list.
fn panel_style(_theme: &iced::Theme) -> container::Style {
    container::Style {
        background: Some(iced::Color::from_rgba8(0xff, 0xff, 0xff, 1.0).into()),
        text_color: Some(iced::Color::from_rgba8(0x1a, 0x1a, 0x1f, 1.0)),
        border: iced::Border {
            color: iced::Color::from_rgba8(0xe5, 0xe5, 0xe8, 1.0),
            width: 1.0,
            radius: 4.0.into(),
        },
        ..container::Style::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> PrivacySnapshot {
        PrivacySnapshot {
            dsar: vec![DsarRow {
                controller: "Spokeo".to_string(),
                kind: "Erasure".to_string(),
                status: "sent".to_string(),
                deadline: "overdue".to_string(),
                overdue: true,
            }],
            aliases: vec![AliasRow {
                site: "example.com".to_string(),
                address: "x@alias.test".to_string(),
                kind: "Plus".to_string(),
                status: "active".to_string(),
            }],
            gpc: vec![GpcRow {
                origin: "https://news.example".to_string(),
                honored: false,
            }],
            anchors: vec![AnchorRow {
                label: "Personal email".to_string(),
                site: "mail.example".to_string(),
                signals: 2,
                linked: true,
            }],
            anchor_recommendations: vec![AnchorRecommendationRow {
                label: "Personal email".to_string(),
                action: "isolate high anchor".to_string(),
                score: 7,
                rationale: "Shared recovery contact links 3 accounts.".to_string(),
            }],
        }
    }

    // Rendering returns an Element without panicking for every tab, both with a
    // populated snapshot and the empty/None states. (iced has no headless
    // renderer to assert pixels; this guards the view-construction logic.)
    #[test]
    fn every_tab_renders_for_populated_loading_and_empty() {
        let populated = snapshot();
        let empty = PrivacySnapshot::default();
        for tab in PrivacyTab::all() {
            let _ = view(Some(&populated), tab, false);
            let _ = view(Some(&empty), tab, false);
            let _ = view(None, tab, true);
        }
    }
}
