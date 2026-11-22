//! Plies inside the holes of other plies.
//!
//! The solver only knows outer outlines, so a hole is wasted material. Before
//! nesting, each copy of a ply with a hole gets the largest other plies that
//! fit in it (with the spacing kept and their fibres still along the roll).
//! The host and its contents then go to the solver as one item, with the
//! host's outline, and the contents are placed relative to the host after.
//!
//! Naive on purpose: bottom-left positions on a grid, largest ply first, and
//! one level only (a ply inside a hole carries no contents of its own).

use crate::{ItemGroup, ang_dist};
use geo::{Coord, Distance, Euclidean, LineString, Polygon};
use plyroll_core::geom::{Affine, BBox, FlattenMode, Pt, point_in_polygon, signed_area};
use plyroll_core::model::{Material, PlyShape};
use plyroll_core::orient::fibre_deviation;

/// Kept on top of the spacing, mm, so the exact check always passes.
const MARGIN: f64 = 0.05;
/// Chord tolerance for the search outlines, mm (outlines enclosing, holes
/// inscribed, so the search is on the safe side).
const TOL: f64 = 0.3;
/// Grid steps across a hole, per axis.
const STEPS: usize = 48;

/// One solver item: copies of a group, maybe with plies in their holes.
#[derive(Clone, Debug, PartialEq)]
pub struct Unit {
    /// The host's group.
    pub group: usize,
    /// Rotations allowed for the host with these contents.
    pub rotations: Vec<f64>,
    pub demand: u32,
    /// Plies in the host's holes.
    pub inner: Vec<Inner>,
}

/// A ply copy in a hole: `rotation` and (`tx`, `ty`) place its drawing in the
/// host's drawing, so its roll placement is the host's transform after this one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Inner {
    pub group: usize,
    pub rotation: f64,
    pub tx: f64,
    pub ty: f64,
}

struct Shape {
    outer: Vec<Pt>,
    area: f64,
    /// Holes with their areas, largest first.
    holes: Vec<(Vec<Pt>, f64)>,
}

/// One unit per group, without filling any holes.
pub fn plain_units(groups: &[ItemGroup]) -> Vec<Unit> {
    groups.iter().enumerate().map(|(g, grp)| Unit { group: g, rotations: grp.rotations.clone(), demand: grp.demand(), inner: vec![] }).collect()
}

/// Fills holes with other plies where they fit and returns the solver items.
/// Every copy of every group is in exactly one unit, as a host or inside one.
pub fn build_units(plies: &[PlyShape], material: &Material, groups: &[ItemGroup]) -> Vec<Unit> {
    let shapes: Vec<Shape> = groups
        .iter()
        .map(|g| {
            let s = &plies[g.shape_idx].shape;
            let outer = s.outer.flatten(TOL, FlattenMode::Enclosing);
            let mut holes: Vec<(Vec<Pt>, f64)> = s
                .holes
                .iter()
                .map(|h| {
                    let pts = h.flatten(TOL, FlattenMode::Inscribed);
                    let a = signed_area(&pts).abs();
                    (pts, a)
                })
                .collect();
            holes.sort_by(|a, b| b.1.total_cmp(&a.1));
            Shape { area: signed_area(&outer).abs(), outer, holes }
        })
        .collect();
    let smallest = shapes.iter().map(|s| s.area).fold(f64::INFINITY, f64::min);
    let mut left: Vec<u32> = groups.iter().map(ItemGroup::demand).collect();
    // Largest holes first, so they get the largest plies.
    let mut hosts: Vec<usize> = (0..groups.len()).filter(|&g| shapes[g].holes.first().is_some_and(|h| h.1 > smallest)).collect();
    hosts.sort_by(|&a, &b| shapes[b].holes[0].1.total_cmp(&shapes[a].holes[0].1));
    let mut units: Vec<Unit> = Vec::new();
    for host in hosts {
        let mut last: Option<(Vec<f64>, Vec<Inner>)> = None;
        while left[host] > 0 {
            left[host] -= 1;
            // Copies of one host usually get the same contents: reuse them while there are plies for it.
            let fill = match last.take() {
                Some(f) if can_take(&left, &f.1) => f,
                _ => fill_host(plies, material, groups, &shapes, &left, host),
            };
            if fill.1.is_empty() {
                left[host] += 1;
                break;
            }
            for i in &fill.1 {
                left[i.group] -= 1;
            }
            match units.last_mut() {
                Some(u) if u.group == host && u.rotations == fill.0 && u.inner == fill.1 => u.demand += 1,
                _ => units.push(Unit { group: host, rotations: fill.0.clone(), demand: 1, inner: fill.1.clone() }),
            }
            last = Some(fill);
        }
    }
    for (g, grp) in groups.iter().enumerate() {
        if left[g] > 0 {
            units.push(Unit { group: g, rotations: grp.rotations.clone(), demand: left[g], inner: vec![] });
        }
    }
    units
}

fn can_take(left: &[u32], inner: &[Inner]) -> bool {
    let mut need = vec![0u32; left.len()];
    for i in inner {
        need[i.group] += 1;
    }
    need.iter().zip(left).all(|(n, l)| n <= l)
}

/// Fills one copy of `host`: for each hole, the largest ply that fits, again
/// and again until none does. Returns the host's remaining rotations and the contents.
fn fill_host(plies: &[PlyShape], material: &Material, groups: &[ItemGroup], shapes: &[Shape], left: &[u32], host: usize) -> (Vec<f64>, Vec<Inner>) {
    let gap = material.spacing + MARGIN;
    let base = groups[host].rotations[0];
    let mut rotations = groups[host].rotations.clone();
    let mut avail = left.to_vec();
    let mut inner = Vec::new();
    let mut by_area: Vec<usize> = (0..groups.len()).collect();
    by_area.sort_by(|&a, &b| shapes[b].area.total_cmp(&shapes[a].area));
    for (hole, hole_area) in &shapes[host].holes {
        let ring = ring(hole);
        let mut placed: Vec<Placed> = Vec::new();
        let mut free = *hole_area;
        // A hole only fills up, so what did not fit once never fits later.
        let mut failed: Vec<(usize, usize)> = Vec::new();
        'more: loop {
            for &g in &by_area {
                if avail[g] == 0 || shapes[g].area >= free {
                    continue;
                }
                let ply = &plies[groups[g].shape_idx];
                let fibre = ply.fibre_deg(&ply.variants[groups[g].variants[0].0]);
                // Exact turns only; the host keeps the turns that keep this ply valid too.
                for (k, &a) in groups[g].rotations.iter().enumerate() {
                    if fibre_deviation(fibre, a, material, ply.cosmetic) > 1e-9 || failed.contains(&(g, k)) {
                        continue;
                    }
                    let turn = a - base;
                    let keep: Vec<f64> =
                        rotations.iter().copied().filter(|&r| groups[g].rotations.iter().any(|&b| ang_dist(r + turn, b) < 1e-6)).collect();
                    if keep.is_empty() {
                        continue;
                    }
                    match find_spot(hole, &ring, &placed, &shapes[g].outer, turn, gap) {
                        Some((tx, ty, poly)) => {
                            rotations = keep;
                            inner.push(Inner { group: g, rotation: turn, tx, ty });
                            avail[g] -= 1;
                            free -= shapes[g].area;
                            placed.push(poly);
                            continue 'more;
                        }
                        None => failed.push((g, k)),
                    }
                }
            }
            break;
        }
    }
    (rotations, inner)
}

/// A ply already in the hole.
struct Placed {
    pts: Vec<Pt>,
    bbox: BBox,
    poly: Polygon<f64>,
}

fn ring(pts: &[Pt]) -> LineString<f64> {
    let mut c: Vec<Coord<f64>> = pts.iter().map(|p| Coord { x: p.x, y: p.y }).collect();
    if let Some(&f) = c.first() {
        c.push(f);
    }
    LineString::from(c)
}

/// Lowest, then leftmost, position on a grid where `outer` turned by `turn`
/// lies inside the hole, at least `gap` from its edge and from the plies already in it.
fn find_spot(hole: &[Pt], hole_ring: &LineString<f64>, placed: &[Placed], outer: &[Pt], turn: f64, gap: f64) -> Option<(f64, f64, Placed)> {
    let r = Affine::rot_trans(turn, 0.0, 0.0);
    let p: Vec<Pt> = outer.iter().map(|&q| r.apply(q)).collect();
    let pb = BBox::of(&p);
    let hb = BBox::of(hole);
    let (x0, x1) = (hb.min.x + gap - pb.min.x, hb.max.x - gap - pb.max.x);
    let (y0, y1) = (hb.min.y + gap - pb.min.y, hb.max.y - gap - pb.max.y);
    if x1 < x0 || y1 < y0 {
        return None;
    }
    let grid = |lo: f64, hi: f64| {
        let n = if hi - lo < 1e-9 { 0 } else { STEPS };
        (0..=n).map(move |k| if n == 0 { lo } else { lo + (hi - lo) * k as f64 / n as f64 })
    };
    for ty in grid(y0, y1) {
        for tx in grid(x0, x1) {
            let qb = BBox { min: Pt::new(pb.min.x + tx, pb.min.y + ty), max: Pt::new(pb.max.x + tx, pb.max.y + ty) };
            let near: Vec<&Placed> = placed.iter().filter(|o| o.bbox.overlaps(&qb, gap)).collect();
            let q: Vec<Pt> = p.iter().map(|v| Pt::new(v.x + tx, v.y + ty)).collect();
            // Cheap rejections first: a corner inside another ply, or outside the hole.
            if near.iter().any(|o| point_in_polygon(o.pts[0], &q) || q.iter().any(|&v| v.x >= o.bbox.min.x && v.x <= o.bbox.max.x && v.y >= o.bbox.min.y && v.y <= o.bbox.max.y && point_in_polygon(v, &o.pts))) {
                continue;
            }
            if !q.iter().all(|&v| point_in_polygon(v, hole)) {
                continue;
            }
            let poly = Polygon::new(ring(&q), vec![]);
            if near.iter().any(|o| Euclidean.distance(&poly, &o.poly) < gap) || Euclidean.distance(poly.exterior(), hole_ring) < gap {
                continue;
            }
            return Some((tx, ty, Placed { pts: q, bbox: qb, poly }));
        }
    }
    None
}
