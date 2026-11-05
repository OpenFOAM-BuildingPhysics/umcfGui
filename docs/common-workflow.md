# The common workflow

A design, not yet built: what the gaps from [row 4 on](gaps.md) build towards. It is the
answer to one question: how does a case stay consistent when its patches, regions or surfaces
change?

Today every case carries hand-written meshing dictionaries and scripts, copied from case to case
and edited. A patch name is written in a dozen places (the blockMesh or snappyHexMesh dictionary,
`createPatchDict`, each region's `changeDictionaryDict`, every field file, the vegetation patch of
the same name, the view factor dictionary, the partner's `neighbourPatch`, `topoSetDict`). Renaming
it means finding all of them, and the case's scripts make the mesh again from the old ones.

The common workflow turns this around. One file in the case, `system/umcfWorkflow`, describes the
case: its regions, how each is meshed, its patches with their roles, its solids and materials,
its radiation. umcfGui generates from it everything that only carries names and types, and the
scripts (`Allprepare`, `Allrun`, `Allclean`) that build every mesh and every field the same way
for every case. A change (a rename, a new solid, another surface) is one change to the
description; the generated files follow.

## What the known cases do

The design covers the chains of the cases at hand (OpenFOAM-8 and -12 mixed):

| Cases | Fluid regions (`air`, `vegetation`) | Solids | Sets |
|---|---|---|---|
| 3×3 reference cases (`Mesh_3x3*`, the test cases) | a multi-block `blockMeshDict.<r>` with the buildings as missing blocks; root `createPatch` (`defaultFaces` → `buildings`); `snappyHexMesh` that only cuts (`castellatedMesh`); `changeDictionary` | `blockMesh -region` each, `createPatch`, `changeDictionary` | `topoSet` (12) or `setSet -batch` (8) |
| `streetCanyon_CFD*` tutorials | `blockMesh -region air`, `createPatch`, `changeDictionary`; vegetation split from the air by `splitMeshRegions` | `blockMesh -region` each, `changeDictionary` | `topoSet` |
| `windAroundBuildings_CFDHAM[_veg]` tutorials | one background `blockMesh`, kept; `snappyHexMesh` per region from it (buildings cut out with layers, trees cut out of the vegetation); `createPatch` (trees → `air_to_vegetation`); `changeDictionary` | `extrudeMesh` in two layers from the building and ground surfaces, `createPatch` (`originalPatch` → `buildings_to_air`, `otherSide` → `interior`), `changeDictionary` | `topoSet` |
| Hong Kong project | as the STL tutorial, each step in a case directory of its own (`0b` terrain, `0c` snappyHexMesh, `0d_*` extrusions), the meshes copied into the run case | | |

Where the names are written, the same in both versions:

| Dictionary | The patch name |
|---|---|
| `blockMeshDict` | `boundary ( <patch> { type …; faces …; } )` |
| `snappyHexMeshDict` | the surface's key in `geometry`, `refinementSurfaces` and `addLayersControls/layers` (a patch per surface, or `<surface>_<region>` per STL region) |
| `createPatchDict` | `{ name <patch>; constructFrom patches; patches (<sources>); }` |
| `changeDictionaryDict` | `boundary { <patch> { type …; neighbourRegion <r>; neighbourPatch <p>; } }` (no case uses `dictionaryReplacement` or field entries) |
| `topoSetDict` | `patchToFace` with `sourceInfo { name <patch>; }` (OpenFOAM-12 also `patch <patch>;`) |
| `extrudeMeshDict` | `sourcePatches`, `exposedPatchName`: the extrusion's own `originalPatch`, `otherSide` |
| fields, `viewFactorsDict` | the patch's own entry |

## `system/umcfWorkflow`

An OpenFOAM-style dictionary, so it travels with the case, reads with the same tools and can be
edited by hand. A sketch of what it holds (the keys will settle as the pieces are built):

```c++
FoamFile { format ascii; class dictionary; object umcfWorkflow; }

version     12;                 // the OpenFOAM version the scripts and dictionaries are for

background                      // the box every snappyHexMesh region starts from
{
    margin      (50 50 0 60);   // around the surfaces: sides, ground, top
    cellSize    2;
    patches { west inlet; east outlet; north side; south side; ground ground; top top; }
}

regions
{
    air
    {
        kind        fluid;
        mesh        snappyHexMesh;          // blockMesh | snappyHexMesh | extrude
        surfaces    { buildings { level (3 5); layers 3; role wall; couple buildings; } }
        patches     { buildings { role coupled; } }
    }
    vegetation
    {
        kind        vegetation;
        mesh        snappyHexMesh;
        surfaces    { buildings { role wall; } trees { role canopy; } }
    }
    buildings
    {
        kind        solid;
        mesh        extrude;
        from        { surface buildings; inward yes; }
        layers      ( { material brick; thickness 0.10; cells 10; grading 1.2; }
                      { material insulation; thickness 0.08; cells 4; } );
    }
}

radiation   { region vegetation; viewFactors yes; solarRayTracing yes; lai air; }
```

Roles (inlet, outlet, side, top, ground, coupled wall, canopy, interior, bottom, sky) decide a
patch's type, its partner and its boundary conditions, by the rules in
[What a case needs](case-requirements.md#coupling-rules): an air patch coupled to a solid is a
`mappedWall` pair, with a vegetation patch of the same name; the canopy becomes
`air_to_vegetation`; the radiation region's sky patches are `patch` or `mapped`.

## What is generated, and what is edited

| | Who writes it |
|---|---|
| `system/umcfWorkflow` | the user, through umcfGui's commands or by hand |
| Geometry: the STL surfaces, the blocks, refinement levels, layers, extrusion thicknesses | settings in `umcfWorkflow`, written into the meshing dictionaries |
| `createPatchDict`, `changeDictionaryDict`, `topoSetDict` (or the `setSet` batch), `viewFactorsDict`, `setFieldsDict` | generated from `umcfWorkflow` |
| `0.orig/<region>/<field>`: the initial fields with a `boundaryField` entry per patch, by role | generated, then edited by `bc.set` |
| `Allprepare`, `Allrun`, `Allclean` | generated, for the version in `umcfWorkflow` |
| Solver dictionaries (`fvSchemes`, `fvSolution`, physical properties, run controls) | the user, as now (templates for a new case) |

A generated file says so in its header (`// Generated by umcfGui from system/umcfWorkflow: edits
here are replaced`) and is written again whenever the description changes. One edited by hand
since it was generated is kept in the backup before it is replaced, and the change says which.

## The fields

`0.orig` is where the initial fields live. `Allprepare` builds the meshes, copies `0.orig` to `0`
(today's cases do this by hand from `0/<region>.bckp`), then runs `setFields` (leaf area density
in the tree zone). A field that has to match the mesh (per-face values) is made after meshing,
not kept. So after `Allprepare` every region has every field the solver and its chosen models
read ([fields per region](case-requirements.md#fields)), with an entry for every patch.

## Renaming and the other edits

With the description the source, a rename is: change the name in `umcfWorkflow`, generate again,
and rename the patch in the mesh as it is (`patch.set` already does the mesh, the fields of the
start time and `0.orig`, the partner patches and the view factor dictionary). The vegetation
twin and the shared background follow because they are generated from the same entry. What
`patch.set` lists today (the meshing dictionaries) is then generated, not listed.

## Existing cases

An import reads a case into a `umcfWorkflow`:

- the 3×3 pattern is read already ([the workflow](user-guide/workflow.md)); its block layout
  becomes block settings;
- the STL chain needs the reading extended to the kept background, `extrudeMesh` and the moves
  between `constant/polyMesh` and the regions;
- what doesn't fit is listed (as today), and the case keeps its own scripts until the user
  takes the generated ones. Writing them keeps the old ones as backups.

Chains spread over several case directories (the Hong Kong project) are out of scope: their run
case is read as it is, without a workflow.

## Running anywhere

OpenFOAM runs on Linux. umcfGui runs on Linux, Windows and macOS, so the runner reaches OpenFOAM
in one of four ways, set once per machine (not in the case):

| Where OpenFOAM is | How umcfGui runs a step |
|---|---|
| Linux, natively | `sh -c '. <OpenFOAM>/etc/bashrc && cd <case> && sh Allprepare'` |
| Windows: WSL | `wsl.exe -d <distribution> -- sh -c '…'`, the case path mapped (`C:\cases\x` → `/mnt/c/cases/x`); a case inside the distribution's own file system (`\\wsl$\…`) is faster for big meshes |
| macOS (or anywhere): a virtual machine or a container | the case directory shared with it (a mount), and the same command run in it (`docker exec`, `ssh` to the VM) |
| A remote machine or cluster | `ssh`, the case synchronised to and from it (`rsync`), the steps run directly or as a SLURM/PBS job |

So the scripts and dictionaries umcfGui writes are plain POSIX `sh` with LF line endings, run as
`sh Allprepare` (an executable bit doesn't survive every file system), with paths relative to the
case. The runner streams the logs back, can cancel a step, and finds OpenFOAM where it is
installed (the distribution's `/opt/openfoam*`, a user's `~/OpenFOAM/OpenFOAM-*`). Reading, editing,
checking and viewing a case never need OpenFOAM; only running the steps does.

## The steps to build it

The [gaps](gaps.md) list the work in order; for the common workflow:

1. `umcfWorkflow`: the format, read and written losslessly; an import from the 3×3 pattern.
2. The scripts from it (today's generator, from the description instead of the read scripts),
   with the fields: `0.orig` copied after meshing, `setFields`.
3. The generated dictionaries from the roles and the coupling rules: `createPatchDict`,
   `changeDictionaryDict`, `topoSetDict`, `viewFactorsDict`, and `0.orig` by patch role.
4. The geometry settings: background and blocks, snappyHexMesh surfaces, extrusion layers.
5. The STL chain read into it (the tutorials).
6. The runner: native, WSL, a VM or container, ssh and batch jobs.
7. Renames, new regions and new solids as edits of the description.
