//! templates.pdf: the layout at true size (1:1), tiled over sheets of paper or
//! plotter-roll pages, for cutting by hand.
//!
//! The first page is an index: how to print and join the pages, and the layout
//! with the tile grid. Every other page is one tile. Tiles overlap their
//! neighbours by `OVERLAP_MM`. To join them, trim a tile along its left and top
//! border and lay it over its neighbours, so that its edge sits on their dashed
//! joint line and the half registration marks on the edge line up.
//!
//! Everything is drawn in roll coordinates and clipped per tile, so a ply or a
//! label that crosses a joint continues on the next page. Roll x runs left to
//! right and roll y bottom to top, as in the DXF: the roll seen from above.
//! Tiles without any ply on them are not printed.

use crate::export::{cut_order, fibre_line, labels, placed_path};
use crate::geom::{BBox, FlattenMode, Pt};
use crate::model::{Layout, PlyShape};
use crate::pdf::{BOLD, Doc, INK, KIT, MUTED, REG, est_width, fit, text, winansi};
use crate::report::ReportMeta;
use anyhow::{Result, bail};
use pdf_writer::{Content, Str};
use serde::{Deserialize, Serialize};

/// Paper to print the templates on.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Paper {
    A4,
    A3,
    Letter,
    /// A plotter roll of this width; pages are as long as needed, at most `MAX_PAGE_MM`.
    Roll { width_mm: f64 },
}

/// Unprinted border on every side of a page, mm. Holds the page's notes.
pub const MARGIN_MM: f64 = 13.0;
/// How far neighbouring tiles overlap, mm.
pub const OVERLAP_MM: f64 = 10.0;
/// Longest page. PDF viewers refuse pages over 200 in (5080 mm).
pub const MAX_PAGE_MM: f64 = 5000.0;
/// Narrowest plotter roll accepted, mm.
pub const MIN_ROLL_MM: f64 = 100.0;

/// Points per mm.
const K: f32 = 72.0 / 25.4;

impl Paper {
    pub fn validate(&self) -> Result<()> {
        if let Paper::Roll { width_mm } = *self
            && !(width_mm.is_finite() && (MIN_ROLL_MM..=MAX_PAGE_MM).contains(&width_mm))
        {
            bail!("plotter roll width must be between {MIN_ROLL_MM} and {MAX_PAGE_MM} mm");
        }
        Ok(())
    }

    /// Sheet size (short side, long side) in mm; None for a plotter roll.
    fn sheet(&self) -> Option<(f64, f64)> {
        match self {
            Paper::A4 => Some((210.0, 297.0)),
            Paper::A3 => Some((297.0, 420.0)),
            Paper::Letter => Some((215.9, 279.4)),
            Paper::Roll { .. } => None,
        }
    }

    pub fn name(&self) -> String {
        match self {
            Paper::A4 => "A4".into(),
            Paper::A3 => "A3".into(),
            Paper::Letter => "Letter".into(),
            Paper::Roll { width_mm } => format!("{} mm plotter roll", crate::pdf::num(*width_mm)),
        }
    }
}

/// One page of the template: the part of the roll it shows, in roll mm.
#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    /// Row 0 (A) is at the top, the far roll edge (largest y).
    pub row: usize,
    /// Column 0 (1) is at the roll start.
    pub col: usize,
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl Tile {
    pub fn name(&self) -> String {
        format!("{}{}", row_name(self.row), self.col + 1)
    }
}

/// Row letters: A … Z, then AA, AB …
pub fn row_name(mut r: usize) -> String {
    let mut s = Vec::new();
    loop {
        s.push(b'A' + (r % 26) as u8);
        if r < 26 {
            break;
        }
        r = r / 26 - 1;
    }
    s.reverse();
    String::from_utf8(s).unwrap()
}

#[derive(Clone, Debug, PartialEq)]
pub struct TilePlan {
    /// Page size, mm (width along the roll, height across it).
    pub page_w: f64,
    pub page_h: f64,
    pub rows: usize,
    pub cols: usize,
    /// Row by row from row A, each row from column 1.
    pub tiles: Vec<Tile>,
}

impl TilePlan {
    pub fn tile(&self, row: usize, col: usize) -> Option<&Tile> {
        (row < self.rows && col < self.cols).then(|| &self.tiles[row * self.cols + col])
    }
}

/// Start and end of each tile along one direction: as few tiles of at most
/// `max` mm as cover 0..total, all the same size, overlapping by `OVERLAP_MM`.
fn spans(total: f64, max: f64) -> Vec<(f64, f64)> {
    let n = if total <= max { 1 } else { ((total - OVERLAP_MM) / (max - OVERLAP_MM)).ceil() as usize };
    let size = (total + (n - 1) as f64 * OVERLAP_MM) / n as f64;
    let step = size - OVERLAP_MM;
    (0..n).map(|i| (i as f64 * step, i as f64 * step + size)).collect()
}

/// Tiles for pages of `page_w` × `page_h` mm (each tile drawn from the page's
/// bottom-left margin). Spreading the tiles evenly keeps the last row and
/// column from being mostly off the roll.
fn grid(length: f64, width: f64, page_w: f64, page_h: f64) -> TilePlan {
    let along = spans(length, page_w - 2.0 * MARGIN_MM);
    let across = spans(width, page_h - 2.0 * MARGIN_MM);
    let (rows, cols) = (across.len(), along.len());
    let mut tiles = Vec::with_capacity(rows * cols);
    for row in 0..rows {
        let (y0, y1) = across[rows - 1 - row];
        for (col, &(x0, x1)) in along.iter().enumerate() {
            tiles.push(Tile { row, col, x0, y0, x1, y1 });
        }
    }
    TilePlan { page_w, page_h, rows, cols, tiles }
}

/// Splits a roll of `length` × `width` mm into pages. Sheets are turned
/// whichever way needs fewer pages (landscape on a tie); the tiles are spread
/// evenly, so they are usually a little smaller than the sheet allows.
/// Plotter-roll pages are as long as the job, split into equal pages when
/// longer than `MAX_PAGE_MM`, and only as long as their tile.
pub fn plan_tiles(length: f64, width: f64, paper: &Paper) -> TilePlan {
    let length = length.max(1.0);
    match (paper.sheet(), paper) {
        (Some((short, long)), _) => {
            let landscape = grid(length, width, long, short);
            let portrait = grid(length, width, short, long);
            if portrait.tiles.len() < landscape.tiles.len() { portrait } else { landscape }
        }
        (None, Paper::Roll { width_mm }) => {
            let mut plan = grid(length, width, MAX_PAGE_MM, *width_mm);
            plan.page_w = plan.tiles[0].x1 - plan.tiles[0].x0 + 2.0 * MARGIN_MM;
            plan
        }
        (None, _) => unreachable!("every sheet paper has a size"),
    }
}

/// Which tiles get a page: those with part of a ply (given by its bounding box) on them.
pub fn printed_tiles(plan: &TilePlan, boxes: &[BBox]) -> Vec<bool> {
    plan.tiles.iter().map(|t| boxes.iter().any(|b| b.max.x > t.x0 && b.min.x < t.x1 && b.max.y > t.y0 && b.min.y < t.y1)).collect()
}

/// One placed ply, flattened finely enough to cut along.
struct Drawn {
    loops: Vec<Vec<Pt>>,
    marks: Vec<(Vec<Pt>, bool)>,
    bbox: BBox,
}

/// Builds templates.pdf. Returns the file and the number of tile pages printed.
pub fn templates_pdf(plies: &[PlyShape], layout: &Layout, paper: &Paper, meta: &ReportMeta) -> (Vec<u8>, usize) {
    let m = &layout.material;
    let plan = plan_tiles(layout.used_length, m.roll_width, paper);
    let drawn: Vec<Drawn> = layout
        .placements
        .iter()
        .map(|pl| {
            let ply = &plies[pl.shape_idx];
            let outer = placed_path(&ply.shape.outer, pl).flatten(0.03, FlattenMode::Inscribed);
            let bbox = BBox::of(&outer);
            let mut loops = vec![outer];
            loops.extend(ply.shape.holes.iter().map(|h| placed_path(h, pl).flatten(0.03, FlattenMode::Inscribed)));
            let marks = ply.shape.marks.iter().map(|mk| (placed_path(mk, pl).flatten(0.05, FlattenMode::Inscribed), mk.closed)).collect();
            Drawn { loops, marks, bbox }
        })
        .collect();
    let boxes: Vec<BBox> = drawn.iter().map(|d| d.bbox).collect();
    let printed = printed_tiles(&plan, &boxes);
    let pages = printed.iter().filter(|p| **p).count();
    let labels = labels(plies, layout);
    let mut cut_no = vec![0usize; layout.placements.len()];
    for (n, &i) in cut_order(plies, layout).iter().enumerate() {
        cut_no[i] = n + 1;
    }

    let mut doc = Doc::default();
    index_page(&mut doc, plies, layout, &plan, &printed, &drawn, paper, meta);
    let mut page_no = 1;
    for (ti, t) in plan.tiles.iter().enumerate() {
        if !printed[ti] {
            continue;
        }
        page_no += 1;
        let neighbour = |row: isize, col: isize| -> Option<String> {
            if row < 0 || col < 0 {
                return None;
            }
            let n = plan.tile(row as usize, col as usize)?;
            printed[n.row * plan.cols + n.col].then(|| n.name())
        };
        let left = neighbour(t.row as isize, t.col as isize - 1);
        let above = neighbour(t.row as isize - 1, t.col as isize);
        let c = doc.new_page_sized(plan.page_w as f32 * K, plan.page_h as f32 * K);
        tile_page(c, plies, layout, &plan, t, &drawn, &labels, &cut_no);
        tile_notes(c, &plan, t, page_no, pages + 1, left.as_deref(), above.as_deref(), meta);
    }
    (doc.finish(&format!("1:1 templates: {}", meta.job), true), pages)
}

/// Joint lines: where each tile's trimmed left and top edges go, roll mm.
fn joints(plan: &TilePlan) -> (Vec<f64>, Vec<f64>) {
    let xs = (1..plan.cols).map(|c| plan.tiles[c].x0).collect();
    let ys = (1..plan.rows).map(|r| plan.tiles[r * plan.cols].y1).collect();
    (xs, ys)
}

#[allow(clippy::too_many_arguments)]
fn tile_page(c: &mut Content, plies: &[PlyShape], layout: &Layout, plan: &TilePlan, t: &Tile, drawn: &[Drawn], labels: &[crate::export::Label], cut_no: &[usize]) {
    let m = &layout.material;
    let mg = MARGIN_MM as f32 * K;
    let (w, h) = ((t.x1 - t.x0) as f32 * K, (t.y1 - t.y0) as f32 * K);
    // The tile hangs from the page's top-left margin.
    let oy = plan.page_h as f32 * K - mg - h;
    let px = |x: f64| mg + (x - t.x0) as f32 * K;
    let py = |y: f64| oy + (y - t.y0) as f32 * K;
    let len = layout.used_length;

    c.save_state();
    c.rect(mg, oy, w, h);
    c.clip_nonzero();
    c.end_path();
    // Outside the used roll: light grey.
    c.set_fill_rgb(0.9, 0.9, 0.9);
    c.rect(mg, oy, w, h);
    c.fill_nonzero();
    c.set_fill_rgb(1.0, 1.0, 1.0);
    c.rect(px(0.0), py(0.0), len as f32 * K, m.roll_width as f32 * K);
    c.fill_nonzero();

    // Trims, dashed.
    c.set_stroke_rgb(0.5, 0.5, 0.5);
    c.set_line_width(0.5);
    c.set_dash_pattern([4.0, 3.0], 0.0);
    if m.edge_trim > 0.0 {
        for y in [m.edge_trim, m.roll_width - m.edge_trim] {
            c.move_to(px(0.0), py(y));
            c.line_to(px(len), py(y));
        }
    }
    if m.lead_trim > 0.0 {
        c.move_to(px(m.lead_trim), py(0.0));
        c.line_to(px(m.lead_trim), py(m.roll_width));
    }
    c.stroke();
    // Joint lines, where the next tiles' edges go.
    let (jx, jy) = joints(plan);
    c.set_stroke_rgb(0.35, 0.35, 0.35);
    c.set_line_width(0.4);
    c.set_dash_pattern([1.5, 2.0], 0.0);
    for &x in &jx {
        c.move_to(px(x), oy);
        c.line_to(px(x), oy + h);
    }
    for &y in &jy {
        c.move_to(mg, py(y));
        c.line_to(mg + w, py(y));
    }
    c.stroke();
    // Kit dividers.
    c.set_stroke_rgb(KIT.0, KIT.1, KIT.2);
    c.set_line_width(0.8);
    c.set_dash_pattern([4.0, 3.0], 0.0);
    for w2 in layout.sections.windows(2) {
        let bx = w2[1].start - m.spacing / 2.0;
        c.move_to(px(bx), py(0.0));
        c.line_to(px(bx), py(m.roll_width));
    }
    c.stroke();
    c.set_dash_pattern(std::iter::empty::<f32>(), 0.0);
    // Roll edges, roll start and the cross-cut.
    c.set_stroke_rgb(0.3, 0.3, 0.3);
    c.set_line_width(0.7);
    c.move_to(px(0.0), py(0.0));
    c.line_to(px(len), py(0.0));
    c.move_to(px(0.0), py(m.roll_width));
    c.line_to(px(len), py(m.roll_width));
    c.move_to(px(0.0), py(0.0));
    c.line_to(px(0.0), py(m.roll_width));
    c.stroke();
    c.set_stroke_rgb(0.75, 0.15, 0.15);
    c.set_line_width(1.0);
    c.move_to(px(len), py(0.0));
    c.line_to(px(len), py(m.roll_width));
    c.stroke();

    let visible = |b: &BBox, pad: f64| b.max.x + pad > t.x0 && b.min.x - pad < t.x1 && b.max.y + pad > t.y0 && b.min.y - pad < t.y1;
    // Marking geometry, dashed green.
    c.set_stroke_rgb(0.1, 0.5, 0.2);
    c.set_line_width(0.4);
    c.set_dash_pattern([2.0, 1.5], 0.0);
    for d in drawn.iter().filter(|d| visible(&d.bbox, 0.0)) {
        for (pts, closed) in &d.marks {
            poly(c, pts, *closed, &px, &py);
        }
    }
    c.stroke();
    c.set_dash_pattern(std::iter::empty::<f32>(), 0.0);
    // Cut lines.
    c.set_stroke_rgb(0.0, 0.0, 0.0);
    c.set_line_width(0.35);
    for d in drawn.iter().filter(|d| visible(&d.bbox, 0.0)) {
        for lp in &d.loops {
            poly(c, lp, true, &px, &py);
        }
    }
    c.stroke();

    // Fibre direction and labels.
    for (i, l) in labels.iter().enumerate() {
        if !visible(&drawn[i].bbox, 0.0) {
            continue;
        }
        if let Some((a, b)) = fibre_line(l) {
            c.set_stroke_rgb(0.25, 0.25, 0.25);
            c.set_line_width(0.5);
            c.move_to(px(a.x), py(a.y));
            c.line_to(px(b.x), py(b.y));
            let head = (a.dist(b) * 0.12).min(4.0);
            let (s, co) = l.fibre_dir.to_radians().sin_cos();
            for (tip, dir) in [(b, 1.0), (a, -1.0)] {
                for side in [-1.0, 1.0] {
                    let (hs, hc) = (side * 25f64.to_radians()).sin_cos();
                    let (dx, dy) = (-dir * (co * hc - s * hs), -dir * (s * hc + co * hs));
                    c.move_to(px(tip.x), py(tip.y));
                    c.line_to(px(tip.x + dx * head), py(tip.y + dy * head));
                }
            }
            c.stroke();
        }
        let hmm = (l.room * 0.3).clamp(2.0, 10.0);
        let size = (hmm as f32 * K / 0.72).max(5.0);
        let no = cut_no[i].to_string();
        let tw = est_width(&no, size);
        text(c, BOLD, size, px(l.at.x) - tw / 2.0, py(l.at.y) + 0.35 * size, INK, &no);
        let small = (size * 0.5).max(5.0);
        let room = (2.0 * l.room) as f32 * K;
        let name = fit(&l.text, room, small);
        let nw = est_width(&name, small);
        text(c, REG, small, px(l.at.x) - nw / 2.0, py(l.at.y) - 1.2 * small, INK, &name);
    }

    // Registration marks centred on the joint lines.
    let mut marks: Vec<(f64, f64)> = Vec::new();
    let row_spans: Vec<(f64, f64)> = (0..plan.rows).map(|r| (plan.tiles[r * plan.cols].y0, plan.tiles[r * plan.cols].y1)).collect();
    let col_spans: Vec<(f64, f64)> = (0..plan.cols).map(|c| (plan.tiles[c].x0, plan.tiles[c].x1)).collect();
    for &x in &jx {
        for &(y0, y1) in &row_spans {
            marks.extend([(x, y0 + 0.25 * (y1 - y0)), (x, y0 + 0.75 * (y1 - y0))]);
        }
        marks.extend(jy.iter().map(|&y| (x, y)));
    }
    for &y in &jy {
        for &(x0, x1) in &col_spans {
            marks.extend([(x0 + 0.25 * (x1 - x0), y), (x0 + 0.75 * (x1 - x0), y)]);
        }
    }
    c.set_stroke_rgb(0.0, 0.0, 0.0);
    c.set_line_width(0.3);
    for (x, y) in marks {
        if x < t.x0 - 5.0 || x > t.x1 + 5.0 || y < t.y0 - 5.0 || y > t.y1 + 5.0 {
            continue;
        }
        reg_mark(c, px(x), py(y));
    }
    c.stroke();
    let _ = plies;
    c.restore_state();

    // The tile's border: the trim line on its left and top.
    c.set_stroke_rgb(0.0, 0.0, 0.0);
    c.set_line_width(0.3);
    c.rect(mg, oy, w, h);
    c.stroke();
}

/// Circle with a cross, 2.5 mm radius.
fn reg_mark(c: &mut Content, x: f32, y: f32) {
    let r = 2.5 * K;
    let arm = 4.0 * K;
    c.move_to(x - arm, y);
    c.line_to(x + arm, y);
    c.move_to(x, y - arm);
    c.line_to(x, y + arm);
    let k = 0.5523 * r;
    c.move_to(x + r, y);
    c.cubic_to(x + r, y + k, x + k, y + r, x, y + r);
    c.cubic_to(x - k, y + r, x - r, y + k, x - r, y);
    c.cubic_to(x - r, y - k, x - k, y - r, x, y - r);
    c.cubic_to(x + k, y - r, x + r, y - k, x + r, y);
}

fn poly(c: &mut Content, pts: &[Pt], closed: bool, px: &dyn Fn(f64) -> f32, py: &dyn Fn(f64) -> f32) {
    for (j, p) in pts.iter().enumerate() {
        if j == 0 {
            c.move_to(px(p.x), py(p.y));
        } else {
            c.line_to(px(p.x), py(p.y));
        }
    }
    if closed {
        c.close_path();
    }
}

/// Text turned 90° counter-clockwise (reading bottom to top), starting at (x, y).
fn text_up(c: &mut Content, font: pdf_writer::Name, size: f32, x: f32, y: f32, color: (f32, f32, f32), s: &str) {
    c.set_fill_rgb(color.0, color.1, color.2);
    c.begin_text();
    c.set_font(font, size);
    c.set_text_matrix([0.0, 1.0, -1.0, 0.0, x, y]);
    c.show(Str(&winansi(s)));
    c.end_text();
}

/// A 100 mm line with end ticks, its right end at (x, y), and what it is for.
fn check_bar(c: &mut Content, x: f32, y: f32) {
    let l = 100.0 * K;
    c.set_stroke_rgb(0.0, 0.0, 0.0);
    c.set_line_width(0.5);
    c.move_to(x - l, y);
    c.line_to(x, y);
    for tx in [x - l, x - l / 2.0, x] {
        c.move_to(tx, y - 4.0);
        c.line_to(tx, y + 4.0);
    }
    c.stroke();
    let t = "100 mm: measure it to check the print scale";
    text(c, REG, 6.5, x - l - est_width(t, 6.5) - 6.0, y - 2.0, MUTED, t);
}

#[allow(clippy::too_many_arguments)]
fn tile_notes(c: &mut Content, plan: &TilePlan, t: &Tile, page: usize, pages: usize, left: Option<&str>, above: Option<&str>, meta: &ReportMeta) {
    let (pw, ph) = (plan.page_w as f32 * K, plan.page_h as f32 * K);
    let mg = MARGIN_MM as f32 * K;
    let top = ph - 6.0 * K - 4.0;
    text(c, BOLD, 11.0, mg, top, INK, &t.name());
    let info = format!(
        "{}  ·  x {:.3}–{:.3} m along the roll, y {:.3}–{:.3} m across  ·  page {page} of {pages}",
        meta.job,
        t.x0 / 1000.0,
        t.x1 / 1000.0,
        t.y0 / 1000.0,
        t.y1 / 1000.0
    );
    let x = mg + est_width(&t.name(), 11.0) + 8.0;
    text(c, REG, 7.0, x, top + 1.0, MUTED, &fit(&info, pw - mg - x, 7.0));
    if let Some(a) = above {
        text(c, REG, 6.5, mg, ph - mg + 3.0, INK, &format!("Trim along this edge and lay it over {a}."));
    }
    if let Some(l) = left {
        let s = format!("Trim along this edge and lay it over {l}.");
        let tile_h = (t.y1 - t.y0) as f32 * K;
        text_up(c, REG, 6.5, mg - 3.0, ph - mg - tile_h, INK, &s);
    }
    check_bar(c, pw - mg, 6.0 * K);
}

#[allow(clippy::too_many_arguments)]
fn index_page(doc: &mut Doc, plies: &[PlyShape], layout: &Layout, plan: &TilePlan, printed: &[bool], drawn: &[Drawn], paper: &Paper, meta: &ReportMeta) {
    // Sheets: the index uses the same sheet, landscape. Roll: an A4 landscape page.
    let (pw, ph) = match paper {
        Paper::Roll { .. } => (297.0, 210.0),
        _ => (plan.page_w.max(plan.page_h), plan.page_w.min(plan.page_h)),
    };
    let (pw, ph) = (pw as f32 * K, ph as f32 * K);
    let c = doc.new_page_sized(pw, ph);
    let mg = MARGIN_MM as f32 * K;
    let mut y = ph - mg - 4.0;
    text(c, BOLD, 15.0, mg, y, INK, &fit(&format!("1:1 templates: {}", meta.job), pw - 2.0 * mg, 15.0));
    y -= 16.0;
    let pages = printed.iter().filter(|p| **p).count();
    let m = &layout.material;
    let info = format!(
        "{}  ·  roll {} mm  ·  {:.3} m used  ·  {} on {} ({} rows × {} columns, {} blank not printed)  ·  {}",
        m.name,
        crate::pdf::num(m.roll_width),
        layout.used_length / 1000.0,
        if pages == 1 { "1 tile".to_string() } else { format!("{pages} tiles") },
        paper.name(),
        plan.rows,
        plan.cols,
        printed.len() - pages,
        meta.date
    );
    text(c, REG, 8.5, mg, y, MUTED, &fit(&info, pw - 2.0 * mg, 8.5));
    y -= 18.0;
    let steps = [
        "1. Print at 100% (\"Actual size\", no \"Fit to page\"). Measure the 100 mm line on any page; it must be 100 mm.".to_string(),
        "2. Lay the tiles out by name: rows A, B … from the far roll edge (top), columns 1, 2 … from the roll start (left).".to_string(),
        format!("3. Trim each tile along its left and top border, lay it over its neighbours with its edge on their dashed joint line ({} mm overlap),", crate::pdf::num(OVERLAP_MM)),
        "    line up the half registration marks on the edge, and tape.".to_string(),
        "4. Seen from above, as the roll comes off the cutter: x along the roll to the right, the roll edge at y = 0 at the bottom. Plies are never flipped.".to_string(),
        "5. Solid black lines are cut lines; dashed green is marking; the arrow is the fibre direction; the big number is the cut number of the checklist.".to_string(),
    ];
    for s in &steps {
        text(c, REG, 8.5, mg, y, INK, &fit(s, pw - 2.0 * mg, 8.5));
        y -= 12.0;
    }
    y -= 8.0;

    // Overview: the roll with the tile grid.
    let len = layout.used_length.max(1.0);
    let total_w = plan.tiles.iter().map(|t| t.x1).fold(len, f64::max);
    let total_h = plan.tiles.iter().map(|t| t.y1).fold(m.roll_width, f64::max);
    let avail_w = pw - 2.0 * mg;
    let avail_h = y - mg - 20.0;
    let s = (avail_w / total_w as f32).min(avail_h / total_h as f32);
    let ox = mg;
    let oy = y - total_h as f32 * s;
    let px = |x: f64| ox + x as f32 * s;
    let py = |v: f64| oy + v as f32 * s;
    c.set_fill_rgb(0.96, 0.95, 0.92);
    c.rect(px(0.0), py(0.0), len as f32 * s, m.roll_width as f32 * s);
    c.fill_nonzero();
    c.set_stroke_rgb(0.45, 0.45, 0.45);
    c.set_line_width(0.3);
    for d in drawn {
        poly(c, &d.loops[0], true, &px, &py);
    }
    c.stroke();
    let _ = plies;
    for (i, t) in plan.tiles.iter().enumerate() {
        let (x, yy, w, h) = (px(t.x0), py(t.y0), (t.x1 - t.x0) as f32 * s, (t.y1 - t.y0) as f32 * s);
        if printed[i] {
            c.set_stroke_rgb(KIT.0, KIT.1, KIT.2);
            c.set_line_width(0.6);
        } else {
            c.set_fill_rgb(0.85, 0.85, 0.85);
            c.rect(x, yy, w, h);
            c.fill_nonzero();
            c.set_stroke_rgb(0.6, 0.6, 0.6);
            c.set_line_width(0.4);
        }
        // Inset a little so overlapping tiles stay distinguishable.
        let inset = (OVERLAP_MM as f32 * s * 0.5).min(2.0);
        c.rect(x + inset, yy + inset, (w - 2.0 * inset).max(0.5), (h - 2.0 * inset).max(0.5));
        c.stroke();
        let name = t.name();
        let size = (h * 0.35).min(w * 0.6 / name.len() as f32).clamp(0.0, 14.0);
        if size >= 4.0 {
            let col = if printed[i] { KIT } else { MUTED };
            text(c, BOLD, size, x + w / 2.0 - est_width(&name, size) / 2.0, yy + h / 2.0 - 0.35 * size, col, &name);
        }
    }
    c.set_stroke_rgb(0.75, 0.15, 0.15);
    c.set_line_width(0.8);
    c.move_to(px(len), py(0.0));
    c.line_to(px(len), py(m.roll_width));
    c.stroke();
    let legend = "Blue: printed tiles. Grey: blank (no ply), not printed. Red: cross-cut at the end of the job.";
    text(c, REG, 7.5, mg, oy - 12.0, MUTED, &fit(legend, pw - 2.0 * mg, 7.5));
    check_bar(c, pw - mg, 6.0 * K);
}
