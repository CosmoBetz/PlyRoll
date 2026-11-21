//! Numbers for a finished layout and the cut files written from it. Shared by
//! the CLI and the desktop app so both produce the same report.json.

use crate::export;
use crate::model::{Layout, PlyShape};
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
}

/// Writes report.json and layout.svg always, and layout.dxf + plies.csv only
/// when verification passed. Returns the files written.
pub fn write_outputs(dir: &Path, plies: &[PlyShape], layout: &Layout, summary: &Summary, run: &RunInfo, warnings: &[String]) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let m = &layout.material;
    let mut json = serde_json::to_value(summary)?;
    let obj = json.as_object_mut().expect("summary is an object");
    obj.insert("material".into(), serde_json::to_value(m)?);
    obj.insert("solve_seconds".into(), run.solve_seconds.into());
    obj.insert("seed".into(), run.seed.into());
    obj.insert("stopped_early".into(), run.stopped.into());
    obj.insert("import_warnings".into(), serde_json::to_value(warnings)?);
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
    }
    Ok(written)
}
