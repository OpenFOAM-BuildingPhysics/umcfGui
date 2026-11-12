# The common workflow

A design, partly built (the description, its import, check and edits, the scripts, new cases and
regions, the settings: [the case workflow](user-guide/workflow.md#the-workflow-description)):
what [gaps](gaps.md) rows 4–8 and 11 build towards. It answers one question: how does a case
stay consistent when its patches, regions or surfaces change?

Today every case carries hand-written meshing dictionaries and scripts, copied from case to case
and edited. A patch name is written in a dozen places (the blockMesh or snappyHexMesh dictionary,
`createPatchDict`, each region's `changeDictionaryDict`, every field file, the vegetation patch of
the same name (with trees), the view factor dictionary, the partner's `neighbourPatch`), a
material's name in the `topoSetDict` and the material properties. Renaming one means finding all of
them, and the case's scripts make the mesh again from the old ones.

The common workflow turns this around. One file in the case, `system/umcfWorkflow`, describes the
case: its regions, how each is meshed, its patches with their roles, its solids and materials,
its radiation. umcfGui generates from it everything that only carries names and types, and the
scripts (`Allprepare`, `Allrun`, `Allclean`) that build every mesh and every field the same way
for every case. A change (a rename, a new solid, another surface) is one change to the
description; the generated files follow.

## What the known cases do

The design covers the chains of the cases at hand, OpenFOAM-8 and -12 mixed:

| Cases | Fluid regions (`air`, `vegetation`) | Solids | Sets and zones |
|---|---|---|---|
| 3×3 reference cases (`Mesh_3x3*`, the test cases; 12, one 8) | per region a multi-block `blockMeshDict.air` / `.veg` with the buildings as missing blocks (the same layout, other cell counts); a root `createPatch` (`defaultFaces` → `buildings`); `snappyHexMesh` from `snappyHexMeshDict.air` / `.veg`: for `air` refinement around `air_to_vegetation` only, for `vegetation` castellated and snapped, the tree cut out as `air_to_vegetation`; `changeDictionary` | `blockMesh -region` each, `createPatch`, `changeDictionary` | `topoSet` |
| `streetCanyon_CFDHAM*` tutorials (12; 8 on their OpenFOAM-8 branch) | `blockMesh -region air`, `createPatch`, `changeDictionary`; `_veg`: the vegetation split from a copy of the air mesh (`topoSet` a zone, `splitMeshRegions -cellZones`, its mesh taken from the time directory it writes, then `createPatch`, `changeDictionary`) | `blockMesh -region` each, `changeDictionary` | `topoSet`; `setSet -batch` on the 8 branch |
| `streetCanyon_CFD` | `blockMesh -region air`, `createPatch` (its `Allrun`; no solids) | | |
| `windAroundBuildings_CFDHAM[_veg]` (8) | one background `blockMesh`, kept; `snappyHexMesh` per region from it (buildings cut out with layers; trees cut out of the vegetation); `changeDictionary`; vegetation also `createPatch` (trees → `air_to_vegetation`) | `extrudeMesh` in two layers: from the building or ground surface (`constructFrom surface`), then from the first layer's `otherSide` (`constructFrom mesh`); `createPatch` (`originalPatch` → `buildings_to_air`, `otherSide` → `interior`); `changeDictionary` | `topoSet` |
| Hong Kong project (8) | each step a case directory of its own: `terrainBlockMesher`, `snappyHexMesh -parallel` under SLURM with `refineMesh` and the domain sides extruded, solids extruded with patches made from face sets; the meshes copied into the run case | | `setSet` |

The Hong Kong chain is out of scope: its run case is read as it is, without a workflow.

Where the names are written:

| Dictionary | Names |
|---|---|
| `blockMeshDict` | the patch: `boundary ( <patch> { type …; faces …; } )` |
| `snappyHexMeshDict` | the patch: a surface's key in `geometry`, `refinementSurfaces` and `addLayersControls/layers` (a patch per surface, or `<surface>_<region>` per STL region) |
| `createPatchDict` | the patch: `{ name <patch>; constructFrom patches; patches (<sources>); }`, or `constructFrom set; set <faceSet>;` |
| `changeDictionaryDict` | the patch's own entry, `boundary { <patch> { type …; … } }`, and its partner: `neighbourRegion`, `neighbourPatch` in OpenFOAM-12, `sampleMode nearestPatchFace; sampleRegion; samplePatch;` in OpenFOAM-8. No case uses `dictionaryReplacement` or field entries |
| `topoSetDict`, `setSet` batch | the cell zones: materials (`setToCellZone`), the tree zone (`boxToCell`, `surfaceToCell`); patches only in the Hong Kong cases (`patchToFace`) |
| `extrudeMeshDict` | the extrusion's own `originalPatch`, `otherSide` (`sourcePatches`, `exposedPatchName`) |
| fields, `viewFactorsDict` | the patch's own entry. The 3×3 cases also copy `system/vegetation/viewFactorsDict` to `system/viewFactorsDict` |
| function objects | patches (`patchAverage`, `surfaces`), in `controlDict` or `system/<r>/` |
| `constant/<r>/boundaryData/<patch>` | mapped boundary data, by the patch's name |

## `system/umcfWorkflow`

An OpenFOAM-style dictionary, so it travels with the case, reads with the same tools and can be
edited by hand. What is built so far (regions, their kinds, the meshing steps as the scripts run
them, patches with roles and partners, a solid's materials, radiation, the
components, and the solver; its import from a case, its check, its edits entry by entry, and a
[new case](user-guide/new-case.md) made from it) is described in [the case
workflow](user-guide/workflow.md#the-workflow-description). A sketch of where it goes (the
meshing keys settle as rows 5–7 are built):

```c++
FoamFile { format ascii; class dictionary; object umcfWorkflow; }

version     12;                 // the OpenFOAM version the scripts and dictionaries are for

regions
{
    air
    {
        kind        fluid;
        // `mesh` says how the mesh is built (`blocks`: blockMesh -region air; or the steps in
        // constant/polyMesh), `blocks` what the blockMeshDict it reads holds: the 3×3 cases and
        // the street canyon, the buildings as missing blocks (there since M0.83).
        mesh        blocks;
        blocks      { x ( 0 (5 1) 50 (3 1) 60 (3 1) 70 ); y ( 0 (6 1) 100 (5 1) 150 );
                      z ( 0 (3 1) 10 ); missing ( (1 1 0) ); patches { west inlet; east outlet;
                      south side; north side; ground ground; top top; missing ground; } }
        // Planned: the box from the surfaces' bounds (the STL cases).
        box         { margin (50 50 0 60); cellSize 2; }
        surfaces    { buildings { level (3 5); layers 3; role coupled; partner buildings; } }
        patches     { west inlet; east outlet; north side; south side; ground ground; top top; }
    }
    vegetation
    {
        kind        vegetation;
        mesh        { from box; }           // or `from split; of air; zone trees;` (street canyon)
        surfaces    { buildings { role coupled; } trees { role canopy; } }
    }
    buildings
    {
        kind        solid;
        mesh        { from extrude; surface buildings; inward yes; }    // or from blocks
        layers      ( { material brick; thickness 0.10; cells 10; grading 1.2; }
                      { material insulation; thickness 0.08; cells 4; } );
    }
}

radiation   { region vegetation; viewFactors yes; solarRayTracing yes; lai air; }
```

Each region is meshed one of four ways: blocks (`blockMesh`), a box and surfaces
(`blockMesh` then `snappyHexMesh`, from a background kept between regions), extrusion
(`extrudeMesh` in layers), or split from another region by a cell zone (`splitMeshRegions`).

A patch's role (inlet, outlet, side, top, ground, coupled, canopy, interior, bottom, sky) decides
its type, its partner and its boundary conditions, by the rules in
[What a case needs](case-requirements.md#coupling-rules): an air patch coupled to a solid is a
`mappedWall` pair and, in a case with trees (a `vegetation` region), has a vegetation patch of the
same name (without trees the solid reads the radiation from the air patch); the canopy becomes
`air_to_vegetation`; the radiation region's sky patches are `patch` or `mapped`. Cell zones are
named by the description too: a solid's materials, the tree zone.

## What is generated, and what is edited

| | Who writes it |
|---|---|
| `system/umcfWorkflow` | the user, through umcfGui's commands or by hand |
| Geometry: the STL surfaces, the blocks, refinement levels, layers, extrusion thicknesses | settings in `umcfWorkflow`, written into the meshing dictionaries |
| A region's `blockMeshDict` (the one its `mesh` reads) | generated from its `blocks` by `mesh.block` ([the workflow](user-guide/workflow.md#blocks)) |
| `createPatchDict`, `changeDictionaryDict`, `topoSetDict` (or the `setSet` batch), `viewFactorsDict`, `setFieldsDict` | generated from `umcfWorkflow` |
| `0.orig/<region>/`: the initial fields with an entry per patch, by role, and the tables there (`Tambient`, `wambient`, `cloudCover`, …) | the fields' entries generated for patches that have none, then set by the user and kept: `bc.set` sets a condition in the start time's field today, and will set it in `0.orig` too (see [the fields](#the-fields)); the tables from the site and weather data (row 9) or the user |
| The materials' entries in `buildingMaterials` (`transportProperties` of each solid) | added with the material from a library of materials with their coefficients, then edited by the user |
| `Allprepare`, `Allrun`, `Allclean` | generated, for the version in `umcfWorkflow` |
| Solver dictionaries (`fvSchemes`, `fvSolution`, physical properties, run controls) | the user, as now (templates for a new case) |

A generated file says so in its header, as the scripts umcfGui writes already do (`Written by
umcfGui from the case workflow; edit the workflow, not this file.`), and is written again
whenever the description changes. One edited by hand since it was generated is kept in the backup
before it is replaced, and the change says which. A `blockMeshDict` carries a checksum in its
header too: one edited since (or not written by umcfGui) is written over only when asked
(`replace`). The fields of `0.orig` are not generated files:
they are edited in place (below).

## The fields

`0.orig` holds the initial fields and the tables beside them. `Allprepare` builds the meshes,
copies `0.orig` to `0`, then runs `setFields` (leaf area density in the tree zone). Today's cases
keep some of this aside by hand while meshing: the 3×3 cases restore only `0/air.bckp/LAD`, the
STL tutorials the vegetation fields from `0/vegetation.bckp`. A field that has to match the mesh
(values per face) is made after meshing, not kept. So after `Allprepare` every region has every
field the solver and its chosen models read ([fields per region](case-requirements.md#fields)),
with an entry for every patch.

The fields in `0.orig` are edited, never written over: a change to the description adds the
entries of new patches (by role), renames the entries of renamed patches and removes those of
removed ones, each a lossless edit of the kind `patch.set` and `bc.set` make now. The conditions the
user set stay; for that `bc.set`, which sets a condition in the start time's field today, will set
it in `0.orig` too. A missing field is made from its role's defaults.

## Renaming and the other edits

With the description the source, a rename is one command: it changes the name in `umcfWorkflow`,
generates the dictionaries again, and renames the patch in the meshes as they are, in every region
the description names it in, so the case runs without meshing again. For an air patch coupled to a
solid that is the air patch and, in a case with trees, its vegetation twin (`patch.set` already
does a region's mesh, the fields of the start time and `0.orig`, the partner patches and the
region's view factor dictionary). The shared background follows because it is generated from the
same entry. What `patch.set` lists today (the meshing dictionaries) is then generated, not listed.
The rename covers the start time, not what was written from it: other time directories,
`processor*` decompositions and `boundaryData/<patch>` keep the old name. `patch.set` names them
(`patch.times`, `patch.decomposed`, `patch.mention`); the case starts again from the start time and
is decomposed again, and `boundaryData` is renamed by hand or written again.

New regions, solids and materials are edits of the description in the same way. A new material
takes its `buildingMaterials` entry and coefficients from a library of materials (the reference
cases' ones to begin with), so its zone never comes without properties; the case check names a
zone without an entry, or an entry without a zone.

## Existing cases

An import reads a case into a `umcfWorkflow`:

- the 3×3 pattern's scripts are read already ([the workflow](user-guide/workflow.md)), and each
  region's blocks from its `blockMeshDict` when that is a grid of blocks (the fluid, vegetation
  and street regions of the 3×3 cases, every region of the street canyon);
- the STL chain and the split vegetation need the reading extended: the kept background,
  `extrudeMesh`, `splitMeshRegions`, the moves between `constant/polyMesh` and the regions;
- what doesn't fit is listed (as today), and the case keeps its own scripts until the user
  takes the generated ones. Writing them keeps the old ones as backups.

## Running anywhere

OpenFOAM runs on Linux. umcfGui runs on Linux, Windows and macOS, so the runner reaches OpenFOAM
where it is, set once per machine (not in the case): for each OpenFOAM version, how to get its
environment (its `etc/bashrc`, or a `module load` on a cluster) and where.

| Where OpenFOAM is | How umcfGui runs a step |
|---|---|
| Linux, natively | `bash -c '. <bashrc> && cd <case> && sh ./Allprepare'` |
| Windows: WSL | `wsl.exe -d <distribution> --exec bash -c '…'`, the case path mapped with `wslpath` (by default `C:\cases\x` is `/mnt/c/cases/x`) |
| macOS (or anywhere): a virtual machine or a container | the case directory shared with it and mapped like a WSL path, the same command run in it (`docker run --rm -v <case>:<path> --user <uid> …`, `ssh` to a VM) |
| A remote machine or cluster | `ssh`, the case synchronised to it and the results back, the steps run directly or as a SLURM/PBS job |

What this asks of each part:

- **The environment** is sourced by `bash`: OpenFOAM's `etc/bashrc` finds its installation
  through `BASH_SOURCE`, so `sh` (dash on Debian and Ubuntu) fails to source it. The scripts
  themselves stay POSIX `sh`. `urbanMicroclimateFoam` and its utilities (`calcLAI`,
  `viewFactorsGen`, `solarRayTracingGen`, …) must be built in that environment
  (`$FOAM_USER_APPBIN`): a stock OpenFOAM installation or container image doesn't have them. A
  machine often has several versions (`~/OpenFOAM-8` and `~/OpenFOAM-12`); `umcfWorkflow` says
  which the case needs.
- **The scripts** are run as `sh ./Allprepare`: they start with `cd "${0%/*}"`, which needs a
  path, and an executable bit doesn't survive every file system. They are written with LF line
  endings, and a case with CRLF (`core.autocrlf` in git on Windows) is pointed out.
- **WSL**: a case on the Windows drive (`/mnt/c`) is fast for umcfGui and slow for OpenFOAM; one
  inside the distribution (`\\wsl.localhost\<distribution>\…`) the other way round. umcfGui says
  which, for big meshes. Files held open on Windows (an editor, a virus scanner) can make the
  scripts' `rm` and `mv` fail; the step's log says so.
- **Names that differ only by case** (two fields, patches or files) can't live together on NTFS
  or default APFS; umcfGui refuses them before writing.
- **Containers and VMs**: shared directories are slow for big meshes (keep the case inside, or
  synchronise); files written as root need the user mapped (`--user`); an image built for
  another processor (amd64 on Apple Silicon) runs emulated, slowly.
- **Remote runs**: the job script sets up the environment itself (a batch job doesn't inherit
  it); the synchronisation is umcfGui's own where `rsync` isn't there (Windows), and brings back
  only what is asked for (logs, the times and fields wanted), not whole `processor*` trees.

The runner streams the logs back and can cancel a step. Reading, editing, checking and viewing a
case never need OpenFOAM; only running the steps does.

## The steps to build it

In the order of the [gaps](gaps.md):

| Row | |
|---|---|
| 4 | The generated dictionaries from the roles and the coupling rules (`createPatchDict`, `changeDictionaryDict`, `topoSetDict` or the `setSet` batch, `viewFactorsDict`), and `0.orig` by patch role |
| 5–7 | The geometry settings: boxes and blocks, snappyHexMesh surfaces, extrusion layers, the split by a zone |
| 8 | The scripts from the description, with the fields put in place after meshing; the STL chain and the split vegetation read into it; reset |
| 11 | The runner: native, WSL, a VM or container, ssh and batch jobs |
