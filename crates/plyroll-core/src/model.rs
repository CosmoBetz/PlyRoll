//! Domain model: ply shapes, angle variants, materials.

use crate::geom::{FlattenMode, Path, Pt, signed_area};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One imported outline, before any orientation is assigned.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Shape {
    pub name: String,
    pub source_file: PathBuf,
    /// Outer contour, counter-clockwise.
    pub outer: Path,
    /// Hole contours, clockwise. Cut, but not used for nesting.
    pub holes: Vec<Path>,
    /// Marking geometry (pen/inkjet), carried along with the part.
    pub marks: Vec<Path>,
    /// Fibre / 0° reference direction found on a fibre layer, in degrees.
    pub layer_ref_deg: Option<f64>,
    /// True if the drawing was mirrored once at import (other face shown in CAD).
    pub mirrored_on_import: bool,
}

impl Shape {
    pub fn area(&self, tol: f64) -> f64 {
        let outer = signed_area(&self.outer.flatten(tol, FlattenMode::Inscribed)).abs();
        let holes: f64 = self.holes.iter().map(|h| signed_area(&h.flatten(tol, FlattenMode::Inscribed)).abs()).sum();
        outer - holes
    }
}

/// How the 0° reference axis of a ply shape was chosen.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RefOrigin {
    /// Typed angle relative to drawing X.
    Manual,
    DrawingX,
    DrawingY,
    LongestEdge,
    /// Long axis of the smallest-area bounding rectangle.
    BoundingBox,
    /// From a line on a fibre / 0° layer in the DXF.
    FromLayer,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Variant {
    /// Ply angle relative to the shape's 0° reference, degrees, CCW positive.
    pub angle_deg: f64,
    pub qty: u32,
    pub kit: Option<String>,
}

/// A shape with its 0° reference and the angles it must be cut at.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlyShape {
    pub shape: Shape,
    /// 0° reference axis in drawing coordinates, degrees.
    pub ref_deg: f64,
    pub ref_origin: RefOrigin,
    /// Visible / cosmetic ply: twill warp may never be swapped with weft.
    pub cosmetic: bool,
    pub variants: Vec<Variant>,
}

impl PlyShape {
    /// Fibre direction of a variant in drawing coordinates.
    pub fn fibre_deg(&self, v: &Variant) -> f64 {
        self.ref_deg + v.angle_deg
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MaterialKind {
    Ud,
    Twill,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Material {
    pub name: String,
    pub kind: MaterialKind,
    /// Nominal roll width, mm.
    pub roll_width: f64,
    /// Trim on each long edge of the roll, mm.
    #[serde(default)]
    pub edge_trim: f64,
    /// Trim at the start of the roll before the first part, mm.
    #[serde(default)]
    pub lead_trim: f64,
    /// Minimum gap between parts, mm.
    #[serde(default)]
    pub spacing: f64,
    /// Allowed deviation from the exact fibre direction, degrees (0 = exact).
    #[serde(default)]
    pub angle_tolerance: f64,
    /// Step used to sample angles inside the tolerance, degrees.
    #[serde(default = "default_tol_step")]
    pub tolerance_step: f64,
    /// Twill only: warp and weft may be swapped (90° turns allowed).
    #[serde(default)]
    pub warp_weft_interchangeable: bool,
    #[serde(default)]
    pub cost_per_m2: Option<f64>,
}

fn default_tol_step() -> f64 {
    0.5
}

impl Material {
    /// Width that parts may occupy, mm.
    pub fn usable_width(&self) -> f64 {
        self.roll_width - 2.0 * self.edge_trim
    }
}

/// A placed copy of a ply variant on the roll.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Placement {
    pub shape_idx: usize,
    pub variant_idx: usize,
    /// Running number of this copy within its variant (1-based).
    pub copy: u32,
    /// Rotation applied to the drawing, degrees CCW.
    pub rotation_deg: f64,
    /// Translation applied after rotation, roll coordinates (mm).
    pub tx: f64,
    pub ty: f64,
}

impl Placement {
    pub fn transform(&self) -> crate::geom::Affine {
        crate::geom::Affine::rot_trans(self.rotation_deg, self.tx, self.ty)
    }
}

/// Roll coordinates: x runs along the roll (0 = roll start, including lead trim),
/// y across it (0 = one roll edge, roll_width = the other).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Layout {
    pub material: Material,
    pub placements: Vec<Placement>,
    /// Roll length consumed, including lead trim, mm.
    pub used_length: f64,
}

pub fn pt_list(path: &Path, tol: f64) -> Vec<Pt> {
    path.flatten(tol, FlattenMode::Inscribed)
}
