//! Writes sample ply DXFs that exercise the importer: bulge polylines,
//! loose lines with tiny gaps and duplicates, holes, a fibre layer line,
//! arcs, a spline edge, two parts in one file, inch units and a
//! negative-extrusion (mirrored OCS) arc.
//!
//! Usage: cargo run -p plyroll-core --example make_samples -- <out_dir>

use dxf::entities::{Arc, Circle, Entity, EntityType, Line, LwPolyline, Spline};
use dxf::enums::{AcadVersion, Units};
use dxf::{Drawing, LwPolylineVertex, Point, Vector};
use std::f64::consts::PI;
use std::path::Path;

fn drawing(units: Units) -> Drawing {
    let mut d = Drawing::new();
    d.header.version = AcadVersion::R2000;
    d.header.default_drawing_units = units;
    d
}

fn add(d: &mut Drawing, layer: &str, et: EntityType) {
    let mut e = Entity::new(et);
    e.common.layer = layer.into();
    d.add_entity(e);
}

fn lw(pts: &[(f64, f64, f64)], closed: bool) -> EntityType {
    let mut p = LwPolyline::default();
    for &(x, y, bulge) in pts {
        p.vertices.push(LwPolylineVertex { x, y, bulge, ..Default::default() });
    }
    p.set_is_closed(closed);
    EntityType::LwPolyline(p)
}

fn line(a: (f64, f64), b: (f64, f64)) -> EntityType {
    EntityType::Line(Line::new(Point::new(a.0, a.1, 0.0), Point::new(b.0, b.1, 0.0)))
}

fn arc(c: (f64, f64), r: f64, a0: f64, a1: f64) -> EntityType {
    EntityType::Arc(Arc::new(Point::new(c.0, c.1, 0.0), r, a0, a1))
}

fn main() -> anyhow::Result<()> {
    let out = std::env::args().nth(1).unwrap_or_else(|| "examples/samples".into());
    let out = Path::new(&out);
    std::fs::create_dir_all(out)?;
    let q = (PI / 8.0).tan(); // bulge of a 90° arc

    // 1. Rib: 320 x 110 rounded rectangle (r = 20), one closed bulge polyline, offset from origin.
    let mut d = drawing(Units::Millimeters);
    let (x0, y0, w, h, r) = (500.0, 200.0, 320.0, 110.0, 20.0);
    add(
        &mut d,
        "OUTLINE",
        lw(
            &[
                (x0 + r, y0, 0.0),
                (x0 + w - r, y0, q),
                (x0 + w, y0 + r, 0.0),
                (x0 + w, y0 + h - r, q),
                (x0 + w - r, y0 + h, 0.0),
                (x0 + r, y0 + h, q),
                (x0, y0 + h - r, 0.0),
                (x0, y0 + r, q),
            ],
            true,
        ),
    );
    d.save_file(out.join("rib.dxf"))?;

    // 2. Gusset: right triangle from loose LINEs, one reversed, a 0.004 mm gap, one duplicate.
    let mut d = drawing(Units::Millimeters);
    add(&mut d, "0", line((0.0, 0.0), (260.0, 0.0)));
    add(&mut d, "0", line((0.0, 180.0), (260.0, 0.004)));
    add(&mut d, "0", line((0.0, 180.0), (0.0, 0.0)));
    add(&mut d, "0", line((0.0, 0.0), (260.0, 0.0)));
    d.save_file(out.join("gusset.dxf"))?;

    // 3. Flange: L-shape with a 40 mm hole, and a fibre line at 30°.
    let mut d = drawing(Units::Millimeters);
    add(&mut d, "CUT", lw(&[(0.0, 0.0, 0.0), (240.0, 0.0, 0.0), (240.0, 80.0, 0.0), (90.0, 80.0, 0.0), (90.0, 200.0, 0.0), (0.0, 200.0, 0.0)], true));
    add(&mut d, "CUT", EntityType::Circle(Circle::new(Point::new(45.0, 140.0, 0.0), 20.0)));
    let (s, c) = (30f64.to_radians().sin(), 30f64.to_radians().cos());
    add(&mut d, "FIBRE", line((130.0, 30.0), (130.0 + 60.0 * c, 30.0 + 60.0 * s)));
    add(&mut d, "MARK", line((10.0, 10.0), (30.0, 10.0)));
    add(&mut d, "DIMENSIONS", line((0.0, -20.0), (240.0, -20.0)));
    d.save_file(out.join("flange.dxf"))?;

    // 4. Patch: 160 mm circle from two ARC entities.
    let mut d = drawing(Units::Millimeters);
    add(&mut d, "0", arc((0.0, 0.0), 80.0, 0.0, 180.0));
    add(&mut d, "0", arc((0.0, 0.0), 80.0, 180.0, 360.0));
    d.save_file(out.join("patch.dxf"))?;

    // 5. Skin: straight bottom and sides, curved top as a clamped cubic B-spline.
    let mut d = drawing(Units::Millimeters);
    add(&mut d, "0", line((0.0, 0.0), (400.0, 0.0)));
    add(&mut d, "0", line((400.0, 0.0), (400.0, 90.0)));
    add(&mut d, "0", line((0.0, 140.0), (0.0, 0.0)));
    let mut sp = Spline::default();
    sp.degree_of_curve = 3;
    sp.control_points = vec![Point::new(400.0, 90.0, 0.0), Point::new(280.0, 200.0, 0.0), Point::new(120.0, 60.0, 0.0), Point::new(0.0, 140.0, 0.0)];
    sp.knot_values = vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
    sp.normal = Vector::new(0.0, 0.0, 1.0);
    add(&mut d, "0", EntityType::Spline(sp));
    d.save_file(out.join("skin.dxf"))?;

    // 6. Doubler: two parts in one file.
    let mut d = drawing(Units::Millimeters);
    add(&mut d, "0", lw(&[(0.0, 0.0, 0.0), (200.0, 0.0, 0.0), (200.0, 80.0, 0.0), (0.0, 80.0, 0.0)], true));
    add(&mut d, "0", lw(&[(300.0, 0.0, 0.0), (450.0, 0.0, 0.0), (450.0, 60.0, 0.0), (300.0, 60.0, 0.0)], true));
    d.save_file(out.join("doubler.dxf"))?;

    // 7. Tab in inches: 4 x 2 in rectangle with a semicircular end.
    let mut d = drawing(Units::Inches);
    add(&mut d, "0", lw(&[(0.0, 0.0, 0.0), (4.0, 0.0, 1.0), (4.0, 2.0, 0.0), (0.0, 2.0, 0.0)], true));
    d.save_file(out.join("tab_inch.dxf"))?;

    // 8. Oval with a mirrored-OCS arc (extrusion -Z), as some CAD exports write them.
    let mut d = drawing(Units::Millimeters);
    add(&mut d, "0", line((0.0, 0.0), (150.0, 0.0)));
    add(&mut d, "0", line((150.0, 60.0), (0.0, 60.0)));
    add(&mut d, "0", arc((150.0, 30.0), 30.0, 270.0, 90.0));
    // Left end: OCS with normal -Z. In OCS x is mirrored, so centre (-0, 30) and the
    // arc from -90° to 90° in OCS is the left half-circle in world coordinates.
    let mut left = Arc::new(Point::new(0.0, 30.0, 0.0), 30.0, 270.0, 90.0);
    left.normal = Vector::new(0.0, 0.0, -1.0);
    add(&mut d, "0", EntityType::Arc(left));
    d.save_file(out.join("oval.dxf"))?;

    println!("wrote samples to {}", out.display());
    Ok(())
}
