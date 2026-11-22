//! Editing a finished nest by hand: move plies, turn them to another allowed
//! angle, lock them against moving.
//!
//! An edit never makes a layout invalid silently: after every edit the used
//! length and kit sections are refitted and the layout is verified again, and
//! export is refused while it fails (as for a solver result). Turning is limited
//! to the rotations that keep the fibres along the roll, so the fibre check
//! cannot fail through an edit; moves may leave plies too close or off the roll,
//! and the verification says which.

use crate::geom::{Affine, BBox, FlattenMode, Pt, norm_deg};
use crate::kits::LayoutSection;
use crate::model::{Layout, PlyShape};
use crate::orient::allowed_rotations;
use crate::project::NestResult;
use crate::verify::placed_outlines;
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};

/// Largest single move, mm.
const MAX_MOVE: f64 = 1.0e6;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum Edit {
    /// Move plies by (dx, dy) in roll mm.
    Move { indices: Vec<usize>, dx: f64, dy: f64 },
    /// Turn one ply by `by` degrees about the centre of its bounding box. The
    /// new rotation must be one the fibre rules allow.
    Rotate { index: usize, by: f64 },
    /// Lock plies (they cannot be moved or turned) or unlock them.
    Lock { indices: Vec<usize>, on: bool },
}

/// What undo needs to put a result back.
#[derive(Clone, Debug, PartialEq)]
pub struct EditSnapshot {
    pub layout: Layout,
    pub locked: Vec<usize>,
    pub edits: u32,
}

fn ang_dist(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d)
}

fn name(plies: &[PlyShape], layout: &Layout, i: usize) -> String {
    let pl = &layout.placements[i];
    let p = &plies[pl.shape_idx];
    format!("{} {:+}° copy {}", p.shape.name, p.variants[pl.variant_idx].angle_deg, pl.copy)
}

fn check_indices(layout: &Layout, indices: &[usize]) -> Result<()> {
    ensure!(!indices.is_empty(), "no ply selected");
    for (k, &i) in indices.iter().enumerate() {
        ensure!(i < layout.placements.len(), "there is no ply {i} in this nest");
        ensure!(!indices[..k].contains(&i), "ply {i} is selected twice");
    }
    Ok(())
}

/// Centre of a placed ply's bounding box, roll mm.
fn placed_centre(ply: &PlyShape, t: &Affine) -> Pt {
    let b = BBox::of(&ply.shape.outer.transformed(t, 0.01).flatten(0.05, FlattenMode::Enclosing));
    Pt::new((b.min.x + b.max.x) / 2.0, (b.min.y + b.max.y) / 2.0)
}

/// Applies an edit to a layout. Nothing changes when it is refused.
pub fn apply(plies: &[PlyShape], layout: &mut Layout, locked: &mut Vec<usize>, e: &Edit) -> Result<()> {
    match e {
        Edit::Move { indices, dx, dy } => {
            check_indices(layout, indices)?;
            ensure!(dx.is_finite() && dy.is_finite() && dx.abs() <= MAX_MOVE && dy.abs() <= MAX_MOVE, "the move is not a valid distance");
            if let Some(&i) = indices.iter().find(|i| locked.contains(i)) {
                bail!("{} is locked; unlock it first", name(plies, layout, i));
            }
            for &i in indices {
                let pl = &mut layout.placements[i];
                pl.tx += dx;
                pl.ty += dy;
            }
        }
        Edit::Rotate { index, by } => {
            check_indices(layout, &[*index])?;
            ensure!(by.is_finite(), "the turn is not a valid angle");
            if locked.contains(index) {
                bail!("{} is locked; unlock it first", name(plies, layout, *index));
            }
            let pl = &layout.placements[*index];
            let ply = &plies[pl.shape_idx];
            let fibre = ply.fibre_deg(&ply.variants[pl.variant_idx]);
            let want = norm_deg(pl.rotation_deg + by);
            let Some(to) = allowed_rotations(fibre, &layout.material, ply.cosmetic).into_iter().find(|&a| ang_dist(a, want) < 1e-6) else {
                bail!("{} cannot turn to {want:.1}°: its fibres would be off the roll direction", name(plies, layout, *index));
            };
            // Keep the centre where it is: c = R(r) p + t = R(to) p + t'.
            let c = placed_centre(ply, &pl.transform());
            let back = Affine::rot_trans(-pl.rotation_deg, 0.0, 0.0).apply(Pt::new(c.x - pl.tx, c.y - pl.ty));
            let turned = Affine::rot_trans(to, 0.0, 0.0).apply(back);
            let pl = &mut layout.placements[*index];
            pl.rotation_deg = norm_deg(to);
            pl.tx = c.x - turned.x;
            pl.ty = c.y - turned.y;
        }
        Edit::Lock { indices, on } => {
            check_indices(layout, indices)?;
            for &i in indices {
                if *on && !locked.contains(&i) {
                    locked.push(i);
                } else if !on {
                    locked.retain(|&l| l != i);
                }
            }
            locked.sort_unstable();
            return Ok(());
        }
    }
    refit(plies, layout);
    Ok(())
}

/// Recomputes the used length (the far end of the last ply) and, kit by kit,
/// each section's end from its plies. Section starts stay where the nest put
/// them, so a ply moved into the next kit's stretch is the one the
/// verification names.
pub fn refit(plies: &[PlyShape], layout: &mut Layout) {
    let m = &layout.material;
    let outlines = placed_outlines(plies, layout, FlattenMode::Enclosing);
    let far = outlines.iter().map(|o| o.bbox.max.x).fold(m.lead_trim, f64::max);
    if layout.sections.is_empty() {
        layout.used_length = far;
        return;
    }
    let sections: Vec<LayoutSection> = layout
        .sections
        .iter()
        .map(|s| {
            let end = layout
                .placements
                .iter()
                .zip(&outlines)
                .filter(|(pl, _)| plies[pl.shape_idx].variants[pl.variant_idx].kit == s.kit)
                .map(|(_, o)| o.bbox.max.x)
                .fold(s.start, f64::max);
            LayoutSection { kit: s.kit.clone(), start: s.start, end }
        })
        .collect();
    layout.used_length = sections.iter().map(|s| s.end).fold(far, f64::max);
    layout.sections = sections;
}

impl NestResult {
    /// Applies an edit and checks the layout again. Moves and turns count as
    /// hand edits on the report; locking does not.
    pub fn edit(&mut self, e: &Edit) -> Result<()> {
        let mut layout = self.layout.clone();
        let mut locked = self.locked.clone();
        apply(&self.plies, &mut layout, &mut locked, e)?;
        if !matches!(e, Edit::Lock { .. }) {
            self.run.edits += 1;
        }
        self.layout = layout;
        self.locked = locked;
        self.summary = crate::report::summarize(&self.plies, &self.layout);
        Ok(())
    }

    pub fn snapshot(&self) -> EditSnapshot {
        EditSnapshot { layout: self.layout.clone(), locked: self.locked.clone(), edits: self.run.edits }
    }

    pub fn restore(&mut self, s: EditSnapshot) {
        self.layout = s.layout;
        self.locked = s.locked;
        self.run.edits = s.edits;
        self.summary = crate::report::summarize(&self.plies, &self.layout);
    }
}
