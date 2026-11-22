//! Splitting plies that are too wide for the roll.
//!
//! A split belongs to one angle variant of a ply. When nesting, a split
//! variant is replaced by one ply per piece (`skin [1/2]`, `skin [2/2]`), so
//! the solver, verification, export and labels need nothing special.
//!
//! Cuts are exact: straight edges and arcs are split where the cut line
//! crosses them, arcs stay arcs, holes the line crosses become part of the
//! piece outlines, and marks are clipped to the pieces.

use crate::geom::{Affine, BBox, FlattenMode, Path, Pt, Vtx, arc_from_bulge, bulge_from_sweep, point_in_polygon, signed_area};
use crate::model::{Material, PlyShape, Shape, Variant};
use crate::orient::allowed_rotations;
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

/// Most cut lines in one split.
pub const MAX_LINES: usize = 20;
/// Largest overlap at a cut, mm.
pub const MAX_OVERLAP: f64 = 100.0;
/// Automatic cuts keep pieces this much under the usable width, mm, so the
/// solver's slightly larger outline still fits.
pub const AUTO_MARGIN: f64 = 0.5;
/// Flattening used for widths, as the solver flattens (enclosing, 0.05 mm).
const FIT_TOL: f64 = 0.05;
const EPS: f64 = 1e-7;

/// An infinite cut line through `p` in direction `dir_deg`, drawing coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CutLine {
    pub p: Pt,
    pub dir_deg: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "kebab-case")]
pub enum Split {
    /// Cuts along the fibre, as few pieces as fit (or `pieces`), each cut
    /// where it is shortest. Recalculated for the material at hand.
    Auto {
        #[serde(default)]
        overlap: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pieces: Option<u32>,
    },
    /// Cut lines placed by hand.
    Manual {
        lines: Vec<CutLine>,
        #[serde(default)]
        overlap: f64,
    },
}

impl Split {
    pub fn overlap(&self) -> f64 {
        match self {
            Split::Auto { overlap, .. } | Split::Manual { overlap, .. } => *overlap,
        }
    }

    pub fn validate(&self) -> Result<()> {
        let o = self.overlap();
        ensure!(o.is_finite() && (0.0..=MAX_OVERLAP).contains(&o), "the overlap must be between 0 and {MAX_OVERLAP} mm");
        match self {
            Split::Auto { pieces, .. } => {
                if let Some(n) = pieces {
                    ensure!((1..=MAX_LINES as u32 + 1).contains(n), "a ply can be split into 1 to {} pieces", MAX_LINES + 1);
                }
            }
            Split::Manual { lines, .. } => {
                ensure!(lines.len() <= MAX_LINES, "a split has at most {MAX_LINES} cut lines");
                for l in lines {
                    ensure!(l.p.x.is_finite() && l.p.y.is_finite() && l.dir_deg.is_finite(), "a cut line is not valid");
                    ensure!(l.p.x.abs() < 1e7 && l.p.y.abs() < 1e7, "a cut line is too far from the ply");
                }
            }
        }
        Ok(())
    }

    /// The same split, with hand-placed lines turned back into automatic ones.
    /// The piece count is kept, so a split of a ply that fits the roll (made to
    /// nest it better) does not disappear.
    pub fn relaxed(&self) -> Split {
        match self {
            Split::Manual { overlap, lines } => {
                Split::Auto { overlap: *overlap, pieces: (!lines.is_empty()).then(|| lines.len().min(MAX_LINES) as u32 + 1) }
            }
            s => s.clone(),
        }
    }
}

// ---------------------------------------------------------------- fit

/// Width across the roll of an outline turned by `rotation_deg`.
fn height_at(pts: &[Pt], rotation_deg: f64) -> f64 {
    let t = Affine::rot_trans(rotation_deg, 0.0, 0.0);
    BBox::of(&pts.iter().map(|&p| t.apply(p)).collect::<Vec<_>>()).height()
}

/// Narrowest width across the roll of `outer` over the turns the fibre rules
/// allow for fibre direction `fibre_deg`, mm.
pub fn width_across(outer: &Path, fibre_deg: f64, material: &Material, cosmetic: bool) -> f64 {
    let pts = outer.flatten(FIT_TOL, FlattenMode::Enclosing);
    allowed_rotations(fibre_deg, material, cosmetic).into_iter().map(|r| height_at(&pts, r)).fold(f64::INFINITY, f64::min)
}

/// Does the outline fit across the usable roll width in some allowed turn?
pub fn fits(outer: &Path, fibre_deg: f64, material: &Material, cosmetic: bool) -> bool {
    width_across(outer, fibre_deg, material, cosmetic) <= material.usable_width() + 1e-6
}

// ---------------------------------------------------------------- cutting

/// One segment: straight (`bulge` 0) or a circular arc.
#[derive(Clone, Copy, Debug)]
struct Seg {
    a: Pt,
    b: Pt,
    bulge: f64,
}

impl Seg {
    fn mid(&self) -> Pt {
        match arc_from_bulge(self.a, self.b, self.bulge) {
            Some(arc) => arc.point_at(arc.start_angle + arc.sweep / 2.0),
            None => Pt::new((self.a.x + self.b.x) / 2.0, (self.a.y + self.b.y) / 2.0),
        }
    }

    /// Splits at parameters `us` in (0, 1) (sorted), along the segment.
    /// `at` maps a parameter to its exact point.
    fn split(&self, us: &[f64], line: &Line) -> Vec<Seg> {
        if us.is_empty() {
            return vec![*self];
        }
        let arc = arc_from_bulge(self.a, self.b, self.bulge);
        let point = |u: f64| -> Pt {
            let p = match arc {
                Some(arc) => arc.point_at(arc.start_angle + u * arc.sweep),
                None => self.a.add(self.b.sub(self.a).scale(u)),
            };
            line.project(p)
        };
        let mut out = Vec::with_capacity(us.len() + 1);
        let mut prev_u = 0.0;
        let mut prev_p = self.a;
        for &u in us.iter().chain(std::iter::once(&1.0)) {
            let p = if u >= 1.0 { self.b } else { point(u) };
            let bulge = match arc {
                Some(arc) => bulge_from_sweep(arc.sweep * (u - prev_u)),
                None => 0.0,
            };
            out.push(Seg { a: prev_p, b: p, bulge });
            prev_u = u;
            prev_p = p;
        }
        out
    }
}

/// A directed line with unit direction; "left" is the side its normal points to.
#[derive(Clone, Copy, Debug)]
struct Line {
    p: Pt,
    d: Pt,
}

impl Line {
    fn new(p: Pt, dir_deg: f64) -> Line {
        let (s, c) = dir_deg.to_radians().sin_cos();
        Line { p, d: Pt::new(c, s) }
    }
    fn normal(&self) -> Pt {
        Pt::new(-self.d.y, self.d.x)
    }
    /// Signed distance, positive on the left.
    fn dist(&self, q: Pt) -> f64 {
        let v = q.sub(self.p);
        self.d.x * v.y - self.d.y * v.x
    }
    /// Position along the line.
    fn t(&self, q: Pt) -> f64 {
        let v = q.sub(self.p);
        self.d.x * v.x + self.d.y * v.y
    }
    fn project(&self, q: Pt) -> Pt {
        q.sub(self.normal().scale(self.dist(q)))
    }
    fn shifted(&self, by: f64) -> Line {
        Line { p: self.p.add(self.normal().scale(by)), d: self.d }
    }
    fn of(c: &CutLine) -> Line {
        Line::new(c.p, c.dir_deg)
    }

    /// Parameters in (0, 1) where `s` crosses the line, sorted.
    fn crossings(&self, s: &Seg) -> Vec<f64> {
        match arc_from_bulge(s.a, s.b, s.bulge) {
            None => {
                let (da, db) = (self.dist(s.a), self.dist(s.b));
                if (da > EPS && db < -EPS) || (da < -EPS && db > EPS) { vec![da / (da - db)] } else { vec![] }
            }
            Some(arc) => {
                let dc = self.dist(arc.center);
                if dc.abs() >= arc.radius {
                    return vec![];
                }
                let foot = arc.center.sub(self.normal().scale(dc));
                let h = (arc.radius * arc.radius - dc * dc).sqrt();
                let mut us: Vec<f64> = [foot.add(self.d.scale(h)), foot.sub(self.d.scale(h))]
                    .iter()
                    .map(|q| {
                        let phi = (q.y - arc.center.y).atan2(q.x - arc.center.x);
                        let off = ((phi - arc.start_angle) * arc.sweep.signum()).rem_euclid(2.0 * PI);
                        off / arc.sweep.abs()
                    })
                    .filter(|&u| u > 1e-9 && u < 1.0 - 1e-9)
                    .collect();
                us.sort_by(f64::total_cmp);
                us.dedup_by(|a, b| (*a - *b).abs() < 1e-12);
                us
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Side {
    Left,
    Right,
}

/// An outline with its holes.
#[derive(Clone, Debug)]
struct Region {
    outer: Path,
    holes: Vec<Path>,
}

fn segs_of(p: &Path) -> Vec<Seg> {
    (0..p.seg_count()).map(|i| p.seg(i)).map(|(a, b, bulge)| Seg { a, b, bulge }).filter(|s| s.a.dist(s.b) > 1e-12).collect()
}

fn path_of(segs: &[Seg]) -> Path {
    Path::new(segs.iter().map(|s| Vtx { p: s.a, bulge: s.bulge }).collect(), true)
}

fn side_of(line: &Line, s: &Seg) -> Side {
    let d = line.dist(s.mid());
    if d > 1e-6 {
        Side::Left
    } else if d < -1e-6 {
        Side::Right
    } else {
        // Along the line: the region lies to the left of its boundary, so an
        // edge running with the line belongs to the left piece.
        let v = s.b.sub(s.a);
        if v.x * line.d.x + v.y * line.d.y > 0.0 { Side::Left } else { Side::Right }
    }
}

struct Chain {
    side: Side,
    segs: Vec<Seg>,
    /// Crossing indices where the chain starts and ends.
    from: usize,
    to: usize,
}

fn loop_area(p: &Path) -> f64 {
    signed_area(&p.flatten(0.05, FlattenMode::Inscribed))
}

/// Cuts a region by a line. Returns the pieces on the left and on the right.
fn cut(region: &Region, line: &Line) -> Result<(Vec<Region>, Vec<Region>)> {
    let mut whole: Vec<(Side, Path)> = Vec::new();
    let mut chains: Vec<Chain> = Vec::new();
    // Crossing points: position along the line and the point itself.
    let mut xs: Vec<(f64, Pt)> = Vec::new();
    for contour in std::iter::once(&region.outer).chain(region.holes.iter()) {
        let mut subs: Vec<(Side, Seg)> = Vec::new();
        for s in segs_of(contour) {
            for piece in s.split(&line.crossings(&s), line) {
                if piece.a.dist(piece.b) > 1e-9 {
                    subs.push((side_of(line, &piece), piece));
                }
            }
        }
        if subs.is_empty() {
            continue;
        }
        let n = subs.len();
        let Some(first_change) = (0..n).find(|&i| subs[i].0 != subs[(i + n - 1) % n].0) else {
            whole.push((subs[0].0, contour.clone()));
            continue;
        };
        // Walk from the first change; every side change is a crossing.
        let first_crossing = xs.len();
        let mut cur: Option<Chain> = None;
        for k in 0..n {
            let (side, seg) = subs[(first_change + k) % n];
            if cur.as_ref().is_none_or(|c| c.side != side) {
                xs.push((line.t(seg.a), seg.a));
                let id = xs.len() - 1;
                if let Some(mut c) = cur.take() {
                    c.to = id;
                    chains.push(c);
                }
                cur = Some(Chain { side, segs: vec![], from: id, to: usize::MAX });
            }
            cur.as_mut().unwrap().segs.push(seg);
        }
        // The last chain ends where the first one started.
        let mut last = cur.unwrap();
        last.to = first_crossing;
        chains.push(last);
    }

    let order: Vec<usize> = {
        let mut o: Vec<usize> = (0..xs.len()).collect();
        o.sort_by(|&a, &b| xs[a].0.total_cmp(&xs[b].0));
        o
    };
    let rank: Vec<usize> = {
        let mut r = vec![0; xs.len()];
        for (k, &i) in order.iter().enumerate() {
            r[i] = k;
        }
        r
    };

    let mut sides = (Vec::new(), Vec::new());
    for side in [Side::Left, Side::Right] {
        let mut loops: Vec<Path> = whole.iter().filter(|w| w.0 == side).map(|w| w.1.clone()).collect();
        let starts: std::collections::HashMap<usize, usize> =
            chains.iter().enumerate().filter(|(_, c)| c.side == side).map(|(i, c)| (c.from, i)).collect();
        let mut used = vec![false; chains.len()];
        for first in 0..chains.len() {
            if used[first] || chains[first].side != side {
                continue;
            }
            let mut segs: Vec<Seg> = Vec::new();
            let mut ci = first;
            let mut guard = 0;
            loop {
                ensure!(!used[ci], "a cut line touches the ply in a way that cannot be split; move it slightly");
                used[ci] = true;
                segs.extend_from_slice(&chains[ci].segs);
                // Along the line to the next crossing: forwards for the left
                // side, backwards for the right.
                let r = rank[chains[ci].to];
                let next = match side {
                    Side::Left => order.get(r + 1),
                    Side::Right => r.checked_sub(1).map(|k| &order[k]),
                };
                let Some(&next) = next else { bail!("a cut line touches the ply in a way that cannot be split; move it slightly") };
                let Some(&nc) = starts.get(&next) else { bail!("a cut line touches the ply in a way that cannot be split; move it slightly") };
                let (from, to) = (xs[chains[ci].to].1, xs[next].1);
                if from.dist(to) > 1e-9 {
                    segs.push(Seg { a: from, b: to, bulge: 0.0 });
                }
                if nc == first {
                    break;
                }
                ci = nc;
                guard += 1;
                ensure!(guard <= chains.len(), "cutting the ply went wrong; move the cut line slightly");
            }
            loops.push(path_of(&segs));
        }
        let regions = assemble(loops);
        match side {
            Side::Left => sides.0 = regions,
            Side::Right => sides.1 = regions,
        }
    }
    Ok(sides)
}

/// Sorts closed loops into outlines (counter-clockwise) with their holes.
fn assemble(loops: Vec<Path>) -> Vec<Region> {
    let mut outers: Vec<(Path, Vec<Pt>, f64)> = Vec::new();
    let mut holes: Vec<(Path, f64)> = Vec::new();
    for l in loops {
        let a = loop_area(&l);
        if a > 1e-6 {
            let pts = l.flatten(0.05, FlattenMode::Inscribed);
            outers.push((l, pts, a));
        } else if a < -1e-6 {
            holes.push((l, a));
        }
    }
    let mut regions: Vec<Region> = outers.iter().map(|o| Region { outer: o.0.clone(), holes: vec![] }).collect();
    for (h, _) in holes {
        // A point of the hole that is not on its own boundary corner: a segment middle.
        let probe = segs_of(&h).first().map(Seg::mid).unwrap_or(h.start());
        let best = outers
            .iter()
            .enumerate()
            .filter(|(_, o)| point_in_polygon(probe, &o.1))
            .min_by(|a, b| a.1.2.total_cmp(&b.1.2))
            .map(|(i, _)| i);
        if let Some(i) = best {
            regions[i].holes.push(h);
        }
    }
    regions
}

/// Splits marks (open or closed paths) at the cut lines and keeps each part
/// in the pieces it lies in.
fn clip_marks(marks: &[Path], lines: &[Line], pieces: &[Region]) -> Vec<Vec<Path>> {
    let polys: Vec<(Vec<Pt>, Vec<Vec<Pt>>)> = pieces
        .iter()
        .map(|r| (r.outer.flatten(0.05, FlattenMode::Inscribed), r.holes.iter().map(|h| h.flatten(0.05, FlattenMode::Inscribed)).collect()))
        .collect();
    let inside = |k: usize, q: Pt| point_in_polygon(q, &polys[k].0) && !polys[k].1.iter().any(|h| point_in_polygon(q, h));
    let mut out = vec![Vec::new(); pieces.len()];
    for m in marks {
        let mut segs: Vec<Seg> = (0..m.seg_count()).map(|i| m.seg(i)).map(|(a, b, bulge)| Seg { a, b, bulge }).collect();
        for l in lines {
            segs = segs.iter().flat_map(|s| s.split(&l.crossings(s), l)).collect();
        }
        for k in 0..pieces.len() {
            let mut run: Vec<Seg> = Vec::new();
            let flush = |run: &mut Vec<Seg>, out: &mut Vec<Path>| {
                if !run.is_empty() {
                    let mut verts: Vec<Vtx> = run.iter().map(|s| Vtx { p: s.a, bulge: s.bulge }).collect();
                    verts.push(Vtx { p: run.last().unwrap().b, bulge: 0.0 });
                    out.push(Path::new(verts, false));
                    run.clear();
                }
            };
            let all_in = segs.iter().all(|s| inside(k, s.mid()));
            if all_in && m.closed && !segs.is_empty() {
                out[k].push(m.clone());
                continue;
            }
            for s in &segs {
                if inside(k, s.mid()) {
                    if run.last().is_some_and(|r: &Seg| r.b.dist(s.a) > 1e-9) {
                        flush(&mut run, &mut out[k]);
                    }
                    run.push(*s);
                } else {
                    flush(&mut run, &mut out[k]);
                }
            }
            flush(&mut run, &mut out[k]);
        }
    }
    out
}

/// Cuts a shape into pieces along `lines`. With an overlap, each piece reaches
/// half of it past every cut, so neighbouring pieces overlap by `overlap`.
/// Pieces are ordered across the roll for fibre direction `fibre_deg`, the
/// top one (in the roll view) first.
pub fn split_shape(shape: &Shape, lines: &[CutLine], overlap: f64, fibre_deg: f64) -> Result<Vec<Shape>> {
    let mut regions = vec![Region { outer: shape.outer.clone(), holes: shape.holes.clone() }];
    let ls: Vec<Line> = lines.iter().map(Line::of).collect();
    for l in &ls {
        let mut next = Vec::new();
        for r in &regions {
            let (left, _) = cut(r, &l.shifted(-overlap / 2.0))?;
            let (_, right) = cut(r, &l.shifted(overlap / 2.0))?;
            next.extend(left);
            next.extend(right);
        }
        regions = next;
    }
    // Order across the roll: in the roll view the fibre runs along x.
    let to_roll = Affine::rot_trans(-fibre_deg, 0.0, 0.0);
    let key = |r: &Region| {
        let b = BBox::of(&r.outer.flatten(0.1, FlattenMode::Inscribed).iter().map(|&p| to_roll.apply(p)).collect::<Vec<_>>());
        (-(b.min.y + b.max.y), b.min.x + b.max.x)
    };
    regions.sort_by(|a, b| {
        let (ka, kb) = (key(a), key(b));
        ka.0.total_cmp(&kb.0).then(ka.1.total_cmp(&kb.1))
    });
    let marks = clip_marks(&shape.marks, &ls, &regions);
    Ok(regions
        .into_iter()
        .zip(marks)
        .map(|(r, marks)| Shape { outer: r.outer, holes: r.holes, marks, ..shape.clone() })
        .collect())
}

// ---------------------------------------------------------------- automatic lines

/// Length of the horizontal line y = `y` inside a polygon set (outline and holes).
fn chord(polys: &[Vec<Pt>], y: f64) -> f64 {
    let mut xs: Vec<f64> = Vec::new();
    for poly in polys {
        let n = poly.len();
        for i in 0..n {
            let (a, b) = (poly[i], poly[(i + 1) % n]);
            if (a.y > y) != (b.y > y) {
                xs.push(a.x + (y - a.y) * (b.x - a.x) / (b.y - a.y));
            }
        }
    }
    xs.sort_by(f64::total_cmp);
    xs.chunks(2).map(|c| if c.len() == 2 { c[1] - c[0] } else { 0.0 }).sum()
}

/// The cut lines an automatic split uses for this shape, fibre direction and
/// material: along the fibre, as few pieces as fit (or `pieces`), each cut
/// where the cut through the ply is shortest. Empty when no cut is needed.
pub fn auto_lines(shape: &Shape, fibre_deg: f64, material: &Material, overlap: f64, pieces: Option<u32>) -> Result<Vec<CutLine>> {
    // Roll view: fibre along x.
    let to_roll = Affine::rot_trans(-fibre_deg, 0.0, 0.0);
    let back = Affine::rot_trans(fibre_deg, 0.0, 0.0);
    let flat = |p: &Path| p.flatten(FIT_TOL, FlattenMode::Inscribed).iter().map(|&q| to_roll.apply(q)).collect::<Vec<Pt>>();
    let mut polys = vec![flat(&shape.outer)];
    polys.extend(shape.holes.iter().map(flat));
    let enclosing: Vec<Pt> = shape.outer.flatten(FIT_TOL, FlattenMode::Enclosing).iter().map(|&q| to_roll.apply(q)).collect();
    let b = BBox::of(&enclosing);
    let (y0, y1) = (b.min.y, b.max.y);
    let w = y1 - y0;
    let usable = material.usable_width() - AUTO_MARGIN;
    let fit_n = if w <= material.usable_width() + 1e-6 {
        1
    } else {
        ensure!(usable > overlap + 1.0, "the overlap ({overlap} mm) leaves no room on a {:.0} mm usable roll", material.usable_width());
        ((w - overlap) / (usable - overlap)).ceil().max(2.0) as u32
    };
    let n = pieces.unwrap_or(fit_n).max(fit_n);
    ensure!(n as usize <= MAX_LINES + 1, "this ply would need {n} pieces; PlyRoll splits into at most {}", MAX_LINES + 1);
    if n <= 1 {
        return Ok(vec![]);
    }
    // Equal pieces first: every piece `p` wide, end pieces reach o/2 past
    // their cut, middle pieces o/2 past both.
    let p = (w + (n - 1) as f64 * overlap) / n as f64;
    // Virtual lines at the ends make every piece width `c[k] - c[k-1] + o`.
    let mut c: Vec<f64> = (0..=n).map(|k| y0 + overlap / 2.0 + k as f64 * (p - overlap)).collect();
    c[n as usize] = y1 - overlap / 2.0;
    // More pieces than needed: keep them close to even (narrower pieces were asked for).
    let (min_w, max_w) = if n > fit_n { ((p * 0.85).max(overlap + 1.0), usable.min(p * 1.15)) } else { ((p * 0.4).max(overlap + 1.0), usable.max(p)) };
    for _ in 0..3 {
        for k in 1..n as usize {
            let lo = (c[k - 1] + min_w - overlap).max(c[k + 1] - (max_w - overlap));
            let hi = (c[k - 1] + max_w - overlap).min(c[k + 1] - (min_w - overlap));
            if hi <= lo {
                continue;
            }
            let steps = 80;
            let mut best = (chord(&polys, c[k]), (c[k] - (lo + hi) / 2.0).abs(), c[k]);
            for s in 0..=steps {
                let y = lo + (hi - lo) * s as f64 / steps as f64;
                let cand = (chord(&polys, y), (y - (lo + hi) / 2.0).abs(), y);
                // Shorter cut wins; within 0.5 mm, the one nearer the middle of its range.
                if cand.0 < best.0 - 0.5 || ((cand.0 - best.0).abs() <= 0.5 && cand.1 < best.1) {
                    best = cand;
                }
            }
            c[k] = best.2;
        }
    }
    let cx = (b.min.x + b.max.x) / 2.0;
    Ok(c[1..n as usize].iter().map(|&y| CutLine { p: back.apply(Pt::new(cx, y)), dir_deg: fibre_deg }).collect())
}

/// The cut lines a split uses on this ply and variant.
pub fn lines_for(ply: &PlyShape, v: &Variant, split: &Split, material: &Material) -> Result<Vec<CutLine>> {
    match split {
        Split::Auto { overlap, pieces } => auto_lines(&ply.shape, ply.fibre_deg(v), material, *overlap, *pieces),
        Split::Manual { lines, .. } => Ok(lines.clone()),
    }
}

/// The pieces of a split variant, or `None` when it is not split (or no
/// cut line crosses it).
pub fn pieces(ply: &PlyShape, v: &Variant, material: &Material) -> Result<Option<Vec<Shape>>> {
    let Some(split) = &v.split else { return Ok(None) };
    split.validate()?;
    let lines = lines_for(ply, v, split, material)?;
    if lines.is_empty() {
        return Ok(None);
    }
    let pieces = split_shape(&ply.shape, &lines, split.overlap(), ply.fibre_deg(v))?;
    Ok((pieces.len() > 1).then_some(pieces))
}

/// Name of piece `k` (1-based) of `n`.
pub fn piece_name(name: &str, k: usize, n: usize) -> String {
    format!("{name} [{k}/{n}]")
}

/// Replaces every split variant with one ply per piece, each with that
/// variant alone. `ids` (one per ply) are repeated for the pieces.
pub fn expand(plies: &[PlyShape], ids: &[u64], material: &Material) -> Result<(Vec<PlyShape>, Vec<u64>)> {
    let mut out = Vec::new();
    let mut out_ids = Vec::new();
    for (k, ply) in plies.iter().enumerate() {
        let mut keep = ply.clone();
        keep.variants.clear();
        let mut extra = Vec::new();
        for v in &ply.variants {
            match pieces(ply, v, material).map_err(|e| e.context(format!("{} {:+}°", ply.shape.name, v.angle_deg)))? {
                None => keep.variants.push(Variant { split: None, ..v.clone() }),
                Some(ps) => {
                    let n = ps.len();
                    for (i, shape) in ps.into_iter().enumerate() {
                        extra.push(PlyShape {
                            shape: Shape { name: piece_name(&ply.shape.name, i + 1, n), ..shape },
                            variants: vec![Variant { split: None, ..v.clone() }],
                            ..ply.clone()
                        });
                    }
                }
            }
        }
        let id = ids.get(k).copied().unwrap_or(0);
        if !keep.variants.is_empty() {
            out.push(keep);
            out_ids.push(id);
        }
        for e in extra {
            out.push(e);
            out_ids.push(id);
        }
    }
    Ok((out, out_ids))
}

/// Plies (and their variants) that do not fit across the roll, as
/// "name angle (width mm)".
pub fn too_wide(plies: &[PlyShape], material: &Material) -> Vec<String> {
    let mut out = Vec::new();
    for p in plies {
        for v in &p.variants {
            if v.qty == 0 {
                continue;
            }
            let w = width_across(&p.shape.outer, p.fibre_deg(v), material, p.cosmetic);
            if w > material.usable_width() + 1e-6 {
                out.push(format!("{} {:+}° ({w:.0} mm)", p.shape.name, v.angle_deg));
            }
        }
    }
    out
}

/// Sets an automatic split on every variant that does not fit and has no
/// split yet. Returns how many variants it split.
pub fn auto_split_plies(plies: &mut [PlyShape], material: &Material) -> usize {
    let mut n = 0;
    for p in plies.iter_mut() {
        let (outer, ref_deg, cosmetic) = (p.shape.outer.clone(), p.ref_deg, p.cosmetic);
        for v in &mut p.variants {
            if v.split.is_none() && !fits(&outer, ref_deg + v.angle_deg, material, cosmetic) {
                v.split = Some(Split::Auto { overlap: 0.0, pieces: None });
                n += 1;
            }
        }
    }
    n
}
