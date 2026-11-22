//! Independent check of a finished layout. Does not trust the solver:
//! it re-derives everything from the exact part geometry.

use crate::geom::{BBox, FlattenMode, Pt};
use crate::model::{Layout, PlyShape};
use crate::orient::{fibre_deviation, fibre_ok};
use geo::{Coord, Distance, Euclidean, LineString, Polygon};
use serde::{Deserialize, Serialize};

/// Flattening tolerance used for checking, mm.
const CHECK_TOL: f64 = 0.005;
/// Slack for flattening and the solver's f32 arithmetic, mm.
const EPS: f64 = 0.02;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VerifyReport {
    pub ok: bool,
    pub issues: Vec<String>,
    /// Smallest gap between any two parts, mm (None if fewer than two parts).
    pub min_gap: Option<f64>,
    /// Largest fibre deviation from the roll direction, degrees.
    pub max_fibre_deviation: f64,
    /// Placements named in an issue (indices into the layout, sorted).
    #[serde(default)]
    pub bad: Vec<usize>,
}

pub struct PlacedOutline {
    pub poly: Vec<Pt>,
    pub bbox: BBox,
}

/// Placed outer outline of every placement (in layout order).
pub fn placed_outlines(plies: &[PlyShape], layout: &Layout, mode: FlattenMode) -> Vec<PlacedOutline> {
    layout
        .placements
        .iter()
        .map(|pl| {
            let outer = plies[pl.shape_idx].shape.outer.transformed(&pl.transform(), CHECK_TOL);
            let poly = outer.flatten(CHECK_TOL, mode);
            let bbox = BBox::of(&poly);
            PlacedOutline { poly, bbox }
        })
        .collect()
}

fn to_geo(pts: &[Pt]) -> Polygon<f64> {
    Polygon::new(LineString::from(pts.iter().map(|p| Coord { x: p.x, y: p.y }).collect::<Vec<_>>()), vec![])
}

pub fn verify(plies: &[PlyShape], layout: &Layout) -> VerifyReport {
    let m = &layout.material;
    let mut issues = Vec::new();
    let mut bad = Vec::new();

    // 1. Quantities.
    for (si, ply) in plies.iter().enumerate() {
        for (vi, v) in ply.variants.iter().enumerate() {
            let n = layout.placements.iter().filter(|p| p.shape_idx == si && p.variant_idx == vi).count() as u32;
            if n != v.qty {
                issues.push(format!("{} {:+}°: {} placed, {} ordered", ply.shape.name, v.angle_deg, n, v.qty));
            }
        }
    }

    // 2. Fibre orientation (no mirroring is possible: placements only rotate and translate).
    let mut max_dev: f64 = 0.0;
    for (i, pl) in layout.placements.iter().enumerate() {
        let ply = &plies[pl.shape_idx];
        let fibre = ply.fibre_deg(&ply.variants[pl.variant_idx]);
        let dev = fibre_deviation(fibre, pl.rotation_deg, m, ply.cosmetic);
        max_dev = max_dev.max(dev);
        if !fibre_ok(fibre, pl.rotation_deg, m, ply.cosmetic) {
            issues.push(format!("{} {:+}° copy {}: fibres {:.2}° off the roll direction", ply.shape.name, ply.variants[pl.variant_idx].angle_deg, pl.copy, dev));
            bad.push(i);
        }
    }

    // 3. Inside the usable roll area. Enclosing outlines are conservative.
    let enclosing = placed_outlines(plies, layout, FlattenMode::Enclosing);
    let (y_min, y_max) = (m.edge_trim, m.roll_width - m.edge_trim);
    for (i, (pl, o)) in layout.placements.iter().zip(&enclosing).enumerate() {
        let b = o.bbox;
        if b.min.x < m.lead_trim - EPS || b.min.y < y_min - EPS || b.max.y > y_max + EPS || b.max.x > layout.used_length + EPS {
            let ply = &plies[pl.shape_idx];
            issues.push(format!(
                "{} copy {} leaves the usable roll area (x {:.2}..{:.2}, y {:.2}..{:.2})",
                ply.shape.name, pl.copy, b.min.x, b.max.x, b.min.y, b.max.y
            ));
            bad.push(i);
        }
    }

    // 4. Kit by kit: every ply inside its kit's stretch of roll.
    if !layout.sections.is_empty() {
        for (i, (pl, o)) in layout.placements.iter().zip(&enclosing).enumerate() {
            let ply = &plies[pl.shape_idx];
            let kit = &ply.variants[pl.variant_idx].kit;
            let kit_name = kit.as_deref().unwrap_or("no kit");
            let k = layout.sections.iter().position(|s| &s.kit == kit);
            match k.map(|k| (&layout.sections[k], layout.sections.get(k + 1))) {
                Some((s, next)) => {
                    // A kit's stretch ends where the next one starts, less the spacing.
                    let limit = next.map_or(s.end, |n| n.start - m.spacing);
                    if o.bbox.min.x < s.start - EPS || o.bbox.max.x > limit + EPS {
                        issues.push(format!(
                            "{} copy {} ({kit_name}) is outside its kit's stretch of roll ({:.1}..{:.1} mm)",
                            ply.shape.name, pl.copy, s.start, limit
                        ));
                        bad.push(i);
                    }
                }
                None => {
                    issues.push(format!("{} copy {} ({kit_name}) has no kit section on the roll", ply.shape.name, pl.copy));
                    bad.push(i);
                }
            }
        }
    }

    // 5. Spacing between parts.
    let inscribed = placed_outlines(plies, layout, FlattenMode::Inscribed);
    let geos: Vec<Polygon<f64>> = inscribed.iter().map(|o| to_geo(&o.poly)).collect();
    let mut order: Vec<usize> = (0..inscribed.len()).collect();
    order.sort_by(|&a, &b| inscribed[a].bbox.min.x.total_cmp(&inscribed[b].bbox.min.x));
    let mut min_gap: Option<f64> = None;
    let reach = m.spacing + 1.0;
    for (k, &i) in order.iter().enumerate() {
        for &j in &order[k + 1..] {
            if inscribed[j].bbox.min.x > inscribed[i].bbox.max.x + reach {
                break;
            }
            if !inscribed[i].bbox.overlaps(&inscribed[j].bbox, reach) {
                continue;
            }
            let d = Euclidean.distance(&geos[i], &geos[j]);
            min_gap = Some(min_gap.map_or(d, |g: f64| g.min(d)));
            if d < m.spacing - EPS {
                let (a, b) = (&layout.placements[i], &layout.placements[j]);
                issues.push(format!(
                    "{} copy {} and {} copy {} are {:.3} mm apart (need {:.3} mm)",
                    plies[a.shape_idx].shape.name, a.copy, plies[b.shape_idx].shape.name, b.copy, d, m.spacing
                ));
                bad.extend([i, j]);
            }
        }
    }
    // geo reports 0 for overlapping polygons, so overlaps are caught above.

    bad.sort_unstable();
    bad.dedup();
    VerifyReport { ok: issues.is_empty(), issues, min_gap, max_fibre_deviation: max_dev, bad }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Path;
    use crate::model::{Material, MaterialKind, Placement, RefOrigin, Shape, Variant};

    fn square_ply(size: f64, qty: u32, angle: f64) -> PlyShape {
        let outer = Path::from_points(&[Pt::new(0.0, 0.0), Pt::new(size, 0.0), Pt::new(size, size), Pt::new(0.0, size)], true);
        PlyShape {
            shape: Shape { name: "sq".into(), source_file: "sq.dxf".into(), outer, holes: vec![], marks: vec![], layer_ref_deg: None, mirrored_on_import: false },
            ref_deg: 0.0,
            ref_origin: RefOrigin::DrawingX,
            cosmetic: false,
            variants: vec![Variant { angle_deg: angle, qty, kit: None }],
        }
    }

    fn material() -> Material {
        Material {
            name: "ud".into(),
            kind: MaterialKind::Ud,
            roll_width: 100.0,
            edge_trim: 5.0,
            lead_trim: 10.0,
            spacing: 2.0,
            angle_tolerance: 0.0,
            tolerance_step: 0.5,
            warp_weft_interchangeable: false,
            cost_per_m2: None,
        }
    }

    fn layout(places: &[(f64, f64, f64)]) -> Layout {
        Layout {
            material: material(),
            placements: places.iter().enumerate().map(|(i, &(r, x, y))| Placement { shape_idx: 0, variant_idx: 0, copy: i as u32 + 1, rotation_deg: r, tx: x, ty: y }).collect(),
            used_length: 100.0,
            sections: vec![],
        }
    }

    #[test]
    fn good_layout_passes() {
        let plies = vec![square_ply(20.0, 2, 0.0)];
        let r = verify(&plies, &layout(&[(0.0, 10.0, 5.0), (0.0, 32.0, 5.0)]));
        assert!(r.ok, "{:?}", r.issues);
        assert!((r.min_gap.unwrap() - 2.0).abs() < 1e-6);
    }

    #[test]
    fn catches_overlap_spacing_bounds_and_fibre() {
        let plies = vec![square_ply(20.0, 2, 0.0)];
        // Overlapping.
        assert!(!verify(&plies, &layout(&[(0.0, 10.0, 5.0), (0.0, 25.0, 5.0)])).ok);
        // Too close (1 mm).
        assert!(!verify(&plies, &layout(&[(0.0, 10.0, 5.0), (0.0, 31.0, 5.0)])).ok);
        // Into the edge trim.
        assert!(!verify(&plies, &layout(&[(0.0, 10.0, 2.0), (0.0, 40.0, 5.0)])).ok);
        // Rotated 90° on a UD 0° ply: wrong fibre direction.
        let r = verify(&plies, &layout(&[(90.0, 30.0, 5.0), (0.0, 40.0, 5.0)]));
        assert!(r.issues.iter().any(|i| i.contains("fibres")), "{:?}", r.issues);
        // Wrong count.
        assert!(!verify(&plies, &layout(&[(0.0, 10.0, 5.0)])).ok);
    }
}
