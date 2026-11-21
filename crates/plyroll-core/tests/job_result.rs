//! Job settings, the material library and the stored nest result.

use plyroll_core::job::{load_materials, read_job_csv, save_materials};
use plyroll_core::model::{Layout, Material, Variant};
use plyroll_core::project::{JobSettings, NestResult, Project, RefPick};
use plyroll_core::report::RunInfo;
use std::path::PathBuf;

fn demo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/demo")
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("plyroll-jr-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ud() -> Material {
    load_materials(&demo().join("materials.toml")).unwrap().into_iter().find(|m| m.name == "UD-T700-150").unwrap()
}

fn project() -> Project {
    let rows = read_job_csv(&demo().join("job.csv")).unwrap();
    Project::from_job("demo", &rows, Default::default()).unwrap().0
}

#[test]
fn material_library_roundtrip() {
    let mut lib = load_materials(&demo().join("materials.toml")).unwrap();
    // One without a cost, to check that empty optional fields survive.
    lib[2].cost_per_m2 = None;
    let p = tmp("lib").join("sub/materials.toml");
    save_materials(&p, &lib).unwrap();
    assert_eq!(load_materials(&p).unwrap(), lib);

    let mut bad = lib[0].clone();
    bad.edge_trim = 400.0;
    assert!(save_materials(&p, &[bad]).is_err(), "invalid material must not be written");
    assert_eq!(load_materials(&p).unwrap(), lib, "failed save must leave the file alone");
}

#[test]
fn old_project_files_still_load() {
    // examples/demo/demo.plyroll was written before job settings existed.
    let p = Project::load(&demo().join("demo.plyroll")).unwrap();
    assert_eq!(p.job, JobSettings::default());
    assert!(p.result.is_none());
    assert!(p.freshness().is_none());
}

#[test]
fn sets_multiply_quantities_and_material_is_required() {
    let mut p = project();
    let err = p.nest_input(None).unwrap_err().to_string();
    assert!(err.contains("material"), "{err}");

    let one: u32 = p.nest_input(Some(&ud())).unwrap().plies.iter().flat_map(|s| s.variants.iter().map(|v| v.qty)).sum();
    p.set_job(JobSettings { material: Some(ud()), time_s: 30, sets: 3 }).unwrap();
    let input = p.nest_input(None).unwrap();
    let three: u32 = input.plies.iter().flat_map(|s| s.variants.iter().map(|v| v.qty)).sum();
    assert_eq!(three, 3 * one);
    assert_eq!(input.plies.len(), input.ply_ids.len());

    assert!(p.set_job(JobSettings { sets: 0, ..p.job.clone() }).is_err());
    assert!(p.set_job(JobSettings { time_s: 0, ..p.job.clone() }).is_err());
    let mut m = ud();
    m.roll_width = -1.0;
    assert!(p.set_job(JobSettings { material: Some(m), ..p.job.clone() }).is_err());
}

#[test]
fn result_goes_stale_on_edits_and_survives_save() {
    let mut p = project();
    p.set_job(JobSettings { material: Some(ud()), time_s: 30, sets: 1 }).unwrap();
    let input = p.nest_input(None).unwrap();
    let layout = Layout { material: input.material.clone(), placements: vec![], used_length: 100.0 };
    p.result = Some(NestResult::new(input, layout, RunInfo { seed: 1, solve_seconds: 2.0, stopped: false }));
    assert!(p.freshness().unwrap().fresh);

    // Editing a quantity makes it stale; changing it back makes it fresh again.
    let id = p.plies[0].id;
    let before = p.plies[0].variants.clone();
    let mut more = before.clone();
    more.push(Variant { angle_deg: 90.0, qty: 1, kit: None });
    p.set_variants(&[id], &more).unwrap();
    let f = p.freshness().unwrap();
    assert!(!f.fresh && f.reason.is_some());
    p.set_variants(&[id], &before).unwrap();
    assert!(p.freshness().unwrap().fresh);

    // A different 0° or material also invalidates it.
    p.set_reference(&[id], &RefPick::Rotate { delta: 1.0 }).unwrap();
    assert!(!p.freshness().unwrap().fresh);
    p.set_reference(&[id], &RefPick::Rotate { delta: -1.0 }).unwrap();
    assert!(p.freshness().unwrap().fresh);
    let mut wider = ud();
    wider.roll_width = 1000.0;
    p.set_job(JobSettings { material: Some(wider), ..p.job.clone() }).unwrap();
    assert!(!p.freshness().unwrap().fresh);
    p.set_job(JobSettings { material: Some(ud()), ..p.job.clone() }).unwrap();

    // Save to another folder and load: still fresh.
    let path = tmp("res").join("p.plyroll");
    p.save(&path).unwrap();
    let back = Project::load(&path).unwrap();
    assert_eq!(back.job, p.job);
    assert!(back.freshness().unwrap().fresh);
    assert_eq!(back.result.as_ref().unwrap().layout, p.result.as_ref().unwrap().layout);
}
