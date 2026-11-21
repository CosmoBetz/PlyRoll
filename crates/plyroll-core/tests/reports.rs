//! Kit checklist, labels.csv and report.pdf.

use plyroll_core::export::placed_path;
use plyroll_core::geom::{BBox, FlattenMode};
use plyroll_core::job::{load_materials, read_job_csv};
use plyroll_core::model::{Layout, Placement, PlyShape};
use plyroll_core::project::Project;
use plyroll_core::report::{ReportMeta, RunInfo, checklist, cut_plies, summarize, write_outputs};
use std::path::PathBuf;

fn demo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/demo")
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("plyroll-rep-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The demo plies, with one shape's kit removed, and a layout that puts every
/// copy on its own 1 m stretch of roll, fibre along the roll. Valid, but no nest.
fn demo_layout() -> (Vec<PlyShape>, Layout) {
    let rows = read_job_csv(&demo().join("job.csv")).unwrap();
    let project = Project::from_job("demo", &rows, Default::default()).unwrap().0;
    let material = load_materials(&demo().join("materials.toml")).unwrap().remove(0);
    let mut plies = project.nest_input(Some(&material)).unwrap().plies;
    for v in &mut plies[1].variants {
        v.kit = None;
    }
    let mut placements = Vec::new();
    for (s, p) in plies.iter().enumerate() {
        for (v, var) in p.variants.iter().enumerate() {
            for copy in 1..=var.qty {
                let rotation_deg = -p.fibre_deg(var);
                placements.push(Placement { shape_idx: s, variant_idx: v, copy, rotation_deg, tx: 0.0, ty: 0.0 });
            }
        }
    }
    // Back to front along the roll, so cut order differs from project order.
    let n = placements.len();
    for (i, pl) in placements.iter_mut().enumerate() {
        let b = BBox::of(&placed_path(&plies[pl.shape_idx].shape.outer, pl).flatten(0.1, FlattenMode::Enclosing));
        pl.tx = 1000.0 * (n - i) as f64 - b.min.x;
        pl.ty = material.edge_trim + 1.0 - b.min.y;
    }
    let used_length = 1000.0 * (n + 2) as f64;
    (plies, Layout { material, placements, used_length, sections: vec![] })
}

#[test]
fn checklist_lists_every_cut_ply_once_in_project_order() {
    let (plies, layout) = demo_layout();
    let groups = checklist(&plies, &layout);
    let kits: Vec<Option<&str>> = groups.iter().map(|g| g.kit.as_deref()).collect();
    assert_eq!(*kits.last().unwrap(), None, "plies without a kit come last: {kits:?}");
    assert_eq!(kits.iter().filter(|k| k.is_none()).count(), 1);

    let mut seen: Vec<usize> = groups.iter().flat_map(|g| g.rows.iter().flat_map(|r| r.cuts.iter().map(|c| c.placement))).collect();
    seen.sort_unstable();
    assert_eq!(seen, (0..layout.placements.len()).collect::<Vec<_>>(), "each placement exactly once");

    for g in &groups {
        // Rows follow the project order (shape, then angle).
        let keys: Vec<(usize, usize)> = g
            .rows
            .iter()
            .map(|r| {
                let pl = &layout.placements[r.cuts[0].placement];
                (pl.shape_idx, pl.variant_idx)
            })
            .collect();
        assert!(keys.windows(2).all(|w| w[0] < w[1]), "{keys:?}");
        for r in &g.rows {
            assert!(r.cuts.iter().all(|c| c.kit == g.kit && c.label == r.label));
            assert!(r.cuts.windows(2).all(|w| w[0].cut_no < w[1].cut_no));
            assert_eq!(r.cuts.len() as u32, r.cuts[0].of, "qty matches the variant");
        }
    }

    // Cut numbers run along the roll: placements were laid out back to front.
    let cuts = cut_plies(&plies, &layout);
    assert_eq!(cuts.first().unwrap().placement, layout.placements.len() - 1);
    assert!(cuts.windows(2).all(|w| w[0].x_mm < w[1].x_mm));
}

#[test]
fn verified_layouts_get_a_pdf_and_labels_matching_plies_csv() {
    let (plies, layout) = demo_layout();
    let sum = summarize(&plies, &layout);
    assert!(sum.verification.ok, "{:?}", sum.verification.issues);
    let dir = tmp("ok");
    let meta = ReportMeta { job: "Demo ü".into(), date: "2026-11-21 10:00 UTC".into() };
    let written = write_outputs(&dir, &plies, &layout, &sum, &RunInfo::default(), &["a warning".into()], &meta).unwrap();
    let names: Vec<String> = written.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["report.json", "layout.svg", "layout.dxf", "plies.csv", "report.pdf", "labels.csv"]);

    let pdf = std::fs::read(dir.join("report.pdf")).unwrap();
    assert!(pdf.starts_with(b"%PDF-"));
    assert!(pdf.trim_ascii_end().ends_with(b"%%EOF"));
    // A layout this long is split into strips over several pages, then the checklist.
    let pat = b"/Type /Page";
    let pages = pdf.windows(pat.len() + 1).filter(|w| &w[..pat.len()] == pat && w[pat.len()] != b's').count();
    assert!(pages >= 3, "pages: {pages}");

    // labels.csv and plies.csv agree row by row.
    let mut labels = csv::Reader::from_path(dir.join("labels.csv")).unwrap();
    let mut plies_csv = csv::Reader::from_path(dir.join("plies.csv")).unwrap();
    let l: Vec<csv::StringRecord> = labels.records().map(Result::unwrap).collect();
    let p: Vec<csv::StringRecord> = plies_csv.records().map(Result::unwrap).collect();
    assert_eq!(l.len(), layout.placements.len());
    assert_eq!(l.len(), p.len());
    for (a, b) in l.iter().zip(&p) {
        assert_eq!(&a[0], &b[0], "cut_no = order");
        assert_eq!(&a[1], &b[1], "label = ply");
        assert_eq!(&a[8], "Demo ü");
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn failed_verification_writes_no_report_pdf_or_labels() {
    let (plies, mut layout) = demo_layout();
    let first = layout.placements[0].clone();
    layout.placements[1].tx = first.tx;
    layout.placements[1].ty = first.ty;
    let sum = summarize(&plies, &layout);
    assert!(!sum.verification.ok);
    let dir = tmp("bad");
    let meta = ReportMeta { job: "bad".into(), date: "x".into() };
    let written = write_outputs(&dir, &plies, &layout, &sum, &RunInfo::default(), &[], &meta).unwrap();
    let names: Vec<String> = written.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["report.json", "layout.svg"]);
    assert!(!dir.join("report.pdf").exists() && !dir.join("labels.csv").exists());
    std::fs::remove_dir_all(&dir).ok();
}
