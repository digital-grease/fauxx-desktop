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

//! Efficacy-snapshot export to CSV / JSON / PDF (C4 #23, A4).
//!
//! Exports the A1 efficacy data (the per-platform KL-divergence drift
//! time-series and the per-category drift, from [`crate::measurement`]) to three
//! formats, each embedding the AS-OF date:
//!
//! - CSV ([`csv`] crate): the underlying time-series + per-category drift rows.
//! - JSON ([`serde_json`]): the same underlying data, structured.
//! - PDF ([`printpdf`], BUILT-IN Helvetica, no bundled TTF): a human-readable
//!   dated snapshot. A title, the as-of date, and PER PLATFORM a dated summary
//!   line plus a VECTOR drift chart (a line chart of the KL-divergence timeline,
//!   drawn with PDF path primitives, not a raster image) and the top per-category
//!   movers. No bundled font or image: the chart is vector lines.
//!
//! ## The signing seam (deliberately not implemented here)
//!
//! Every export produces an in-memory [`ExportArtifact`]: the serialized
//! `bytes` plus typed [`ExportMetadata`] (format, content type, as-of date,
//! suggested filename). PRODUCING the artifact and WRITING it
//! ([`ExportArtifact::write_to`]) are separate steps. That split is the clean
//! seam a FUTURE ed25519 signing layer slots into: it can take the produced
//! artifact, sign `bytes`, and wrap the output without reworking the export
//! pipeline. No signing, hashing, or timestamping is done now.
//!
//! Export runs entirely in the core and is reachable headless. There is NO GUI
//! or CLI type here; the bytes are returned for a client to render or save.

use std::path::Path;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::error::{CoreError, Result};
use crate::measurement::platform::{HeatmapSeries, PlatformDrift};

/// How many top per-category movers the PDF summary lists per platform.
const PDF_TOP_MOVERS: usize = 5;

/// One exportable efficacy snapshot's underlying DATA (C4 #23): the per-platform
/// drift bundles plus the as-of timestamp the export is dated to.
///
/// This is the single structure CSV, JSON, and the PDF summary are all derived
/// from, so every format describes the same numbers. It is plain serializable
/// data; the JSON export is essentially this verbatim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EfficacySnapshotData {
    /// The persona this snapshot describes (free label; the export does not
    /// resolve it against the store).
    pub persona_id: String,
    /// Epoch milliseconds the export is dated AS OF. Every format embeds this.
    pub as_of_millis: i64,
    /// The per-platform A1 drift bundles (scalar KL timeline + per-category
    /// heatmap), in the order supplied (the dashboard's display order).
    pub platforms: Vec<PlatformDrift>,
}

impl EfficacySnapshotData {
    /// Bundle the per-platform drift series for `persona_id` as of
    /// `as_of_millis`.
    pub fn new(persona_id: &str, as_of_millis: i64, platforms: Vec<PlatformDrift>) -> Self {
        Self {
            persona_id: persona_id.to_string(),
            as_of_millis,
            platforms,
        }
    }

    /// The as-of date formatted as an ISO `YYYY-MM-DD` string (UTC). A
    /// timestamp outside the representable range falls back to the raw epoch
    /// millis as a string, so formatting never fails the export.
    pub fn as_of_date(&self) -> String {
        format_as_of_date(self.as_of_millis)
    }
}

/// The wire/output format an efficacy snapshot is exported to (C4 #23).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    /// Comma-separated rows: the underlying time-series + per-category drift.
    Csv,
    /// Structured JSON: the same underlying data.
    Json,
    /// A human-readable, dated PDF summary (built-in Helvetica).
    Pdf,
}

impl ExportFormat {
    /// The lowercase file extension for this format.
    pub fn extension(&self) -> &'static str {
        match self {
            ExportFormat::Csv => "csv",
            ExportFormat::Json => "json",
            ExportFormat::Pdf => "pdf",
        }
    }

    /// The MIME content type for this format.
    pub fn content_type(&self) -> &'static str {
        match self {
            ExportFormat::Csv => "text/csv",
            ExportFormat::Json => "application/json",
            ExportFormat::Pdf => "application/pdf",
        }
    }
}

/// Typed metadata describing a produced [`ExportArtifact`] (C4 #23).
///
/// Carried ALONGSIDE the bytes so a future signing layer can record what it
/// signed (format, content type, as-of date) without re-parsing the payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportMetadata {
    /// The format the bytes are in.
    pub format: ExportFormat,
    /// The MIME content type of the bytes.
    pub content_type: String,
    /// The persona id the snapshot describes.
    pub persona_id: String,
    /// Epoch milliseconds the export is dated as of.
    pub as_of_millis: i64,
    /// The as-of date as an ISO `YYYY-MM-DD` string (embedded in every format).
    pub as_of_date: String,
    /// A suggested file name (no directory), e.g.
    /// `fauxx-efficacy-<persona>-<date>.csv`.
    pub suggested_filename: String,
}

/// An in-memory export artifact (C4 #23): the serialized `bytes` plus typed
/// `metadata`.
///
/// THE SIGNING SEAM. Export is a two-step pipeline by design: produce this
/// artifact, then separately [`write_to`](Self::write_to) it (or hand the bytes
/// to a client). A future ed25519 signing step wraps this artifact, signs
/// [`bytes`](Self::bytes), and emits a signed output without touching the
/// producers. Nothing is signed, hashed, or timestamped here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportArtifact {
    /// The serialized export payload (CSV text, JSON text, or PDF binary).
    pub bytes: Vec<u8>,
    /// Typed metadata describing the payload.
    pub metadata: ExportMetadata,
}

impl ExportArtifact {
    /// The payload size in bytes.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Whether the payload is empty (it never is for a real export).
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Write the artifact's bytes to `path`. The SEPARATE write step of the
    /// pipeline; a future signing layer can interpose before this is called.
    pub fn write_to(&self, path: impl AsRef<Path>) -> Result<()> {
        std::fs::write(path, &self.bytes)?;
        Ok(())
    }
}

/// Produce an export artifact for `data` in `format` (C4 #23). The single entry
/// point; it dispatches to the per-format producer and attaches the metadata.
pub fn export_efficacy_snapshot(
    data: &EfficacySnapshotData,
    format: ExportFormat,
) -> Result<ExportArtifact> {
    let bytes = match format {
        ExportFormat::Csv => to_csv_bytes(data)?,
        ExportFormat::Json => to_json_bytes(data)?,
        ExportFormat::Pdf => to_pdf_bytes(data)?,
    };
    let metadata = ExportMetadata {
        format,
        content_type: format.content_type().to_string(),
        persona_id: data.persona_id.clone(),
        as_of_millis: data.as_of_millis,
        as_of_date: data.as_of_date(),
        suggested_filename: format!(
            "fauxx-efficacy-{}-{}.{}",
            sanitize(&data.persona_id),
            data.as_of_date(),
            format.extension()
        ),
    };
    Ok(ExportArtifact { bytes, metadata })
}

// --- CSV ---------------------------------------------------------------------

/// The header row the CSV export emits. Frozen so downstream tooling can rely
/// on it. `kind` distinguishes a scalar timeline row from a per-category row.
const CSV_HEADER: [&str; 6] = [
    "as_of_date",
    "platform",
    "timestamp",
    "kind",
    "category",
    "value",
];

/// Serialize the underlying time-series + per-category drift to CSV bytes.
///
/// Two row KINDS share one table so the file is self-describing:
///
/// - `kind = "drift"`: one row per scalar timeline point, `category` empty,
///   `value` the KL divergence at that timestamp.
/// - `kind = "category"`: one row per `(category, timestamp)` heatmap cell with
///   a non-zero contribution, `value` that category's contribution.
///
/// Every row carries the as-of date so a single row is self-dating.
fn to_csv_bytes(data: &EfficacySnapshotData) -> Result<Vec<u8>> {
    let as_of = data.as_of_date();
    let mut wtr = csv::Writer::from_writer(Vec::new());
    wtr.write_record(CSV_HEADER)
        .map_err(|e| CoreError::Key(format!("csv header write failed: {e}")))?;

    for bundle in &data.platforms {
        let platform = bundle.series.platform.label();
        // Scalar drift timeline rows.
        for point in &bundle.series.points {
            wtr.write_record([
                as_of.as_str(),
                platform.as_str(),
                &point.timestamp.to_string(),
                "drift",
                "",
                &format_value(point.divergence),
            ])
            .map_err(|e| CoreError::Key(format!("csv drift row write failed: {e}")))?;
        }
        // Per-category drift rows (only non-zero contributions, for compactness).
        for (category, values) in &bundle.heatmap.rows {
            for (idx, value) in values.iter().enumerate() {
                if *value == 0.0 {
                    continue;
                }
                let timestamp = bundle.heatmap.timestamps.get(idx).copied().unwrap_or(0);
                wtr.write_record([
                    as_of.as_str(),
                    platform.as_str(),
                    &timestamp.to_string(),
                    "category",
                    category.as_str(),
                    &format_value(*value),
                ])
                .map_err(|e| CoreError::Key(format!("csv category row write failed: {e}")))?;
            }
        }
    }

    wtr.flush()?;
    wtr.into_inner()
        .map_err(|e| CoreError::Key(format!("csv finalize failed: {e}")))
}

// --- JSON --------------------------------------------------------------------

/// Serialize the same underlying data as pretty JSON. Round-trips back into
/// [`EfficacySnapshotData`].
fn to_json_bytes(data: &EfficacySnapshotData) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec_pretty(data)?)
}

// --- PDF ---------------------------------------------------------------------

/// Render a human-readable, dated PDF: a title, the as-of date, and PER PLATFORM
/// a summary line, a VECTOR drift line-chart (PDF path primitives, no raster
/// image), and the top per-category movers. Uses printpdf's BUILT-IN Helvetica
/// (no bundled TTF). Coordinates are millimetres from the bottom-left origin
/// (printpdf's convention), so a larger `y` is higher on the page.
fn to_pdf_bytes(data: &EfficacySnapshotData) -> Result<Vec<u8>> {
    use printpdf::{BuiltinFont, Mm, Op, PdfDocument, PdfPage, PdfSaveOptions};

    // A4 portrait. Layout band geometry (mm).
    let page_w = Mm(210.0);
    let page_h = Mm(297.0);
    const LEFT: f32 = 20.0;
    const RIGHT: f32 = 190.0;
    const CHART_H: f32 = 28.0;
    const LINE_MM: f32 = 5.0;
    const BOTTOM_MARGIN: f32 = 20.0;

    let mut ops: Vec<Op> = Vec::new();

    // Header: title, the embedded as-of date (per the requirement), and persona.
    ops.extend(text_at(
        LEFT,
        285.0,
        &[
            (
                BuiltinFont::HelveticaBold,
                20.0,
                "Fauxx Efficacy Snapshot".to_string(),
            ),
            (
                BuiltinFont::Helvetica,
                10.0,
                format!("As of: {}", data.as_of_date()),
            ),
            (
                BuiltinFont::Helvetica,
                10.0,
                format!("Persona: {}", data.persona_id),
            ),
        ],
    ));
    let mut y = 285.0 - 3.0 * LINE_MM - 6.0;

    for bundle in &data.platforms {
        // Stop if we have run out of vertical room (rare: the platform set is
        // small). The remaining platforms are still in the CSV/JSON exports.
        if y < BOTTOM_MARGIN + CHART_H {
            break;
        }
        let platform = bundle.series.platform.label();
        let summary = if bundle.series.is_empty() {
            "No data recorded yet for this platform.".to_string()
        } else {
            format!(
                "Snapshots: {}    Latest drift: {}",
                bundle.series.points.len(),
                format_value(bundle.series.latest().unwrap_or(0.0)),
            )
        };
        ops.extend(text_at(
            LEFT,
            y,
            &[
                (
                    BuiltinFont::HelveticaBold,
                    12.0,
                    format!("Platform: {platform}"),
                ),
                (BuiltinFont::Helvetica, 9.0, summary),
            ],
        ));
        y -= 2.0 * LINE_MM + 3.0;

        if !bundle.series.is_empty() {
            // The vector drift chart in a box below the label.
            let chart_top = y;
            let chart_bottom = (chart_top - CHART_H).max(BOTTOM_MARGIN);
            ops.extend(drift_chart_ops(
                bundle,
                LEFT,
                RIGHT,
                chart_bottom,
                chart_top,
            ));
            y = chart_bottom - 4.0;

            // Top per-category movers as one compact italic line.
            let movers = top_movers(&bundle.heatmap, PDF_TOP_MOVERS);
            if !movers.is_empty() {
                let listed = movers
                    .iter()
                    .map(|(category, value)| format!("{category} {}", format_value(*value)))
                    .collect::<Vec<_>>()
                    .join("    ");
                ops.extend(text_at(
                    LEFT,
                    y,
                    &[(
                        BuiltinFont::HelveticaOblique,
                        8.0,
                        format!("Top movers (latest): {listed}"),
                    )],
                ));
                y -= LINE_MM;
            }
        }
        y -= 8.0; // gap before the next platform band
    }

    let page = PdfPage::new(page_w, page_h, ops);
    let bytes = PdfDocument::new("Fauxx Efficacy Snapshot")
        .with_pages(vec![page])
        .save(&PdfSaveOptions::default(), &mut Vec::new());
    Ok(bytes)
}

/// Emit a self-contained text block at the absolute position `(x_mm, y_mm)`
/// (the cursor for the first line; later lines drop one line height each). Each
/// `(font, size_pt, text)` is one line. Used for the PDF header and per-platform
/// labels so charts and text can be interleaved at known positions.
fn text_at(
    x_mm: f32,
    y_mm: f32,
    lines: &[(printpdf::BuiltinFont, f32, String)],
) -> Vec<printpdf::Op> {
    use printpdf::{Color, Mm, Op, PdfFontHandle, Point, Pt, Rgb, TextItem};
    let black = Color::Rgb(Rgb {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        icc_profile: None,
    });
    let mut ops = vec![
        Op::StartTextSection,
        Op::SetTextCursor {
            pos: Point::new(Mm(x_mm), Mm(y_mm)),
        },
        Op::SetLineHeight { lh: Pt(14.0) },
        Op::SetFillColor { col: black },
    ];
    for (i, (font, size, text)) in lines.iter().enumerate() {
        ops.push(Op::SetFont {
            font: PdfFontHandle::Builtin(*font),
            size: Pt(*size),
        });
        ops.push(Op::ShowText {
            items: vec![TextItem::Text(text.clone())],
        });
        if i + 1 < lines.len() {
            ops.push(Op::AddLineBreak);
        }
    }
    ops.push(Op::EndTextSection);
    ops
}

/// The PDF graphics ops for one platform's drift line-chart, drawn in the box
/// `[x_left, x_right] x [y_bottom, y_top]` (mm): grey L-shaped axes plus a blue
/// polyline of the KL-divergence timeline (scaled to the box by the series max).
/// Returns an EMPTY vec when the series has no data (the caller shows a "no data"
/// label instead). These are path ops, so the caller emits them OUTSIDE any text
/// section.
fn drift_chart_ops(
    bundle: &PlatformDrift,
    x_left: f32,
    x_right: f32,
    y_bottom: f32,
    y_top: f32,
) -> Vec<printpdf::Op> {
    use printpdf::{Color, Line, LinePoint, Mm, Op, Point, Pt, Rgb};

    let points = &bundle.series.points;
    if points.is_empty() {
        return Vec::new();
    }
    let axis_color = Color::Rgb(Rgb {
        r: 0.5,
        g: 0.5,
        b: 0.5,
        icc_profile: None,
    });
    let series_color = Color::Rgb(Rgb {
        r: 0.10,
        g: 0.30,
        b: 0.80,
        icc_profile: None,
    });
    let mut ops = Vec::new();

    // Axes: y-axis up the left, x-axis along the bottom (one open L-shaped line).
    ops.push(Op::SetOutlineColor { col: axis_color });
    ops.push(Op::SetOutlineThickness { pt: Pt(0.75) });
    ops.push(Op::DrawLine {
        line: Line {
            points: vec![
                LinePoint {
                    p: Point::new(Mm(x_left), Mm(y_top)),
                    bezier: false,
                },
                LinePoint {
                    p: Point::new(Mm(x_left), Mm(y_bottom)),
                    bezier: false,
                },
                LinePoint {
                    p: Point::new(Mm(x_right), Mm(y_bottom)),
                    bezier: false,
                },
            ],
            is_closed: false,
        },
    });

    // Drift polyline (needs >= 2 points to form a line). KL divergence is >= 0;
    // clamp defensively and scale to the series max (with an epsilon floor so an
    // all-zero series draws a flat baseline rather than dividing by zero).
    if points.len() >= 2 {
        let width = (x_right - x_left).max(1.0);
        let height = (y_top - y_bottom).max(1.0);
        let max = points
            .iter()
            .map(|p| p.divergence.max(0.0))
            .fold(0.0_f64, f64::max)
            .max(1e-9);
        let last = points.len() - 1;
        let series: Vec<LinePoint> = points
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let x = x_left + (i as f32 / last as f32) * width;
                let frac = (p.divergence.max(0.0) / max) as f32;
                LinePoint {
                    p: Point::new(Mm(x), Mm(y_bottom + frac * height)),
                    bezier: false,
                }
            })
            .collect();
        ops.push(Op::SetOutlineColor { col: series_color });
        ops.push(Op::SetOutlineThickness { pt: Pt(1.2) });
        ops.push(Op::DrawLine {
            line: Line {
                points: series,
                is_closed: false,
            },
        });
    }
    ops
}

/// The top `n` per-category movers from a heatmap, by absolute contribution at
/// the LATEST timestamp, largest first. Empty when the heatmap has no data.
fn top_movers(heatmap: &HeatmapSeries, n: usize) -> Vec<(String, f64)> {
    if heatmap.is_empty() {
        return Vec::new();
    }
    let last_idx = heatmap.timestamps.len().saturating_sub(1);
    let mut movers: Vec<(String, f64)> = heatmap
        .rows
        .iter()
        .filter_map(|(category, values)| {
            values.get(last_idx).and_then(|v| {
                if *v == 0.0 {
                    None
                } else {
                    Some((category.clone(), *v))
                }
            })
        })
        .collect();
    // Largest absolute contribution first; ties broken by category for stability.
    movers.sort_by(|a, b| {
        b.1.abs()
            .partial_cmp(&a.1.abs())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    movers.truncate(n);
    movers
}

// --- shared helpers ----------------------------------------------------------

/// Format the as-of date as ISO `YYYY-MM-DD` (UTC). Falls back to the raw epoch
/// millis on an out-of-range timestamp so the export never fails on the date.
fn format_as_of_date(as_of_millis: i64) -> String {
    let seconds = as_of_millis.div_euclid(1000);
    match OffsetDateTime::from_unix_timestamp(seconds) {
        Ok(dt) => {
            // `[year]-[month]-[day]`, zero-padded, via the compile-time format.
            let fmt = time::macros::format_description!("[year]-[month]-[day]");
            dt.format(fmt).unwrap_or_else(|_| as_of_millis.to_string())
        }
        Err(_) => as_of_millis.to_string(),
    }
}

/// Format an `f64` drift/contribution value with a stable, compact precision so
/// CSV and PDF agree and the output is deterministic. Non-finite values (which
/// the metric never produces) render as `0`.
fn format_value(v: f64) -> String {
    if v.is_finite() {
        format!("{v:.6}")
    } else {
        "0".to_string()
    }
}

/// Sanitize a string for use in a file name: keep ASCII alphanumerics, `-`, and
/// `_`; replace anything else with `_`. Empty input becomes `unknown`.
fn sanitize(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "unknown".to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measurement::distribution::CategoryDistribution;
    use crate::measurement::platform::{build_platform_drift, Baseline, Platform};
    use crate::measurement::Smoothing;

    /// A small, known two-point Google series with real drift, plus an empty
    /// Meta series, so the export covers both the data and no-data paths.
    fn sample_data(as_of_millis: i64) -> EfficacySnapshotData {
        let snaps = vec![
            (
                100_i64,
                CategoryDistribution::from_counts([("a", 1.0), ("b", 1.0)]),
            ),
            (
                200_i64,
                CategoryDistribution::from_counts([("a", 1.0), ("c", 1.0)]),
            ),
        ];
        let baseline =
            Baseline::Explicit(CategoryDistribution::from_counts([("a", 1.0), ("b", 1.0)]));
        let google = build_platform_drift(Platform::Google, &snaps, &baseline, Smoothing::new());
        let meta = PlatformDrift::empty(Platform::Meta);
        EfficacySnapshotData::new("persona-1", as_of_millis, vec![google, meta])
    }

    // 2021-01-01T00:00:00Z in epoch millis.
    const AS_OF: i64 = 1_609_459_200_000;

    #[test]
    fn as_of_date_formats_iso() {
        let data = sample_data(AS_OF);
        assert_eq!(data.as_of_date(), "2021-01-01");
    }

    #[test]
    fn csv_export_has_header_and_rows() -> Result<()> {
        let data = sample_data(AS_OF);
        let artifact = export_efficacy_snapshot(&data, ExportFormat::Csv)?;
        assert_eq!(artifact.metadata.format, ExportFormat::Csv);
        assert_eq!(artifact.metadata.content_type, "text/csv");
        let text = String::from_utf8(artifact.bytes.clone())
            .map_err(|e| CoreError::Key(format!("csv not utf8: {e}")))?;
        let mut lines = text.lines();
        // The frozen header.
        assert_eq!(
            lines.next(),
            Some("as_of_date,platform,timestamp,kind,category,value")
        );
        // At least the two Google drift rows are present.
        let drift_rows: Vec<&str> = text
            .lines()
            .filter(|l| l.contains(",Google,") && l.contains(",drift,"))
            .collect();
        assert_eq!(drift_rows.len(), 2);
        // Every data row embeds the as-of date.
        for line in text.lines().skip(1) {
            assert!(line.starts_with("2021-01-01,"), "row missing date: {line}");
        }
        // Per-category rows exist for the drifting categories.
        assert!(text.contains(",Google,200,category,"));
        Ok(())
    }

    #[test]
    fn json_export_round_trips() -> Result<()> {
        let data = sample_data(AS_OF);
        let artifact = export_efficacy_snapshot(&data, ExportFormat::Json)?;
        assert_eq!(artifact.metadata.content_type, "application/json");
        let back: EfficacySnapshotData = serde_json::from_slice(&artifact.bytes)?;
        assert_eq!(back, data);
        Ok(())
    }

    #[test]
    fn pdf_export_is_valid_nonempty_and_dated() -> Result<()> {
        let data = sample_data(AS_OF);
        let artifact = export_efficacy_snapshot(&data, ExportFormat::Pdf)?;
        assert_eq!(artifact.metadata.content_type, "application/pdf");
        assert!(!artifact.is_empty());
        // A valid PDF starts with the "%PDF" magic.
        assert!(
            artifact.bytes.starts_with(b"%PDF"),
            "PDF must start with %PDF magic"
        );
        // The as-of date is embedded in the metadata (and rendered in-document).
        assert_eq!(artifact.metadata.as_of_date, "2021-01-01");
        assert!(artifact.metadata.suggested_filename.ends_with(".pdf"));
        assert!(artifact.metadata.suggested_filename.contains("2021-01-01"));
        // A snapshot WITH a drifting series renders its vector chart(s), so it is
        // larger than an empty (charts-less) snapshot of the same shape.
        let empty = export_efficacy_snapshot(
            &EfficacySnapshotData::new("persona-1", AS_OF, Vec::new()),
            ExportFormat::Pdf,
        )?;
        assert!(
            artifact.len() > empty.len(),
            "a data snapshot with charts must exceed an empty one ({} vs {})",
            artifact.len(),
            empty.len()
        );
        Ok(())
    }

    #[test]
    fn drift_chart_emits_axes_and_series_lines() {
        use printpdf::Op;
        let data = sample_data(AS_OF);
        // Google has a real two-point series -> axes line + drift polyline.
        let google = &data.platforms[0];
        let ops = drift_chart_ops(google, 20.0, 190.0, 100.0, 128.0);
        let drawn = ops
            .iter()
            .filter(|op| matches!(op, Op::DrawLine { .. }))
            .count();
        assert_eq!(
            drawn, 2,
            "a data series draws the axes plus the drift polyline"
        );
        // The empty Meta platform draws no chart (the label shows "no data").
        let meta = &data.platforms[1];
        assert!(
            drift_chart_ops(meta, 20.0, 190.0, 100.0, 128.0).is_empty(),
            "an empty series renders no chart"
        );
    }

    #[test]
    fn artifact_write_to_persists_bytes() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let data = sample_data(AS_OF);
        let artifact = export_efficacy_snapshot(&data, ExportFormat::Json)?;
        let path = dir.path().join("snap.json");
        artifact.write_to(&path)?;
        let read_back = std::fs::read(&path)?;
        assert_eq!(read_back, artifact.bytes);
        Ok(())
    }

    #[test]
    fn signing_seam_exposes_structured_artifact_not_just_a_write() -> Result<()> {
        // The pipeline produces a structured artifact (bytes + typed metadata)
        // BEFORE any write, which is exactly what a future signing layer wraps.
        let data = sample_data(AS_OF);
        let artifact = export_efficacy_snapshot(&data, ExportFormat::Csv)?;
        // A future signer can read what it is about to sign without re-parsing.
        assert_eq!(artifact.metadata.persona_id, "persona-1");
        assert_eq!(artifact.metadata.as_of_millis, AS_OF);
        assert!(!artifact.bytes.is_empty());
        assert_eq!(artifact.len(), artifact.bytes.len());
        Ok(())
    }

    #[test]
    fn export_format_extensions_and_types() {
        assert_eq!(ExportFormat::Csv.extension(), "csv");
        assert_eq!(ExportFormat::Json.extension(), "json");
        assert_eq!(ExportFormat::Pdf.extension(), "pdf");
        assert_eq!(ExportFormat::Pdf.content_type(), "application/pdf");
    }

    #[test]
    fn empty_data_still_exports_each_format() -> Result<()> {
        let data = EfficacySnapshotData::new("p", AS_OF, Vec::new());
        for format in [ExportFormat::Csv, ExportFormat::Json, ExportFormat::Pdf] {
            let artifact = export_efficacy_snapshot(&data, format)?;
            assert!(!artifact.is_empty(), "{format:?} produced no bytes");
        }
        Ok(())
    }

    #[test]
    fn out_of_range_date_falls_back_without_failing() {
        // An absurd timestamp must not fail the export; the date falls back.
        let data = EfficacySnapshotData::new("p", i64::MAX, Vec::new());
        let date = data.as_of_date();
        assert!(!date.is_empty());
    }

    #[test]
    fn sanitize_cleans_filename_component() {
        assert_eq!(sanitize("abc-123_x"), "abc-123_x");
        assert_eq!(sanitize("a b/c"), "a_b_c");
        assert_eq!(sanitize(""), "unknown");
    }
}
