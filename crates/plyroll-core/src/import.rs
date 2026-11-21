//! DXF import: entities -> exact paths -> closed contours -> part shapes.

use crate::geom::{Affine, FlattenMode, Path, Pt, Vtx, bulge_from_sweep, find_self_intersection, point_in_polygon, signed_area};
use crate::model::Shape;
use anyhow::{Context, Result, bail};
use dxf::Drawing;
use dxf::entities::{Entity, EntityType};
use dxf::enums::Units;
use std::collections::{BTreeMap, HashMap};
use std::f64::consts::{PI, TAU};
use std::path::Path as FsPath;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UnitsOverride {
    /// Use $INSUNITS from the file (mm when unset).
    Auto,
    Mm,
    Cm,
    M,
    Inch,
}

#[derive(Clone, Debug)]
pub struct LayerRules {
    pub fibre: Vec<String>,
    pub mark: Vec<String>,
    pub ignore: Vec<String>,
}

impl Default for LayerRules {
    fn default() -> Self {
        let v = |s: &[&str]| s.iter().map(|x| x.to_string()).collect();
        Self {
            fibre: v(&["FIBRE*", "FIBER*", "0DEG*", "ORIENT*", "FASER*"]),
            mark: v(&["MARK*", "ENGRAV*", "PEN*", "INK*"]),
            ignore: v(&["INFO*", "DIM*", "DEFPOINTS", "TEXT*", "ANNO*", "HIDDEN*", "CENTER*", "CONSTR*", "BORDER*", "TITLE*"]),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Cut,
    Mark,
    Fibre,
    Ignore,
}

fn glob_match(pat: &str, s: &str) -> bool {
    let (p, s) = (pat.to_ascii_uppercase(), s.to_ascii_uppercase());
    match p.strip_suffix('*') {
        Some(prefix) => s.starts_with(prefix),
        None => p == s,
    }
}

impl LayerRules {
    fn role(&self, layer: &str) -> Role {
        let any = |pats: &[String]| pats.iter().any(|p| glob_match(p, layer));
        if any(&self.fibre) {
            Role::Fibre
        } else if any(&self.mark) {
            Role::Mark
        } else if any(&self.ignore) {
            Role::Ignore
        } else {
            Role::Cut
        }
    }
}

#[derive(Clone, Debug)]
pub struct ImportOptions {
    pub units: UnitsOverride,
    /// Endpoints closer than this are treated as connected, mm.
    pub join_tol: f64,
    /// Larger gaps up to this size are bridged with a straight line (with a warning), mm.
    pub gap_tol: f64,
    /// Chord tolerance for splines and ellipses, mm.
    pub curve_tol: f64,
    pub layers: LayerRules,
    /// Mirror the drawing once (CAD shows the other face of the ply).
    pub mirror: bool,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self { units: UnitsOverride::Auto, join_tol: 0.01, gap_tol: 0.1, curve_tol: 0.01, layers: LayerRules::default(), mirror: false }
    }
}

#[derive(Debug, Default)]
pub struct ImportResult {
    pub shapes: Vec<Shape>,
    pub warnings: Vec<String>,
}

struct Piece {
    role: Role,
    path: Path,
}

struct Walker<'a> {
    opts: &'a ImportOptions,
    blocks: HashMap<String, &'a dxf::Block>,
    pieces: Vec<Piece>,
    warnings: Vec<String>,
    skipped: BTreeMap<&'static str, usize>,
}

fn read_units(file: &FsPath) -> Result<Units> {
    let drawing = Drawing::load_file(file).with_context(|| format!("cannot read DXF {}", file.display()))?;
    Ok(drawing.header.default_drawing_units)
}

/// mm per drawing unit for a file's `$INSUNITS` (unset counts as mm).
fn header_scale(u: Units) -> Option<f64> {
    match u {
        Units::Millimeters | Units::Unitless => Some(1.0),
        Units::Centimeters => Some(10.0),
        Units::Meters => Some(1000.0),
        Units::Inches => Some(25.4),
        Units::Feet => Some(304.8),
        _ => None,
    }
}

impl UnitsOverride {
    fn forced_scale(self) -> Option<f64> {
        match self {
            UnitsOverride::Auto => None,
            UnitsOverride::Mm => Some(1.0),
            UnitsOverride::Cm => Some(10.0),
            UnitsOverride::M => Some(1000.0),
            UnitsOverride::Inch => Some(25.4),
        }
    }
}

/// mm per drawing unit that `import_dxf` uses for this file with these units.
pub fn units_scale(units: UnitsOverride, file: &FsPath) -> Result<f64> {
    if let Some(s) = units.forced_scale() {
        return Ok(s);
    }
    let u = read_units(file)?;
    header_scale(u).ok_or_else(|| anyhow::anyhow!("unsupported drawing units {u:?} in {}", file.display()))
}

/// The units a DXF declares in `$INSUNITS`: "mm", "cm", "m", "inch", "ft",
/// "none" when unset, or the raw name of an unsupported unit.
pub fn file_units(file: &FsPath) -> Result<String> {
    Ok(match read_units(file)? {
        Units::Millimeters => "mm".into(),
        Units::Centimeters => "cm".into(),
        Units::Meters => "m".into(),
        Units::Inches => "inch".into(),
        Units::Feet => "ft".into(),
        Units::Unitless => "none".into(),
        other => format!("{other:?}"),
    })
}

pub fn import_dxf(file: &FsPath, opts: &ImportOptions) -> Result<ImportResult> {
    let drawing = Drawing::load_file(file).with_context(|| format!("cannot read DXF {}", file.display()))?;
    let mut warnings = Vec::new();

    let header = drawing.header.default_drawing_units;
    let scale = match opts.units.forced_scale() {
        Some(s) => s,
        None => {
            if header == Units::Unitless {
                warnings.push("no units in the file ($INSUNITS unset); read as mm".into());
            }
            match header_scale(header) {
                Some(s) => s,
                None => bail!("unsupported drawing units {header:?} in {}", file.display()),
            }
        }
    };
    let mut root = Affine::scale(scale, scale);
    if opts.mirror {
        root = root.then(Affine::mirror_x());
    }

    let mut w = Walker {
        opts,
        blocks: drawing.blocks().map(|b| (b.name.clone(), b)).collect(),
        pieces: Vec::new(),
        warnings: Vec::new(),
        skipped: BTreeMap::new(),
    };
    for e in drawing.entities() {
        if e.common.is_in_paper_space {
            continue;
        }
        w.entity(e, &root, None, 0);
    }
    warnings.append(&mut w.warnings);
    for (kind, n) in &w.skipped {
        warnings.push(format!("ignored {n} {kind} entit{}", if *n == 1 { "y" } else { "ies" }));
    }
    let pieces = std::mem::take(&mut w.pieces);
    let stem = file.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "part".into());
    let shapes = build_shapes(pieces, &stem, file, opts, &mut warnings)?;
    Ok(ImportResult { shapes, warnings })
}

impl<'a> Walker<'a> {
    fn entity(&mut self, e: &Entity, t: &Affine, parent_layer: Option<&str>, depth: usize) {
        let layer = match (e.common.layer.as_str(), parent_layer) {
            ("0", Some(p)) => p.to_string(),
            (l, _) => l.to_string(),
        };
        let role = self.opts.layers.role(&layer);
        if role == Role::Ignore {
            return;
        }
        let tol = self.opts.curve_tol;
        let add = |w: &mut Walker, p: Path, ocs_mirror: bool| {
            let local = if ocs_mirror { Affine::mirror_x().then(*t) } else { *t };
            let path = p.transformed(&local, tol);
            if path.verts.len() >= 2 || (path.closed && !path.verts.is_empty()) {
                w.pieces.push(Piece { role, path });
            }
        };
        match &e.specific {
            EntityType::Line(l) => {
                add(self, Path::from_points(&[Pt::new(l.p1.x, l.p1.y), Pt::new(l.p2.x, l.p2.y)], false), false);
            }
            EntityType::Arc(a) => {
                let (s, mut en) = (a.start_angle.to_radians(), a.end_angle.to_radians());
                while en <= s {
                    en += TAU;
                }
                let sweep = en - s;
                let c = Pt::new(a.center.x, a.center.y);
                let p0 = Pt::new(c.x + a.radius * s.cos(), c.y + a.radius * s.sin());
                let p1 = Pt::new(c.x + a.radius * en.cos(), c.y + a.radius * en.sin());
                let path = if sweep >= TAU - 1e-9 {
                    circle_path(c, a.radius)
                } else {
                    Path::new(vec![Vtx { p: p0, bulge: bulge_from_sweep(sweep) }, Vtx { p: p1, bulge: 0.0 }], false)
                };
                add(self, path, a.normal.z < 0.0);
            }
            EntityType::Circle(c) => {
                add(self, circle_path(Pt::new(c.center.x, c.center.y), c.radius), c.normal.z < 0.0);
            }
            EntityType::LwPolyline(p) => {
                let verts = p.vertices.iter().map(|v| Vtx { p: Pt::new(v.x, v.y), bulge: v.bulge }).collect();
                add(self, Path::new(verts, p.is_closed()), p.extrusion_direction.z < 0.0);
            }
            EntityType::Polyline(p) => {
                // Bits 8/16/64: 3D polyline, polygon mesh, polyface mesh.
                if p.flags & (16 | 64) != 0 {
                    *self.skipped.entry("mesh").or_default() += 1;
                    return;
                }
                let verts: Vec<Vtx> = p
                    .vertices()
                    .filter(|v| v.flags & 16 == 0)
                    .map(|v| Vtx { p: Pt::new(v.location.x, v.location.y), bulge: v.bulge })
                    .collect();
                add(self, Path::new(verts, p.is_closed()), p.normal.z < 0.0);
            }
            EntityType::Spline(s) => match spline_points(s, tol) {
                Ok((pts, closed, approx)) => {
                    if approx {
                        self.warnings.push(format!("spline on layer {layer} has only fit points; approximated with straight segments"));
                    }
                    add(self, Path::from_points(&pts, closed), false);
                }
                Err(err) => self.warnings.push(format!("skipped spline on layer {layer}: {err}")),
            },
            EntityType::Ellipse(el) => {
                add(self, ellipse_path(el, tol), false);
            }
            EntityType::Insert(ins) => {
                if depth > 16 {
                    self.warnings.push(format!("block nesting too deep at {}", ins.name));
                    return;
                }
                let Some(block) = self.blocks.get(&ins.name).copied() else {
                    self.warnings.push(format!("missing block definition {}", ins.name));
                    return;
                };
                let (cols, rows) = (ins.column_count.max(1), ins.row_count.max(1));
                for ci in 0..cols {
                    for ri in 0..rows {
                        let mut local = Affine::translate(-block.base_point.x, -block.base_point.y)
                            .then(Affine::scale(ins.x_scale_factor, ins.y_scale_factor))
                            .then(Affine::translate(ci as f64 * ins.column_spacing, ri as f64 * ins.row_spacing))
                            .then(Affine::rot_trans(ins.rotation, ins.location.x, ins.location.y));
                        if ins.extrusion_direction.z < 0.0 {
                            local = local.then(Affine::mirror_x());
                        }
                        let full = local.then(*t);
                        for be in &block.entities {
                            self.entity(be, &full, Some(&layer), depth + 1);
                        }
                    }
                }
            }
            EntityType::Text(_) | EntityType::MText(_) => *self.skipped.entry("text").or_default() += 1,
            EntityType::RotatedDimension(_) | EntityType::RadialDimension(_) | EntityType::DiameterDimension(_) | EntityType::AngularThreePointDimension(_) | EntityType::OrdinateDimension(_) | EntityType::Leader(_) => {
                *self.skipped.entry("dimension").or_default() += 1
            }
            EntityType::ModelPoint(_) => *self.skipped.entry("point").or_default() += 1,
            _ => *self.skipped.entry("other").or_default() += 1,
        }
    }
}

fn circle_path(c: Pt, r: f64) -> Path {
    Path::new(vec![Vtx { p: Pt::new(c.x + r, c.y), bulge: 1.0 }, Vtx { p: Pt::new(c.x - r, c.y), bulge: 1.0 }], true)
}

fn ellipse_path(el: &dxf::entities::Ellipse, tol: f64) -> Path {
    let c = Pt::new(el.center.x, el.center.y);
    let maj = Pt::new(el.major_axis.x, el.major_axis.y);
    let nz = if el.normal.z < 0.0 { -1.0 } else { 1.0 };
    let minor = Pt::new(-nz * maj.y, nz * maj.x).scale(el.minor_axis_ratio);
    let (s, mut e) = (el.start_parameter, el.end_parameter);
    while e <= s {
        e += TAU;
    }
    let closed = (e - s - TAU).abs() < 1e-6;
    let a = maj.len().max(1e-9);
    let step = if a > tol { (2.0 * (1.0 - tol / a).acos()).min(PI / 8.0) } else { PI / 8.0 };
    let n = (((e - s) / step).ceil() as usize).max(8);
    let pts: Vec<Pt> = (0..=n)
        .take(if closed { n } else { n + 1 })
        .map(|k| {
            let t = s + (e - s) * k as f64 / n as f64;
            c.add(maj.scale(t.cos())).add(minor.scale(t.sin()))
        })
        .collect();
    Path::from_points(&pts, closed)
}

/// Evaluates a (rational) B-spline. Returns points, closed flag, and whether
/// the result is only a fit-point approximation.
fn spline_points(s: &dxf::entities::Spline, tol: f64) -> Result<(Vec<Pt>, bool, bool)> {
    let deg = s.degree_of_curve.max(1) as usize;
    let ctrl: Vec<Pt> = s.control_points.iter().map(|p| Pt::new(p.x, p.y)).collect();
    if ctrl.len() < deg + 1 {
        if s.fit_points.len() >= 2 {
            let pts: Vec<Pt> = s.fit_points.iter().map(|p| Pt::new(p.x, p.y)).collect();
            let closed = pts.len() > 2 && pts[0].dist(pts[pts.len() - 1]) < 1e-9;
            return Ok((pts, closed, true));
        }
        bail!("not enough control points");
    }
    let knots = &s.knot_values;
    if knots.len() != ctrl.len() + deg + 1 {
        bail!("knot vector length {} does not match {} control points of degree {deg}", knots.len(), ctrl.len());
    }
    let weights: Vec<f64> = if s.weight_values.len() == ctrl.len() { s.weight_values.clone() } else { vec![1.0; ctrl.len()] };
    let (u0, u1) = (knots[deg], knots[ctrl.len()]);
    if u1 <= u0 {
        bail!("empty parameter range");
    }
    let eval = |u: f64| nurbs_point(deg, knots, &ctrl, &weights, u);
    // Distinct knot spans, each refined adaptively.
    let mut breaks: Vec<f64> = knots[deg..=ctrl.len()].to_vec();
    breaks.dedup_by(|a, b| (*a - *b).abs() < 1e-12);
    let mut pts = vec![eval(u0)];
    for w in breaks.windows(2) {
        let n0 = 4;
        for k in 0..n0 {
            let a = w[0] + (w[1] - w[0]) * k as f64 / n0 as f64;
            let b = w[0] + (w[1] - w[0]) * (k + 1) as f64 / n0 as f64;
            refine(&eval, a, b, eval(a), eval(b), tol, 0, &mut pts);
        }
    }
    let closed = pts.len() > 2 && pts[0].dist(pts[pts.len() - 1]) < 1e-6;
    if closed {
        pts.pop();
    }
    Ok((pts, closed, false))
}

fn refine(eval: &dyn Fn(f64) -> Pt, a: f64, b: f64, pa: Pt, pb: Pt, tol: f64, depth: usize, out: &mut Vec<Pt>) {
    let m = (a + b) / 2.0;
    let pm = eval(m);
    let q1 = eval(a + (b - a) * 0.25);
    let q3 = eval(a + (b - a) * 0.75);
    let dev = [pm, q1, q3].iter().map(|q| dist_to_segment(*q, pa, pb)).fold(0.0, f64::max);
    if dev > tol && depth < 18 {
        refine(eval, a, m, pa, pm, tol, depth + 1, out);
        refine(eval, m, b, pm, pb, tol, depth + 1, out);
    } else {
        out.push(pb);
    }
}

fn dist_to_segment(p: Pt, a: Pt, b: Pt) -> f64 {
    let ab = b.sub(a);
    let l2 = ab.x * ab.x + ab.y * ab.y;
    if l2 < 1e-24 {
        return p.dist(a);
    }
    let t = (((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / l2).clamp(0.0, 1.0);
    p.dist(a.add(ab.scale(t)))
}

fn nurbs_point(deg: usize, knots: &[f64], ctrl: &[Pt], w: &[f64], u: f64) -> Pt {
    let n = ctrl.len();
    // Find span k with knots[k] <= u < knots[k+1], k in [deg, n-1].
    let mut k = deg;
    while k < n - 1 && u >= knots[k + 1] {
        k += 1;
    }
    let mut d: Vec<(f64, f64, f64)> = (0..=deg)
        .map(|j| {
            let i = j + k - deg;
            (ctrl[i].x * w[i], ctrl[i].y * w[i], w[i])
        })
        .collect();
    for r in 1..=deg {
        for j in (r..=deg).rev() {
            let i = j + k - deg;
            let denom = knots[i + deg + 1 - r] - knots[i];
            let alpha = if denom.abs() < 1e-15 { 0.0 } else { (u - knots[i]) / denom };
            d[j] = (
                (1.0 - alpha) * d[j - 1].0 + alpha * d[j].0,
                (1.0 - alpha) * d[j - 1].1 + alpha * d[j].1,
                (1.0 - alpha) * d[j - 1].2 + alpha * d[j].2,
            );
        }
    }
    let (x, y, ww) = d[deg];
    Pt::new(x / ww, y / ww)
}

/// Joins open pieces into closed loops. Returns loops and leftover open chains.
fn chain_pieces(mut open: Vec<Path>, join_tol: f64, gap_tol: f64, warnings: &mut Vec<String>) -> (Vec<Path>, Vec<Path>) {
    let mut loops = Vec::new();
    let mut leftovers = Vec::new();
    let mut used = vec![false; open.len()];
    for i in 0..open.len() {
        if used[i] {
            continue;
        }
        used[i] = true;
        let mut chain = std::mem::replace(&mut open[i], Path::new(vec![], false));
        let mut flipped_once = false;
        loop {
            let end = chain.end();
            let start = chain.start();
            // Best continuation among unused pieces.
            let mut best: Option<(usize, bool, f64)> = None;
            for (j, p) in open.iter().enumerate() {
                if used[j] {
                    continue;
                }
                for (rev, d) in [(false, end.dist(p.start())), (true, end.dist(p.end()))] {
                    if best.is_none_or(|b| d < b.2) {
                        best = Some((j, rev, d));
                    }
                }
            }
            let close_d = end.dist(start);
            let can_close = chain.seg_count() >= 2 || chain.verts.iter().any(|v| v.bulge != 0.0);
            if can_close && close_d <= join_tol && best.is_none_or(|b| b.2 >= close_d) {
                chain.verts.pop();
                chain.closed = true;
                loops.push(chain);
                break;
            }
            match best {
                Some((j, rev, d)) if d <= gap_tol && (d <= join_tol || !can_close || d < close_d) => {
                    used[j] = true;
                    let next = if rev { open[j].reversed() } else { open[j].clone() };
                    if d <= join_tol {
                        chain.verts.pop();
                    } else {
                        warnings.push(format!("bridged a {:.3} mm gap near ({:.2}, {:.2})", d, end.x, end.y));
                    }
                    chain.verts.extend(next.verts);
                }
                _ => {
                    if can_close && close_d <= gap_tol {
                        warnings.push(format!("closed a {:.3} mm gap near ({:.2}, {:.2})", close_d, end.x, end.y));
                        if let Some(last) = chain.verts.last_mut() {
                            last.bulge = 0.0;
                        }
                        chain.closed = true;
                        loops.push(chain);
                        break;
                    }
                    if !flipped_once {
                        // Try growing from the other end.
                        flipped_once = true;
                        chain = chain.reversed();
                        continue;
                    }
                    leftovers.push(chain);
                    break;
                }
            }
        }
    }
    (loops, leftovers)
}

/// Removes exact duplicate pieces (the same entity drawn twice is common).
fn dedup_pieces(pieces: Vec<Path>) -> (Vec<Path>, usize) {
    let key = |p: &Path| -> Vec<i64> {
        let fwd = p;
        let rev = p.reversed();
        let enc = |q: &Path| -> Vec<i64> {
            let mut k: Vec<i64> = q.verts.iter().flat_map(|v| [(v.p.x * 1e4).round() as i64, (v.p.y * 1e4).round() as i64, (v.bulge * 1e6).round() as i64]).collect();
            k.push(q.closed as i64);
            k
        };
        let (a, b) = (enc(fwd), enc(&rev));
        if p.closed { a } else { a.min(b) }
    };
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let mut dropped = 0;
    for p in pieces {
        if seen.insert(key(&p)) {
            out.push(p);
        } else {
            dropped += 1;
        }
    }
    (out, dropped)
}

fn build_shapes(pieces: Vec<Piece>, stem: &str, file: &FsPath, opts: &ImportOptions, warnings: &mut Vec<String>) -> Result<Vec<Shape>> {
    let tol = opts.curve_tol;
    let mut cut_closed = Vec::new();
    let mut cut_open = Vec::new();
    let mut marks = Vec::new();
    let mut fibres = Vec::new();
    for pc in pieces {
        // Drop zero-length geometry.
        let pts = pc.path.flatten(tol, FlattenMode::Inscribed);
        if pts.len() < 2 {
            continue;
        }
        match pc.role {
            Role::Cut if pc.path.closed => cut_closed.push(pc.path),
            Role::Cut => cut_open.push(pc.path),
            Role::Mark => marks.push(pc.path),
            Role::Fibre => fibres.push(pc.path),
            Role::Ignore => {}
        }
    }
    let (cut_open, d1) = dedup_pieces(cut_open);
    let (cut_closed, d2) = dedup_pieces(cut_closed);
    if d1 + d2 > 0 {
        warnings.push(format!("removed {} duplicate cut entit{}", d1 + d2, if d1 + d2 == 1 { "y" } else { "ies" }));
    }

    // Open polylines whose ends meet are closed loops already.
    let mut loops = cut_closed;
    let mut still_open = Vec::new();
    for mut p in cut_open {
        if p.seg_count() >= 2 && p.start().dist(p.end()) <= opts.join_tol {
            p.verts.pop();
            p.closed = true;
            loops.push(p);
        } else {
            still_open.push(p);
        }
    }
    let (chained, leftovers) = chain_pieces(still_open, opts.join_tol, opts.gap_tol, warnings);
    loops.extend(chained);
    for l in &leftovers {
        let (s, e) = (l.start(), l.end());
        warnings.push(format!(
            "open contour ignored: from ({:.2}, {:.2}) to ({:.2}, {:.2}), gap {:.3} mm (raise --gap-tol or fix in CAD)",
            s.x,
            s.y,
            e.x,
            e.y,
            s.dist(e)
        ));
    }

    // Classify loops by containment.
    struct L {
        path: Path,
        poly: Vec<Pt>,
        area: f64,
    }
    let mut ls: Vec<L> = Vec::new();
    for path in loops {
        let poly = path.flatten(tol, FlattenMode::Inscribed);
        let area = signed_area(&poly);
        if area.abs() < 1e-4 {
            warnings.push("ignored a closed contour with (almost) zero area".into());
            continue;
        }
        if let Some(at) = find_self_intersection(&poly) {
            bail!("{}: contour crosses itself near ({:.2}, {:.2}); fix it in CAD", file.display(), at.x, at.y);
        }
        ls.push(L { path, poly, area });
    }
    ls.sort_by(|a, b| b.area.abs().total_cmp(&a.area.abs()));
    let n = ls.len();
    let mut parent: Vec<Option<usize>> = vec![None; n];
    let mut depth = vec![0usize; n];
    for i in 0..n {
        let probes: Vec<Pt> = probe_points(&ls[i].poly);
        for j in (0..i).rev() {
            let inside = probes.iter().filter(|p| point_in_polygon(**p, &ls[j].poly)).count();
            if inside * 2 > probes.len() {
                // Smallest containing loop: since j runs from small to large area... take first hit scanning smaller first.
                if parent[i].is_none_or(|pj| ls[j].area.abs() < ls[pj].area.abs()) {
                    parent[i] = Some(j);
                }
            }
        }
        depth[i] = parent[i].map_or(0, |p| depth[p] + 1);
    }

    // Outer loops (even depth) become shapes; odd-depth loops are their holes.
    let mut shape_of_loop: Vec<Option<usize>> = vec![None; n];
    let mut shapes: Vec<Shape> = Vec::new();
    let mut outer_polys: Vec<Vec<Pt>> = Vec::new();
    for i in 0..n {
        if depth[i] % 2 == 0 {
            let mut outer = ls[i].path.clone();
            if ls[i].area < 0.0 {
                outer = outer.reversed();
            }
            shape_of_loop[i] = Some(shapes.len());
            outer_polys.push(outer.flatten(tol, FlattenMode::Inscribed));
            shapes.push(Shape {
                name: String::new(),
                source_file: file.to_path_buf(),
                outer,
                holes: vec![],
                marks: vec![],
                layer_ref_deg: None,
                mirrored_on_import: opts.mirror,
            });
        }
    }
    for i in 0..n {
        if depth[i] % 2 == 1 {
            let p = parent[i].expect("hole has a parent");
            let s = shape_of_loop[p].expect("parent is an outer loop");
            let mut hole = ls[i].path.clone();
            if ls[i].area > 0.0 {
                hole = hole.reversed();
            }
            shapes[s].holes.push(hole);
        }
    }
    if shapes.is_empty() {
        bail!("{}: no closed cut contour found", file.display());
    }

    // Find which shape a point belongs to (smallest containing outer).
    let owner = |p: Pt| -> Option<usize> {
        (0..outer_polys.len()).filter(|&s| point_in_polygon(p, &outer_polys[s])).min_by(|&a, &b| signed_area(&outer_polys[a]).total_cmp(&signed_area(&outer_polys[b])))
    };
    let mut unassigned_marks = 0;
    for m in marks {
        let pts = m.flatten(tol, FlattenMode::Inscribed);
        match owner(pts[pts.len() / 2]).or_else(|| owner(pts[0])) {
            Some(s) => shapes[s].marks.push(m),
            None => unassigned_marks += 1,
        }
    }
    if unassigned_marks > 0 {
        warnings.push(format!("{unassigned_marks} mark entit{} outside every part ignored", if unassigned_marks == 1 { "y" } else { "ies" }));
    }
    let mut best_fibre: Vec<Option<(f64, f64)>> = vec![None; shapes.len()];
    for f in fibres {
        let (s, e) = (f.start(), f.end());
        let len = s.dist(e);
        if len < 1e-6 {
            continue;
        }
        let ang = (e.y - s.y).atan2(e.x - s.x).to_degrees();
        let mid = Pt::new((s.x + e.x) / 2.0, (s.y + e.y) / 2.0);
        let target = owner(mid).or(if shapes.len() == 1 { Some(0) } else { None });
        match target {
            Some(t) => {
                if best_fibre[t].is_none_or(|b| len > b.1) {
                    best_fibre[t] = Some((ang, len));
                }
            }
            None => warnings.push(format!("fibre line at ({:.1}, {:.1}) is not inside any part; ignored", mid.x, mid.y)),
        }
    }
    for (s, b) in best_fibre.into_iter().enumerate() {
        shapes[s].layer_ref_deg = b.map(|x| x.0);
    }

    // Stable naming: left-to-right, bottom-to-top.
    let mut order: Vec<usize> = (0..shapes.len()).collect();
    let bb: Vec<crate::geom::BBox> = outer_polys.iter().map(|p| crate::geom::BBox::of(p)).collect();
    order.sort_by(|&a, &b| bb[a].min.x.total_cmp(&bb[b].min.x).then(bb[a].min.y.total_cmp(&bb[b].min.y)));
    let mut sorted: Vec<Shape> = Vec::with_capacity(shapes.len());
    let multi = shapes.len() > 1;
    for (k, idx) in order.into_iter().enumerate() {
        let mut s = shapes[idx].clone();
        s.name = if multi { format!("{stem}#{}", k + 1) } else { stem.to_string() };
        sorted.push(s);
    }
    Ok(sorted)
}

/// A few points along a polygon used for robust containment tests.
fn probe_points(poly: &[Pt]) -> Vec<Pt> {
    let n = poly.len();
    let k = n.min(5);
    (0..k)
        .map(|i| {
            let a = poly[i * n / k];
            let b = poly[(i * n / k + 1) % n];
            Pt::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(a: (f64, f64), b: (f64, f64)) -> Path {
        Path::from_points(&[Pt::new(a.0, a.1), Pt::new(b.0, b.1)], false)
    }

    #[test]
    fn chains_lines_with_tiny_gaps_and_reversed_pieces() {
        let pieces = vec![
            line((0.0, 0.0), (10.0, 0.0)),
            line((10.0, 10.0), (10.0, 0.005)), // reversed and slightly off
            line((10.0, 10.0), (0.0, 10.0)),
            line((0.0, 0.0), (0.0, 10.0)),
        ];
        let mut w = vec![];
        let (loops, left) = chain_pieces(pieces, 0.01, 0.1, &mut w);
        assert_eq!(loops.len(), 1);
        assert!(left.is_empty());
        assert_eq!(loops[0].verts.len(), 4);
        assert!((signed_area(&loops[0].flatten(0.01, FlattenMode::Inscribed)).abs() - 100.0).abs() < 0.1);
    }

    #[test]
    fn bridges_small_gap_with_warning() {
        let pieces = vec![line((0.0, 0.0), (10.0, 0.0)), line((10.0, 0.0), (10.0, 10.0)), line((10.0, 10.0), (0.0, 10.0)), line((0.0, 10.0), (0.0, 0.05))];
        let mut w = vec![];
        let (loops, _) = chain_pieces(pieces, 0.01, 0.1, &mut w);
        assert_eq!(loops.len(), 1);
        assert!(w.iter().any(|m| m.contains("gap")));
    }

    #[test]
    fn leaves_large_gap_open() {
        let pieces = vec![line((0.0, 0.0), (10.0, 0.0)), line((10.0, 0.0), (10.0, 10.0)), line((10.0, 10.0), (0.0, 10.0)), line((0.0, 10.0), (0.0, 2.0))];
        let mut w = vec![];
        let (loops, left) = chain_pieces(pieces, 0.01, 0.1, &mut w);
        assert!(loops.is_empty());
        assert_eq!(left.len(), 1);
    }

    #[test]
    fn nurbs_clamped_quadratic_is_parabola() {
        // Clamped quadratic Bezier from (0,0) via (1,2) to (2,0): y = 2t(1-t)*2 at x = 2t.
        let knots = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let ctrl = [Pt::new(0.0, 0.0), Pt::new(1.0, 2.0), Pt::new(2.0, 0.0)];
        let w = [1.0; 3];
        let p = nurbs_point(2, &knots, &ctrl, &w, 0.5);
        assert!((p.x - 1.0).abs() < 1e-12 && (p.y - 1.0).abs() < 1e-12);
        let e = nurbs_point(2, &knots, &ctrl, &w, 1.0);
        assert!((e.x - 2.0).abs() < 1e-12 && e.y.abs() < 1e-12);
    }

    #[test]
    fn layer_rules() {
        let r = LayerRules::default();
        assert_eq!(r.role("fibre_dir"), Role::Fibre);
        assert_eq!(r.role("Mark"), Role::Mark);
        assert_eq!(r.role("Dimensions"), Role::Ignore);
        assert_eq!(r.role("0"), Role::Cut);
        assert_eq!(r.role("OUTLINE"), Role::Cut);
    }
}
