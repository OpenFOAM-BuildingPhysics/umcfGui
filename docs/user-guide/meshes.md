# Meshes

Every region of a case has its own mesh in `constant/<region>/polyMesh` (a single-region case,
such as a meshing step, has `constant/polyMesh`). umcfGui reads the files OpenFOAM writes there:

| File | Holds |
|---|---|
| `points` | point coordinates |
| `faces` | the points of each face (`faceList`, or `faceCompactList` in binary meshes) |
| `owner`, `neighbour` | the cells on either side of each face |
| `boundary` | the patches: name, type, faces, groups and parameters (`neighbourRegion`, `samplePatch`, …) |
| `cellZones` | named groups of cells (materials and cell zones of the solid regions) |

ASCII and binary (any `arch`), gzip-compressed or not, from OpenFOAM-8 and OpenFOAM-12.

## Checked before use

A mesh is checked as it is read; a mesh that fails is reported, not shown half-read:

- every face has at least three points, and every point it uses exists,
- there is one owner per face, and no more neighbours than faces,
- the number of cells matches `nCells` in the `owner` header,
- the patches cover the boundary faces exactly: each starts where the previous one ends, the
  first right after the internal faces, the last at the final face,
- every cell of a cell zone exists.

## Mesh quality

`umcf-cli mesh <case>` ([Command line](command-line.md#checking-a-mesh)) runs the geometric
checks OpenFOAM's `checkMesh` runs by default, on each region's mesh, with `checkMesh`'s
thresholds. The computation is ported from OpenFOAM-12 (face areas and centres, cell centres
and volumes, and each check). `checkMesh` stays the authority: this is a quick look at meshes
nobody checked, and it says which faces and cells are involved, so they can be shown.

| Code | Level | What it means (`checkMesh`'s threshold) |
|---|---|---|
| `mesh.non-finite` | Error | Points with a coordinate that is not a number or infinite, or so far apart that face areas or cell volumes overflow: no other number can be trusted |
| `mesh.unreadable` | Error | The region's mesh could not be read (the message says why) |
| `mesh.negative-volume` | Error | Cells with zero or negative volume |
| `mesh.face-pyramids` | Error | Faces wrongly oriented or twisted: the pyramid from the face to a cell centre has a negative volume (counted per pyramid, as `checkMesh` does: an internal face can have two) |
| `mesh.zero-area` | Error | Faces with zero area |
| `mesh.open-cells` | Error | Cells whose face area vectors don't add up to zero (openness above 1e-6) |
| `mesh.non-orthogonality-error` | Error | Internal faces 90° or more non-orthogonal: the two cell centres are on the wrong sides |
| `mesh.non-orthogonal` | Warning | Internal faces more than 70° non-orthogonal |
| `mesh.skewness` | Warning | Faces with skewness above 4 |
| `mesh.aspect-ratio` | Warning | Cells with an aspect ratio above 1000 (only the directions the mesh solves in count: the normal of `empty` patches doesn't) |
| `mesh.open-boundary` | Warning | The boundary's face area vectors don't add up to zero (above 1e-6, OpenFOAM-8's threshold; OpenFOAM-12's `checkMesh` passes 1 here, so it never fails this check) |

`checkMesh` counts high aspect ratios and skewness as failed checks. They are warnings here
because a case usually runs with them, if more slowly or less accurately.

Faces of `cyclic` and `processor` patches are treated as boundary faces: the cell on the other
side is not followed. So their non-orthogonality is not computed, and their skewness uses the
boundary formula. Wedge directions are not left out of the aspect ratio.

Tested against `checkMesh`:
- The five regions of the 3×3 test case give the same cell counts, volumes (min, max, total),
  face areas (min, max), aspect ratios, non-orthogonality (max and average) and skewness as
  OpenFOAM-12's `checkMesh`, to the six digits it prints (the opt-in test
  `matches_checkmesh_on_the_3x3_case`). The number of high-aspect cells is the same too (2880 in
  `buildings`, 1632 in `street`).
- The Hong Kong project's `air` mesh (16 million cells) gives what OpenFOAM-8's `checkMesh`
  printed for it: non-orthogonality max 80.6° and average 11.5°, 41 faces above 70°, and
  skewness max 5.79 with 8 faces above 4.

Checking that mesh takes about 40 seconds and 4 GB on one core: 13 seconds to read it, the rest
to check it. It can't be cancelled once it has started.

## Tested on

Every region of six projects, from 25,000 to 16 million cells, reads and passes these checks:
the test cases, the partly ported cases, the meshing steps and the Hong Kong project (32 meshes
in all). Copies converted to binary by OpenFOAM-8's and OpenFOAM-12's `foamFormatConvert` read
back identical to their ASCII originals.

The largest, 16 million cells with 51 million faces and 19 million points, takes about
13 seconds: the four big files are read in parallel. Mesh indices are stored as 32-bit
integers, so the mesh, once read, takes about 1.7 GB of memory. Reading it needs more: the
files' text is held while they are parsed, and the measured peak was 3.7 GB. Plan memory for the
peak.
