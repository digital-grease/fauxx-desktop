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

//! The C4 #20 A1 efficacy DASHBOARD: the measurement command-center.
//!
//! Pure rendering of a [`DashboardSnapshot`] already loaded into state. It draws
//! the per-platform KL-divergence drift TIMELINES (Google / Brokers / Meta) as a
//! single multi-series line chart on `Canvas`, and the per-category drift HEATMAP
//! for the inspected platform's cross-device combined bundle, also on `Canvas`.
//! It issues no core calls: the platform selector and reload are [`Message`]s the
//! update fn turns into background tasks.
//!
//! The empty / no-data state (no store, or no persona/read-backs yet) renders a
//! plain guidance panel and the charts show their own "No data yet" placeholder,
//! so nothing panics on an empty profile.

use fauxx_core::HeatmapSeries;
use iced::widget::{button, canvas, column, container, pick_list, row, scrollable, text, Space};
use iced::{Element, Length};

use crate::message::{DashboardSnapshot, Message};
use crate::views::charts::{heat_color, Heatmap, HeatmapRow, LineChart, LineSeries};

/// A persona/device choice for the #20 per-device filter pick-list.
#[derive(Clone, PartialEq)]
struct DeviceChoice {
    id: String,
    label: String,
}

impl std::fmt::Display for DeviceChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.label)
    }
}

pub fn view(
    snapshot: Option<&DashboardSnapshot>,
    selected_platform: usize,
    busy: bool,
) -> Element<'_, Message> {
    let body: Element<'_, Message> = match snapshot {
        Some(snapshot) => loaded(snapshot, selected_platform),
        None => text("Loading efficacy measurements...").size(14).into(),
    };

    column![toolbar(busy), body]
        .spacing(12)
        .height(Length::Fill)
        .into()
}

/// The top bar: title, reload, and back-to-Running.
fn toolbar(busy: bool) -> Element<'static, Message> {
    let reload = button(text(if busy { "Working..." } else { "Reload" }))
        .on_press_maybe((!busy).then_some(Message::RefreshDashboard))
        .padding(8);
    let back = button(text("< Back"))
        .on_press(Message::CloseDashboard)
        .padding(8);

    row![
        text("Efficacy dashboard").size(20),
        Space::new().width(Length::Fill),
        reload,
        back,
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center)
    .into()
}

/// The loaded body: a summary strip, the multi-series timeline, the platform
/// selector, and the selected platform's heatmap.
fn loaded(snapshot: &DashboardSnapshot, selected_platform: usize) -> Element<'_, Message> {
    if snapshot.persona_id.is_none() {
        return empty_state();
    }

    let timeline = timeline_panel(snapshot);
    let heatmap = heatmap_panel(snapshot, selected_platform);

    let content = column![
        summary_strip(snapshot),
        device_picker(snapshot),
        timeline,
        platform_selector(snapshot, selected_platform),
        heatmap,
    ]
    .spacing(12);

    scrollable(content).height(Length::Fill).into()
}

/// The #20 per-device filter: a pick-list of personas/devices whose drift the
/// dashboard shows. Only rendered when there is more than one device (a single
/// device needs no filter).
fn device_picker(snapshot: &DashboardSnapshot) -> Element<'_, Message> {
    if snapshot.devices.len() < 2 {
        return Space::new().into();
    }
    let choices: Vec<DeviceChoice> = snapshot
        .devices
        .iter()
        .map(|(id, name)| DeviceChoice {
            id: id.clone(),
            label: name.clone(),
        })
        .collect();
    let selected = snapshot
        .persona_id
        .as_ref()
        .and_then(|pid| choices.iter().find(|c| &c.id == pid).cloned());

    container(
        row![
            text("Device:").size(12).width(Length::Fixed(80.0)),
            pick_list(choices, selected, |c: DeviceChoice| {
                Message::DashboardSelectDevice(c.id)
            })
            .padding(6)
            .width(Length::Fill),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center),
    )
    .padding(12)
    .width(Length::Fill)
    .style(crate::style::panel)
    .into()
}

/// The #20 heatmap colour-scale legend: a low-to-high gradient key so the cell
/// shading is interpretable.
fn heatmap_legend() -> Element<'static, Message> {
    let mut legend = row![text("low").size(10)]
        .spacing(3)
        .align_y(iced::Alignment::Center);
    for step in 0..=5 {
        let color = heat_color(step as f32 / 5.0);
        legend = legend.push(
            container(Space::new())
                .width(Length::Fixed(18.0))
                .height(Length::Fixed(12.0))
                .style(move |_: &iced::Theme| container::Style {
                    background: Some(color.into()),
                    ..container::Style::default()
                }),
        );
    }
    legend.push(text("high").size(10)).into()
}

/// The no-data guidance panel (no persona / no measurements yet).
fn empty_state() -> Element<'static, Message> {
    let col = column![
        text("No measurement data yet").size(16),
        text(
            "Once a persona is stored and the decoy has recorded Topics read-backs \
             or broker scans, drift timelines and the per-category heatmap appear here."
        )
        .size(12),
    ]
    .spacing(8);
    container(col).padding(12).style(crate::style::panel).into()
}

/// A one-line summary: which persona, how many devices fed the aggregate, and
/// the latest divergence per platform.
fn summary_strip(snapshot: &DashboardSnapshot) -> Element<'_, Message> {
    let device_word = if snapshot.device_count == 1 {
        "device".to_string()
    } else {
        "devices".to_string()
    };
    let mut col = column![text(format!(
        "Aggregate over {} {} (single-device fallback when 1).",
        snapshot.device_count, device_word
    ))
    .size(12),]
    .spacing(4);

    for drift in &snapshot.per_platform {
        let label = drift.series.platform.label();
        let latest = match drift.series.latest() {
            Some(v) => format!("{v:.3}"),
            None => "no data".to_string(),
        };
        col = col.push(text(format!("{label}: latest D_KL = {latest}")).size(12));
    }

    container(col)
        .padding(12)
        .width(Length::Fill)
        .style(crate::style::panel)
        .into()
}

/// The multi-series KL-divergence timeline chart panel.
fn timeline_panel(snapshot: &DashboardSnapshot) -> Element<'_, Message> {
    let series: Vec<LineSeries> = snapshot
        .per_platform
        .iter()
        .enumerate()
        .map(|(i, drift)| LineSeries {
            label: drift.series.platform.label(),
            points: drift
                .series
                .points
                .iter()
                .map(|p| (p.timestamp as f64, p.divergence))
                .collect(),
            color: crate::views::charts::series_color(i),
        })
        .collect();

    let chart = canvas(LineChart::new(series))
        .width(Length::Fill)
        .height(Length::Fixed(200.0));

    container(column![text("Drift over time (KL divergence)").size(14), chart].spacing(8))
        .padding(12)
        .width(Length::Fill)
        .style(crate::style::panel)
        .into()
}

/// The platform selector for the heatmap: one button per built-in platform,
/// the active one highlighted.
fn platform_selector(
    snapshot: &DashboardSnapshot,
    selected_platform: usize,
) -> Element<'_, Message> {
    let mut buttons = row![text("Heatmap platform:").size(13)]
        .spacing(8)
        .align_y(iced::Alignment::Center);

    for (i, drift) in snapshot.per_platform.iter().enumerate() {
        let is_active = i == selected_platform;
        let press = (!is_active).then_some(Message::DashboardSelectPlatform(i));
        buttons = buttons.push(
            button(text(drift.series.platform.label()))
                .on_press_maybe(press)
                .padding(6)
                .style(if is_active {
                    button::primary
                } else {
                    button::secondary
                }),
        );
    }

    container(buttons)
        .padding(8)
        .width(Length::Fill)
        .style(crate::style::panel)
        .into()
}

/// The per-category heatmap panel for the inspected platform's combined bundle.
fn heatmap_panel(snapshot: &DashboardSnapshot, selected_platform: usize) -> Element<'_, Message> {
    let platform_label = snapshot
        .per_platform
        .get(selected_platform)
        .map(|d| d.series.platform.label())
        .unwrap_or_else(|| snapshot.combined.series.platform.label());

    let (rows, columns) = heatmap_rows(&snapshot.combined.heatmap);
    let row_count = rows.len().max(1);
    // Grow with the category count so rows stay readable.
    let chart_height = (row_count as f32 * 22.0).clamp(60.0, 360.0);

    let chart = canvas(Heatmap::new(rows, columns))
        .width(Length::Fill)
        .height(Length::Fixed(chart_height));

    container(
        column![
            text(format!(
                "Per-category drift heatmap ({platform_label}, cross-device)"
            ))
            .size(14),
            chart,
            heatmap_legend(),
        ]
        .spacing(8),
    )
    .padding(12)
    .width(Length::Fill)
    .style(crate::style::panel)
    .into()
}

/// Convert a [`HeatmapSeries`] into normalized chart rows plus the column count.
/// Each cell is normalized against the global max contribution so the most-
/// drifting category band reads as the most saturated. Empty input yields no
/// rows (the chart then shows its no-data placeholder).
fn heatmap_rows(series: &HeatmapSeries) -> (Vec<HeatmapRow>, usize) {
    let columns = series.timestamps.len();
    if columns == 0 || series.rows.is_empty() {
        return (Vec::new(), 0);
    }

    let global_max = series
        .rows
        .values()
        .flat_map(|v| v.iter().copied())
        .fold(0.0_f64, f64::max)
        .max(f64::MIN_POSITIVE);

    let rows = series
        .rows
        .iter()
        .map(|(label, values)| HeatmapRow {
            label: label.clone(),
            values: values.iter().map(|v| (v / global_max) as f32).collect(),
        })
        .collect();

    (rows, columns)
}
