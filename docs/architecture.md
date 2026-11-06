# umcfGui architecture

An OpenFOAM case editor, runner and viewer for urbanMicroclimateFoam, built as strictly layered
Rust crates.

!!! info "Status"
    The crates that exist today are `core`, `geom`, `foam-dict`, `foam-field`, `foam-mesh`, `schema` (case layout
    and the generated [solver catalog](solver-coverage.md)), `model`, `validate`, `render`, `engine`, `ui-egui` and the `umcf-cli` and `umcf-gui` apps, plus the `xtask` tooling.
    The others in the table below are planned.

## Core patterns

| Pattern | How it works here |
|---|---|
| Strict layers checked by a tool | `cargo xtask layers` reads `cargo metadata`. A crate may depend only on lower layers, plus explicit chains within a layer (table below) |
| UI quarantine | UI crates (`egui*`, `eframe`, `winit`, `rfd`, `wgpu*`) only at L6+. `wgpu` only if the 3D view needs it, and only in `ui-egui` |
| Format crates kept narrow | `foam-*` depend only on `core`, `geom` and the `foam-*` below them. They know OpenFOAM syntax, not UMCF |
| One command registry | `CommandSpec { id, label, menu, shortcut, params, enabled, run, journal, undoable }`. `enabled` returns `Err(reason)`, which the UI shows as a tooltip |
| Session as the single entry point | `Session::execute(id, Json)` with a panic guard, an undo snapshot and a journal. The case model is `Arc`-cloned for cheap undo snapshots |
| Headless parity | `execute_headless` rejects commands that would only open a dialog. The CLI and MCP use it |
| Control channel | JSON lines over TCP: `engine.execute`, `ui.screenshot`, `ui.inspect`, plus `run.events` (a stream of job progress) |
| Agent control | `mcp` crate: tools = commands; `--connect` drives the running app |
| Window snapshot | `umcf-gui <case> --screenshot shot.png` (under `xvfb-run` without a display) writes a PNG of the window once it has drawn; the UI tests drive the interface without a window (egui_kittest). This is how agents check UI work |
| Fuzzing invariants | Every command survives empty params. Random command sequences are checked against undo/redo and "save → reload → identical model" |
| `xtask` as the only tooling | `ci`, `layers`, `assets`, `catalog`, `wasm`, `docs`, `deny`, `examples` |

## Layer table

```
L0  core        ids, units (SI + typed quantities), errors, case paths
    geom        Vec3, bbox, transforms, STL/OBJ triSurface I/O
L1  foam-dict   lossless FoamFile dictionary CST: parse, edit-in-place, write
    foam-field  vol*/surface* fields, uniform/nonuniform lists, gzip, time dirs, processor* dirs
    foam-mesh   polyMesh (points/faces/owner/neighbour/boundary, cellZones), ASCII + binary;
                the boundary alone (BoundaryMesh) for drawing and picking
L2  schema      catalog of solver types and keywords (generated in part by xtask catalog)
    model       typed Case built on foam-dict nodes: regions, materials, BCs, controls, sun/weather
L3  validate    case diagnostics (missing dicts, region/patch coupling mismatches, unknown types)
    post        probes, time series, slices, residual/log parsing, derived quantities the solver writes
    render      CPU rasteriser for patches, slices and colour maps (headless snapshots, wasm)
L4  runner      optional (feature `runner`): runs the pipeline steps locally (processes, MPI, logs, cancel)
L5  engine      Session, commands, history, journal, guard, sample case
L6  ui-egui     panels, 3D view, theme tokens, control channel server
    mcp         MCP server over engine (headless) or over the control channel (--connect)
L7  apps        umcf-gui (desktop), umcf-cli (check / run / render / mcp)
```

Chains within a layer: `foam-dict → foam-field → foam-mesh` (the mesh reader uses the field
reader's header, gzip and number parsing), `schema → model`, `post → render`.

`runner` is optional and the only native-only crate below L6 (it spawns processes). `engine`
depends on it only through `features = ["runner"]`, which the desktop and CLI apps turn on by
default and can be built without (`--no-default-features`).

The split, given that most cases are too big to run locally:
- **Always there (`model` and `engine`):** the pipeline as data (`Pipeline` = ordered `Step`s,
  each with a command line, inputs, outputs and the number of processors) and `run.export`. That
  export writes `Allprepare`/`Allrun` and a cluster job script (SLURM/PBS template: nodes, tasks,
  wall time, modules). The usual workflow is export → run on the cluster → copy back or mount →
  `case.open`, and `post` reads the results.
- **Only with `runner`:** `run.start` / `run.stop` / `run.status` and the `run.events` stream.
  Without the feature these commands stay in the registry with `enabled` returning
  `Err("built without local runner")`, so the UI, MCP and scripts all see the same set of
  commands.
- OpenFOAM runs only on Linux. The runner (planned, row 12 of [the gaps](gaps.md)) is to run
  steps locally first, then reach OpenFOAM where it is: through WSL on Windows, in a virtual
  machine or container (macOS), or on a remote machine by ssh with a scheduler, all through the
  same `Pipeline` (see [the common workflow](common-workflow.md#running-anywhere)).

## Domain mapping (from the solver source)

- **Regions:** `air` (fluid), solid regions (`buildings`, `street`, `roofs`, … one per material
  group) and `vegetation`. Each region has `constant/<r>`, `system/<r>` and `0/<r>`.
  `model::Case` is the set of regions plus the top-level `system/controlDict` and
  `decomposeParDict`.
- **Selectable types** (`schema`): building materials (`buildingMaterialModel`: Hamstad*,
  VanGenuchten*, Soil, PorousAsphalt, OSB, …), vegetation (`simplifiedVegetation`,
  `noVegetation`), grass (`simpleGrass`, `noGrass`), solar (`directAndDiffuse`, `none`),
  turbulence (`porousrealizableKE`), and the coupled boundary conditions
  (`compressible::CFDHAM*`, `mappedLeafTemperature`, `viewFactorSky`, …). Each catalog entry
  gives its keywords, their types and units, defaults, and the source file it came from.
- **Pipeline** (`runner`), taken from the reference `Allprepare`/`Allrun`: mesh generation per
  region, `decomposePar -allRegions`, copying non-mesh `constant/` files into `processor*`
  (needed on OF12), `faceAgglomerate`, `calcLAI`, `viewFactorsGen`, `solarRayTracingGen`,
  `urbanMicroclimateFoam -parallel` and `reconstructPar -allRegions`. Each step is a typed
  `Step` (in `model`) with inputs, outputs and a log. `run.export` always writes the equivalent
  scripts, so the GUI never locks anyone in. With the local runner, steps whose inputs haven't
  changed are skipped.
- **Results** (`post`): time directories per region, `postProcessing/` probes, and the residuals
  and continuity errors parsed from `log.*`.

## Geometry, meshing and the 3D view

The workflow the GUI supports end to end:

1. **Import geometry** (`geometry.import`): STL files (ASCII or binary, with or without named
   `solid`s) for buildings, terrain and trees go to `constant/triSurface`. `geom` reads them
   without trusting the header, and reports what would break meshing:
   - open and non-manifold edges;
   - inconsistent or inward normals;
   - degenerate and duplicate triangles;
   - coordinates that look like millimetres;
   - surfaces outside the domain.

   Each report carries the triangles involved. Safe repairs are commands of their own:
   - welding with a tolerance;
   - dropping degenerate triangles;
   - making normals consistent;
   - scaling.
2. **Set up the mesh** (`mesh.setup`, `mesh.block`, `mesh.refine`, `mesh.layers`): typed views
   in `model` over `blockMeshDict`, `snappyHexMeshDict` and `surfaceFeaturesDict`, edited
   losslessly like every other dictionary. The keyword set differs per OpenFOAM version
   (`insidePoint` vs `locationInMesh`), so `schema` profiles decide which ones are used. Before
   anything runs, `validate` checks the set-up:
   - the inside point lies in the fluid, not inside a building;
   - refinement surfaces exist and are closed where they must be;
   - the background mesh covers the geometry;
   - the cell size matches the refinement levels.
3. **Mesh** (pipeline steps): `blockMesh`, `surfaceFeatures`, `snappyHexMesh`, `checkMesh` and
   the region-extrusion steps of the urbanMicroclimateFoam workflow. They are always exported as
   scripts, and run locally with `runner`.
4. **Check the mesh** (`mesh.check`): the result of `checkMesh` (its log, and the face and cell
   sets of `-writeSets`) is the authority. `validate` also computes the same metrics natively
   from `foam-mesh`, with `checkMesh`'s thresholds, as a fast preview and for meshes that were
   never checked:
   - non-orthogonality, skewness, aspect ratio;
   - negative or tiny volumes, open cells;
   - layer coverage.

   Every problem carries the faces or cells involved.
5. **Assign boundary conditions in 3D** (`bc.set`): picking in the view selects patches (or
   groups, or a region's whole boundary). `bc.set` takes that selection as its parameters, so
   the same assignment is a CLI or MCP call too. Picking faces to create a new patch
   (`patch.create`, via `createPatch`/`topoSet`) is the same flow.

### The 3D view

The GUI's 3D view borrows proven patterns from
[SolidCraft](https://github.com/tangency-labs/solidcraft) (MIT/Apache-2.0, also a pure-Rust,
egui, command-driven app). Code ported from it keeps its copyright notice and gets a row in
`ATTRIBUTION.md`.

| Pattern | Here |
|---|---|
| Renderer-agnostic scene in a low layer | `render::Scene` (meshes, per-face colour classes, opacity) is built from `foam-mesh` and `geom` data and stays wasm-clean. The wgpu viewport in `ui-egui` and the CPU rasteriser (headless snapshots, CI, remote machines without a GPU) draw the same scene |
| Colour by palette, not by vertex | Patches, regions and quality metrics are colour classes. Recolouring (by patch, by BC type, by non-orthogonality) changes the palette, never re-uploads a 50-million-face mesh |
| Level of detail | Big surfaces and meshes get simplified display meshes built off the UI thread. Picking always uses the full data |
| CPU picking with a BVH | Ray casts against a triangle BVH (`geom`), built on the first pick (in the desktop app, on the UI thread). Each triangle maps back to its patch and face (or STL triangle), so a pick is a patch, a surface region or (picking faces) a mesh face, not a pixel |
| Selection lives in the engine | `Session.selection` (`Sel::Patch`, `Sel::Solid` for a surface region, `Sel::Face` for a boundary face of a mesh; `Sel::Cells` to come) is plain data, set by the `select.*` commands. A command run with no explicit target uses the current selection, which is how "pick, then assign" works |
| Hover and "select other" | Hovering highlights the patch under the cursor. Holding still lists everything under it (overlapping regions, internal patches) |
| Navigation in one place | One input handler with presets (orbit / pan / zoom to cursor, ParaView-like and CAD-like), view cube, standard views, section planes to see inside the domain |
| UI built from the command registry | Toolbar, menus and the command palette are generated from `CommandSpec`. A disabled command shows its reason |
| Headless UI scenarios | Scripted steps (open, pick at a world point, run a command) run the real app without a window, plus PNG snapshots from the CPU renderer |

## Command families (initial)

`case.*` (new, open, save, info, check, clean, export-allrun) · `workflow.*` (show, import; edits
of the description to come) · `geometry.*` (import, trees, check, repair, scale) · `select.*` (pick,
rect, set, clear) · `scene.snapshot` · `dict.set` / `dict.remove` ·
`mesh.*` (setup, block, refine, layers, check) · `patch.*` · `region.*` · `material.*` · `bc.*` ·
`control.*` (controlDict, fvSolution, fvSchemes entries) · `mesh.*` · `veg.*` · `solar.*` ·
`run.*` (export always; start, stop, status only with `runner`) · `post.*` (probe, slice, timeseries) · `view.*` (UI-only,
in `ui-egui`) · `edit.undo` / `edit.redo`.
