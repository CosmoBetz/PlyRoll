//! Kit-grouped nesting: splitting a job into kit sections and joining them.

use plyroll_core::job::{load_materials, read_job_csv};
use plyroll_core::kits::{Grouping, kit_order, merge, section_material, sections};
use plyroll_core::model::{Layout, Placement, PlyShape};
use plyroll_core::project::{JobSettings, NestResult, Project};
use plyroll_core::report::RunInfo;
use std::path::PathBuf;

fn demo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/demo")
}

fn project() -> Project {
    let rows = read_job_csv(&demo().join("job.csv")).unwrap();
    let mut p = Project::from_job("demo", &rows, Default::default()).unwrap().0;
    let material = load_materials(&demo().join("materials.toml")).unwrap().remove(0);
    p.set_job(JobSettings { material: Some(material), sets: 2, ..Default::default() }).unwrap();
    p
}

/// Demo plies with one shape's variants spread over a new kit and no kit.
fn plies() -> Vec<PlyShape> {
    let mut plies = project().nest_input(None).unwrap().plies;
    let v = &mut plies[0].variants;
    assert!(v.len() >= 3);
    v[1].kit = Some("Z".into());
    v[2].kit = None;
    plies
}

#[test]
fn every_variant_lands_in_exactly_one_section_of_its_kit() {
    let plies = plies();
    let order = kit_order(&plies);
    assert_eq!(order.last(), Some(&None), "no kit comes last: {order:?}");
    let secs = sections(&plies);
    assert_eq!(secs.iter().map(|s| s.kit.clone()).collect::<Vec<_>>(), order);

    let mut seen = Vec::new();
    for sec in &secs {
        for (sp, (si, vars)) in sec.plies.iter().zip(&sec.index) {
            assert_eq!(sp.shape.name, plies[*si].shape.name);
            for (sv, &vi) in sp.variants.iter().zip(vars) {
                let full = &plies[*si].variants[vi];
                assert_eq!((sv.angle_deg, sv.qty, &sv.kit), (full.angle_deg, full.qty, &full.kit));
                assert_eq!(sv.kit, sec.kit);
                seen.push((*si, vi));
            }
        }
    }
    seen.sort_unstable();
    let mut all: Vec<(usize, usize)> =
        plies.iter().enumerate().flat_map(|(si, p)| (0..p.variants.len()).filter(move |&vi| p.variants[vi].qty > 0).map(move |vi| (si, vi))).collect();
    all.sort_unstable();
    assert_eq!(seen, all);
}

#[test]
fn merge_maps_back_and_lays_sections_end_to_end() {
    let plies = plies();
    let material = project().nest_input(None).unwrap().material;
    let secs = sections(&plies);
    // Fake section layouts: one placement of each section ply, each section 500 mm long.
    let mut start = material.lead_trim;
    let mut lays = Vec::new();
    for sec in &secs {
        let placements =
            (0..sec.plies.len()).map(|i| Placement { shape_idx: i, variant_idx: sec.plies[i].variants.len() - 1, copy: 1, rotation_deg: 0.0, tx: start, ty: 0.0 }).collect();
        let lay = Layout { material: section_material(&material, start), placements, used_length: start + 500.0, sections: vec![] };
        start += 500.0 + material.spacing;
        lays.push(lay);
    }
    let parts: Vec<_> = secs.iter().zip(&lays).collect();
    let merged = merge(&material, &parts).unwrap();
    assert_eq!(merged.material, material);
    assert_eq!(merged.sections.len(), secs.len());
    assert_eq!(merged.used_length, merged.sections.last().unwrap().end);
    let mut k = 0;
    for (sec, lay) in &parts {
        for p in &lay.placements {
            let m = &merged.placements[k];
            let (si, vars) = &sec.index[p.shape_idx];
            assert_eq!((m.shape_idx, m.variant_idx), (*si, vars[p.variant_idx]));
            assert_eq!(plies[m.shape_idx].variants[m.variant_idx].kit, sec.kit);
            k += 1;
        }
    }
    // A section nested at the wrong place is refused.
    let mut bad = lays.clone();
    bad[1].material.lead_trim += 1.0;
    let parts: Vec<_> = secs.iter().zip(&bad).collect();
    assert!(merge(&material, &parts).is_err());
}

#[test]
fn changing_the_grouping_makes_the_result_stale() {
    let mut p = project();
    let input = p.nest_input(None).unwrap();
    assert_eq!(input.grouping, Grouping::Global);
    let layout = Layout { material: input.material.clone(), placements: vec![], used_length: 100.0, sections: vec![] };
    p.result = Some(NestResult::new(input, layout, RunInfo::default()));
    assert!(p.freshness().unwrap().fresh);
    p.set_job(JobSettings { grouping: Grouping::ByKit, ..p.job.clone() }).unwrap();
    let f = p.freshness().unwrap();
    assert!(!f.fresh && f.reason.unwrap().contains("kit grouping"));
    p.set_job(JobSettings { grouping: Grouping::Global, ..p.job.clone() }).unwrap();
    assert!(p.freshness().unwrap().fresh);

    // The setting is saved with the project, and old files default to global.
    p.set_job(JobSettings { grouping: Grouping::ByKit, ..p.job.clone() }).unwrap();
    let json = serde_json::to_string(&p.job).unwrap();
    assert!(json.contains("\"grouping\":\"by-kit\""), "{json}");
    let old: JobSettings = serde_json::from_str(r#"{"material":null,"time_s":30,"sets":1}"#).unwrap();
    assert_eq!(old.grouping, Grouping::Global);
}
