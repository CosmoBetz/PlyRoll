//! Job input: a CSV listing ply files, their 0° reference and angle variants,
//! plus a TOML material library.
//!
//! CSV columns (header row required, `#` starts a comment line):
//! - `file`      DXF path, relative to the CSV
//! - `variants`  angles with quantities, e.g. `0x4 +45x2 -45x2` (qty defaults to 1)
//! - `ref`       0° reference: an angle in degrees from drawing X, or
//!               `x`, `y`, `longest-edge`, `bbox`, `layer`. Empty = `layer`.
//! - `cosmetic`  yes/no (optional)
//! - `mirror`    yes/no: mirror the drawing once at import (optional)
//! - `kit`       kit / layup name (optional)
//! - `name`      display name (optional; defaults to the file name)

use crate::geom::{FlattenMode, Pt};
use crate::import::{ImportOptions, import_dxf};
use crate::model::{Material, PlyShape, RefOrigin, Shape, Variant};
use anyhow::{Context, Result, anyhow, bail};
use geo::MinimumRotatedRect;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub enum RefSpec {
    Angle(f64),
    DrawingX,
    DrawingY,
    LongestEdge,
    BoundingBox,
    Layer,
}

#[derive(Clone, Debug)]
pub struct JobRow {
    pub line: usize,
    pub file: PathBuf,
    pub ref_spec: RefSpec,
    pub variants: Vec<(f64, u32)>,
    pub cosmetic: bool,
    pub mirror: bool,
    pub kit: Option<String>,
    pub name: Option<String>,
}

pub fn parse_ref(s: &str) -> Result<RefSpec> {
    let t = s.trim().to_ascii_lowercase();
    Ok(match t.as_str() {
        "" | "layer" => RefSpec::Layer,
        "x" => RefSpec::DrawingX,
        "y" => RefSpec::DrawingY,
        "longest-edge" | "edge" | "longest" => RefSpec::LongestEdge,
        "bbox" => RefSpec::BoundingBox,
        _ => RefSpec::Angle(parse_angle(&t).with_context(|| format!("invalid ref '{s}' (use an angle, x, y, longest-edge, bbox or layer)"))?),
    })
}

fn parse_angle(s: &str) -> Result<f64> {
    let t = s.trim().replace('−', "-").replace('°', "");
    let t = t.strip_suffix("deg").unwrap_or(&t);
    t.parse::<f64>().map_err(|_| anyhow!("'{s}' is not an angle"))
}

/// Parses `0x4 +45x2 -45x2` (also `×`, `*`, `;` or `,` separators).
pub fn parse_variants(s: &str) -> Result<Vec<(f64, u32)>> {
    let norm = s.replace(['×', '*'], "x").replace([';', ','], " ");
    let mut out = Vec::new();
    for tok in norm.split_whitespace() {
        let (a, q) = match tok.split_once(['x', 'X']) {
            Some((a, q)) => (a, q.parse::<u32>().map_err(|_| anyhow!("bad quantity in '{tok}'"))?),
            None => (tok, 1),
        };
        let angle = parse_angle(a).with_context(|| format!("bad angle in '{tok}'"))?;
        if q > 0 {
            out.push((angle, q));
        }
    }
    if out.is_empty() {
        bail!("no angle variants given (example: '0x4 +45x2 -45x2')");
    }
    Ok(out)
}

fn parse_bool(s: &str) -> Result<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "" | "no" | "n" | "false" | "0" => Ok(false),
        "yes" | "y" | "true" | "1" | "x" => Ok(true),
        _ => bail!("expected yes/no, got '{s}'"),
    }
}

pub fn read_job_csv(path: &Path) -> Result<Vec<JobRow>> {
    let mut rdr = csv::ReaderBuilder::new().comment(Some(b'#')).trim(csv::Trim::All).flexible(true).from_path(path).with_context(|| format!("cannot read {}", path.display()))?;
    let headers: HashMap<String, usize> = rdr.headers()?.iter().enumerate().map(|(i, h)| (h.to_ascii_lowercase(), i)).collect();
    for req in ["file", "variants"] {
        if !headers.contains_key(req) {
            bail!("{}: missing column '{req}' (columns: file, ref, variants, cosmetic, mirror, kit, name)", path.display());
        }
    }
    let base = path.parent().unwrap_or(Path::new("."));
    let mut rows = Vec::new();
    for (i, rec) in rdr.records().enumerate() {
        let rec = rec?;
        let line = rec.position().map_or(i + 2, |p| p.line() as usize);
        let get = |k: &str| headers.get(k).and_then(|&i| rec.get(i)).unwrap_or("").to_string();
        let ctx = || format!("{} line {line}", path.display());
        let file = get("file");
        if file.is_empty() {
            continue;
        }
        let opt = |s: String| if s.is_empty() { None } else { Some(s) };
        rows.push(JobRow {
            line,
            file: base.join(&file),
            ref_spec: parse_ref(&get("ref")).with_context(ctx)?,
            variants: parse_variants(&get("variants")).with_context(ctx)?,
            cosmetic: parse_bool(&get("cosmetic")).with_context(ctx)?,
            mirror: parse_bool(&get("mirror")).with_context(ctx)?,
            kit: opt(get("kit")),
            name: opt(get("name")),
        });
    }
    if rows.is_empty() {
        bail!("{}: no plies listed", path.display());
    }
    Ok(rows)
}

/// Resolves the 0° reference of a shape. Returns degrees in drawing coordinates.
pub fn resolve_ref(spec: &RefSpec, shape: &Shape) -> Result<(f64, RefOrigin)> {
    Ok(match spec {
        RefSpec::Angle(a) => (*a, RefOrigin::Manual),
        RefSpec::DrawingX => (0.0, RefOrigin::DrawingX),
        RefSpec::DrawingY => (90.0, RefOrigin::DrawingY),
        RefSpec::Layer => (
            shape.layer_ref_deg.ok_or_else(|| {
                anyhow!("{}: no 0° reference set and no line on a fibre layer (FIBRE*, 0DEG*, ...); set the 'ref' column", shape.name)
            })?,
            RefOrigin::FromLayer,
        ),
        RefSpec::LongestEdge => {
            let segs = shape.outer.straight_segments();
            let best = segs.iter().copied().max_by(|a, b| a.0.total_cmp(&b.0)).ok_or_else(|| anyhow!("{}: outline has no straight edge for 'longest-edge'", shape.name))?;
            (best.1, RefOrigin::LongestEdge)
        }
        RefSpec::BoundingBox => {
            let pts = shape.outer.flatten(0.05, FlattenMode::Inscribed);
            let poly = geo::Polygon::new(pts.iter().map(|p| geo::Coord { x: p.x, y: p.y }).collect::<Vec<_>>().into(), vec![]);
            let rect = poly.minimum_rotated_rect().ok_or_else(|| anyhow!("{}: cannot compute bounding box", shape.name))?;
            let c: Vec<Pt> = rect.exterior().coords().map(|c| Pt::new(c.x, c.y)).collect();
            let (e1, e2) = (c[1].sub(c[0]), c[2].sub(c[1]));
            let long = if e1.len() >= e2.len() { e1 } else { e2 };
            (long.y.atan2(long.x).to_degrees(), RefOrigin::BoundingBox)
        }
    })
}

pub struct LoadedJob {
    pub plies: Vec<PlyShape>,
    pub warnings: Vec<String>,
}

/// Imports every file in the job and attaches 0° references and variants.
pub fn load_job(rows: &[JobRow], base_opts: &ImportOptions) -> Result<LoadedJob> {
    let mut plies = Vec::new();
    let mut warnings = Vec::new();
    for row in rows {
        let opts = ImportOptions { mirror: row.mirror, ..base_opts.clone() };
        let res = import_dxf(&row.file, &opts).with_context(|| format!("job line {}", row.line))?;
        for w in res.warnings {
            warnings.push(format!("{}: {w}", row.file.display()));
        }
        let multi = res.shapes.len() > 1;
        if multi {
            warnings.push(format!("{}: contains {} parts; each gets the same 0° rule and variants", row.file.display(), res.shapes.len()));
        }
        for mut shape in res.shapes {
            if let Some(n) = &row.name {
                shape.name = if multi { format!("{n}{}", shape.name.rsplit_once('#').map(|x| format!("#{}", x.1)).unwrap_or_default()) } else { n.clone() };
            }
            let (ref_deg, ref_origin) = resolve_ref(&row.ref_spec, &shape).with_context(|| format!("job line {}", row.line))?;
            let variants = row.variants.iter().map(|&(angle_deg, qty)| Variant { angle_deg, qty, kit: row.kit.clone() }).collect();
            plies.push(PlyShape { shape, ref_deg, ref_origin, cosmetic: row.cosmetic, variants });
        }
    }
    // Shape names must be unique for labels.
    let mut seen: HashMap<String, usize> = HashMap::new();
    for p in &mut plies {
        let n = seen.entry(p.shape.name.clone()).or_default();
        *n += 1;
        if *n > 1 {
            p.shape.name = format!("{}~{}", p.shape.name, n);
        }
    }
    Ok(LoadedJob { plies, warnings })
}

#[derive(serde::Deserialize)]
struct MaterialFile {
    material: Vec<Material>,
}

pub fn load_materials(path: &Path) -> Result<Vec<Material>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let mf: MaterialFile = toml::from_str(&text).with_context(|| format!("invalid material file {}", path.display()))?;
    for m in &mf.material {
        validate_material(m)?;
    }
    Ok(mf.material)
}

pub fn validate_material(m: &Material) -> Result<()> {
    let ctx = |msg: &str| anyhow!("material '{}': {msg}", m.name);
    if m.roll_width <= 0.0 {
        return Err(ctx("roll_width must be positive"));
    }
    if m.edge_trim < 0.0 || m.lead_trim < 0.0 || m.spacing < 0.0 || m.angle_tolerance < 0.0 {
        return Err(ctx("trims, spacing and angle_tolerance must not be negative"));
    }
    if m.usable_width() <= 0.0 {
        return Err(ctx("edge_trim leaves no usable width"));
    }
    if m.angle_tolerance > 10.0 {
        return Err(ctx("angle_tolerance above 10° is not plausible"));
    }
    if m.warp_weft_interchangeable && m.kind != crate::model::MaterialKind::Twill {
        return Err(ctx("warp_weft_interchangeable only applies to twill"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_syntax() {
        assert_eq!(parse_variants("0x4 +45x2 -45x2").unwrap(), vec![(0.0, 4), (45.0, 2), (-45.0, 2)]);
        assert_eq!(parse_variants("90;30×3, −60*2").unwrap(), vec![(90.0, 1), (30.0, 3), (-60.0, 2)]);
        assert!(parse_variants("").is_err());
        assert!(parse_variants("45xa").is_err());
    }

    #[test]
    fn ref_syntax() {
        assert_eq!(parse_ref("").unwrap(), RefSpec::Layer);
        assert_eq!(parse_ref("X").unwrap(), RefSpec::DrawingX);
        assert_eq!(parse_ref("12.5°").unwrap(), RefSpec::Angle(12.5));
        assert_eq!(parse_ref("longest-edge").unwrap(), RefSpec::LongestEdge);
        assert!(parse_ref("diagonal").is_err());
    }
}
