//! Roll inventory: file round trip, validation, booking, and the roll on the report.

use plyroll_core::job::{load_materials, read_job_csv};
use plyroll_core::model::Layout;
use plyroll_core::project::{JobSettings, NestResult, Project};
use plyroll_core::report::{ReportMeta, RunInfo};
use plyroll_core::rolls::{Roll, RollInfo, Usage, book, load_rolls, result_key, save_rolls, unbook, validate_rolls};
use std::path::PathBuf;

fn demo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/demo")
}

fn roll(id: &str, left: f64) -> Roll {
    Roll {
        id: id.into(),
        material: "UD-T700-150".into(),
        batch: "B-2311".into(),
        remaining_m: left,
        expires: Some("2027-03-01".into()),
        note: String::new(),
        usage: vec![],
    }
}

fn usage(key: &str, m: f64) -> Usage {
    Usage { job: "demo".into(), date: "2026-11-21 10:00 UTC".into(), length_m: m, result: key.into() }
}

#[test]
fn inventory_roundtrip_and_validation() {
    let dir = std::env::temp_dir().join(format!("plyroll-rolls-{}", std::process::id()));
    let path = dir.join("rolls.toml");
    let mut rolls = vec![roll("R1", 25.0), Roll { expires: None, batch: String::new(), note: "freezer 2".into(), ..roll("R2", 3.5) }];
    rolls[0].usage.push(usage("abc:1", 2.0));
    save_rolls(&path, &rolls).unwrap();
    assert_eq!(load_rolls(&path).unwrap(), rolls);

    for bad in [
        vec![roll("R1", 1.0), roll("r1", 2.0)],               // same id, any case
        vec![Roll { remaining_m: -1.0, ..roll("R1", 0.0) }],  // negative
        vec![Roll { remaining_m: f64::NAN, ..roll("R1", 0.0) }],
        vec![Roll { expires: Some("2027-02-30".into()), ..roll("R1", 1.0) }], // no such day
        vec![Roll { expires: Some("1.3.2027".into()), ..roll("R1", 1.0) }],
        vec![Roll { material: " ".into(), ..roll("R1", 1.0) }],
        vec![Roll { id: "".into(), ..roll("R1", 1.0) }],
    ] {
        assert!(validate_rolls(&bad).is_err(), "{bad:?}");
        assert!(save_rolls(&path, &bad).is_err());
    }
    assert_eq!(load_rolls(&path).unwrap(), rolls, "a refused save leaves the file alone");
    assert!(validate_rolls(&[Roll { expires: Some("2028-02-29".into()), ..roll("R1", 1.0) }]).is_ok(), "leap day");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn booking_takes_length_off_once_and_can_be_taken_back() {
    let mut rolls = vec![roll("R1", 5.0), roll("R2", 1.0)];
    book(&mut rolls, "R1", usage("k:1", 2.4)).unwrap();
    assert!((rolls[0].remaining_m - 2.6).abs() < 1e-9);
    assert!(book(&mut rolls, "R1", usage("k:1", 2.4)).is_err(), "same nest twice");
    assert!(book(&mut rolls, "R2", usage("k:1", 0.5)).is_err(), "same nest on another roll");
    let err = book(&mut rolls, "R2", usage("k:2", 1.5)).unwrap_err().to_string();
    assert!(err.contains("1.00 m left"), "{err}");
    assert_eq!(rolls[1].remaining_m, 1.0, "a refused booking changes nothing");
    assert!(book(&mut rolls, "R9", usage("k:3", 0.1)).is_err());

    // The report shows the length before this nest, even after booking it.
    let info = RollInfo::of(&rolls[0], "k:1", "2026-11-21");
    assert!((info.before_m - 5.0).abs() < 1e-9);
    assert!(!info.expired);
    assert!(RollInfo::of(&rolls[0], "k:1", "2027-03-02").expired);
    assert!((RollInfo::of(&rolls[0], "other", "2026-11-21").before_m - 2.6).abs() < 1e-9);

    // Lengths are stored to 0.1 mm.
    let mut r3 = vec![roll("R3", 3.0)];
    book(&mut r3, "R3", usage("k:9", 2.277680112156638)).unwrap();
    assert_eq!((r3[0].remaining_m, r3[0].usage[0].length_m), (0.7223, 2.2777));
    unbook(&mut r3, "R3", "k:9").unwrap();
    assert_eq!(r3[0].remaining_m, 3.0);

    let u = unbook(&mut rolls, "R1", "k:1").unwrap();
    assert_eq!(u.length_m, 2.4);
    assert!((rolls[0].remaining_m - 5.0).abs() < 1e-9 && rolls[0].usage.is_empty());
    assert!(unbook(&mut rolls, "R1", "k:1").is_err());
}

#[test]
fn picking_a_roll_keeps_the_result_fresh() {
    let rows = read_job_csv(&demo().join("job.csv")).unwrap();
    let mut p = Project::from_job("demo", &rows, Default::default()).unwrap().0;
    let ud = load_materials(&demo().join("materials.toml")).unwrap().remove(0);
    p.set_job(JobSettings { material: Some(ud), ..Default::default() }).unwrap();
    let input = p.nest_input(None).unwrap();
    let layout = Layout { material: input.material.clone(), placements: vec![], used_length: 100.0, sections: vec![] };
    p.result = Some(NestResult::new(input, layout, RunInfo { seed: 3, ..Default::default() }));
    assert!(p.freshness().unwrap().fresh);
    p.set_job(JobSettings { roll: Some("R1".into()), templates: Some(plyroll_core::template::Paper::A3), ..p.job.clone() }).unwrap();
    assert!(p.freshness().unwrap().fresh, "a roll or template paper does not change the layout");
    assert!(p.set_job(JobSettings { roll: Some(" ".into()), ..p.job.clone() }).is_err());
    assert!(p.set_job(JobSettings { templates: Some(plyroll_core::template::Paper::Roll { width_mm: 10.0 }), ..p.job.clone() }).is_err());

    // Saved and loaded, the settings stay; an old file without them loads as none.
    let json = serde_json::to_string(&p.job).unwrap();
    assert_eq!(serde_json::from_str::<JobSettings>(&json).unwrap(), p.job);
    let old: JobSettings = serde_json::from_str(r#"{"material":null,"time_s":30,"sets":1}"#).unwrap();
    assert_eq!((old.roll, old.templates), (None, None));
    let r = p.result.as_ref().unwrap();
    assert_eq!(result_key(&r.fingerprint, r.run.seed), format!("{}:3", r.fingerprint));
}

#[test]
fn roll_and_batch_reach_labels_and_report_json() {
    use plyroll_core::report::{summarize, write_outputs};
    let rows = read_job_csv(&demo().join("job.csv")).unwrap();
    let mut p = Project::from_job("demo", &rows, Default::default()).unwrap().0;
    let ud = load_materials(&demo().join("materials.toml")).unwrap().remove(0);
    p.set_job(JobSettings { material: Some(ud.clone()), ..Default::default() }).unwrap();
    // One ply only, alone on the roll, so the layout verifies.
    let mut plies = p.nest_input(None).unwrap().plies;
    plies.truncate(1);
    plies[0].variants.truncate(1);
    plies[0].variants[0].qty = 1;
    let v = plies[0].variants[0].clone();
    let rot = -plies[0].fibre_deg(&v);
    let mut pl = plyroll_core::model::Placement { shape_idx: 0, variant_idx: 0, copy: 1, rotation_deg: rot, tx: 0.0, ty: 0.0 };
    let b = plyroll_core::geom::BBox::of(&plyroll_core::export::placed_path(&plies[0].shape.outer, &pl).flatten(0.1, plyroll_core::geom::FlattenMode::Enclosing));
    pl.tx = ud.lead_trim + 1.0 - b.min.x;
    pl.ty = ud.edge_trim + 1.0 - b.min.y;
    let layout = Layout { material: ud, placements: vec![pl], used_length: b.width() + 100.0, sections: vec![] };
    let sum = summarize(&plies, &layout);
    assert!(sum.verification.ok, "{:?}", sum.verification.issues);

    let dir = std::env::temp_dir().join(format!("plyroll-rollrep-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let info = RollInfo::of(&roll("R7", 0.05), "", "2026-11-21");
    let meta = ReportMeta::new("demo").with_roll(Some(info));
    write_outputs(&dir, &plies, &layout, &sum, &RunInfo::default(), &[], &meta, &Default::default()).unwrap();
    let mut labels = csv::Reader::from_path(dir.join("labels.csv")).unwrap();
    let headers = labels.headers().unwrap().clone();
    assert_eq!(&headers[11], "roll");
    assert_eq!(&headers[12], "batch");
    let row = labels.records().next().unwrap().unwrap();
    assert_eq!((&row[11], &row[12]), ("R7", "B-2311"));
    let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("report.json")).unwrap()).unwrap();
    assert_eq!(json["roll"]["id"], "R7");
    assert_eq!(json["roll"]["batch"], "B-2311");
    // The roll is too short (0.05 m): the PDF says so.
    let pdf = std::fs::read(dir.join("report.pdf")).unwrap();
    assert!(pdf.starts_with(b"%PDF-"));
    std::fs::remove_dir_all(&dir).ok();
}
