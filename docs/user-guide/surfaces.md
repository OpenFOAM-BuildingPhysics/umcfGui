# Surfaces

The geometry `snappyHexMesh` meshes around (buildings, terrain, trees) and `surfaceFeatures`
takes feature edges from is a set of STL files in `constant/triSurface`. umcfGui reads them,
says what in them would break meshing, and repairs what can be repaired safely.

## Reading STL files

umcfGui reads ASCII and binary STL, gzip-compressed or not, without trusting the file's header:
a binary STL is an 80-byte header, a triangle count and 50 bytes per triangle, so a file of
exactly that size is binary, even when its header starts with `solid`. A file that starts
with `solid` but isn't text is binary too. Files with a few bytes after the last triangle, or
a count of 0 followed by triangles, are read as well, with a note.

It also says when **OpenFOAM** would read the file differently (`surface.openfoam`):

- OpenFOAM picks the reader by the extension, case-sensitively: `.stl` (and `.stl.gz`) by the
  first word, `.stlb` always as binary. A `.STL` file it doesn't read at all
  (`surface.extension`); `geometry.import` takes `Building.STL` in as `Building.stl`.
- A `.stl` file whose first word starts with `solid` is ASCII to OpenFOAM, so a binary STL
  whose header starts with `solid` (some CAD programs write one) fails to read. An ASCII
  `.stlb` file fails too.
- OpenFOAM refuses a binary file whose triangle count doesn't fit its size.
- OpenFOAM stops at the first line of an ASCII file it can't read: anything but `solid`,
  `facet normal`, `outer loop`, `vertex`, `endloop`, `endfacet`, `endsolid` and `color`;
  keywords in mixed case (`Solid`); a number such as `inf` or a normal of `nan`; words after a
  vertex or `endfacet`; a solid name that isn't ASCII. umcfGui skips facets it can't read
  either, reads the rest, and counts both.

Writing the surface again gives a file OpenFOAM reads: `geometry.repair` does. With
`"degenerate": false` it repairs nothing, but the file is still written anew: triangles grouped
by solid, solid names as OpenFOAM reads them, facet normals from the winding, `color` lines and
a binary file's header dropped, and triangles with coordinates that are not numbers left out
(no reader takes them; the message counts them).

An ASCII file's `solid`s are the surface's regions, which become patches after
`snappyHexMesh`. OpenFOAM names a region after its solid without the characters a word can't
have (`Building 1` is `Building1`), calls an unnamed one `solid`, and joins solids of the same
name; a solid without triangles is still a region (an empty patch). A binary file's regions are
its triangles' attribute values: OpenFOAM makes `patch0` up to `patch<N>` for the largest value
N, empty ones included, so a program that keeps a colour there gives thousands of empty
patches (`surface.regions`). umcfGui names and counts them the same way.

## Checks

Vertices are joined as OpenFOAM joins them when it reads an STL file: where they are within
1e-13 m (ASCII) or 1e-14 m (binary) of each other. Edges then follow from the triangles. On the
local cases and OpenFOAM's tutorial surfaces the vertex, open and non-manifold edge, region
and part counts are the same as `surfaceCheck`'s.

| Code | Level | What it means |
|---|---|---|
| `surface.unreadable` | Error | The file can't be read as an STL surface (the message says why) |
| `surface.openfoam` | Error | OpenFOAM can't read the file as it is (see above) |
| `surface.non-finite` | Error | Triangles with a coordinate that is not a finite number |
| `surface.read` | Info | Something odd that did not stop the file from being read (facets or lines skipped, bytes after the last triangle) |
| `surface.summary` | Info | Format, triangles, regions, extent, parts and how many of them are closed |
| `surface.empty` | Warning | The file has no triangles |
| `surface.extension` | Error | The file's extension is an STL one in the wrong case (`.STL`): OpenFOAM doesn't read it |
| `surface.renamed` | Info | A solid whose name OpenFOAM changes, and the name it uses |
| `surface.regions` | Warning | Regions without triangles: empty solids, or attribute values no triangle has below the largest one |
| `surface.degenerate` | Warning | Triangles without area: two vertices in the same place, or all three in a line. Measured as OpenFOAM measures a triangle's quality (its area over that of the equilateral triangle with the same circumcircle), below 1e-15: where `surfaceCheck` warns the surface "might give problems in self-intersection testing" |
| `surface.duplicate` | Warning | Triangles with the same three vertices as another |
| `surface.non-manifold` | Warning | Edges shared by more than two triangles: the surface has no single inside there |
| `surface.normals` | Warning | Two triangles on an edge face opposite ways. `surfaceFeatures` then takes the edge for a fold (on the 3×3 case's tree crowns it classes 880 of 1920 convex edges as concave), and `snappyHexMesh` decides what is inside a closed surface from its normals (cell zones, `inside`/`outside` refinement) |
| `surface.inward` | Warning | A closed, consistently wound part whose normals point inward: its outside is taken for its inside |
| `surface.open` | Info | Edges of one triangle. Fine for terrain or a surface that reaches the domain boundary; a solid cut out of the mesh, or a cell zone, must be closed |
| `surface.stored-normals` | Info | Facet normals in the file that point against the winding. OpenFOAM uses the winding and ignores them; other programs may not |
| `surface.units` | Warning | The surface is more than 20 km across: probably millimetres (OpenFOAM reads STL coordinates as metres) |
| `surface.far` | Warning | Coordinates more than 131 km from the origin (projected coordinates): an STL file's 32-bit numbers are a centimetre or more apart there (0.25 m at Swiss LV95 coordinates), which moves vertices and can open gaps |

A *part* is a set of triangles connected through their edges; a part with no open or
non-manifold edges is closed. Duplicate triangles stay in the edges, as they do for OpenFOAM:
a copy among other triangles makes their edges non-manifold, and two copies on their own make
a closed pair (removing one of them leaves a single, open triangle).

With `--json` (or the commands' data), each finding lists its triangles (the first 1000, by
their index in the file) with their count, so they can be shown.

## Repairs

| Command | Does |
|---|---|
| `geometry.repair` with `weld` | Moves every vertex within the given distance (metres) of one kept before it onto the nearest such one, which closes gaps narrower than that; every copy of a vertex goes to the same place, and triangles that lose their area are removed. A distance close to the size of the surface's features can join vertices that shouldn't be: keep it well below the smallest edge |
| `geometry.repair` with `degenerate` | Removes triangles that are not finite, copies of another, and triangles without area. A triangle without area whose vertices are in a line shares its long edge with a neighbour: removing it alone would open a crack there, so the neighbour is split in two at the middle vertex instead, and the surface stays closed. One that shares no edge with exactly one other triangle is removed as it is. The others are left and counted: their long edge is open or shared by more than two triangles while a short one is shared, or they lie in a chain of triangles without area that splitting doesn't undo |
| `geometry.repair` with `orient` | Makes the normals of each part consistent across the edges two triangles share, then turns closed parts that point inward outward. An open part keeps the side most of its triangles already face. Where no consistent choice exists (a Möbius-like part) the triangles are left as they are and counted |
| `geometry.scale` | Multiplies every coordinate by `factor` (0.001 turns millimetres into metres); a factor that is not positive is refused, as it would mirror the surface, and so is one that takes a coordinate beyond what STL's 32-bit numbers hold |

Without `weld`, `degenerate` or `orient`, `geometry.repair` does `degenerate` and `orient`. They
run in that order, and the surface is written again in its own format, as described under
[Reading STL files](#reading-stl-files) (binary for a `.stlb`, with the triangles' attribute
values). A repair is checked again after it runs, so what is left shows straight away.

On the 3×3 case, `geometry.repair` removes all 2252 zero-area triangles of `walls.stl` (in the
ground) by splitting their neighbours: `surfaceCheck` then finds
the same 320948 triangles, 4 regions and 1088 open edges, and its lowest triangle quality goes
from 0 to 2e-4. It makes the normals of the 40 tree crowns of `air_to_vegetation.stl`
consistent (720 triangles turned over), after which `surfaceFeatures` classes all 1920 of
their edges as convex.

## Importing

`geometry.import` copies an STL file into `constant/triSurface`, as it is, under its own name or
`name`. It is written again only when it is scaled (`scale`), when OpenFOAM can't read it as it
is (an ASCII file named `.stlb` is written binary), or when its compression doesn't match its
name (`.gz`). A surface already there is replaced only with `"replace": true`. The name must
end in `.stl`, `.stlb`, `.stl.gz` or `.stlb.gz`, in lower case, which OpenFOAM reads as STL.

## Tree crowns

`geometry.trees` writes the trees' crowns as one surface (`trees.stl`, or `name`), for meshing
the vegetation: a rhombicuboctahedron per tree, of its crown diameter and height, standing on
its z (the bottom of the foliage). Each crown is closed and wound outward, 44 triangles. The
trees come from a table, in the case (`table`) or outside it (`path`), or as a list (`trees`):

```text
x       y       z       CD      CH
260     260     2.5     5.5     5.5
330     330     2.5     5.5     5.5
```

One tree a line, the numbers separated by tabs, spaces or commas. The first line names the columns,
`x y z CD CH` in any order, with other columns left alone (a tree height `H` beside the crown's
`CH`, say); `D`/`H` or `diameter`/`height` stand for the crown's when there is no `CD`/`CH`.
Without that line a tree is exactly five numbers, in that order. Lines starting with `#` or `//`
are skipped. At most 80 000 trees, which keeps the surface within the case's file size limit; a
crown much smaller than its coordinates (beyond the seven digits STL holds) is refused. The surface
has one region, `trees` (or `solid`). Only the crowns are made, no trunks: the solver's vegetation
model works on the leaf area density of the tree zone, which the crowns bound.

The shape is the one the vegetation workflow's tree script (`tree_shape_simple.py`) makes. That
script writes the eight triangular faces as squares with a corner twice (16 triangles without
area for two trees), winds some faces inward, and centres each crown on its z, which the table
calls the bottom of the foliage: a crown of 5.5 m at z = 2.5 reaches 0.25 m below the ground. A
table written for it gives crowns half their height higher here.

Imported and repaired surfaces are part of the case's unsaved changes: `case.save` writes them,
after a backup of the file each replaces, and refuses to overwrite a file that changed on disk
since the change was made from it. Until then, `edit.undo` takes a change back; after a save,
undo doesn't put the old file back on disk.

## On the command line

```sh
umcf-cli surface path/to/case [--file walls.stl] [--json]
umcf-cli surface path/to/file.stl [--json]
```

Given a case, it checks every STL surface in `constant/triSurface` (or the one `--file`
names); given a file, that file:

```text
info     surface.summary              constant/triSurface/walls.stl: ASCII 320948 triangles, 4 regions (roofs, ground, street, buildings), 590 × 590 × 10 m from (0 0 0); 1 part (0 closed)
warning  surface.degenerate           constant/triSurface/walls.stl: 2252 triangles without area (…)
1 surface; 0 errors, 1 warning, 2 notes
```

The exit status is 0 when there are no errors, 1 when there are (a file that can't be read is
one), and 2 when the case or file can't be opened. Repairs run through `umcf-cli run`:

```sh
umcf-cli run --case path/to/case --cmd 'geometry.repair={"file": "walls.stl"}' --cmd case.save
```

## Limits

- Only STL is read. OBJ, VTK and the other formats OpenFOAM reads in `constant/triSurface` are
  not checked.
- Whether a surface lies inside the background mesh, and whether `snappyHexMesh` needs it
  closed (a cell zone, an `inside` refinement), is not checked yet: that comes with the meshing
  set-up checks.
- A surface is read whole, up to 1 GB (decompressed) and 10 million triangles. Checking 5
  million triangles takes about 5 s and 1 GB of memory. Each change keeps its own copy of the
  file's bytes in memory for undo, up to the 100 steps undo keeps: repairing a large surface
  many times takes that many copies (`case.save` doesn't release them).
