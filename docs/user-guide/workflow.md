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
solver      urbanMicroclimateFoam;
reconstruct yes;
```

| Key | What |
|---|---|
| `version` | `8` or `12`: the OpenFOAM version the scripts are for |
| `kind` | `fluid`, `solid` or `vegetation` |
| `mesh` | `blocks` (`blockMesh -region`), or the steps that build it in `constant/polyMesh` before it is moved to the region: `blockMesh`, `createPatch`, `snappyHexMesh`, each meshing step with its dictionary when it isn't the default one |
| `steps` | after the mesh: `createPatch`, `changeDictionary`, `topoSet` (or `setSet "batch"` in OpenFOAM-8), `setFields` with the fields it restores from their backups first |
| `role` | `inlet`, `outlet`, `side`, `top`, `ground`, `coupled`, `canopy`, `interior`, `bottom`, `sky` |
| `partner` | the region and patch a coupled patch is mapped to |
| `materials` | a solid region's materials, in the order of its `buildingMaterials`: each the name of the cell zone built of it, with its `model` (`buildingMaterialModel`); the coefficients stay in the region's `transportProperties` |
| `radiation` | the region the view factors and solar ray tracing are computed on, and the one `calcLAI` runs on |

`workflow.import` writes it from the case: the scripts read as above, the region kinds,
each patch's role, and each solid's materials as its `constant/<region>/transportProperties`
lists them (a material or its model given by a macro or a directive is noted and left out, and
a name listed more than once keeps its first entry). Coupled patches and their partners come from the mesh
(`neighbourRegion`/`neighbourPatch`, or `sampleRegion`/`samplePatch` in OpenFOAM-8); the canopy
is the vegetation patch that maps to the air's cells; the air's inlet, outlet and sides come
from the patch's own velocity condition, not one given by a group or a pattern
(`atmBoundaryLayerInletVelocity`, `inletOutlet`, `slip`); `top`, `ground`, `interior`, `bottom`
and `sides` from their names; the vegetation region's patches
take their air twin's role, the open sides and the top becoming `sky`. What it can't tell is
listed and left without a role. On the 3×3 test case every patch gets its role. A region split
from another (the street canyon's vegetation) is not described yet: its steps are listed.

`workflow.show` returns the description, read from the file or, when the case has none, as the
import would write it; it also says when a file is there that doesn't load (not a dictionary, a
directory, or over the size limit), or one the open case doesn't hold (written or undone since it was opened).
`workflow.import` writes over neither, nor a symbolic link (to a dictionary, a field or
nothing: the save would write the file it points to); with `replace` it writes over the case's own, whose
comments and hand edits are not kept (`edit.undo` brings them back). In the desktop app, both
commands show in the results card the problems and the import's notes first, then the settings
(version, radiation, solver), then the regions with their meshes, steps, patch roles and
materials. The case
check reads the file against the case (`workflow.description`): what can't be read in it, regions
and patches the case doesn't have or it leaves out, other kinds, partners that aren't there, a
solid's materials the case doesn't list (or lists with another model), a model the solver
doesn't have for the case's version (one the case's file has too is the case check's to say),
materials of the case it leaves out, and another OpenFOAM version.


### Editing the description

Three commands change the description entry by entry, so its comments, order and layout stay:
a patch kept on one line (`west { role inlet; }`) is written on its line again, with what else
it holds as it was written (a value the reader can't read, a quoted name, another key), and one
laid out by hand over several lines, or holding a macro or a directive, is edited key by key,
its comments kept.

| Command | What |
|---|---|
| `workflow.region` | `region` with `kind` (`fluid`, `solid` or `vegetation`) adds the region or gives it that kind (an empty `materials { }` goes when a solid becomes another kind); with `remove: true` it is taken out, and what still names it (coupled patches, the radiation) is listed. A solid with materials stays one |
| `workflow.patch` | `region`, `patch` and any of `role`, `partner` (`{"region": "street", "patch": "top"}`) and `name`. `null` takes a role or partner away; a patch the region doesn't list is added. A new name is followed by the partners that named the patch (its own too, when it is coupled to itself, unless the same call sets or takes away its partner) |
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
