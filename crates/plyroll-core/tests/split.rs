//! Splitting plies that are wider than the roll.

use plyroll_core::geom::{FlattenMode, Path, Pt, Vtx, find_self_intersection, point_in_polygon, signed_area};
use plyroll_core::import::{ImportOptions, import_dxf};
use plyroll_core::job::{load_materials, read_job_csv};
use plyroll_core::model::{Material, PlyShape, RefOrigin, Shape, Variant};
use plyroll_core::project::Project;
use plyroll_core::split::{self, CutLine, Split};
use std::path::PathBuf;

fn demo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/demo")
}

fn ud() -> Material {
    load_materials(&demo().join("materials.toml")).unwrap().into_iter().find(|m| m.name == "UD-T700-150").unwrap()
}

fn shape(name: &str, outer: Path, holes: Vec<Path>) -> Shape {
    Shape { name: name.into(), source_file: PathBuf::new(), outer, holes, marks: vec![], layer_ref_deg: None, mirrored_on_import: false }
}

fn poly(pts: &[(f64, f64)]) -> Path {
    Path::from_points(&pts.iter().map(|&(x, y)| Pt::new(x, y)).collect::<Vec<_>>(), true)
}

fn area(s: &Shape) -> f64 {
    s.area(0.01)
}

fn demo_shapes() -> Vec<Shape> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(demo().join("samples")).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "dxf") {
            out.extend(import_dxf(&p, &ImportOptions::default()).unwrap().shapes);
        }
    }
    assert!(out.len() >= 8);
    out
}

/// Pieces are closed, outlines counter-clockwise, holes clockwise and inside,
/// no outline crosses itself, and the areas add up (at zero overlap).
fn check_pieces(orig: &Shape, pieces: &[Shape], overlap: f64) {
    for p in pieces {
        let outer = p.outer.flatten(0.01, FlattenMode::Inscribed);
        assert!(p.outer.closed);
        assert!(signed_area(&outer) > 0.0, "{}: outline not counter-clockwise", orig.name);
        assert!(find_self_intersection(&outer).is_none(), "{}: piece outline crosses itself", orig.name);
        for h in &p.holes {
            let hp = h.flatten(0.01, FlattenMode::Inscribed);
            assert!(signed_area(&hp) < 0.0, "{}: hole not clockwise", orig.name);
            assert!(hp.iter().all(|&q| point_in_polygon(q, &outer) || outer.iter().any(|o| o.dist(q) < 1e-3)), "{}: hole outside its piece", orig.name);
        }
    }
    let sum: f64 = pieces.iter().map(area).sum();
    if overlap == 0.0 {
        assert!((sum - area(orig)).abs() < 1e-3 * area(orig).max(1.0), "{}: pieces {sum} vs ply {}", orig.name, area(orig));
    } else {
        assert!(sum > area(orig), "{}: overlapping pieces must add area", orig.name);
    }
}

#[test]
fn demo_shapes_cut_cleanly_at_many_lines() {
    let (mut cuts, mut split_ones) = (0, 0);
    for s in demo_shapes() {
        let b = plyroll_core::geom::BBox::of(&s.outer.flatten(0.1, FlattenMode::Inscribed));
        for dir in [0.0, 17.0, 45.0, 90.0, 123.4, -30.0] {
            for f in [0.21, 0.5, 0.77] {
                let p = Pt::new(b.min.x + f * b.width(), b.min.y + (1.0 - f) * b.height());
                let pieces = split::split_shape(&s, &[CutLine { p, dir_deg: dir }], 0.0, dir).unwrap();
                assert!(!pieces.is_empty());
                cuts += 1;
                split_ones += usize::from(pieces.len() > 1);
                check_pieces(&s, &pieces, 0.0);
            }
            // Two lines, with and without overlap.
            let lines = [
                CutLine { p: Pt::new(b.min.x + 0.33 * b.width(), b.min.y + 0.33 * b.height()), dir_deg: dir },
                CutLine { p: Pt::new(b.min.x + 0.66 * b.width(), b.min.y + 0.66 * b.height()), dir_deg: dir },
            ];
            check_pieces(&s, &split::split_shape(&s, &lines, 0.0, dir).unwrap(), 0.0);
            check_pieces(&s, &split::split_shape(&s, &lines, 4.0, dir).unwrap(), 4.0);
        }
    }
    assert!(split_ones * 10 >= cuts * 9, "most lines through the middle should cut: {split_ones} of {cuts}");
}

#[test]
fn arcs_stay_arcs() {
    // A stadium: two straight edges and two half circles.
    let r = 50.0;
    let outer = Path::new(
        vec![
            Vtx { p: Pt::new(0.0, 0.0), bulge: 0.0 },
            Vtx { p: Pt::new(300.0, 0.0), bulge: 1.0 },
            Vtx { p: Pt::new(300.0, 2.0 * r), bulge: 0.0 },
            Vtx { p: Pt::new(0.0, 2.0 * r), bulge: 1.0 },
        ],
        true,
    );
    let s = shape("stadium", outer, vec![]);
    // Along x through the middle: both half circles are cut in two.
    let pieces = split::split_shape(&s, &[CutLine { p: Pt::new(0.0, r), dir_deg: 0.0 }], 0.0, 0.0).unwrap();
    assert_eq!(pieces.len(), 2);
    check_pieces(&s, &pieces, 0.0);
    for p in &pieces {
        let arcs: Vec<f64> = p.outer.verts.iter().map(|v| v.bulge).filter(|b| b.abs() > 1e-9).collect();
        assert_eq!(arcs.len(), 2, "each half keeps two quarter arcs");
        for b in arcs {
            assert!((b.abs() - (std::f64::consts::PI / 8.0).tan()).abs() < 1e-9, "quarter circle bulge, got {b}");
        }
    }
    // Tangent to an arc (touches the top of the right half circle's extent): no cut.
    let tangent = split::split_shape(&s, &[CutLine { p: Pt::new(350.0, 0.0), dir_deg: 90.0 }], 0.0, 90.0).unwrap();
    assert_eq!(tangent.len(), 1);
    check_pieces(&s, &tangent, 0.0);
}

#[test]
fn awkward_lines() {
    let sq = shape("square", poly(&[(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)]), vec![]);
    // Along an edge: nothing to cut.
    for y in [0.0, 100.0] {
        let p = split::split_shape(&sq, &[CutLine { p: Pt::new(0.0, y), dir_deg: 0.0 }], 0.0, 0.0).unwrap();
        assert_eq!(p.len(), 1, "line along the edge at y = {y}");
        check_pieces(&sq, &p, 0.0);
    }
    // Through two corners (the diagonal).
    let p = split::split_shape(&sq, &[CutLine { p: Pt::new(0.0, 0.0), dir_deg: 45.0 }], 0.0, 45.0).unwrap();
    assert_eq!(p.len(), 2);
    check_pieces(&sq, &p, 0.0);
    // Touching one corner only.
    let p = split::split_shape(&sq, &[CutLine { p: Pt::new(100.0, 0.0), dir_deg: 45.0 }], 0.0, 45.0).unwrap();
    assert_eq!(p.len(), 1);
    // Missing the shape.
    let p = split::split_shape(&sq, &[CutLine { p: Pt::new(0.0, 500.0), dir_deg: 0.0 }], 0.0, 0.0).unwrap();
    assert_eq!(p.len(), 1);

    // Through a hole: the hole becomes part of both outlines.
    let hole = poly(&[(40.0, 40.0), (40.0, 60.0), (60.0, 60.0), (60.0, 40.0)]);
    let holed = shape("holed", poly(&[(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)]), vec![hole]);
    let p = split::split_shape(&holed, &[CutLine { p: Pt::new(0.0, 50.0), dir_deg: 0.0 }], 0.0, 0.0).unwrap();
    assert_eq!(p.len(), 2);
    assert!(p.iter().all(|x| x.holes.is_empty()));
    check_pieces(&holed, &p, 0.0);
    // Missing the hole: it stays a hole in its piece.
    let p = split::split_shape(&holed, &[CutLine { p: Pt::new(0.0, 20.0), dir_deg: 0.0 }], 0.0, 0.0).unwrap();
    assert_eq!(p.len(), 2);
    assert_eq!(p.iter().map(|x| x.holes.len()).sum::<usize>(), 1);
    check_pieces(&holed, &p, 0.0);

    // A U: a line across both legs gives three pieces.
    let u = shape("u", poly(&[(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (70.0, 100.0), (70.0, 30.0), (30.0, 30.0), (30.0, 100.0), (0.0, 100.0)]), vec![]);
    let p = split::split_shape(&u, &[CutLine { p: Pt::new(0.0, 60.0), dir_deg: 0.0 }], 0.0, 0.0).unwrap();
    assert_eq!(p.len(), 3);
    check_pieces(&u, &p, 0.0);
    // Through the inner corners exactly.
    let p = split::split_shape(&u, &[CutLine { p: Pt::new(0.0, 30.0), dir_deg: 0.0 }], 0.0, 0.0).unwrap();
    check_pieces(&u, &p, 0.0);
    assert_eq!(p.len(), 3);
}

#[test]
fn marks_are_clipped_to_their_pieces() {
    let mut s = shape("marked", poly(&[(0.0, 0.0), (200.0, 0.0), (200.0, 100.0), (0.0, 100.0)]), vec![]);
    s.marks = vec![Path::from_points(&[Pt::new(10.0, 10.0), Pt::new(10.0, 90.0)], false), Path::from_points(&[Pt::new(150.0, 10.0), Pt::new(190.0, 10.0)], false)];
    let p = split::split_shape(&s, &[CutLine { p: Pt::new(0.0, 50.0), dir_deg: 0.0 }], 0.0, 0.0).unwrap();
    assert_eq!(p.len(), 2);
    // Top piece first: half of the vertical mark. Bottom: the other half and the short mark.
    assert_eq!(p[0].marks.len(), 1);
    assert_eq!(p[1].marks.len(), 2);
    assert!(p[0].marks[0].verts.iter().all(|v| v.p.y >= 50.0 - 1e-9));
}

fn wide_ply(angle: f64) -> PlyShape {
    // 1500 × 400 mm with rounded ends; the demo roll is 580 mm usable.
    let outer = Path::new(
        vec![
            Vtx { p: Pt::new(0.0, 0.0), bulge: 0.0 },
            Vtx { p: Pt::new(1500.0, 0.0), bulge: 1.0 },
            Vtx { p: Pt::new(1500.0, 400.0), bulge: 0.0 },
            Vtx { p: Pt::new(0.0, 400.0), bulge: 1.0 },
        ],
        true,
    );
    PlyShape {
        shape: shape("panel", outer, vec![]),
        ref_deg: 0.0,
        ref_origin: RefOrigin::DrawingX,
        cosmetic: false,
        variants: vec![Variant { angle_deg: angle, qty: 2, kit: Some("A".into()), split: Some(Split::Auto { overlap: 0.0, pieces: None }) }],
    }
}

#[test]
fn automatic_split_fits_every_piece() {
    let m = ud();
    for angle in [90.0, 45.0, -45.0, 60.0] {
        let ply = wide_ply(angle);
        let v = &ply.variants[0];
        assert!(!split::fits(&ply.shape.outer, ply.fibre_deg(v), &m, false), "{angle}°: too wide to start with");
        let pieces = split::pieces(&ply, v, &m).unwrap().expect("split");
        assert!(pieces.len() >= 2);
        check_pieces(&ply.shape, &pieces, 0.0);
        for p in &pieces {
            let w = split::width_across(&p.outer, ply.fibre_deg(v), &m, false);
            assert!(w <= m.usable_width() - split::AUTO_MARGIN + 1e-6, "{angle}°: piece {w} mm");
        }
        // More pieces on request.
        let more = PlyShape { variants: vec![Variant { split: Some(Split::Auto { overlap: 0.0, pieces: Some(pieces.len() as u32 + 1) }), ..v.clone() }], ..ply.clone() };
        let more_pieces = split::pieces(&more, &more.variants[0], &m).unwrap().unwrap();
        assert_eq!(more_pieces.len(), pieces.len() + 1);
        // Asked-for pieces stay close to even.
        let ws: Vec<f64> = more_pieces.iter().map(|p| split::width_across(&p.outer, ply.fibre_deg(v), &m, false)).collect();
        let (lo, hi) = (ws.iter().cloned().fold(f64::INFINITY, f64::min), ws.iter().cloned().fold(0.0, f64::max));
        assert!(hi / lo < 1.5, "{angle}°: pieces {ws:?}");
        // With an overlap the pieces still fit.
        let lap = PlyShape { variants: vec![Variant { split: Some(Split::Auto { overlap: 20.0, pieces: None }), ..v.clone() }], ..ply.clone() };
        for p in split::pieces(&lap, &lap.variants[0], &m).unwrap().unwrap() {
            assert!(split::width_across(&p.outer, ply.fibre_deg(v), &m, false) <= m.usable_width());
        }
    }
    // At 0° the 400 mm width fits: no cut.
    let flat = wide_ply(0.0);
    assert!(split::pieces(&flat, &flat.variants[0], &m).unwrap().is_none());
}

#[test]
fn automatic_cut_goes_where_the_ply_is_narrow() {
    // An hourglass across the roll: wide at both ends (top and bottom in the
    // roll view), a 100 mm waist off centre. The cut should go through the waist.
    let s = shape(
        "hourglass",
        poly(&[(0.0, 0.0), (600.0, 0.0), (600.0, 250.0), (350.0, 300.0), (350.0, 400.0), (600.0, 450.0), (600.0, 900.0), (0.0, 900.0), (0.0, 450.0), (250.0, 400.0), (250.0, 300.0), (0.0, 250.0)]),
        vec![],
    );
    let ply = PlyShape {
        shape: s,
        ref_deg: 0.0,
        ref_origin: RefOrigin::DrawingX,
        cosmetic: false,
        variants: vec![Variant { angle_deg: 0.0, qty: 1, kit: None, split: Some(Split::Auto { overlap: 0.0, pieces: None }) }],
    };
    let m = ud();
    let lines = split::lines_for(&ply, &ply.variants[0], ply.variants[0].split.as_ref().unwrap(), &m).unwrap();
    assert_eq!(lines.len(), 1);
    let y = lines[0].p.y;
    assert!((300.0..=400.0).contains(&y), "cut at y = {y}, not through the waist");
}

#[test]
fn expand_names_and_keeps_kits() {
    let m = ud();
    let mut ply = wide_ply(90.0);
    ply.variants.push(Variant { angle_deg: 0.0, qty: 3, kit: None, split: None });
    let (plies, ids) = split::expand(&[ply], &[7], &m).unwrap();
    assert_eq!(ids.len(), plies.len());
    assert!(ids.iter().all(|&i| i == 7));
    assert_eq!(plies[0].shape.name, "panel");
    assert_eq!(plies[0].variants.len(), 1);
    assert_eq!(plies[0].variants[0].angle_deg, 0.0);
    let n = plies.len() - 1;
    for (k, p) in plies[1..].iter().enumerate() {
        assert_eq!(p.shape.name, format!("panel [{}/{n}]", k + 1));
        assert_eq!(p.variants.len(), 1);
        assert_eq!(p.variants[0].kit.as_deref(), Some("A"));
        assert_eq!(p.variants[0].qty, 2);
        assert!(p.variants[0].split.is_none());
    }
}

#[test]
fn saved_nests_stay_current_without_splits() {
    // Variant without a split serialises exactly as before.
    let v = Variant { angle_deg: 45.0, qty: 2, kit: None, split: None };
    assert_eq!(serde_json::to_string(&v).unwrap(), r#"{"angle_deg":45.0,"qty":2,"kit":null}"#);
    let back: Variant = serde_json::from_str(r#"{"angle_deg":45.0,"qty":2,"kit":null}"#).unwrap();
    assert_eq!(back, v);
    // A project's nest input does not change because the field exists.
    let rows = read_job_csv(&demo().join("job.csv")).unwrap();
    let (mut proj, _) = Project::from_job("demo", &rows, Default::default()).unwrap();
    proj.job.material = Some(ud());
    let a = proj.nest_input(None).unwrap();
    assert!(a.plies.iter().all(|p| p.variants.iter().all(|v| v.split.is_none())));
}

#[test]
fn validation() {
    assert!(Split::Auto { overlap: -1.0, pieces: None }.validate().is_err());
    assert!(Split::Auto { overlap: 0.0, pieces: Some(0) }.validate().is_err());
    assert!(Split::Manual { lines: vec![CutLine { p: Pt::new(f64::NAN, 0.0), dir_deg: 0.0 }], overlap: 0.0 }.validate().is_err());
    assert!(Split::Manual { lines: vec![CutLine { p: Pt::new(0.0, 0.0), dir_deg: 0.0 }; 30], overlap: 0.0 }.validate().is_err());
    assert!(Split::Manual { lines: vec![CutLine { p: Pt::new(0.0, 0.0), dir_deg: 0.0 }], overlap: 5.0 }.validate().is_ok());
}

fn narrow() -> Material {
    Material { roll_width: 250.0, edge_trim: 10.0, ..ud() }
}

fn demo_project() -> Project {
    let rows = read_job_csv(&demo().join("job.csv")).unwrap();
    let (mut proj, _) = Project::from_job("demo", &rows, Default::default()).unwrap();
    proj.job.material = Some(narrow());
    proj
}

#[test]
fn project_names_too_wide_plies_and_splits_them() {
    let mut proj = demo_project();
    let err = format!("{:#}", proj.nest_input(None).unwrap_err());
    assert!(err.contains("wider than the roll (230 mm usable)"), "{err}");
    assert!(err.contains("skin"), "{err}");
    let n = proj.auto_split(None).unwrap();
    assert!(n > 0);
    let input = proj.nest_input(None).unwrap();
    assert!(input.plies.iter().any(|p| p.shape.name.ends_with("[1/2]")));
    assert_eq!(input.plies.len(), input.ply_ids.len());
    // Splitting again changes nothing.
    assert_eq!(proj.auto_split(None).unwrap(), 0);
    // Saved and loaded, the splits stay and the nest input is the same.
    let dir = std::env::temp_dir().join(format!("plyroll-split-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("demo.plyroll");
    proj.save(&path).unwrap();
    let back = Project::load(&path).unwrap();
    assert_eq!(back.nest_input(None).unwrap().fingerprint, input.fingerprint);
}

#[test]
fn manual_lines_follow_their_ply() {
    let mut proj = demo_project();
    proj.auto_split(None).unwrap();
    let (id, vi) = proj
        .plies
        .iter()
        .find_map(|p| p.variants.iter().position(|v| v.split.is_some()).map(|i| (p.id, i)))
        .unwrap();
    let manual = Split::Manual { lines: vec![CutLine { p: Pt::new(0.0, 0.0), dir_deg: 0.0 }], overlap: 0.0 };
    proj.set_split(id, vi, Some(manual.clone())).unwrap();
    let get = |p: &Project| p.plies.iter().find(|x| x.id == id).unwrap().variants[vi].split.clone();
    // Re-sending the same list keeps the lines.
    let vs = proj.plies.iter().find(|x| x.id == id).unwrap().variants.clone();
    proj.set_variants(&[id], &vs).unwrap();
    assert_eq!(get(&proj), Some(manual.clone()));
    // Copying the list onto another ply makes them automatic there.
    let other = proj.plies.iter().find(|x| x.id != id).unwrap().id;
    proj.set_variants(&[id, other], &vs).unwrap();
    assert_eq!(get(&proj), Some(manual.clone()));
    assert!(matches!(proj.plies.iter().find(|x| x.id == other).unwrap().variants[vi].split, Some(Split::Auto { .. })));
    // Changing the angle makes them automatic.
    let mut turned = vs.clone();
    turned[vi].angle_deg += 10.0;
    proj.set_variants(&[id], &turned).unwrap();
    assert!(matches!(get(&proj), Some(Split::Auto { .. })));
    // So does turning the 0° reference.
    proj.set_variants(&[id], &vs).unwrap();
    proj.set_split(id, vi, Some(manual.clone())).unwrap();
    proj.set_reference(&[id], &plyroll_core::project::RefPick::Rotate { delta: 5.0 }).unwrap();
    assert!(matches!(get(&proj), Some(Split::Auto { .. })));
    // And mirroring.
    proj.set_split(id, vi, Some(manual.clone())).unwrap();
    proj.set_mirrored(&[id], true).unwrap();
    assert!(matches!(get(&proj), Some(Split::Auto { .. })));
    // Removing the split.
    proj.set_split(id, vi, None).unwrap();
    assert_eq!(get(&proj), None);
    assert!(proj.set_split(id, 99, None).is_err());
}

#[test]
fn split_json_from_the_app() {
    // The shapes app/src/lib/SplitView.svelte sends to `set_split`.
    let auto: Split = serde_json::from_str(r#"{"mode":"auto","overlap":0,"pieces":null}"#).unwrap();
    assert_eq!(auto, Split::Auto { overlap: 0.0, pieces: None });
    let three: Split = serde_json::from_str(r#"{"mode":"auto","overlap":10,"pieces":3}"#).unwrap();
    assert_eq!(three, Split::Auto { overlap: 10.0, pieces: Some(3) });
    let manual: Split = serde_json::from_str(r#"{"mode":"manual","lines":[{"p":{"x":12.5,"y":-3},"dir_deg":45}],"overlap":5}"#).unwrap();
    assert_eq!(manual, Split::Manual { lines: vec![CutLine { p: Pt::new(12.5, -3.0), dir_deg: 45.0 }], overlap: 5.0 });
    let none: Option<Split> = serde_json::from_str("null").unwrap();
    assert!(none.is_none());
    // And back: what the app reads in Variant.split.
    assert_eq!(serde_json::to_string(&auto).unwrap(), r#"{"mode":"auto","overlap":0.0}"#);
}
