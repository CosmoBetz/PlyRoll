//! Plies inside the holes of other plies.
//!
//! The solver only knows outer outlines, so a hole is wasted material. Before
//! nesting, each copy of a ply with a hole gets other plies packed into it
//! (with the spacing kept and their fibres still along the roll). The host and
//! its contents then go to the solver as one item, with the host's outline,
//! and the contents are placed relative to the host after.
//!
//! A hole is packed greedily: the first ply in a given order that fits, at the
//! lowest, then leftmost, grid position, again and again. The whole job is
//! packed this way with several orders, and the run that takes the most room
//! off the roll is kept. A ply that lands in a hole and has holes itself gets
//! those filled too.

use crate::{ItemGroup, ang_dist};
use geo::{Area, ConvexHull, Coord, Distance, Euclidean, LineString, Polygon};
use plyroll_core::geom::{Affine, BBox, FlattenMode, Pt, norm_deg, point_in_polygon, signed_area};
use plyroll_core::model::{Material, PlyShape};
use plyroll_core::orient::fibre_deviation;
use rand::SeedableRng;
use rand::rngs::Xoshiro256PlusPlus;
use rand::seq::SliceRandom;

/// Kept on top of the spacing, mm, so the exact check always passes.
const MARGIN: f64 = 0.05;
/// Chord tolerance for the search outlines, mm (outlines enclosing, holes
/// inscribed, so the search is on the safe side).
const TOL: f64 = 0.3;
/// Grid steps across a hole, per axis.
const STEPS: usize = 48;
/// How deep plies go inside plies inside plies.
const MAX_DEPTH: usize = 3;
/// Shuffled orders tried, on top of largest, most awkward and smallest first.
const SHUFFLES: usize = 6;

/// One solver item: copies of a group, maybe with plies in their holes.
#[derive(Clone, Debug, PartialEq)]
pub struct Unit {
    /// The host's group.
    pub group: usize,
    /// Rotations allowed for the host with these contents.
    pub rotations: Vec<f64>,
    pub demand: u32,
    /// Plies in the host's holes, and in their holes.
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
    /// Convex hull area: about the room the ply takes up on the roll.
    hull: f64,
    /// Hull area times diameter: how hard the ply is to fit.
    awkward: f64,
    /// Holes, largest first.
    holes: Vec<Hole>,
}

struct Hole {
    pts: Vec<Pt>,
    area: f64,
    ring: LineString<f64>,
}

/// One unit per group, without filling any holes.
pub fn plain_units(groups: &[ItemGroup]) -> Vec<Unit> {
    groups.iter().enumerate().map(|(g, grp)| Unit { group: g, rotations: grp.rotations.clone(), demand: grp.demand(), inner: vec![] }).collect()
}

/// Fills holes with other plies where they fit and returns the solver items.
/// Every copy of every group is in exactly one unit, as a host or inside one.
///
/// What one hole takes, the next one lacks, so the orders are compared over
/// the whole job, by the hull area of everything inside.
pub fn build_units(plies: &[PlyShape], material: &Material, groups: &[ItemGroup]) -> Vec<Unit> {
    let shapes: Vec<Shape> = groups
        .iter()
        .map(|g| {
            let s = &plies[g.shape_idx].shape;
            let outer = s.outer.flatten(TOL, FlattenMode::Enclosing);
            let mut holes: Vec<Hole> = s
                .holes
                .iter()
                .map(|h| {
                    let pts = h.flatten(TOL, FlattenMode::Inscribed);
                    Hole { area: signed_area(&pts).abs(), ring: ring(&pts), pts }
                })
                .collect();
            holes.sort_by(|a, b| b.area.total_cmp(&a.area));
            let (hull, awkward) = hull_and_awkwardness(&outer);
            Shape { area: signed_area(&outer).abs(), hull, awkward, outer, holes }
        })
        .collect();
    let smallest = shapes.iter().map(|s| s.area).fold(f64::INFINITY, f64::min);
    // Largest holes first, so they get the largest plies.
    let mut hosts: Vec<usize> = (0..groups.len()).filter(|&g| shapes[g].holes.first().is_some_and(|h| h.area > smallest)).collect();
    if hosts.is_empty() {
        return plain_units(groups);
    }
    hosts.sort_by(|&a, &b| shapes[b].holes[0].area.total_cmp(&shapes[a].holes[0].area));

    let mut by_area: Vec<usize> = (0..groups.len()).collect();
    by_area.sort_by(|&a, &b| shapes[b].area.total_cmp(&shapes[a].area));
    let mut by_awkward = by_area.clone();
    by_awkward.sort_by(|&a, &b| shapes[b].awkward.total_cmp(&shapes[a].awkward));
    let smallest_first: Vec<usize> = by_area.iter().rev().copied().collect();
    let mut orders = vec![by_area.clone(), by_awkward, smallest_first];
    let mut rng = Xoshiro256PlusPlus::seed_from_u64(0x9e37_79b9);
    for _ in 0..SHUFFLES {
        let mut o = by_area.clone();
        o.shuffle(&mut rng);
        orders.push(o);
    }
    let mut seen: Vec<Vec<usize>> = Vec::new();
    orders.retain(|o| !seen.contains(o) && {
        seen.push(o.clone());
        true
    });

    let filler = Filler { plies, material, groups, shapes: &shapes, smallest };
    let mut best: Option<(f64, f64, Vec<Unit>)> = None;
    for order in &orders {
        let units = filler.fill_all(order, &hosts);
        let inside = units.iter().flat_map(|u| u.inner.iter().map(|i| (u.demand as f64, shapes[i.group].hull)));
        let (room, big) = inside.fold((0.0, 0.0), |(a, b), (n, x)| (a + n * x, b + n * x * x));
        // Near ties go to larger plies inside: small ones are easy to nest outside.
        let better = best.as_ref().is_none_or(|(br, bb, _)| room > br * 1.002 || (room > br * 0.998 && big > *bb));
        if better {
            best = Some((room, big, units));
        }
    }
    best.map(|b| b.2).unwrap_or_else(|| plain_units(groups))
}

fn can_take(left: &[u32], inner: &[Inner]) -> bool {
    let mut need = vec![0u32; left.len()];
    for i in inner {
        need[i.group] += 1;
    }
    need.iter().zip(left).all(|(n, l)| n <= l)
}

fn hull_and_awkwardness(pts: &[Pt]) -> (f64, f64) {
    let hull = Polygon::new(ring(pts), vec![]).convex_hull();
    let h = &hull.exterior().0;
    let mut d: f64 = 0.0;
    for (i, a) in h.iter().enumerate() {
        for b in &h[i + 1..] {
            d = d.max((a.x - b.x).hypot(a.y - b.y));
        }
    }
    (hull.unsigned_area(), hull.unsigned_area() * d)
}

/// A ply already in the hole being filled.
struct Placed {
    pts: Vec<Pt>,
    bbox: BBox,
    poly: Polygon<f64>,
}

struct Filler<'a> {
    plies: &'a [PlyShape],
    material: &'a Material,
    groups: &'a [ItemGroup],
    shapes: &'a [Shape],
    smallest: f64,
}

impl Filler<'_> {
    /// One run: every copy of every host, largest holes first, plies tried in `order`.
    fn fill_all(&self, order: &[usize], hosts: &[usize]) -> Vec<Unit> {
        let mut left: Vec<u32> = self.groups.iter().map(ItemGroup::demand).collect();
        let mut units: Vec<Unit> = Vec::new();
        for &host in hosts {
            let mut last: Option<(Vec<f64>, Vec<Inner>)> = None;
            while left[host] > 0 {
                left[host] -= 1;
                // Copies of one host usually get the same contents: reuse them while there are plies for it.
                let fill = match last.take() {
                    Some(f) if can_take(&left, &f.1) => f,
                    _ => {
                        let mut rotations = self.groups[host].rotations.clone();
                        let mut avail = left.clone();
                        let inner = self.fill_ply(order, host, 0.0, &mut rotations, &mut avail, 0);
                        (rotations, inner)
                    }
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
        for (g, grp) in self.groups.iter().enumerate() {
            if left[g] > 0 {
                units.push(Unit { group: g, rotations: grp.rotations.clone(), demand: left[g], inner: vec![] });
            }
        }
        units
    }

    /// Fills the holes of ply `g`, which sits `frame` degrees turned against
    /// the host. Contents are in `g`'s drawing.
    fn fill_ply(&self, order: &[usize], g: usize, frame: f64, rotations: &mut Vec<f64>, avail: &mut [u32], depth: usize) -> Vec<Inner> {
        let mut out = Vec::new();
        for hole in &self.shapes[g].holes {
            if hole.area <= self.smallest {
                break;
            }
            out.extend(self.pack(order, hole, frame, rotations, avail, depth));
        }
        out
    }

    /// Greedy fill of one hole: the first ply in `order` that fits, again and
    /// again. A ply with holes gets them filled as soon as it is placed: room
    /// there is lost otherwise. Narrows `rotations` and takes from `avail`;
    /// returns the plies placed.
    fn pack(&self, order: &[usize], hole: &Hole, frame: f64, rotations: &mut Vec<f64>, avail: &mut [u32], depth: usize) -> Vec<Inner> {
        // Turns are measured from the host's first rotation. It always stays
        // allowed (it is where each ply inside has exactly its own angle).
        let host = rotations[0];
        let gap = self.material.spacing + MARGIN;
        let mut inner = Vec::new();
        let mut placed: Vec<Placed> = Vec::new();
        let mut free = hole.area;
        // A hole only fills up, so what did not fit once never fits later.
        let mut failed: Vec<(usize, usize)> = Vec::new();
        'more: loop {
            for &c in order {
                if avail[c] == 0 || self.shapes[c].area >= free {
                    continue;
                }
                let ply = &self.plies[self.groups[c].shape_idx];
                let fibre = ply.fibre_deg(&ply.variants[self.groups[c].variants[0].0]);
                // Exact turns only; the host keeps the turns that keep this ply valid too.
                for (k, &a) in self.groups[c].rotations.iter().enumerate() {
                    if fibre_deviation(fibre, a, self.material, ply.cosmetic) > 1e-9 || failed.contains(&(c, k)) {
                        continue;
                    }
                    let keep: Vec<f64> = rotations.iter().copied().filter(|&r| self.groups[c].rotations.iter().any(|&b| ang_dist(r + (a - host), b) < 1e-6)).collect();
                    if keep.is_empty() {
                        continue;
                    }
                    // Turn against the drawing of the ply that has the hole.
                    let turn = norm_deg(a - host - frame);
                    match find_spot(hole, &placed, &self.shapes[c].outer, turn, gap) {
                        Some((tx, ty, poly)) => {
                            *rotations = keep;
                            inner.push(Inner { group: c, rotation: turn, tx, ty });
                            avail[c] -= 1;
                            free -= self.shapes[c].area;
                            placed.push(poly);
                            if depth + 1 < MAX_DEPTH {
                                let place = Affine::rot_trans(turn, tx, ty);
                                for q in self.fill_ply(order, c, frame + turn, rotations, avail, depth + 1) {
                                    let t = place.apply(Pt::new(q.tx, q.ty));
                                    inner.push(Inner { group: q.group, rotation: turn + q.rotation, tx: t.x, ty: t.y });
                                }
                            }
                            continue 'more;
                        }
                        None => failed.push((c, k)),
                    }
                }
            }
            break;
        }
        inner
    }
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
fn find_spot(hole: &Hole, placed: &[Placed], outer: &[Pt], turn: f64, gap: f64) -> Option<(f64, f64, Placed)> {
    let r = Affine::rot_trans(turn, 0.0, 0.0);
    let p: Vec<Pt> = outer.iter().map(|&q| r.apply(q)).collect();
    let pb = BBox::of(&p);
    let hb = BBox::of(&hole.pts);
    let (x0, x1) = (hb.min.x + gap - pb.min.x, hb.max.x - gap - pb.max.x);
    let (y0, y1) = (hb.min.y + gap - pb.min.y, hb.max.y - gap - pb.max.y);
    if x1 < x0 || y1 < y0 {
        return None;
    }
    // The grid's edges sit exactly `gap` from the hole's: check with a little
    // less, or rounding turns snug fits away.
    let check = gap - MARGIN / 2.0;
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
            if !q.iter().all(|&v| point_in_polygon(v, &hole.pts)) {
                continue;
            }
            let poly = Polygon::new(ring(&q), vec![]);
            if near.iter().any(|o| Euclidean.distance(&poly, &o.poly) < check) || Euclidean.distance(poly.exterior(), &hole.ring) < check {
                continue;
            }
            return Some((tx, ty, Placed { pts: q, bbox: qb, poly }));
        }
    }
    None
}
