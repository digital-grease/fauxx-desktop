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

//! The C4 #22 A3 broker-DIFF view.
//!
//! Pure rendering of a [`BrokerDiffSnapshot`] already loaded into state. It
//! shows, for a selected `(broker, persona)`, the per-broker, time-ordered
//! field-level diff produced by `core.broker_diff_timeline`: each consecutive
//! snapshot pair as a step, with every field flagged added / removed /
//! unchanged, and RE-LISTING (a removed field reappearing) called out
//! distinctly. Fewer than two snapshots renders the clear "no diff yet" state.
//!
//! It issues no core calls: the two selectors emit [`Message`]s the update fn
//! turns into background reloads.

use fauxx_core::{BrokerDiffTimeline, FieldChange, SnapshotDiff};
use iced::widget::{column, container, pick_list, row, scrollable, text, Space};
use iced::{Element, Length};

use crate::message::{BrokerDiffSnapshot, Message};

pub fn view(snapshot: Option<&BrokerDiffSnapshot>, busy: bool) -> Element<'_, Message> {
    let body: Element<'_, Message> = match snapshot {
        Some(snapshot) => loaded(snapshot, busy),
        None => text("Loading broker diff timeline...").size(14).into(),
    };

    column![toolbar(busy), body]
        .spacing(12)
        .height(Length::Fill)
        .into()
}

fn toolbar(busy: bool) -> Element<'static, Message> {
    let reload = iced::widget::button(text(if busy { "Working..." } else { "Reload" }))
        .on_press_maybe((!busy).then_some(Message::RefreshBrokers))
        .padding(8);
    let back = iced::widget::button(text("< Back"))
        .on_press(Message::CloseBrokers)
        .padding(8);

    row![
        text("Broker exposure diff").size(20),
        Space::new().width(Length::Fill),
        reload,
        back,
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center)
    .into()
}

/// One labeled `(id, label)` pick-list choice. `iced::pick_list` needs
/// `T: ToString + PartialEq + Clone`; equality and selection are on the id.
#[derive(Clone, PartialEq, Eq)]
pub struct IdChoice {
    pub id: String,
    pub label: String,
}

impl std::fmt::Display for IdChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

fn loaded<'a>(snapshot: &'a BrokerDiffSnapshot, busy: bool) -> Element<'a, Message> {
    let selectors = selector_panel(snapshot, busy);

    let timeline: Element<'a, Message> = match &snapshot.timeline {
        Some(timeline) => timeline_panel(timeline),
        None => {
            container(text("No persona to inspect yet. Add or import a persona first.").size(13))
                .padding(12)
                .width(Length::Fill)
                .style(crate::style::panel)
                .into()
        }
    };

    column![selectors, scrollable(timeline).height(Length::Fill)]
        .spacing(12)
        .height(Length::Fill)
        .into()
}

/// The persona + broker selectors plus a relisting banner when present.
fn selector_panel<'a>(snapshot: &'a BrokerDiffSnapshot, busy: bool) -> Element<'a, Message> {
    let persona_choices: Vec<IdChoice> = snapshot
        .personas
        .iter()
        .map(|(id, name)| IdChoice {
            id: id.clone(),
            label: name.clone(),
        })
        .collect();
    let persona_selected = snapshot
        .selected_persona
        .as_ref()
        .and_then(|sel| persona_choices.iter().find(|c| &c.id == sel).cloned());

    let broker_choices: Vec<IdChoice> = snapshot
        .brokers
        .iter()
        .map(|(id, name)| IdChoice {
            id: id.clone(),
            label: name.clone(),
        })
        .collect();
    let broker_selected = broker_choices
        .iter()
        .find(|c| c.id == snapshot.selected_broker)
        .cloned();

    let persona_picker: Element<'a, Message> = if busy || persona_choices.is_empty() {
        text("(no persona)").size(12).width(Length::Fill).into()
    } else {
        pick_list(persona_choices, persona_selected, |c: IdChoice| {
            Message::BrokersSelectPersona(c.id)
        })
        .padding(6)
        .width(Length::Fill)
        .into()
    };

    let broker_picker: Element<'a, Message> = if busy || broker_choices.is_empty() {
        text("(no broker)").size(12).width(Length::Fill).into()
    } else {
        pick_list(broker_choices, broker_selected, |c: IdChoice| {
            Message::BrokersSelectBroker(c.id)
        })
        .padding(6)
        .width(Length::Fill)
        .into()
    };

    let mut col = column![
        row![
            text("Persona:").size(12).width(Length::Fixed(80.0)),
            persona_picker,
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center),
        row![
            text("Broker:").size(12).width(Length::Fixed(80.0)),
            broker_picker,
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center),
    ]
    .spacing(8);

    if snapshot
        .timeline
        .as_ref()
        .is_some_and(BrokerDiffTimeline::has_relisting)
    {
        col = col.push(
            text("RE-LISTING DETECTED: a previously-removed field has reappeared.")
                .size(12)
                .style(|t| crate::style::text_in(crate::style::danger_color(t))),
        );
    }

    container(col)
        .padding(12)
        .width(Length::Fill)
        .style(crate::style::panel)
        .into()
}

/// The diff timeline: either the no-diff-yet state or one panel per step.
fn timeline_panel(timeline: &BrokerDiffTimeline) -> Element<'_, Message> {
    let mut col = column![text("Diff timeline").size(16)].spacing(10);

    if timeline.no_diff_yet() {
        col = col.push(
            text(format!(
                "No diff yet: {} snapshot(s) recorded. Two or more are needed to \
                 compare exposures over time.",
                timeline.snapshot_count
            ))
            .size(12),
        );
        return container(col)
            .padding(12)
            .width(Length::Fill)
            .style(crate::style::panel)
            .into();
    }

    col = col.push(
        text(format!(
            "{} snapshot(s), {} diff step(s), oldest first.",
            timeline.snapshot_count,
            timeline.diffs.len()
        ))
        .size(11),
    );

    for (index, diff) in timeline.diffs.iter().enumerate() {
        col = col.push(diff_step(index, diff));
    }

    container(col)
        .padding(12)
        .width(Length::Fill)
        .style(crate::style::panel)
        .into()
}

/// One consecutive-snapshot diff step: a header plus every field's change.
fn diff_step(index: usize, diff: &SnapshotDiff) -> Element<'_, Message> {
    let header = row![
        text(format!("Step {}", index + 1)).size(13),
        Space::new().width(Length::Fill),
        text(format!(
            "{} \u{2192} {}",
            short_millis(diff.from_scanned_at),
            short_millis(diff.to_scanned_at)
        ))
        .size(10),
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center);

    let mut fields = column![].spacing(3);
    if diff.deltas.is_empty() {
        fields = fields.push(text("(both snapshots empty)").size(11));
    } else {
        for delta in &diff.deltas {
            let (tag, color) = change_tag(delta.change);
            fields = fields.push(
                row![
                    text(tag)
                        .size(10)
                        .style(move |t| crate::style::text_in(color(t)))
                        .width(Length::Fixed(90.0)),
                    text(delta.field.clone()).size(11),
                ]
                .spacing(8),
            );
        }
    }

    container(column![header, fields].spacing(6))
        .padding(10)
        .width(Length::Fill)
        .style(crate::style::panel_strong)
        .into()
}

/// The display tag + a theme-aware color resolver for one field change.
///
/// The color is returned as a `fn(&Theme) -> Color` so the tag stays legible
/// under both the Light and Dark themes: added reads as the accent, removed and
/// unchanged as muted captions, and a re-listing as the privacy-critical danger
/// color (still distinct from the muted "unchanged" row in either theme).
fn change_tag(change: FieldChange) -> (&'static str, fn(&iced::Theme) -> iced::Color) {
    match change {
        FieldChange::Added => ("ADDED", crate::style::accent_color),
        FieldChange::Removed => ("REMOVED", crate::style::muted_color),
        FieldChange::Unchanged => ("unchanged", crate::style::muted_color),
        FieldChange::Relisted => ("RE-LISTED", crate::style::danger_color),
    }
}

/// A compact epoch-millis label. The exact wall-clock is not load-bearing for
/// the diff order, so this shows the raw millis (deterministic, no tz logic).
fn short_millis(millis: i64) -> String {
    format!("t={millis}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn change_tag_labels_and_flags_each_field_change() {
        assert_eq!(change_tag(FieldChange::Added).0, "ADDED");
        assert_eq!(change_tag(FieldChange::Removed).0, "REMOVED");
        assert_eq!(change_tag(FieldChange::Unchanged).0, "unchanged");
        // A re-listing (a broker re-adding deleted data) is the privacy-critical
        // signal the view must flag distinctly from an unchanged row.
        assert_eq!(change_tag(FieldChange::Relisted).0, "RE-LISTED");
        // The color is now resolved from the active theme. Resolve both against
        // a concrete theme to confirm the re-listed flag stays a distinct color
        // from the muted "unchanged" row.
        let theme = iced::Theme::Light;
        let relisted = (change_tag(FieldChange::Relisted).1)(&theme);
        let unchanged = (change_tag(FieldChange::Unchanged).1)(&theme);
        assert!(
            (relisted.r - unchanged.r).abs() > f32::EPSILON,
            "the re-listed flag must be a distinct colour from unchanged"
        );
    }

    #[test]
    fn short_millis_is_a_deterministic_label() {
        assert_eq!(short_millis(123), "t=123");
        assert_eq!(short_millis(0), "t=0");
    }

    #[test]
    fn view_renders_loading_no_timeline_and_no_diff_states() {
        use crate::message::BrokerDiffSnapshot;
        use fauxx_core::BrokerDiffTimeline;

        // Loading (no snapshot): renders, no panic.
        let _ = view(None, false);

        // A snapshot with no timeline selected yet (the picker-only state).
        let no_timeline = BrokerDiffSnapshot {
            personas: vec![("p1".to_string(), "Persona One".to_string())],
            brokers: vec![("spokeo".to_string(), "Spokeo".to_string())],
            selected_persona: Some("p1".to_string()),
            selected_broker: "spokeo".to_string(),
            timeline: None,
        };
        let _ = view(Some(&no_timeline), false);

        // AC5: the "no diff yet" state (fewer than two snapshots) must render a
        // clear state at the VIEW layer, not panic.
        let no_diff = BrokerDiffSnapshot {
            personas: vec![("p1".to_string(), "Persona One".to_string())],
            brokers: vec![("spokeo".to_string(), "Spokeo".to_string())],
            selected_persona: Some("p1".to_string()),
            selected_broker: "spokeo".to_string(),
            timeline: Some(BrokerDiffTimeline {
                broker_id: "spokeo".to_string(),
                persona_id: "p1".to_string(),
                snapshot_count: 1,
                diffs: Vec::new(),
            }),
        };
        assert!(
            no_diff
                .timeline
                .as_ref()
                .is_some_and(BrokerDiffTimeline::no_diff_yet),
            "the fixture must be the no-diff-yet state"
        );
        let _ = view(Some(&no_diff), false);
    }
}
