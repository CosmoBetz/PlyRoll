# PlyRoll

Free, local nesting for carbon-fibre prepreg plies on an endless roll.
Every ply keeps its fibre direction (UD) or warp direction (twill); plies are never flipped.

Status: **Phase 1 command-line tool; Phase 2 desktop app with orientation editor, material library, nesting and reports; Phase 3 started (kit-by-kit nesting).** See [docs/PLAN.md](docs/PLAN.md) for the full plan.
Nesting is done by [sparrow](https://github.com/JeroenGar/sparrow) on top of
[jagua-rs](https://github.com/JeroenGar/jagua-rs).

## Build

Needs Rust (install from <https://rustup.rs>).

```bash
cargo build --release
```

The binary is `target/release/plyroll`.

## Quick start (demo job)

```bash
cargo run -p plyroll-core --example make_samples -- examples/demo/samples
```

```bash
./target/release/plyroll check examples/demo/job.csv -m UD-T700-150 -o out/demo
```

```bash
./target/release/plyroll nest examples/demo/job.csv -m UD-T700-150 -o out/demo --time 60
```

`check` only imports the job and writes `orientation.svg`, which shows each shape's 0° axis (dashed) and the fibre
direction of every angle variant (coloured hatching). Look at it before nesting.

## Desktop app

A Tauri app for setting each ply's 0° direction by eye, listing its ply angles, and nesting the job.
It saves a `.plyroll` project (JSON) with the job settings and the last result. The CLI nests that project directly.

Needs Node.js 20.19+ as well as Rust. On macOS, the Xcode command-line tools are enough.

```bash
cd app && npm install
```

```bash
cd app && npm run tauri dev
```

How to use it:

1. **Import DXF…** (or drop files on the window). Every outline becomes a ply.
   A line on a `FIBRE*` / `0DEG*` layer sets its 0° automatically.
2. Set 0° on each ply in the large view:
   - **click an edge** that runs along 0°;
   - or **P** and two points (they snap to corners);
   - or **X**, **Y**, **L** (longest edge), **B** (bounding box), **F** (fibre layer), or type an angle;
   - **[ ]** or **Q E** turn 0° by 1°; hold Shift for 15°.
3. Add the ply angles and quantities (quick buttons 0 / +45 / −45 / 90, quasi-iso set).
   The hatching previews each angle's fibre direction.
4. Badges show *no 0°*, *check 0°* (outline changed in the DXF, or mirrored) and *0° set*.
   **N** jumps to the next ply that needs attention; **Enter** confirms.
   Select several plies (⌘/⇧-click) to apply a rule or angle list to all of them.
5. Switch to **Nest** (⌘2; ⌘1 goes back to the plies).

Nesting:

1. Pick a material, or **Manage materials…** to add one. The library is shared by all projects and uses the same
   TOML format as the CLI. The project keeps its own copy of the material. If the library entry changes later, the app
   offers to use the new version.
2. Choose a time (30 s / 2 min / 10 min or seconds) and the number of **sets** (complete layups; multiplies every quantity).
   **Layout:** *Mixed* nests all plies together for the shortest roll. *Kit by kit* gives each kit its own stretch of
   roll, one after another, so a kit's plies are cut together and get consecutive cut numbers. Kits come in project
   order (the order they first appear in the ply list); plies without a kit come last. With several sets, a kit's
   stretch holds all its sets. The time is shared between kits by ply area. It usually costs a little roll length.
3. **Nest.** The roll view shows the best layout so far, with fibre hatching per ply (zoom in) and the used length.
   **Stop and keep best** ends early and keeps a complete, verified layout.
4. The result is saved with the project. Changing plies, quantities, sets or material marks it *out of date*.
   Export is then disabled until you nest again. Undo brings it back.
5. **Export cut files…** writes the same files as the CLI (see [Output](#output)) to a folder you pick.

Or nest the saved project from the command line. It uses the project's material, time, sets and layout
(`--grouping global|by-kit` overrides the layout, and works for job CSVs too):

```bash
./target/release/plyroll nest my-job.plyroll -o out/my-job
```

**Reload DXFs** re-reads every file. 0° references are kept, and parts whose outline changed are flagged for a check.
**Import settings…** sets the units (from each file's `$INSUNITS`, or forced to mm / cm / m / inch for every file) and
the join and gap tolerances, the same as `--units`, `--join-tol` and `--gap-tol` in the CLI. It lists each DXF with the
units it declares. Applying re-reads every DXF in place. A ply whose outline only changed scale keeps its confirmed 0°;
any other change is flagged. Undo goes back. Tolerances are in mm after scaling, so a larger unit may need a larger gap
tolerance. The inspector shows each ply's size in mm, so a wrong unit is easy to spot.
`plyroll convert job.csv` turns an existing job CSV into a project. `npm run dev` (in `app/`) runs the UI in a
browser with demo data, for UI work without the desktop shell.

## Job file (CSV)

```csv
file,ref,variants,cosmetic,mirror,kit,name
plies/rib.dxf,x,0x4 +45x2 -45x2,,,A,
plies/flange.dxf,layer,0x2 +45x2 -45x2,,,B,
plies/skin.dxf,12.5,0x2 90x1,yes,,A,Skin-outer
```

| column | meaning |
|---|---|
| `file` | DXF path, relative to the CSV |
| `variants` | angles and quantities: `0x4 +45x2 -45x2`. Any angle works, quantity defaults to 1 |
| `ref` | the shape's 0° direction: an angle from drawing X, or `x`, `y`, `longest-edge`, `bbox` (long axis of the smallest bounding rectangle), `layer` (a line on a `FIBRE*`/`0DEG*` layer). Empty = `layer` |
| `cosmetic` | `yes` keeps twill warp in place even when the material allows swapping |
| `mirror` | `yes` mirrors the drawing once at import, if CAD shows the other face |
| `kit` | kit / layup name (written to `plies.csv`) |
| `name` | label name (default: file name) |

The fibre direction of a variant in the drawing is `ref + angle`, counter-clockwise positive, as seen in the drawing.
A shape with no usable `ref` stops the job; there is no silent default.

`plyroll inspect plies/*.dxf` lists every part with its size, longest straight edge (and its angle), holes and any fibre-layer line. Use it to pick `ref` values.

## Materials (TOML)

```toml
[[material]]
name = "UD-T700-150"
kind = "ud"            # or "twill"
roll_width = 600       # mm
edge_trim = 10         # each long edge
lead_trim = 20         # start of the roll
spacing = 2            # gap between plies
angle_tolerance = 0    # allowed fibre deviation, degrees
cost_per_m2 = 45.0     # optional
# twill only: allow 90° turns (warp <-> weft) for non-cosmetic plies
# warp_weft_interchangeable = true
```

By default the file `materials.toml` next to the job CSV is used; `--materials` overrides it.
For a CSV job `-m` is required. For a `.plyroll` project it is optional once a material is picked in the app (`-m` then overrides it).

The desktop app keeps its library in the same format, at
`~/Library/Application Support/org.plyroll.desktop/materials.toml` on macOS. Pass it to the CLI with `--materials`.

Allowed placement rotations:
- **UD and default twill:** fibre/warp along the roll, turned by 0° or 180°.
- **Twill with `warp_weft_interchangeable`:** also 90° and 270°. Plies marked `cosmetic` keep the strict rule.

## Output

| file | content |
|---|---|
| `layout.dxf` | layers `CUT` (outlines + holes, exact arcs, roll order, holes first), `MARK` (fibre line per ply + source marks), `LABEL` (text), `ROLL` (edges, trims, lead trim) |
| `layout.svg` | preview, true size in mm |
| `plies.csv` | every cut ply in cutting order with position, rotation, kit and source |
| `report.pdf` | nest report (A4 landscape): material, length, utilisation, scrap, cost, verification, the layout with cut numbers and a position scale, then a kit checklist with a tick box per ply, its cut number and its position on the roll |
| `labels.csv` | one row per cut ply for label printers: cut number, label text, shape, angle, copy *n of m*, kit, material, job, position |
| `report.json` | length, utilisation, cost, verification result, full layout |
| `orientation.svg` | 0° axes and fibre directions before nesting |

Roll coordinates: x along the roll from its start, y across it from one edge.

Every layout is re-checked with the exact geometry before any cut file is written:
quantities, fibre direction, no part outside the trims, spacing between parts.
If a check fails, only `layout.svg` and `report.json` are written and the exit code is 2.

Kit by kit, the report's layout shows a divider and the kit name where each kit starts, the checklist headings give
each kit's stretch of roll, and `report.json` lists the sections.

The cut number is the same everywhere: the `order` column of `plies.csv`, `cut_no` in `labels.csv`, and the numbers on
the report's layout and checklist. The checklist groups plies by kit, in the project's order (shape, then angle); there
is no separate layup sequence yet.

## Importer

Reads LINE, ARC, CIRCLE, LWPOLYLINE (bulges), POLYLINE, SPLINE (NURBS), ELLIPSE and block INSERTs, with mirrored
(-Z extrusion) entities and `$INSUNITS`. Joins loose segments (`--join-tol`, default 0.01 mm), bridges small gaps with a
warning (`--gap-tol`, default 0.1 mm), removes duplicate entities, finds holes, splits several parts in one file, and
stops on self-intersecting outlines. Layers named `MARK*`, `PEN*` … are carried as marking; `DIM*`, `TEXT*`, `INFO*` …
are ignored.

## Known limits

- No manual editing of a layout, and no warm start from a previous result.
- No true-size PDF of the layout (for printing templates) yet. The SVG prints at true size.
- Parts are not nested inside other parts' holes.
- DXF table headers have no handles (a quirk of the `dxf` crate writer). ezdxf and PlyRoll read the files fine.
  Check that your cutter software does too.

## Tests

```bash
cargo test --release
```

## License

PlyRoll is released under the [MIT License](LICENSE).

Its dependencies keep their own licenses. Notably:
- [sparrow](https://github.com/JeroenGar/sparrow) is MIT.
- [jagua-rs](https://github.com/JeroenGar/jagua-rs) is MPL-2.0. That is file-level copyleft: changes to jagua-rs's own files must be shared under MPL-2.0. PlyRoll's code is not affected.

If you distribute PlyRoll binaries, include these license notices.
