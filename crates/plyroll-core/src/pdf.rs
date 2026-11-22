//! report.pdf: the nest report for the shop floor. The first page has the job's
//! numbers and the layout with cut numbers; the kit checklist follows.
//!
//! Only the PDF base fonts are used (Helvetica, Courier), so nothing is embedded.
//! Text is WinAnsi-encoded; characters outside it print as '?'. Text widths are
//! not measured: columns are left-aligned and long names are cut short with a
//! conservative width estimate. Numbers that must line up use Courier, which is
//! monospaced (0.6 em per character).

use crate::export::{angle_color, cut_order, labels, placed_path};
use crate::geom::{BBox, FlattenMode, Pt};
use crate::model::{Layout, MaterialKind, PlyShape};
use crate::report::{KitGroup, ReportMeta, RunInfo, Summary, checklist};
use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect, Ref, Str, TextStr};

// A4 landscape, points.
const PAGE_W: f32 = 841.89;
const PAGE_H: f32 = 595.28;
const MARGIN: f32 = 36.0;
const BODY_W: f32 = PAGE_W - 2.0 * MARGIN;
/// Lowest y the body may use; the footer sits below it.
const BOTTOM: f32 = MARGIN + 16.0;

pub(crate) const REG: Name = Name(b"F1");
pub(crate) const BOLD: Name = Name(b"F2");
pub(crate) const MONO: Name = Name(b"F3");

pub(crate) const INK: (f32, f32, f32) = (0.1, 0.1, 0.1);
pub(crate) const MUTED: (f32, f32, f32) = (0.42, 0.42, 0.42);
pub(crate) const KIT: (f32, f32, f32) = (0.18, 0.40, 0.80);
pub(crate) const RULE: (f32, f32, f32) = (0.82, 0.82, 0.82);

/// Builds the whole report.
pub fn nest_report(plies: &[PlyShape], layout: &Layout, summary: &Summary, run: &RunInfo, warnings: &[String], meta: &ReportMeta) -> Vec<u8> {
    let groups = checklist(plies, layout);
    let mut doc = Doc::default();
    let mut y = header(&mut doc, plies, layout, summary, run, warnings, meta, &groups);
    layout_strips(&mut doc, &mut y, plies, layout, meta);
    checklist_pages(&mut doc, &groups, &layout.sections, meta);
    let material = &layout.material.name;
    let seed = run.seed;
    doc.each_page(|i, n, c, _, _| {
        let left = format!("PlyRoll nest report  ·  {}  ·  {material}  ·  {}  ·  seed {seed}", meta.job, meta.date);
        text(c, REG, 7.0, MARGIN, MARGIN, MUTED, &fit(&left, BODY_W - 80.0, 7.0));
        let right = format!("page {} of {n}", i + 1);
        text(c, REG, 7.0, PAGE_W - MARGIN - est_width(&right, 7.0), MARGIN, MUTED, &right);
    });
    doc.finish(&format!("Nest report: {}", meta.job), false)
}

#[derive(Default)]
pub(crate) struct Doc {
    /// Content with its page width and height, points.
    pages: Vec<(Content, f32, f32)>,
}

impl Doc {
    /// A new A4 landscape page.
    fn new_page(&mut self) -> &mut Content {
        self.new_page_sized(PAGE_W, PAGE_H)
    }
    pub(crate) fn new_page_sized(&mut self, w: f32, h: f32) -> &mut Content {
        self.pages.push((Content::new(), w, h));
        &mut self.pages.last_mut().unwrap().0
    }
    pub(crate) fn page(&mut self) -> &mut Content {
        &mut self.pages.last_mut().unwrap().0
    }
    /// Calls `f(index, count, content, width, height)` on every page, for footers.
    pub(crate) fn each_page(&mut self, mut f: impl FnMut(usize, usize, &mut Content, f32, f32)) {
        let n = self.pages.len();
        for (i, (c, w, h)) in self.pages.iter_mut().enumerate() {
            f(i, n, c, *w, *h);
        }
    }

    /// The finished file. `actual_size` asks the print dialog not to scale the
    /// pages (`/PrintScaling /None`), for true-size templates.
    pub(crate) fn finish(self, title: &str, actual_size: bool) -> Vec<u8> {
        let n = self.pages.len();
        let mut pdf = Pdf::new();
        let catalog = Ref::new(1);
        let tree = Ref::new(2);
        let fonts = [(REG, Ref::new(3), "Helvetica"), (BOLD, Ref::new(4), "Helvetica-Bold"), (MONO, Ref::new(5), "Courier")];
        let info = Ref::new(6);
        let page_ids: Vec<Ref> = (0..n).map(|i| Ref::new(7 + 2 * i as i32)).collect();

        {
            let mut cat = pdf.catalog(catalog);
            cat.pages(tree);
            if actual_size {
                cat.viewer_preferences().pair(Name(b"PrintScaling"), Name(b"None"));
            }
        }
        pdf.pages(tree).kids(page_ids.iter().copied()).count(n as i32);
        for (i, (content, w, h)) in self.pages.into_iter().enumerate() {
            let content_id = Ref::new(8 + 2 * i as i32);
            let mut page = pdf.page(page_ids[i]);
            page.media_box(Rect::new(0.0, 0.0, w, h));
            page.parent(tree);
            page.contents(content_id);
            {
                let mut res = page.resources();
                let mut f = res.fonts();
                for (name, id, _) in fonts {
                    f.pair(name, id);
                }
            }
            page.finish();
            let data = miniz_oxide::deflate::compress_to_vec_zlib(&content.finish(), 6);
            pdf.stream(content_id, &data).filter(Filter::FlateDecode);
        }
        for (_, id, base) in fonts {
            pdf.type1_font(id).base_font(Name(base.as_bytes())).encoding_predefined(Name(b"WinAnsiEncoding"));
        }
        pdf.document_info(info).title(TextStr(title)).producer(TextStr("PlyRoll"));
        pdf.finish()
    }
}

// ---------------------------------------------------------------- page 1

#[allow(clippy::too_many_arguments)]
fn header(
    doc: &mut Doc,
    plies: &[PlyShape],
    layout: &Layout,
    s: &Summary,
    run: &RunInfo,
    warnings: &[String],
    meta: &ReportMeta,
    groups: &[KitGroup],
) -> f32 {
    let m = &layout.material;
    let c = doc.new_page();
    let mut y = PAGE_H - MARGIN - 14.0;
    text(c, BOLD, 16.0, MARGIN, y, INK, &fit(&format!("Nest report: {}", meta.job), BODY_W - 140.0, 16.0));
    text(c, REG, 9.0, PAGE_W - MARGIN - est_width(&meta.date, 9.0), y + 4.0, MUTED, &meta.date);
    y -= 18.0;
    let kind = match m.kind {
        MaterialKind::Ud => "UD".to_string(),
        MaterialKind::Twill if m.warp_weft_interchangeable => "twill, warp/weft may swap".to_string(),
        MaterialKind::Twill => "twill".to_string(),
    };
    let tol = if m.angle_tolerance > 0.0 { format!("±{}°", num(m.angle_tolerance)) } else { "exact".into() };
    let line = format!(
        "{} ({kind})  ·  roll {} mm, usable {} mm  ·  edge trim {} mm  ·  lead trim {} mm  ·  spacing {} mm  ·  fibre {tol}",
        m.name,
        num(m.roll_width),
        num(m.usable_width()),
        num(m.edge_trim),
        num(m.lead_trim),
        num(m.spacing)
    );
    text(c, REG, 9.5, MARGIN, y, INK, &fit(&line, BODY_W, 9.5));
    y -= 12.0;
    if let Some(r) = &meta.roll {
        let needed = layout.used_length / 1000.0;
        let after = r.before_m - needed;
        let short = after < -1e-9;
        let mut line = format!("Roll {}", r.id);
        if !r.batch.is_empty() {
            line.push_str(&format!("  ·  batch {}", r.batch));
        }
        line.push_str(&format!("  ·  {:.2} m on the roll before this job, {:.2} m after", r.before_m, after.max(0.0)));
        if let Some(e) = &r.expires {
            line.push_str(&format!("  ·  use by {e}{}", if r.expired { " (EXPIRED)" } else { "" }));
        }
        let color = if short || r.expired { (0.7, 0.1, 0.1) } else { INK };
        text(c, REG, 9.5, MARGIN, y, color, &fit(&line, BODY_W, 9.5));
        y -= 12.0;
        if short {
            let t = format!("Not enough roll: this job needs {needed:.2} m, roll {} has {:.2} m.", r.id, r.before_m);
            text(c, BOLD, 9.5, MARGIN, y, (0.7, 0.1, 0.1), &fit(&t, BODY_W, 9.5));
            y -= 12.0;
        }
    }

    // Verification banner.
    let v = &s.verification;
    let (bg, fg, msg) = if v.ok {
        let gap = v.min_gap.map_or("-".to_string(), |g| format!("{g:.2} mm"));
        (
            (0.89, 0.96, 0.9),
            (0.1, 0.42, 0.2),
            format!(
                "Verified: every ply is on the roll inside the trims, no overlaps, smallest gap {gap}, largest fibre deviation {:.3}°, quantities complete.",
                v.max_fibre_deviation
            ),
        )
    } else {
        ((0.99, 0.9, 0.9), (0.7, 0.1, 0.1), format!("FAILED VERIFICATION, do not cut: {}", v.issues.join("; ")))
    };
    fill_rect(c, MARGIN, y - 18.0, BODY_W, 16.0, bg);
    text(c, BOLD, 9.0, MARGIN + 6.0, y - 13.0, fg, &fit(&msg, BODY_W - 12.0, 9.0));
    y -= 30.0;

    // Numbers: two rows of five.
    let scrap = (s.roll_area_m2 - s.parts_area_m2).max(0.0);
    let scrap_pct = if s.roll_area_m2 > 0.0 { 100.0 * scrap / s.roll_area_m2 } else { 0.0 };
    let shapes = {
        let mut v: Vec<usize> = layout.placements.iter().map(|p| p.shape_idx).collect();
        v.sort_unstable();
        v.dedup();
        v.len()
    };
    let kits = groups.iter().filter(|g| g.kit.is_some()).count();
    let time = format!("{:.0} s", run.solve_seconds);
    let cost = match (s.material_cost, m.cost_per_m2) {
        (Some(c), Some(per)) => (format!("{c:.2}"), format!("Material cost ({}/m² of roll)", num(per))),
        _ => ("-".into(), "Material cost (no price set)".into()),
    };
    let cells: [(String, String); 10] = [
        (format!("{:.3} m", s.used_length_mm / 1000.0), "Used length (with lead trim)".into()),
        (format!("{:.1}%", 100.0 * s.utilization_of_usable_area), "Utilisation of usable area".into()),
        (format!("{:.1}%", 100.0 * s.utilization_of_roll), "Utilisation of whole roll".into()),
        (format!("{scrap:.3} m²"), format!("Scrap ({scrap_pct:.1}% of {:.3} m² roll)", s.roll_area_m2)),
        cost,
        (s.plies.to_string(), "Plies to cut".into()),
        (shapes.to_string(), "Different shapes".into()),
        (if kits == 0 { "-".into() } else { kits.to_string() }, "Kits".into()),
        (format!("{:.3} m²", s.parts_area_m2), "Ply area".into()),
        (
            time,
            {
                let mut label = String::from("Solve time");
                if run.stopped {
                    label.push_str(" (stopped early)");
                }
                if run.kept > 0 {
                    label.push_str(&format!(", after {} locked", run.kept));
                }
                if run.edits > 0 {
                    label.push_str(&format!(", {} hand edit{}", run.edits, if run.edits == 1 { "" } else { "s" }));
                }
                label
            },
        ),
    ];
    let cw = BODY_W / 5.0;
    for (i, (value, label)) in cells.iter().enumerate() {
        let x = MARGIN + cw * (i % 5) as f32;
        let row_y = y - 46.0 * (i / 5) as f32;
        text(c, BOLD, 14.0, x, row_y - 14.0, INK, value);
        text(c, REG, 7.5, x, row_y - 26.0, MUTED, &fit(label, cw - 8.0, 7.5));
    }
    y -= 98.0;

    if !warnings.is_empty() {
        text(c, BOLD, 8.0, MARGIN, y, INK, "Import notes");
        y -= 11.0;
        for w in warnings.iter().take(3) {
            text(c, REG, 7.5, MARGIN, y, MUTED, &fit(w, BODY_W, 7.5));
            y -= 10.0;
        }
        if warnings.len() > 3 {
            text(c, REG, 7.5, MARGIN, y, MUTED, &format!("+ {} more in report.json", warnings.len() - 3));
            y -= 10.0;
        }
        y -= 6.0;
    }

    // Legend: colour = angle.
    let mut angles: Vec<f64> = layout.placements.iter().map(|p| plies[p.shape_idx].variants[p.variant_idx].angle_deg).collect();
    angles.sort_by(f64::total_cmp);
    angles.dedup();
    let mut x = MARGIN;
    text(c, BOLD, 8.0, x, y, INK, "Layout");
    x += 40.0;
    for a in angles {
        if x > PAGE_W - MARGIN - 60.0 {
            text(c, REG, 8.0, x, y, MUTED, "…");
            break;
        }
        fill_rect(c, x, y - 1.0, 10.0, 8.0, light(hex(angle_color(a))));
        stroke_rect(c, x, y - 1.0, 10.0, 8.0, (0.3, 0.3, 0.3), 0.4);
        let t = fmt_angle(a);
        text(c, REG, 8.0, x + 13.0, y, INK, &t);
        x += 13.0 + est_width(&t, 8.0) + 12.0;
    }
    let note = "Numbers = cut order (see the checklist). Roll runs left to right, 0° along the roll.";
    if PAGE_W - MARGIN - x > 120.0 {
        text(c, REG, 8.0, x + 6.0, y, MUTED, &fit(note, PAGE_W - MARGIN - x - 6.0, 8.0));
    }
    y - 10.0
}

/// The roll, cut into stacked strips when it is long so the scale stays readable.
fn layout_strips(doc: &mut Doc, y: &mut f32, plies: &[PlyShape], layout: &Layout, meta: &ReportMeta) {
    let m = &layout.material;
    let len = layout.used_length.max(1.0) as f32;
    let rw = m.roll_width as f32;
    const MAX_H: f32 = 200.0; // tallest strip, pt
    const MIN_H: f32 = 90.0; // a strip is never squeezed below this, pt
    let target = (BODY_W / len).max(MIN_H / rw).min(MAX_H / rw);
    let strips = ((len * target / BODY_W).ceil() as usize).max(1);
    let seg = len / strips as f32;
    let s = (BODY_W / seg).min(MAX_H / rw); // pt per mm
    let h = rw * s;

    let order = cut_order(plies, layout);
    let mut cut_no = vec![0usize; layout.placements.len()];
    for (n, &i) in order.iter().enumerate() {
        cut_no[i] = n + 1;
    }
    let tol = (0.4 / s as f64).clamp(0.1, 5.0);
    let outlines: Vec<(Vec<Vec<Pt>>, BBox)> = layout
        .placements
        .iter()
        .map(|pl| {
            let ply = &plies[pl.shape_idx];
            let outer = placed_path(&ply.shape.outer, pl).flatten(tol, FlattenMode::Inscribed);
            let b = BBox::of(&outer);
            let mut loops = vec![outer];
            loops.extend(ply.shape.holes.iter().map(|hl| placed_path(hl, pl).flatten(tol, FlattenMode::Inscribed)));
            (loops, b)
        })
        .collect();
    let labels = labels(plies, layout);

    for k in 0..strips {
        let x0 = seg * k as f32;
        let x1 = if k + 1 == strips { len } else { seg * (k + 1) as f32 };
        if *y - h - 26.0 < BOTTOM {
            let c = doc.new_page();
            *y = PAGE_H - MARGIN - 12.0;
            text(c, BOLD, 11.0, MARGIN, *y, INK, &fit(&format!("Layout (continued): {}", meta.job), BODY_W, 11.0));
            *y -= 16.0;
        }
        let c = doc.page();
        let oy = *y - h;
        let px = |x: f64| MARGIN + (x as f32 - x0) * s;
        let py = |v: f64| oy + v as f32 * s;
        let w = (x1 - x0) * s;

        c.save_state();
        c.rect(MARGIN, oy, w, h);
        c.clip_nonzero();
        c.end_path();
        fill_rect(c, MARGIN, oy, w, h, (0.957, 0.945, 0.918));
        // Trims, dashed.
        c.set_stroke_rgb(0.55, 0.55, 0.55);
        c.set_line_width(0.5);
        c.set_dash_pattern([3.0, 2.0], 0.0);
        if m.edge_trim > 0.0 {
            for t in [m.edge_trim, m.roll_width - m.edge_trim] {
                c.move_to(MARGIN, py(t));
                c.line_to(MARGIN + w, py(t));
            }
        }
        if m.lead_trim > 0.0 && (m.lead_trim as f32) > x0 && (m.lead_trim as f32) < x1 {
            c.move_to(px(m.lead_trim), oy);
            c.line_to(px(m.lead_trim), oy + h);
        }
        c.stroke();
        c.set_dash_pattern(std::iter::empty::<f32>(), 0.0);
        // Plies.
        c.set_line_width(0.4);
        c.set_stroke_rgb(0.2, 0.2, 0.2);
        for (i, (loops, b)) in outlines.iter().enumerate() {
            if (b.max.x as f32) < x0 || (b.min.x as f32) > x1 {
                continue;
            }
            let pl = &layout.placements[i];
            let (r, g, bl) = light(hex(angle_color(plies[pl.shape_idx].variants[pl.variant_idx].angle_deg)));
            c.set_fill_rgb(r, g, bl);
            for lp in loops {
                for (j, p) in lp.iter().enumerate() {
                    if j == 0 {
                        c.move_to(px(p.x), py(p.y));
                    } else {
                        c.line_to(px(p.x), py(p.y));
                    }
                }
                c.close_path();
            }
            c.fill_even_odd_and_stroke();
        }
        c.restore_state();

        // Roll edges and, on the last strip, the cross-cut line.
        c.set_stroke_rgb(0.3, 0.3, 0.3);
        c.set_line_width(0.7);
        c.move_to(MARGIN, oy);
        c.line_to(MARGIN + w, oy);
        c.move_to(MARGIN, oy + h);
        c.line_to(MARGIN + w, oy + h);
        if k == 0 {
            c.move_to(MARGIN, oy);
            c.line_to(MARGIN, oy + h);
        }
        c.stroke();
        // Kit by kit: a divider between kits and each kit's name at its start.
        c.set_stroke_rgb(KIT.0, KIT.1, KIT.2);
        c.set_line_width(0.9);
        c.set_dash_pattern([2.0, 2.0], 0.0);
        for w2 in layout.sections.windows(2) {
            let bx = (w2[1].start - layout.material.spacing / 2.0) as f32;
            if bx > x0 && bx < x1 {
                c.move_to(px(bx as f64), oy - 3.0);
                c.line_to(px(bx as f64), oy + h + 3.0);
            }
        }
        c.stroke();
        c.set_dash_pattern(std::iter::empty::<f32>(), 0.0);
        for sec in &layout.sections {
            let sx = sec.start as f32;
            if sx >= x0 && sx < x1 {
                let name = sec.kit.as_deref().map_or_else(|| "no kit".to_string(), |k| format!("Kit {k}"));
                let room = (x1.min(sec.end as f32) - sx) * s - 4.0;
                let t = fit(&name, room.max(12.0), 7.0);
                let tw = est_width(&t, 7.0);
                fill_rect(c, px(sec.start) + 1.0, oy + h - 10.0, tw + 4.0, 9.0, (1.0, 1.0, 1.0));
                text(c, BOLD, 7.0, px(sec.start) + 3.0, oy + h - 7.5, KIT, &t);
            }
        }
        if k + 1 == strips {
            c.set_stroke_rgb(0.75, 0.15, 0.15);
            c.set_line_width(1.0);
            c.move_to(MARGIN + w, oy - 3.0);
            c.line_to(MARGIN + w, oy + h + 3.0);
            c.stroke();
        }

        // Cut numbers at each ply's label point.
        for (i, l) in labels.iter().enumerate() {
            let lx = l.at.x as f32;
            if lx < x0 || lx >= x1 {
                continue;
            }
            let t = cut_no[i].to_string();
            let room = l.room as f32 * s;
            let digits = t.len() as f32 * 0.556;
            let size = (room * 0.9).min(9.0).min(1.9 * room / digits).max(3.0);
            let tw = digits * size;
            // Keep the number inside its strip when the ply straddles a strip end.
            // min/max, not clamp: a strip can be narrower than the number.
            let tx = (px(l.at.x) - tw / 2.0).min(MARGIN + w - tw - 1.0).max(MARGIN + 1.0);
            text(c, BOLD, size, tx, py(l.at.y) - 0.36 * size, INK, &t);
        }

        // Position scale under the strip.
        let tick = [50.0f32, 100.0, 200.0, 250.0, 500.0, 1000.0, 2000.0, 2500.0, 5000.0, 10000.0, 20000.0]
            .into_iter()
            .find(|t| t * s >= 42.0)
            .unwrap_or(50000.0);
        let dec = if tick % 1000.0 == 0.0 { 0 } else if tick % 100.0 == 0.0 { 1 } else { 2 };
        c.set_stroke_rgb(0.4, 0.4, 0.4);
        c.set_line_width(0.4);
        let mut t = (x0 / tick).ceil() * tick;
        let mut marks = Vec::new();
        while t <= x1 + 0.01 {
            let x = MARGIN + (t - x0) * s;
            c.move_to(x, oy - 2.0);
            c.line_to(x, oy - 6.0);
            marks.push((x, format!("{:.*} m", dec, t / 1000.0)));
            t += tick;
        }
        c.stroke();
        for (x, label) in marks {
            text(c, REG, 6.5, x - est_width(&label, 6.5) / 2.0, oy - 13.0, MUTED, &label);
        }
        if k + 1 == strips {
            let end = format!("end {:.3} m", layout.used_length / 1000.0);
            let ew = est_width(&end, 7.0);
            let ex = (MARGIN + w - ew).max(MARGIN);
            text(c, BOLD, 7.0, ex, oy - 22.0, (0.75, 0.15, 0.15), &end);
        }
        *y = oy - 30.0;
    }
}

// ---------------------------------------------------------------- checklist

const COL_PLY: f32 = MARGIN + 14.0;
const COL_ANGLE: f32 = MARGIN + 200.0;
const COL_QTY: f32 = MARGIN + 250.0;
const COL_CUTS: f32 = MARGIN + 290.0;
const ENTRY_SIZE: f32 = 8.0;
const LINE: f32 = 12.0;

fn checklist_pages(doc: &mut Doc, groups: &[KitGroup], sections: &[crate::kits::LayoutSection], meta: &ReportMeta) {
    let all: Vec<usize> = groups.iter().flat_map(|g| g.rows.iter().flat_map(|r| r.cuts.iter().map(|c| c.cut_no))).collect();
    let no_w = all.iter().max().map_or(1, |n| n.to_string().len());
    let max_pos = groups.iter().flat_map(|g| g.rows.iter().flat_map(|r| r.cuts.iter().map(|c| c.x_mm))).fold(0.0f64, f64::max);
    let pos_w = format!("{:.2}", max_pos / 1000.0).len();
    // "☐ 12 @ 1.45 m": box, then Courier text of fixed length.
    let entry_chars = no_w + 3 + pos_w + 2;
    let entry_w = 10.0 + entry_chars as f32 * 0.6 * ENTRY_SIZE + 14.0;
    let per_line = (((PAGE_W - MARGIN - COL_CUTS) / entry_w).floor() as usize).max(1);

    let mut y = checklist_page(doc, meta, false);
    let total: usize = groups.iter().map(KitGroup::plies).sum();
    for g in groups {
        let title = match &g.kit {
            Some(k) => format!("Kit {k}"),
            None => "No kit".to_string(),
        };
        let heading = |doc: &mut Doc, y: &mut f32, cont: bool| {
            let c = doc.page();
            fill_rect(c, MARGIN, *y - 5.0, BODY_W, 17.0, (0.93, 0.93, 0.93));
            let mut t = format!("{title}{}  ·  {} plies", if cont { " (continued)" } else { "" }, g.plies());
            if let Some(sec) = sections.iter().find(|s| s.kit == g.kit) {
                t.push_str(&format!("  ·  roll {:.2}–{:.2} m", sec.start / 1000.0, sec.end / 1000.0));
            }
            text(c, BOLD, 10.5, MARGIN + 4.0, *y, INK, &fit(&t, BODY_W - 8.0, 10.5));
            *y -= 19.0;
        };
        if y - 17.0 - 2.0 * LINE < BOTTOM {
            y = checklist_page(doc, meta, true);
        }
        heading(doc, &mut y, false);
        for r in &g.rows {
            let lines = r.cuts.len().div_ceil(per_line);
            let row_h = lines as f32 * LINE + 4.0;
            if y - row_h < BOTTOM {
                y = checklist_page(doc, meta, true);
                heading(doc, &mut y, true);
            }
            let c = doc.page();
            text(c, REG, 9.0, COL_PLY, y, INK, &fit(&r.shape, COL_ANGLE - COL_PLY - 8.0, 9.0));
            text(c, REG, 9.0, COL_ANGLE, y, INK, &fmt_angle(r.angle_deg));
            text(c, MONO, 9.0, COL_QTY, y, INK, &format!("{:>3}", r.cuts.len()));
            for (j, cut) in r.cuts.iter().enumerate() {
                let x = COL_CUTS + (j % per_line) as f32 * entry_w;
                let ly = y - (j / per_line) as f32 * LINE;
                stroke_rect(c, x, ly - 1.0, 7.0, 7.0, (0.25, 0.25, 0.25), 0.6);
                let e = format!("{:>nw$} @ {:>pw$.2} m", cut.cut_no, cut.x_mm / 1000.0, nw = no_w, pw = pos_w);
                text(c, MONO, ENTRY_SIZE, x + 10.0, ly, INK, &e);
            }
            y -= row_h;
            c.set_stroke_rgb(RULE.0, RULE.1, RULE.2);
            c.set_line_width(0.3);
            c.move_to(MARGIN, y + 11.0);
            c.line_to(PAGE_W - MARGIN, y + 11.0);
            c.stroke();
        }
        y -= 8.0;
    }
    if y - 14.0 < BOTTOM {
        y = checklist_page(doc, meta, true);
    }
    let kits = groups.iter().filter(|g| g.kit.is_some()).count();
    let t = format!("Total: {total} plies{}", if kits > 0 { format!(" in {kits} kit{}", if kits == 1 { "" } else { "s" }) } else { String::new() });
    text(doc.page(), BOLD, 9.5, MARGIN, y, INK, &t);
}

/// Starts a checklist page with its title and column headings; returns the
/// first row's baseline.
fn checklist_page(doc: &mut Doc, meta: &ReportMeta, cont: bool) -> f32 {
    let c = doc.new_page();
    let mut y = PAGE_H - MARGIN - 14.0;
    let title = format!("Kit checklist{}: {}", if cont { " (continued)" } else { "" }, meta.job);
    text(c, BOLD, 14.0, MARGIN, y, INK, &fit(&title, BODY_W, 14.0));
    y -= 14.0;
    if !cont {
        text(
            c,
            REG,
            8.5,
            MARGIN,
            y,
            MUTED,
            "Tick each ply as you pick it. The cut number matches the layout and the order column of plies.csv; @ is the distance from the roll start.",
        );
        y -= 10.0;
        text(c, REG, 8.5, MARGIN, y, MUTED, "Kits and plies are in project order (shape, then angle); the project has no layup sequence.");
        y -= 6.0;
    }
    y -= 12.0;
    for (x, h) in [(COL_PLY, "Ply"), (COL_ANGLE, "Angle"), (COL_QTY, "Qty"), (COL_CUTS, "Cut no. @ position on the roll")] {
        text(c, BOLD, 8.0, x, y, MUTED, h);
    }
    c.set_stroke_rgb(0.5, 0.5, 0.5);
    c.set_line_width(0.5);
    c.move_to(MARGIN, y - 4.0);
    c.line_to(PAGE_W - MARGIN, y - 4.0);
    c.stroke();
    y - 20.0
}

// ---------------------------------------------------------------- helpers

pub(crate) fn text(c: &mut Content, font: Name, size: f32, x: f32, y: f32, color: (f32, f32, f32), s: &str) {
    c.set_fill_rgb(color.0, color.1, color.2);
    c.begin_text();
    c.set_font(font, size);
    c.next_line(x, y);
    c.show(Str(&winansi(s)));
    c.end_text();
}

pub(crate) fn fill_rect(c: &mut Content, x: f32, y: f32, w: f32, h: f32, color: (f32, f32, f32)) {
    c.set_fill_rgb(color.0, color.1, color.2);
    c.rect(x, y, w, h);
    c.fill_nonzero();
}

pub(crate) fn stroke_rect(c: &mut Content, x: f32, y: f32, w: f32, h: f32, color: (f32, f32, f32), lw: f32) {
    c.set_stroke_rgb(color.0, color.1, color.2);
    c.set_line_width(lw);
    c.rect(x, y, w, h);
    c.stroke();
}

/// Rough Helvetica width: an average of 0.56 em per character. Only used to
/// place short right-aligned or centred labels.
pub(crate) fn est_width(s: &str, size: f32) -> f32 {
    s.chars().count() as f32 * 0.56 * size
}

/// Cuts `s` so it fits `max_w`, assuming a wide 0.6 em per character.
pub(crate) fn fit(s: &str, max_w: f32, size: f32) -> String {
    let max = (max_w / (0.6 * size)).floor().max(1.0) as usize;
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Encodes text for the WinAnsi base-font encoding.
pub(crate) fn winansi(s: &str) -> Vec<u8> {
    s.chars()
        .map(|ch| match ch {
            ' '..='~' => ch as u8,
            '\u{a0}'..='\u{ff}' => ch as u32 as u8,
            '€' => 0x80,
            '‚' => 0x82,
            'ƒ' => 0x83,
            '„' => 0x84,
            '…' => 0x85,
            '†' => 0x86,
            '‡' => 0x87,
            'ˆ' => 0x88,
            '‰' => 0x89,
            'Š' => 0x8a,
            '‹' => 0x8b,
            'Œ' => 0x8c,
            'Ž' => 0x8e,
            '‘' => 0x91,
            '’' => 0x92,
            '“' => 0x93,
            '”' => 0x94,
            '•' => 0x95,
            '–' => 0x96,
            '—' => 0x97,
            '˜' => 0x98,
            '™' => 0x99,
            'š' => 0x9a,
            '›' => 0x9b,
            'œ' => 0x9c,
            'ž' => 0x9e,
            'Ÿ' => 0x9f,
            '−' => b'-',
            _ => b'?',
        })
        .collect()
}

pub(crate) fn hex(c: &str) -> (f32, f32, f32) {
    let v = u32::from_str_radix(c.trim_start_matches('#'), 16).unwrap_or(0x888888);
    (((v >> 16) & 255) as f32 / 255.0, ((v >> 8) & 255) as f32 / 255.0, (v & 255) as f32 / 255.0)
}

/// The angle colour mixed with white, so outlines and numbers stay readable.
pub(crate) fn light((r, g, b): (f32, f32, f32)) -> (f32, f32, f32) {
    let k = 0.55;
    (r + (1.0 - r) * k, g + (1.0 - g) * k, b + (1.0 - b) * k)
}

pub(crate) fn fmt_angle(a: f64) -> String {
    if a == 0.0 {
        "0°".into()
    } else if a.fract() == 0.0 {
        format!("{a:+}°")
    } else {
        format!("{a:+.1}°")
    }
}

/// A length without trailing zeros: 600, 2.5.
pub(crate) fn num(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_and_fitting() {
        assert_eq!(winansi("rib +45° ±2 m² ü−€"), b"rib +45\xb0 \xb12 m\xb2 \xfc-\x80".to_vec());
        assert_eq!(winansi("板"), b"?".to_vec());
        assert_eq!(fit("short", 100.0, 10.0), "short");
        let cut = fit("a very long shape name indeed", 60.0, 10.0);
        assert_eq!(cut.chars().count(), 10);
        assert!(cut.ends_with('…'));
        assert_eq!(fmt_angle(-45.0), "-45°");
        assert_eq!(fmt_angle(22.5), "+22.5°");
        assert_eq!(num(600.0), "600");
        assert_eq!(num(2.5), "2.5");
    }
}
