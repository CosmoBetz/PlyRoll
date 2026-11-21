//! Nest -> verify -> write DXF -> re-import the cut layer and compare.

use std::f64::consts::PI;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use plyroll_core::export::write_dxf;
use plyroll_core::geom::{Path, Pt, Vtx, bulge_from_sweep};
use plyroll_core::import::{ImportOptions, LayerRules, import_dxf};
use plyroll_core::model::{Material, MaterialKind, PlyShape, RefOrigin, Shape, Variant};
use plyroll_core::orient::fibre_ok;
use plyroll_core::verify::verify;
use plyroll_nest::{NestOptions, build_groups, nest};

fn shape(name: &str, outer: Path, holes: Vec<Path>) -> Shape {
    Shape { name: name.into(), source_file: format!("{name}.dxf").into(), outer, holes, marks: vec![], layer_ref_deg: None, mirrored_on_import: false }
}

fn rounded_rect(w: f64, h: f64, r: f64, x0: f64, y0: f64) -> Path {
    let q = bulge_from_sweep(PI / 2.0);
    let v = |x: f64, y: f64, b: f64| Vtx { p: Pt::new(x0 + x, y0 + y), bulge: b };
    Path::new(vec![v(r, 0.0, 0.0), v(w - r, 0.0, q), v(w, r, 0.0), v(w, h - r, q), v(w - r, h, 0.0), v(r, h, q), v(0.0, h - r, 0.0), v(0.0, r, q)], true)
}

fn circle(cx: f64, cy: f64, r: f64, ccw: bool) -> Path {
    let b = if ccw { 1.0 } else { -1.0 };
    Path::new(vec![Vtx { p: Pt::new(cx + r, cy), bulge: b }, Vtx { p: Pt::new(cx - r, cy), bulge: b }], true)
}

fn plies() -> Vec<PlyShape> {
    let tri = Path::from_points(&[Pt::new(1000.0, 1000.0), Pt::new(1180.0, 1000.0), Pt::new(1000.0, 120.0 + 1000.0)], true);
    vec![
        PlyShape {
            shape: shape("rib", rounded_rect(220.0, 80.0, 15.0, 300.0, -50.0), vec![]),
            ref_deg: 0.0,
            ref_origin: RefOrigin::DrawingX,
            cosmetic: false,
            variants: vec![Variant { angle_deg: 0.0, qty: 3, kit: None }, Variant { angle_deg: 45.0, qty: 2, kit: None }, Variant { angle_deg: -45.0, qty: 2, kit: None }],
        },
        PlyShape {
            shape: shape("plate", rounded_rect(140.0, 140.0, 10.0, 0.0, 0.0), vec![circle(70.0, 70.0, 25.0, false)]),
            ref_deg: 10.0,
            ref_origin: RefOrigin::Manual,
            cosmetic: false,
            variants: vec![Variant { angle_deg: 0.0, qty: 2, kit: None }, Variant { angle_deg: 90.0, qty: 2, kit: None }],
        },
        PlyShape {
            shape: shape("tri", tri, vec![]),
            ref_deg: 0.0,
            ref_origin: RefOrigin::DrawingX,
            cosmetic: false,
            variants: vec![Variant { angle_deg: 30.0, qty: 3, kit: None }],
        },
    ]
}

fn material(kind: MaterialKind, swap: bool) -> Material {
    Material {
        name: "test".into(),
        kind,
        roll_width: 400.0,
        edge_trim: 8.0,
        lead_trim: 15.0,
        spacing: 3.0,
        angle_tolerance: 0.0,
        tolerance_step: 0.5,
        warp_weft_interchangeable: swap,
        cost_per_m2: None,
    }
}

fn opts() -> NestOptions {
    NestOptions { time: Duration::from_secs(4), seed: Some(7), workers: 2, early_stop: true, nest_tol: 0.05 }
}

#[test]
fn nest_verify_and_dxf_roundtrip() {
    let plies = plies();
    let mat = material(MaterialKind::Ud, false);
    let layout = nest(&plies, &mat, &opts(), |_| {}, Arc::new(AtomicBool::new(false))).expect("nest");
    let report = verify(&plies, &layout);
    assert!(report.ok, "verification failed: {:?}", report.issues);
    assert_eq!(layout.placements.len(), 14);
    for pl in &layout.placements {
        let p = &plies[pl.shape_idx];
        assert!(fibre_ok(p.fibre_deg(&p.variants[pl.variant_idx]), pl.rotation_deg, &mat, false));
    }

    // Write the cut file and read its CUT layer back.
    let dir = std::env::temp_dir().join(format!("plyroll-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dxf = dir.join("layout.dxf");
    write_dxf(&dxf, &plies, &layout).unwrap();
    let mut rules = LayerRules::default();
    rules.ignore.extend(["ROLL".to_string(), "LABEL".to_string()]);
    let back = import_dxf(&dxf, &ImportOptions { layers: rules, ..ImportOptions::default() }).unwrap();
    assert_eq!(back.shapes.len(), layout.placements.len(), "warnings: {:?}", back.warnings);
    let holes: usize = back.shapes.iter().map(|s| s.holes.len()).sum();
    assert_eq!(holes, 4, "plate holes must survive");
    let area_in: f64 = layout.placements.iter().map(|pl| plies[pl.shape_idx].shape.area(0.005)).sum();
    let area_out: f64 = back.shapes.iter().map(|s| s.area(0.005)).sum();
    assert!((area_in - area_out).abs() / area_in < 1e-5, "area {area_in} vs {area_out}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn twill_swap_merges_plus_minus_45() {
    let plies = plies();
    let strict = build_groups(&plies, &material(MaterialKind::Twill, false)).unwrap();
    let swap = build_groups(&plies, &material(MaterialKind::Twill, true)).unwrap();
    // rib: 0 / +45 / -45 are three items strictly; with swap, +45 and -45 merge.
    let ribs = |g: &[plyroll_nest::ItemGroup]| g.iter().filter(|g| g.shape_idx == 0).count();
    assert_eq!(ribs(&strict), 3);
    assert_eq!(ribs(&swap), 2);
    // plate 0 and 90 merge with swap too.
    let plates = |g: &[plyroll_nest::ItemGroup]| g.iter().filter(|g| g.shape_idx == 1).count();
    assert_eq!(plates(&strict), 2);
    assert_eq!(plates(&swap), 1);
}

#[test]
fn too_wide_ply_is_rejected_with_a_clear_message() {
    let mut plies = plies();
    plies[0].shape.outer = rounded_rect(900.0, 500.0, 10.0, 0.0, 0.0);
    let err = build_groups(&plies, &material(MaterialKind::Ud, false)).unwrap_err().to_string();
    assert!(err.contains("does not fit across the roll"), "{err}");
}
