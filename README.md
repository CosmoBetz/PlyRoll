# PlyRoll

Free, local nesting for carbon-fibre prepreg plies on an endless roll.
Every ply keeps its fibre direction (UD) or warp direction (twill); plies are never flipped.

Status: **Phase 1 — command-line tool.** See [docs/PLAN.md](docs/PLAN.md) for the full plan.
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

Allowed placement rotations:
- **UD and default twill:** fibre/warp along the roll, turned by 0° or 180°.
- **Twill with `warp_weft_interchangeable`:** also 90° and 270°. Plies marked `cosmetic` keep the strict rule.

## Output

| file | content |
|---|---|
| `layout.dxf` | layers `CUT` (outlines + holes, exact arcs, roll order, holes first), `MARK` (fibre line per ply + source marks), `LABEL` (text), `ROLL` (edges, trims, lead trim) |
| `layout.svg` | preview, true size in mm |
| `plies.csv` | every cut ply in cutting order with position, rotation, kit and source |
| `report.json` | length, utilisation, cost, verification result, full layout |
| `orientation.svg` | 0° axes and fibre directions before nesting |

Roll coordinates: x along the roll from its start, y across it from one edge.

Every layout is re-checked with the exact geometry before any cut file is written:
quantities, fibre direction, no part outside the trims, spacing between parts.
If a check fails, only `layout.svg` and `report.json` are written and the exit code is 2.

## Importer

Reads LINE, ARC, CIRCLE, LWPOLYLINE (bulges), POLYLINE, SPLINE (NURBS), ELLIPSE and block INSERTs, with mirrored
(-Z extrusion) entities and `$INSUNITS`. Joins loose segments (`--join-tol`, default 0.01 mm), bridges small gaps with a
warning (`--gap-tol`, default 0.1 mm), removes duplicate entities, finds holes, splits several parts in one file, and
stops on self-intersecting outlines. Layers named `MARK*`, `PEN*` … are carried as marking; `DIM*`, `TEXT*`, `INFO*` …
are ignored.

## Known limits (Phase 1)

- No GUI yet. The 0° reference comes from the CSV (the orientation editor is Phase 2).
- No PDF output yet. The SVG prints at true size.
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
