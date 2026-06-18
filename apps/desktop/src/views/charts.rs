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

//! iced 0.14 `Canvas` chart programs shared by the dashboard and the studio.
//!
//! These are PURE rendering of already-loaded, owned data: a multi-series line
//! chart for the per-platform KL-divergence timelines (C4 #20 A1), a category
//! heatmap for the per-category drift contribution, and a week-activity
//! timeline for the studio's #26 P3 preview. No `Program` here calls the core
//! or emits a `Message`; each takes a tiny owned model and draws it. The
//! plotters-iced crate is incompatible with iced 0.14, so this draws directly
//! on `Frame` primitives, which is sufficient for these compact panels.

use iced::mouse;
use iced::widget::canvas::{self, Frame, Geometry, Path, Stroke, Text};
use iced::{Color, Point, Rectangle, Renderer, Size, Theme};

/// The axis / gridline color (muted grey).
const AXIS: Color = Color {
    r: 0.55,
    g: 0.55,
    b: 0.60,
    a: 1.0,
};

/// The label color (dark grey, legible on the light panels).
const LABEL: Color = Color {
    r: 0.10,
    g: 0.10,
    b: 0.12,
    a: 1.0,
};

/// The fixed, deterministic palette for line series (Google, Brokers, Meta,
/// then wraps). Indexing wraps so an extra platform never panics.
const SERIES_COLORS: [Color; 4] = [
    Color {
        r: 0.16,
        g: 0.50,
        b: 0.73,
        a: 1.0,
    }, // blue (Google)
    Color {
        r: 0.85,
        g: 0.37,
        b: 0.01,
        a: 1.0,
    }, // orange (Brokers)
    Color {
        r: 0.17,
        g: 0.63,
        b: 0.17,
        a: 1.0,
    }, // green (Meta)
    Color {
        r: 0.58,
        g: 0.40,
        b: 0.74,
        a: 1.0,
    }, // purple (overflow)
];

/// Pick a series color by index, wrapping so any count is safe.
pub fn series_color(index: usize) -> Color {
    SERIES_COLORS[index % SERIES_COLORS.len()]
}

/// One named line series for the [`LineChart`]: an x/y point list already in
/// draw order plus a display color.
#[derive(Clone, Debug)]
pub struct LineSeries {
    /// The series label (the platform name) for the legend.
    pub label: String,
    /// The `(x, y)` points in draw order. `x` is normalized later against the
    /// chart's shared x-range; `y` is the raw divergence.
    pub points: Vec<(f64, f64)>,
    /// The line color.
    pub color: Color,
}

/// A multi-series line chart (C4 #20 A1 drift timelines). All series share one
/// x-range (their combined min/max) and one y-range (`0..max`), so the platforms
/// are directly comparable. An empty model draws a no-data placeholder.
#[derive(Clone, Debug)]
pub struct LineChart {
    series: Vec<LineSeries>,
}

impl LineChart {
    /// Build a line chart from its series.
    pub fn new(series: Vec<LineSeries>) -> Self {
        Self { series }
    }

    /// Whether there is anything to draw (any series with at least one point).
    fn has_data(&self) -> bool {
        self.series.iter().any(|s| !s.points.is_empty())
    }
}

impl<Message> canvas::Program<Message> for LineChart {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());

        if !self.has_data() {
            draw_no_data(&mut frame, bounds.size());
            return vec![frame.into_geometry()];
        }

        // The plot area inside a margin for axis labels.
        let margin_left = 44.0_f32;
        let margin_bottom = 22.0_f32;
        let margin_top = 8.0_f32;
        let margin_right = 8.0_f32;
        let plot = Rectangle {
            x: margin_left,
            y: margin_top,
            width: (bounds.width - margin_left - margin_right).max(1.0),
            height: (bounds.height - margin_top - margin_bottom).max(1.0),
        };

        // Shared ranges across every series.
        let (x_min, x_max) = x_range(&self.series);
        let y_max = y_max(&self.series).max(f64::MIN_POSITIVE);

        draw_axes(&mut frame, plot, y_max);

        for series in &self.series {
            if series.points.len() < 2 {
                // A single point is drawn as a dot so it does not vanish.
                if let Some((x, y)) = series.points.first() {
                    let p = project(*x, *y, plot, x_min, x_max, y_max);
                    frame.fill(&Path::circle(p, 2.5), series.color);
                }
                continue;
            }
            let path = Path::new(|builder| {
                let mut points = series.points.iter();
                if let Some((x, y)) = points.next() {
                    builder.move_to(project(*x, *y, plot, x_min, x_max, y_max));
                }
                for (x, y) in points {
                    builder.line_to(project(*x, *y, plot, x_min, x_max, y_max));
                }
            });
            frame.stroke(
                &path,
                Stroke::default().with_color(series.color).with_width(2.0),
            );
        }

        draw_legend(&mut frame, &self.series, plot);

        vec![frame.into_geometry()]
    }
}

/// One named heatmap row (a category) and its per-timestamp contributions, all
/// in `[0, 1]` after normalization.
#[derive(Clone, Debug)]
pub struct HeatmapRow {
    /// The category label.
    pub label: String,
    /// The normalized cell values, one per column (time step), in `[0, 1]`.
    pub values: Vec<f32>,
}

/// A per-category drift heatmap (C4 #20 A1). Rows are categories, columns are
/// timestamps; each cell's intensity is its normalized contribution. An empty
/// model draws a no-data placeholder.
#[derive(Clone, Debug)]
pub struct Heatmap {
    rows: Vec<HeatmapRow>,
    columns: usize,
}

impl Heatmap {
    /// Build a heatmap from its rows and the column (timestamp) count.
    pub fn new(rows: Vec<HeatmapRow>, columns: usize) -> Self {
        Self { rows, columns }
    }
}

impl<Message> canvas::Program<Message> for Heatmap {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());

        if self.rows.is_empty() || self.columns == 0 {
            draw_no_data(&mut frame, bounds.size());
            return vec![frame.into_geometry()];
        }

        let margin_left = 90.0_f32;
        let grid_x = margin_left;
        let grid_w = (bounds.width - margin_left - 8.0).max(1.0);
        let grid_h = bounds.height.max(1.0);
        let cell_w = grid_w / self.columns as f32;
        let cell_h = grid_h / self.rows.len() as f32;

        for (r, row) in self.rows.iter().enumerate() {
            let y = r as f32 * cell_h;
            // The row label, vertically centered in its band.
            frame.fill_text(Text {
                content: truncate(&row.label, 14),
                position: Point::new(2.0, y + cell_h / 2.0),
                color: LABEL,
                size: 10.0.into(),
                align_y: iced::alignment::Vertical::Center,
                ..Text::default()
            });
            for (c, value) in row.values.iter().enumerate() {
                if c >= self.columns {
                    break;
                }
                let x = grid_x + c as f32 * cell_w;
                let color = heat_color(value.clamp(0.0, 1.0));
                frame.fill_rectangle(
                    Point::new(x, y),
                    Size::new((cell_w - 1.0).max(1.0), (cell_h - 1.0).max(1.0)),
                    color,
                );
            }
        }

        vec![frame.into_geometry()]
    }
}

/// One day's activity count for the [`WeekTimeline`] preview.
#[derive(Clone, Debug)]
pub struct DayBar {
    /// The day label (e.g. `"D1"`).
    pub label: String,
    /// The number of queries that day.
    pub count: u32,
}

/// A simple per-day bar chart of the simulated week's query volume (C5 #26 P3).
/// An empty week draws a no-data placeholder.
#[derive(Clone, Debug)]
pub struct WeekTimeline {
    days: Vec<DayBar>,
}

impl WeekTimeline {
    /// Build the timeline from its per-day bars.
    pub fn new(days: Vec<DayBar>) -> Self {
        Self { days }
    }
}

impl<Message> canvas::Program<Message> for WeekTimeline {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());

        let max = self.days.iter().map(|d| d.count).max().unwrap_or(0);
        if self.days.is_empty() || max == 0 {
            draw_no_data(&mut frame, bounds.size());
            return vec![frame.into_geometry()];
        }

        let margin_bottom = 16.0_f32;
        let plot_h = (bounds.height - margin_bottom).max(1.0);
        let slot_w = bounds.width / self.days.len() as f32;
        let bar_color = series_color(0);

        for (i, day) in self.days.iter().enumerate() {
            let frac = day.count as f32 / max as f32;
            let bar_h = (frac * plot_h).max(1.0);
            let x = i as f32 * slot_w + slot_w * 0.2;
            let w = (slot_w * 0.6).max(1.0);
            let y = plot_h - bar_h;
            frame.fill_rectangle(Point::new(x, y), Size::new(w, bar_h), bar_color);
            frame.fill_text(Text {
                content: day.label.clone(),
                position: Point::new(x + w / 2.0, plot_h + 2.0),
                color: LABEL,
                size: 9.0.into(),
                align_x: iced::alignment::Horizontal::Center.into(),
                ..Text::default()
            });
        }

        vec![frame.into_geometry()]
    }
}

// --- shared drawing helpers ------------------------------------------------

/// Project a data `(x, y)` into plot pixel space (y grows downward).
fn project(x: f64, y: f64, plot: Rectangle, x_min: f64, x_max: f64, y_max: f64) -> Point {
    let x_span = (x_max - x_min).max(f64::MIN_POSITIVE);
    let fx = ((x - x_min) / x_span) as f32;
    let fy = (y / y_max) as f32;
    Point::new(plot.x + fx * plot.width, plot.y + (1.0 - fy) * plot.height)
}

/// The combined x-range (min, max) across all series, defaulting to `(0, 1)`.
fn x_range(series: &[LineSeries]) -> (f64, f64) {
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for s in series {
        for (x, _) in &s.points {
            min = min.min(*x);
            max = max.max(*x);
        }
    }
    if !min.is_finite() || !max.is_finite() || (max - min).abs() < f64::EPSILON {
        (0.0, 1.0)
    } else {
        (min, max)
    }
}

/// The maximum y across all series (at least a small positive so the axis is
/// well-formed).
fn y_max(series: &[LineSeries]) -> f64 {
    series
        .iter()
        .flat_map(|s| s.points.iter().map(|(_, y)| *y))
        .fold(0.0_f64, f64::max)
}

/// Draw the plot box plus a `0` and `max` y-axis tick label.
fn draw_axes(frame: &mut Frame, plot: Rectangle, y_max: f64) {
    let box_path = Path::new(|b| {
        b.move_to(Point::new(plot.x, plot.y));
        b.line_to(Point::new(plot.x, plot.y + plot.height));
        b.line_to(Point::new(plot.x + plot.width, plot.y + plot.height));
    });
    frame.stroke(
        &box_path,
        Stroke::default().with_color(AXIS).with_width(1.0),
    );

    frame.fill_text(Text {
        content: format!("{y_max:.2}"),
        position: Point::new(plot.x - 4.0, plot.y),
        color: LABEL,
        size: 10.0.into(),
        align_x: iced::alignment::Horizontal::Right.into(),
        align_y: iced::alignment::Vertical::Top,
        ..Text::default()
    });
    frame.fill_text(Text {
        content: "0".to_string(),
        position: Point::new(plot.x - 4.0, plot.y + plot.height),
        color: LABEL,
        size: 10.0.into(),
        align_x: iced::alignment::Horizontal::Right.into(),
        align_y: iced::alignment::Vertical::Bottom,
        ..Text::default()
    });
}

/// Draw a compact per-series color-keyed legend along the top of the plot.
fn draw_legend(frame: &mut Frame, series: &[LineSeries], plot: Rectangle) {
    let mut x = plot.x + 4.0;
    let y = plot.y + 2.0;
    for s in series {
        if s.points.is_empty() {
            continue;
        }
        frame.fill_rectangle(Point::new(x, y), Size::new(8.0, 8.0), s.color);
        x += 11.0;
        frame.fill_text(Text {
            content: s.label.clone(),
            position: Point::new(x, y),
            color: LABEL,
            size: 10.0.into(),
            align_y: iced::alignment::Vertical::Top,
            ..Text::default()
        });
        x += 8.0 + 7.0 * s.label.len() as f32;
    }
}

/// Centered "no data" placeholder.
fn draw_no_data(frame: &mut Frame, size: Size) {
    frame.fill_text(Text {
        content: "No data yet".to_string(),
        position: Point::new(size.width / 2.0, size.height / 2.0),
        color: AXIS,
        size: 12.0.into(),
        align_x: iced::alignment::Horizontal::Center.into(),
        align_y: iced::alignment::Vertical::Center,
        ..Text::default()
    });
}

/// Map a normalized intensity `[0, 1]` to a white-to-teal heat color.
pub fn heat_color(intensity: f32) -> Color {
    // Linear blend from near-white (low) to the Fauxx teal (high).
    let lo = (0.96, 0.97, 0.98);
    let hi = (0.10, 0.50, 0.47);
    Color {
        r: lo.0 + (hi.0 - lo.0) * intensity,
        g: lo.1 + (hi.1 - lo.1) * intensity,
        b: lo.2 + (hi.2 - lo.2) * intensity,
        a: 1.0,
    }
}

/// Truncate a label to `max` chars with an ellipsis, for tight axis gutters.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let kept: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{kept}\u{2026}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn series_color_is_deterministic_and_cycles() {
        // Stable for a given index, and wraps at the palette length (so any
        // platform index is colourable without panicking).
        assert_eq!(series_color(0), series_color(0));
        assert_eq!(series_color(0), series_color(SERIES_COLORS.len()));
        assert_eq!(series_color(1), series_color(SERIES_COLORS.len() + 1));
        // Total over a large index range, fully opaque.
        for i in 0..1000 {
            assert_eq!(series_color(i).a, 1.0);
        }
    }

    #[test]
    fn heat_color_blends_low_to_high_opaque() {
        // Endpoints match the documented near-white -> teal blend.
        let lo = heat_color(0.0);
        let hi = heat_color(1.0);
        assert!((lo.r - 0.96).abs() < 1e-5 && (lo.g - 0.97).abs() < 1e-5);
        assert!((hi.r - 0.10).abs() < 1e-5 && (hi.b - 0.47).abs() < 1e-5);
        // Higher intensity is darker (red channel decreases); always opaque.
        assert!(hi.r < lo.r);
        assert_eq!(lo.a, 1.0);
        assert_eq!(hi.a, 1.0);
        // The midpoint lies strictly between the endpoints.
        let mid = heat_color(0.5);
        assert!(mid.r < lo.r && mid.r > hi.r);
    }

    #[test]
    fn truncate_adds_an_ellipsis_only_when_over_length() {
        assert_eq!(truncate("hello", 10), "hello"); // under: unchanged
        assert_eq!(truncate("hello", 5), "hello"); // exactly max: unchanged
        assert_eq!(truncate("hello", 3), "he\u{2026}"); // over: 2 chars + ellipsis
        assert_eq!(truncate("", 3), "");
    }
}
