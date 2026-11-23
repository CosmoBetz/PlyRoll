//! Ply drawings for the plybook: every ply at every angle as a DXF, with
//! the same CUT, MARK and LABEL layers as layout.dxf.
//!
//! - As cut: turned so the fibre runs along x, the way the ply comes off the
//!   roll (a mirrored drawing stays mirrored, as it is cut).
//! - As drawn: the ply as in the CAD drawing, with the fibre line at the ply
//!   angle (a drawing mirrored on import is shown the way CAD shows it).

use crate::export::{Dxf, Label, angle_text, fibre_line, inside_point};
use crate::geom::{Affine, BBox, FlattenMode, Path, Pt, norm_deg};
use crate::model::PlyShape;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path as FsPath, PathBuf};

/// Folder for one file per ply.
pub const FOLDER: &str = "ply-drawings";
/// The single file with all plies.
pub const ONE_FILE: &str = "ply-drawings.dxf";

const TOL: f64 = 0.01;
/// Space between plies in the single file, mm.
const GAP: f64 = 40.0;
/// Caption text height, mm.
const CAPTION: f64 = 6.0;
/// Rough caption width per character, mm.
const CAPTION_CHAR: f64 = CAPTION * 0.7;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Turn {
    /// Fibre along x, as the ply comes off the roll.
    AsCut,
    /// As in the CAD drawing.
    AsDrawn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Files {
    /// ply-drawings/<ply>_<angle>.dxf
    PerPly,
    /// ply-drawings.dxf
    OneFile,
}

/// Which ply drawings to export.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlyDrawings {
    pub turn: Turn,
    pub files: Files,
}

/// One ply at one angle, ready to draw.
#[derive(Clone, Debug)]
pub struct Drawing {
    /// File name without extension, unique within the job.
    pub stem: String,
    /// "rib +45°"
    pub caption: String,
    pub outer: Path,
    pub holes: Vec<Path>,
    pub marks: Vec<Path>,
    /// Fibre direction in the drawing, degrees.
    pub fibre_deg: f64,
}

impl Drawing {
    fn bbox(&self) -> BBox {
        BBox::of(&self.outer.flatten(0.1, FlattenMode::Enclosing))
    }

    fn moved(&self, dx: f64, dy: f64) -> Drawing {
        let t = Affine::translate(dx, dy);
        Drawing {
            outer: self.outer.transformed(&t, TOL),
            holes: self.holes.iter().map(|h| h.transformed(&t, TOL)).collect(),
            marks: self.marks.iter().map(|m| m.transformed(&t, TOL)).collect(),
            ..self.clone()
        }
    }
}

/// Every ply at every angle it is cut at, in project order. Copies, kits and
/// sets do not matter here: one drawing per shape and angle.
pub fn drawings(plies: &[PlyShape], turn: Turn) -> Vec<Drawing> {
    let mut out: Vec<Drawing> = Vec::new();
    for ply in plies {
        let mut angles: Vec<f64> = Vec::new();
        for v in ply.variants.iter().filter(|v| v.qty > 0) {
            if angles.iter().any(|a| (a - v.angle_deg).abs() < 1e-9) {
                continue;
            }
            angles.push(v.angle_deg);
            let f = ply.fibre_deg(v);
            let (t, fibre) = match turn {
                Turn::AsCut => (Affine::rot_trans(-f, 0.0, 0.0), 0.0),
                // The stored geometry is mirrored (x -> -x) when the drawing was mirrored on import.
                Turn::AsDrawn if ply.shape.mirrored_on_import => (Affine::mirror_x(), -f),
                Turn::AsDrawn => (Affine::IDENTITY, f),
            };
            let s = &ply.shape;
            let mut d = Drawing {
                stem: String::new(),
                caption: format!("{} {}°", s.name, angle_text(v.angle_deg)),
                outer: s.outer.transformed(&t, TOL),
                holes: s.holes.iter().map(|h| h.transformed(&t, TOL)).collect(),
                marks: s.marks.iter().map(|m| m.transformed(&t, TOL)).collect(),
                fibre_deg: norm_deg(fibre) % 180.0,
            };
            // As cut there are no drawing coordinates to keep: start at the origin.
            if turn == Turn::AsCut {
                let b = d.bbox();
                d = d.moved(-b.min.x, -b.min.y);
            }
            let base = format!("{}_{}", file_safe(&s.name), angle_text(v.angle_deg));
            let mut stem = base.clone();
            let mut n = 2;
            while out.iter().any(|o| o.stem.eq_ignore_ascii_case(&stem)) {
                stem = format!("{base}_{n}");
                n += 1;
            }
            d.stem = stem;
            out.push(d);
        }
    }
    out
}

/// Letters, digits and `-_.+`; anything else becomes one `_` ("rib [1/2]" -> "rib_1_2").
fn file_safe(name: &str) -> String {
    let mut s = String::new();
    for c in name.chars() {
        if c.is_alphanumeric() || "-.+".contains(c) {
            s.push(c);
        } else if !s.is_empty() && !s.ends_with('_') {
            s.push('_');
        }
    }
    let s = s.trim_end_matches('_');
    if s.is_empty() { "ply".into() } else { s.into() }
}

/// Outline and holes on CUT, marks and the fibre line on MARK, the caption under the ply on LABEL.
fn draw(d: &mut Dxf, p: &Drawing) {
    let (at, room) = inside_point(&p.outer, &p.holes);
    let label = Label { at, room, text: String::new(), fibre_dir: p.fibre_deg };
    for m in &p.marks {
        d.path("MARK", m);
    }
    if let Some((a, b)) = fibre_line(&label) {
        d.line("MARK", a, b);
    }
    let b = p.bbox();
    d.text("LABEL", Pt::new((b.min.x + b.max.x) / 2.0, b.min.y - CAPTION * 1.5), CAPTION, &p.caption);
    for h in &p.holes {
        d.path("CUT", h);
    }
    d.path("CUT", &p.outer);
}

fn layers() -> Dxf {
    Dxf::new(&[("CUT", 7), ("MARK", 3), ("LABEL", 5)])
}

/// Writes the ply drawings into `dir` and returns the files written. One
/// file per ply goes in a folder of its own; old drawings there are removed
/// first, so the folder never holds plies of another job.
pub fn write(dir: &FsPath, plies: &[PlyShape], opts: &PlyDrawings) -> Result<Vec<PathBuf>> {
    let all = drawings(plies, opts.turn);
    match opts.files {
        Files::PerPly => {
            let folder = dir.join(FOLDER);
            std::fs::create_dir_all(&folder).with_context(|| format!("cannot create {}", folder.display()))?;
            clear(&folder)?;
            let mut written = Vec::new();
            for p in &all {
                let mut d = layers();
                draw(&mut d, p);
                let f = folder.join(format!("{}.dxf", p.stem));
                d.save(&f)?;
                written.push(f);
            }
            Ok(written)
        }
        Files::OneFile => {
            // Rows from the top down, in project order, each row at most as wide as the widest ply or 2 m.
            let width = all.iter().map(|p| p.bbox().width()).fold(2000.0, f64::max);
            let mut d = layers();
            let (mut x, mut top, mut row_h) = (0.0, 0.0, 0.0f64);
            for p in &all {
                let b = p.bbox();
                // A caption wider than its ply gets the room, so it stays clear of the next one.
                let w = b.width().max(p.caption.chars().count() as f64 * CAPTION_CHAR);
                if x > 0.0 && x + w > width {
                    top -= row_h + GAP + 3.0 * CAPTION;
                    x = 0.0;
                    row_h = 0.0;
                }
                draw(&mut d, &p.moved(x + (w - b.width()) / 2.0 - b.min.x, top - b.max.y));
                x += w + GAP;
                row_h = row_h.max(b.height());
            }
            let f = dir.join(ONE_FILE);
            d.save(&f)?;
            Ok(vec![f])
        }
    }
}

/// Removes the DXF files in a ply-drawings folder.
pub fn clear(folder: &FsPath) -> Result<()> {
    let Ok(entries) = std::fs::read_dir(folder) else { return Ok(()) };
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dxf")) {
            std::fs::remove_file(&p).with_context(|| format!("cannot remove the old {}", p.display()))?;
        }
    }
    Ok(())
}
