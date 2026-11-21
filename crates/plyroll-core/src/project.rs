//! A PlyRoll project (`*.plyroll`, JSON): imported ply shapes with their
//! 0° reference, angle variants and flags.
//!
//! The geometry is cached in the project, so it opens even when the DXFs have
//! moved. Re-importing a DXF keeps each part's 0° reference (matched by file
//! and part name) and flags it for a re-check if the outline changed.
//! Source paths are stored relative to the project file.
//!
//! The project also holds the job settings (material, time, sets) and the
//! last nesting result together with the exact plies it was solved from, so
//! a result can be checked and exported even after the plies were edited
//! (it is then marked as out of date).

use crate::geom::{Affine, BBox, FlattenMode, Path, Pt, signed_area};
use crate::import::{ImportOptions, UnitsOverride, import_dxf};
use crate::job::{RefSpec, make_names_unique, resolve_ref};
use crate::job::validate_material;
use crate::model::{Layout, Material, PlyShape, RefOrigin, Shape, Variant};
use crate::report::{RunInfo, Summary};
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Component, Path as FsPath, PathBuf};

pub const PROJECT_VERSION: u32 = 1;
pub const EXTENSION: &str = "plyroll";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ImportSettings {
    pub units: UnitsOverride,
    pub join_tol: f64,
    pub gap_tol: f64,
    pub curve_tol: f64,
}

impl Default for ImportSettings {
    fn default() -> Self {
        let d = ImportOptions::default();
        Self { units: d.units, join_tol: d.join_tol, gap_tol: d.gap_tol, curve_tol: d.curve_tol }
    }
}

impl ImportSettings {
    pub fn options(&self) -> ImportOptions {
        ImportOptions { units: self.units, join_tol: self.join_tol, gap_tol: self.gap_tol, curve_tol: self.curve_tol, ..ImportOptions::default() }
    }
}

/// 0° reference axis of a ply shape, degrees from drawing X (CCW), kept in (−90, 90].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reference {
    pub deg: f64,
    pub origin: RefOrigin,
}

/// Translation-invariant fingerprint of an imported outline. Deliberately not
/// rotation-invariant: the 0° reference is stored in drawing coordinates, so a
/// part that was rotated in CAD must be re-checked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Signature {
    pub area: f64,
    pub perimeter: f64,
    pub width: f64,
    pub height: f64,
    /// Centroid relative to the bounding box minimum.
    pub cx: f64,
    pub cy: f64,
    pub holes: usize,
}

impl Signature {
    pub fn of(shape: &Shape) -> Self {
        let pts = shape.outer.flatten(0.01, FlattenMode::Inscribed);
        let bb = BBox::of(&pts);
        let n = pts.len();
        let (mut a2, mut cx, mut cy, mut per) = (0.0, 0.0, 0.0, 0.0);
        for i in 0..n {
            let (p, q) = (pts[i].sub(bb.min), pts[(i + 1) % n].sub(bb.min));
            let cr = p.x * q.y - q.x * p.y;
            a2 += cr;
            cx += (p.x + q.x) * cr;
            cy += (p.y + q.y) * cr;
            per += p.dist(q);
        }
        let (cx, cy) = if a2.abs() > 1e-12 { (cx / (3.0 * a2), cy / (3.0 * a2)) } else { (0.0, 0.0) };
        Signature { area: shape.area(0.01), perimeter: per, width: bb.width(), height: bb.height(), cx, cy, holes: shape.holes.len() }
    }

    pub fn matches(&self, o: &Signature) -> bool {
        let close = |a: f64, b: f64| (a - b).abs() <= 0.01 + 1e-5 * a.abs().max(b.abs());
        self.holes == o.holes
            && close(self.area.sqrt(), o.area.sqrt())
            && close(self.perimeter, o.perimeter)
            && close(self.width, o.width)
            && close(self.height, o.height)
            && close(self.cx, o.cx)
            && close(self.cy, o.cy)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectPly {
    /// Stable id inside the project (UI selection, commands).
    pub id: u64,
    /// Part name assigned by the importer (`stem` or `stem#n`); with
    /// `shape.source_file` it identifies the part on re-import.
    pub part: String,
    /// Geometry; already mirrored when `shape.mirrored_on_import` is set.
    /// `shape.name` is the display name.
    pub shape: Shape,
    pub reference: Option<Reference>,
    /// The user has set or accepted the 0° reference.
    pub confirmed: bool,
    /// Why the reference needs a look (outline changed, mirrored, ...).
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub cosmetic: bool,
    #[serde(default)]
    pub variants: Vec<Variant>,
    /// Fingerprint of the outline as imported (unmirrored).
    pub signature: Signature,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlyStatus {
    /// No 0° reference yet.
    Unset,
    /// A reference exists but must be looked at.
    Check,
    Ok,
}

impl ProjectPly {
    pub fn status(&self) -> PlyStatus {
        match (self.reference, self.confirmed) {
            (None, _) => PlyStatus::Unset,
            (Some(_), false) => PlyStatus::Check,
            (Some(_), true) => PlyStatus::Ok,
        }
    }

    pub fn total_qty(&self) -> u32 {
        self.variants.iter().map(|v| v.qty).sum()
    }

    fn set_reference(&mut self, deg: f64, origin: RefOrigin) {
        self.reference = Some(Reference { deg: norm_axis(deg), origin });
        self.confirmed = true;
        self.note = None;
    }
}

/// How the user picks a 0° reference.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RefPick {
    /// Typed angle, degrees from drawing X.
    Angle { deg: f64 },
    DrawingX,
    DrawingY,
    LongestEdge,
    BoundingBox,
    /// The line on a FIBRE / 0DEG layer.
    Layer,
    /// A straight edge: `contour` 0 = outline, k = hole k−1; `seg` = segment index.
    Edge { contour: usize, seg: usize },
    /// Direction from point `a` to point `b`, drawing coordinates.
    Points { a: Pt, b: Pt },
    /// Turn the current reference by `delta` degrees.
    Rotate { delta: f64 },
    /// Accept the current reference as it is.
    Confirm,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ImportReport {
    pub added: usize,
    pub unchanged: usize,
    /// Outline changed; reference kept but flagged.
    pub changed: usize,
    pub warnings: Vec<String>,
}

/// Nesting settings stored with the project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct JobSettings {
    /// Copy of the chosen library material, so the project nests the same
    /// way later even if the library changes.
    pub material: Option<Material>,
    /// Optimisation time, seconds.
    pub time_s: u64,
    /// Complete layups to cut: every quantity is multiplied by this.
    pub sets: u32,
}

impl Default for JobSettings {
    fn default() -> Self {
        Self { material: None, time_s: 60, sets: 1 }
    }
}

pub const MAX_TIME_S: u64 = 3600;
pub const MAX_SETS: u32 = 1000;

/// Exactly what the solver is given.
#[derive(Clone, Debug)]
pub struct NestInput {
    /// Plies with quantities already multiplied by the number of sets.
    pub plies: Vec<PlyShape>,
    /// Project ply id of each entry in `plies`.
    pub ply_ids: Vec<u64>,
    pub material: Material,
    pub warnings: Vec<String>,
    pub fingerprint: String,
}

/// A finished nest with the plies it was solved from. Placements index into
/// `plies`, never into the live project, so later edits cannot scramble them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NestResult {
    pub fingerprint: String,
    pub plies: Vec<PlyShape>,
    pub ply_ids: Vec<u64>,
    pub layout: Layout,
    pub summary: Summary,
    pub run: RunInfo,
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl NestResult {
    pub fn new(input: NestInput, layout: Layout, run: RunInfo) -> Self {
        let summary = crate::report::summarize(&input.plies, &layout);
        NestResult { fingerprint: input.fingerprint, plies: input.plies, ply_ids: input.ply_ids, layout, summary, run, warnings: input.warnings }
    }
}

/// Whether the stored result still matches the project.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Freshness {
    pub fresh: bool,
    /// Why it is out of date.
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub import: ImportSettings,
    #[serde(default)]
    pub next_id: u64,
    pub plies: Vec<ProjectPly>,
    #[serde(default)]
    pub job: JobSettings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<NestResult>,
}

impl Project {
    pub fn new(name: impl Into<String>) -> Self {
        Project {
            version: PROJECT_VERSION,
            name: name.into(),
            import: ImportSettings::default(),
            next_id: 1,
            plies: Vec::new(),
            job: JobSettings::default(),
            result: None,
        }
    }

    pub fn load(path: &FsPath) -> Result<Project> {
        let text = std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
        let mut p: Project = serde_json::from_str(&text).with_context(|| format!("{} is not a PlyRoll project", path.display()))?;
        if p.version > PROJECT_VERSION {
            bail!("{} was written by a newer PlyRoll (project version {})", path.display(), p.version);
        }
        let base = abs_parent(path);
        for ply in &mut p.plies {
            ply.shape.source_file = canon(&base.join(&ply.shape.source_file));
            p.next_id = p.next_id.max(ply.id + 1);
        }
        if let Some(r) = &mut p.result {
            for ply in &mut r.plies {
                ply.shape.source_file = canon(&base.join(&ply.shape.source_file));
            }
        }
        Ok(p)
    }

    /// Writes the project; source paths become relative to the project file.
    pub fn save(&self, path: &FsPath) -> Result<()> {
        let base = abs_parent(path);
        let mut out = self.clone();
        out.version = PROJECT_VERSION;
        for ply in &mut out.plies {
            ply.shape.source_file = relative_to(&ply.shape.source_file, &base);
        }
        if let Some(r) = &mut out.result {
            for ply in &mut r.plies {
                ply.shape.source_file = relative_to(&ply.shape.source_file, &base);
            }
        }
        let text = serde_json::to_string_pretty(&out)?;
        let tmp = path.with_extension("plyroll.tmp");
        std::fs::write(&tmp, text).with_context(|| format!("cannot write {}", tmp.display()))?;
        std::fs::rename(&tmp, path).with_context(|| format!("cannot write {}", path.display()))?;
        Ok(())
    }

    pub fn ply(&self, id: u64) -> Result<&ProjectPly> {
        self.plies.iter().find(|p| p.id == id).ok_or_else(|| anyhow!("no ply with id {id}"))
    }

    fn ply_mut(&mut self, id: u64) -> Result<&mut ProjectPly> {
        self.plies.iter_mut().find(|p| p.id == id).ok_or_else(|| anyhow!("no ply with id {id}"))
    }

    /// Imports DXF files. Parts already in the project (same file and part
    /// name) keep their reference, variants and flags; their geometry is
    /// refreshed and flagged for a re-check when the outline changed.
    pub fn import_files(&mut self, files: &[PathBuf]) -> Result<ImportReport> {
        let mut rep = ImportReport::default();
        let opts = self.import.options();
        for file in files {
            let file = canon(file);
            let res = import_dxf(&file, &opts)?;
            let fname = file.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            rep.warnings.extend(res.warnings.into_iter().map(|w| format!("{fname}: {w}")));
            let mut seen = BTreeSet::new();
            for shape in res.shapes {
                let part = shape.name.clone();
                seen.insert(part.clone());
                let sig = Signature::of(&shape);
                if let Some(ply) = self.plies.iter_mut().find(|p| p.shape.source_file == file && p.part == part) {
                    let name = std::mem::take(&mut ply.shape.name);
                    let mirrored = ply.shape.mirrored_on_import;
                    ply.shape = if mirrored { mirror_shape(&shape) } else { shape };
                    ply.shape.name = name;
                    if sig.matches(&ply.signature) {
                        rep.unchanged += 1;
                    } else {
                        ply.signature = sig;
                        if ply.reference.is_some() {
                            ply.confirmed = false;
                            ply.note = Some("outline changed in the DXF; check the 0° reference".into());
                        }
                        rep.changed += 1;
                    }
                } else {
                    let id = self.next_id;
                    self.next_id += 1;
                    let mut ply = ProjectPly {
                        id,
                        part,
                        reference: None,
                        confirmed: false,
                        note: None,
                        cosmetic: false,
                        variants: Vec::new(),
                        signature: sig,
                        shape,
                    };
                    if let Some(d) = ply.shape.layer_ref_deg {
                        ply.set_reference(d, RefOrigin::FromLayer);
                    }
                    self.plies.push(ply);
                    rep.added += 1;
                }
            }
            for ply in self.plies.iter_mut().filter(|p| p.shape.source_file == file && !seen.contains(&p.part)) {
                ply.note = Some(format!("part '{}' is no longer in {fname}", ply.part));
                rep.warnings.push(format!("{}: part '{}' is no longer in the file", fname, ply.part));
            }
        }
        Ok(rep)
    }

    /// Builds a project from CSV job rows (same result as `load_job`).
    pub fn from_job(name: impl Into<String>, rows: &[crate::job::JobRow], import: ImportSettings) -> Result<(Project, Vec<String>)> {
        let mut proj = Project::new(name);
        proj.import = import;
        let mut warnings = Vec::new();
        for row in rows {
            let before: BTreeSet<u64> = proj.plies.iter().map(|p| p.id).collect();
            let rep = proj.import_files(std::slice::from_ref(&row.file)).with_context(|| format!("job line {}", row.line))?;
            warnings.extend(rep.warnings);
            let ids: Vec<u64> = proj.plies.iter().map(|p| p.id).filter(|id| !before.contains(id)).collect();
            if ids.is_empty() {
                warnings.push(format!("job line {}: {} is already in the project", row.line, row.file.display()));
                continue;
            }
            proj.set_mirrored(&ids, row.mirror)?;
            let pick = match &row.ref_spec {
                RefSpec::Angle(d) => RefPick::Angle { deg: *d },
                RefSpec::DrawingX => RefPick::DrawingX,
                RefSpec::DrawingY => RefPick::DrawingY,
                RefSpec::LongestEdge => RefPick::LongestEdge,
                RefSpec::BoundingBox => RefPick::BoundingBox,
                RefSpec::Layer => RefPick::Layer,
            };
            proj.set_reference(&ids, &pick).with_context(|| format!("job line {}", row.line))?;
            let variants: Vec<Variant> = row.variants.iter().map(|&(angle_deg, qty)| Variant { angle_deg, qty, kit: row.kit.clone() }).collect();
            proj.set_variants(&ids, &variants)?;
            proj.set_cosmetic(&ids, row.cosmetic)?;
            if let Some(n) = &row.name {
                let multi = ids.len() > 1;
                for &id in &ids {
                    let part = proj.ply(id)?.part.clone();
                    let suffix = if multi { part.rsplit_once('#').map(|x| format!("#{}", x.1)).unwrap_or_default() } else { String::new() };
                    proj.rename(id, &format!("{n}{suffix}"))?;
                }
            }
        }
        Ok((proj, warnings))
    }

    /// Re-imports every source file of the project.
    pub fn reload(&mut self) -> Result<ImportReport> {
        let files: BTreeSet<PathBuf> = self.plies.iter().map(|p| p.shape.source_file.clone()).collect();
        let (found, missing): (Vec<PathBuf>, Vec<PathBuf>) = files.into_iter().partition(|f| f.exists());
        let mut rep = self.import_files(&found)?;
        rep.warnings.extend(missing.iter().map(|f| format!("{} not found; keeping the stored geometry", f.display())));
        Ok(rep)
    }

    /// Applies a reference pick to plies. Edge and point picks need exactly one ply.
    pub fn set_reference(&mut self, ids: &[u64], pick: &RefPick) -> Result<()> {
        if matches!(pick, RefPick::Edge { .. } | RefPick::Points { .. }) && ids.len() != 1 {
            bail!("pick an edge or two points on one ply at a time");
        }
        // Validate all first so a bulk action applies to all or none.
        let mut updates = Vec::new();
        for &id in ids {
            let ply = self.ply(id)?;
            let spec = |s: RefSpec| resolve_ref(&s, &ply.shape).map(Some);
            let r: Option<(f64, RefOrigin)> = match pick {
                RefPick::Angle { deg } => spec(RefSpec::Angle(*deg))?,
                RefPick::DrawingX => spec(RefSpec::DrawingX)?,
                RefPick::DrawingY => spec(RefSpec::DrawingY)?,
                RefPick::LongestEdge => spec(RefSpec::LongestEdge)?,
                RefPick::BoundingBox => spec(RefSpec::BoundingBox)?,
                RefPick::Layer => spec(RefSpec::Layer)?,
                RefPick::Edge { contour, seg } => {
                    let path = contour_of(&ply.shape, *contour).ok_or_else(|| anyhow!("{}: no contour {contour}", ply.shape.name))?;
                    if *seg >= path.seg_count() {
                        bail!("{}: no edge {seg}", ply.shape.name);
                    }
                    let (a, b, bulge) = path.seg(*seg);
                    if bulge.abs() > 1e-12 {
                        bail!("{}: edge {seg} is an arc; pick a straight edge or two points", ply.shape.name);
                    }
                    Some((dir_deg(a, b)?, RefOrigin::Manual))
                }
                RefPick::Points { a, b } => Some((dir_deg(*a, *b)?, RefOrigin::Manual)),
                RefPick::Rotate { delta } => {
                    let cur = ply.reference.ok_or_else(|| anyhow!("{}: set a 0° reference before turning it", ply.shape.name))?;
                    Some((cur.deg + delta, RefOrigin::Manual))
                }
                RefPick::Confirm => {
                    if ply.reference.is_none() {
                        bail!("{}: no 0° reference to confirm", ply.shape.name);
                    }
                    None
                }
            };
            updates.push((id, r));
        }
        for (id, r) in updates {
            let ply = self.ply_mut(id)?;
            match r {
                Some((deg, origin)) => ply.set_reference(deg, origin),
                None => {
                    ply.confirmed = true;
                    ply.note = None;
                }
            }
        }
        Ok(())
    }

    pub fn set_variants(&mut self, ids: &[u64], variants: &[Variant]) -> Result<()> {
        for v in variants {
            if !v.angle_deg.is_finite() || v.angle_deg.abs() > 360.0 {
                bail!("ply angle {} is not valid", v.angle_deg);
            }
        }
        for &id in ids {
            self.ply_mut(id)?.variants = variants.to_vec();
        }
        Ok(())
    }

    pub fn set_cosmetic(&mut self, ids: &[u64], on: bool) -> Result<()> {
        for &id in ids {
            self.ply_mut(id)?.cosmetic = on;
        }
        Ok(())
    }

    /// Mirrors the geometry once (CAD shows the other face). The reference is
    /// mirrored too but must be confirmed again.
    pub fn set_mirrored(&mut self, ids: &[u64], on: bool) -> Result<()> {
        for &id in ids {
            let ply = self.ply_mut(id)?;
            if ply.shape.mirrored_on_import == on {
                continue;
            }
            ply.shape = mirror_shape(&ply.shape);
            ply.shape.mirrored_on_import = on;
            if let Some(r) = ply.reference {
                ply.reference = Some(Reference { deg: norm_axis(-r.deg), origin: r.origin });
                ply.confirmed = false;
                ply.note = Some(if on { "mirrored; check the 0° reference" } else { "mirror removed; check the 0° reference" }.into());
            }
        }
        Ok(())
    }

    pub fn rename(&mut self, id: u64, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            bail!("name must not be empty");
        }
        self.ply_mut(id)?.shape.name = name.to_string();
        Ok(())
    }

    pub fn remove(&mut self, ids: &[u64]) {
        self.plies.retain(|p| !ids.contains(&p.id));
    }

    /// Plies ready for nesting. Plies without quantities are left out (with a
    /// warning); a missing or unconfirmed 0° reference is an error.
    pub fn to_plies(&self) -> Result<(Vec<PlyShape>, Vec<String>)> {
        let (plies, _, warnings) = self.plies_with_ids()?;
        Ok((plies, warnings))
    }

    fn plies_with_ids(&self) -> Result<(Vec<PlyShape>, Vec<u64>, Vec<String>)> {
        let mut out = Vec::new();
        let mut ids = Vec::new();
        let mut warnings = Vec::new();
        let mut blocked = Vec::new();
        for p in &self.plies {
            if p.total_qty() == 0 {
                warnings.push(format!("{}: no quantities set; left out", p.shape.name));
                continue;
            }
            match (p.reference, p.confirmed) {
                (Some(r), true) => {
                    ids.push(p.id);
                    out.push(PlyShape {
                        shape: p.shape.clone(),
                        ref_deg: r.deg,
                        ref_origin: r.origin,
                        cosmetic: p.cosmetic,
                        variants: p.variants.iter().filter(|v| v.qty > 0).cloned().collect(),
                    })
                }
                (None, _) => blocked.push(format!("{} (no 0° reference)", p.shape.name)),
                (Some(_), false) => blocked.push(format!("{} ({})", p.shape.name, p.note.as_deref().unwrap_or("0° reference not confirmed"))),
            }
        }
        if !blocked.is_empty() {
            bail!("set the 0° reference before nesting:\n  {}", blocked.join("\n  "));
        }
        if out.is_empty() {
            bail!("no plies with quantities in the project");
        }
        make_names_unique(&mut out);
        Ok((out, ids, warnings))
    }

    /// The solver input for this project: confirmed plies with quantities
    /// times the number of sets, and the project material (or `material`).
    pub fn nest_input(&self, material: Option<&Material>) -> Result<NestInput> {
        let material = match material.or(self.job.material.as_ref()) {
            Some(m) => m.clone(),
            None => bail!("choose a material for this project first"),
        };
        validate_material(&material)?;
        let sets = self.job.sets.max(1);
        let (mut plies, ply_ids, warnings) = self.plies_with_ids()?;
        if sets > 1 {
            for p in &mut plies {
                for v in &mut p.variants {
                    v.qty *= sets;
                }
            }
        }
        let fingerprint = fingerprint(&plies, &ply_ids, &material);
        Ok(NestInput { plies, ply_ids, material, warnings, fingerprint })
    }

    /// Is the stored result still what nesting the project now would solve?
    pub fn freshness(&self) -> Option<Freshness> {
        self.result.as_ref().map(|r| self.check_result(r))
    }

    /// Like `freshness`, for a result kept outside the project.
    pub fn check_result(&self, r: &NestResult) -> Freshness {
        match self.nest_input(None) {
            Ok(input) if input.fingerprint == r.fingerprint => Freshness { fresh: true, reason: None },
            Ok(_) => Freshness { fresh: false, reason: Some("plies, quantities, sets or material changed since this nest".into()) },
            Err(e) => Freshness { fresh: false, reason: Some(format!("{e:#}")) },
        }
    }

    pub fn set_job(&mut self, job: JobSettings) -> Result<()> {
        if job.sets == 0 || job.sets > MAX_SETS {
            bail!("sets must be between 1 and {MAX_SETS}");
        }
        if job.time_s == 0 || job.time_s > MAX_TIME_S {
            bail!("time must be between 1 s and {MAX_TIME_S} s");
        }
        if let Some(m) = &job.material {
            validate_material(m)?;
        }
        self.job = job;
        Ok(())
    }
}

/// Identifies a solver input. Source paths and how the 0° was picked do not
/// change the nest, so they are left out (a moved project keeps its result).
fn fingerprint(plies: &[PlyShape], ids: &[u64], material: &Material) -> String {
    let mut plies = plies.to_vec();
    for p in &mut plies {
        p.shape.source_file = PathBuf::new();
        p.ref_origin = RefOrigin::Manual;
    }
    let json = serde_json::to_string(&(&plies, ids, material)).unwrap_or_default();
    // FNV-1a, 64 bit: stable across Rust versions, unlike DefaultHasher.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in json.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Axis direction normalised to (−90, 90]; fibre axes are symmetric under 180°.
pub fn norm_axis(deg: f64) -> f64 {
    let r = (deg + 90.0).rem_euclid(180.0) - 90.0;
    let r = if r <= -90.0 + 1e-9 { 90.0 } else { r };
    // Clean up float noise from atan2 (e.g. 44.99999999 -> 45).
    let rounded = (r * 1e6).round() / 1e6;
    if rounded == 0.0 { 0.0 } else { rounded }
}

fn dir_deg(a: Pt, b: Pt) -> Result<f64> {
    let d = b.sub(a);
    if d.len() < 1e-6 {
        bail!("the two points are the same");
    }
    Ok(d.y.atan2(d.x).to_degrees())
}

pub fn contour_of(shape: &Shape, contour: usize) -> Option<&Path> {
    if contour == 0 { Some(&shape.outer) } else { shape.holes.get(contour - 1) }
}

/// Mirrors about the drawing Y axis, keeping the outline CCW and holes CW.
pub fn mirror_shape(s: &Shape) -> Shape {
    let m = Affine::mirror_x();
    let tol = 0.01;
    let fix = |p: &Path, want_ccw: bool| {
        let t = p.transformed(&m, tol);
        let ccw = signed_area(&t.flatten(tol, FlattenMode::Inscribed)) > 0.0;
        if ccw == want_ccw { t } else { t.reversed() }
    };
    Shape {
        name: s.name.clone(),
        source_file: s.source_file.clone(),
        outer: fix(&s.outer, true),
        holes: s.holes.iter().map(|h| fix(h, false)).collect(),
        marks: s.marks.iter().map(|k| k.transformed(&m, tol)).collect(),
        layer_ref_deg: s.layer_ref_deg.map(|d| norm_axis(-d)),
        mirrored_on_import: !s.mirrored_on_import,
    }
}

fn abs_parent(path: &FsPath) -> PathBuf {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(FsPath::new("."));
    canon(parent)
}

/// Canonical path when it exists, otherwise an absolute, lexically cleaned one.
pub fn canon(p: &FsPath) -> PathBuf {
    if let Ok(c) = std::fs::canonicalize(p) {
        return c;
    }
    let abs = if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir().map(|d| d.join(p)).unwrap_or_else(|_| p.to_path_buf()) };
    let mut out = PathBuf::new();
    for c in abs.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// `path` relative to directory `base` (both absolute); unchanged when they
/// share no directory (e.g. another drive, or only `/`).
pub fn relative_to(path: &FsPath, base: &FsPath) -> PathBuf {
    let pc: Vec<Component> = path.components().collect();
    let bc: Vec<Component> = base.components().collect();
    let common = pc.iter().zip(&bc).take_while(|(a, b)| a == b).count();
    // Sharing only the root (or drive) means unrelated places: stay absolute.
    if !pc[..common].iter().any(|c| matches!(c, Component::Normal(_))) {
        return path.to_path_buf();
    }
    let mut out = PathBuf::new();
    for _ in common..bc.len() {
        out.push("..");
    }
    for c in &pc[common..] {
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_normalisation() {
        assert_eq!(norm_axis(0.0), 0.0);
        assert_eq!(norm_axis(180.0), 0.0);
        assert_eq!(norm_axis(135.0), -45.0);
        assert_eq!(norm_axis(-90.0), 90.0);
        assert_eq!(norm_axis(90.0), 90.0);
        assert_eq!(norm_axis(270.0), 90.0);
        assert_eq!(norm_axis(-45.0000000001), -45.0);
    }

    #[test]
    fn relative_paths() {
        let r = relative_to(FsPath::new("/a/b/c/x.dxf"), FsPath::new("/a/b/d"));
        assert_eq!(r, PathBuf::from("../c/x.dxf"));
        assert_eq!(relative_to(FsPath::new("/a/x.dxf"), FsPath::new("/a")), PathBuf::from("x.dxf"));
        assert_eq!(relative_to(FsPath::new("/Users/x.dxf"), FsPath::new("/tmp/p")), PathBuf::from("/Users/x.dxf"));
    }
}
