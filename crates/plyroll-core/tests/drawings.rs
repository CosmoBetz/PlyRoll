//! Ply drawings for the plybook.

use plyroll_core::drawings::{self, Files, PlyDrawings, Turn};
use plyroll_core::geom::{FlattenMode, Path, Pt, signed_area};
use plyroll_core::import::{ImportOptions, import_dxf};
use plyroll_core::model::{Layout, Material, MaterialKind, Placement, PlyShape, RefOrigin, Shape, Variant};
use plyroll_core::project::mirror_shape;
use plyroll_core::report::{OutputOptions, ReportMeta, RunInfo, summarize, write_outputs};

fn variant(a: f64, qty: u32, kit: Option<&str>) -> Variant {
    Variant { angle_deg: a, qty, kit: kit.map(Into::into), split: None }
}

/// An L with a hole: no symmetry, so a wrong turn or mirror shows.
fn ell(name: &str) -> Shape {
    let outer = Path::from_points(&[Pt::new(0.0, 0.0), Pt::new(120.0, 0.0), Pt::new(120.0, 40.0), Pt::new(40.0, 40.0), Pt::new(40.0, 90.0), Pt::new(0.0, 90.0)], true);
    let hole = Path::from_points(&[Pt::new(10.0, 10.0), Pt::new(10.0, 25.0), Pt::new(25.0, 25.0), Pt::new(25.0, 10.0)], true);
    Shape { name: name.into(), source_file: format!("{name}.dxf").into(), outer, holes: vec![hole], marks: vec![], layer_ref_deg: None, mirrored_on_import: false }
}

fn ply(shape: Shape, ref_deg: f64, variants: Vec<Variant>) -> PlyShape {
    PlyShape { shape, ref_deg, ref_origin: RefOrigin::Manual, cosmetic: false, variants }
}

fn pts(p: &Path) -> Vec<Pt> {
    p.verts.iter().map(|v| v.p).collect()
}

fn same_points(a: &[Pt], b: &[Pt]) -> bool {
    a.len() == b.len() && a.iter().all(|p| b.iter().any(|q| (p.x - q.x).abs() < 1e-6 && (p.y - q.y).abs() < 1e-6))
}

#[test]
fn as_cut_turns_the_fibre_along_x() {
    let plies = vec![ply(ell("ell"), 10.0, vec![variant(0.0, 2, None), variant(45.0, 1, None), variant(0.0, 1, Some("B"))])];
    let d = drawings::drawings(&plies, Turn::AsCut);
    // One drawing per angle: copies and kits do not add any.
    assert_eq!(d.iter().map(|x| x.caption.as_str()).collect::<Vec<_>>(), ["ell 0°", "ell +45°"]);
    assert_eq!(d.iter().map(|x| x.stem.as_str()).collect::<Vec<_>>(), ["ell_0", "ell_+45"]);
    for (x, f) in d.iter().zip([10.0, 55.0]) {
        assert_eq!(x.fibre_deg, 0.0);
        // The drawing turned by -fibre, then moved to the origin.
        let t = plyroll_core::geom::Affine::rot_trans(-f, 0.0, 0.0);
        let turned: Vec<Pt> = pts(&plies[0].shape.outer).iter().map(|&p| t.apply(p)).collect();
        let min = turned.iter().fold(Pt::new(f64::MAX, f64::MAX), |m, p| Pt::new(m.x.min(p.x), m.y.min(p.y)));
        let want: Vec<Pt> = turned.iter().map(|p| Pt::new(p.x - min.x, p.y - min.y)).collect();
        assert!(same_points(&pts(&x.outer), &want), "{:?}", x.caption);
        assert_eq!(x.holes.len(), 1);
    }
}

#[test]
fn as_drawn_shows_the_cad_drawing() {
    // Mirrored on import: the stored geometry is the mirror image of the drawing.
    let cad = ell("ell");
    let plies = vec![
        ply(cad.clone(), 30.0, vec![variant(-45.0, 1, None)]),
        ply(mirror_shape(&ell("ell")), 30.0, vec![variant(-45.0, 1, None)]),
    ];
    let d = drawings::drawings(&plies, Turn::AsDrawn);
    assert!(same_points(&pts(&d[0].outer), &pts(&cad.outer)), "kept in drawing coordinates");
    assert!((d[0].fibre_deg - 165.0).abs() < 1e-9, "-15° as a line direction: {}", d[0].fibre_deg);
    // The mirrored ply is shown as drawn in CAD, its fibre mirrored with it.
    assert!(same_points(&pts(&d[1].outer), &pts(&cad.outer)));
    assert!((d[1].fibre_deg - 15.0).abs() < 1e-9, "{}", d[1].fibre_deg);
    assert_eq!(d[1].stem, "ell_-45_2", "names stay unique");
    // As cut, the mirrored ply stays mirrored: the same face as on the roll.
    let c = drawings::drawings(&plies, Turn::AsCut);
    assert!(signed_area(&c[1].outer.flatten(0.1, FlattenMode::Inscribed)) > 0.0);
    assert!(!same_points(&pts(&c[0].outer), &pts(&c[1].outer)));
}

#[test]
fn file_names_are_safe_and_unique() {
    let names = ["doubler#1", "doubler 1", "rib [1/2]", "§"];
    let plies: Vec<PlyShape> = names.iter().map(|n| ply(ell(n), 0.0, vec![variant(22.5, 1, None)])).collect();
    let d = drawings::drawings(&plies, Turn::AsCut);
    assert_eq!(d.iter().map(|x| x.stem.as_str()).collect::<Vec<_>>(), ["doubler_1_+22.5", "doubler_1_+22.5_2", "rib_1_2_+22.5", "ply_+22.5"]);
}

fn material() -> Material {
    Material {
        name: "t".into(),
        kind: MaterialKind::Ud,
        roll_width: 600.0,
        edge_trim: 10.0,
        lead_trim: 20.0,
        spacing: 2.0,
        angle_tolerance: 0.0,
        tolerance_step: 0.5,
        warp_weft_interchangeable: false,
        cost_per_m2: None,
    }
}

#[test]
fn drawings_are_exported_and_old_ones_go() {
    let plies = vec![ply(ell("ell"), 0.0, vec![variant(0.0, 1, None), variant(90.0, 1, None)]), ply(ell("tab"), 0.0, vec![variant(45.0, 1, None)])];
    let placements = (0..3)
        .map(|i| {
            let (s, v) = [(0, 0), (0, 1), (1, 0)][i];
            let r = -plies[s].fibre_deg(&plies[s].variants[v]);
            Placement { shape_idx: s, variant_idx: v, copy: 1, rotation_deg: r, tx: 200.0 + 300.0 * i as f64, ty: 300.0 }
        })
        .collect();
    let layout = Layout { material: material(), placements, used_length: 1200.0, sections: vec![] };
    let sum = summarize(&plies, &layout);
    assert!(sum.verification.ok, "{:?}", sum.verification.issues);
    let dir = std::env::temp_dir().join(format!("plyroll-drawings-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let meta = ReportMeta::new("t");
    let out = |d: Option<PlyDrawings>| write_outputs(&dir, &plies, &layout, &sum, &RunInfo::default(), &[], &meta, &OutputOptions { ply_drawings: d, ..Default::default() }).unwrap();

    let w = out(Some(PlyDrawings { turn: Turn::AsCut, files: Files::PerPly }));
    let folder = dir.join(drawings::FOLDER);
    let names: Vec<String> = w.iter().filter(|f| f.starts_with(&folder)).map(|f| f.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["ell_0.dxf", "ell_+90.dxf", "tab_+45.dxf"]);
    // Read back: one part with its hole, the same area, and the fibre line as a mark.
    let r = import_dxf(&folder.join("ell_+90.dxf"), &ImportOptions::default()).unwrap();
    assert_eq!(r.shapes.len(), 1);
    let s = &r.shapes[0];
    assert_eq!(s.holes.len(), 1);
    let area = |s: &Shape| s.area(0.01);
    assert!((area(s) - area(&plies[0].shape)).abs() < 1e-6, "{} vs {}", area(s), area(&plies[0].shape));
    assert_eq!(s.marks.len(), 1);

    // One file instead: the folder goes, the single file holds all three.
    let w = out(Some(PlyDrawings { turn: Turn::AsDrawn, files: Files::OneFile }));
    assert!(w.contains(&dir.join(drawings::ONE_FILE)));
    assert!(!folder.exists());
    let r = import_dxf(&dir.join(drawings::ONE_FILE), &ImportOptions::default()).unwrap();
    assert_eq!(r.shapes.len(), 3);

    // None asked for: no drawings are left from before.
    out(None);
    assert!(!dir.join(drawings::ONE_FILE).exists() && !folder.exists());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_split_ply_gets_a_drawing_per_piece() {
    let mut v0 = variant(0.0, 2, None);
    v0.split = Some(plyroll_core::split::Split::Auto { overlap: 0.0, pieces: Some(2) });
    let whole = ply(ell("ell"), 0.0, vec![v0, variant(90.0, 1, None)]);
    let (plies, _) = plyroll_core::split::expand(std::slice::from_ref(&whole), &[1], &material()).unwrap();
    let d = drawings::drawings(&plies, Turn::AsCut);
    let mut stems: Vec<&str> = d.iter().map(|x| x.stem.as_str()).collect();
    stems.sort();
    assert_eq!(stems, ["ell_+90", "ell_1_2_0", "ell_2_2_0"]);
    // The pieces together are the whole ply.
    let area = |p: &Path| signed_area(&p.flatten(0.01, FlattenMode::Inscribed)).abs();
    let pieces: f64 = d.iter().filter(|x| x.stem.contains("_2_0")).map(|x| area(&x.outer) - x.holes.iter().map(area).sum::<f64>()).sum();
    let ply_area = whole.shape.area(0.01);
    assert!((pieces - ply_area).abs() < 1.0, "{pieces} vs {ply_area}");
}
