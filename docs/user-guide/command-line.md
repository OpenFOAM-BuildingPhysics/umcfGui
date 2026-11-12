# Command line

`umcf-cli` works on a case without the GUI. It runs the same commands as the desktop app
(`umcf-gui`). Build it with `cargo build --release -p umcf-cli`; the program is
`target/release/umcf-cli`.

## Checking a case

The examples below write `umcf-cli` for short: put `target/release` on your `PATH`, or give
the full path.

```sh
target/release/umcf-cli check path/to/case
```

This opens the case and prints every problem the [case check](checks.md) finds, errors first,
then a summary:

```text
error    workflow.all-regions         Allrun: line 46: `reconstructPar -allRegions` reads the region list from controlDict `regionSolvers`, …
warning  fv.options-ignored           system/air/fvOptions [air]: OpenFOAM-12 reads fvOptions only when there is no fvConstraints: …
1 error, 3 warnings, 2 notes
```

Each line gives the level, the code, then the file and, in brackets, the region, patch and
field.

| Option | Does |
|---|---|
| `--json` | Prints one JSON object: `errors`, `warnings`, `infos` (the counts) and `diagnostics`, a list of `{level, code, path, region, patch, field, message}` |
| `--no-fields` | Doesn't read the field files. Opening is faster, but boundary conditions are not checked: each field file gets a note saying so. A region with no field files at all is still an error (`region.no-fields`), so the status can still be 1 |

The exit status is 0 when there are no errors (warnings are allowed), 1 when there are, and 2
when the case could not be checked (it isn't a directory, or doesn't open). In a script:
`umcf-cli check case || exit 1`.

## Checking a mesh

```sh
umcf-cli mesh path/to/case [--region air] [--all-geometry] [--json]
```

This reads each region's mesh (or only `--region`'s) and runs the
[mesh quality checks](meshes.md#mesh-quality); `--all-geometry` adds those of
`checkMesh -allGeometry`. It prints the problems found, then one line per
region:

```text
warning  mesh.aspect-ratio            constant/buildings/polyMesh [buildings]: 2880 cells have an aspect ratio above 1000 (max 2046): cells 0, 1, 2, …
air: 25588 cells, non-orthogonality max 69.2°, skewness max 0.82, 0 errors, 0 warnings
```

`--json` prints, for each region:
- `quality`: the numbers (cells, faces, volumes, face areas, aspect ratio, non-orthogonality,
  skewness, openness), with the first 1000 face or cell labels of each problem and their count;
- `checkmesh`: what `checkMesh` left in the case, its `logs` (verdict, failed checks, remarks)
  and `sets` (each `-writeSets` set with its count, first 1000 labels and whether it is stale);
- `diagnostics`: the problems as above, the native checks' and `checkMesh`'s.

The top-level `diagnostics` holds notes about the case's `checkMesh` logs as a whole, such as a
log for no region of the case.

The exit status is 0 when there are no errors, 1 when there are, and 2 when the case can't be
opened. A mesh that can't be read is an error of its region (`mesh.unreadable`); the other
regions are still checked.

## Checking surfaces

```sh
umcf-cli surface path/to/case [--file walls.stl] [--json]
umcf-cli surface path/to/file.stl [--json]
```

This checks the STL surfaces in the case's `constant/triSurface` (or the one `--file` names),
or one file anywhere, for [what breaks meshing](surfaces.md#checks), and prints the problems
found, then a line with the counts. `--json` prints each surface's numbers with the triangles
of each finding. The exit status is 0 when there are no errors, 1 when there are, and 2 when
the case or file can't be opened.

## Drawing the case

```sh
umcf-cli render path/to/case --out case.png [--view top] [--size 1600x1000] [--edges]
                [--no-grid] [--no-origin] [--light]
                [--region air] [--surface walls.stl] [--select air/inlet] [--pick 640,400]
                [--cut air:x=115] [--found air] [--layers air/buildings=3]
                [--near air/buildings.stl=2] [--json]
```

This draws the case's meshes (their boundary patches) or STL surfaces into a PNG file with the
CPU renderer, over a dark background with the grid and the origin (`--light`, `--no-grid`,
`--no-origin` leave them out), and can select and pick patches first: see [3D scene and
selection](scene.md#on-the-command-line). `--cut air:x=115` cuts a region's mesh across an axis
and draws the cells the plane crosses, the region's boundary hidden ([a
cut](scene.md#a-cut-through-the-mesh)); `--found air` checks a region's mesh and draws the faces
and cells the check found, the region's boundary hidden ([what a check
found](scene.md#what-a-check-found)); `--layers air/buildings=3` draws the first 3 layers of
cells at a patch, each in its colour, the patch faces where they stop short marked ([the layers
at a wall](scene.md#the-layers-at-a-wall)); `--near air/buildings.stl=2` draws the cells with a
point within 2 m of an STL surface (a cell the surface crosses with all its points farther away
isn't one), each size in its colour ([the cells near a
surface](scene.md#the-cells-near-a-surface)). The exit status is 0 when the image is written and 2
when the case can't be opened or drawn. How many regions are read at once, and how much is kept
of hidden ones, follows the memory free, or `UMCF_MEMORY` when it is set to a whole number of
megabytes above 0 ([memory](scene.md#memory)).

## Running commands

```sh
umcf-cli run --case path/to/case --cmd 'control.set={"endTime": 7200}' --cmd case.save
```

This runs the commands in order and stops at the first that fails. Each command is given as
`<id>=<parameters>`, with the parameters as a JSON object; a command without parameters is just
its id. `--case` opens the case first (it is the command `case.open`).

Edits change the case in memory. Only `case.save` writes them to disk:

- It writes only the files that changed, and edits each one in place (comments, layout and
  `#include`s stay as they were).
- The first time a file is saved, its original is copied under
  `.umcfgui/backup/<session>/` in the case.
- If a file changed on disk since the case was opened, nothing is saved. Each file is checked
  once more right before it is replaced; a file changed in that moment stops the save, and the
  error names the files already written.

A run that ends with unsaved edits says so.

When a command fails, the case in memory is left as it was before that command: `control.set`
with several entries sets all of them or none. Files already written are not: a `case.save`
that stops part way leaves the files it wrote saved (the error names them).

| Option | Does |
|---|---|
| `--cmd '<id>=<json>'` | One command; repeat it for more |
| `--cmds <file>` | Commands from a file, one JSON object per line: `{"command": "control.set", "params": {"endTime": 7200}}`. Blank lines and lines starting with `#` are skipped |
| `--journal <file>` | When the run ends, writes the edits made to the case open at that point, in the same format. Opening a case is not recorded, and starts a new journal |
| `--json` | One JSON line per command: `{command, ok: true, message, data}`, or `{command, ok: false, message}` for the one that failed |

A journal from `--journal` holds the edits of one case, and runs again with `--cmds` on
whichever case is open: `umcf-cli run --case copy --cmds journal.jsonl` makes the same edits
to `copy`. A run that opens a second case journals only the edits made after that.

The exit status is 0 when every command ran, 1 when one failed or the journal could not be
written (the commands before may have run, and `case.save` may have written files), and 2 for
bad arguments.

## The commands

`umcf-cli commands` lists them with their parameters (`--json` for programs).

| Command | Does |
|---|---|
| `case.new` | [Makes a case](new-case.md) in a new or empty directory and opens it: `path`, `version` (`8` or `12`) and `regions` (`[{"name": "air", "kind": "fluid"}, {"name": "street", "kind": "solid", "materials": [{"name": "soil", "model": "Soil"}]}]`) required; the components `radiation` (a region), `grass` (`true` or the patches), `temperatureLimit`, `blending`; `up` (`x`, `y`, `z`); `discard`; `settings` (as `workflow.settings` takes them) |
| `case.open` | Opens a case: `path` (required), `fields` (`false` skips reading field files, `"later"` lists them for `case.fields` to read), `discard` (`true` opens even if the open case has unsaved changes) |
| `case.fields` | Reads the start-time field files of a case opened with `fields: "later"` |
| `case.save` | Writes the changed files (dictionaries, fields, surfaces, face sets, the workflow description), after a backup of each; a file a command wrote whole is refused when it is a symbolic link, checked once before anything is written ([Cases](cases.md#editing-and-saving)) |
| `case.info` | The case: OpenFOAM version, kind, regions with their patch and field counts, `processor*` directories, unsaved files, what didn't read |
| `case.check` | The [case check](checks.md) |
| `workflow.show` | The [workflow description](workflow.md#the-workflow-description): `system/umcfWorkflow` as read, with its problems, or as `workflow.import` would write it when the case has none: `scripts` (`true` also returns the scripts `workflow.scripts` would write) |
| `workflow.import` | Writes `system/umcfWorkflow` from the case's scripts, meshes and solids' materials, with notes on what it couldn't tell: `replace` (`true` writes over the case's own) |
| `region.add` | [Adds a region](new-case.md#adding-a-region) to the case: `region`, `kind` (`fluid`, `solid`, `vegetation`) required; `materials` (a solid's, `[{"name": "brick", "model": "Hamstad5Brick"}]`). Lists it where the case's version reads its regions and writes the settings of its kind; the files there are kept |
| `material.set` | [Sets a material's coefficients](new-case.md#a-materials-coefficients) in its solid's `transportProperties`: `region`, `material`, `coefficients` (`{"rho": 1700, "wcap": 2, "mu": null}`), that material only |
| `workflow.region` | [Edits the description](workflow.md#editing-the-description): `region`, and `kind` (`fluid`, `solid`, `vegetation`: adds it or gives it that kind) or `remove: true` |
| `workflow.patch` | Edits a patch of the description: `region`, `patch`, and any of `role` (`null` takes it away), `partner` (`{"region": "street", "patch": "top"}`, or `null`), `name` (renames it; the partners that named it follow) |
| `workflow.material` | Edits a solid's material in the description: `region`, `material`, and `model` (its `buildingMaterialModel`, or `null`) or `remove: true` |
| `workflow.settings` | [Chooses settings](workflow.md#settings): any of `deltaT`, `endTime`, `writeInterval`, `turbulence`, `temperatureLimit` (`[min, max]`), `blending` (`{"dampingThickness": 60, "alphaCoeffU": 0.3, "alphaCoeffT": 0.3}`), `materials` (the library: `{"HamstadConcrete": {"rho": 1950, …}}`), `numerics` (per region kind: `{"fluid": {"SIMPLE": {"nNonOrthogonalCorrectors": 1}}}`), `from` (another case, read only), into the case's files and its description |
| `workflow.component` | [Switches a component](workflow.md#components) on or off: `component` (`grass`, `temperatureLimit`, `blending`), `on` (`true`, `false`), `patches` (the grass's, of `air`), in the case's files and its description |
| `workflow.scripts` | Writes `Allprepare`, `Allrun` and `Allclean` from the description, for its OpenFOAM version ([Writing the scripts](workflow.md#writing-the-scripts)): `replace` (`true` writes over scripts that differ) |
| `geometry.import` | Copies an STL file into `constant/triSurface` ([Surfaces](surfaces.md#importing)): `path` (required), `name`, `replace`, `scale` |
| `geometry.trees` | Writes [tree crowns](surfaces.md#tree-crowns) as one surface in `constant/triSurface`: the trees from `table` (a file of the case), `path` (a file outside it) or `trees` (`[{"x": 10, "y": 5, "z": 0, "diameter": 6, "height": 8}]`); `name` (default `trees.stl`), `solid` (default `trees`), `replace` |
| `geometry.check` | The [surface checks](surfaces.md#checks): `file` (one surface of the case), `path` (a file anywhere, no case needed), or neither (every STL surface in `constant/triSurface`) |
| `geometry.repair` | The [repairs](surfaces.md#repairs) of one surface: `file` (required), `weld` (a distance in metres), `degenerate`, `orient` (`true`/`false`; without any of the three: `degenerate` and `orient`) |
| `geometry.scale` | Multiplies a surface's coordinates: `file`, `factor` (both required) |
| `scene.snapshot` | Draws the case in 3D into a PNG image (`png`, base64): `regions`, `surfaces`, `view`, `azimuth`, `elevation`, `zoom`, `width`, `height`, `edges`, `colour_by`, `reload`, `display` (layers as surfaces, wireframe, points or hidden, with an opacity: [display modes](scene.md#display-modes)), `grid`, `origin`, `dark` (each default `true`: [the grid, the origin and the dark view](desktop.md#the-grid-the-origin-and-the-dark-view)), or `camera` as it returns it ([3D scene](scene.md#snapshots)) |
| `select.pick` | Selects the patch or surface region under a `pixel` of that image, or along a `ray`, and lists everything under it: `mode` (`replace`, `add`, `toggle`, `none`), `depth`, `faces` (`true`: the mesh face under it, not its patch), and the snapshot's parameters ([Picking](scene.md#picking)) |
| `select.rect` | Selects what is in a `rect` (`[x0, y0, x1, y1]`) of that image, seen or (`through: true`) hidden too: `mode` (`replace`, `add`, `remove`, `none`), `faces`, and the snapshot's parameters ([Selecting in a rectangle](scene.md#selecting-in-a-rectangle)) |
| `select.brush` | Selects what is under a brush `stroke` (`[[x, y], ...]`) with a `radius` in pixels of that image: the mesh faces by default (`faces: false`: patches), `through`, `mode`, and the snapshot's parameters ([Selecting with a brush](scene.md#selecting-with-a-brush)) |
| `select.set`, `select.clear` | Sets the selection from `items` (`{"region", "patch"}`, `{"region", "face"}`, `{"surface", "solid"}`) with `mode` (`replace`, `add`, `remove`), or empties it |
| `patch.set` | Changes [patches](meshes.md#editing-patches) of a region's mesh: `type`, `groups` (`["wall"]`), `entries` (`{"neighbourRegion": "air"}`), `name` (one patch; its field files and the mapped patches of other regions that name it follow), `patches` (`[{"region", "patch"}]`; default: the selected patches) |
| `patch.create` | Prepares a [new patch](meshes.md#new-patches) for one run of `createPatch -dict …createPatchDict.umcf` (the line for the case's version is in the result) from boundary faces: `name` (required), `region` (default: the region of the selected patches, or of the picked faces when they are the faces), `type` (default `patch`), `groups`, `entries`, and the faces: the selected faces when none of the following is given, or one of `faces` (labels), `box` (`{"min": [x, y, z], "max": [x, y, z]}`) or `surface` (`{"file", "solid", "distance"}`); `from` (patch names, for `box` and `surface`) |
| `bc.set` | Sets the [boundary condition](fields.md#setting-boundary-conditions) of patches in a field of the start time: `field`, `type` (both required), `value` (`300` is `uniform 300`, `[2, 0, 0]` is `uniform (2 0 0)`, or OpenFOAM text), `entries` (other keys: `{"inletValue": "uniform 0"}`), `defaults` (keys only a new entry gets, such as `{"value": "uniform 300"}`: a patch keeping its entry keeps its own), `patches` (`[{"region", "patch"}]`; default: the selected patches). Returns the case check's findings for those patches |
| `mesh.check` | The [mesh quality checks](meshes.md#mesh-quality): `region` (default: every region with a mesh), `all_geometry` (`true`: also the checks of `checkMesh -allGeometry`) |
| `mesh.block` | Sets a region's [blocks](workflow.md#blocks) in the description and writes its `blockMeshDict` from them: `region` (required), any of `x`, `y`, `z` (`[0, [5, 1], 50, …]`), `missing`, `patches`, `faces`, `noFaces`, `types` (the maps merge, `null` takes an entry away); `dropPatches` (wall or constraint patches the blocks may drop), `dropEntries` (patches whose other entries may be lost), `unreadPatches` (`true`: write over a dictionary whose patches can't all be read), `dropSettings` (`true`: lose its cell zones, curved edges, merged patches, scale, other keys or expressions); refused while the region's `blocks` entry doesn't read whole, or when another region's mesh reads the same dictionary; with only `region`, writes the dictionary again; `replace` (`true` writes over one edited by hand or not written by umcfGui), `remove` (`true` takes the blocks out, and the box they were made from); gives a region without a `mesh` `mesh blocks;`; a change of the grid takes the region's box out |
| `mesh.box` | Makes a region's blocks from a [box](workflow.md#box) around STL surfaces and writes its `blockMeshDict` as `mesh.block` does: `region` (required), `surfaces` (default: the box's, else every surface in `constant/triSurface`), `margin` (`50`: 0 at the ground the gravity points to; or `[west, east, south, north, ground, top]`), `cellSize` (cubes) or `cells` (`[nx, ny, nz]`), each defaulting to the region's box; `reshape` (`true`: replace blocks not made from the box, or edited since); `patches`, `types`, `noFaces`, `replace`, `dropPatches`, `dropEntries`, `unreadPatches`, `dropSettings` as `mesh.block` takes them; `remove` (`true` takes the box out, the blocks stay) |
| `control.set` | Sets `system/controlDict` entries, by name: `{"endTime": 7200, "writeInterval": 600}` |
| `dict.set` | Sets one entry of any dictionary the case loaded: `file`, `path` (`"PIMPLE/nOuterCorrectors"`), `value`. Parent dictionaries that are missing are added |
| `dict.remove` | Removes one entry: `file`, `path` |
| `edit.undo`, `edit.redo` | Step back and forward through the edits (up to 100) |

Values are given as JSON:

- a number or `true`/`false` is written as it is;
- an array becomes an OpenFOAM list: `[0, 0, -9.81]` is `(0 0 -9.81)`;
- a string is OpenFOAM text, written as given: `"uniform 300"`, `"Gauss linear"`, or a
  dictionary such as `"{ type fixedValue; value uniform 300; }"`.

A value can only change its own entry. Text that would end the entry early is refused, such as
a `;` or anything after a dictionary's closing `}`. A comment in a value is dropped. Keys are
plain OpenFOAM words (`endTime`, `div(phi,U)`) or patterns, given without their quotes
(`(U|k)` reaches `"(U|k)"`, and a new one starting with `(` or holding `|` or `*`, such as
`U.*`, is written quoted): directives (`#include`), macros (`$x`) and keys
with unbalanced brackets are refused. So are unknown parameter names, which catches typos.
