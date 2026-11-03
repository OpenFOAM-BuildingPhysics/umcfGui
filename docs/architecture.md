# umcfGui architecture

An OpenFOAM case editor, runner and viewer for urbanMicroclimateFoam, built as strictly layered
Rust crates.

!!! info "Status"
    The crates that exist today are `core`, `foam-dict`, `foam-field` and `schema`, plus the
    `xtask` tooling.
    The others in the table below are planned.

## Core patterns

| Pattern | How it works here |
|---|---|
| Strict layers checked by a tool | `cargo xtask layers` reads `cargo metadata`. A crate may depend only on lower layers, plus explicit chains within a layer (table below) |
| UI quarantine | UI crates (`egui*`, `eframe`, `winit`, `rfd`, `wgpu*`) only at L6+. `wgpu` only if the 3D view needs it, and only in `ui-egui` |
| Format crates kept narrow | `foam-*` depend only on `core` and `geom`. They know OpenFOAM syntax, not UMCF |
| One command registry | `CommandSpec { id, label, menu, shortcut, params, enabled, run, journal, undoable }`. `enabled` returns `Err(reason)`, which the UI shows as a tooltip |
| Session as the single entry point | `Session::execute(id, Json)` with a panic guard, an undo snapshot and a journal. The case model is `Arc`-cloned for cheap undo snapshots |
| Headless parity | `execute_headless` rejects commands that would only open a dialog. The CLI and MCP use it |
| Control channel | JSON lines over TCP: `engine.execute`, `ui.screenshot`, `ui.inspect`, plus `run.events` (a stream of job progress) |
| Agent control | `mcp` crate: tools = commands; `--connect` drives the running app |
| Offscreen snapshot | `cargo run -p umcf-ui-egui --example snapshot`. This is how agents check UI work |
| Fuzzing invariants | Every command survives empty params. Random command sequences are checked against undo/redo and "save → reload → identical model" |
| `xtask` as the only tooling | `ci`, `layers`, `assets`, `catalog`, `wasm` |

## Layer table

```
L0  core        ids, units (SI + typed quantities), errors, case paths
    geom        Vec3, bbox, transforms, STL/OBJ triSurface I/O
L1  foam-dict   lossless FoamFile dictionary CST: parse, edit-in-place, write
    foam-mesh   polyMesh (points/faces/owner/neighbour/boundary), ASCII + binary, cellZones
    foam-field  vol*/surface* fields, uniform/nonuniform lists, time dirs, processor* dirs
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

Chains within a layer: `foam-dict → foam-mesh → foam-field`, `schema → model`, `post → render`.

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
- A future `remote` runner (ssh + scheduler) would plug into the same `Pipeline`. It is not
  planned yet.

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

## Command families (initial)

`case.*` (new, open, save, clean, export-allrun) · `region.*` · `material.*` · `bc.*` ·
`control.*` (controlDict, fvSolution, fvSchemes entries) · `mesh.*` · `veg.*` · `solar.*` ·
`run.*` (export always; start, stop, status only with `runner`) · `post.*` (probe, slice, timeseries) · `view.*` (UI-only,
in `ui-egui`) · `edit.undo` / `edit.redo`.
