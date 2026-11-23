//! Numbers for a finished layout and the files written from it: report.json,
//! the cut files, the kit checklist (report.pdf) and labels.csv. Shared by the
//! CLI and the desktop app so both write exactly the same files.

use crate::export;
use crate::geom::{BBox, FlattenMode};
use crate::model::{Layout, PlyShape};
use crate::rolls::RollInfo;
use crate::template::Paper;
use crate::verify::{VerifyReport, verify};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub used_length_mm: f64,
    pub roll_area_m2: f64,
    pub parts_area_m2: f64,
    /// Parts area / (used length × roll width).
    pub utilization_of_roll: f64,
    /// Parts area / (used length after the lead trim × usable width).
    pub utilization_of_usable_area: f64,
    pub material_cost: Option<f64>,
    pub plies: usize,
    pub verification: VerifyReport,
}

pub fn summarize(plies: &[PlyShape], layout: &Layout) -> Summary {
    let m = &layout.material;
    let parts_area: f64 = layout.placements.iter().map(|pl| plies[pl.shape_idx].shape.area(0.01)).sum();
    let roll_area = layout.used_length * m.roll_width;
    let usable_area = (layout.used_length - m.lead_trim).max(0.0) * m.usable_width();
    Summary {
        used_length_mm: layout.used_length,
        roll_area_m2: roll_area / 1e6,
        parts_area_m2: parts_area / 1e6,
        utilization_of_roll: if roll_area > 0.0 { parts_area / roll_area } else { 0.0 },
        utilization_of_usable_area: if usable_area > 0.0 { parts_area / usable_area } else { 0.0 },
        material_cost: m.cost_per_m2.map(|c| c * roll_area / 1e6),
        plies: layout.placements.len(),
        verification: verify(plies, layout),
    }
}

/// How the layout was made; written to report.json.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RunInfo {
    pub seed: u64,
    pub solve_seconds: f64,
    /// Stopped by the user before the time limit.
    #[serde(default)]
    pub stopped: bool,
    /// Moves and turns made by hand after nesting.
    #[serde(default)]
    pub edits: u32,
    /// Locked plies kept in place from the previous layout (the rest was nested after them).
    #[serde(default)]
    pub kept: u32,
}

/// Job name, date and roll printed on report.pdf and in labels.csv.
#[derive(Clone, Debug, PartialEq)]
pub struct ReportMeta {
    pub job: String,
    pub date: String,
    /// The roll the job is cut from, if one was picked.
    pub roll: Option<RollInfo>,
}

impl ReportMeta {
    /// Dated now (UTC), no roll.
    pub fn new(job: impl Into<String>) -> Self {
        Self { job: job.into(), date: now_utc(), roll: None }
    }

    pub fn with_roll(self, roll: Option<RollInfo>) -> Self {
        Self { roll, ..self }
    }
}

/// Which optional files to write.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OutputOptions {
    /// Write templates.pdf (1:1, tiled) on this paper.
    pub templates: Option<Paper>,
    /// Write a DXF drawing of every ply at every angle.
    pub ply_drawings: Option<crate::drawings::PlyDrawings>,
}

/// Today as "2026-11-21" (UTC).
pub fn today_utc() -> String {
    now_utc()[..10].to_string()
}

/// Current date and time as "2026-11-21 14:05 UTC".
pub fn now_utc() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let t = secs.rem_euclid(86_400);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02} UTC", t / 3600, t % 3600 / 60)
}

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// One cut ply. `cut_no` is its place in the cutting order, the same number
/// as the `order` column of plies.csv and the number on the report's layout.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CutPly {
    pub cut_no: usize,
    /// Index into `layout.placements`.
    #[serde(skip)]
    pub placement: usize,
    pub label: String,
    pub shape: String,
    pub angle_deg: f64,
    pub copy: u32,
    /// Copies of this shape at this angle in the job.
    pub of: u32,
    pub kit: Option<String>,
    /// Lower-left corner of the ply's bounding box on the roll: x from the
    /// roll start, y from the roll edge at y = 0.
    pub x_mm: f64,
    pub y_mm: f64,
}

/// Every placement in cutting order.
pub fn cut_plies(plies: &[PlyShape], layout: &Layout) -> Vec<CutPly> {
    export::cut_order(plies, layout)
        .into_iter()
        .enumerate()
        .map(|(n, i)| {
            let pl = &layout.placements[i];
            let ply = &plies[pl.shape_idx];
            let v = &ply.variants[pl.variant_idx];
            let b = BBox::of(&export::placed_path(&ply.shape.outer, pl).flatten(0.5, FlattenMode::Inscribed));
            CutPly {
                cut_no: n + 1,
                placement: i,
                label: export::label_text(ply, pl),
                shape: ply.shape.name.clone(),
                angle_deg: v.angle_deg,
                copy: pl.copy,
                of: v.qty,
                kit: v.kit.clone(),
                x_mm: b.min.x,
                y_mm: b.min.y,
            }
        })
        .collect()
}

/// One line of the kit checklist: one shape at one angle.
#[derive(Clone, Debug, PartialEq)]
pub struct ChecklistRow {
    pub label: String,
    pub shape: String,
    pub angle_deg: f64,
    /// Its cut plies, by cut number.
    pub cuts: Vec<CutPly>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct KitGroup {
    /// None = plies without a kit.
    pub kit: Option<String>,
    pub rows: Vec<ChecklistRow>,
}

impl KitGroup {
    pub fn plies(&self) -> usize {
        self.rows.iter().map(|r| r.cuts.len()).sum()
    }
}

/// Plies grouped by kit, for picking. The model has no layup sequence, so kits
/// and rows follow the project's order (shape, then angle); plies without a
/// kit come last.
pub fn checklist(plies: &[PlyShape], layout: &Layout) -> Vec<KitGroup> {
    let mut rows: Vec<((usize, usize), ChecklistRow)> = Vec::new();
    for c in cut_plies(plies, layout) {
        let pl = &layout.placements[c.placement];
        let key = (pl.shape_idx, pl.variant_idx);
        match rows.iter_mut().find(|(k, _)| *k == key) {
            Some((_, r)) => r.cuts.push(c),
            None => rows.push((
                key,
                ChecklistRow { label: c.label.clone(), shape: c.shape.clone(), angle_deg: c.angle_deg, cuts: vec![c] },
            )),
        }
    }
    rows.sort_by_key(|(k, _)| *k);
    let mut groups: Vec<KitGroup> = Vec::new();
    for (_, row) in rows {
        let kit = row.cuts[0].kit.clone();
        match groups.iter_mut().find(|g| g.kit == kit) {
            Some(g) => g.rows.push(row),
            None => groups.push(KitGroup { kit, rows: vec![row] }),
        }
    }
    // Stable sort: kits keep their first-seen order, "no kit" moves to the end.
    groups.sort_by_key(|g| g.kit.is_none());
    groups
}

/// labels.csv: one row per cut ply in cutting order, for label printers.
pub fn write_label_csv(path: &Path, plies: &[PlyShape], layout: &Layout, meta: &ReportMeta) -> Result<()> {
    let mut w = csv::Writer::from_path(path).with_context(|| format!("cannot write {}", path.display()))?;
    w.write_record(["cut_no", "label", "shape", "angle_deg", "copy", "of", "kit", "material", "job", "x_mm", "y_mm", "roll", "batch"])?;
    let (roll, batch) = meta.roll.as_ref().map_or((String::new(), String::new()), |r| (r.id.clone(), r.batch.clone()));
    for c in cut_plies(plies, layout) {
        w.write_record([
            c.cut_no.to_string(),
            c.label,
            c.shape,
            format!("{}", c.angle_deg),
            c.copy.to_string(),
            c.of.to_string(),
            c.kit.unwrap_or_default(),
            layout.material.name.clone(),
            meta.job.clone(),
            format!("{:.1}", c.x_mm),
            format!("{:.1}", c.y_mm),
            roll.clone(),
            batch.clone(),
        ])?;
    }
    w.flush()?;
    Ok(())
}

/// Writes report.json and layout.svg always; layout.dxf, plies.csv,
/// report.pdf, labels.csv and (when asked for) templates.pdf and the ply
/// drawings only when
/// verification passed, removing any of them left from an earlier export.
/// Returns the files written.
#[allow(clippy::too_many_arguments)]
pub fn write_outputs(
    dir: &Path,
    plies: &[PlyShape],
    layout: &Layout,
    summary: &Summary,
    run: &RunInfo,
    warnings: &[String],
    meta: &ReportMeta,
    opts: &OutputOptions,
) -> Result<Vec<PathBuf>> {
    if let Some(p) = &opts.templates {
        p.validate()?;
    }
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let m = &layout.material;
    let mut json = serde_json::to_value(summary)?;
    let obj = json.as_object_mut().expect("summary is an object");
    obj.insert("material".into(), serde_json::to_value(m)?);
    obj.insert("solve_seconds".into(), run.solve_seconds.into());
    obj.insert("seed".into(), run.seed.into());
    obj.insert("stopped_early".into(), run.stopped.into());
    obj.insert("hand_edits".into(), run.edits.into());
    obj.insert("kept_locked".into(), run.kept.into());
    obj.insert("import_warnings".into(), serde_json::to_value(warnings)?);
    if let Some(r) = &meta.roll {
        obj.insert("roll".into(), serde_json::to_value(r)?);
    }
    obj.insert("layout".into(), serde_json::to_value(layout)?);
    let mut written = Vec::new();
    let report = dir.join("report.json");
    std::fs::write(&report, serde_json::to_string_pretty(&json)?).with_context(|| format!("cannot write {}", report.display()))?;
    written.push(report);

    let title = format!(
        "{} — {} plies — used length {:.1} mm — utilisation {:.1}% of roll{}",
        m.name,
        summary.plies,
        layout.used_length,
        100.0 * summary.utilization_of_roll,
        if summary.verification.ok { "" } else { " — FAILED VERIFICATION" }
    );
    let svg = dir.join("layout.svg");
    export::write_svg(&svg, plies, layout, &title)?;
    written.push(svg);
    if summary.verification.ok {
        let dxf = dir.join("layout.dxf");
        export::write_dxf(&dxf, plies, layout)?;
        written.push(dxf);
        let csv = dir.join("plies.csv");
        export::write_ply_csv(&csv, plies, layout)?;
        written.push(csv);
        let pdf = dir.join("report.pdf");
        let bytes = crate::pdf::nest_report(plies, layout, summary, run, warnings, meta);
        std::fs::write(&pdf, bytes).with_context(|| format!("cannot write {}", pdf.display()))?;
        written.push(pdf);
        let labels = dir.join("labels.csv");
        write_label_csv(&labels, plies, layout, meta)?;
        written.push(labels);
        if let Some(paper) = &opts.templates {
            let tpl = dir.join("templates.pdf");
            let (bytes, _) = crate::template::templates_pdf(plies, layout, paper, meta);
            std::fs::write(&tpl, bytes).with_context(|| format!("cannot write {}", tpl.display()))?;
            written.push(tpl);
        }
        if let Some(d) = &opts.ply_drawings {
            written.extend(crate::drawings::write(dir, plies, d)?);
        }
    }
    // A file from an earlier export that was not written this time would
    // describe another layout: remove it, so a folder never mixes two nests.
    let folder = dir.join(crate::drawings::FOLDER);
    if !written.iter().any(|f| f.starts_with(&folder)) {
        crate::drawings::clear(&folder)?;
        let _ = std::fs::remove_dir(&folder); // only goes when empty
    }
    for name in ["layout.dxf", "plies.csv", "report.pdf", "labels.csv", "templates.pdf", crate::drawings::ONE_FILE] {
        let f = dir.join(name);
        if !written.contains(&f) {
            match std::fs::remove_file(&f) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    return Err(e).with_context(|| format!("cannot remove the old {}", f.display()));
                }
                _ => {}
            }
        }
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::civil_from_days;

    #[test]
    fn dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(20_778), (2026, 11, 21));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }
}
