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

## Tested on

Every region of six projects, from 25,000 to 16 million cells, reads and passes these checks:
the test cases, the partly ported cases, the meshing steps and the Hong Kong project (32 meshes
in all). Copies converted to binary by OpenFOAM-8's and OpenFOAM-12's `foamFormatConvert` read
back identical to their ASCII originals.

The largest, 16 million cells with 51 million faces and 19 million points, takes about
13 seconds: the four big files are read in parallel. Mesh indices are stored as 32-bit
integers, so that mesh needs about 1.7 GB of memory.
