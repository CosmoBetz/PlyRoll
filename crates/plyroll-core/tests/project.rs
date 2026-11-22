//! The project path must give the same plies as the CSV job path.

use plyroll_core::import::ImportOptions;
use plyroll_core::job::{load_job, read_job_csv};
use plyroll_core::project::{PlyStatus, Project, RefPick, Signature};
use std::path::PathBuf;

fn demo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/demo")
}

fn project_from_demo() -> Project {
    let rows = read_job_csv(&demo().join("job.csv")).unwrap();
    let (proj, warnings) = Project::from_job("demo", &rows, Default::default()).unwrap();
    // The demo gusset deliberately contains a duplicate line.
    assert!(warnings.iter().all(|w| w.contains("duplicate")), "{warnings:?}");
    proj
}

#[test]
fn project_matches_csv_job() {
    let rows = read_job_csv(&demo().join("job.csv")).unwrap();
    let job = load_job(&rows, &ImportOptions::default()).unwrap();
    let proj = project_from_demo();
    let (plies, warnings) = proj.to_plies().unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(plies.len(), job.plies.len());
    for (a, b) in plies.iter().zip(&job.plies) {
        assert_eq!(a.shape.name, b.shape.name);
        // Same axis modulo 180°.
        let d = (a.ref_deg - b.ref_deg).rem_euclid(180.0);
        assert!(d < 1e-6 || d > 180.0 - 1e-6, "{}: {} vs {}", a.shape.name, a.ref_deg, b.ref_deg);
        assert_eq!(a.cosmetic, b.cosmetic);
        assert_eq!(a.shape.mirrored_on_import, b.shape.mirrored_on_import);
        let va: Vec<_> = a.variants.iter().map(|v| (v.angle_deg, v.qty, v.kit.clone())).collect();
        let vb: Vec<_> = b.variants.iter().map(|v| (v.angle_deg, v.qty, v.kit.clone())).collect();
        assert_eq!(va, vb);
        // In-place mirroring equals mirroring at import (up to translation).
        assert!(Signature::of(&a.shape).matches(&Signature::of(&b.shape)), "{}", a.shape.name);
        assert!((a.shape.area(0.01) - b.shape.area(0.01)).abs() < 1e-6);
    }
}

#[test]
fn save_load_roundtrip_and_reimport_keeps_references() {
    let proj = project_from_demo();
    let dir = std::env::temp_dir().join(format!("plyroll-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("demo.plyroll");
    proj.save(&path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains(dir.to_str().unwrap()), "source paths must be relative");
    let mut back = Project::load(&path).unwrap();
    assert_eq!(back.plies.len(), proj.plies.len());
    for (a, b) in back.plies.iter().zip(&proj.plies) {
        assert_eq!(a.shape.source_file, b.shape.source_file);
        assert_eq!(a.reference, b.reference);
    }
    let rep = back.reload().unwrap();
    assert_eq!(rep.added, 0);
    assert_eq!(rep.changed, 0);
    assert!(back.plies.iter().all(|p| p.status() == PlyStatus::Ok));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn unset_reference_blocks_nesting_and_mirror_needs_recheck() {
    let mut proj = Project::new("t");
    proj.import_files(&[demo().join("samples/rib.dxf"), demo().join("samples/flange.dxf")]).unwrap();
    let rib = proj.plies.iter().find(|p| p.part == "rib").unwrap().id;
    let flange = proj.plies.iter().find(|p| p.part == "flange").unwrap().id;
    // Flange has a FIBRE line: prefilled and accepted.
    assert_eq!(proj.ply(flange).unwrap().status(), PlyStatus::Ok);
    assert_eq!(proj.ply(rib).unwrap().status(), PlyStatus::Unset);
    let v = [plyroll_core::model::Variant { angle_deg: 45.0, qty: 1, kit: None, split: None }];
    proj.set_variants(&[rib, flange], &v).unwrap();
    let err = proj.to_plies().unwrap_err().to_string();
    assert!(err.contains("rib"), "{err}");
    proj.set_reference(&[rib], &RefPick::Edge { contour: 0, seg: 0 }).unwrap();
    assert!(proj.to_plies().is_ok());
    let before = proj.ply(flange).unwrap().reference.unwrap().deg;
    proj.set_mirrored(&[flange], true).unwrap();
    assert_eq!(proj.ply(flange).unwrap().status(), PlyStatus::Check);
    assert!((proj.ply(flange).unwrap().reference.unwrap().deg + before).abs() < 1e-6);
    assert!(proj.to_plies().is_err());
    proj.set_reference(&[flange], &RefPick::Confirm).unwrap();
    proj.set_reference(&[flange], &RefPick::Rotate { delta: 15.0 }).unwrap();
    assert!((proj.ply(flange).unwrap().reference.unwrap().deg - (15.0 - before)).abs() < 1e-6);
    assert!(proj.to_plies().is_ok());
}

/// The demo job in a temporary folder, so files can be removed.
fn demo_copy(name: &str) -> (PathBuf, Project) {
    let dir = std::env::temp_dir().join(format!("plyroll-imp-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("samples")).unwrap();
    for e in std::fs::read_dir(demo().join("samples")).unwrap() {
        let e = e.unwrap();
        std::fs::copy(e.path(), dir.join("samples").join(e.file_name())).unwrap();
    }
    std::fs::copy(demo().join("job.csv"), dir.join("job.csv")).unwrap();
    let rows = read_job_csv(&dir.join("job.csv")).unwrap();
    let proj = Project::from_job("demo", &rows, Default::default()).unwrap().0;
    (dir, proj)
}

#[test]
fn changing_units_rescales_plies_and_keeps_confirmed_references() {
    use plyroll_core::import::UnitsOverride;
    use plyroll_core::project::ImportSettings;
    let (dir, mut proj) = demo_copy("units");
    let mut material = plyroll_core::job::load_materials(&demo().join("materials.toml")).unwrap().remove(0);
    // Wide enough for the plies read as inches (25.4 times larger).
    material.roll_width = 10_000.0;
    // A mirrored ply must also count as only rescaled.
    let skin = proj.plies.iter().find(|p| p.part == "skin").unwrap().id;
    proj.set_mirrored(&[skin], true).unwrap();
    proj.set_reference(&[skin], &RefPick::Confirm).unwrap();
    let before = proj.clone();
    let fp = |p: &Project| p.nest_input(Some(&material)).unwrap().fingerprint;
    assert!(proj.plies.iter().all(|p| p.status() == PlyStatus::Ok));

    let sources = proj.sources();
    assert_eq!(sources.iter().map(|s| s.plies).sum::<usize>(), proj.plies.len());
    assert!(sources.iter().all(|s| s.found && s.file_units.is_some()), "{sources:?}");
    let declared_inch: Vec<&PathBuf> = sources.iter().filter(|s| s.file_units.as_deref() == Some("inch")).map(|s| &s.path).collect();

    // Tolerances are in mm after scaling: at the default gap tolerance the
    // gusset's 0.004 gap (0.10 mm in inch) no longer closes, and nothing changes.
    let inch_default = ImportSettings { units: UnitsOverride::Inch, ..Default::default() };
    let err = format!("{:#}", proj.set_import(inch_default).unwrap_err());
    assert!(err.contains("gusset") && err.contains("gap tolerance"), "{err}");
    assert_eq!(proj.import.units, UnitsOverride::Auto);
    assert_eq!(fp(&proj), fp(&before));

    let inch = ImportSettings { units: UnitsOverride::Inch, gap_tol: 0.2, ..Default::default() };
    let rep = proj.set_import(inch.clone()).unwrap();
    // The gusset's gap is now bridged instead of snapped, which moves a corner
    // by 0.1 mm: a real change of outline, so it alone is flagged for a check.
    assert_eq!((rep.added, rep.changed), (0, 1), "{rep:?}");
    assert_eq!(rep.rescaled + rep.unchanged + rep.changed, proj.plies.len());
    assert!(rep.rescaled > 0);
    for (a, b) in proj.plies.iter().zip(&before.plies) {
        let want = if a.part == "gusset" { PlyStatus::Check } else { PlyStatus::Ok };
        assert_eq!(a.status(), want, "{}", a.part);
        assert_eq!(a.reference, b.reference);
        assert_eq!(a.variants, b.variants);
        assert_eq!(a.shape.name, b.shape.name);
        let k = if declared_inch.contains(&&a.shape.source_file) { 1.0 } else { 25.4 * 25.4 };
        let (aa, ab) = (a.shape.area(0.01), b.shape.area(0.01));
        assert!((aa / ab - k).abs() < 1e-3 * k, "{}: {aa} vs {ab}", a.part);
    }
    let gusset = proj.plies.iter().find(|p| p.part == "gusset").unwrap().id;
    assert!(proj.nest_input(Some(&material)).is_err(), "a flagged ply blocks nesting");
    proj.set_reference(&[gusset], &RefPick::Confirm).unwrap();
    assert_ne!(fp(&proj), fp(&before), "a rescale makes the saved result stale");

    // Back to the file's own units: the original geometry again.
    let rep = proj.set_import(ImportSettings::default()).unwrap();
    assert_eq!(rep.changed, 0);
    assert_eq!(fp(&proj), fp(&before));

    // A removed ply is not brought back by a settings change.
    let multi = proj.sources().into_iter().find(|s| s.plies > 1).expect("a DXF with several parts").path;
    let gone = proj.plies.iter().find(|p| p.shape.source_file == multi).unwrap().id;
    proj.remove(&[gone]);
    let rep = proj.set_import(ImportSettings { gap_tol: 0.2, ..Default::default() }).unwrap();
    assert_eq!(rep.added, 0);
    assert!(rep.warnings.iter().any(|w| w.contains("not in the project")), "{:?}", rep.warnings);
    assert!(proj.plies.iter().all(|p| p.id != gone));

    // Bad tolerances and missing files are refused, leaving the settings as they were.
    assert!(proj.set_import(ImportSettings { join_tol: 0.5, gap_tol: 0.1, ..Default::default() }).is_err());
    std::fs::remove_file(dir.join("samples/rib.dxf")).unwrap();
    let err = proj.set_import(inch).unwrap_err().to_string();
    assert!(err.contains("rib.dxf"), "{err}");
    assert_eq!(proj.import.units, UnitsOverride::Auto);
    assert!(!proj.sources().iter().find(|s| s.path.ends_with("rib.dxf")).unwrap().found);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn unitless_dxf_switched_to_inch_keeps_its_reference() {
    use dxf::entities::{Entity, EntityType, LwPolyline};
    use dxf::enums::{AcadVersion, Units};
    use dxf::{Drawing, LwPolylineVertex};
    use plyroll_core::import::UnitsOverride;
    use plyroll_core::project::ImportSettings;
    let dir = std::env::temp_dir().join(format!("plyroll-unitless-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("plate.dxf");
    let mut d = Drawing::new();
    d.header.version = AcadVersion::R2000; // R12 has no LWPOLYLINE
    d.header.default_drawing_units = Units::Unitless;
    let mut p = LwPolyline::default();
    for (x, y) in [(0.0, 0.0), (4.0, 0.0), (4.0, 2.0), (1.0, 2.0)] {
        p.vertices.push(LwPolylineVertex { x, y, ..Default::default() });
    }
    p.set_is_closed(true);
    d.add_entity(Entity::new(EntityType::LwPolyline(p)));
    d.save_file(&file).unwrap();

    let mut proj = Project::new("t");
    let rep = proj.import_files(std::slice::from_ref(&file)).unwrap();
    assert!(rep.warnings.iter().any(|w| w.contains("$INSUNITS")), "{:?}", rep.warnings);
    assert_eq!(proj.sources()[0].file_units.as_deref(), Some("none"));
    let id = proj.plies[0].id;
    proj.set_reference(&[id], &RefPick::LongestEdge).unwrap();
    let deg = proj.ply(id).unwrap().reference.unwrap().deg;
    let area = proj.plies[0].shape.area(0.01);
    assert!((area - 7.0).abs() < 1e-6, "read as mm: {area}");

    let rep = proj.set_import(ImportSettings { units: UnitsOverride::Inch, ..Default::default() }).unwrap();
    assert_eq!((rep.rescaled, rep.changed, rep.added), (1, 0, 0), "{rep:?}");
    let ply = proj.ply(id).unwrap();
    assert_eq!(ply.status(), PlyStatus::Ok);
    assert_eq!(ply.reference.unwrap().deg, deg);
    assert!((ply.shape.area(0.01) - 7.0 * 25.4 * 25.4).abs() < 1e-3);
    std::fs::remove_dir_all(&dir).ok();
}
