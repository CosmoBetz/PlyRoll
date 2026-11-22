//! Adapter between PlyRoll's ply model and the sparrow strip-packing solver.
//!
//! All solver-specific code lives here so the solver can be upgraded or
//! replaced without touching the rest of PlyRoll.
//!
//! Coordinate notes:
//! - The roll runs along +x; the strip's fixed height is the usable roll width.
//! - jagua-rs keeps items `min_item_separation` away from the strip boundary
//!   (items inflated and the strip deflated by half the separation each).
//!   Parts may touch the trim lines in PlyRoll, so the strip is made
//!   `2 * spacing` taller and the result shifted back by `spacing`.
//! - Shapes are centred before they are handed to the solver (f32 inside),
//!   and solver rotations are snapped back to the exact allowed angle.

use anyhow::{Result, anyhow, bail, ensure};
use jagua_rs::io::ext_repr::{ExtItem as BaseItem, ExtOrientation, ExtRotation, ExtSPolygon, ExtShape};
use jagua_rs::io::import::Importer;
use jagua_rs::probs::spp::entities::SPSolution;
use jagua_rs::probs::spp::io::ext_repr::{ExtItem, ExtSPInstance};
use rand::SeedableRng;
use rand::rngs::Xoshiro256PlusPlus;
use sparrow::config::{DEFAULT_SPARROW_CONFIG, ShrinkDecayStrategy};
use sparrow::consts::{DEFAULT_FAIL_DECAY_RATIO_CMPR, DEFAULT_MAX_CONSEQ_FAILS_EXPL};
use sparrow::util::listener::{ReportType, SolutionListener};
use sparrow::util::terminator::Terminator;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use plyroll_core::geom::{Affine, BBox, FlattenMode, Pt, norm_deg};
use plyroll_core::kits::{self, Grouping, LayoutSection, Section};
use plyroll_core::model::{Layout, Material, Placement, PlyShape};
use plyroll_core::orient::{allowed_rotations, rotation_key};
use plyroll_core::verify::placed_outlines;

pub use plyroll_core;

#[derive(Clone, Debug)]
pub struct NestOptions {
    /// Total optimisation time (80% explore, 20% compress).
    pub time: Duration,
    pub seed: Option<u64>,
    /// Worker threads for the solver.
    pub workers: usize,
    /// Stop early when the search stalls.
    pub early_stop: bool,
    /// Chord tolerance used to turn arcs into the (enclosing) nesting polygon, mm.
    pub nest_tol: f64,
    /// Attach the current layout to progress reports, at most this often.
    pub live_layout: Option<Duration>,
}

impl Default for NestOptions {
    fn default() -> Self {
        Self {
            time: Duration::from_secs(60),
            seed: None,
            workers: std::thread::available_parallelism().map_or(4, |n| n.get()),
            early_stop: false,
            nest_tol: 0.05,
            live_layout: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Progress {
    pub phase: &'static str,
    /// Roll length of the best feasible layout so far, mm (approximate).
    pub length: f64,
    pub density: f64,
    pub elapsed: Duration,
    /// Best feasible layout so far (only with `NestOptions::live_layout`).
    /// When nesting kit by kit: the finished kits plus the current one.
    pub layout: Option<Layout>,
    /// When nesting kit by kit: which kit is being nested.
    pub section: Option<SectionProgress>,
}

#[derive(Clone, Debug)]
pub struct SectionProgress {
    /// 0-based.
    pub index: usize,
    pub count: usize,
    pub kit: Option<String>,
}

/// One solver item: a shape at one set of allowed rotations, possibly
/// shared by several angle variants with identical rotation sets.
#[derive(Debug)]
pub struct ItemGroup {
    pub shape_idx: usize,
    pub rotations: Vec<f64>,
    /// (variant index, quantity)
    pub variants: Vec<(usize, u32)>,
    /// Centre subtracted from the drawing before nesting.
    pub center: Pt,
}

impl ItemGroup {
    pub fn demand(&self) -> u32 {
        self.variants.iter().map(|v| v.1).sum()
    }
}

/// Groups variants into solver items. Variants of one shape merge only when
/// their allowed rotation sets are identical (e.g. ±45° on interchangeable twill).
pub fn build_groups(plies: &[PlyShape], material: &Material) -> Result<Vec<ItemGroup>> {
    let usable = material.usable_width();
    let mut groups = Vec::new();
    for (si, ply) in plies.iter().enumerate() {
        ensure!(!ply.variants.is_empty(), "{}: no angle variants", ply.shape.name);
        let pts = ply.shape.outer.flatten(0.2, FlattenMode::Inscribed);
        let b = BBox::of(&pts);
        let center = Pt::new((b.min.x + b.max.x) / 2.0, (b.min.y + b.max.y) / 2.0);
        let mut by_key: BTreeMap<Vec<i64>, ItemGroup> = BTreeMap::new();
        for (vi, v) in ply.variants.iter().enumerate() {
            if v.qty == 0 {
                continue;
            }
            let rots = allowed_rotations(ply.fibre_deg(v), material, ply.cosmetic);
            // Reject early if no allowed rotation fits across the roll.
            let fits = rots.iter().any(|&r| {
                let t = Affine::rot_trans(r, 0.0, 0.0);
                let rb = BBox::of(&pts.iter().map(|&p| t.apply(p)).collect::<Vec<_>>());
                rb.height() <= usable + 1e-6
            });
            if !fits {
                bail!(
                    "{} at {:+}° does not fit across the roll: usable width is {:.1} mm (roll {:.1} mm minus edge trims)",
                    ply.shape.name,
                    v.angle_deg,
                    usable,
                    material.roll_width
                );
            }
            by_key
                .entry(rotation_key(&rots))
                .or_insert_with(|| ItemGroup { shape_idx: si, rotations: rots.clone(), variants: vec![], center })
                .variants
                .push((vi, v.qty));
        }
        groups.extend(by_key.into_values());
    }
    ensure!(!groups.is_empty(), "nothing to nest (all quantities are zero)");
    Ok(groups)
}

struct Listener<'a, F: FnMut(&Progress)> {
    cb: &'a mut F,
    plies: &'a [PlyShape],
    material: &'a Material,
    groups: &'a [ItemGroup],
    live: Option<Duration>,
    last_live: Option<std::time::Instant>,
    start: std::time::Instant,
}

impl<F: FnMut(&Progress)> SolutionListener for Listener<'_, F> {
    fn report(&mut self, report: ReportType, solution: &SPSolution) {
        let phase = match report {
            ReportType::ExplFeas => "explore",
            ReportType::CmprFeas => "compress",
            ReportType::Final => "final",
            _ => return,
        };
        let length = solution.strip().width as f64 - 2.0 * self.material.spacing + self.material.lead_trim;
        let now = std::time::Instant::now();
        let layout = match self.live {
            Some(every) if phase != "final" && self.last_live.is_none_or(|t| now - t >= every) => {
                self.last_live = Some(now);
                map_solution(self.plies, self.material, self.groups, solution).ok()
            }
            _ => None,
        };
        (self.cb)(&Progress { phase, length, density: solution.density() as f64, elapsed: self.start.elapsed(), layout, section: None });
    }
}

struct CancelTerminator {
    timeout: Option<jagua_rs::Instant>,
    cancel: Arc<AtomicBool>,
}

impl Terminator for CancelTerminator {
    fn kill(&self) -> bool {
        self.cancel.load(Ordering::Relaxed) || self.timeout.is_some_and(|t| jagua_rs::Instant::now() > t)
    }
    fn new_timeout(&mut self, timeout: Duration) {
        self.timeout = Some(jagua_rs::Instant::now() + timeout);
    }
    fn timeout_at(&self) -> Option<jagua_rs::Instant> {
        self.timeout
    }
}

/// Builds the solver instance (also useful for debugging: serialise it to JSON
/// and run the sparrow CLI on it).
pub fn build_instance(plies: &[PlyShape], material: &Material, groups: &[ItemGroup], nest_tol: f64) -> ExtSPInstance {
    let s = material.spacing;
    let items = groups
        .iter()
        .enumerate()
        .map(|(id, g)| {
            let outer = &plies[g.shape_idx].shape.outer;
            let poly: Vec<(f32, f32)> =
                outer.flatten(nest_tol, FlattenMode::Enclosing).iter().map(|p| ((p.x - g.center.x) as f32, (p.y - g.center.y) as f32)).collect();
            ExtItem {
                base: BaseItem {
                    id: id as u64,
                    orientation: ExtOrientation {
                        rotation: ExtRotation::Discrete { angles: g.rotations.iter().map(|&r| r as f32).collect() },
                        reflection_axes: vec![], // flipping is never allowed
                    },
                    shape: ExtShape::SimplePolygon(ExtSPolygon(poly)),
                    min_quality: None,
                },
                demand: g.demand() as u64,
            }
        })
        .collect();
    ExtSPInstance { name: "plyroll".into(), min_item_separation: s as f32, items, strip_height: (material.usable_width() + 2.0 * s) as f32 }
}

/// Nests all plies on one roll. Returns a layout in roll coordinates.
pub fn nest(plies: &[PlyShape], material: &Material, opts: &NestOptions, mut on_progress: impl FnMut(&Progress), cancel: Arc<AtomicBool>) -> Result<Layout> {
    plyroll_core::job::validate_material(material)?;
    let groups = build_groups(plies, material)?;
    let ext = build_instance(plies, material, &groups, opts.nest_tol);

    let mut config = DEFAULT_SPARROW_CONFIG;
    config.expl_cfg.time_limit = opts.time.mul_f32(0.8);
    config.cmpr_cfg.time_limit = opts.time.mul_f32(0.2);
    config.expl_cfg.separator_config.n_workers = opts.workers.max(1);
    config.cmpr_cfg.separator_config.n_workers = opts.workers.max(1);
    if opts.early_stop {
        config.expl_cfg.max_conseq_failed_attempts = Some(DEFAULT_MAX_CONSEQ_FAILS_EXPL);
        config.cmpr_cfg.shrink_decay = ShrinkDecayStrategy::FailureBased(DEFAULT_FAIL_DECAY_RATIO_CMPR);
    }
    let importer = Importer::new(config.cde_config, config.poly_simpl_tolerance, config.narrow_concavity_cutoff_ratio);
    let instance = jagua_rs::probs::spp::io::import_instance(&importer, &ext)?;
    let seed = opts.seed.unwrap_or_else(rand::random);
    let rng = Xoshiro256PlusPlus::seed_from_u64(seed);
    let mut listener = Listener {
        cb: &mut on_progress,
        plies,
        material,
        groups: &groups,
        live: opts.live_layout,
        last_live: None,
        start: std::time::Instant::now(),
    };
    let mut term = CancelTerminator { timeout: None, cancel };
    let solution = sparrow::optimizer::optimize(instance, rng, &mut listener, &mut term, &config.expl_cfg, &config.cmpr_cfg, None)?;
    map_solution(plies, material, &groups, &solution)
}

/// Nests a job the way its grouping asks: all plies mixed, or kit by kit with
/// each kit on its own section of roll (see `plyroll_core::kits`).
///
/// Kit by kit, the time is shared by ply area (at least 1 s per kit) and the
/// kits are nested one after another. On stop, the current kit keeps its best
/// layout and the remaining kits get a quick first layout, so the result is
/// always complete.
pub fn nest_job(
    plies: &[PlyShape],
    material: &Material,
    grouping: Grouping,
    opts: &NestOptions,
    mut on_progress: impl FnMut(&Progress),
    cancel: Arc<AtomicBool>,
) -> Result<Layout> {
    let sections = match grouping {
        Grouping::Global => return nest(plies, material, opts, on_progress, cancel),
        Grouping::ByKit => kits::sections(plies),
    };
    if sections.len() <= 1 {
        let mut layout = nest(plies, material, opts, on_progress, cancel)?;
        if let Some(sec) = sections.first() {
            layout.sections = vec![LayoutSection { kit: sec.kit.clone(), start: material.lead_trim, end: layout.used_length }];
        }
        return Ok(layout);
    }
    plyroll_core::job::validate_material(material)?;
    // Fail before any solving if a ply does not fit.
    for sec in &sections {
        build_groups(&sec.plies, material)?;
    }
    let total_area: f64 = sections.iter().map(Section::area).sum::<f64>().max(1e-9);
    let start_time = std::time::Instant::now();
    let seed = opts.seed.unwrap_or_else(rand::random);
    let count = sections.len();
    let mut done: Vec<Layout> = Vec::new();
    let mut start = material.lead_trim;
    for (k, sec) in sections.iter().enumerate() {
        let mat = kits::section_material(material, start);
        let share = opts.time.mul_f64(sec.area() / total_area).max(Duration::from_secs(1));
        let sec_opts = NestOptions { time: share, seed: Some(seed.wrapping_add(k as u64)), ..opts.clone() };
        let info = SectionProgress { index: k, count, kit: sec.kit.clone() };
        let finished: Vec<(&Section, &Layout)> = sections.iter().zip(done.iter()).collect();
        let layout = nest(
            &sec.plies,
            &mat,
            &sec_opts,
            |p| {
                let layout = p.layout.as_ref().and_then(|cur| {
                    let mut parts = finished.clone();
                    parts.push((sec, cur));
                    kits::merge(material, &parts).ok()
                });
                on_progress(&Progress { elapsed: start_time.elapsed(), layout, section: Some(info.clone()), ..p.clone() });
            },
            cancel.clone(),
        )?;
        start = kits::next_start(material, layout.used_length.max(start));
        done.push(layout);
    }
    let parts: Vec<(&Section, &Layout)> = sections.iter().zip(done.iter()).collect();
    kits::merge(material, &parts)
}

/// Keeps the `locked` placements of a finished layout where they are and nests
/// every other ply on the roll after them: from the far end of the last locked
/// ply plus the spacing. Gaps between locked plies are not filled (the solver
/// cannot work around fixed parts). Mixed layouts only.
///
/// The kept plies come first in the new layout, so they are placements
/// `0..locked.len()`; they keep their copy numbers and the new plies get the
/// remaining ones in roll order.
pub fn nest_after_locked(
    plies: &[PlyShape],
    layout: &Layout,
    locked: &[usize],
    opts: &NestOptions,
    mut on_progress: impl FnMut(&Progress),
    cancel: Arc<AtomicBool>,
) -> Result<Layout> {
    let material = &layout.material;
    ensure!(!locked.is_empty(), "no ply is locked");
    ensure!(
        layout.sections.is_empty(),
        "re-nesting after locked plies works on mixed layouts only, and this nest is kit by kit; nest everything again instead"
    );
    for (k, &i) in locked.iter().enumerate() {
        ensure!(i < layout.placements.len() && !locked[..k].contains(&i), "locked ply {i} is not in this nest");
    }
    let kept: Vec<Placement> = locked.iter().map(|&i| layout.placements[i].clone()).collect();
    let kept_layout = Layout { material: material.clone(), placements: kept.clone(), used_length: 0.0, sections: vec![] };
    let kept_end = placed_outlines(plies, &kept_layout, FlattenMode::Enclosing).iter().map(|o| o.bbox.max.x).fold(material.lead_trim, f64::max);
    let start = kits::next_start(material, kept_end);

    // What is left to cut: the ordered quantities less the kept copies.
    let mut rest = plies.to_vec();
    for (si, p) in rest.iter_mut().enumerate() {
        for (vi, v) in p.variants.iter_mut().enumerate() {
            let n = kept.iter().filter(|k| k.shape_idx == si && k.variant_idx == vi).count() as u32;
            ensure!(n <= v.qty, "{} {:+}°: {n} copies locked, but only {} ordered", p.shape.name, v.angle_deg, v.qty);
            v.qty -= n;
        }
    }
    ensure!(rest.iter().any(|p| p.variants.iter().any(|v| v.qty > 0)), "every ply is locked, so there is nothing left to nest");

    let merge = |new: &Layout| -> Layout {
        let mut placements = kept.clone();
        // New copies take the numbers the kept ones do not use, in roll order.
        let mut fresh: Vec<Placement> = new.placements.clone();
        fresh.sort_by(|a, b| (a.shape_idx, a.variant_idx).cmp(&(b.shape_idx, b.variant_idx)).then(a.copy.cmp(&b.copy)));
        let mut last: Option<(usize, usize)> = None;
        let mut free: Vec<u32> = Vec::new();
        for mut pl in fresh {
            if last != Some((pl.shape_idx, pl.variant_idx)) {
                last = Some((pl.shape_idx, pl.variant_idx));
                let used: Vec<u32> = kept.iter().filter(|k| (k.shape_idx, k.variant_idx) == (pl.shape_idx, pl.variant_idx)).map(|k| k.copy).collect();
                let total = plies[pl.shape_idx].variants[pl.variant_idx].qty;
                free = (1..=total).filter(|c| !used.contains(c)).rev().collect();
            }
            pl.copy = free.pop().unwrap_or(pl.copy);
            placements.push(pl);
        }
        Layout { material: material.clone(), placements, used_length: new.used_length.max(kept_end), sections: vec![] }
    };

    let mat = kits::section_material(material, start);
    let new = nest(
        &rest,
        &mat,
        opts,
        |p| {
            let layout = p.layout.as_ref().map(&merge);
            on_progress(&Progress { layout, length: p.length.max(kept_end), ..p.clone() });
        },
        cancel,
    )?;
    Ok(merge(&new))
}

/// Maps a solver solution back to ply placements in roll coordinates.
fn map_solution(plies: &[PlyShape], material: &Material, groups: &[ItemGroup], solution: &SPSolution) -> Result<Layout> {
    let out = jagua_rs::probs::spp::io::export(solution, *sparrow::EPOCH);
    let s = material.spacing;
    let (dx, dy) = (material.lead_trim - s, material.edge_trim - s);
    let mut per_group: Vec<Vec<(f64, f64, f64)>> = vec![vec![]; groups.len()];
    for pi in &out.layout.placed_items {
        let g = groups.get(pi.item_id as usize).ok_or_else(|| anyhow!("solver returned unknown item {}", pi.item_id))?;
        ensure!(!pi.transformation.reflected, "solver returned a mirrored placement; refusing (flipping is not allowed)");
        let raw = pi.transformation.rotation as f64;
        let (tx, ty) = (pi.transformation.translation.0 as f64, pi.transformation.translation.1 as f64);
        // Snap to the exact allowed angle, keeping the part's centre where the solver put it.
        let snapped = g
            .rotations
            .iter()
            .copied()
            .min_by(|a, b| ang_dist(*a, raw).total_cmp(&ang_dist(*b, raw)))
            .filter(|r| ang_dist(*r, raw) < 0.05)
            .ok_or_else(|| anyhow!("solver rotation {raw:.4}° is not one of the allowed angles"))?;
        // Solver map: p' = R(raw)(p - c) + t. Centre c maps to t. Keep that, use R(snapped).
        let rc = Affine::rot_trans(snapped, 0.0, 0.0).apply(g.center);
        per_group[pi.item_id as usize].push((snapped, tx - rc.x + dx, ty - rc.y + dy));
    }
    let mut placements = Vec::new();
    for (gi, g) in groups.iter().enumerate() {
        let mut list = std::mem::take(&mut per_group[gi]);
        ensure!(list.len() as u32 == g.demand(), "{}: solver placed {} of {} copies", plies[g.shape_idx].shape.name, list.len(), g.demand());
        // Fill variants in roll order so each variant's copies sit together.
        list.sort_by(|a, b| a.1.total_cmp(&b.1));
        let mut it = list.into_iter();
        for &(vi, qty) in &g.variants {
            for copy in 1..=qty {
                let (r, tx, ty) = it.next().expect("count checked above");
                placements.push(Placement { shape_idx: g.shape_idx, variant_idx: vi, copy, rotation_deg: norm_deg(r), tx, ty });
            }
        }
    }
    let mut layout = Layout { material: material.clone(), placements, used_length: 0.0, sections: vec![] };
    let max_x = placed_outlines(plies, &layout, FlattenMode::Enclosing).iter().map(|o| o.bbox.max.x).fold(material.lead_trim, f64::max);
    layout.used_length = max_x;
    Ok(layout)
}

fn ang_dist(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d)
}
