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
| A memory budget | `Session::budget` (`Budget`: from the memory free when the first session starts, as Linux, Windows or macOS tell it through `sysinfo`, on Linux no more than the cgroup v2 groups from `/proc/self/cgroup` up leave (resolved through the cgroup2 mount in `/proc/self/mountinfo`), or `UMCF_MEMORY` in MB): half for what is read at once (the 3D scene's readers start, biggest first, only while their estimates fit; one always runs), a quarter for the scene objects kept while hidden (the ones shown least recently dropped first). Estimates come from the `owner` header's counts or the file sizes (`CaseFs::size`) |
| One command registry | `CommandSpec { id, label, menu, shortcut, params, enabled, run, journal, undoable, reads_only }`. `enabled` returns `Err(reason)`, which the UI shows as a tooltip; `reads_only` marks the commands that can be cancelled |
| Session as the single entry point | `Session::execute(id, Json)` with a panic guard, an undo snapshot and a journal. The case model is `Arc`-cloned for cheap undo snapshots |
| Long work as jobs | The desktop app moves the `Session` to a thread of its own for long work and draws meanwhile from `Session::stand_in`, a copy that reads no file (case, history, selection, and the 3D scene's cache, shared through an `Arc`); opening and saving take the whole window. Work asked meanwhile waits in a queue (a scene only queued gives way, and the newest scene asked takes its place). `Session::canceller` gives the job's flag and its `Progress` (bytes read through `Cancellable` of those the job expects, and steps where the job knows them: the field files, the scene's objects, the faces the mesh check has checked): a case `case.open` opened reads through `Cancellable`, whose reads fail once the flag is set, and a command marked `reads_only` fails as `Error::Cancelled` with the case restored; one that writes runs to its end. The desktop app opens a case with `fields: "later"` and reads them with `case.fields`. The CLI and tests call `Session::execute` as before |
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
    foam-field  vol*/surface* fields, uniform/nonuniform lists, gzip, time dirs, processor* dirs;
                a field's outline read as a stream (Field::outline: lists by kind and length)
    foam-mesh   polyMesh (points/faces/owner/neighbour/boundary, cellZones), ASCII + binary;
                the boundary alone (BoundaryMesh) for drawing and picking; ASCII points
                (and the boundary's faces) parsed as a stream (CaseFs::reader)
L2  schema      catalog of solver types and keywords (generated in part by xtask catalog); the
                one configuration a new case's settings come from (defaults: the tutorials')
    model       typed Case built on foam-dict nodes: regions, materials, BCs, controls, sun/weather;
                the workflow description, its edits, the settings a case chooses (settings),
                a region's grid of blocks and its blockMeshDict, written and imported (blocks),
                a new case's files (new_case), a region added to a case (region_add)
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
- OpenFOAM runs only on Linux. The runner (planned, row 11 of [the gaps](gaps.md)) is to run
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

   Every problem carries the faces or cells involved. The session keeps them per region
   (`render::Found`, the first 1000 of each problem and of `checkMesh`'s sets), with the whole
   mesh the check read handed to the 3D scene within the budget for hidden objects; the scene
   draws them as the region's cut (`render::Cells::Found`, `Object::from_found`).

   The 3D view also shows the layers of cells at a wall, apart from the check: the region's cut
   (`render::Cells::Layers`, `Object::from_layers`), walked from each patch face through the
   opposite faces, the patch faces where they stop short marked, and the cells near an STL surface
   by size (`render::Cells::Near`, `Object::from_near`, with `geom::near`), so the refinement
   around buildings and trees shows. They are views of the mesh, not findings of `mesh.check`.
5. **Assign boundary conditions in 3D** (`bc.set`): picking in the view selects patches (or
   groups, or a region's whole boundary). `bc.set` takes that selection as its parameters, so
   the same assignment is a CLI or MCP call too. Picking faces to create a new patch
   (`patch.create`, via `createPatch`/`topoSet`) is the same flow.

### The 3D view

The GUI's 3D view borrows proven patterns from
[SolidCraft](https://github.com/tangency-labs/solidcraft) (MIT/Apache-2.0, also a pure-Rust,
egui, command-driven app). Code ported from it keeps its copyright notice and gets a row in
`ATTRIBUTION.md`. Its UI kit (ribbon, menus, bars, theme, icons) has been under a separate brand
licence since 2026-11-10 that doesn't allow reuse elsewhere: nothing is taken from it. The
ribbon's fitting to a narrow window (groups compact then collapsed into drop-downs, the tabs'
» menu, the bar-less scrolling row and its edge chevrons) is ported instead from
[WordCraft](https://github.com/storytold/wordcraft) (MIT/Apache-2.0, a pure-Rust, egui word
processor): `ui-egui/src/ribbon_fit.rs`, and `App::group` and `App::collapsed_group` after its group drawing.

| Pattern | Here |
|---|---|
| Renderer-agnostic scene in a low layer | `render::Scene` (meshes, per-face colour classes, opacity) is built from `foam-mesh` and `geom` data and stays wasm-clean. The GPU view in `ui-egui` (wgpu: `gpu.rs`; transparent layers: the nearest on each pixel exact, the rest weighted and blended, order-independent, where the device blends half floats) and the CPU rasteriser (headless snapshots, CI, remote machines without a GPU, and what the GPU view can't draw on a device: transparent layers without half-float blending, a region too big for a buffer) draw the same scene. What isn't the case's (the grid on the ground, the origin's planes and axes, a cut's plane with its frame and arrow while it is moved) is made once by `render::guides` as lines and squares, which both draw. The GPU view uploads each object once (its points, triangles' corners and classes, read by the vertex shader from storage buffers) and keeps it by `Object::id`, with a region's edges and points (`Object::wire`) once a wireframe or points layer shows them; the selection and the display modes are a colour, a mode and an opacity per class, and the faces' edges are found per pixel from the triangle's corners on the image, as the CPU does. It draws into a texture egui shows, so picking (on the CPU BVH) and screenshots don't change. The window's backend is chosen once, at the start: wgpu when the probe finds a device that can draw the view (a hardware one, unless `--renderer gpu`), the view starting on the CPU unless the GPU is asked for, else, and with `--renderer cpu`, OpenGL (glow) with the CPU view. On wgpu, the GPU view can still fall back to the CPU while the window stays on wgpu: when it can't be built on the window's device (or, with `auto` or no `--renderer`, the window's device is a software or virtual one), and for the rest of the session when the device reports an error (error scopes, an uncaptured-error handler, a device-lost callback) |
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
