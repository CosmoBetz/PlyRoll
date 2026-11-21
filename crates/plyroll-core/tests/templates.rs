//! templates.pdf: tiling the roll over pages at true size.

use plyroll_core::export::placed_path;
use plyroll_core::geom::{BBox, FlattenMode};
use plyroll_core::job::{load_materials, read_job_csv};
use plyroll_core::model::{Layout, Placement, PlyShape};
use plyroll_core::project::Project;
use plyroll_core::report::{OutputOptions, ReportMeta, RunInfo, summarize, write_outputs};
use plyroll_core::template::{MARGIN_MM, MAX_PAGE_MM, OVERLAP_MM, Paper, TilePlan, plan_tiles, printed_tiles, row_name, templates_pdf};
use std::path::PathBuf;

fn demo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/demo")
}

/// The demo plies, every copy on its own 1 m of roll (valid, mostly empty).
fn spread_layout() -> (Vec<PlyShape>, Layout) {
    let rows = read_job_csv(&demo().join("job.csv")).unwrap();
    let project = Project::from_job("demo", &rows, Default::default()).unwrap().0;
    let material = load_materials(&demo().join("materials.toml")).unwrap().remove(0);
    let plies = project.nest_input(Some(&material)).unwrap().plies;
    let mut placements = Vec::new();
    for (s, p) in plies.iter().enumerate() {
        for (v, var) in p.variants.iter().enumerate() {
            for copy in 1..=var.qty {
                placements.push(Placement { shape_idx: s, variant_idx: v, copy, rotation_deg: -p.fibre_deg(var), tx: 0.0, ty: 0.0 });
            }
        }
    }
    for (i, pl) in placements.iter_mut().enumerate() {
        let b = BBox::of(&placed_path(&plies[pl.shape_idx].shape.outer, pl).flatten(0.1, FlattenMode::Enclosing));
        pl.tx = 1000.0 * (i + 1) as f64 - b.min.x;
        pl.ty = material.edge_trim + 1.0 - b.min.y;
    }
    let used_length = 1000.0 * (placements.len() + 1) as f64;
    (plies, Layout { material, placements, used_length, sections: vec![] })
}

/// Tiles cover the roll exactly, neighbours overlap by OVERLAP_MM, and every
/// tile fits its page inside the margins.
fn check_plan(plan: &TilePlan, length: f64, width: f64) {
    assert_eq!(plan.tiles.len(), plan.rows * plan.cols);
    let eps = 1e-6;
    for t in &plan.tiles {
        assert!(t.x1 - t.x0 <= plan.page_w - 2.0 * MARGIN_MM + eps, "{t:?} on {plan:?}");
        assert!(t.y1 - t.y0 <= plan.page_h - 2.0 * MARGIN_MM + eps, "{t:?}");
        // Same size everywhere.
        let f = &plan.tiles[0];
        assert!(((t.x1 - t.x0) - (f.x1 - f.x0)).abs() < eps && ((t.y1 - t.y0) - (f.y1 - f.y0)).abs() < eps);
    }
    let first = plan.tile(plan.rows - 1, 0).unwrap();
    assert!(first.x0.abs() < eps && first.y0.abs() < eps, "bottom-left tile starts at the roll start and edge");
    let last = plan.tile(0, plan.cols - 1).unwrap();
    assert!((last.x1 - length).abs() < eps && (last.y1 - width).abs() < eps, "top-right tile ends at the roll end and far edge: {last:?}");
    for r in 0..plan.rows {
        for c in 0..plan.cols {
            let t = plan.tile(r, c).unwrap();
            assert_eq!((t.row, t.col), (r, c));
            if let Some(n) = plan.tile(r, c + 1) {
                assert!((t.x1 - n.x0 - OVERLAP_MM).abs() < eps, "columns overlap");
                assert_eq!((n.y0, n.y1), (t.y0, t.y1));
            }
            if let Some(below) = plan.tile(r + 1, c) {
                assert!((below.y1 - t.y0 - OVERLAP_MM).abs() < eps, "row A is on top; rows overlap");
            }
        }
    }
}

#[test]
fn tiles_cover_the_roll_and_overlap() {
    for (len, width) in [(2370.0, 600.0), (11_500.0, 1000.0), (150.0, 300.0), (263.9, 184.0)] {
        for paper in [Paper::A4, Paper::A3, Paper::Letter, Paper::Roll { width_mm: 914.0 }, Paper::Roll { width_mm: 420.0 }] {
            let plan = plan_tiles(len, width, &paper);
            check_plan(&plan, len, width);
            if let Paper::Roll { width_mm } = paper {
                assert_eq!(plan.page_h, width_mm);
                assert!(plan.page_w <= MAX_PAGE_MM + 1e-9, "a plotter page stays under the PDF limit");
            }
        }
    }
    // A4 on a 600 mm roll: landscape (271 × 184 mm usable) gives 4 × 10 tiles,
    // portrait (184 × 271 mm) 3 × 14. Landscape wins.
    let a4 = plan_tiles(2370.0, 600.0, &Paper::A4);
    assert_eq!((a4.page_w, a4.page_h, a4.rows, a4.cols), (297.0, 210.0, 4, 10));
    // 11.5 m on a 914 mm plotter: three equal pages, one row.
    let roll = plan_tiles(11_500.0, 600.0, &Paper::Roll { width_mm: 914.0 });
    assert_eq!((roll.rows, roll.cols), (1, 3));
    assert!((roll.page_w - ((11_500.0 + 2.0 * OVERLAP_MM) / 3.0 + 2.0 * MARGIN_MM)).abs() < 1e-9);
    // Something smaller than a page is one tile.
    assert_eq!(plan_tiles(100.0, 100.0, &Paper::A4).tiles.len(), 1);
    assert_eq!([row_name(0), row_name(25), row_name(26), row_name(27), row_name(701), row_name(702)], ["A", "Z", "AA", "AB", "ZZ", "AAA"]);
    assert!(Paper::Roll { width_mm: 50.0 }.validate().is_err());
    assert!(Paper::Roll { width_mm: f64::NAN }.validate().is_err());
}

#[test]
fn only_tiles_with_plies_are_printed() {
    let (plies, layout) = spread_layout();
    let plan = plan_tiles(layout.used_length, layout.material.roll_width, &Paper::A4);
    let boxes: Vec<BBox> = layout.placements.iter().map(|pl| BBox::of(&placed_path(&plies[pl.shape_idx].shape.outer, pl).flatten(0.1, FlattenMode::Enclosing))).collect();
    let printed = printed_tiles(&plan, &boxes);
    let n = printed.iter().filter(|p| **p).count();
    assert!(n > 0 && n < plan.tiles.len() / 2, "the spread layout is mostly empty roll: {n} of {}", plan.tiles.len());
    // Every point of every ply is on a printed tile: sample each box's corners and centre.
    for b in &boxes {
        for (x, y) in [(b.min.x, b.min.y), (b.max.x, b.max.y), (b.min.x, b.max.y), (b.max.x, b.min.y), ((b.min.x + b.max.x) / 2.0, (b.min.y + b.max.y) / 2.0)] {
            let on = plan.tiles.iter().zip(&printed).any(|(t, p)| *p && x >= t.x0 && x <= t.x1 && y >= t.y0 && y <= t.y1);
            assert!(on, "({x}, {y}) of {b:?} is not printed");
        }
    }

    let meta = ReportMeta { job: "spread".into(), date: "2026-11-21 10:00 UTC".into(), roll: None };
    let (pdf, pages) = templates_pdf(&plies, &layout, &Paper::A4, &meta);
    assert_eq!(pages, n);
    assert!(pdf.starts_with(b"%PDF-") && pdf.trim_ascii_end().ends_with(b"%%EOF"));
    let pat = b"/Type /Page";
    let count = pdf.windows(pat.len() + 1).filter(|w| &w[..pat.len()] == pat && w[pat.len()] != b's').count();
    assert_eq!(count, n + 1, "an index page, then one page per printed tile");
    let has = |s: &[u8]| pdf.windows(s.len()).any(|w| w == s);
    assert!(has(b"/PrintScaling /None"), "asks the print dialog for actual size");
    // Page sizes in mm: the index is A4 landscape, the tiles A4 the way the plan turned them.
    let text = String::from_utf8_lossy(&pdf);
    let sizes: Vec<(f64, f64)> = text
        .split("/MediaBox [")
        .skip(1)
        .map(|s| {
            let v: Vec<f64> = s.split(']').next().unwrap().split_whitespace().map(|n| n.parse().unwrap()).collect();
            (v[2] * 25.4 / 72.0, v[3] * 25.4 / 72.0)
        })
        .collect();
    assert_eq!(sizes.len(), n + 1);
    let near = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).abs() < 0.01 && (a.1 - b.1).abs() < 0.01;
    assert!(sizes.iter().filter(|s| near(**s, (297.0, 210.0))).count() >= 1, "landscape index: {sizes:?}");
    assert_eq!(sizes.iter().filter(|s| near(**s, (plan.page_w, plan.page_h))).count(), if plan.page_w > plan.page_h { n + 1 } else { n });
}

#[test]
fn templates_are_written_only_when_asked_and_verified() {
    let (plies, layout) = spread_layout();
    let sum = summarize(&plies, &layout);
    assert!(sum.verification.ok, "{:?}", sum.verification.issues);
    let dir = std::env::temp_dir().join(format!("plyroll-tpl-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let meta = ReportMeta::new("t");
    let names = |w: Vec<PathBuf>| w.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect::<Vec<_>>();
    let w = write_outputs(&dir, &plies, &layout, &sum, &RunInfo::default(), &[], &meta, &OutputOptions::default()).unwrap();
    assert!(!names(w).contains(&"templates.pdf".to_string()));
    let opts = OutputOptions { templates: Some(Paper::Roll { width_mm: 914.0 }) };
    let w = write_outputs(&dir, &plies, &layout, &sum, &RunInfo::default(), &[], &meta, &opts).unwrap();
    assert_eq!(names(w).last().unwrap(), "templates.pdf");
    // Exported again without templates: the old templates.pdf goes.
    write_outputs(&dir, &plies, &layout, &sum, &RunInfo::default(), &[], &meta, &OutputOptions::default()).unwrap();
    assert!(!dir.join("templates.pdf").exists());
    write_outputs(&dir, &plies, &layout, &sum, &RunInfo::default(), &[], &meta, &opts).unwrap();

    let mut bad = layout.clone();
    bad.placements[1].tx = bad.placements[0].tx;
    let bad_sum = summarize(&plies, &bad);
    assert!(!bad_sum.verification.ok);
    // Into the same folder: nothing from the good layout's cut files is left beside the failed one.
    let w = write_outputs(&dir, &plies, &bad, &bad_sum, &RunInfo::default(), &[], &meta, &opts).unwrap();
    assert!(!names(w).contains(&"templates.pdf".to_string()));
    for f in ["templates.pdf", "layout.dxf", "plies.csv", "report.pdf", "labels.csv"] {
        assert!(!dir.join(f).exists(), "{f} is left from the earlier export");
    }
    assert!(dir.join("report.json").exists() && dir.join("layout.svg").exists());
    std::fs::remove_dir_all(&dir).ok();
}
