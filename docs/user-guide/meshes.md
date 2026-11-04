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

In a vegetation region every problem is a warning, except a mesh that can't be read or has
coordinates that are not numbers. The solver solves no flow equations on that mesh (it
carries the leaf temperature and the radiation exchange), and cases with a few wrongly oriented
faces there run: the large 3×3 cases have 6 to 12, which `checkMesh` reports too.

### With `-allGeometry`

`umcf-cli mesh --all-geometry` (`mesh.check` with `all_geometry: true`) adds the checks of
`checkMesh -allGeometry`, ported from OpenFOAM-12 with its thresholds. They are slower (33 s
for 5.4 million cells, against 6 minutes for `checkMesh`) and all warnings: they hurt accuracy
or convergence, not the run.

| Code | Level | What it means (`checkMesh`'s threshold) |
|---|---|---|
| `mesh.face-tets` | Warning | Faces whose decomposition into tets has a tet of low quality or negative volume (counted per failed test, up to three per face, as `checkMesh` does). Particle tracking and cell-point interpolation use the tets; the flow solution doesn't |
| `mesh.short-edges` | Warning | Points on edges shorter than a millionth of the mesh's extent |
| `mesh.near-points` | Warning | Pairs of points closer than a millionth of the mesh's extent. Like `checkMesh`, this counts only pairs about equally far from the origin, so it finds near-duplicate points |
| `mesh.concave-faces` | Warning | Faces with an angle between consecutive edges concave by more than 10° |
| `mesh.warped-faces` | Warning | Faces whose area is less than 0.8 of the sum of their triangles' areas |
| `mesh.cell-determinant` | Warning | Cells whose internal faces leave a direction poorly determined (determinant below 0.001), which gradients need. Cells of thin solid layers, with internal faces in one direction only, have it. The normals of `empty` and `wedge` patches are left out, as `checkMesh` does |
| `mesh.concave-cells` | Warning | Cells with a face centre on the outer side of another face's plane |
| `mesh.face-weights` | Warning | Faces with an interpolation weight below 0.05 (the face much nearer one cell centre) |
| `mesh.volume-ratio` | Warning | Faces between cells whose volumes differ by more than 100 times |
| `mesh.unchecked` | Info | Faces, cells or points left out of the pairwise checks (tet base points, concave cells, near points): over 1000 points or faces each, or past a work budget proportional to the mesh's size. Only a hostile or broken mesh reaches it |

If a coordinate is so large that a millionth of the extent overflows, short edges and near
points are not looked for (the mesh has `mesh.non-finite` or nonsense coordinates anyway).

Face-tet failures in a `checkMesh` log are warnings since M0.16 (they were errors): the
finite-volume solvers don't use the tet decomposition.

`checkMesh` counts high aspect ratios and skewness as failed checks. They are warnings here
because a case usually runs with them, if more slowly or less accurately.

Faces of `cyclic` and `processor` patches are treated as boundary faces: the cell on the other
side is not followed. So their non-orthogonality is not computed, and their skewness uses the
boundary formula. Wedge directions are not left out of the aspect ratio.

### checkMesh's own results

`checkMesh` is the authority, so what it wrote is read too, for each region:

- **Its log:** `log.checkMesh*` in the case directory or its `log/`, matched to the region by its `-region`
  argument.
  - A run over several times (`-time 3,8`) checked one mesh per time; each gives its own result,
    labelled with its time.
  - A result is used only for a mesh with as many cells, faces and points as it checked: a log of the meshes in
    time directories, or of an older mesh, gets a note saying why it was left out. A log for no
    region of the case, or too large to read (over 16 MB), gets a note too.
  - Each failed check (`***`) becomes `mesh.checkmesh`. It is an error, as `checkMesh` means it
    (wrong orientation, zero or negative volumes or areas, open cells, non-orthogonality
    errors, invalid labels, boundary, zone and wedge errors). The quality findings are
    warnings: aspect ratio, skewness, face tets, small weights and volume ratios, small determinants,
    concave cells, non-aligned edges, boundary openness.
  - Each remark (`*`) becomes a note.
  - A log that ends in a fatal error is an error; one that stops before its verdict is a
    warning.
  - A finding the native check also makes (its default checks: aspect ratio, skewness,
    pyramids, volumes, areas, open cells, non-orthogonality, boundary openness) is a note when
    the mesh was checked natively too, so nothing counts twice. So is an `-allGeometry` finding
    (face tets, weights, volume ratios, determinants, concave cells) when the native check ran
    with `--all-geometry`. The others (topology errors) count as `checkMesh` says.
- **Its sets:** the face, cell and point sets `checkMesh` writes (OpenFOAM-8 on every run,
  OpenFOAM-12 with `-writeSets`) in `constant/<region>/polyMesh/sets`, such as `skewFaces`,
  `wrongOrientedFaces` and `nonClosedCells`. Their labels are listed lowest first, for the 3D
  view. Sets with other names (`topoSet`'s `trees`, …) are left alone. Sets written next to a
  mesh in a time directory, or in `processor*` by a parallel run, are not read.

A set that names faces, cells or points the mesh doesn't have was written for another mesh, and
is reported as `mesh.stale-set`.

| Code | Level | What it means |
|---|---|---|
| `mesh.checkmesh` | Error / Warning / Info | What a `checkMesh` log in the case reported for the region |
| `mesh.stale-set` | Warning | A `checkMesh` set names more faces, cells or points than the mesh has: it was written for another mesh |
| `mesh.unreadable-set` | Warning | A `checkMesh` set could not be read |
| `mesh.checkmesh-incomplete` | Error | A `checkMesh` log longer than is read has failures that are not listed: times past the first 1000 that failed a check or stopped (summarized by the size of mesh they checked, and matched to the mesh as other results are), or failed checks past 200 findings of one time. A warning in a vegetation region, as the findings themselves would be |
| `mesh.checkmesh-unmatched` | Warning | Such failed times whose mesh size isn't known (past 16 sizes, or without mesh stats, which `checkMesh` prints only when the topology changes: usually times with moved points): they may be of another mesh |

Tested against `checkMesh`:
- The five regions of the 3×3 test case give the same cell counts, volumes (min, max, total),
  face areas (min, max), aspect ratios, non-orthogonality (max and average) and skewness as
  OpenFOAM-12's `checkMesh`, to the six digits it prints (the opt-in test
  `matches_checkmesh_on_the_3x3_case`). The number of high-aspect cells is the same too (2880 in
  `buildings`, 1632 in `street`).
- With `-allGeometry`, the five regions give the same edge lengths, concave faces and maximum
  angle, flatness, determinants and the cells below 0.001, concave cells, interpolation weights
  and volume ratios (minimum and average) as OpenFOAM-12's `checkMesh -allGeometry` (the opt-in
  test `matches_checkmesh_all_geometry_on_the_3x3_case`).
- On a copy of the `vegetation` mesh of the partly ported 3×3 case (5.4 million cells), OpenFOAM-12's `checkMesh -writeSets` finds 1440 incorrectly oriented faces. Its `wrongOrientedFaces` set holds the same faces as `mesh.face-pyramids`. With `-allGeometry` it finds 2880 face-tet errors, 96 concave faces (max 25.62°) and 44,396 concave cells, as `--all-geometry` does.
- The Hong Kong project's `air` mesh (16 million cells) gives what OpenFOAM-8's `checkMesh`
  printed for it: non-orthogonality max 80.6° and average 11.5°, 41 faces above 70°, and
  skewness max 5.79 with 8 faces above 4.

Checking that mesh takes about 40 seconds and 4 GB on one core: 13 seconds to read it, the rest
to check it. It can't be cancelled once it has started.

### Layers

When `snappyHexMesh` added layers, its log says how many each patch got: at the end of the
layer phase it prints, for each patch that asked for layers, the layers grown (on average over
the patch's faces) and the thickness reached as a percentage of the one wanted, after a table
of the layers wanted. `mesh.check` reads that summary from `log.snappyHexMesh*` in the case
directory or its `log/`, for each region:

- The region is the log's `-region`, or the end of its name when that is a region of the case
  (`log.snappyHexMesh.air`, as cases that swap `snappyHexMeshDict`s name their logs). A log
  of the default mesh in a case of regions (meshed, then moved: `mv constant/polyMesh
  constant/air`) is the region whose mesh has its size.
- A log is used only for a mesh with as many cells, faces and points as the one it wrote: a
  region meshed again, or changed since (extruded, subset), gets a note saying why it was left
  out. A log whose run added no layers is not reported. In a log of several runs appended, the
  last one counts.
- A log in `log/` with the name of one in the case directory is left out (the case scripts move
  older logs into `log/`). A log over 16 MB is not read.

| Code | Level | What it means |
|---|---|---|
| `mesh.layers` | Warning | A patch got less than 90% of the layers it wanted (on average over its faces), or none; `addLayers` is on but no patch asked for layers (`No layers to generate`: the names under `layers` match no patch); or the log stops before the layer summary |
| `mesh.layers` | Info | The patches that got 90% of their layers or more (one note) |
| `mesh.snappyhexmesh` | Warning | `snappyHexMesh` stopped with a fatal error (in the layer phase or before), so the mesh it wrote may be unfinished |
| `mesh.snappyhexmesh` | Info | A `snappyHexMesh` log that could not be read, or is not used (another region's, another mesh's, or a copy in `log/` of one in the case directory) |
| `mesh.layers` | Info | Patches that grew layers when the log doesn't say how many they wanted (one note) |

OpenFOAM-8 and 12 print the same tables; the `iglooWithFridges` tutorial meshed by either gives
`igloo` its 1 layer, and the fridges 1.96 and 2.09 of their 3 (65% and 70%: warnings).

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
