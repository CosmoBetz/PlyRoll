//! Kit-grouped nesting: each kit is nested on its own section of roll, and the
//! sections follow each other along the roll, so a kit's plies are cut together.
//!
//! A kit belongs to an angle variant, not to a ply shape, so one shape can be in
//! several sections. Sections come in the order kits first appear in the project
//! (shape, then variant), the same order as the report's checklist; plies
//! without a kit form the last section. With several sets, a section holds all
//! sets of its kit.

use crate::model::{Layout, Material, Placement, PlyShape};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

/// How plies are spread over the roll.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Grouping {
    /// All plies mixed: the shortest roll.
    #[default]
    Global,
    /// One section of roll per kit, one after another.
    ByKit,
}

/// One kit's stretch of roll in a kit-grouped layout, roll x in mm.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayoutSection {
    pub kit: Option<String>,
    /// Where the section's plies may start (lead trim, or the previous section's end plus spacing).
    pub start: f64,
    /// Far end of its last ply.
    pub end: f64,
}

/// The plies of one kit, ready to nest on their own.
#[derive(Clone, Debug)]
pub struct Section {
    pub kit: Option<String>,
    pub plies: Vec<PlyShape>,
    /// Per section ply: its index in the full list, and per variant its full variant index.
    pub index: Vec<(usize, Vec<usize>)>,
}

impl Section {
    /// Total ply area of the section (outline area × quantity), mm².
    pub fn area(&self) -> f64 {
        self.plies.iter().map(|p| p.shape.area(0.1) * p.variants.iter().map(|v| v.qty as f64).sum::<f64>()).sum()
    }

    /// A placement of this section in the full ply list's indices.
    pub fn to_full(&self, p: &Placement) -> Placement {
        let (shape_idx, vars) = &self.index[p.shape_idx];
        Placement { shape_idx: *shape_idx, variant_idx: vars[p.variant_idx], ..p.clone() }
    }
}

/// Kits with something to cut, in first-seen order; `None` (no kit) last.
pub fn kit_order(plies: &[PlyShape]) -> Vec<Option<String>> {
    let mut kits: Vec<Option<String>> = Vec::new();
    for v in plies.iter().flat_map(|p| p.variants.iter()).filter(|v| v.qty > 0) {
        if !kits.contains(&v.kit) {
            kits.push(v.kit.clone());
        }
    }
    kits.sort_by_key(|k| k.is_none()); // stable: kits keep their order
    kits
}

/// Splits the plies into one section per kit.
pub fn sections(plies: &[PlyShape]) -> Vec<Section> {
    kit_order(plies)
        .into_iter()
        .map(|kit| {
            let mut sec = Section { kit: kit.clone(), plies: Vec::new(), index: Vec::new() };
            for (si, p) in plies.iter().enumerate() {
                let vars: Vec<usize> = (0..p.variants.len()).filter(|&vi| p.variants[vi].qty > 0 && p.variants[vi].kit == kit).collect();
                if vars.is_empty() {
                    continue;
                }
                let mut sub = p.clone();
                sub.variants = vars.iter().map(|&vi| p.variants[vi].clone()).collect();
                sec.plies.push(sub);
                sec.index.push((si, vars));
            }
            sec
        })
        .collect()
}

/// The material for nesting a section whose plies start at roll x `start`:
/// the solver puts plies after the lead trim, so the lead trim moves there.
pub fn section_material(material: &Material, start: f64) -> Material {
    Material { lead_trim: start, ..material.clone() }
}

/// Where the next section starts after one that ends at `end`.
pub fn next_start(material: &Material, end: f64) -> f64 {
    end + material.spacing
}

/// Joins section layouts (each nested with `section_material`, in roll
/// coordinates) into one layout of the full ply list.
pub fn merge(material: &Material, parts: &[(&Section, &Layout)]) -> Result<Layout> {
    let mut placements = Vec::new();
    let mut sections = Vec::new();
    let mut start = material.lead_trim;
    for (sec, lay) in parts {
        ensure!((lay.material.lead_trim - start).abs() < 1e-6, "kit section nested at {} mm, expected {} mm", lay.material.lead_trim, start);
        placements.extend(lay.placements.iter().map(|p| sec.to_full(p)));
        let end = lay.used_length.max(start);
        sections.push(LayoutSection { kit: sec.kit.clone(), start, end });
        start = next_start(material, end);
    }
    let used_length = sections.last().map_or(material.lead_trim, |s| s.end);
    Ok(Layout { material: material.clone(), placements, used_length, sections })
}
