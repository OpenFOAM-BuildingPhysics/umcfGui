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
