//! Editing a nest by hand: moves, turns, locks, refitting and verification.

use plyroll_core::edit::Edit;
use plyroll_core::export::placed_path;
use plyroll_core::geom::{BBox, FlattenMode};
use plyroll_core::job::{load_materials, read_job_csv};
use plyroll_core::model::{Layout, Placement};
use plyroll_core::project::{JobSettings, NestResult, Project};
use plyroll_core::report::RunInfo;
use std::path::PathBuf;

fn demo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/demo")
}

/// The demo project with a valid result: every copy on its own 400 mm of roll.
fn project() -> Project {
    let rows = read_job_csv(&demo().join("job.csv")).unwrap();
    let mut p = Project::from_job("demo", &rows, Default::default()).unwrap().0;
    let ud = load_materials(&demo().join("materials.toml")).unwrap().remove(0);
    p.set_job(JobSettings { material: Some(ud), ..Default::default() }).unwrap();
    let input = p.nest_input(None).unwrap();
    let plies = &input.plies;
    let m = &input.material;
    let mut placements = Vec::new();
    for (s, ply) in plies.iter().enumerate() {
        for (v, var) in ply.variants.iter().enumerate() {
            for copy in 1..=var.qty {
                placements.push(Placement { shape_idx: s, variant_idx: v, copy, rotation_deg: (-ply.fibre_deg(var)).rem_euclid(360.0), tx: 0.0, ty: 0.0 });
            }
        }
    }
    let mut x = m.lead_trim;
    for pl in placements.iter_mut() {
        let b = bbox(plies, pl);
        pl.tx = x - b.min.x;
        pl.ty = m.edge_trim - b.min.y;
        x += b.width() + 100.0;
    }
    let mut layout = Layout { material: m.clone(), placements, used_length: 0.0, sections: vec![] };
    plyroll_core::edit::refit(plies, &mut layout);
    p.result = Some(NestResult::new(input, layout, RunInfo { seed: 7, ..Default::default() }));
    assert!(p.result.as_ref().unwrap().summary.verification.ok, "{:?}", p.result.as_ref().unwrap().summary.verification.issues);
    p
}

fn bbox(plies: &[plyroll_core::model::PlyShape], pl: &Placement) -> BBox {
    BBox::of(&placed_path(&plies[pl.shape_idx].shape.outer, pl).flatten(0.01, FlattenMode::Enclosing))
}

#[test]
fn moves_are_checked_and_refit_the_roll() {
    let mut p = project();
    let r = p.result.as_mut().unwrap();
    let before = r.layout.used_length;
    let b0 = bbox(&r.plies, &r.layout.placements[0]);
    let b1 = bbox(&r.plies, &r.layout.placements[1]);

    // Onto its neighbour: fails, both named.
    r.edit(&Edit::Move { indices: vec![0], dx: b1.min.x - b0.min.x, dy: 0.0 }).unwrap();
    assert!(!r.summary.verification.ok);
    assert_eq!(r.summary.verification.bad, vec![0, 1]);
    assert!(r.summary.verification.issues.iter().any(|i| i.contains("apart")), "{:?}", r.summary.verification.issues);
    // Back: passes again.
    r.edit(&Edit::Move { indices: vec![0], dx: b0.min.x - b1.min.x, dy: 0.0 }).unwrap();
    assert!(r.summary.verification.ok, "{:?}", r.summary.verification.issues);
    assert_eq!(r.run.edits, 2);

    // Off the roll edge and before the lead trim.
    r.edit(&Edit::Move { indices: vec![0], dx: 0.0, dy: -5.0 }).unwrap();
    assert_eq!(r.summary.verification.bad, vec![0]);
    r.edit(&Edit::Move { indices: vec![0], dx: -5.0, dy: 5.0 }).unwrap();
    assert!(r.summary.verification.issues.iter().any(|i| i.contains("usable roll area")));
    r.edit(&Edit::Move { indices: vec![0], dx: 5.0, dy: 0.0 }).unwrap();
    assert!(r.summary.verification.ok);

    // Moving the last ply on shortens or lengthens the used length with it.
    let last = r.layout.placements.len() - 1;
    r.edit(&Edit::Move { indices: vec![last], dx: 250.0, dy: 0.0 }).unwrap();
    assert!((r.layout.used_length - (before + 250.0)).abs() < 1e-6);
    assert!((r.summary.used_length_mm - r.layout.used_length).abs() < 1e-9);
    r.edit(&Edit::Move { indices: vec![last], dx: -60.0, dy: 0.0 }).unwrap();
    assert!((r.layout.used_length - (before + 190.0)).abs() < 1e-6);
    assert!(r.summary.verification.ok);

    // A group moves together.
    r.edit(&Edit::Move { indices: vec![2, 3], dx: 1.5, dy: 2.0 }).unwrap();
    assert!(r.summary.verification.ok);

    // Refused edits change nothing.
    let snap = r.snapshot();
    for bad in [
        Edit::Move { indices: vec![], dx: 1.0, dy: 0.0 },
        Edit::Move { indices: vec![999], dx: 1.0, dy: 0.0 },
        Edit::Move { indices: vec![1, 1], dx: 1.0, dy: 0.0 },
        Edit::Move { indices: vec![1], dx: f64::NAN, dy: 0.0 },
    ] {
        assert!(r.edit(&bad).is_err(), "{bad:?}");
    }
    assert_eq!(r.snapshot(), snap);
    // The edited project still counts as current: the plies did not change.
    assert!(p.freshness().unwrap().fresh);
}

#[test]
fn turns_keep_fibres_and_centre() {
    let mut p = project();
    let r = p.result.as_mut().unwrap();
    let c = |r: &NestResult| {
        let b = bbox(&r.plies, &r.layout.placements[0]);
        ((b.min.x + b.max.x) / 2.0, (b.min.y + b.max.y) / 2.0)
    };
    let (rot, centre) = (r.layout.placements[0].rotation_deg, c(r));
    r.edit(&Edit::Rotate { index: 0, by: 180.0 }).unwrap();
    let pl = &r.layout.placements[0];
    assert!(((pl.rotation_deg - rot).rem_euclid(360.0) - 180.0).abs() < 1e-9);
    let after = c(r);
    assert!((after.0 - centre.0).abs() < 0.05 && (after.1 - centre.1).abs() < 0.05, "{centre:?} → {after:?}");
    assert!(r.summary.verification.max_fibre_deviation < 1e-6);
    // UD without tolerance: 90° and 0.5° turns would put the fibres off the roll.
    let snap = r.snapshot();
    let e = r.edit(&Edit::Rotate { index: 0, by: 90.0 }).unwrap_err().to_string();
    assert!(e.contains("fibres would be off"), "{e}");
    assert!(r.edit(&Edit::Rotate { index: 0, by: 0.5 }).is_err());
    assert_eq!(r.snapshot(), snap);
    r.edit(&Edit::Rotate { index: 0, by: -180.0 }).unwrap();
    assert!((r.layout.placements[0].rotation_deg - rot).abs() < 1e-9);

    // With a 1° tolerance in 0.5° steps, fine turns up to 1° are allowed.
    let mut m = r.layout.material.clone();
    m.angle_tolerance = 1.0;
    r.layout.material = m;
    r.edit(&Edit::Rotate { index: 0, by: 0.5 }).unwrap();
    r.edit(&Edit::Rotate { index: 0, by: 0.5 }).unwrap();
    assert!(r.edit(&Edit::Rotate { index: 0, by: 0.5 }).is_err(), "1.5° is past the tolerance");
    assert!((r.summary.verification.max_fibre_deviation - 1.0).abs() < 1e-6);
}

#[test]
fn locked_plies_do_not_move_and_edits_survive_save() {
    let mut p = project();
    let r = p.result.as_mut().unwrap();
    r.edit(&Edit::Lock { indices: vec![3, 1], on: true }).unwrap();
    assert_eq!((r.locked.clone(), r.run.edits), (vec![1, 3], 0), "locking is not a hand edit");
    let e = r.edit(&Edit::Move { indices: vec![0, 1], dx: 1.0, dy: 0.0 }).unwrap_err().to_string();
    assert!(e.contains("locked"), "{e}");
    assert!(r.edit(&Edit::Rotate { index: 3, by: 180.0 }).is_err());
    r.edit(&Edit::Move { indices: vec![0], dx: 1.0, dy: 0.0 }).unwrap();
    r.edit(&Edit::Lock { indices: vec![1], on: false }).unwrap();
    assert_eq!(r.locked, vec![3]);
    r.edit(&Edit::Move { indices: vec![1], dx: 1.0, dy: 0.0 }).unwrap();

    // Undo puts the layout, locks and edit count back.
    let snap = r.snapshot();
    r.edit(&Edit::Move { indices: vec![0], dx: 500.0, dy: 0.0 }).unwrap();
    r.restore(snap.clone());
    assert_eq!(r.snapshot(), snap);
    assert!(r.summary.verification.ok);

    // Saved and opened again: still current, locks and edit count kept.
    let dir = std::env::temp_dir().join(format!("plyroll-edit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("demo.plyroll");
    p.save(&path).unwrap();
    let q = Project::load(&path).unwrap();
    let (a, b) = (p.result.as_ref().unwrap(), q.result.as_ref().unwrap());
    assert_eq!((&a.locked, a.run.edits, &a.layout), (&b.locked, b.run.edits, &b.layout));
    assert!(q.freshness().unwrap().fresh);
    std::fs::remove_dir_all(&dir).ok();

    // An old result without locks or edits loads as none.
    let mut json: serde_json::Value = serde_json::to_value(a).unwrap();
    json.as_object_mut().unwrap().remove("locked");
    json["run"].as_object_mut().unwrap().remove("edits");
    json["summary"]["verification"].as_object_mut().unwrap().remove("bad");
    let old: NestResult = serde_json::from_value(json).unwrap();
    assert_eq!((old.locked.len(), old.run.edits), (0, 0));
}
