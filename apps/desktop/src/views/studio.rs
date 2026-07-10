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

//! The C5 persona STUDIO.
//!
//! Pure rendering of a [`StudioSnapshot`] already loaded into state. Four
//! panels, mirroring the Devices view's structure:
//!
//! - The persona EDITOR (#24 P1): every persona field, with per-field LOCK
//!   toggles (a checkbox driving [`Message::StudioToggleLock`]) and the rotation
//!   config (enabled cadence vs pinned, [`Message::StudioSetRotation`]).
//! - The coherence-linter findings (#25 P2), shown inline with Warning vs
//!   HardImplausible styling.
//! - The week-SIMULATOR preview (#26 P3) on `Canvas`, with a re-roll seed button.
//! - The persona LIBRARY (#27 P4): installed packs, plus import/export buttons
//!   that the update fn turns into native-file-dialog background tasks.
//!
//! It issues no core calls and contains no business logic: text edits mutate the
//! in-memory edit buffer (in `update`), and every action is a [`Message`].

use fauxx_core::persona::{AgeRange, CategoryPool, Profession, Region};
use fauxx_core::{
    DeviceProfile, Finding, PersonaField, PersonaSettings, RotationSchedule, Severity,
    SimulatedWeek, SyntheticPersona,
};
use iced::widget::{
    button, canvas, checkbox, column, container, pick_list, row, scrollable, text, text_input,
    Space,
};
use iced::{Element, Length};

use crate::message::{Message, PersonaDetail, PersonaEnumField, PersonaTextField, StudioSnapshot};
use crate::views::charts::{DayBar, WeekTimeline};

pub fn view(snapshot: Option<&StudioSnapshot>, busy: bool) -> Element<'_, Message> {
    let body: Element<'_, Message> = match snapshot {
        Some(snapshot) => loaded(snapshot, busy),
        None => text("Loading persona studio...").size(14).into(),
    };

    column![toolbar(busy), body]
        .spacing(12)
        .height(Length::Fill)
        .into()
}

fn toolbar(busy: bool) -> Element<'static, Message> {
    let reload = button(text(if busy { "Working..." } else { "Reload" }))
        .on_press_maybe((!busy).then_some(Message::RefreshStudio))
        .padding(8);
    let back = button(text("< Back"))
        .on_press(Message::CloseStudio)
        .padding(8);

    row![
        text("Persona studio").size(20),
        Space::new().width(Length::Fill),
        reload,
        back,
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center)
    .into()
}

/// Two columns: the library/persona list on the left, the editor + linter +
/// simulator on the right.
fn loaded<'a>(snapshot: &'a StudioSnapshot, busy: bool) -> Element<'a, Message> {
    let left = scrollable(library_column(snapshot, busy))
        .width(Length::FillPortion(2))
        .height(Length::Fill);

    let right: Element<'a, Message> = match &snapshot.detail {
        Some(detail) => scrollable(detail_column(detail, busy))
            .width(Length::FillPortion(3))
            .height(Length::Fill)
            .into(),
        None => container(
            text("No persona selected. Import a pack or add a persona to begin.").size(13),
        )
        .padding(12)
        .width(Length::FillPortion(3))
        .style(crate::style::panel)
        .into(),
    };

    row![left, right].spacing(16).height(Length::Fill).into()
}

// --- Left column: persona list + library (#27 P4) --------------------------

fn library_column<'a>(snapshot: &'a StudioSnapshot, busy: bool) -> Element<'a, Message> {
    let selected_id = snapshot
        .detail
        .as_ref()
        .map(|d| d.persona.id.as_str())
        .unwrap_or("");

    let mut personas = column![text("Personas").size(16)].spacing(6);
    if snapshot.personas.is_empty() {
        personas = personas.push(text("No personas stored yet.").size(12));
    } else {
        for persona in &snapshot.personas {
            personas = personas.push(persona_row(persona, persona.id == selected_id, busy));
        }
    }

    let mut packs = column![text("Installed packs").size(16)].spacing(6);
    if snapshot.installed_packs.is_empty() {
        packs = packs.push(text("No packs imported yet.").size(12));
    } else {
        for pack in &snapshot.installed_packs {
            let record = &pack.record;
            let remove = button(text("Remove").size(11))
                .on_press_maybe((!busy).then_some(Message::StudioRemovePack(record.id.clone())))
                .padding(4);
            packs = packs.push(
                row![
                    column![
                        text(record.provenance.source_distribution.clone()).size(13),
                        text(format!("{} persona(s)", record.persona_count())).size(11),
                        text(short_key(&record.signer_public_key)).size(10),
                    ]
                    .spacing(2),
                    iced::widget::Space::new().width(Length::Fill),
                    remove,
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            );
        }
    }

    let actions = row![
        button(text("Import pack..."))
            .on_press_maybe((!busy).then_some(Message::StudioImportPack))
            .padding(6),
        button(text("Export selected..."))
            .on_press_maybe((!busy).then_some(Message::StudioExportPack))
            .padding(6),
    ]
    .spacing(8);

    let col = column![
        container(personas)
            .padding(12)
            .width(Length::Fill)
            .style(crate::style::panel),
        container(column![packs, actions].spacing(10))
            .padding(12)
            .width(Length::Fill)
            .style(crate::style::panel),
    ]
    .spacing(12);

    col.into()
}

fn persona_row(persona: &SyntheticPersona, selected: bool, busy: bool) -> Element<'_, Message> {
    let press = (!busy && !selected).then_some(Message::StudioSelectPersona(persona.id.clone()));
    button(
        column![
            text(persona.name.clone()).size(13),
            text(format!("{} - {}", persona.region, persona.profession)).size(10),
        ]
        .spacing(2),
    )
    .on_press_maybe(press)
    .padding(6)
    .width(Length::Fill)
    .style(if selected {
        button::primary
    } else {
        button::secondary
    })
    .into()
}

// --- Right column: editor + linter + simulator -----------------------------

fn detail_column<'a>(detail: &'a PersonaDetail, busy: bool) -> Element<'a, Message> {
    column![
        editor_panel(detail, busy),
        device_panel(&detail.desktop_device, &detail.mobile_device),
        rotation_panel(&detail.settings, busy),
        linter_panel(&detail.findings),
        simulator_panel(&detail.week, detail.seed, busy),
    ]
    .spacing(12)
    .into()
}

/// The #24 P1 editor: per-field rows with a value control and a lock checkbox.
fn editor_panel<'a>(detail: &'a PersonaDetail, busy: bool) -> Element<'a, Message> {
    let p = &detail.persona;
    let s = &detail.settings;

    let mut col = column![text("Editor").size(16)].spacing(8);

    col = col.push(text_field_row(
        "Name",
        &p.name,
        PersonaTextField::Name,
        PersonaField::Name,
        s,
        busy,
    ));
    // The enum-typed identity fields are now editable through dropdown PICKERS
    // (C5 P1), built from the core enum `all()` lists. The chosen variant is
    // stored as its wire NAME on the buffer; the per-field lock still pins it
    // across regeneration/rotation.
    col = col.push(age_range_row(&p.age_range, s, busy));
    col = col.push(profession_row(&p.profession, s, busy));
    col = col.push(region_row(&p.region, s, busy));
    // The interest set is a multi-select toggle over CategoryPool::all(),
    // enforcing the 3..=5 count (enforcement lives in `update`).
    col = col.push(interest_editor(&p.interests, s, busy));
    col = col.push(text_field_row(
        "Home location",
        p.home_location.as_deref().unwrap_or(""),
        PersonaTextField::HomeLocation,
        PersonaField::HomeLocation,
        s,
        busy,
    ));
    col = col.push(text_field_row(
        "Schedule",
        p.schedule.as_deref().unwrap_or(""),
        PersonaTextField::Schedule,
        PersonaField::Schedule,
        s,
        busy,
    ));
    col = col.push(text_field_row(
        "Browsing style",
        p.browsing_style.as_deref().unwrap_or(""),
        PersonaTextField::BrowsingStyle,
        PersonaField::BrowsingStyle,
        s,
        busy,
    ));

    col = col.push(
        button(text("Save persona"))
            .on_press_maybe((!busy).then_some(Message::StudioSavePersona))
            .padding(8),
    );

    container(col)
        .padding(12)
        .width(Length::Fill)
        .style(crate::style::panel)
        .into()
}

/// The #47 DESKTOP-DEVICE panel: a READ-ONLY view of the coherent desktop
/// identity (UA + client hints + screen/navigator) this companion presents on
/// the decoy browser for the selected persona, plus a compact summary of the
/// paired phone's mobile identity. It is deliberately not editable: the identity
/// is a pure function of the persona (id + creation time), so both platforms
/// derive the same bytes and nothing crosses the LAN wire. Adjusting it would
/// mean editing the persona, not the device.
fn device_panel<'a>(desktop: &DeviceProfile, mobile: &DeviceProfile) -> Element<'a, Message> {
    let caption = text(
        "Derived deterministically from this persona and stable for its life \
         (only slow Chrome auto-update drift). The paired phone derives the same \
         set, so the device never crosses the LAN wire, and it never carries a \
         HeadlessChrome tell.",
    )
    .size(11)
    .style(|t: &iced::Theme| crate::style::text_in(crate::style::muted_color(t)));

    let screen = format!(
        "{} x {} @ {}x",
        desktop.screen_width, desktop.screen_height, desktop.device_pixel_ratio
    );
    let model = if desktop.model.is_empty() {
        "(desktop reports no model)".to_string()
    } else {
        desktop.model.clone()
    };
    let arch = format!("{}, {}-bit", desktop.architecture(), desktop.bitness());
    let legacy_platform = desktop
        .navigator_platform()
        .unwrap_or("(unset)")
        .to_string();

    let desktop_rows = column![
        device_kv("User agent", desktop.user_agent.clone()),
        device_kv("Platform", desktop.platform.clone()),
        device_kv("Platform ver.", desktop.platform_version.clone()),
        device_kv("navigator.platform", legacy_platform),
        device_kv("Model", model),
        device_kv("Screen", screen),
        device_kv("CPU cores", desktop.hardware_concurrency.to_string()),
        device_kv("Memory", format!("{} GB", desktop.device_memory)),
        device_kv("Architecture", arch),
        device_kv("Client hints", brands_summary(desktop)),
    ]
    .spacing(6);

    // A compact, secondary summary of the phone's half of the same persona, so
    // the coherent phone-plus-laptop pair is visible at a glance.
    let phone = column![
        text("Paired phone (this companion never emits it)").size(12),
        device_kv(
            "Device",
            format!("{} {}", mobile.platform, mobile.model)
                .trim()
                .to_string()
        ),
        device_kv("User agent", mobile.user_agent.clone()),
    ]
    .spacing(4);

    container(
        column![
            text("Desktop device").size(16),
            caption,
            desktop_rows,
            phone,
        ]
        .spacing(10),
    )
    .padding(12)
    .width(Length::Fill)
    .style(crate::style::panel)
    .into()
}

/// One read-only device attribute: a fixed-width label and a wrapping value. The
/// value is `size 11` so a long UA string wraps within the panel rather than
/// forcing horizontal scroll.
fn device_kv(label: &'static str, value: String) -> Element<'static, Message> {
    row![
        text(format!("{label}:"))
            .size(11)
            .width(Length::Fixed(130.0)),
        text(value).size(11).width(Length::Fill),
    ]
    .spacing(8)
    .into()
}

/// The `Sec-CH-UA` brand list rendered as one readable line, e.g.
/// `Chromium 142, Google Chrome 142, Not?A_Brand 24`.
fn brands_summary(device: &DeviceProfile) -> String {
    device
        .brands
        .iter()
        .map(|b| format!("{} {}", b.name, b.version))
        .collect::<Vec<_>>()
        .join(", ")
}

/// An editable field row: a labeled text input plus a lock checkbox.
fn text_field_row<'a>(
    label: &'a str,
    value: &str,
    text_field: PersonaTextField,
    lock_field: PersonaField,
    settings: &PersonaSettings,
    busy: bool,
) -> Element<'a, Message> {
    let input = text_input(label, value)
        .on_input(move |v| Message::StudioEditField(text_field, v))
        .padding(6)
        .width(Length::Fill);

    row![
        text(format!("{label}:"))
            .size(12)
            .width(Length::Fixed(110.0)),
        input,
        lock_checkbox(lock_field, settings, busy),
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center)
    .into()
}

/// One choice in an enum-field dropdown: the wire NAME plus a readable label.
/// `iced::pick_list` needs `T: ToString + PartialEq + Clone`; equality is on the
/// wire name so the currently-stored value selects even when it is a legacy name
/// the enum does not know (which is appended as an explicit choice).
#[derive(Clone, PartialEq, Eq)]
struct EnumChoice {
    /// The wire enum name stored on the persona (e.g. `"AGE_35_44"`).
    name: String,
    /// The human-readable dropdown label.
    label: String,
}

impl std::fmt::Display for EnumChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

/// Build the dropdown choices from a list of enum wire names, ensuring the
/// currently-stored `current` value is always selectable (a legacy/unknown name
/// the enum dropped is appended, tagged, so the picker still reflects it).
fn enum_choices(names: &[&'static str], current: &str) -> Vec<EnumChoice> {
    let mut choices: Vec<EnumChoice> = names
        .iter()
        .map(|name| EnumChoice {
            name: (*name).to_string(),
            label: humanize(name),
        })
        .collect();
    if !current.is_empty() && !choices.iter().any(|c| c.name == current) {
        choices.push(EnumChoice {
            name: current.to_string(),
            label: format!("{} (unknown)", humanize(current)),
        });
    }
    choices
}

/// An enum-picker field row: a labeled dropdown plus a lock checkbox (C5 P1).
fn enum_field_row<'a>(
    label: &'a str,
    choices: Vec<EnumChoice>,
    current: &str,
    enum_field: PersonaEnumField,
    lock_field: PersonaField,
    settings: &PersonaSettings,
    busy: bool,
) -> Element<'a, Message> {
    let selected = choices.iter().find(|c| c.name == current).cloned();
    let picker = pick_list(choices, selected, move |choice: EnumChoice| {
        Message::StudioSetEnumField(enum_field, choice.name)
    })
    .padding(6)
    .width(Length::Fill);

    // Disable the picker while a write is in flight by overlaying nothing extra;
    // `pick_list` has no `_maybe`, so a busy picker is wrapped to swallow input.
    let control: Element<'a, Message> = if busy {
        text(if current.is_empty() {
            "(unset)".to_string()
        } else {
            humanize(current)
        })
        .size(12)
        .width(Length::Fill)
        .into()
    } else {
        picker.into()
    };

    row![
        text(format!("{label}:"))
            .size(12)
            .width(Length::Fixed(110.0)),
        control,
        lock_checkbox(lock_field, settings, busy),
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center)
    .into()
}

/// The age-range picker row (`AgeRange::all()`).
fn age_range_row<'a>(
    current: &str,
    settings: &PersonaSettings,
    busy: bool,
) -> Element<'a, Message> {
    let names: Vec<&'static str> = AgeRange::all().iter().map(AgeRange::as_name).collect();
    enum_field_row(
        "Age range",
        enum_choices(&names, current),
        current,
        PersonaEnumField::AgeRange,
        PersonaField::AgeRange,
        settings,
        busy,
    )
}

/// The profession picker row (`Profession::all()`).
fn profession_row<'a>(
    current: &str,
    settings: &PersonaSettings,
    busy: bool,
) -> Element<'a, Message> {
    let names: Vec<&'static str> = Profession::all().iter().map(Profession::as_name).collect();
    enum_field_row(
        "Profession",
        enum_choices(&names, current),
        current,
        PersonaEnumField::Profession,
        PersonaField::Profession,
        settings,
        busy,
    )
}

/// The region picker row (`Region::all()`).
fn region_row<'a>(current: &str, settings: &PersonaSettings, busy: bool) -> Element<'a, Message> {
    let names: Vec<&'static str> = Region::all().iter().map(Region::as_name).collect();
    enum_field_row(
        "Region",
        enum_choices(&names, current),
        current,
        PersonaEnumField::Region,
        PersonaField::Region,
        settings,
        busy,
    )
}

/// The interest MULTI-SELECT editor (C5 P1): a toggle chip per
/// `CategoryPool::all()` member plus the unknown names already on the persona,
/// with a live count against the 3..=5 rule and the field lock. Membership
/// toggles drive [`Message::StudioToggleInterest`]; the count rule is enforced
/// in `update` (a toggle that would break the bounds is refused).
fn interest_editor<'a>(
    interests: &'a [String],
    settings: &PersonaSettings,
    busy: bool,
) -> Element<'a, Message> {
    let count = interests.len();
    let in_bounds = (3..=5).contains(&count);

    let header = row![
        text("Interests:").size(12).width(Length::Fixed(110.0)),
        text(format!("{count} selected (need 3 to 5)"))
            .size(11)
            .style(move |t| {
                let color = if in_bounds {
                    crate::style::success_color(t)
                } else {
                    crate::style::danger_color(t)
                };
                crate::style::text_in(color)
            }),
        Space::new().width(Length::Fill),
        lock_checkbox(PersonaField::Interests, settings, busy),
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center);

    // The full category pool, plus any unknown names already on the persona so
    // a legacy interest is still visible and removable.
    let mut names: Vec<String> = CategoryPool::all()
        .iter()
        .map(|c| c.as_name().to_string())
        .collect();
    for interest in interests {
        if !names.iter().any(|n| n == interest) {
            names.push(interest.clone());
        }
    }

    let mut chips = column![].spacing(6);
    let mut current_row = row![].spacing(6);
    let mut per_row = 0;
    for name in names {
        let selected = interests.iter().any(|i| i == &name);
        current_row = current_row.push(interest_chip(name, selected, busy));
        per_row += 1;
        if per_row == 4 {
            chips = chips.push(current_row);
            current_row = row![].spacing(6);
            per_row = 0;
        }
    }
    if per_row > 0 {
        chips = chips.push(current_row);
    }

    column![header, chips].spacing(8).into()
}

/// One interest toggle chip: a selected chip is primary-styled, an unselected
/// one secondary. Disabled while a write is in flight.
fn interest_chip(name: String, selected: bool, busy: bool) -> Element<'static, Message> {
    let label = humanize(&name);
    let press = (!busy).then_some(Message::StudioToggleInterest(name));
    button(text(label).size(11))
        .on_press_maybe(press)
        .padding(5)
        .width(Length::FillPortion(1))
        .style(if selected {
            button::primary
        } else {
            button::secondary
        })
        .into()
}

/// Turn a SCREAMING_SNAKE_CASE enum name into a Title Case label for display.
fn humanize(name: &str) -> String {
    name.split('_')
        .filter(|w| !w.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => {
                    let rest: String = chars.as_str().to_lowercase();
                    format!("{}{}", first.to_uppercase(), rest)
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The per-field lock toggle (#24 P1, core.set_field_locked). iced 0.14's
/// `checkbox` carries no label, so a "Lock" caption rides alongside it.
fn lock_checkbox(
    field: PersonaField,
    settings: &PersonaSettings,
    busy: bool,
) -> Element<'static, Message> {
    let locked = settings.is_locked(field);
    let mut cb = checkbox(locked).size(16);
    if !busy {
        cb = cb.on_toggle(move |v| Message::StudioToggleLock(field, v));
    }
    row![cb, text("Lock").size(11)]
        .spacing(4)
        .align_y(iced::Alignment::Center)
        .into()
}

/// The rotation-config panel (#24 P1, core.set_rotation_schedule): a frozen
/// cadence vs pinned (disabled) toggle.
/// One cadence preset: applies `schedule` and is highlighted (and disabled) when
/// it is the persona's current schedule, so the active interval/jitter is clear.
fn cadence_button(
    label: &'static str,
    schedule: RotationSchedule,
    current: RotationSchedule,
    busy: bool,
) -> Element<'static, Message> {
    let active = current == schedule;
    button(text(label))
        .on_press_maybe((!busy && !active).then_some(Message::StudioSetRotation(schedule)))
        .padding(6)
        .style(if active {
            button::primary
        } else {
            button::secondary
        })
        .into()
}

fn rotation_panel(settings: &PersonaSettings, busy: bool) -> Element<'static, Message> {
    let current = settings.rotation;
    let enabled = current.is_enabled();
    let window = match current.window_days() {
        Some((min, max)) => format!("auto-rotate every {min} to {max} days"),
        None => "pinned (never auto-rotate)".to_string(),
    };

    // Selectable cadence presets (interval + asymmetric jitter), C5 #24. Each
    // applies a concrete schedule and the active one is highlighted; the frozen
    // default is the 8-to-10 day "Weekly" preset.
    let pin_btn = button(text("Pin (disable)"))
        .on_press_maybe(
            (!busy && enabled).then_some(Message::StudioSetRotation(RotationSchedule::Disabled)),
        )
        .padding(6)
        .style(if enabled {
            button::secondary
        } else {
            button::primary
        });

    container(
        column![
            text("Rotation").size(16),
            text("Auto-rotate cadence (interval + jitter):").size(11),
            row![
                cadence_button("Weekly", RotationSchedule::frozen_cadence(), current, busy),
                cadence_button(
                    "Biweekly",
                    RotationSchedule::cadence(14, 2, 6),
                    current,
                    busy
                ),
                cadence_button(
                    "Monthly",
                    RotationSchedule::cadence(30, 3, 9),
                    current,
                    busy
                ),
            ]
            .spacing(8),
            pin_btn,
            text(window).size(11),
        ]
        .spacing(8),
    )
    .padding(12)
    .width(Length::Fill)
    .style(crate::style::panel)
    .into()
}

/// The #25 P2 coherence-linter panel: findings styled by severity.
fn linter_panel(findings: &[Finding]) -> Element<'_, Message> {
    let mut col = column![text("Coherence linter").size(16)].spacing(6);
    if findings.is_empty() {
        col = col.push(text("No coherence issues found.").size(12));
    } else {
        for finding in findings {
            let tag = match finding.severity {
                Severity::HardImplausible => "IMPLAUSIBLE",
                Severity::Warning => "WARNING",
                // Severity is #[non_exhaustive]; render any future tier plainly.
                _ => "NOTE",
            };
            let tag_style = move |t: &iced::Theme| {
                let color = match finding.severity {
                    Severity::HardImplausible => crate::style::danger_color(t),
                    Severity::Warning => crate::style::warning_color(t),
                    // Severity is #[non_exhaustive]; render any future tier plainly.
                    _ => crate::style::text_color(t),
                };
                crate::style::text_in(color)
            };
            col = col.push(
                column![
                    text(tag).size(11).style(tag_style),
                    text(finding.reason.clone()).size(12),
                    text(format!("fields: {}", finding.fields.join(", "))).size(10),
                ]
                .spacing(2),
            );
        }
    }

    container(col)
        .padding(12)
        .width(Length::Fill)
        .style(crate::style::panel)
        .into()
}

/// The #26 P3 week-simulator preview panel: a per-day activity bar chart plus a
/// re-roll seed button.
fn simulator_panel(week: &SimulatedWeek, seed: u64, busy: bool) -> Element<'_, Message> {
    let days: Vec<DayBar> = week
        .sessions
        .iter()
        .map(|s| DayBar {
            label: format!("D{}", s.day + 1),
            count: s.queries.len() as u32,
        })
        .collect();

    let chart = canvas(WeekTimeline::new(days))
        .width(Length::Fill)
        .height(Length::Fixed(140.0));

    let reroll = button(text("Re-roll"))
        .on_press_maybe((!busy).then_some(Message::StudioRerollWeek))
        .padding(6);

    let header = row![
        text("Week preview").size(16),
        Space::new().width(Length::Fill),
        text(format!("seed {seed} - {} queries", week.total_queries())).size(11),
        reroll,
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center);

    // The #26 P3 per-category breakdown: which interests the simulated week
    // leaned on, highest first.
    let mut breakdown = column![text("By category").size(13)].spacing(2);
    let counts = week.category_counts();
    if counts.is_empty() {
        breakdown = breakdown.push(text("No queries in this week.").size(11));
    } else {
        for (category, count) in counts {
            breakdown = breakdown.push(
                row![
                    text(category).size(11).width(Length::FillPortion(3)),
                    text(count.to_string())
                        .size(11)
                        .width(Length::FillPortion(1)),
                ]
                .spacing(6),
            );
        }
    }

    container(column![header, chart, breakdown].spacing(8))
        .padding(12)
        .width(Length::Fill)
        .style(crate::style::panel)
        .into()
}

/// Shorten a base64 signer key for the compact library list.
fn short_key(key: &str) -> String {
    let head: String = key.chars().take(12).collect();
    format!("signer {head}\u{2026}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use fauxx_core::{desktop_for, mobile_for, simulate_week, IntensityLevel, PersonaSettings};

    /// A day past the #47 Chrome-version baseline so the derived UA carries a
    /// resolved, non-baseline major, exercising the substitution end to end.
    const CREATED_AT: i64 = 1_768_262_400_000 + 40 * 24 * 60 * 60 * 1000;

    fn persona() -> SyntheticPersona {
        SyntheticPersona::new(
            "11111111-1111-4111-8111-111111111111".to_string(),
            "Test Persona".to_string(),
            "AGE_35_44".to_string(),
            "ENGINEER".to_string(),
            "US_MIDWEST".to_string(),
            vec!["TECHNOLOGY".to_string(), "FINANCE".to_string()],
            CREATED_AT,
            CREATED_AT + 7 * 24 * 60 * 60 * 1000,
        )
    }

    fn detail() -> PersonaDetail {
        let persona = persona();
        PersonaDetail {
            desktop_device: desktop_for(&persona),
            mobile_device: mobile_for(&persona),
            settings: PersonaSettings::default_for(&persona.id),
            findings: Vec::new(),
            week: simulate_week(&persona, IntensityLevel::Medium, 7),
            seed: 7,
            persona,
        }
    }

    // iced has no headless renderer to assert pixels; these guard the
    // view-construction logic against panics (mirrors the privacy smoke test).
    #[test]
    fn studio_renders_with_a_populated_detail_including_the_device_panel() {
        let d = detail();
        let snapshot = StudioSnapshot {
            personas: vec![d.persona.clone()],
            installed_packs: Vec::new(),
            detail: Some(d),
        };
        let _ = view(Some(&snapshot), false);
        // The no-detail and loading states must render too.
        let empty = StudioSnapshot {
            personas: Vec::new(),
            installed_packs: Vec::new(),
            detail: None,
        };
        let _ = view(Some(&empty), false);
        let _ = view(None, true);
    }

    #[test]
    fn device_panel_renders_desktop_and_paired_phone() {
        let p = persona();
        let _ = device_panel(&desktop_for(&p), &mobile_for(&p));
    }

    #[test]
    fn brands_summary_joins_name_and_version_per_brand() {
        let desktop = desktop_for(&persona());
        let summary = brands_summary(&desktop);
        // Every brand appears as "<name> <version>", comma-separated, in order.
        let expected = desktop
            .brands
            .iter()
            .map(|b| format!("{} {}", b.name, b.version))
            .collect::<Vec<_>>()
            .join(", ");
        assert_eq!(summary, expected);
        assert!(summary.contains("Google Chrome "));
        // The panel surfaces the real device, never a headless tell.
        assert!(!summary.contains("Headless"));
    }
}
