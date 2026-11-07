# Checking a case

The case check finds many of the mistakes that make urbanMicroclimateFoam stop at the start,
or compute something other than what the case says (the codes below). Others, such as a
misspelt standard OpenFOAM boundary condition type, are not caught yet.

The check reads nothing beyond what opening the case already read, so it is fast enough to run
again after every edit.

Run it with `umcf-cli check <case>` ([Command line](command-line.md)), or from Rust with
`umcf_validate::check(&case)`. The GUI will run it before a run.

Each problem has a level, a code and the file it is about, and where there is one, the region,
patch and field:

| Level | Meaning |
|---|---|
| **Error** | The run fails, or doesn't do what the case says |
| **Warning** | Probably a mistake, or something the check couldn't verify |
| **Info** | Worth knowing |

## Boundary conditions against the mesh

Every field needs an entry for every patch of its region's mesh. umcfGui picks the entry for a
patch the way OpenFOAM does (in both OpenFOAM-8 and 12):

A key written twice (quoted or not) is merged into its first place, entry by entry: a value
written only the first time is kept, one written again takes the later value.

1. the entry named after the patch;
2. else an entry named after one of the patch's groups. If several match, the one written
   last wins. A patch's groups are its `inGroups` in the `boundary` file, plus the ones
   OpenFOAM adds itself:
   - constraint patches (`empty`, `cyclic`, `symmetryPlane`, …) are in the group of their
     type;
   - wall patches (`wall`, `mappedWall`, `mappedWallVariableThickness`, …) are in `wall`;
   - mapped patches are in `mapped` (OpenFOAM-12) or `mappedPatch` (OpenFOAM-8).

   A group with the same name as a patch is ignored, as OpenFOAM does.
3. else nothing, for an `empty` patch: OpenFOAM gives it `empty` itself;
4. else a quoted key that is exactly the patch name, else the last quoted pattern that matches
   the whole patch name, such as `"(wall|roof).*"`. OpenFOAM reads patterns as POSIX extended regular expressions (glibc's,
   on Linux).

| Code | Level | What it means |
|---|---|---|
| `bc.missing` | Error | A patch has no entry: OpenFOAM stops with *Cannot find patchField entry*. It is only Info when the `boundaryField` takes entries from an `#include` or `$macro`, or has a pattern this check can't read: the entry may be there |
| `bc.no-boundary-field` | Error | The field file has no `boundaryField`, and nothing (`#include`, `$macro`) that could bring one in |
| `bc.constraint` | Error | An `empty`, `wedge`, `symmetryPlane`, `cyclic`, … patch has a condition of another type and no `patchType` naming the patch type. Constraint patches need their own type. Only Info when the entry came from a group or pattern and the `boundaryField` also includes entries (`#includeEtc "caseDicts/setConstraintTypes"` brings group entries that win over patterns) |
| `bc.unused` | Warning | An entry or pattern matches no patch of the mesh: a renamed or removed patch, or a typo. Entries for processor patches (`processor`, `"procBoundary.*" { type processor; }`) are left alone: they are kept for the decomposed mesh |
| `bc.pattern` | Warning | A quoted pattern can't be read the way OpenFOAM reads it: it uses `\d` (a literal `d` for OpenFOAM; `\w`, `\s` and `\b` are fine), has unbalanced parentheses, or is too long. The patches it covers aren't checked |
| `fields.pending` | Info | The start-time field files are listed but not read yet (the desktop app reads them as a job after a case opens): boundary conditions and field sizes aren't checked until they are |
| `field.object` | Info | The header's `object` is not the file's name: probably a copy (`T_old` saying `object T`). Errors in such a file are reported as Info |
| `bc.unknown` | Error | A `compressible::CFDHAM…` type that urbanMicroclimateFoam doesn't have, usually with a "did you mean" |
| `bc.version` | Error | An urbanMicroclimateFoam boundary condition that exists only in the solver for the other OpenFOAM version |

Only field files are checked: files whose class is a field (`volScalarField`,
`surfaceScalarField`, …). Data tables in a time directory, such as `0/air/Tambient` read by a
`tableFile`, are not, and neither are copies the solver never reads (`T~`, `T.orig`, `T.bak`,
`T.old`, …). A `type` or `patchType` written as a `$macro` is not judged. Quoted words
(`type "empty";`) read like plain ones, as in OpenFOAM.
Standard OpenFOAM types (`fixedValue`, `inletOutlet`, …) are not in the solver
catalog, so a misspelt one is not reported yet.

## Coupled regions

Regions exchange heat and moisture through `mapped*` patches. The partner comes from
`neighbourRegion`/`neighbourPatch` (OpenFOAM-12, which still reads the old names) or
`sampleRegion`/`samplePatch` (OpenFOAM-8).

| Code | Level | What it means |
|---|---|---|
| `coupling.region` | Error | The partner region is not one of the case's regions, or is empty (`neighbourRegion "";`: OpenFOAM stops reading the boundary file; next to an OpenFOAM-12 `coupleGroup` it is a `coupling.group` error). A patch without one couples within its own region |
| `coupling.group` | Error | An OpenFOAM-12 patch has a `coupleGroup` and also `neighbourRegion`, `neighbourPatch` (or the `sample*` names) or `samePatch true`: OpenFOAM-12 stops, it takes one or the other. OpenFOAM-8 ignores the extra keywords. `mappedInternal` reads no `coupleGroup` and is checked by its region |
| `coupling.patch` | Error | The partner region has no patch of that name. Not checked for patches that use no patch there: `mappedInternal`, `sampleMode nearestCell`, `nearestOnlyCell` or `nearestFace`, or `samplePatch none` |
| `coupling.not-mutual` | Warning | The partner points back into this region, but at another patch. One-way sampling (vegetation reading the air's patches) is normal and not reported |

A patch with a `coupleGroup` finds its partner by group, so its region and patch names are not
checked further.

A meshing step names regions that only exist once the steps are put together, so coupling is
checked in solver cases only.

## What urbanMicroclimateFoam needs from regions and patches

The solver finds some regions, patches and files by name, and reads radiation and couples
regions through patches of particular types ([what a case needs](../case-requirements.md#regions)).
Most of these stop the run where the solver looks; a sky or surface mix-up runs, wrong, without a
word. Checked in solver cases.

| Code | Level | What it means |
|---|---|---|
| `case.fluid` | Warning | The case lists no fluid region, or more than one: urbanMicroclimateFoam cases have one, `air` |
| `case.air` | Error | The fluid region is not named `air`, and a fluid or vegetation region solves radiation with `viewFactorSky`, which looks up the mesh `air` |
| `case.ambient` | Error | A solid uses `CFDHAMsolidTemperatureCoupledMixed` or `…TransferCoeff`, and `0/air/Tambient` or `0/air/wambient` (or its `.gz`) is missing: the condition reads them by that path for the rain's temperature, whatever the fluid region is called. `0/air/rainTemp` takes their place, except for OpenFOAM-12's `CoupledMixed`, which looks for it at a path it never finds (`$FOAM_CASE` unexpanded) and always reads the two tables |
| `case.vegetation` | Error | The vegetation region is not named `vegetation` (the solid temperature conditions, `simplifiedVegetation` and `calcLAI` find it by that name), or there is more than one |
| `case.canopy` | Error | With `simplifiedVegetation`, the `vegetation` region has no `air_to_vegetation` patch (the trees' surface, found by name), or one the radiation takes as sky. The usual canopy types, both surfaces, are `mappedInternal` (OpenFOAM-12) and `mappedWall` (OpenFOAM-8) |
| `case.vegetation-patch` | Error | A solid temperature condition with `qrNbr` or `qsNbr` reads the radiation from the `vegetation` patch named like its partner patch in the air, and there is no such patch; or, in OpenFOAM-8, `simpleGrass` reads it from the `vegetation` patch named like each of its `grassPatches` (OpenFOAM-12 falls back to the air's) |
| `case.sky-surface` | Warning | With `viewFactorSky` in `vegetation`, a patch is a surface in the air and sky in `vegetation`, or the other way round. The radiation takes the wall types (`wall`, `mappedWall`, `mappedExtrudedWall` and `nonConformalMappedWall` in OpenFOAM-12; `wall`, `mappedWall`, `regionCoupledWall` and `mappedWallVariableThickness` in OpenFOAM-8) and, in OpenFOAM-12, `mappedInternal` as surfaces, every other patch as sky: the ground and buildings are surfaces, the domain's sides and top sky, so one of the two types is likely wrong |
| `case.material-zone` | Error | A material of a solid's `buildingMaterials` (`constant/<solid>/transportProperties`) has no cell zone of its `name` in that region's mesh (`cellZones`, which `topoSet` or `setSet` writes): the solver assigns each material to the cells of its zone and stops. Also a material's zone that lists a cell the region doesn't have (with the cell count known): the run fails there |
| `case.material-overlap` | Error | A cell is in the zones of two materials, a zone lists a cell twice, or a material is listed twice: the solver stops on a cell assigned twice |
| `case.material-cover` | Error | Cells of a solid are in no material's zone (how many, and the first): the solver stops unless every cell has a material. Zones no material names don't count. Judged when the cell count is known (the `owner` header's note), and every material has a plain name (not a `$macro` or `#calc`) and a zone |
| `case.vegetation-field` | Error | A solid temperature condition's `qrNbr` or `qsNbr` names a field the `vegetation` region doesn't have: the condition looks it up there and stops |
| `case.regions-entry` | Error | OpenFOAM-8: `constant/regionProperties` has no `fluid`, `solid` or `vegetation` entry: urbanMicroclimateFoam looks each up and stops without it, even when there are none (`vegetation ()` will do) |
| `case.table` | Error / Warning | A data table a model in use reads by file name is not there, or is one OpenFOAM's table reader stops on: a `FoamFile` header, times not strictly increasing, a row without a value (Error: the run stops); or it can't be read here (Warning: its times aren't checked). The tables: `constant/sunPosVector` (`directAndDiffuse`, `simplifiedVegetation`); `constant/LW` when it is there (OpenFOAM-12), else `0/air/Tambient`, and `0/air/cloudCover` when there (`viewFactorSky`); `0/air/rainTemp` when there, else `Tambient` and `wambient` (the solid temperature conditions; a missing one is `case.ambient`). A `.gz` counts |
| `case.table-span` | Warning | A table in use starts after the run's start time or ends before `endTime`: OpenFOAM holds its first or last value outside it (`outOfBounds clamp`, the default), so the sun or the weather stops changing |
| `case.cloud-cover` | Info | OpenFOAM-12: `0/air/cloudCover` is there, but `viewFactorSky` looks for it at `$FOAM_CASE/0/air/cloudCover` unexpanded, never finds it, and takes a cloud cover of 0 |
| `case.rain-temp` | Info | OpenFOAM-12: `0/air/rainTemp` is there, but `CFDHAMsolidTemperatureCoupledMixed` looks for it at `$FOAM_CASE/0/air/rainTemp` unexpanded, never finds it, and takes the rain's temperature from `Tambient` and `wambient` |
| `case.view-factors` | Warning | With `viewFactorSky`, radiating patches (a fixed-value `qr` with faces) have no entry, by name or quoted pattern, in the `viewFactorsDict` `faceAgglomerate` reads: its `-dict` in the case's scripts, else `system/viewFactorsDict` in OpenFOAM-12 (not the region's), `constant/<region>/viewFactorsDict` in OpenFOAM-8. It leaves them unagglomerated, every face its own, which makes `viewFactorsGen` and the run slow and big. Not judged when that dictionary isn't there yet (a script copying it in place before the step) |
| `case.qr-type` | Error | With `viewFactorSky`, a fixed-value `qr` other than `greyDiffusiveRadiationViewFactor` (a plain `fixedValue`): viewFactorSky takes every one as its own condition and stops |
| `bc.patch-type` | Error | A condition on a patch that can't carry it: urbanMicroclimateFoam's coupled conditions (`compressible::CFDHAM…`, apart from `…TransferCoeffInterior`) need a mapped patch (`mapped`, `mappedWall`, `mappedExtrudedWall`, `nonConformalMappedWall` in OpenFOAM-12; `mappedPatch`, `mappedWall`, `mappedWallVariableThickness` in OpenFOAM-8), `mappedLeafTemperature` a `mappedInternal` patch in OpenFOAM-12 (a mapped one in OpenFOAM-8). The run stops where the condition is built. Only entries that name the patch are checked, not patterns or groups |

The materials' zones come from the mesh the run reads (`constant/<solid>/polyMesh`, or a time
directory's), read with the start-time fields (the desktop app reads them after the case opens)
within the size limit for a field file (256 MB by default). Overlaps are found from the zones'
labels alone; a cell past the region's, and coverage, need the cell count the `owner` header
notes. Nothing is judged for a solid whose mesh is only decomposed (`processor*`) or a list of
materials given by a `$macro` or `#include`; and in a decomposed run the solver reads each
processor's zones, so zones changed with `topoSet -parallel` after `decomposePar` are not what
is checked.

## Solver types

The model types a region's dictionaries select are checked against the
[solver catalog](../solver-coverage.md) for the case's OpenFOAM version. This covers
`vegetationModel`, `grassModel`, `solarLoadModel`, the solar absorption and scattering
models, and `buildingMaterialModel`. They are checked where the solver reads them:
- the top level of `vegetationProperties`, `grassProperties` and `solarLoadProperties`;
- each entry of the `buildingMaterials ( { … } )` list in `transportProperties`.

These files sit directly in `constant/<region>/` (or `constant/`). Other files, such as
`vegetationProperties.orig` or a nested `alternatives { … }`, are left alone. A selector written
twice counts once, with its last value.

| Code | Level | What it means |
|---|---|---|
| `model.unknown` | Error | The solver has no such type. The message lists the ones it has, and the closest match. Only a Warning when `controlDict` loads extra `libs`, which may add it |
| `model.version` | Error | The type exists only in the solver for the other OpenFOAM version (for example `OSB` in an OpenFOAM-8 case) |

## Fields the solver reads

Every region needs the fields the solver reads for its kind of region, and the fields the
models selected there read. A missing one stops the run at the start. The list comes from the
solver source, per version ([Solver coverage](../solver-coverage.md#fields-read)), plus what
OpenFOAM itself reads for what the solver sets up:

| Region | Read by | Fields |
|---|---|---|
| Fluid | urbanMicroclimateFoam | `U`, `w`, `gcr`, `p_rgh` at the start time, `g` in `constant/<region>` |
| Fluid | OpenFOAM's thermophysics | `T`, `p` |
| Fluid | OpenFOAM's RAS or LES turbulence (`momentumTransport`) | `nut`, `alphat`, and the RAS model's own: `k` and `epsilon` (`kEpsilon`, `realizableKE`, `RNGkEpsilon`), `k` and `omega` (`kOmega`, `kOmegaSST`) |
| Fluid | vegetationModel `simplifiedVegetation` | `LAD`, always from `0`, whatever time the run starts from |
| Fluid | RAS model `porousrealizableKE` (the solver's) | `k`, `epsilon`; `LAD` from `0` (OpenFOAM-8) or `constant/<region>` (OpenFOAM-12) |
| Solid | urbanMicroclimateFoam | `Ts`, `ws`, `pc`, and `g` in `constant/<region>` |
| Vegetation | urbanMicroclimateFoam | `T` |
| Fluid, vegetation | radiation model `viewFactorSky` (`radiationProperties`) | `qr` |
| Fluid, vegetation | solar load model `directAndDiffuse` (`solarLoadProperties`) | `qs` |

Notes:
- Fields read only if present (`phi`, `Tl`, `Tg`) are not required.
- Files the radiation utilities write (`F`, `finalAgglom`, …) are covered by the
  [script checks](#scripts).
- A model can read different fields in each of its constructors. Only the constructors the
  solver builds it with count (it builds `solarLoadModel` from a field, so `directAndDiffuse`
  needs `qs`, not the `Qs` its dictionary constructor reads). A read in a member function,
  which may never run, is not required.

| Code | Level | What it means |
|---|---|---|
| `field.missing` | Error | A field the solver or a selected model reads is not where it is read from. Only a Warning in a decomposed case, whose parallel run reads `processor*/`. Fields read from the start time are not reported for a region that has none there (`region.no-fields`); those read from `0` or `constant/<region>` still are |

## Field sizes

A `nonuniform` `internalField` has one value per cell of a volume field (`vol…Field`), per
internal face of a surface field (`surface…Field`) and per point of a point field
(`point…Field`). OpenFOAM stops on any other length. The check compares the list's length
with the counts in the `owner` header of the mesh the run reads, which OpenFOAM writes there
(`nPoints:… nCells:… nFaces:… nInternalFaces:…`): the region's mesh in the latest time
directory up to the start time that has one (meshing without `-overwrite` writes one), else
the one in `constant`. A mesh without that note is not checked.

| Code | Level | What it means |
|---|---|---|
| `field.size` | Error | The `internalField` lists another number of values than the mesh has cells, internal faces or points: most often a field from another mesh (copied in, or left from before meshing again). Only a Warning in a decomposed case, whose parallel run reads `processor*/`, and Info in a copy (`field.object`) |

## Keywords the selected models need

The vegetation, grass, solar load and radiation model selected in a region's dictionary
(`vegetationProperties`, `grassProperties`, `solarLoadProperties`, `radiationProperties`)
reads its coefficients from `<Type>Coeffs` in the same file. The keywords it reads with
`lookup` whenever the solver builds it must be there. Those are the keywords read in the
constructors the solver uses, or in a function each of them calls, outside any condition (a
block run only on the master process counts: every run has one)
([Solver coverage](../solver-coverage.md#keywords-read-at-start)). For example,
`simplifiedVegetation` needs `a1`, `a2`, `a3`, `C`, `D0`, `H`, `kc`, `l`, `nEvapSides`, `Rg0`,
`Rl0` and `rsMin`, and `viewFactorSky` needs `smoothing` and `constantEmissivity`.

A dictionary whose entries can come from elsewhere (`#include`, `$macro`) or match by pattern
(`"a.*"`) is not judged.

| Code | Level | What it means |
|---|---|---|
| `keyword.missing` | Error | A keyword the selected model reads when it is built is not in its `<Type>Coeffs` dictionary (or that dictionary is not in the file) |

## Regions and run controls

| Code | Level | What it means |
|---|---|---|
| `region.no-mesh` | Warning | A region has no mesh in `constant/<region>/polyMesh` (not meshed yet, or the case is only decomposed) |
| `region.no-fields` | Error | A region of a solver case has no fields in the time directory the run starts from (for example `startFrom latestTime` and the latest time has only the solid regions). Only a Warning in a decomposed case, whose parallel run reads `processor*/` |
| `control.application` | Warning | `application` is not set in `controlDict` |
| `control.value` | Error | `startFrom` or `stopAt` has a value OpenFOAM doesn't know |
| `control.end-time` | Error | `endTime` is not after the start time, with `stopAt endTime` and `startFrom startTime` (or `firstTime`) |
| `control.finished` | Info | The run resumes from the latest time (`startFrom latestTime`, also OpenFOAM's default), which is already at `endTime`: it does nothing until `endTime` is raised |
| `control.step` | Error | `deltaT` or `writeInterval` is zero or negative, or `writeInterval` is below 1 with `writeControl timeStep` |
| `case.version` | Warning | The OpenFOAM version can't be told from the case, or the case has files for both (a partly ported case) |
| `case.note` | Warning / Info | What opening the case reported: files that didn't read, a missing start time, files the version ignores |

Values written as `$macros` are not judged.

## Scripts

The case's scripts (`Allprepare`, `Allrun`, `Allclean`, `*.sh` and batch job files) are read
as described in [The case workflow](workflow.md) and checked against the case and its
OpenFOAM version. Each problem names the script and line.

| Code | Level | What it means |
|---|---|---|
| `workflow.utility` | Error | A utility the case's OpenFOAM version doesn't have: `setSet` and `foamCleanPolyMesh` are not in OpenFOAM-12, `foamCleanCase` and `foamRun` not in OpenFOAM-8, `surfaceFeatureExtract` in neither (OpenFOAM-12 only prints that `surfaceFeatures` replaced it). Only a Warning inside an `if`, and for OpenFOAM-12 names that run their replacement (`simpleFoam` runs `foamRun -solver incompressibleFluid`, `postProcess` runs `foamPostProcess`) |
| `workflow.all-regions` | Error | `-allRegions` (decomposePar, reconstructPar) in OpenFOAM-12 without `regionSolvers` in `controlDict`, from which OpenFOAM-12 takes the region list, or in OpenFOAM-8 without `constant/regionProperties`. A Warning when `regionSolvers` names other regions than the case has |
| `workflow.np` | Error | `mpirun -np N` (or `runParallel`) starts another number of processes than the case is decomposed into. A Warning when `runParallel` has no `system/decomposeParDict` to take the count from |
| `workflow.ntasks` | Warning | A batch job asks for fewer tasks (`#SBATCH --ntasks`) than it starts processes |
| `workflow.decompose` | Warning | OpenFOAM-8: regions decomposed into different counts (`system/<r>/decomposeParDict`, which `decomposePar -region` reads) |
| `workflow.decomposed` | Warning | The processor directories are from another decomposition than `decomposeParDict` says |
| `workflow.calclai` | Error | `calcLAI` runs, and the case has no region named `vegetation`: calcLAI reads that region by name |
| `workflow.missing` | Warning | A file a step reads isn't there when the step runs: a copy's or move's source, or the dictionary the utility reads, where the version reads it (for example `system/<r>/topoSetDict`; OpenFOAM-8 `viewFactorsGen` reads `constant/<r>/viewFactorsDict`). Earlier copies, moves, removals and meshing steps are followed, `Allprepare` then `Allrun`. After a command whose effect isn't known, the rest of the script (and of the scripts after it) is not judged; steps in an `if`, or after a test and `&&`, are not judged. `blockMesh` also finds its dictionary in `constant/[<r>/]polyMesh/` |
| `workflow.overwrites` | Warning | `Allprepare` or `Allrun` copies one dictionary over another that differs: the copy is what the run uses, and edits to the other are lost each time the script runs. Info for `system/blockMeshDict` and `system/snappyHexMeshDict`, the working copies of the region variants |
| `workflow.shebang` | Warning | The first line is a broken `#!` line (`$#!/bin/sh`): the script runs only because a shell runs it itself |
| `workflow.custom` | Info | A step that doesn't fit the [workflow](workflow.md) |
| `workflow.not-needed` | Info | A step that fits but isn't needed |
| `workflow.description` | Warning | The [workflow description](workflow.md#the-workflow-description) (`system/umcfWorkflow`) has a key it doesn't know or a value it can't read, names a region the case doesn't have or leaves one out, gives a region another kind, names a patch its mesh doesn't have or a partner that isn't there, couples a patch to no partner, or is for another OpenFOAM version. Info for a mesh patch it leaves out, and for a patch without a role |

## Finite-volume constraints and pressure limits (OpenFOAM-12)

| Code | Level | What it means |
|---|---|---|
| `fv.options-ignored` | Warning | A region has both `fvConstraints` and `fvOptions`: OpenFOAM-12 reads `fvOptions` only when there is no `fvConstraints`, so its constraints (a `limitTemperature`, say) don't apply |
| `fv.limit-pressure` | Warning | A `limitPressure` constraint: urbanMicroclimateFoam (commit `5da4b3a`) constrains the `p_rgh` equation and `U`, never `p`, so it has no effect |
| `control.pressure-limits` | Error | `SIMPLE` in a fluid region's `fvSolution` has `pMin`, `pMax`, `pMinFactor`, `pMaxFactor`, `rhoMin` or `rhoMax`: OpenFOAM-12 stops and asks for them in `fvConstraints` |

## Tested on

The check runs on the local reference cases: the Hong Kong project's meshing chain and runs,
the 3×3 test cases and the large parallel cases, in OpenFOAM-8 and 12. These cases ran, so
every error there must be real. Those found:
- the solid-initialisation run of the Hong Kong project starts from its latest time, which
  has no `air` fields, so running it again would fail;
- a 3×3 case ported to OpenFOAM-12 ends its `Allrun` with `reconstructPar -allRegions` and
  has no `regionSolvers`; its `fvConstraints` also hides the `fvOptions` with the air's
  temperature limit;
- the large 3×3 cases (OpenFOAM-12) clean with `foamCleanPolyMesh`, which OpenFOAM-12 doesn't
  have.
