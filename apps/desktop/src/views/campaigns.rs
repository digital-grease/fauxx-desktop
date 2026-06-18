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

//! The C8 #33 U2 campaign PANEL.
//!
//! Pure rendering of a [`CampaignsSnapshot`] plus the new-campaign draft. Two
//! sections: the campaign LIST (each with its last metric, gap, status, and a
//! Start/Pause control) and the CREATE form (the goal is a target segment, a
//! comparator, and a threshold). It issues no core calls: every control emits a
//! [`Message`] the update fn turns into a background task over the core campaign
//! API (list, save, start, pause).

use fauxx_core::persona::CategoryPool;
use fauxx_core::{Campaign, CampaignStatus, Comparator};
use iced::widget::{
    button, column, container, pick_list, row, scrollable, text, text_input, Space,
};
use iced::{Color, Element, Length};

use crate::message::{CampaignDraft, CampaignsSnapshot, Message};

pub fn view<'a>(
    snapshot: Option<&'a CampaignsSnapshot>,
    draft: &'a CampaignDraft,
    busy: bool,
) -> Element<'a, Message> {
    let body: Element<'a, Message> = match snapshot {
        Some(snapshot) => loaded(snapshot, draft, busy),
        None => text("Loading campaigns...").size(14).into(),
    };

    column![toolbar(busy), body]
        .spacing(12)
        .height(Length::Fill)
        .into()
}

fn toolbar(busy: bool) -> Element<'static, Message> {
    let reload = button(text(if busy { "Working..." } else { "Reload" }))
        .on_press_maybe((!busy).then_some(Message::RefreshCampaigns))
        .padding(8);
    let back = button(text("< Back"))
        .on_press(Message::CloseCampaigns)
        .padding(8);

    row![
        text("Campaigns").size(20),
        Space::new().width(Length::Fill),
        reload,
        back,
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center)
    .into()
}

fn loaded<'a>(
    snapshot: &'a CampaignsSnapshot,
    draft: &'a CampaignDraft,
    busy: bool,
) -> Element<'a, Message> {
    let left = scrollable(list_column(&snapshot.campaigns, busy))
        .width(Length::FillPortion(3))
        .height(Length::Fill);
    let right = scrollable(create_form(snapshot, draft, busy))
        .width(Length::FillPortion(2))
        .height(Length::Fill);

    row![left, right].spacing(16).height(Length::Fill).into()
}

/// The campaign list, each with progress (last metric, gap, status) and a
/// Start/Pause control.
fn list_column<'a>(campaigns: &'a [Campaign], busy: bool) -> Element<'a, Message> {
    let mut col = column![text("Active and planned campaigns").size(16)].spacing(10);
    if campaigns.is_empty() {
        col = col.push(text("No campaigns yet. Create one on the right.").size(12));
    } else {
        for campaign in campaigns {
            col = col.push(campaign_card(campaign, busy));
        }
    }

    container(col)
        .padding(12)
        .width(Length::Fill)
        .style(panel_style)
        .into()
}

fn campaign_card<'a>(campaign: &'a Campaign, busy: bool) -> Element<'a, Message> {
    let goal = &campaign.goal;
    let comparator = match goal.comparator {
        Comparator::AtLeast => "\u{2265}",
        Comparator::AtMost => "\u{2264}",
    };

    let metric = campaign
        .progress
        .last_metric
        .map(|m| format!("{m:.4}"))
        .unwrap_or_else(|| "not yet measured".to_string());
    let gap = campaign
        .progress
        .gap()
        .map(|g| {
            if g <= 0.0 {
                format!("goal met (gap {g:.4})")
            } else {
                format!("{g:.4} to go")
            }
        })
        .unwrap_or_else(|| "-".to_string());

    let (status_label, status_color) = status_style(campaign.status);

    // Start/Resume is offered for non-running, non-achieved campaigns; Pause for
    // running ones. Achieved campaigns are terminal (no control).
    let control: Element<'a, Message> = match campaign.status {
        CampaignStatus::Running => button(text("Pause"))
            .on_press_maybe((!busy).then_some(Message::CampaignPause(campaign.id.clone())))
            .padding(6)
            .into(),
        CampaignStatus::Planned | CampaignStatus::Paused => button(text("Start"))
            .on_press_maybe((!busy).then_some(Message::CampaignStart(campaign.id.clone())))
            .padding(6)
            .style(button::primary)
            .into(),
        CampaignStatus::Achieved => text("achieved").size(11).color(status_color).into(),
    };

    let header = row![
        text(campaign.label.clone()).size(14),
        Space::new().width(Length::Fill),
        text(status_label).size(11).color(status_color),
        control,
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center);

    let detail = column![
        labeled("Persona", campaign.persona_id.clone()),
        labeled("Segment", campaign.target_segment.clone()),
        labeled(
            "Goal",
            format!(
                "{} {comparator} {:.4}",
                goal.metric.as_str(),
                goal.threshold
            ),
        ),
        labeled("Last metric", metric),
        labeled("Gap", gap),
    ]
    .spacing(2);

    container(column![header, detail].spacing(6))
        .padding(10)
        .width(Length::Fill)
        .style(card_style)
        .into()
}

/// The new-campaign create form: label + persona + segment + comparator +
/// threshold, all feeding [`Message::CampaignCreate`].
fn create_form<'a>(
    snapshot: &'a CampaignsSnapshot,
    draft: &'a CampaignDraft,
    busy: bool,
) -> Element<'a, Message> {
    let label = text_input("Campaign label", &draft.label)
        .on_input(Message::CampaignDraftLabel)
        .padding(6)
        .width(Length::Fill);

    // Persona picker (id + display name).
    let persona_choices: Vec<IdChoice> = snapshot
        .personas
        .iter()
        .map(|(id, name)| IdChoice {
            id: id.clone(),
            label: name.clone(),
        })
        .collect();
    let persona_selected = persona_choices
        .iter()
        .find(|c| c.id == draft.persona_id)
        .cloned();
    let persona_picker: Element<'a, Message> = if persona_choices.is_empty() {
        text("(no personas; add one first)").size(11).into()
    } else {
        pick_list(persona_choices, persona_selected, |c: IdChoice| {
            Message::CampaignDraftPersona(c.id)
        })
        .padding(6)
        .width(Length::Fill)
        .into()
    };

    // Target segment picker, over the CategoryPool names.
    let segment_choices: Vec<String> = CategoryPool::all()
        .iter()
        .map(|c| c.as_name().to_string())
        .collect();
    let segment_selected = if draft.segment.is_empty() {
        None
    } else {
        Some(draft.segment.clone())
    };
    let segment_picker = pick_list(
        segment_choices,
        segment_selected,
        Message::CampaignDraftSegment,
    )
    .padding(6)
    .width(Length::Fill);

    // Comparator picker.
    let comparator_choices = vec![
        ComparatorChoice(Comparator::AtLeast),
        ComparatorChoice(Comparator::AtMost),
    ];
    let comparator_picker = pick_list(
        comparator_choices,
        Some(ComparatorChoice(draft.comparator)),
        |c: ComparatorChoice| Message::CampaignDraftComparator(c.0),
    )
    .padding(6)
    .width(Length::Fill);

    let threshold = text_input("0.5", &draft.threshold)
        .on_input(Message::CampaignDraftThreshold)
        .padding(6)
        .width(Length::Fill);

    let create = button(text("Create campaign"))
        .on_press_maybe((!busy).then_some(Message::CampaignCreate))
        .padding(8)
        .style(button::primary);

    let col = column![
        text("New campaign").size(16),
        field_row("Label", label.into()),
        field_row("Persona", persona_picker),
        field_row("Segment", segment_picker.into()),
        field_row("Comparator", comparator_picker.into()),
        field_row("Threshold", threshold.into()),
        text("Goal metric: segment drift (the A1 KL-divergence for the segment).").size(10),
        create,
    ]
    .spacing(8);

    container(col)
        .padding(12)
        .width(Length::Fill)
        .style(panel_style)
        .into()
}

/// A labeled form row: a fixed-width caption plus the control.
fn field_row<'a>(label: &'a str, control: Element<'a, Message>) -> Element<'a, Message> {
    row![
        text(format!("{label}:"))
            .size(12)
            .width(Length::Fixed(90.0)),
        control,
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center)
    .into()
}

fn labeled(label: &str, value: String) -> Element<'static, Message> {
    row![
        text(format!("{label}:"))
            .size(11)
            .width(Length::Fixed(90.0)),
        text(value).size(11),
    ]
    .spacing(8)
    .into()
}

/// A `(id, label)` pick-list choice (selection on the id).
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

/// A comparator pick-list choice with a readable label.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ComparatorChoice(pub Comparator);

impl std::fmt::Display for ComparatorChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self.0 {
            Comparator::AtLeast => "at least (drive up)",
            Comparator::AtMost => "at most (drive down)",
        };
        f.write_str(s)
    }
}

fn status_style(status: CampaignStatus) -> (&'static str, Color) {
    match status {
        CampaignStatus::Planned => ("PLANNED", Color::from_rgba8(0x55, 0x55, 0x60, 1.0)),
        CampaignStatus::Running => ("RUNNING", Color::from_rgba8(0x10, 0x6a, 0x30, 1.0)),
        CampaignStatus::Achieved => ("ACHIEVED", Color::from_rgba8(0x16, 0x50, 0x8a, 1.0)),
        CampaignStatus::Paused => ("PAUSED", Color::from_rgba8(0x9a, 0x6a, 0x00, 1.0)),
    }
}

fn panel_style(_theme: &iced::Theme) -> container::Style {
    container::Style {
        background: Some(iced::Color::from_rgba8(0xf6, 0xf6, 0xf8, 1.0).into()),
        text_color: Some(iced::Color::from_rgba8(0x1a, 0x1a, 0x1f, 1.0)),
        border: iced::Border {
            color: iced::Color::from_rgba8(0xdd, 0xdd, 0xe0, 1.0),
            width: 1.0,
            radius: 6.0.into(),
        },
        ..container::Style::default()
    }
}

fn card_style(_theme: &iced::Theme) -> container::Style {
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
