//! Exact 2D geometry: paths made of straight and circular-arc segments.
//!
//! A path is a list of vertices; the segment from vertex `i` to vertex `i+1`
//! is an arc described by the DXF "bulge" of vertex `i`
//! (`bulge = tan(sweep / 4)`, 0 = straight line, positive = counter-clockwise).
//! Arcs stay exact for cut output and are only flattened into polygons for
//! nesting and checking.

use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pt {
    pub x: f64,
    pub y: f64,
}

impl Pt {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
    pub fn dist(self, o: Pt) -> f64 {
        (self.x - o.x).hypot(self.y - o.y)
    }
    pub fn sub(self, o: Pt) -> Pt {
        Pt::new(self.x - o.x, self.y - o.y)
    }
    pub fn add(self, o: Pt) -> Pt {
        Pt::new(self.x + o.x, self.y + o.y)
    }
    pub fn scale(self, s: f64) -> Pt {
        Pt::new(self.x * s, self.y * s)
    }
    pub fn len(self) -> f64 {
        self.x.hypot(self.y)
    }
}

/// Affine map `p' = [a b; c d] p + [e f]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Affine {
    pub const IDENTITY: Affine = Affine { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };

    /// Rotation by `deg` (counter-clockwise) about the origin, then translation.
    pub fn rot_trans(deg: f64, tx: f64, ty: f64) -> Affine {
        let (s, c) = deg.to_radians().sin_cos();
        Affine { a: c, b: -s, c: s, d: c, e: tx, f: ty }
    }
    pub fn scale(sx: f64, sy: f64) -> Affine {
        Affine { a: sx, b: 0.0, c: 0.0, d: sy, e: 0.0, f: 0.0 }
    }
    pub fn translate(tx: f64, ty: f64) -> Affine {
        Affine { e: tx, f: ty, ..Affine::IDENTITY }
    }
    /// Mirror across the drawing's Y axis (x -> -x).
    pub fn mirror_x() -> Affine {
        Affine::scale(-1.0, 1.0)
    }
    /// Composition: apply `self` first, then `next`.
    pub fn then(self, next: Affine) -> Affine {
        Affine {
            a: next.a * self.a + next.b * self.c,
            b: next.a * self.b + next.b * self.d,
            c: next.c * self.a + next.d * self.c,
            d: next.c * self.b + next.d * self.d,
            e: next.a * self.e + next.b * self.f + next.e,
            f: next.c * self.e + next.d * self.f + next.f,
        }
    }
    pub fn apply(&self, p: Pt) -> Pt {
        Pt::new(self.a * p.x + self.b * p.y + self.e, self.c * p.x + self.d * p.y + self.f)
    }
    pub fn det(&self) -> f64 {
        self.a * self.d - self.b * self.c
    }
    /// True if the map scales equally in all directions (arcs stay circular).
    pub fn is_conformal(&self) -> bool {
        let sx = self.a.hypot(self.c);
        let sy = self.b.hypot(self.d);
        let dot = self.a * self.b + self.c * self.d;
        (sx - sy).abs() <= 1e-9 * sx.max(1.0) && dot.abs() <= 1e-9 * sx.max(1.0) * sy.max(1.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Vtx {
    pub p: Pt,
    /// Bulge of the segment that starts at this vertex.
    pub bulge: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Path {
    pub verts: Vec<Vtx>,
    pub closed: bool,
}

/// A circular arc in centre form.
#[derive(Clone, Copy, Debug)]
pub struct ArcGeom {
    pub center: Pt,
    pub radius: f64,
    pub start_angle: f64,
    /// Signed sweep in radians (positive = counter-clockwise).
    pub sweep: f64,
}

impl ArcGeom {
    pub fn point_at(&self, angle: f64) -> Pt {
        Pt::new(self.center.x + self.radius * angle.cos(), self.center.y + self.radius * angle.sin())
    }
}

/// Converts a bulge segment to centre form. Returns `None` for straight segments.
pub fn arc_from_bulge(p0: Pt, p1: Pt, bulge: f64) -> Option<ArcGeom> {
    let c = p0.dist(p1);
    if bulge.abs() < 1e-12 || c < 1e-12 {
        return None;
    }
    let sweep = 4.0 * bulge.atan();
    let radius = c * (1.0 + bulge * bulge) / (4.0 * bulge.abs());
    // Centre lies on the chord's left normal for CCW arcs, at signed distance h.
    let h = c * (1.0 - bulge * bulge) / (4.0 * bulge);
    let m = Pt::new((p0.x + p1.x) / 2.0, (p0.y + p1.y) / 2.0);
    let n = Pt::new(-(p1.y - p0.y) / c, (p1.x - p0.x) / c);
    let center = m.add(n.scale(h));
    let start_angle = (p0.y - center.y).atan2(p0.x - center.x);
    Some(ArcGeom { center, radius, start_angle, sweep })
}

/// Bulge for an arc from `start_angle` sweeping `sweep` radians.
pub fn bulge_from_sweep(sweep: f64) -> f64 {
    (sweep / 4.0).tan()
}

/// Number of chords needed so that the chord deviation stays under `tol`.
fn arc_steps(radius: f64, sweep: f64, tol: f64) -> usize {
    let max_step = if radius > tol {
        (2.0 * (1.0 - tol / radius).clamp(-1.0, 1.0).acos()).min(PI / 4.0)
    } else {
        PI / 4.0
    };
    ((sweep.abs() / max_step).ceil() as usize).max(1)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlattenMode {
    /// All points lie on the true curve (chords cut slightly inside convex arcs).
    Inscribed,
    /// For closed contours: the polygon fully encloses the true shape.
    /// Arcs bulging outward are replaced by tangent lines (circumscribed).
    Enclosing,
}

impl Path {
    pub fn new(verts: Vec<Vtx>, closed: bool) -> Self {
        Self { verts, closed }
    }

    pub fn from_points(pts: &[Pt], closed: bool) -> Self {
        Self::new(pts.iter().map(|&p| Vtx { p, bulge: 0.0 }).collect(), closed)
    }

    pub fn start(&self) -> Pt {
        self.verts[0].p
    }

    pub fn end(&self) -> Pt {
        if self.closed { self.verts[0].p } else { self.verts[self.verts.len() - 1].p }
    }

    /// Number of segments.
    pub fn seg_count(&self) -> usize {
        if self.closed { self.verts.len() } else { self.verts.len().saturating_sub(1) }
    }

    /// Segment `i` as (start, end, bulge).
    pub fn seg(&self, i: usize) -> (Pt, Pt, f64) {
        let n = self.verts.len();
        (self.verts[i].p, self.verts[(i + 1) % n].p, self.verts[i].bulge)
    }

    pub fn reversed(&self) -> Path {
        let n = self.verts.len();
        if n == 0 {
            return self.clone();
        }
        let mut verts = Vec::with_capacity(n);
        if self.closed {
            // Keep vertex 0 first; walk backwards. Segment (v[i+1] -> v[i]) has bulge -b[i].
            verts.push(Vtx { p: self.verts[0].p, bulge: -self.verts[n - 1].bulge });
            for i in (1..n).rev() {
                verts.push(Vtx { p: self.verts[i].p, bulge: -self.verts[i - 1].bulge });
            }
        } else {
            for i in (0..n).rev() {
                let bulge = if i > 0 { -self.verts[i - 1].bulge } else { 0.0 };
                verts.push(Vtx { p: self.verts[i].p, bulge });
            }
        }
        Path { verts, closed: self.closed }
    }

    /// Applies an affine map. Arcs survive conformal maps (bulge sign flips
    /// when the map mirrors); otherwise arcs are flattened first.
    pub fn transformed(&self, t: &Affine, flatten_tol: f64) -> Path {
        let src = if t.is_conformal() { self.clone() } else { self.flattened_path(flatten_tol) };
        let flip = if t.det() < 0.0 { -1.0 } else { 1.0 };
        let mut verts: Vec<Vtx> = src.verts.iter().map(|v| Vtx { p: t.apply(v.p), bulge: v.bulge * flip }).collect();
        if !src.closed {
            if let Some(last) = verts.last_mut() {
                last.bulge = 0.0;
            }
        }
        Path { verts, closed: src.closed }
    }

    /// Replaces arcs with inscribed line segments.
    pub fn flattened_path(&self, tol: f64) -> Path {
        let pts = self.flatten(tol, FlattenMode::Inscribed);
        Path::from_points(&pts, self.closed)
    }

    /// Flattens to points. For closed paths the first point is not repeated.
    pub fn flatten(&self, tol: f64, mode: FlattenMode) -> Vec<Pt> {
        let mut out = Vec::new();
        if self.verts.is_empty() {
            return out;
        }
        // Orientation is needed to know which arcs bulge outward.
        let orient = if mode == FlattenMode::Enclosing && self.closed {
            signed_area(&self.flatten(tol, FlattenMode::Inscribed)).signum()
        } else {
            0.0
        };
        for i in 0..self.seg_count() {
            let (p0, p1, b) = self.seg(i);
            out.push(p0);
            if let Some(arc) = arc_from_bulge(p0, p1, b) {
                let n = arc_steps(arc.radius, arc.sweep, tol);
                let step = arc.sweep / n as f64;
                let outward = mode == FlattenMode::Enclosing && b * orient > 0.0;
                if outward {
                    // Tangent-line polygon: vertices at the sub-arc mid-angles,
                    // pushed out to r / cos(step/2). Encloses the arc.
                    let r_out = arc.radius / (step / 2.0).cos();
                    for k in 0..n {
                        let a = arc.start_angle + step * (k as f64 + 0.5);
                        out.push(Pt::new(arc.center.x + r_out * a.cos(), arc.center.y + r_out * a.sin()));
                    }
                } else {
                    for k in 1..n {
                        out.push(arc.point_at(arc.start_angle + step * k as f64));
                    }
                }
            }
        }
        if !self.closed {
            out.push(self.verts[self.verts.len() - 1].p);
        }
        dedup_points(&mut out, 1e-9, self.closed);
        out
    }

    /// Length of all straight segments, with their direction in degrees.
    pub fn straight_segments(&self) -> Vec<(f64, f64)> {
        (0..self.seg_count())
            .filter_map(|i| {
                let (p0, p1, b) = self.seg(i);
                (b.abs() < 1e-12).then(|| (p0.dist(p1), (p1.y - p0.y).atan2(p1.x - p0.x).to_degrees()))
            })
            .collect()
    }
}

pub fn dedup_points(pts: &mut Vec<Pt>, eps: f64, closed: bool) {
    pts.dedup_by(|a, b| a.dist(*b) <= eps);
    if closed {
        while pts.len() > 1 && pts[0].dist(pts[pts.len() - 1]) <= eps {
            pts.pop();
        }
    }
}

/// Shoelace area: positive for counter-clockwise polygons.
pub fn signed_area(pts: &[Pt]) -> f64 {
    let n = pts.len();
    if n < 3 {
        return 0.0;
    }
    let mut s = 0.0;
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        s += a.x * b.y - b.x * a.y;
    }
    s / 2.0
}

pub fn point_in_polygon(p: Pt, poly: &[Pt]) -> bool {
    let mut inside = false;
    let n = poly.len();
    let mut j = n.wrapping_sub(1);
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BBox {
    pub min: Pt,
    pub max: Pt,
}

impl BBox {
    pub fn of(pts: &[Pt]) -> BBox {
        let mut b = BBox { min: Pt::new(f64::INFINITY, f64::INFINITY), max: Pt::new(f64::NEG_INFINITY, f64::NEG_INFINITY) };
        for p in pts {
            b.min.x = b.min.x.min(p.x);
            b.min.y = b.min.y.min(p.y);
            b.max.x = b.max.x.max(p.x);
            b.max.y = b.max.y.max(p.y);
        }
        b
    }
    pub fn width(&self) -> f64 {
        self.max.x - self.min.x
    }
    pub fn height(&self) -> f64 {
        self.max.y - self.min.y
    }
    pub fn overlaps(&self, o: &BBox, margin: f64) -> bool {
        self.min.x - margin <= o.max.x && o.min.x - margin <= self.max.x && self.min.y - margin <= o.max.y && o.min.y - margin <= self.max.y
    }
}

/// Proper intersection test between two segments (shared endpoints of
/// neighbouring segments are not reported by the caller).
fn segments_intersect(p1: Pt, p2: Pt, p3: Pt, p4: Pt) -> bool {
    fn orient(a: Pt, b: Pt, c: Pt) -> f64 {
        (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
    }
    fn on_seg(a: Pt, b: Pt, c: Pt) -> bool {
        c.x >= a.x.min(b.x) - 1e-12 && c.x <= a.x.max(b.x) + 1e-12 && c.y >= a.y.min(b.y) - 1e-12 && c.y <= a.y.max(b.y) + 1e-12
    }
    let (d1, d2, d3, d4) = (orient(p3, p4, p1), orient(p3, p4, p2), orient(p1, p2, p3), orient(p1, p2, p4));
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0)) && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0)) {
        return true;
    }
    let eps = 1e-12;
    (d1.abs() < eps && on_seg(p3, p4, p1))
        || (d2.abs() < eps && on_seg(p3, p4, p2))
        || (d3.abs() < eps && on_seg(p1, p2, p3))
        || (d4.abs() < eps && on_seg(p1, p2, p4))
}

/// Finds a self-intersection of a closed polygon, if any. Returns the
/// approximate location. Uses a sweep over x to stay fast on dense outlines.
pub fn find_self_intersection(poly: &[Pt]) -> Option<Pt> {
    let n = poly.len();
    if n < 4 {
        return None;
    }
    let mut segs: Vec<(usize, f64, f64)> = (0..n)
        .map(|i| {
            let (a, b) = (poly[i], poly[(i + 1) % n]);
            (i, a.x.min(b.x), a.x.max(b.x))
        })
        .collect();
    segs.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut active: Vec<(usize, f64, f64)> = Vec::new();
    for s in segs {
        active.retain(|a| a.2 >= s.1 - 1e-12);
        for a in &active {
            let (i, j) = (a.0, s.0);
            let adjacent = (i + 1) % n == j || (j + 1) % n == i;
            if adjacent {
                continue;
            }
            let (p1, p2) = (poly[i], poly[(i + 1) % n]);
            let (p3, p4) = (poly[j], poly[(j + 1) % n]);
            if segments_intersect(p1, p2, p3, p4) {
                return Some(Pt::new((p1.x + p2.x) / 2.0, (p1.y + p2.y) / 2.0));
            }
        }
        active.push(s);
    }
    None
}

/// Normalises an angle in degrees to [0, 360).
pub fn norm_deg(a: f64) -> f64 {
    let r = a.rem_euclid(360.0);
    if (360.0 - r).abs() < 1e-9 { 0.0 } else { r }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn semicircle_bulge() {
        let arc = arc_from_bulge(Pt::new(0.0, 0.0), Pt::new(2.0, 0.0), 1.0).unwrap();
        assert!(close(arc.radius, 1.0));
        assert!(close(arc.center.x, 1.0) && close(arc.center.y, 0.0));
        assert!(close(arc.sweep, PI));
        // CCW from (0,0) to (2,0) around (1,0) passes through (1,-1).
        let mid = arc.point_at(arc.start_angle + arc.sweep / 2.0);
        assert!(close(mid.x, 1.0) && close(mid.y, -1.0));
    }

    fn rounded_square(r: f64) -> Path {
        // 10x10 square with one outward quarter-round corner at (10,10), CCW.
        let b = bulge_from_sweep(PI / 2.0);
        Path::new(
            vec![
                Vtx { p: Pt::new(0.0, 0.0), bulge: 0.0 },
                Vtx { p: Pt::new(10.0, 0.0), bulge: 0.0 },
                Vtx { p: Pt::new(10.0, 10.0 - r), bulge: b },
                Vtx { p: Pt::new(10.0 - r, 10.0), bulge: 0.0 },
                Vtx { p: Pt::new(0.0, 10.0), bulge: 0.0 },
            ],
            true,
        )
    }

    #[test]
    fn enclosing_flatten_contains_true_arc() {
        let p = rounded_square(5.0);
        let enc = p.flatten(0.5, FlattenMode::Enclosing);
        let ins = p.flatten(0.5, FlattenMode::Inscribed);
        assert!(signed_area(&enc) > signed_area(&ins));
        // Every point of the true arc is inside (or on) the enclosing polygon.
        let arc = arc_from_bulge(Pt::new(10.0, 5.0), Pt::new(5.0, 10.0), bulge_from_sweep(PI / 2.0)).unwrap();
        for k in 0..=100 {
            let q = arc.point_at(arc.start_angle + arc.sweep * k as f64 / 100.0);
            let pulled = Pt::new(q.x - (q.x - 5.0) * 1e-6, q.y - (q.y - 5.0) * 1e-6);
            assert!(point_in_polygon(pulled, &enc), "arc point {q:?} outside");
        }
    }

    #[test]
    fn reverse_keeps_shape() {
        let p = rounded_square(5.0);
        let r = p.reversed();
        let a1 = signed_area(&p.flatten(0.01, FlattenMode::Inscribed));
        let a2 = signed_area(&r.flatten(0.01, FlattenMode::Inscribed));
        assert!((a1 + a2).abs() < 1e-6, "{a1} vs {a2}");
        assert_eq!(r.reversed(), p);
    }

    #[test]
    fn mirror_flips_bulge_and_area() {
        let p = rounded_square(5.0);
        let m = p.transformed(&Affine::mirror_x(), 0.01);
        let a1 = signed_area(&p.flatten(0.01, FlattenMode::Inscribed));
        let a2 = signed_area(&m.flatten(0.01, FlattenMode::Inscribed));
        assert!((a1 + a2).abs() < 1e-6);
    }

    #[test]
    fn detects_self_intersection() {
        let bowtie = [Pt::new(0.0, 0.0), Pt::new(10.0, 10.0), Pt::new(10.0, 0.0), Pt::new(0.0, 10.0)];
        assert!(find_self_intersection(&bowtie).is_some());
        let square = [Pt::new(0.0, 0.0), Pt::new(10.0, 0.0), Pt::new(10.0, 10.0), Pt::new(0.0, 10.0)];
        assert!(find_self_intersection(&square).is_none());
    }

    #[test]
    fn affine_compose() {
        let t = Affine::rot_trans(90.0, 0.0, 0.0).then(Affine::translate(5.0, 0.0));
        let p = t.apply(Pt::new(1.0, 0.0));
        assert!(close(p.x, 5.0) && close(p.y, 1.0));
    }
}
