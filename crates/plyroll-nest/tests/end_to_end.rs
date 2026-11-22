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
use plyroll_core::kits::Grouping;
use plyroll_core::report::cut_plies;
use plyroll_nest::{NestOptions, build_groups, nest, nest_after_locked, nest_job};

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
            variants: vec![Variant { angle_deg: 0.0, qty: 3, kit: None, split: None }, Variant { angle_deg: 45.0, qty: 2, kit: None, split: None }, Variant { angle_deg: -45.0, qty: 2, kit: None, split: None }],
        },
        PlyShape {
            shape: shape("plate", rounded_rect(140.0, 140.0, 10.0, 0.0, 0.0), vec![circle(70.0, 70.0, 25.0, false)]),
            ref_deg: 10.0,
            ref_origin: RefOrigin::Manual,
            cosmetic: false,
            variants: vec![Variant { angle_deg: 0.0, qty: 2, kit: None, split: None }, Variant { angle_deg: 90.0, qty: 2, kit: None, split: None }],
        },
        PlyShape {
            shape: shape("tri", tri, vec![]),
            ref_deg: 0.0,
            ref_origin: RefOrigin::DrawingX,
            cosmetic: false,
            variants: vec![Variant { angle_deg: 30.0, qty: 3, kit: None, split: None }],
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
    NestOptions { time: Duration::from_secs(4), seed: Some(7), workers: 2, early_stop: true, nest_tol: 0.05, live_layout: None }
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

#[test]
fn live_layouts_are_complete_and_verify() {
    let plies = plies();
    let mat = material(MaterialKind::Ud, false);
    let o = NestOptions { time: Duration::from_secs(3), early_stop: false, live_layout: Some(Duration::ZERO), ..opts() };
    let mut live = Vec::new();
    let layout = nest(&plies, &mat, &o, |p| live.extend(p.layout.clone()), Arc::new(AtomicBool::new(false))).expect("nest");
    assert!(!live.is_empty(), "no live layouts reported");
    for l in live.iter().chain([&layout]) {
        let r = verify(&plies, l);
        assert!(r.ok, "live layout failed verification: {:?}", r.issues);
    }
}

#[test]
fn stop_right_away_still_gives_a_complete_layout() {
    let plies = plies();
    let mat = material(MaterialKind::Ud, false);
    let cancel = Arc::new(AtomicBool::new(false));
    let c = cancel.clone();
    let stopper = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        c.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    let t = std::time::Instant::now();
    let layout = nest(&plies, &mat, &NestOptions { time: Duration::from_secs(60), early_stop: false, ..opts() }, |_| {}, cancel).expect("nest");
    stopper.join().unwrap();
    assert!(t.elapsed() < Duration::from_secs(10), "stop took {:?}", t.elapsed());
    let r = verify(&plies, &layout);
    assert!(r.ok, "{:?}", r.issues);
    assert_eq!(layout.placements.len(), 14);
}

/// The test plies with kits: one shape split over two kits, and some plies without a kit.
fn kitted() -> Vec<PlyShape> {
    let mut p = plies();
    let k = |s: &str| Some(s.to_string());
    p[0].variants[0].kit = k("B"); // rib 0°
    p[0].variants[1].kit = k("A"); // rib +45°
    p[0].variants[2].kit = k("A"); // rib -45°
    p[1].variants[0].kit = k("B"); // plate 0°
    // plate 90° and tri: no kit
    p
}

#[test]
fn kit_by_kit_cuts_each_kit_together() {
    let plies = kitted();
    let mat = material(MaterialKind::Ud, false);
    let mut live = Vec::new();
    let o = NestOptions { live_layout: Some(Duration::ZERO), ..opts() };
    let layout = nest_job(&plies, &mat, Grouping::ByKit, &o, |p| live.extend(p.layout.clone()), Arc::new(AtomicBool::new(false))).expect("nest");
    let r = verify(&plies, &layout);
    assert!(r.ok, "{:?}", r.issues);
    assert_eq!(layout.placements.len(), 14);

    // Sections in first-seen kit order, no kit last, end to end along the roll.
    let kits: Vec<Option<&str>> = layout.sections.iter().map(|s| s.kit.as_deref()).collect();
    assert_eq!(kits, [Some("B"), Some("A"), None]);
    assert_eq!(layout.sections[0].start, mat.lead_trim);
    for w in layout.sections.windows(2) {
        assert!((w[1].start - (w[0].end + mat.spacing)).abs() < 1e-9, "{w:?}");
    }
    assert_eq!(layout.used_length, layout.sections[2].end);

    // Each kit's cut numbers form one unbroken run, in section order.
    let cuts = cut_plies(&plies, &layout);
    let mut runs: Vec<Option<String>> = Vec::new();
    for c in &cuts {
        if runs.last() != Some(&c.kit) {
            runs.push(c.kit.clone());
        }
    }
    assert_eq!(runs, [Some("B".to_string()), Some("A".to_string()), None], "kits interleave in cut order");
    // Every ply sits inside its kit's section.
    for c in &cuts {
        let s = layout.sections.iter().find(|s| s.kit == c.kit).unwrap();
        assert!(c.x_mm >= s.start - 1e-6 && c.x_mm <= s.end, "{} at {} outside {s:?}", c.label, c.x_mm);
    }

    // Refitting an untouched layout changes nothing; a ply moved into the next
    // kit's stretch of roll fails verification.
    let mut refit = layout.clone();
    plyroll_core::edit::refit(&plies, &mut refit);
    for (a, b) in refit.sections.iter().zip(&layout.sections) {
        assert!((a.start - b.start).abs() < 1e-9 && (a.end - b.end).abs() < 1e-9, "{a:?} vs {b:?}");
    }
    assert!((refit.used_length - layout.used_length).abs() < 1e-9);
    let first_b = layout.placements.iter().position(|p| plies[p.shape_idx].variants[p.variant_idx].kit.as_deref() == Some("B")).unwrap();
    let mut moved = layout.clone();
    let mut locked = vec![];
    let dx = layout.sections[1].start + 5.0 - layout.sections[0].start;
    plyroll_core::edit::apply(&plies, &mut moved, &mut locked, &plyroll_core::edit::Edit::Move { indices: vec![first_b], dx, dy: 0.0 }).unwrap();
    let r = verify(&plies, &moved);
    assert!(!r.ok && r.bad.contains(&first_b), "{:?}", r.issues);
    assert!(r.issues.iter().filter(|i| i.contains("stretch")).count() == 1, "only the moved ply is outside its kit: {:?}", r.issues);
    assert_eq!(moved.sections[1].start, layout.sections[1].start, "kit starts stay put");
    assert!(r.issues.iter().any(|i| i.contains("outside its kit's stretch")), "{:?}", r.issues);

    // Live layouts grow kit by kit and are always valid placements of the full list.
    assert!(!live.is_empty());
    for l in &live {
        assert!(l.placements.iter().all(|p| p.shape_idx < plies.len() && p.variant_idx < plies[p.shape_idx].variants.len()));
    }
}

#[test]
fn kit_by_kit_without_kits_is_one_section() {
    let plies = plies();
    let mat = material(MaterialKind::Ud, false);
    let layout = nest_job(&plies, &mat, Grouping::ByKit, &opts(), |_| {}, Arc::new(AtomicBool::new(false))).expect("nest");
    assert!(verify(&plies, &layout).ok);
    assert_eq!(layout.placements.len(), 14);
    assert_eq!(layout.sections.len(), 1);
    assert_eq!(layout.sections[0].kit, None);
}

#[test]
fn stopping_kit_by_kit_still_gives_every_kit() {
    let plies = kitted();
    let mat = material(MaterialKind::Ud, false);
    let cancel = Arc::new(AtomicBool::new(false));
    let c = cancel.clone();
    let stopper = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        c.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    let t = std::time::Instant::now();
    let o = NestOptions { time: Duration::from_secs(60), early_stop: false, ..opts() };
    let layout = nest_job(&plies, &mat, Grouping::ByKit, &o, |_| {}, cancel).expect("nest");
    stopper.join().unwrap();
    assert!(t.elapsed() < Duration::from_secs(15), "stop took {:?}", t.elapsed());
    let r = verify(&plies, &layout);
    assert!(r.ok, "{:?}", r.issues);
    assert_eq!(layout.placements.len(), 14);
    assert_eq!(layout.sections.len(), 3);
}

#[test]
fn renest_after_locked_keeps_them_and_nests_the_rest_behind() {
    let plies = plies();
    let mat = material(MaterialKind::Ud, false);
    let layout = nest(&plies, &mat, &opts(), |_| {}, Arc::new(AtomicBool::new(false))).expect("nest");
    // Lock the three plies nearest the roll start.
    let mut order: Vec<usize> = (0..layout.placements.len()).collect();
    order.sort_by(|&a, &b| layout.placements[a].tx.total_cmp(&layout.placements[b].tx));
    let locked = vec![order[2], order[0], order[1]];
    let mut live = Vec::new();
    let o = NestOptions { live_layout: Some(Duration::ZERO), ..opts() };
    let out = nest_after_locked(&plies, &layout, &locked, &o, |p| live.extend(p.layout.clone()), Arc::new(AtomicBool::new(false))).expect("re-nest");

    let r = verify(&plies, &out);
    assert!(r.ok, "{:?}", r.issues);
    assert_eq!(out.placements.len(), layout.placements.len());
    // The locked plies are kept exactly and come first.
    for (k, &i) in locked.iter().enumerate() {
        assert_eq!(out.placements[k], layout.placements[i]);
    }
    // Everything else starts after the last locked ply plus the spacing.
    let outlines = plyroll_core::verify::placed_outlines(&plies, &out, plyroll_core::geom::FlattenMode::Enclosing);
    let kept_end = outlines[..3].iter().map(|o| o.bbox.max.x).fold(0.0, f64::max);
    for o in &outlines[3..] {
        assert!(o.bbox.min.x >= kept_end + mat.spacing - 0.05, "{} before {}", o.bbox.min.x, kept_end + mat.spacing);
    }
    // Copy numbers per variant are 1..qty, each once.
    for (si, p) in plies.iter().enumerate() {
        for (vi, v) in p.variants.iter().enumerate() {
            let mut c: Vec<u32> = out.placements.iter().filter(|x| x.shape_idx == si && x.variant_idx == vi).map(|x| x.copy).collect();
            c.sort_unstable();
            assert_eq!(c, (1..=v.qty).collect::<Vec<_>>(), "{} {:+}°", p.shape.name, v.angle_deg);
        }
    }
    // Live layouts include the locked plies.
    assert!(!live.is_empty() && live.iter().all(|l| l.placements.len() == out.placements.len() && l.placements[..3] == out.placements[..3]));

    // Refused: nothing locked, everything locked, kit by kit.
    let none = Arc::new(AtomicBool::new(false));
    assert!(nest_after_locked(&plies, &layout, &[], &opts(), |_| {}, none.clone()).is_err());
    let all: Vec<usize> = (0..layout.placements.len()).collect();
    let e = nest_after_locked(&plies, &layout, &all, &opts(), |_| {}, none.clone()).unwrap_err().to_string();
    assert!(e.contains("nothing left"), "{e}");
    let kitted_layout = nest_job(&kitted(), &mat, Grouping::ByKit, &opts(), |_| {}, none.clone()).unwrap();
    let e = nest_after_locked(&kitted(), &kitted_layout, &[0], &opts(), |_| {}, none).unwrap_err().to_string();
    assert!(e.contains("mixed layouts only"), "{e}");
}

#[test]
fn split_plies_nest_and_verify() {
    use plyroll_core::split::{self, Split};
    let m = material(MaterialKind::Ud, false);
    // 900 × 500 with a hole and rounded corners, at three angles; the roll is 384 mm usable.
    let mut big = PlyShape {
        shape: shape("panel", rounded_rect(900.0, 500.0, 30.0, 0.0, 0.0), vec![circle(450.0, 250.0, 40.0, false)]),
        ref_deg: 0.0,
        ref_origin: RefOrigin::DrawingX,
        cosmetic: false,
        variants: vec![
            Variant { angle_deg: 0.0, qty: 1, kit: None, split: None },
            Variant { angle_deg: 45.0, qty: 1, kit: None, split: None },
            Variant { angle_deg: 90.0, qty: 1, kit: None, split: Some(Split::Auto { overlap: 10.0, pieces: None }) },
        ],
    };
    assert_eq!(split::too_wide(std::slice::from_ref(&big), &m).len(), 3);
    assert_eq!(split::auto_split_plies(std::slice::from_mut(&mut big), &m), 2, "the 90° angle was already split");
    let (plies, _) = split::expand(&[big], &[1], &m).unwrap();
    assert!(plies.len() >= 6, "every angle is in pieces: {}", plies.len());
    assert!(split::too_wide(&plies, &m).is_empty());
    // The widest piece is right at the limit.
    let widest = plies.iter().map(|p| split::width_across(&p.shape.outer, p.fibre_deg(&p.variants[0]), &m, false)).fold(0.0, f64::max);
    assert!(widest > m.usable_width() - 20.0, "pieces use the roll width: {widest}");
    let layout = nest(&plies, &m, &opts(), |_| {}, Arc::new(AtomicBool::new(false))).unwrap();
    let r = verify(&plies, &layout);
    assert!(r.ok, "{:?}", r.issues);
    assert_eq!(layout.placements.len(), plies.len());
}

#[test]
fn ply_that_only_just_fits_still_nests() {
    // The demo rib at ±45° on a 230 mm usable roll: auto split leaves pieces
    // just under the roll width. The solver's simplified outline of one of them
    // was wider than the roll, so it could not be placed.
    use plyroll_core::split;
    let m = Material { name: "narrow".into(), kind: MaterialKind::Ud, roll_width: 250.0, edge_trim: 10.0, lead_trim: 20.0, spacing: 2.0, angle_tolerance: 0.0, tolerance_step: 0.5, warp_weft_interchangeable: false, cost_per_m2: None };
    let q = bulge_from_sweep(PI / 2.0);
    let v = |x: f64, y: f64, b: f64| Vtx { p: Pt::new(x, y), bulge: b };
    let outer = Path::new(vec![v(520.0, 200.0, 0.0), v(800.0, 200.0, q), v(820.0, 220.0, 0.0), v(820.0, 290.0, q), v(800.0, 310.0, 0.0), v(520.0, 310.0, q), v(500.0, 290.0, 0.0), v(500.0, 220.0, q)], true);
    let mut rib = PlyShape {
        shape: shape("rib", outer, vec![]),
        ref_deg: 0.0,
        ref_origin: RefOrigin::DrawingX,
        cosmetic: false,
        variants: vec![Variant { angle_deg: 45.0, qty: 2, kit: None, split: None }, Variant { angle_deg: -45.0, qty: 2, kit: None, split: None }],
    };
    assert_eq!(split::auto_split_plies(std::slice::from_mut(&mut rib), &m), 2);
    let (plies, _) = split::expand(&[rib], &[1], &m).unwrap();
    let widest = plies.iter().map(|p| split::width_across(&p.shape.outer, p.fibre_deg(&p.variants[0]), &m, false)).fold(0.0, f64::max);
    assert!(widest > m.usable_width() - 2.0 && widest <= m.usable_width(), "{widest}");
    let layout = nest(&plies, &m, &opts(), |_| {}, Arc::new(AtomicBool::new(false))).expect("nest");
    let r = verify(&plies, &layout);
    assert!(r.ok, "{:?}", r.issues);
    assert_eq!(layout.placements.len(), 8);
}

/// Placements whose centre lies inside a hole of another placement.
fn in_holes(plies: &[PlyShape], layout: &plyroll_core::model::Layout) -> Vec<(usize, usize)> {
    let mut out = vec![];
    for (h, host) in layout.placements.iter().enumerate() {
        for hole in &plies[host.shape_idx].shape.holes {
            let poly = hole.transformed(&host.transform(), 0.05).flatten(0.05, plyroll_core::geom::FlattenMode::Inscribed);
            for (i, pl) in layout.placements.iter().enumerate() {
                let b = plyroll_core::geom::BBox::of(&plies[pl.shape_idx].shape.outer.transformed(&pl.transform(), 0.05).flatten(0.05, plyroll_core::geom::FlattenMode::Inscribed));
                let c = Pt::new((b.min.x + b.max.x) / 2.0, (b.min.y + b.max.y) / 2.0);
                if i != h && plyroll_core::geom::point_in_polygon(c, &poly) {
                    out.push((h, i));
                }
            }
        }
    }
    out
}

#[test]
fn plies_go_inside_holes() {
    let m = material(MaterialKind::Ud, false);
    // A ring with a 300 mm hole, and plies that fit in it.
    let plies = vec![
        PlyShape {
            shape: shape("ring", circle(0.0, 0.0, 190.0, true), vec![circle(0.0, 0.0, 150.0, false)]),
            ref_deg: 0.0,
            ref_origin: RefOrigin::DrawingX,
            cosmetic: false,
            variants: vec![Variant { angle_deg: 0.0, qty: 2, kit: None, split: None }, Variant { angle_deg: 45.0, qty: 1, kit: None, split: None }],
        },
        PlyShape {
            shape: shape("tab", rounded_rect(120.0, 60.0, 8.0, 0.0, 0.0), vec![]),
            ref_deg: 0.0,
            ref_origin: RefOrigin::DrawingX,
            cosmetic: false,
            variants: vec![Variant { angle_deg: 0.0, qty: 6, kit: None, split: None }, Variant { angle_deg: 90.0, qty: 4, kit: None, split: None }],
        },
        PlyShape {
            shape: shape("disc", circle(0.0, 0.0, 30.0, true), vec![]),
            ref_deg: 0.0,
            ref_origin: RefOrigin::DrawingX,
            cosmetic: false,
            variants: vec![Variant { angle_deg: 0.0, qty: 8, kit: None, split: None }],
        },
    ];
    let groups = build_groups(&plies, &m).unwrap();
    let units = plyroll_nest::build_units(&plies, &m, &groups);
    // Every copy is in exactly one unit.
    let mut count = vec![0u32; groups.len()];
    for u in &units {
        count[u.group] += u.demand;
        for i in &u.inner {
            count[i.group] += u.demand;
        }
    }
    assert_eq!(count, groups.iter().map(|g| g.demand()).collect::<Vec<_>>());
    assert!(units.iter().any(|u| u.inner.len() >= 2), "{units:#?}");
    let layout = nest(&plies, &m, &opts(), |_| {}, Arc::new(AtomicBool::new(false))).expect("nest");
    let r = verify(&plies, &layout);
    assert!(r.ok, "{:?}", r.issues);
    assert_eq!(layout.placements.len(), 21);
    let inside = in_holes(&plies, &layout);
    assert!(inside.len() >= 6, "{inside:?}");
    assert!(inside.iter().all(|&(h, _)| plies[layout.placements[h].shape_idx].shape.name == "ring"));
}

#[test]
fn a_cosmetic_ply_inside_a_twill_ply_keeps_its_weave() {
    // Warp and weft may swap on the frame, not on the cosmetic patch inside it.
    let m = material(MaterialKind::Twill, true);
    let frame = Path::from_points(&[Pt::new(0.0, 0.0), Pt::new(300.0, 0.0), Pt::new(300.0, 300.0), Pt::new(0.0, 300.0)], true);
    let hole = Path::from_points(&[Pt::new(40.0, 40.0), Pt::new(40.0, 260.0), Pt::new(260.0, 260.0), Pt::new(260.0, 40.0)], true);
    let plies = vec![
        PlyShape {
            shape: shape("frame", frame, vec![hole]),
            ref_deg: 0.0,
            ref_origin: RefOrigin::DrawingX,
            cosmetic: false,
            variants: vec![Variant { angle_deg: 0.0, qty: 2, kit: None, split: None }],
        },
        PlyShape {
            shape: shape("patch", rounded_rect(150.0, 90.0, 10.0, 0.0, 0.0), vec![]),
            ref_deg: 0.0,
            ref_origin: RefOrigin::DrawingX,
            cosmetic: true,
            variants: vec![Variant { angle_deg: 0.0, qty: 2, kit: None, split: None }],
        },
    ];
    let groups = build_groups(&plies, &m).unwrap();
    assert_eq!(groups[0].rotations.len(), 4);
    let units = plyroll_nest::build_units(&plies, &m, &groups);
    let host = units.iter().find(|u| !u.inner.is_empty()).expect("the patch goes in the frame");
    assert_eq!(host.rotations.len(), 2, "only the turns that keep the patch's warp");
    let layout = nest(&plies, &m, &opts(), |_| {}, Arc::new(AtomicBool::new(false))).expect("nest");
    let r = verify(&plies, &layout);
    assert!(r.ok, "{:?}", r.issues);
    for pl in &layout.placements {
        let ply = &plies[pl.shape_idx];
        assert!(fibre_ok(ply.fibre_deg(&ply.variants[pl.variant_idx]), pl.rotation_deg, &m, ply.cosmetic));
    }
    assert_eq!(in_holes(&plies, &layout).len(), 2);
}
