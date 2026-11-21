//! Display data for the orientation editor: flattened outlines, pickable
//! straight edges and the ply's reference state. Drawing coordinates (mm, y up).

use crate::geom::{BBox, FlattenMode, Path};
use crate::model::{RefOrigin, Variant};
use crate::project::{PlyStatus, Project, ProjectPly};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct EdgeView {
    /// 0 = outline, k = hole k−1.
    pub contour: usize,
    pub seg: usize,
    pub a: [f64; 2],
    pub b: [f64; 2],
    pub len: f64,
    /// Direction, degrees from drawing X, normalised to (−90, 90].
    pub angle: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct PlyView {
    pub id: u64,
    pub name: String,
    pub file: String,
    pub part: String,
    pub status: PlyStatus,
    pub note: Option<String>,
    pub ref_deg: Option<f64>,
    pub ref_origin: Option<RefOrigin>,
    pub layer_ref_deg: Option<f64>,
    pub cosmetic: bool,
    pub mirrored: bool,
    pub variants: Vec<Variant>,
    pub total_qty: u32,
    pub area: f64,
    /// [min x, min y, max x, max y]
    pub bbox: [f64; 4],
    pub outline: Vec<[f64; 2]>,
    pub holes: Vec<Vec<[f64; 2]>>,
    pub marks: Vec<MarkView>,
    pub edges: Vec<EdgeView>,
}

#[derive(Clone, Debug, Serialize)]
pub struct MarkView {
    pub pts: Vec<[f64; 2]>,
    pub closed: bool,
}

fn pts(p: &Path, tol: f64) -> Vec<[f64; 2]> {
    p.flatten(tol, FlattenMode::Inscribed).into_iter().map(|q| [q.x, q.y]).collect()
}

pub fn ply_view(p: &ProjectPly) -> PlyView {
    let s = &p.shape;
    let outer_pts = s.outer.flatten(0.05, FlattenMode::Inscribed);
    let bb = BBox::of(&outer_pts);
    // Display tolerance scales with the part so big skins stay light.
    let tol = (bb.width().max(bb.height()) * 0.0005).clamp(0.01, 0.5);
    let mut edges = Vec::new();
    let contours = std::iter::once(&s.outer).chain(s.holes.iter());
    for (ci, path) in contours.enumerate() {
        for i in 0..path.seg_count() {
            let (a, b, bulge) = path.seg(i);
            let len = a.dist(b);
            if bulge.abs() < 1e-12 && len > 1e-6 {
                let angle = crate::project::norm_axis((b.y - a.y).atan2(b.x - a.x).to_degrees());
                edges.push(EdgeView { contour: ci, seg: i, a: [a.x, a.y], b: [b.x, b.y], len, angle });
            }
        }
    }
    PlyView {
        id: p.id,
        name: s.name.clone(),
        file: s.source_file.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default(),
        part: p.part.clone(),
        status: p.status(),
        note: p.note.clone(),
        ref_deg: p.reference.map(|r| r.deg),
        ref_origin: p.reference.map(|r| r.origin),
        layer_ref_deg: s.layer_ref_deg,
        cosmetic: p.cosmetic,
        mirrored: s.mirrored_on_import,
        variants: p.variants.clone(),
        total_qty: p.total_qty(),
        area: s.area(0.05),
        bbox: [bb.min.x, bb.min.y, bb.max.x, bb.max.y],
        outline: pts(&s.outer, tol),
        holes: s.holes.iter().map(|h| pts(h, tol)).collect(),
        marks: s.marks.iter().map(|m| MarkView { pts: pts(m, tol), closed: m.closed }).collect(),
        edges,
    }
}

pub fn project_view(p: &Project) -> Vec<PlyView> {
    p.plies.iter().map(ply_view).collect()
}
