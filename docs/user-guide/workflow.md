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
- names and paths are quoted for the shell when they need it.

A mesh built in `constant/polyMesh` keeps its steps in the order the case's script ran them
(`createPatch` before or after `snappyHexMesh`); a variant copied into `system/` or passed
with `-dict` is the same dictionary. A region step with its own
`-dict`, or a field restored without a `setFields` after it, doesn't fit and is listed.

Run on the 3×3 test case (OpenFOAM-12, two processes), the written scripts mesh, pre-process
and run the case through to its first written time.

For now this is the `workflow` example of the `umcf-model` crate:
`cargo run -p umcf-model --example workflow -- <case> [--write]` prints the scripts, or with
`--write` replaces the case's (the old ones are kept as `<name>.bak`).
