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
    let v = [plyroll_core::model::Variant { angle_deg: 45.0, qty: 1, kit: None }];
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
