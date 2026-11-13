# The case workflow

An urbanMicroclimateFoam case is prepared and run by the same sequence of steps every time.
Cases spell the sequence out in shell scripts (`Allprepare`, `Allrun`, `Allclean`) that are
copied from case to case and edited by hand. umcfGui reads those scripts into a **workflow**:
the sequence with its few choices. It checks the workflow and can write the scripts again
for the case's OpenFOAM version.

## The pattern

**Prepare** (`Allprepare`), region by region:

| Region | Steps |
|---|---|
| Fluid or vegetation, meshed with snappyHexMesh | `surfaceFeatures` (once, for all regions); `blockMesh` from the region's variant `system/blockMeshDict.<r>`; `createPatch`; `snappyHexMesh -overwrite` from `system/snappyHexMeshDict.<r>`; move `constant/polyMesh` to `constant/<r>/polyMesh`. Case scripts copy the variants over `system/blockMeshDict`; the written scripts pass them with `-dict` |
| Solid, meshed with blockMesh | `blockMesh -region <r>` from `system/<r>/blockMeshDict` |
| Then, any region | `createPatch -region`, `changeDictionary -region`, `topoSet -region` (`setSet -batch` in OpenFOAM-8), and `setFields -region` after restoring the field it sets from its backup (`0/air.bckp/LAD`) |

**Run** (`Allrun`): decompose every region; copy the region dictionaries into each
processor directory; the radiation pre-processing of the vegetation region (`faceAgglomerate`,
`calcLAI` on the air, `viewFactorsGen`, `solarRayTracingGen`); the solver; reconstruct every
region.

**Clean** (`Allclean`): `cleanCase`, the region meshes, and the radiation pre-processing
outputs of a serial run.

## Reading a case's scripts

The scripts are read without running anything: commands, `for` loops over words, variables
set to words, `if` blocks (their steps may not run), `mpirun`/`srun`, `#SBATCH`/`#PBS`
lines, and the OpenFOAM `RunFunctions` and `CleanFunctions` helpers. A word that can't be known
without running the script (`$(…)`, a variable set from a command, a `cd` elsewhere) is kept
and marked; the check doesn't guess it.

Scripts are case files, read every time a case opens, so their reading is bounded: blocks
nested more than 32 deep are not read, and a script stops being read after a million
commands (loops count each pass) or 100 000 steps. Such a script gets a note, and is
checked only as far as it was read.

Steps that don't fit the pattern are listed (`workflow.custom`), as are steps that fit but
aren't needed (`workflow.not-needed`, for example a serial `faceAgglomerate` before the
parallel one).

## Writing the scripts

The scripts umcfGui writes follow the pattern, with what each OpenFOAM version needs:

| | OpenFOAM-8 | OpenFOAM-12 |
|---|---|---|
| Sets in solid regions | `setSet -batch system/<r>/setset.batch` | `topoSet -region <r>` (`system/<r>/topoSetDict`): OpenFOAM-12 has no `setSet` |
| `faceAgglomerate` dictionary | `constant/<r>/viewFactorsDict` | `system/<r>/viewFactorsDict`, passed with `-dict` (on its own it reads `system/viewFactorsDict`) |
| `viewFactorsGen`, `solarRayTracingGen` | `constant/<r>/viewFactorsDict` | `system/<r>/viewFactorsDict` |

Also, in both versions:
- regions are decomposed and reconstructed one by one with `-region`, so OpenFOAM-12 needs no
  `regionSolvers` in `controlDict`;
- no process count is written into the scripts. OpenFOAM-12 parallel steps use
  `runParallel`, which takes it from `system/decomposeParDict`. OpenFOAM-8 decomposes each
  region by its own `system/<r>/decomposeParDict`, and a case may have no
  `system/decomposeParDict` (which `runParallel` always reads): there the count is read from
  the first region's and the steps call `mpirun` themselves;
- each step logs to `log.<application>.<region>` and overwrites its log (with `runApplication`
  a step whose log exists is otherwise skipped, silently);
- the scripts stop at the first step that fails;
- the region dictionaries are copied into each processor directory. urbanMicroclimateFoam
  built before June 2026 (solver commit `40f31d4`) looks for `vegetationProperties` and the
  like in `processor*/constant/<region>/`; without them it builds no vegetation model and
  stops at the first time step (*request for volScalarField Tl from objectRegistry air
  failed*). Later builds read them from `constant/`. The copies work with both;
- `Allclean` removes the files exactly, by name (no `F*`), and only the meshes `Allprepare`
  builds;
- names and paths are quoted for the shell when they need it. Since they come from the case,
  no script is written with a region, field or file name that isn't a plain name (`/`, `..`,
  a control character, a leading `-`) or a dictionary path outside the case. A region
  directory that is a symbolic link out of the case is not caught: the scripts' `rm` and `mv`
  would follow it.

A region [split](#split) from another's mesh by a cell zone is made with `-overwrite`, which
writes the parts into `constant/` in both versions, not into a time directory that depends on
the time step. OpenFOAM-8 reads a `-dict` given with `-region` below the region's own directory
(`system/air/topoSetDict` as `system/<r>/air/topoSetDict`), so its path goes up one there
(`system/../air/topoSetDict`).

A mesh built in `constant/polyMesh` keeps its steps in the order the case's script ran them
(`createPatch` before or after `snappyHexMesh`); a variant copied into `system/` or passed
with `-dict` is the same dictionary. A region step with its own
`-dict`, or a field restored without a `setFields` after it, doesn't fit and is listed.

Run on the 3×3 test case (OpenFOAM-12, two processes), the written scripts mesh, pre-process
and run the case through to its first written time.

`workflow.scripts` writes them from the case's [workflow description](#the-workflow-description),
for its OpenFOAM version (the case's when it names none): every region's mesh and steps, the
radiation pre-processing and the solver as the description has them, and the dictionaries Allrun
copies into `processor*/` from the case. `case.save` writes them executable (on Linux and macOS;
Windows has no such flag, and the scripts run through WSL, a virtual machine or a remote
machine), after a backup of each one there in `.umcfgui/backup/`.

```sh
umcf-cli run --case path/to/case --cmd 'workflow.import={}' \
  --cmd 'workflow.scripts={"replace":true}' --cmd 'case.save={}'
```

- Scripts that are there and differ are written over only with `replace: true`; ones that are
  as the description has them already are left alone. A script that is a symbolic link isn't
  written over.
- `workflow.show` with `scripts: true` returns the three scripts without writing them (or why
  none can be written).
- The case's checks and `workflow.import` read them as written, before they are saved.
- A description with problems is refused (what can't be read would be left out of the
  scripts; `workflow.show` lists them), and so is a name that isn't safe in a script.

The `workflow` example of the `umcf-model` crate
(`cargo run -p umcf-model --example workflow -- <case> [--write]`) still writes them from the
case's own scripts, read as above, without a description.

## The workflow description

The workflow is also kept in the case itself, as `system/umcfWorkflow`: an OpenFOAM-style
dictionary that names each region with its kind, how its mesh is built and the steps after it,
each patch with its role and partner, the radiation pre-processing and the solver. It is the
first piece of [the common workflow](../common-workflow.md): the scripts are written from it
(`workflow.scripts`, [above](#writing-the-scripts)), and the dictionaries are to be.

```c++
version     12;

regions
{
    air
    {
        kind        fluid;
        mesh        ( blockMesh "system/blockMeshDict.air" createPatch snappyHexMesh "system/snappyHexMeshDict.air" );
        steps       ( changeDictionary topoSet setFields ( ( "0/air.bckp/LAD" LAD ) ) );
        patches
        {
            west            { role inlet; }
            street          { role coupled; partner ( street street ); }
        }
    }
    street
    {
        kind        solid;
        mesh        blocks;
        steps       ( createPatch changeDictionary topoSet );
        patches
        {
            street          { role coupled; partner ( air street ); }
            interior        { role interior; }
        }
        materials
        {
            soil            { model Soil; }
            brick           { model Hamstad5Brick; }
        }
    }
}

features    no;
radiation   { region vegetation; lai air; viewFactors yes; solarRayTracing yes; }
components  ( grass temperatureLimit );
solver      urbanMicroclimateFoam;
reconstruct yes;

settings
{
    deltaT           1800;
    turbulence       realizableKE;
    temperatureLimit ( 283.15 370.15 );
}
```

| Key | What |
|---|---|
| `version` | `8` or `12`: the OpenFOAM version the scripts are for |
| `profile` | written by an earlier `case.new`: read and left out (there is [one configuration](new-case.md#the-configuration)) |
| `kind` | `fluid`, `solid` or `vegetation` |
| `mesh` | `blocks` (`blockMesh -region`), the steps that build it in `constant/polyMesh` before it is moved to the region (`blockMesh`, `createPatch`, `snappyHexMesh`, each meshing step with its dictionary when it isn't the default one), or `{ from split; of air; zone trees; }`: [split](#split) from another region's mesh by a cell zone |
| `blocks` | the grid of blocks the region's `blockMeshDict` is written from ([below](#blocks)) |
| `box` | the box around the STL surfaces its blocks are made from ([below](#box)) |
| `snappy` | the STL surfaces snappyHexMesh makes its mesh with, refinement boxes and the inside point ([below](#snappy)) |
| `steps` | after the mesh: `createPatch`, `changeDictionary`, `topoSet` (or `setSet "batch"` in OpenFOAM-8), `setFields` with the fields it restores from their backups first |
| `role` | `inlet`, `outlet`, `side`, `top`, `ground`, `coupled`, `canopy`, `interior`, `bottom`, `sky` |
| `partner` | the region and patch a coupled patch is mapped to |
| `materials` | a solid region's materials, in the order of its `buildingMaterials`: each the name of the cell zone built of it, with its `model` (`buildingMaterialModel`); the coefficients stay in the region's `transportProperties` |
| `radiation` | the region the view factors and solar ray tracing are computed on, and the one `calcLAI` runs on |
| `settings` | the [settings](#settings) the case chose; one that isn't there is the default |
| `components` | the [components](new-case.md#components) the regions don't tell: `grass`, `temperatureLimit`, `blending` |

`workflow.import` writes it from the case: the scripts read as above, the region kinds, the
components its files switch on (a fluid's `grassProperties` whose `grassModel` isn't `none` and whose `grass` isn't off, a `limitTemperature` entry in a
fluid's file the solver reads it from ([below](#components)), `blending` on in `controlDict`),
the [settings](#settings) its files have, each patch's role, and each solid's materials as its `constant/<region>/transportProperties`
lists them (a material or its model given by a macro or a directive is noted and left out, and
a name listed more than once keeps its first entry). Coupled patches and their partners come from the mesh
(`neighbourRegion`/`neighbourPatch`, or `sampleRegion`/`samplePatch` in OpenFOAM-8); the canopy
is the vegetation patch that maps to the air's cells; the air's inlet, outlet and sides come
from the patch's own velocity condition, not one given by a group or a pattern
(`atmBoundaryLayerInletVelocity`, `inletOutlet`, `slip`); `top`, `ground`, `interior`, `bottom`
and `sides` from their names; the vegetation region's patches
take their air twin's role, the open sides and the top becoming `sky`. What it can't tell is
listed and left without a role. Each region's [blocks](#blocks) are read from the
`blockMeshDict` its mesh reads, when that is a grid of blocks; the import writes nothing but the
description. On the 3×3 test case every patch gets its role. A region
[split](#split) from another (the street canyon's vegetation) is read with its zone.

`workflow.show` returns the description, read from the file or, when the case has none, as the
import would write it; it also says when a file is there that doesn't load (not a dictionary, a
directory, or over the size limit), or one the open case doesn't hold (written or undone since it was opened).
`workflow.import` writes over neither, nor a symbolic link (to a dictionary, a field or
nothing: the save would write the file it points to); with `replace` it writes over the case's own, whose
comments and hand edits are not kept (`edit.undo` brings them back). In the desktop app, both
commands show in the results card the problems and the import's notes first, then the settings
(version, radiation, components, solver), then the regions with their meshes, steps, patch roles and
materials. The case
check reads the file against the case (`workflow.description`): what can't be read in it, regions
and patches the case doesn't have or it leaves out, other kinds, partners that aren't there, a
solid's materials the case doesn't list (or lists with another model), a model the solver
doesn't have for the case's version (one the case's file has too is the case check's to say),
materials of the case it leaves out, and another OpenFOAM version.


### Blocks

A region's `blocks` say what its `blockMeshDict` holds, and `mesh` still says how the mesh is
built: with `mesh blocks;` the blocks are written to `system/<region>/blockMeshDict`, and with
steps in `constant/polyMesh` to the dictionary its `blockMesh` step names
(`system/blockMeshDict.air` above; `system/blockMeshDict` by default). A plain box is the same
with one span along each axis.

```c++
air
{
    kind        fluid;
    mesh        blocks;
    blocks
    {
        x       ( 0 (5 1) 50 (3 1) 60 (3 1) 70 (3 1) 80 (6 1) 230 );
        y       ( 0 (6 1) 100 (5 1) 150 (6 ((0.5 0.5 13.9) (0.5 0.5 0.072))) 250 );
        z       ( 0 (3 1) 10 (5 1) 60 );
        missing ( (1 1 0) (3 1 0) );
        patches { west inlet; east outlet; south side1; north side2; ground ground; top top; missing defaultFaces; }
        faces   { air_to_street ( (2 1 0 ground) ); air_to_windward ( (2 1 0 east) ); }
        noFaces ( street );
        types   { side1 slip; side2 slip; }
    }
    …
}
```

| Key | What |
|---|---|
| `x`, `y`, `z` | the stations along the axis, and between each two `(cells grading)`: the grading is the ratio of the last cell to the first, or blockMesh's sections `((fraction cells-fraction expansion) …)`. Every span has its cells and grading; numbers are written back with the same value (`0.00739296596393882` stays so; `60.00` becomes `60`, and one whose plain text would be longer than 24 characters is written with an exponent, `1e-300`), which OpenFOAM reads as the same number |
| `missing` | the blocks `(i j k)` left out, counted from 0 along x, y and z: the buildings of the street canyon and the 3×3 cases |
| `patches` | each side's patch: `west`/`east` are x min and max, `south`/`north` y min and max, `ground`/`top` z min and max, whatever the case's gravity (the OpenFOAM-8 street canyon, whose height is along y, has its ground at `south`); `missing` the faces around the missing blocks. `defaultFaces` leaves faces to blockMesh's own patch, of type `empty` unless `types` gives it another (the 3×3 cases leave their buildings' faces so, for a `createPatch` to rename) |
| `faces` | single faces of blocks in another patch than their side's, `(i j k side)`: the canyon's floor and walls |
| `noFaces` | patches the dictionary declares without faces, for later steps to fill |
| `types` | patch types given explicitly (`slip`, `symmetryPlane`, …), over what the roles give |

A patch's blockMesh type is the one `types` gives it; else the one its role in the region's
`patches` gives: `wall` for `ground`, `coupled`, `interior` and `bottom` in a fluid or vegetation
region, `patch` for the others, and `patch` for every patch of a solid (as the reference cases
have them; `changeDictionary` makes the coupled ones `mappedWall`); else (no role) `patch`. The
import fills `types` for every patch whose type in the dictionary isn't the one its role (or no
role) gives, so a dictionary is written again with the types it had: the OpenFOAM-8 street
canyon's `slip` sides and top, a `symmetryPlane`, the examples' walls (they carry no mesh, so
their patches have no role).

`mesh.block` sets a region's blocks and writes its `blockMeshDict` from them. Its parameters are
the region, and any part of the grid as JSON (a region without blocks needs `x`, `y` and `z`).
The axes, `missing` and `noFaces` replace the region's; the maps `patches`, `faces` and `types`
merge into them: a key given replaces its entry, `null` takes it away, and the others stay (so
giving one type keeps the others). A new entry goes after the others, in the order of its name
(a JSON object's order isn't kept).

```sh
umcf-cli run --case path/to/case \
  --cmd 'mesh.block={"region":"air","x":[0,[5,1],50,[3,1],60,[3,1],70,[3,1],80,[12,0.5],230],"replace":true}' \
  --cmd 'case.save={}'
```

| Parameter | What |
|---|---|
| `region` | the region of `system/umcfWorkflow` (required) |
| `x`, `y`, `z` | `[0, [5, 1], 50, [10, [[0.5, 0.5, 13.9], [0.5, 0.5, 0.072]]], 230]`: stations, with `[cells, grading]` between |
| `missing` | `[[1, 1, 0], [3, 1, 0]]` |
| `patches` | `{"west": "inlet", …, "missing": "defaultFaces"}` |
| `faces` | `{"air_to_street": [[2, 1, 0, "ground"]]}` |
| `noFaces` | `["street"]` |
| `types` | `{"side1": "symmetryPlane", "top": null}` |
| `dropPatches` | `["air_to_street"]`: patches of the dictionary there, of type `wall` or a constraint type, that the blocks may drop |
| `dropEntries` | `["side1"]`: patches of the dictionary there whose entries besides `type` and `faces` (`neighbourPatch`, `inGroups`, …) may be lost |
| `unreadPatches` | `true` to write over a dictionary whose patches can't all be read (an `#include`, a macro) |
| `dropSettings` | `true` to lose what the dictionary there holds besides its patches (cell zones, curved edges, a scale, other keys) |
| `replace` | `true` to write over a `blockMeshDict` umcfGui didn't write, or one edited since |
| `remove` | `true` takes the blocks out of the description; the dictionary stays, the case's own |

With only `region`, the dictionary is written again from the blocks there. A region without a
`mesh` (as [`case.new`](new-case.md) makes them) gets `mesh blocks;`, its dictionary
`system/<region>/blockMeshDict`, and the outcome says so. A call that changes the grid (its
stations, cells, gradings, missing blocks or single faces) of a region made from a [box](#box)
takes the box out of the description, and says so; `remove` takes the box out with the blocks. A grid with problems
(stations that don't increase, a span without cells or with a section blockMesh would give none, a
missing block outside the grid or listed twice, a `cyclic`, `nonConformal`, `mapped` or `processor`
type, a whole layer of missing blocks, a face without a patch, a single face inside the mesh, more
than 20 000 blocks, more than 10 000 names in `faces`, `noFaces` or `types` (or in `dropPatches`,
`dropEntries`), more than 40 000 missing blocks or 120 000 single faces, a dictionary too large to
read back) is refused and nothing changes. The blocks go into the
description key by key, only those that change, so the comments around them stay (those inside a
key that changes are not kept: the key is written again on one line). The dictionary is written
whole, generated: its header says so (`Written by umcfGui from the case workflow; edit the
workflow, not this file.`) with a checksum of the rest. One without that header, or whose
checksum no longer matches (edited by hand), is written over only with `replace`, and
`case.save` keeps it in the backup (`.umcfgui/backup/`); the outcome says so. `edit.undo` takes
the blocks and the dictionary back together. The vertices are those of the whole grid, x first,
then y, then z; each block is a hex of one cell of it, its vertices in blockMesh's order
(right-handed), and each boundary face is listed pointing out of its block. Faces left to
`defaultFaces` get a `defaultPatch { name defaultFaces; type empty; }` (its type from `types`
when given), which is blockMesh's
default said out loud.

The patches are listed in the dictionary in the grid's order: the sides' (west, east, south,
north, ground, top, missing, as `patches` has them), then those of single faces (in the order of
`faces`), then those without faces (`noFaces`).

What the dictionary it replaces makes is read before it is written over, and nothing of it is
lost unsaid but its comments, layout and banner (that file is in the backup once the case is
saved). Its patches are read whole: those of `boundary`, or of the older `patches ( type name
( faces ) … )` (which blockMesh reads instead when both are there; a note says so), and its
default patch when blockMesh leaves faces to it: the faces of its hexes no other hex shares (by
their vertex labels, as blockMesh's topology matches them) that no patch lists, whether the
dictionary is a grid or not (the 3×3 cases' `buildings`, say). That patch is its
`defaultPatch`, `defaultFaces` of type `empty` unless it names or types it otherwise (without a
type, `empty`, as OpenFOAM-8 takes it; OpenFOAM-12 asks for one; a note says so). Besides its
patches, it is read for what the blocks don't hold: cell zones named by blocks (a solid's
materials are such zones), curved `edges`, projected `faces` and `geometry`,
`mergePatchPairs`, a `convertToMeters` or `scale` other than 1, and any other key at its top
(`fastMerge`, `checkBlockFaceOrientation`, `namedVertices`, a variable of its own), a key given
twice, expressions in its vertices or blocks (`$macros`, `#calc`); blocks that can't all be read
for their cell zones are said so. The outcome lists each patch
whose type changes (`inlet patch → wall`, after a role was given, say; `typesChanged` in its
data), each patch the blocks drop or add (`patchesRemoved`, `patchesAdded`), the entries besides
`type` and `faces` that are lost (`entriesLost`) and the rest it held (`settingsLost`). The call
is refused, and nothing changes, when:

- a `wall` (the fields' wall functions need one) or a constraint type (`symmetryPlane`,
  `symmetry`, `empty`, `wedge`, `cyclic…`, `processor…`, and OpenFOAM-12's `nonConformalCyclic`,
  `nonConformalError` and `nonConformalProcessorCyclic`, which the fields' conditions are of)
  would change, unless this call's `types` gives the patch its new type;
- a patch of such a type would be dropped, unless `dropPatches` names it (a patch renamed in the
  description reads as one dropped and one added, and the refusal says so);
- a patch's other entries (`neighbourPatch`, `inGroups`, `sampleMode`, …), which the blocks
  don't hold, would be lost, unless `dropEntries` names the patch;
- it holds cell zones, curved edges, projections, merged patches, a scale or other keys, unless
  `dropSettings` is `true`;
- the region's `blocks` entry in the description doesn't read whole (a value it can't read, or an
  `#include`, a `$macro` or a value without a key in it or in its `patches`, `faces` or `types`:
  the parts not read would be taken for absent and lost): it is mended by hand first, or taken
  out with `remove`; `workflow.patch` renaming a patch of the region is refused then too;
- another region's mesh reads the same dictionary (it would change that region's mesh too; the
  import notes such a dictionary);
- the dictionary's patches can't all be read (an `#include` or a macro at its top or in a patch,
  a type given by a macro, a list of another shape, a name given twice, a block that isn't a
  hex, one point under two vertex labels, so that the faces left to its default patch can't be
  told), unless `unreadPatches` is `true`.

The default patch is typed like the others: `types { defaultFaces patch; }` writes
`defaultPatch { name defaultFaces; type patch; }` (a snappyHexMesh background's open sides, say),
and the import fills it from the dictionary's `defaultPatch`. A `defaultPatch` named otherwise
(`floor`) is read as a patch of that name, its faces the sides' or single faces like any other's,
and written so: the same faces in the same patch, listed in `boundary`.

A `cyclic…`, `nonConformal…`, `mapped…` or `processor…` type in `types` is a problem of the grid:
it needs entries the blocks don't hold, so it is given after blockMesh (`changeDictionary`, `createPatch`). The grid's size is bounded
before anything is written: 20 000 blocks, 10 000 stations along an axis (9 999 spans; the
description, the parameters and the import take as many), 10 000 names in each of `faces`,
`noFaces` and `types`, 1 000 sections in a
grading, and a dictionary that reads back: 48 MB and 3 000 000 tokens (as the dictionary
parser counts them, whitespace included: about 20 000 plain blocks), and no larger than the
case reads a dictionary (4 MB by default). In the desktop app it is *Block mesh* in the *More*
group of the *Mesh* tab: a [form](desktop.md#forms) of the region (starting on the first with
blocks), a table for each axis (a row per station, with the cells and the grading to the next:
a ratio, or sections `fraction cells-fraction expansion; …`; *+ Station* and *−* add and take
rows away; an axis of more than 200 stations is edited as text, a row a line), the missing
blocks (ticked on a layer of the grid, x across and y up, for grids of up to 40 blocks along x
and y; and listed, a block `i j k` a line), each side's patch (empty: none), *Write over a
blockMeshDict edited by hand* and *Take the blocks out of the description*. It is filled from
the region's blocks (choosing another region fills it from that one's, what was typed going;
*Back to the form* from *Edit as JSON* fills it again from the blocks of the region the JSON
names, as they are then, with the JSON over it; after
another command, the fields not typed in follow the region's grid as it is then); what is
left as it was filled is left out of the parameters (the region
keeps it), so *OK* sends only the axes, missing blocks and sides changed (a side emptied as
`null`). Single faces, patches without faces and types are kept from *Edit as JSON*. While it
is open, the 3D view previews the grid the form makes (its stations strong, its cells' lines
faint where the grading puts them, at most about 120 along an axis, and the missing blocks by
their edges, the first 2000; on the GPU and the CPU alike), on its own when the browser shows
nothing (a case without a mesh or surfaces), and the card says its blocks and cells, and when
*OK* takes the region's box out; what keeps it from being made (a value the command refuses, a
problem of the grid) is said there instead.

`workflow.import` reads a region's blocks from its `blockMeshDict` when that is a grid: its
blocks' vertices on stations along x, y and z, each block one cell of that grid (its vertices
in any of blockMesh's right-handed orders), each span with the same cells and grading in every
block (an `edgeGrading` the same along each direction counts as a `simpleGrading`), a face of a
patch on the boundary of the blocks, `convertToMeters` 1 and no curved edges, `#include`,
`$macro` or key the blocks don't hold, and no point under two vertex labels in its blocks
(blockMesh joins blocks by their labels, so it keeps a baffle there the grid wouldn't). A block
numbered against an axis has its grading turned
round: a ratio inverted, sections in the other order with their expansions inverted. Sections
are turned round only when blockMesh shares the span's cells among them alike from either end
(OpenFOAM's `lineDivide` rounds each section's share and gives what is lacking to the first of
the largest share, so tied sections would get their cells otherwise; umcfGui follows the same
rule, checked against blockMesh's own points by the opt-in test), and only when they are
positive numbers; else the dictionary isn't read as a grid, and the note says why. A grid whose
sections blockMesh would share so that one gets no cells (0.9, 0.05 and 0.05 of 3 cells) is a
problem: more cells, or fewer sections. Each side's patch is
the one most of its faces are in, the others' faces listed in `faces`; the types go into
`types` as above. A region without a mesh gets the patches its blocks name, without a role, in
its `patches`. A dictionary that isn't such a grid is left as it is and named in the import's
notes, with why. The examples' dictionaries and the 3×3 cases' fluid, vegetation and street
regions are grids; the 3×3 cases' `buildings` (blocks spanning several stations) and `roofs`
(separate boxes) are not. Written again from the grid, each makes the same mesh in blockMesh, on
OpenFOAM-12 and -8 (an opt-in test, `UMCF_SOLVER=1`, on the examples, the 3×3 cases' meshes of
up to a million cells and a block numbered against its axes with sections): the same numbers of
points, faces and cells, the same points within a relative 1e-9 of the domain's size (a grading
turned round is computed from the other end, so its points can differ by rounding), and the
same patches with their types and sizes. The written
dictionary isn't the same text: its patches are listed in the grid's order (the sides', then
the single faces', then those without faces), so the faces are numbered otherwise (other
`startFace`s, owners and neighbours), and the comments and banner of the dictionary it replaces
are not kept (that one is in the backup once the case is saved).

The case check (`workflow.blocks`) reads each region's blocks: the grid's problems, a patch the
region's `patches` don't list, a patch on a side its role doesn't fit (an inlet on top, say; a
side's patch counts there only where it still has a face once `faces` took theirs; the ground
and top are the sides across the gravity of `constant/<region>/g`, else `constant/g`; without
either, or with one that isn't three finite numbers not all 0, z is taken as up and the warning
says so), a `blockMeshDict` that isn't there, doesn't load, or isn't what the blocks make
(edited by hand, written from other blocks, or a patch's type the blocks' `types` or its role
give otherwise now). The grid's
problems are listed together, the first 20 of them and then how many more; so are the patches
the description doesn't list, and the patches where a role doesn't fit, 20 of each.

`workflow.patch` renaming a patch renames it in the region's blocks too (sides, single faces,
patches without faces, types), and refuses a new name the blocks already use for another
patch. While the region has blocks it renames no patch to or from `defaultFaces`, the faces left
to blockMesh (that would join or split them): change the blocks with `mesh.block` instead.

### Box

A box around the case's STL surfaces is the simplest block mesh: one block from the surfaces'
bounds plus a margin on each side, as a background for snappyHexMesh or a mesh of its own. A
region's `box` says what its [blocks](#blocks) are made from:

```c++
air
{
    kind        fluid;
    mesh        blocks;
    blocks      { … }                       // made from the box
    box
    {
        surfaces ( buildings.stl trees.stl );   // in constant/triSurface
        margin   ( 100 200 50 50 0 60 );        // west east south north ground top
        cellSize 2;                             // or cells ( 200 75 40 );
    }
}
```

| Key | What |
|---|---|
| `surfaces` | the STL surfaces of `constant/triSurface` whose bounds the box is around: file names ending `.stl`, `.stlb`, `.stl.gz` or `.stlb.gz` |
| `margin` | how far the box reaches past the bounds on each side: `west`/`east` along x, `south`/`north` along y, `ground`/`top` along z, as the blocks name the sides |
| `cellSize` | the cells' size: cubes, the box grown by less than a cell so that each axis is a whole number of cells: at the far end of each axis (east, north, top), except along the gravity, where it grows away from the ground (at `ground` with the gravity along +z, where `top` is the ground); a length within rounding of whole cells keeps its end, so the box never falls short of the bounds plus margins |
| `cells` | instead of `cellSize`, the cells along x, y and z: the box is as the bounds and margins make it |
| `made` | written by `mesh.box`: a checksum of the blocks it made (their stations, cells, gradings, missing blocks and single faces), so that blocks edited since are told from a box or surfaces that changed |

`mesh.box` reads the surfaces (each as OpenFOAM reads it: ASCII or binary, compressed or not),
makes the region's blocks from their bounds and the box, one block with one span along each axis
and a grading of 1, and writes the `blockMeshDict` from them as [`mesh.block`](#blocks) does:
generated, refused over one edited by hand without `replace`, nothing of the dictionary there
lost unsaid. The outcome is `mesh.block`'s, with the box (`box`), the surfaces' bounds
(`bounds`) and whether the box changed (`boxChanged`).

```sh
umcf-cli run --case path/to/case \
  --cmd 'mesh.box={"region":"air","margin":[200,400,200,200,0,150],"cellSize":17.5}' \
  --cmd 'case.save={}'
```

| Parameter | What |
|---|---|
| `region` | the region of `system/umcfWorkflow` (required) |
| `surfaces` | `["buildings.stl", …]`; default: the region's box's, else every STL surface in `constant/triSurface` |
| `margin` | `50` (every side but the ground, which is 0, so the box stands on the surfaces' bottom), or six `[west, east, south, north, ground, top]`, each 0 or more |
| `cellSize`, `cells` | one of the two: `2`, or `[200, 75, 40]` |
| `patches`, `types`, `noFaces` | as `mesh.block` takes them |
| `reshape` | `true` to replace blocks that weren't made from the region's box, or aren't what it made (edited since) |
| `replace`, `dropPatches`, `dropEntries`, `unreadPatches`, `dropSettings` | as `mesh.block` takes them, for the `blockMeshDict` there |
| `remove` | `true` takes the box out of the description; the blocks stay as they are |

Each part defaults to the region's box, so with only `region` the blocks are made again from the
box there (after a surface was moved or replaced, say). A region without a box needs `margin` and
`cellSize` or `cells`. The ground is the side the gravity points to, as the case check takes it
(`constant/<region>/g`, else `constant/g`, else z down): with the gravity along z it is
`ground`, in the OpenFOAM-8 street canyon (y up) `south`; the outcome names it (`ground`), and
says so when no `g` was read and z was taken as up. One `margin` leaves it 0, and a
`cellSize` grows the box away from it. A region without blocks gets the sides' patches `ground`
and `top` across the gravity, `inlet` and `outlet` at the low and high ends of the first other
axis and `side` on the two ends of the last (along z: `inlet` west, `outlet` east, `side` south
and north; with y up: `inlet` west, `outlet` east, `ground` south, `top` north, `side` on
`ground` and `top`), which `patches` changes; in a fluid
region, those of them the description doesn't list yet are added with their roles of the same
names, on whichever side `patches` puts them, so the ground is a `wall` and the others `patch`. A region without a `mesh` gets
`mesh blocks;`. Blocks that weren't made from the region's box (given by `mesh.block`, read by
`workflow.import`, or kept when `remove` took the box out: its checksum went with it), or edited since the box made them (their checksum isn't its `made`: other
stations, cells or gradings, even of one span; an edit of the box entry or of the surfaces
isn't one of the blocks) are replaced only with `reshape`:
their stations, missing blocks and single faces give way to the box's, and the patch around
the missing blocks goes, with the `types` of the patches the box no longer has (as any patch the dictionary drops: a wall only when `dropPatches`
names it); the rest stays (`edit.undo` takes it back). The surfaces are read one after the
other, and the command can be cancelled between them. The call is refused,
nothing changed, for a surface that isn't there, doesn't read or has no point with finite
coordinates, a negative margin, a cell size not above 0, an axis without cells, a box without
length along an axis (flat surfaces with margins of 0 along it), or a box of more than
1 000 000 000 cells (a cell size in millimetres for a case in metres, say); as by `mesh.block`
for the dictionary. The box goes into the description key by key, only those that change.
A later `mesh.block` that changes the grid's stations, cells, gradings, missing blocks or single
faces takes the box out, since the blocks are no longer what it makes; one that changes only
patches or types leaves it. The case check (`workflow.blocks`) names a box without blocks, one with a
problem of its own, and surfaces it names that the case doesn't have; it doesn't read the
surfaces, so a surface changed since is found by `mesh.box` with only `region`. In the desktop
app it is *Box mesh* in the *More* group of the *Mesh* tab: a [form](desktop.md#forms) (the
region, starting on the first with a box; its surfaces; the six margins; the cell size or the
cells along each axis; *Replace blocks not made from the box* and *Write over a blockMeshDict
edited by hand*), filled from the region's box when it has one. While it is open, the 3D view
previews the box the form makes (its sides, its stations strong and its cells' lines faint, at
most 120 along an axis; on the GPU and the CPU alike) and the card says its size, its cells and
which side is the ground, before anything is written. The preview is of the parameters *OK*
sends (what *Edit as JSON* kept too, one margin for all say), with the region's box for what
they leave out, as the command takes them; what keeps it from being made (a margin missing, a
cell size and cells both given, a value the command refuses, a region the description doesn't
have, a surface that doesn't read) is said there instead. The surfaces' bounds are read in the
background (the status bar shows it, *Cancel* stops it), again only after a geometry command,
an undo or redo, or another case.
Patches, types and the other options go through *Edit as JSON*.

### Snappy

A region meshed from the case's STL surfaces by snappyHexMesh, from a background of
[blocks](#blocks) (a [box](#box) around the surfaces, say): its `snappy` entry says which
surfaces it is made with and how, and its `snappyHexMeshDict` is written from it.

```c++
air
{
    kind        fluid;
    mesh        ( blockMesh "system/air/blockMeshDict" snappyHexMesh "system/snappyHexMeshDict.air" );
    blocks      { … }
    snappy
    {
        inside     ( 1 1 5 );               // locationInMesh: a point in the cells the region keeps
        surfaces
        {
            buildings { mode cut; level ( 2 3 ); layers 2; features 3; }   // buildings.stl
            trees     { file trees.stl; mode refine; distance ( ( 1 3 ) ( 3 2 ) ); }
        }
        boxes      { street { min ( 0 0 0 ); max ( 40 30 4 ); level 1; } }
        expansion  1.2;
        firstLayer 0.15;
    }
}
```

| Key | What |
|---|---|
| `inside` | a point in the cells the region keeps (snappyHexMesh's `locationInMesh`): outside every surface that cuts, inside the blocks (not in a missing block). The vegetation region is the air without the trees, so its point is outside them too, as the air's |
| `surfaces` | by name, each surface of `constant/triSurface` the mesh is made with |
| `file` | the surface's file: `<name>.stl` unless another is given (`.stl`, `.stlb`, `.stl.gz`, `.stlb.gz`; no space, quote, `;`, braces, parentheses, `$` or `#`, which OpenFOAM doesn't take in a file name) |
| `mode` | `cut`: the mesh ends at the surface (the buildings cut out of the air, the trees out of the vegetation), which becomes a patch of the surface's name (`<name>_<solid>` for each solid of a surface of several), its type from the patch's role as for the [blocks](#blocks) (`ground`, `coupled`, `interior`, `bottom`: `wall`); `refine`: only the cells near it are refined (the air around the trees) |
| `level` | a cutting surface's refinement levels, the least and the most (at sharp angles), each halving the cells' size, 10 at most |
| `distance` | the cells within each distance of the surface refined to its level, the nearest first (the distances growing, the levels not); a refining surface's, and a cutting one's besides its `level` |
| `layers` | cells added along a cutting surface (100 at most) |
| `features` | the level the surface's feature edges are refined to, read from `constant/triSurface/<stem>.eMesh`, which `surfaceFeatures` writes (the workflow's `features yes;`, and `system/surfaceFeaturesDict` listing the surface); an uncompressed surface only |
| `solids` | written by `mesh.snappy` for a surface whose file has several solids: their names as OpenFOAM makes them, each a patch `<name>_<solid>` of a cutting surface, with the surface's layers and the type its own role gives (solids of different types each get their own `patchInfo`) |
| `boxes` | by name, boxes whose cells are refined to their `level`: `min` and `max` corners |
| `expansion` | the layers' expansion ratio (default 1.2) |
| `firstLayer` | the first layer's thickness; without it, the thicknesses are relative to the cells (the last layer half a cell, at least a quarter) |

The dictionary written is self-contained: castellated and, with a surface that cuts, snapped,
with layers where a surface has them, the feature edges snapped explicitly when a surface has
them, OpenFOAM's own mesh quality settings (`#includeEtc
"caseDicts/mesh/generation/meshQualityDict.cfg"`), and the rest as OpenFOAM's
`snappyHexMeshDict.cfg` gives it. Like a [`blockMeshDict`](#blocks) written from the blocks it
says it is generated, with a checksum.

`mesh.snappy` sets a region's `snappy` entry, checks its surfaces and writes the dictionary its
`snappyHexMesh` step names. A region meshed by blocks alone (`mesh blocks;`), or without a mesh,
is given the steps `( blockMesh "system/<region>/blockMeshDict" snappyHexMesh
"system/snappyHexMeshDict.<region>" )`, built in `constant/polyMesh` and moved to the region; one
built by steps without `snappyHexMesh` gets it after them. In an OpenFOAM-8 case,
`system/fvSchemes` and `system/fvSolution` are written where the case has none: snappyHexMesh
reads them there, and a case of regions has them only in each region's directory.

```sh
umcf-cli run --case path/to/case \
  --cmd 'mesh.box={"region": "air", "margin": 20, "cellSize": 2}' \
  --cmd 'mesh.snappy={"region": "air", "inside": [1, 1, 5],
    "surfaces": {"buildings": {"mode": "cut", "level": [2, 3], "layers": 2, "role": "coupled"},
                 "trees": {"mode": "refine", "distance": [[1, 3], [3, 2]]}},
    "boxes": {"street": {"min": [0, 0, 0], "max": [40, 30, 4], "level": 1}}}' \
  --cmd 'workflow.scripts={}' --cmd 'case.save={}'
```

| Parameter | What |
|---|---|
| `region` | a fluid or vegetation region of `system/umcfWorkflow` (required) |
| `inside` | `[x, y, z]`; required for a region without a `snappy` entry |
| `surfaces` | `{"buildings": {…}, …}`: each merged into the surface of its name, a key given `null` taken away (`"features": null`), a surface given `null` taken out; a new surface needs its `mode`. Its keys are the entry's (`file`, `mode`, `level`, `distance`, `layers`, `features`) and `role`, the role given to a cutting surface's patch (each of its `<name>_<solid>` patches) in the description. A surface switched to `refine` loses its `level` and `layers` unless the call gives them |
| `boxes` | `{"street": {"min": […], "max": […], "level": 1}}`, merged as the surfaces |
| `expansion`, `firstLayer` | numbers; `null` takes either away |
| `replace` | `true` writes over a `snappyHexMeshDict` edited by hand or not written by umcfGui (case.save keeps it in the backup) |
| `remove` | `true` takes the `snappy` entry out (its dictionary stays, the case's own) |

Before anything is written, each surface is read as OpenFOAM reads it (the call can be cancelled
between them) and the call is refused, nothing changed, for a surface that isn't there or doesn't
read, a patch name made twice (two cutting surfaces, or a cutting surface and a patch of the
region's blocks), an inside point outside the region's blocks or in one of their missing blocks
(snappyHexMesh would find no cell there) or inside a closed surface that cuts (it would keep the building's inside and cut away the
rest), a problem of the entry itself (a cutting surface without a level, levels that don't go
down away from a surface, a box without a length along an axis, …), a solid region, a region
split from another, or a dictionary another region's mesh reads too. What the outcome notes
instead: a cutting surface that isn't closed (the side snappyHexMesh keeps can't be told, and the
mesh leaks through its holes unless the blocks' sides close them: `geometry.check` lists them), a
cutting surface reaching past the blocks (cut at their sides) or outside them (it cuts nothing),
a region without blocks (where the inside point and the surfaces are isn't checked), an
OpenFOAM-12 case with `constant/geometry` (its snappyHexMesh reads the surfaces there, not those
checked in `constant/triSurface`), features
asked for while the workflow doesn't run `surfaceFeatures`, and cutting surfaces' patches the
description doesn't list (without a role: `patch`). Each surface's solids are recorded in its
entry as the file has them now. The outcome names the dictionary, whether it
was written, the entry, and each surface's patches, bounds and whether it is closed. The case
check (`workflow.snappy`) names the entry's problems, a mesh that doesn't run snappyHexMesh,
surfaces the case doesn't have, patches the description doesn't list, features no step makes,
and a `snappyHexMeshDict` that isn't what the entry writes (missing, written from another entry,
or edited by hand); it doesn't read the surfaces. In the desktop app it is *Snappy mesh* in the
*More* group of the *Mesh* tab: a form of the region (the first with a snappy entry to start,
else the first fluid), the inside point, the layers' expansion
and first thickness, *replace* and *remove*; the surfaces and boxes go through *Edit as JSON*.
Checked with snappyHexMesh on OpenFOAM-8 and -12: a building cut out with two layers, the cells
around a tree and in a box refined, `checkMesh` without a failed check.

### Split

A region whose mesh is another region's split by a cell zone: the copy's cells outside the zone.
The street canyon's vegetation is made this way, the air without the trees. Its `mesh` entry
names the region copied, the zone and, when it isn't the copied region's own
(`system/<of>/topoSetDict`), the `topoSetDict` that makes the zone:

```c++
vegetation
{
    kind        vegetation;
    mesh        { from split; of air; zone trees; }
    steps       ( createPatch changeDictionary );
}
```

`workflow.scripts` writes it into `Allprepare` after the region copied (listed before it), then
the region's own steps (OpenFOAM-12):

```sh
rm -rf constant/vegetation/polyMesh
mkdir -p constant/vegetation
cp -r constant/air/polyMesh constant/vegetation/polyMesh
rm -rf constant/vegetation/polyMesh/cellZones constant/vegetation/polyMesh/cellZones.gz constant/vegetation/polyMesh/sets
rm -rf constant/region1/polyMesh constant/region2/polyMesh
runApplication -o -s vegetation topoSet -region vegetation -dict system/air/topoSetDict
runApplication -o -s vegetation splitMeshRegions -region vegetation -cellZones -overwrite -noFields
if [ ! -d constant/region1/polyMesh ] || [ -d constant/region2/polyMesh ]
then
    echo 'region vegetation: the split of air by the cell zone trees left no single part outside it: …' >&2
    exit 1
fi
rm -rf constant/vegetation/polyMesh
mv constant/region1/polyMesh constant/vegetation/polyMesh
rm -rf constant/trees/polyMesh
rmdir constant/region1 constant/trees || true
rm -f constant/vegetation/cellToRegion constant/vegetation/cellToRegion.gz 0/vegetation/cellToRegion 0/vegetation/cellToRegion.gz
```

- The copy loses the cell zones and sets it had (made by the air's own `topoSet`, say):
  `-cellZones` splits by every zone of the mesh, and the tutorial copies the air before its
  zones are made.
- `-overwrite` writes the parts into `constant/` in both versions, not into a time directory that
  depends on the time step; `-noFields` writes none of the region's fields into them (without
  it, the split reads every field of `0/<r>` on the copy).
- `splitMeshRegions -cellZones` names a part after the zone it matches and the cells outside
  every zone `region1` when they are one connected part. That part is the region's mesh, and its
  patch facing the zone is `region1_to_<zone>` (the street canyon's `createPatchDict` makes
  `air_to_vegetation` from it). A zone with no cells, or one that cuts the rest in pieces
  (`region1`, `region2`, …), stops the script with a message, the copy in place of the region's
  mesh. The meshes of parts an earlier split left that it looks at (`constant/region1/polyMesh`,
  `constant/region2/polyMesh`) are removed before the split, so that the check sees only this one's; no region of the case may be named
  `region` and a number.
- Nothing is removed or written through a symbolic link: before its first line touches
  anything, the script stops when a directory the split writes to or removes from
  (`constant/<r>`, `constant/region1`, `constant/region2`, `constant/<zone>`, `0/<r>`, and in
  OpenFOAM-8 `system/<r>` and its `region1`, `region2` and `<zone>`) is one, and
  `workflow.scripts` writes none while the case has such a link (or `constant/`, `system/` or
  `0/` is one).
- Only what the split made is removed: the zone's mesh, the parts' directories when they are
  empty (a directory with anything else in it stays), the `cellToRegion` maps, and in
  OpenFOAM-8 the `fvSchemes` and `fvSolution` it writes for each part in `system/<r>/<part>/`.
  OpenFOAM-8 also reads a `-dict` given with `-region` below the region's own directory, so the
  path goes up one there (`system/../air/topoSetDict`).

Run on the street canyon (OpenFOAM-8 and -12, by hand: the tests don't run OpenFOAM), the
scripts written from the imported description make the same vegetation mesh as the tutorial's
own (its `boundary`, `points`, `faces`, `owner` and `neighbour`). No script is written when the
zone isn't known, when the zone or `region1` is the name of a region of the case (described or
not: the clean-up works in their directories), when a region is named `region` and a number,
when the dictionary makes other cell zones too (`splitMeshRegions -cellZones` would split those
off as well, writing their meshes to `constant/<zone>`), makes zones that can't be told (it
doesn't read, or its actions use `#include` or a macro), doesn't make the zone, or isn't there
(the script would replace the region's mesh by the copy before `topoSet` stopped it), when the
case has a directory of the zone's name in `constant/` or `system/<region>/` that isn't a
region's (the split writes its part there, and the clean-up takes its mesh), or a time
directory with a mesh of the region (`topoSet` and `splitMeshRegions` read the mesh of the time
they start at, which may be that one and not the copy: the street canyon starts from its latest
time; with results there, the split's `cellToRegion` field is also written into that time's
region directory and stays), or when the region copied isn't built before it.
The fields the tutorial copies into the region after the split (`cp 0/vegetation.bckp/*
0/vegetation/`) are listed by the import and not yet written ([gaps](../gaps.md), row 8).

`workflow.import` reads the split from the scripts: the copy of the mesh into the region (with
the `mkdir` before it), the zones taken out of the copy and the parts an earlier split left, a `topoSet -region <r>` on it (its
`-dict`, OpenFOAM-8's `system/../air/topoSetDict` read as `system/air/topoSetDict`; without one,
the region's own; another option doesn't fit), `splitMeshRegions -region <r> -cellZones` (with or
without `-overwrite` and `-noFields`; another option doesn't fit), the part outside the zone put
in from where the split wrote it (`cp -r 3600/region1/polyMesh constant/vegetation/` without
`-overwrite`, the `mv` from `constant/` above with it; a copy from elsewhere doesn't take it),
and what the split
leaves removed: the parts (`region1`, the zone, `region*`) in `constant/` and `0/` or their
meshes, their `fvSchemes` and `fvSolution` in `system/<r>/`, the maps, and the time directory
when the split wrote one. Anything else is listed, as are a copy no split follows, a `mkdir`
no copy follows, and a split whose part outside the zone is never put in the region (it builds
no mesh there). The zone is the one the clean-up names (`system/vegetation/trees`,
`constant/trees/polyMesh`; not a dictionary's or a region's name) when the dictionary makes it,
else the one cell zone the dictionary makes (its `cellZoneSet` actions, in order: `new` and
`clear` make a zone, `remove` takes it away, and another action on a zone not made yet, which
would stop `topoSet` on the copy, leaves the zones untold).

`mesh.split` sets it: `region`, `of` (the region copied), `zone` and `dict` (default:
`system/<of>/topoSetDict`), in place of the mesh the region had, or `remove: true` to take it
out. It is refused, nothing changed, while the region has blocks or a box (take them out first),
for a zone named as the parts outside it (`region1`, `region2`, …) or as a region (the region
itself too), while a region is named
`region` and a number, a zone of the name of a
directory the case has (`constant/<zone>`, `0/<zone>`, `system/<region>/<zone>`: the split
writes its parts there), a region split from itself, and names that aren't plain OpenFOAM
words. It says when the dictionary doesn't make the zone, makes others too, or isn't there, and
returns the zones it makes (`zones`).

```sh
umcf-cli run --case path/to/case \
  --cmd 'mesh.split={"region":"vegetation","of":"air","zone":"trees"}' --cmd 'case.save={}'
```

The case check (`workflow.split`) names a split from a region that isn't described, that comes
after it or whose mesh isn't built, a zone that isn't named or that the dictionary doesn't make,
a dictionary that makes other zones too (or one the case doesn't have), and a zone or `region1` named as a region. In the
desktop app it is *Split mesh* in the *More* group of the *Mesh* tab: a
[form](desktop.md#forms) with the region, the region it is split from (only those listed before it
whose meshes the description builds offered), the cell zone (those the dictionary makes offered), the dictionary (left empty for
the copied region's own, so it follows *Split from*) and *Take the split out*, starting on the first split region (else the first vegetation region) and filled from
the region's split when it has one.

### Settings

The `settings` block holds what a case chooses of its settings; every other one is the [one
configuration](new-case.md#the-configuration)'s, or what the case's files say:

| Key | Where the case has it |
|---|---|
| `deltaT`, `endTime`, `writeInterval` | `system/controlDict` (numbers above 0; `endTime` may be 0; `writeInterval` counts in the unit of the case's `writeControl`: time steps, or seconds with `adjustableRunTime`) |
| `turbulence` | each `simulationType RAS` fluid's `constant/<r>/momentumTransport`: the RAS model (`RAS/model`, or `RASModel` where the file has that), one of OpenFOAM's for the case's version (`kEpsilon`, `realizableKE`, `kOmegaSST`, …) or the solver's (`porousrealizableKE`); the one the case's fluids have is accepted too (a model of the case's own library), and a case whose version isn't known takes any name |
| `temperatureLimit` | `( min max )` in kelvin: the bounds of each fluid's `limitTemperature` entries (in the file the solver reads it from: OpenFOAM-12 `system/<r>/fvConstraints`, else `system/<r>/fvOptions`, else `constant/<r>/fvOptions`; OpenFOAM-8 `constant/<r>/fvOptions`, else `system/<r>/fvOptions`; in the entry's `limitTemperatureCoeffs` when it has one, where OpenFOAM reads them) |
| `blending` | `{ dampingThickness 60; alphaCoeffU 0.3; alphaCoeffT 0.3; }`: `blendingCoeffs` in `system/controlDict` |
| `numerics` | the solver numerics per region kind: `{ fluid { SIMPLE { nNonOrthogonalCorrectors 1; } relaxationFactors { equations { "(h\|e)" 0.3; } } } solid { … } vegetation { … } }`, entries of the regions' `system/<r>/fvSolution` under `solvers`, `SIMPLE`, `PIMPLE`, `PISO` and `relaxationFactors` (three levels at most; numbers or plain words such as a solver's name; field names or patterns as keys, brackets balanced; an entry given twice refused), written into every region of that kind and into new ones (a new region: the description's entries over those of the case's regions). An entry goes only where it is one: never over a dictionary of the file nor through a value of it, never a field's dictionary the file's `solvers` hasn't (OpenFOAM would take it before the pattern that covers the field, with only the entries given: one is added by hand), nor a dictionary the file hasn't next to a directive or macro in the dictionary it would go into (an `#include` or `$common` may bring it); a value right under `solvers` is refused (a field's entry there is a dictionary); what isn't written is noted. A pattern key is written quoted. The description keeps the entries chosen, written or not (a note says which weren't), and refuses a value where it holds a dictionary or the other way round. A solver's dictionary needs what its solver reads (a `smoothSolver` its `smoother`): a solver changed in a field's dictionary keeps the dictionary's other entries. Read from a case as what its regions have that differs from the [configuration](new-case.md#the-configuration), per kind what all its regions share (regions of a kind that differ noted), so that choosing them back changes none; a file with a directive or macro at its top noted. Taken `from` a case of another OpenFOAM version, they are left out with a note (give those wanted with `numerics`: a turbulence model taken from it may need its fluids' relaxation, [below](#settings-tested)); those given with `from` go over its entry by entry. `workflow.show` and `workflow.settings` return the values as written, as strings |
| `materials` | the material library: `{ HamstadConcrete { rho 1950; cap 850; lambda1 1.3; lambda2 0; } … }`, each `buildingMaterialModel` the solver has for the version with the coefficients its new materials start from (numbers, or a dictionary of them such as `VanGenuchtenCoeffs`), at least those the solver reads of every material (`rho`, `cap`, `lambda1`, `lambda2`). Not written into the case's materials: they keep theirs, which may differ for one model (layers). `workflow.settings` says so, and whether the library is kept: that needs a `system/umcfWorkflow` that loads and isn't a symbolic link. A model the case's solids have already is taken even when the solver's catalog lacks it (one of the case's own library) |

`workflow.settings` chooses some of them, and `from` takes another case's, read only (a
relative path is from the open case's directory; refused when that case's materials lack a
coefficient the solver reads of every material; those given too come first: `{"from": "../other-case", "endTime": 86400}`): each goes into the case's files where the case has
it, nothing else in them changing, and into the description's `settings` block entry by entry
when the case has one. A temperature limit or blending coefficients in a case without that
component are kept in the description only, with a note, until the component is switched on
([below](#components)). `case.new` takes the same as `settings`, with the same note. A key the block gives twice
is refused until the file is mended. The block holds what was chosen through these commands:
`control.set` and `dict.set` change the case's files only, so after them the block may say
something else than the files (`workflow.import` with `replace` reads them again). `workflow.import` reads them from the case's
files (the library from the solids' materials, each model's first; a model whose materials
differ is noted once, and a material with a macro or a directive among its coefficients is left
out with a note): a value given by a macro or a directive, or differently in two fluids, is left out with a
note. Tested read-only on 23 cases of both versions (the 3×3 cases, the Hong Kong project's
steps, the terrain cases, the examples): each case's settings read, and written back change
nothing. With the solver: a new case with `turbulence realizableKE` and a temperature limit of 283.15
to 370.15 K runs on OpenFOAM-8 with the examples' meshes, and a 3×3 case switched from
`realizableKE` to `kEpsilon` runs.

<a id="settings-tested"></a>The Hong Kong benchmark's settings (`from` its OpenFOAM-8 run
`3_umcFoam_vegLAD1`: `deltaT` 1800, `realizableKE`, the limit 283.15 to 370.15 K, its material
library, its blending coefficients kept in the description) run five steps on the street canyon
example of either version, the temperature limit switched on (`endTime` given as 9000 with
`from`). On OpenFOAM-8 its numerics come along; on OpenFOAM-12 they are left out, and
`realizableKE` with the example's relaxation (0.5 on `U`, `(h|e)`, `k`, `epsilon`) diverges in
the first step on that coarse mesh, the temperature running to the limit's bounds until
OpenFOAM's temperature from enthalpy doesn't converge. With the Hong Kong run's relaxation given
too (`"numerics": {"fluid": {"relaxationFactors": {"equations": {"U": 0.3, "(h|e)": 0.3, "k":
0.3, "epsilon": 0.3, "w": 0.5}}}}`) it runs. Each of its other settings runs alone with the
example's.

```sh
umcf-cli run --case path/to/case \
  --cmd 'workflow.settings={"endTime": 86400, "turbulence": "realizableKE", "temperatureLimit": [283.15, 370.15]}' \
  --cmd 'case.save={}'
```

In the desktop app it is *Choose settings* in the *More* group of the *Case* tab, a
[form](desktop.md#forms) that starts with the case's settings as its files have them: the time
step, end time and write interval, the turbulence model (picked from the RAS models of the
case's version), the temperature limit's min and max, the blending coefficients, and a case to
take settings *From* (*Browse…*). Only what is changed is sent; the material library and the
numerics are edited in *Edit as JSON*, and sent only when changed there. After another command
or an undo, what wasn't changed follows the case, entry by entry (a numerics entry edited
stays, the others follow), in the JSON shown too.

### Components

`workflow.component` switches a [component](new-case.md#components) on or off in a case, in
its files as `case.new` writes them and in the description's `components` when the case has
one (its other components as they are):

| Component | On | Off |
|---|---|---|
| `grass` | `constant/air/grassProperties` (where `case.new` puts it): its model (the one its coefficients are for, else `simpleGrass`) and the `patches` given, which must be `air`'s; the configuration's file when there is none, without the tutorial's patches, and what a file lacks of it filled in; a `grass off;` switched back on (a note when it names no patch). Needs a fluid region named `air` and the radiation | `grassModel none;` (the solver's `noGrass`) in every fluid's `grassProperties`, its coefficients and patches kept for switching it back on |
| `temperatureLimit` | a `limitTemperature` entry in each fluid without one in the file the solver reads its limit from: OpenFOAM-12 `system/<r>/fvConstraints`, else `system/<r>/fvOptions`, else `constant/<r>/fvOptions` (it never reads `constant/<r>/fvConstraints`); OpenFOAM-8 `constant/<r>/fvOptions`, else `system/<r>/fvOptions`; else a new one in `system/`. Its bounds the description's settings', else 273.15 to 373.15 K | every fluid's entries taken out of the file the solver reads them from; their bounds kept in the description's settings when it has none, and those not kept noted; one in a file the solver doesn't read is left as it is, with a note |
| `blending` | `blending true;` in `controlDict`, with `blendingCoeffs` (the description's, else the configuration's) when it has none. Needs the gravity along z and the patches `west`, `east`, `north` and `south` in each fluid's mesh, which the solver's blending layer finds by name (a note when a fluid has no mesh yet) | `blending false;`, the coefficients kept |

A case whose version isn't known, a patch `air` doesn't have, a description that gives
`components` twice, or a link where a file would go is refused, with nothing changed. A component already as asked changes nothing, with a note.

```sh
umcf-cli run --case path/to/case \
  --cmd 'workflow.component={"component": "grass", "on": true, "patches": ["air_to_street"]}' \
  --cmd 'case.save={}'
```

In the desktop app it is *Switch a component* in the *More* group of the *Case* tab: a
[form](desktop.md#forms) of the component, *On* (ticked to start with) and, for the grass, the
patches of air to tick (its mesh's and those the description gives it), those the case's
`grassProperties` names ticked to start with; the patches are sent only when changed (all
unticked: `[]`, an empty `grassPatches` list written, the grass staying on), and only with the
grass switched on; after another command or an undo, the ticks follow the file while none was
changed, a changed selection staying as it is. Its card starts
with the temperature limit.

### Editing the description

Three commands change the description entry by entry, so its comments, order and layout stay
(they change the description only; [`region.add`](new-case.md#adding-a-region) adds a region
to the case itself, and describes it):
a patch kept on one line (`west { role inlet; }`) is written on its line again, with what else
it holds as it was written (a value the reader can't read, a quoted name, another key), and one
laid out by hand over several lines, or holding a macro or a directive, is edited key by key,
its comments kept.

| Command | What |
|---|---|
| `workflow.region` | `region` with `kind` (`fluid`, `solid` or `vegetation`) adds the region or gives it that kind (an empty `materials { }` goes when a solid becomes another kind); with `remove: true` it is taken out, and what still names it (coupled patches, the radiation) is listed. A solid with materials stays one |
| `workflow.patch` | `region`, `patch` and any of `role`, `partner` (`{"region": "street", "patch": "top"}`) and `name`. `null` takes a role or partner away; a patch the region doesn't list is added. A new name is followed by the region's [blocks](#blocks) and the partners that named the patch (its own too, when it is coupled to itself, unless the same call sets or takes away its partner) |
| `workflow.material` | `region` (a solid), `material` (the name of its cell zone) and `model` (its `buildingMaterialModel`, or `null`), or `remove: true` |

```sh
umcf-cli run --case path/to/case \
  --cmd 'workflow.patch={"region":"air","patch":"west","role":"inlet"}' \
  --cmd 'workflow.material={"region":"street","material":"brick","model":"Hamstad5Brick"}' \
  --cmd 'case.save={}'
```

They change the description only. The case's own files (the mesh, the fields, the dictionaries
that name patches and materials) follow once they are written from it ([gaps](../gaps.md) rows
4–8); until then, `patch.set` renames a patch in the case. A name is one plain OpenFOAM word
(letters, digits, `_ . - :`, not starting with a digit or `-`). A key the file gives twice (the
reader takes the first, an edit would change the last), and a patch or material with an entry
that ends without its `;` (a key set after it would be read as part of it), are refused until
the file is mended by hand,
and a case without a description gets one from `workflow.import` first. An edit's outcome lists
the problems the file still has. Each edit is one step in the history (Ctrl+Z) and the journal.

In the desktop app they are *Describe a region*, *Describe a patch* and *Describe a material*
in the *More* group of the *Case* tab, each a [form](desktop.md#forms) whose names can be typed
or picked from those the description has (*▾*): its regions, the chosen region's patches and
materials, the partner region's patches, the solver's material models for the case's version
(the patch card starts on the description's first region, the material card on its first
solid).
Choosing what it describes fills in what the description says of it (a region's kind, a patch's
role and partner, a material's model; a name the description doesn't have keeps what was
typed for it, and clears what another's filled in), and only what is changed from that is sent: a role, partner or model emptied is taken
away (`null`). A region's kind is sent as chosen. After another command or an undo, what wasn't
typed follows the description. A patch is added with a role or a partner (a patch without either
goes through *Edit as JSON*: `"role": null`, sent as given). *Back to the form* fills the form
again from what the JSON names, as the description has it then, with the JSON over it; a value
a field can't hold (a name that isn't text) is kept and sent as it is, for the command to
refuse. *Write workflow description*
and *Write the scripts* are forms of their *replace* checkbox.
