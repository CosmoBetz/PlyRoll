//! Output: cut DXF, preview SVG, ply list CSV.
//!
//! DXF layers:
//! - `CUT`   part outlines and holes (exact arcs), holes before outlines,
//!           parts in roll order (by position along the roll)
//! - `MARK`  marking geometry from the source plus a fibre-direction line per ply
//! - `LABEL` text: ply name and angle
//! - `ROLL`  roll edges, edge-trim lines, lead trim and the cross-cut line

use crate::geom::{Affine, BBox, FlattenMode, Path, Pt, arc_from_bulge, norm_deg};
use crate::model::{Layout, Placement, PlyShape};
use anyhow::{Context, Result};
use geo::{Coord, Distance, Euclidean, LineString, Point, Polygon};
use std::fmt::Write as _;
use std::path::Path as FsPath;

const TOL: f64 = 0.01;

/// Label position, size and fibre direction for one placement.
#[derive(Clone, Debug)]
pub struct Label {
    pub at: Pt,
    /// Radius of the largest circle around `at` that fits inside the part.
    pub room: f64,
    pub text: String,
    /// Fibre direction on the roll, degrees (0 = along the roll).
    pub fibre_dir: f64,
}

pub fn placed_path(p: &Path, pl: &Placement) -> Path {
    p.transformed(&pl.transform(), TOL)
}

pub fn label_text(ply: &PlyShape, pl: &Placement) -> String {
    let v = &ply.variants[pl.variant_idx];
    let a = v.angle_deg;
    let ang = if a == 0.0 { "0".to_string() } else if a.fract() == 0.0 { format!("{a:+}") } else { format!("{a:+.1}") };
    format!("{} {}°", ply.shape.name, ang)
}

pub fn labels(plies: &[PlyShape], layout: &Layout) -> Vec<Label> {
    layout
        .placements
        .iter()
        .map(|pl| {
            let ply = &plies[pl.shape_idx];
            let outer = placed_path(&ply.shape.outer, pl).flatten(0.2, FlattenMode::Inscribed);
            let holes: Vec<LineString<f64>> =
                ply.shape.holes.iter().map(|h| to_ls(&placed_path(h, pl).flatten(0.2, FlattenMode::Inscribed))).collect();
            let poly = Polygon::new(to_ls(&outer), holes);
            let at = polylabel::polylabel(&poly, &0.5).map(|p| Pt::new(p.x(), p.y())).unwrap_or_else(|_| {
                let b = BBox::of(&outer);
                Pt::new((b.min.x + b.max.x) / 2.0, (b.min.y + b.max.y) / 2.0)
            });
            let room = Euclidean.distance(&Point::new(at.x, at.y), poly.exterior());
            let room = poly.interiors().iter().map(|h| Euclidean.distance(&Point::new(at.x, at.y), h)).fold(room, f64::min);
            let fibre = ply.fibre_deg(&ply.variants[pl.variant_idx]) + pl.rotation_deg;
            Label { at, room, text: label_text(ply, pl), fibre_dir: norm_deg(fibre) % 180.0 }
        })
        .collect()
}

fn to_ls(pts: &[Pt]) -> LineString<f64> {
    LineString::from(pts.iter().map(|p| Coord { x: p.x, y: p.y }).collect::<Vec<_>>())
}

/// Placement indices in cutting order: along the roll, then across it.
pub fn cut_order(plies: &[PlyShape], layout: &Layout) -> Vec<usize> {
    let keys: Vec<(f64, f64)> = layout
        .placements
        .iter()
        .map(|pl| {
            let b = BBox::of(&placed_path(&plies[pl.shape_idx].shape.outer, pl).flatten(0.5, FlattenMode::Inscribed));
            (b.min.x, b.min.y)
        })
        .collect();
    let mut idx: Vec<usize> = (0..keys.len()).collect();
    idx.sort_by(|&a, &b| keys[a].0.total_cmp(&keys[b].0).then(keys[a].1.total_cmp(&keys[b].1)));
    idx
}

/// Fibre-direction line through the label point, sized to the part.
pub(crate) fn fibre_line(l: &Label) -> Option<(Pt, Pt)> {
    let half = (l.room * 0.8).min(30.0);
    if half < 3.0 {
        return None;
    }
    let (s, c) = l.fibre_dir.to_radians().sin_cos();
    let d = Pt::new(c * half, s * half);
    Some((l.at.sub(d), l.at.add(d)))
}

fn text_height(l: &Label) -> f64 {
    (l.room * 0.35).clamp(1.5, 8.0)
}

pub fn write_dxf(path: &FsPath, plies: &[PlyShape], layout: &Layout) -> Result<()> {
    use dxf::entities::{Entity, EntityType, Line, LwPolyline, Text};
    use dxf::enums::{AcadVersion, HorizontalTextJustification, Units, VerticalTextJustification};
    use dxf::tables::Layer;
    use dxf::{Color, LwPolylineVertex, Point as DPoint};

    let mut d = dxf::Drawing::new();
    d.header.version = AcadVersion::R2000;
    d.header.default_drawing_units = Units::Millimeters;
    for (name, color) in [("CUT", 7u8), ("MARK", 3), ("LABEL", 5), ("ROLL", 8)] {
        d.add_layer(Layer { name: name.into(), color: Color::from_index(color), ..Default::default() });
    }
    let add = |d: &mut dxf::Drawing, layer: &str, et: EntityType| {
        let mut e = Entity::new(et);
        e.common.layer = layer.into();
        d.add_entity(e);
    };
    let poly = |p: &Path| {
        let mut lw = LwPolyline::default();
        for v in &p.verts {
            lw.vertices.push(LwPolylineVertex { x: v.p.x, y: v.p.y, bulge: v.bulge, ..Default::default() });
        }
        lw.set_is_closed(p.closed);
        EntityType::LwPolyline(lw)
    };
    let line = |a: Pt, b: Pt| EntityType::Line(Line::new(DPoint::new(a.x, a.y, 0.0), DPoint::new(b.x, b.y, 0.0)));

    let m = &layout.material;
    let len = layout.used_length;
    // Roll outline, trims and cross-cut.
    add(&mut d, "ROLL", poly(&Path::from_points(&[Pt::new(0.0, 0.0), Pt::new(len, 0.0), Pt::new(len, m.roll_width), Pt::new(0.0, m.roll_width)], true)));
    if m.edge_trim > 0.0 {
        add(&mut d, "ROLL", line(Pt::new(0.0, m.edge_trim), Pt::new(len, m.edge_trim)));
        add(&mut d, "ROLL", line(Pt::new(0.0, m.roll_width - m.edge_trim), Pt::new(len, m.roll_width - m.edge_trim)));
    }
    if m.lead_trim > 0.0 {
        add(&mut d, "ROLL", line(Pt::new(m.lead_trim, 0.0), Pt::new(m.lead_trim, m.roll_width)));
    }

    let labels = labels(plies, layout);
    let order = cut_order(plies, layout);
    // Marking first (pen/inkjet runs before the knife), then labels, then cuts.
    for &i in &order {
        let pl = &layout.placements[i];
        let ply = &plies[pl.shape_idx];
        for mk in &ply.shape.marks {
            add(&mut d, "MARK", poly(&placed_path(mk, pl)));
        }
        if let Some((a, b)) = fibre_line(&labels[i]) {
            add(&mut d, "MARK", line(a, b));
        }
    }
    for &i in &order {
        let l = &labels[i];
        let h = text_height(l);
        let mut t = Text::default();
        t.location = DPoint::new(l.at.x, l.at.y - h * 1.2, 0.0);
        t.second_alignment_point = t.location.clone();
        t.text_height = h;
        t.value = l.text.clone();
        t.horizontal_text_justification = HorizontalTextJustification::Center;
        t.vertical_text_justification = VerticalTextJustification::Middle;
        add(&mut d, "LABEL", EntityType::Text(t));
    }
    for &i in &order {
        let pl = &layout.placements[i];
        let ply = &plies[pl.shape_idx];
        for h in &ply.shape.holes {
            add(&mut d, "CUT", poly(&placed_path(h, pl)));
        }
        add(&mut d, "CUT", poly(&placed_path(&ply.shape.outer, pl)));
    }
    d.save_file(path).with_context(|| format!("cannot write {}", path.display()))?;
    Ok(())
}

/// Colour per ply angle (same angle -> same colour across shapes).
pub(crate) fn angle_color(angle: f64) -> &'static str {
    const PALETTE: [&str; 8] = ["#e45756", "#54a24b", "#4c78a8", "#f2b701", "#b279a2", "#72b7b2", "#ff9da6", "#9d755d"];
    let a = norm_deg(angle);
    let idx = match a.round() as i64 {
        0 | 180 => 0,
        45 | 225 => 1,
        315 | 135 => 2,
        90 | 270 => 3,
        _ => 4 + ((a.round() as usize) % 4),
    };
    PALETTE[idx]
}

fn svg_path_d(p: &Path, flip: &dyn Fn(Pt) -> Pt) -> String {
    let mut s = String::new();
    let n = p.verts.len();
    if n == 0 {
        return s;
    }
    let p0 = flip(p.verts[0].p);
    let _ = write!(s, "M{:.3} {:.3}", p0.x, p0.y);
    for i in 0..p.seg_count() {
        let (a, b, bulge) = p.seg(i);
        let q = flip(b);
        match arc_from_bulge(a, b, bulge) {
            Some(arc) => {
                let large = (arc.sweep.abs() > std::f64::consts::PI) as u8;
                // The y-flip turns counter-clockwise arcs into SVG sweep-flag 0.
                let sweep = (bulge < 0.0) as u8;
                let _ = write!(s, " A{:.3} {:.3} 0 {} {} {:.3} {:.3}", arc.radius, arc.radius, large, sweep, q.x, q.y);
            }
            None => {
                let _ = write!(s, " L{:.3} {:.3}", q.x, q.y);
            }
        }
    }
    if p.closed {
        s.push_str(" Z");
    }
    s
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

pub fn write_svg(path: &FsPath, plies: &[PlyShape], layout: &Layout, title: &str) -> Result<()> {
    let m = &layout.material;
    let (w, h) = (layout.used_length.max(1.0), m.roll_width);
    let flip = |p: Pt| Pt::new(p.x, h - p.y);
    let margin = 20.0;
    let mut s = String::new();
    let _ = writeln!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{:.1} {:.1} {:.1} {:.1}" width="{:.0}mm" height="{:.0}mm" font-family="Helvetica, Arial, sans-serif">"#,
        -margin,
        -margin - 30.0,
        w + 2.0 * margin,
        h + 2.0 * margin + 30.0,
        w + 2.0 * margin,
        h + 2.0 * margin + 30.0
    );
    let _ = writeln!(s, r#"<text x="0" y="-12" font-size="14">{}</text>"#, esc(title));
    let _ = writeln!(s, r##"<rect x="0" y="0" width="{w:.3}" height="{h:.3}" fill="#f4f1ea" stroke="#555" stroke-width="0.6"/>"##);
    if m.edge_trim > 0.0 {
        for y in [m.edge_trim, m.roll_width - m.edge_trim] {
            let _ = writeln!(s, r##"<line x1="0" y1="{:.3}" x2="{w:.3}" y2="{:.3}" stroke="#999" stroke-dasharray="6 4" stroke-width="0.5"/>"##, h - y, h - y);
        }
    }
    if m.lead_trim > 0.0 {
        let _ = writeln!(s, r##"<line x1="{0:.3}" y1="0" x2="{0:.3}" y2="{h:.3}" stroke="#999" stroke-dasharray="6 4" stroke-width="0.5"/>"##, m.lead_trim);
    }
    // Roll direction arrow.
    let _ = writeln!(s, r##"<text x="0" y="{:.1}" font-size="12" fill="#555">roll direction → (fibre / warp 0°)</text>"##, h + 16.0);

    let labels = labels(plies, layout);
    for (i, pl) in layout.placements.iter().enumerate() {
        let ply = &plies[pl.shape_idx];
        let color = angle_color(ply.variants[pl.variant_idx].angle_deg);
        let mut d = svg_path_d(&placed_path(&ply.shape.outer, pl), &flip);
        for hole in &ply.shape.holes {
            d.push(' ');
            d.push_str(&svg_path_d(&placed_path(hole, pl), &flip));
        }
        let _ = writeln!(s, r##"<path d="{d}" fill="{color}" fill-opacity="0.35" fill-rule="evenodd" stroke="#222" stroke-width="0.5"/>"##);
        for mk in &ply.shape.marks {
            let _ = writeln!(s, r##"<path d="{}" fill="none" stroke="#2a7" stroke-width="0.4"/>"##, svg_path_d(&placed_path(mk, pl), &flip));
        }
        let l = &labels[i];
        if let Some((a, b)) = fibre_line(l) {
            let (a, b) = (flip(a), flip(b));
            let _ = writeln!(s, r##"<line x1="{:.3}" y1="{:.3}" x2="{:.3}" y2="{:.3}" stroke="#222" stroke-width="0.6"/>"##, a.x, a.y, b.x, b.y);
        }
        let th = text_height(l);
        let at = flip(Pt::new(l.at.x, l.at.y - th * 1.2));
        let _ = writeln!(s, r##"<text x="{:.3}" y="{:.3}" font-size="{:.2}" text-anchor="middle" dominant-baseline="middle">{}</text>"##, at.x, at.y, th, esc(&l.text));
    }
    let _ = writeln!(s, "</svg>");
    std::fs::write(path, s).with_context(|| format!("cannot write {}", path.display()))?;
    Ok(())
}

/// One row per cut ply, in cutting order.
pub fn write_ply_csv(path: &FsPath, plies: &[PlyShape], layout: &Layout) -> Result<()> {
    let mut w = csv::Writer::from_path(path).with_context(|| format!("cannot write {}", path.display()))?;
    w.write_record(["order", "ply", "shape", "angle_deg", "copy", "kit", "source_file", "x_mm", "y_mm", "rotation_deg", "ref_deg", "ref_origin"])?;
    for (n, &i) in cut_order(plies, layout).iter().enumerate() {
        let pl = &layout.placements[i];
        let ply = &plies[pl.shape_idx];
        let v = &ply.variants[pl.variant_idx];
        let b = BBox::of(&placed_path(&ply.shape.outer, pl).flatten(0.5, FlattenMode::Inscribed));
        w.write_record([
            (n + 1).to_string(),
            label_text(ply, pl),
            ply.shape.name.clone(),
            format!("{}", v.angle_deg),
            pl.copy.to_string(),
            v.kit.clone().unwrap_or_default(),
            ply.shape.source_file.display().to_string(),
            format!("{:.2}", b.min.x),
            format!("{:.2}", b.min.y),
            format!("{:.4}", pl.rotation_deg),
            format!("{:.3}", ply.ref_deg),
            serde_json::to_value(ply.ref_origin).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default(),
        ])?;
    }
    w.flush()?;
    Ok(())
}

/// Shapes before nesting, drawn side by side with their 0° axis and the fibre
/// direction of every variant — a quick visual check of the orientation input.
pub fn write_orientation_preview(path: &FsPath, plies: &[PlyShape]) -> Result<()> {
    let gap = 30.0;
    let mut x = 0.0;
    let mut items = String::new();
    let mut max_h: f64 = 0.0;
    for ply in plies {
        let pts = ply.shape.outer.flatten(0.2, FlattenMode::Inscribed);
        let b = BBox::of(&pts);
        let t = Affine::translate(x - b.min.x, -b.min.y);
        let (w, h) = (b.width(), b.height());
        max_h = max_h.max(h);
        let outer = ply.shape.outer.transformed(&t, TOL);
        let flip = |p: Pt| Pt::new(p.x, -p.y);
        let mut d = svg_path_d(&outer, &flip);
        for hole in &ply.shape.holes {
            d.push(' ');
            d.push_str(&svg_path_d(&hole.transformed(&t, TOL), &flip));
        }
        let _ = writeln!(items, r##"<path d="{d}" fill="#eee" fill-rule="evenodd" stroke="#222" stroke-width="0.6"/>"##);
        // Fibre hatching per variant, clipped to the part.
        let clip_id = format!("c{}", ply.shape.name.replace(|c: char| !c.is_ascii_alphanumeric(), "_"));
        let _ = writeln!(items, r#"<clipPath id="{clip_id}"><path d="{d}" fill-rule="evenodd"/></clipPath>"#);
        let c = Pt::new(x + w / 2.0, h / 2.0);
        let reach = w.hypot(h);
        for (k, v) in ply.variants.iter().enumerate() {
            let ang = (ply.ref_deg + v.angle_deg).to_radians();
            let (s, co) = ang.sin_cos();
            let spacing = 8.0 * ply.variants.len() as f64;
            let mut lines = String::new();
            let mut off = -reach + k as f64 * 8.0;
            while off < reach {
                let p = c.add(Pt::new(-s * off, co * off));
                let (a, bpt) = (flip(p.sub(Pt::new(co * reach, s * reach))), flip(p.add(Pt::new(co * reach, s * reach))));
                let _ = write!(lines, "M{:.2} {:.2} L{:.2} {:.2} ", a.x, a.y, bpt.x, bpt.y);
                off += spacing;
            }
            let _ = writeln!(items, r#"<path d="{lines}" stroke="{}" stroke-width="0.8" clip-path="url(#{clip_id})"/>"#, angle_color(v.angle_deg));
        }
        // 0° reference axis.
        let (s, co) = ply.ref_deg.to_radians().sin_cos();
        let half = 0.45 * w.max(h);
        let (a, bpt) = (flip(c.sub(Pt::new(co * half, s * half))), flip(c.add(Pt::new(co * half, s * half))));
        let _ = writeln!(items, r##"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="#000" stroke-width="1.6" stroke-dasharray="6 3"/>"##, a.x, a.y, bpt.x, bpt.y);
        let vs: Vec<String> = ply.variants.iter().map(|v| format!("<tspan fill=\"{}\">{:+}°×{}</tspan>", angle_color(v.angle_deg), v.angle_deg, v.qty)).collect();
        let _ = writeln!(items, r#"<text x="{:.2}" y="{:.2}" font-size="10">{} (0° = {:.1}°, {:?})</text>"#, x, 14.0, esc(&ply.shape.name), ply.ref_deg, ply.ref_origin);
        let _ = writeln!(items, r#"<text x="{:.2}" y="{:.2}" font-size="10">{}</text>"#, x, 27.0, vs.join(" "));
        x += w.max(120.0) + gap;
    }
    let s = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="-20 {:.1} {:.1} {:.1}" font-family="Helvetica, Arial, sans-serif">
<text x="0" y="{:.1}" font-size="12">Dashed black: 0° reference. Coloured hatching: fibre direction of each angle variant.</text>
{items}</svg>
"#,
        -max_h - 40.0,
        x + 40.0,
        max_h + 80.0,
        -max_h - 22.0
    );
    std::fs::write(path, s).with_context(|| format!("cannot write {}", path.display()))?;
    Ok(())
}
