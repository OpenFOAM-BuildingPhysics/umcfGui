# 3D scene and selection

umcfGui shows a case in 3D as its meshes' boundary patches and its STL surfaces. What is drawn
or picked is a patch, a surface region or a single mesh face, never just a pixel: each triangle
knows the mesh face or STL triangle it came from. Picking selects what later commands act on (boundary
conditions by picking, for example).

## What the scene shows

- **A mesh region**: the faces of its boundary patches, read from `constant/<region>/polyMesh`
  (`constant/polyMesh` in a single-region case), each patch in its own colour. A face with
  more than three points is split into triangles from its first point; the edges drawn with
  `edges` are the face's own, never those splits.
- **An STL surface** in `constant/triSurface`: its triangles, each region in its own colour.
  Regions are named as OpenFOAM names them (see [Surfaces](surfaces.md#reading-stl-files)): an
  ASCII file's solids, a binary file's `patch<N>`. A surface a command changed and the case has
  not saved yet is shown as changed.

By default the scene is every region with a mesh, or every surface when no region has one;
`regions` and `surfaces` choose. In a multi-region case the regions often share their outer
patches (the domain top of `air` and of `vegetation`): pick one region to look inside.

Points far from the origin (projected coordinates) keep their precision: the scene stores them
relative to the middle of what it shows (of the points the boundary faces use, for a mesh). A
face with a coordinate that is not finite, or too far from the rest to be drawn, is left out,
and the snapshot says how many were (`left_out`).

## Snapshots

`scene.snapshot` draws the scene with the CPU renderer and returns a PNG image: flat shading
with the light at the eye, both sides of a face lit alike, an outline where patches meet, and
the selection in orange (in front of a patch of another region in the same place), over a dark
background with a grid on the ground and the origin's planes and axes, as the desktop app's view
([the grid, the origin and the dark view](desktop.md#the-grid-the-origin-and-the-dark-view)).

| Parameter | Default | Does |
|---|---|---|
| `view` | `iso` | `iso` (from the front, the right and above), `top`, `bottom`, `front` (from −y), `back`, `left` (from −x), `right` |
| `azimuth`, `elevation` | the view's | Degrees: azimuth 0 from −y, 90 from +x; elevation from −90 (below) to 90 (above) |
| `zoom` | 1 | 1 frames the whole scene; 2 is twice as close |
| `width`, `height` | 1280, 800 | Pixels: each at most 8192, together at most 4096 × 4096 |
| `edges` | `false` | Draws the faces' edges (the cells at the boundary) |
| `colour_by` | `patch` | `patch` (each patch or region its own colour) or `object` (each mesh region or surface) |
| `regions`, `surfaces` | see above | Which mesh regions and surfaces |
| `reload` | `false` | Reads the files again (after OpenFOAM rewrote a mesh) |
| `display` | none | Each layer as surfaces, wireframe or points, or hidden, with an opacity ([display modes](#display-modes)) |
| `grid` | `true` | The grid on the ground (z = 0), under the scene |
| `origin` | `true` | The origin's xy, xz and yz planes and its axes |
| `dark` | `true` | A dark background; `false` a light one |

The scene is built the first time it is needed: each region's mesh (only its boundary: its
points that its boundary faces use, boundary faces and patches) and each surface is read once,
the missing ones in parallel, and kept, also while it is hidden. So a pick after a snapshot, or
showing a region again, reads nothing, while what is shown stays near where the scene is centred.
Both keep to the session's [memory budget](#memory): readers start only while their estimates
fit, and what is kept of hidden regions and surfaces is dropped past its share, the ones shown
least recently first (and read again when shown). Points are kept in single
precision relative to the middle of what is shown; when that moves more than 10 km (a mesh in
local and a surface in projected coordinates, say), what is shown is read again around the new
middle and what was kept of hidden ones is dropped, so that nothing loses precision. The scene
follows the surfaces and patch lists commands change (a repair, a rename, an undo). A mesh
rewritten on disk (by OpenFOAM) is read again only with `reload`, which drops what was kept of
every region and surface and reads what is shown again.

## Display modes

`display` (in `scene.snapshot`, `select.pick`, `select.rect` and `select.brush`) says how each layer is drawn: a
list of entries, each naming a layer and giving it a `mode`, an `opacity`, or both:

| Layer | Entry |
|---|---|
| A region's mesh | `{"region": "air"}` |
| One of its patches | `{"region": "air", "patch": "inlet"}` |
| Its patches in a patch group (`inGroups`) | `{"region": "air", "group": "buildings"}` |
| A surface | `{"surface": "walls.stl"}` |
| One of its regions | `{"surface": "walls.stl", "solid": "roofs"}` |

| `mode` | Draws |
|---|---|
| `surface` (default) | Filled, shaded faces |
| `wireframe` | The faces' edges (not the diagonals that split a face into triangles) |
| `points` | The faces' points |
| `hidden` | Nothing; picking passes over it |

`opacity` goes from 0 to 1 (default, opaque). Later entries win over earlier ones, mode and
opacity each: `[{"region": "air", "mode": "wireframe"}, {"region": "air", "patch": "ground",
"mode": "surface"}]` draws the air region as a wireframe and its ground as a surface. Each layer
named must be in the case.

```sh
umcf-cli run --case my-case \
  --cmd 'scene.snapshot={"display": [{"region": "air", "opacity": 0.3}, {"region": "buildings", "mode": "wireframe"}]}'
```

Opaque surfaces are drawn first. Edges and points go over them where nothing opaque is in front,
and transparent surfaces last, the farthest first, each blended over what is behind it; the
nearest on each pixel is found first and blended last, so that it is exact (of layers within
1e-4 of the distance of the nearest, as the faces of two regions on their interface, the first
region's, on every pixel). The outlines where one class meets another are drawn on the nearest
transparent layer too, as much as it covers what is behind it: at an opacity near 1 a layer is
drawn as it is opaque.

What is hidden is not picked, and doesn't hide anything. Everything else is picked, nearest
first: a click on a transparent surface picks it, and `depth` picks what is behind it. A
wireframe or points layer is picked where its faces are, between the edges or points drawn. A
rectangle (below) sees through what isn't an opaque surface. It takes a transparent surface, a
wireframe or points where they are seen, and also what is seen through them.

## A cut through the mesh

`cuts` shows the inside of a region's mesh: the cells a plane crosses, each whole (a "crinkle"
cut: no cell is cut in two, so the cells are seen as the mesh has them), one plane a region:

```json
{"cuts": [{"region": "air", "point": [115, 0, 0], "normal": [1, 0, 0]}],
 "display": [{"region": "air", "mode": "hidden"}], "edges": true}
```

- **Which cells**: those with points on the plane's negative side (against `normal`) and points
  on it or past it. A plane lying on the faces between two layers of cells (an axis-aligned
  plane on a block mesh's lines, say) takes one layer, the one behind it; turning `normal`
  round takes the other.
- **What is drawn**: the layer's skin, each face between a cut cell and one not cut, or of the
  boundary, once; with `edges` the cells' edges. The faces between two cut cells lie inside the
  layer and are left out. Its classes are the cell zones (`cells` for the cells in none), so a
  solid's materials show each in its colour.
- **It is an object of its own** (`{"cut": "air"}` in the snapshot's `objects`), drawn as a patch
  is, by the GPU and the CPU. The region's boundary, which hides it, is hidden with
  `{"region": "air", "mode": "hidden"}` (a region's entry doesn't reach its cut); a cut has
  entries of its own, `{"cut": "air", "mode": "wireframe", "opacity": 0.5}`. Picks and selections
  pass through it (its faces are inside the mesh: no patch to set).
- **Reading**: the region's whole mesh (`points`, `faces`, `owner`, `neighbour`, `cellZones`) is
  read the first time it is cut and kept while it is: moving the plane builds the cut again,
  reading nothing. Uncut, the mesh is kept aside with the hidden objects ([memory](#memory)).
  The 26 000-cell air region of the small 3×3 case is cut in 0.14 s; the Hong Kong project's
  16-million-cell one is read in about 24 s and holds 4.1 GB, and each move of the plane takes
  about 4 s more (every face is looked at again, on one core).

On the command line, `umcf-cli render --cut air:x=115` (repeatable, one a region; `:z=2` in a
single-region case) cuts across an axis and hides the region's boundary. In the desktop app it is
[the cut card](desktop.md#a-cut-through-the-mesh).

### What a check found

A region's cut can show, in place of a plane's cells, the faces and cells `mesh.check` found in
it last:

```json
{"cuts": [{"region": "air", "found": true}], "display": [{"region": "air", "mode": "hidden"}]}
```

- **What is drawn**: for each problem the check found (skew faces, faces more than 70°
  non-orthogonal, open cells, with `all_geometry` concave cells and the others it adds), and
  for each of checkMesh's sets of faces or cells in `polyMesh/sets` that isn't stale, its first
  1000 faces or cells (those the check lists), a face as it is and a cell by all its faces. Each
  problem is a class of its own, in its own colour (its classes are the problems' names, such
  as `skew faces` or `checkMesh: faces with skewness above 4`). Points (`nearPoints`,
  `shortEdges`) are not drawn.
- **Before a check** of the region the cut is refused (*run mesh.check on it first*); a region
  whose mesh the check couldn't read has nothing found. A check of the region again replaces what
  was found; opening the case again or reading its meshes again (`reload`) drops it, as the
  labels may no longer be of the same faces and cells.
- **Reading**: `mesh.check` reads the whole mesh to check it and hands it to the scene, kept
  while it fits in what the budget for hidden objects leaves ([memory](#memory)): showing what it
  found then reads nothing. A mesh that doesn't fit is read again when it is shown, as for a
  plane's cut.

`umcf-cli render --found air` checks the region's mesh and draws what it found, the region's
boundary hidden.

## Picking

`select.pick` casts a ray through a pixel of the image `scene.snapshot` draws with the same
parameters (`pixel: [x, y]`, from the top left), or along a ray in world coordinates
(`ray: {"origin": [x, y, z], "direction": [x, y, z]}`). It lists everything the ray passes
through, nearest first, once per patch or region at its nearest hit (`hits`, each with its face
or triangle, the distance and the point), and selects one of them:

| `mode` | Does |
|---|---|
| `replace` (default) | The selection becomes what was picked; nothing under the cursor clears it |
| `add` | Adds it |
| `toggle` | Adds it, or takes it out if it was selected |
| `none` | Only lists what is under the cursor |

With `faces: true` it picks the mesh face under the cursor instead of its patch: the hits are
then one per face (with the patch it is in), and the selection gets `{"region": "air", "face":
75548}`. Surfaces have no faces to pick: their triangles are passed over. Selected faces are drawn
in the selection colour, and [`patch.create`](meshes.md#new-patches) makes a patch of them;
`bc.set` and `patch.set` act on the selected patches and leave the faces out. A mesh read again
(`"reload": true`, after OpenFOAM rewrote it) may number its faces otherwise, so the faces picked
on it leave the selection.

`depth` picks the one behind: 0 (default) is the nearest, 1 the next ("select other": a patch
hidden by another, an internal patch), up to 63; 64 are listed at most. A `depth` past what is under the cursor
changes nothing.

Picking uses a bounding volume hierarchy of each object's triangles, built the first time a
ray is cast, on the full mesh or surface.

## Selecting in a rectangle

`select.rect` selects what is in a rectangle of the same image (`rect: [x0, y0, x1, y1]`, two
opposite corners in pixels from the top left; a pixel is in it when its middle is):

| `through` | Takes |
|---|---|
| `false` (default) | What is seen in it: the faces in front at one of its pixels or more. A face smaller than a pixel can be missed: zoom in, or go through |
| `true` | Everything in it, seen or behind something: the faces whose middle (their area-weighted centre) is inside it, on the image, and in front of the camera |

Without `faces` it selects the patches and surface regions of those faces; with `faces: true`,
the mesh faces themselves (surfaces are passed over and hide nothing, as for a click). `mode` is `replace` (default), `add`,
`remove` or `none` (only count). It returns how many were in the rectangle (`in`), up to 1000 of
them (`items`), and the size of the selection (`selected`).

```sh
umcf-cli run --case my-case \
  --cmd 'select.rect={"view": "top", "width": 1280, "height": 800, "rect": [300, 200, 700, 500], "faces": true, "through": true}' \
  --cmd 'patch.create={"name": "plaza", "type": "wall"}'
```

## Selecting with a brush

`select.brush` selects what is under a brush stroke over the same image: `stroke`, the points the
brush went through (`[[x, y], ...]`, pixels from the top left, 1 to 4096 of them; one point is a
dab), and `radius` (pixels, default 10, at most 2048). A pixel is under it when its middle is
within the radius of the stroke's path, and the pixel under a point always is. It paints the mesh
faces by default (surfaces passed over, as for a click); `faces: false` selects the patches and
surface regions instead. `through`, `mode` and what it returns are as for
[a rectangle](#selecting-in-a-rectangle): the faces seen at one of the stroke's pixels or more,
or (`through: true`) every face whose middle lies in a pixel under it.

```sh
umcf-cli run --case my-case \
  --cmd 'select.brush={"view": "top", "width": 1280, "height": 800, "stroke": [[300, 400], [500, 420], [700, 380]], "radius": 25}' \
  --cmd 'patch.create={"name": "path", "type": "wall"}'
```

## The camera

`scene.snapshot` returns the camera it drew with (`camera`: `target`, `distance`, `azimuth`,
`elevation`, `fov`). `select.pick`, `select.rect` and `select.brush` (and `scene.snapshot` itself) take it back
as `camera`, in place of `view`, `azimuth`, `elevation` and `zoom`, to work on exactly that
view: the desktop app sends its own camera this way. `fov` may be left out (30°).

## Selecting by name

`select.set` sets the selection from a list of `items`: patches as
`{"region": "air", "patch": "inlet"}` (region `""` in a single-region case) and surface regions
as `{"surface": "walls.stl", "solid": "roofs"}`, and boundary faces of a mesh as
`{"region": "air", "face": 75548}`, by label. Each must exist. `mode` is `replace`
(default), `add` or `remove`. `select.clear` empties it.

The selection belongs to the session, not the case: opening a case clears it, and undo doesn't
change it. `select.*` commands go into the journal, so a journal replays the same selections.

## On the command line

```sh
umcf-cli render path/to/case --out case.png [--view top] [--size 1600x1000] [--edges]
umcf-cli render path/to/case --out plain.png --no-grid --no-origin --light
umcf-cli render path/to/case --out walls.png --surface walls.stl --pick 640,400
umcf-cli render path/to/case --out inlet.png --region air --select air/inlet
```

`--select` takes `region/patch`, `patch` in a single-region case, or `file.stl/region`.
`--pick x,y` selects what is under that pixel and lists everything under it:

```text
under the cursor: region roofs of constant/triSurface/walls.stl, 1634.832 m away
region roofs of constant/triSurface/walls.stl (1 under the cursor); 1 selected
walls.png: 1280 × 800 of surface constant/triSurface/walls.stl (320948 triangles), 1 selected
```

## Memory

The session plans with the memory free when the program starts, as Linux, Windows or macOS
tell it (on Linux no more than the control groups it runs in leave, with cgroup v2: a
container, a SLURM job, `systemd-run -p MemoryMax=`; the page cache they can drop counts as
free, shared memory doesn't). Where the free memory can't be told it plans with half the
total, and with 1 GB where neither can. `UMCF_MEMORY`, set to a whole number of megabytes
above 0, takes their place (`UMCF_MEMORY=4000 umcf-gui case`; another value is ignored):

- **Half of it for reading at once.** Each region's read is estimated from the counts its
  `owner` header notes: about 32 bytes per point of the mesh and per boundary face, and for a
  binary mesh (read whole, not as a stream) 24 more per point and its `faces` file's size.
  Without the counts it is estimated from the sizes of its `points` and `faces` files, a
  surface from its file's size; a read whose size can't be told runs alone. The biggest start
  first, and a reader starts only while its estimate and those running fit. One always runs,
  so a region bigger than the budget is read, alone. Building the drawn objects from what was
  read is planned the same way, from the size of each object.
- **A quarter for what the scene keeps while it is hidden.** Past it, the regions and surfaces
  shown least recently are dropped, and read again when they are shown. A region's whole mesh,
  read for [a cut](#a-cut-through-the-mesh), is kept while the region is cut, and in what the
  hidden objects leave of the quarter once it isn't (the least recently cut dropped first). Its
  read is estimated as about 24 bytes a point and 28 a face, twice that for a binary mesh (its
  files held besides), and 24 more a point for an ASCII one (its points are parsed into a list
  that grows). Building the cut adds about 21 bytes a cell and 12 a point. The whole mesh
  `mesh.check` read is handed to the scene while it fits in what the hidden objects and the
  whole meshes kept leave of the quarter ([what a check found](#what-a-check-found)).

The estimates are plans, not limits: what is shown is always kept, however big. The Hong Kong
project's five regions (16, 8.4, 8.5, 3.2 and 1.9 million cells) are drawn in 4.3 s with at most
1.27 GB with the memory of a 32 GB machine, 5.9 s and 1.15 GB with `UMCF_MEMORY=2000`, 7.0 s and
1.05 GB with `UMCF_MEMORY=1000` (all at once and every point kept: 5.95 s and 1.84 GB).

## Limits

- Drawing a mesh reads its boundary only, and keeps only the points its boundary faces use
  (a [cut](#a-cut-through-the-mesh) reads the whole mesh).
  ASCII files are read as a stream, compressed or not (the file isn't held, nor the text of its
  points or internal faces); a binary file is held whole while it is read, and every point of
  the mesh is held until its boundary's are picked out. The 16-million-cell air region of the HK
  project takes 3.5 s and 0.87 GB the first time, all five of its regions 4.3 s and 1.27 GB
  ([memory](#memory)). Drawing and picking boundary triangles is quick: the 573 000 of a 2.8-million-cell
  air region of a 3×3 street-grid case in 0.14 s.
- No level of detail; the CPU renderer draws every triangle on one core, and sorts transparent
  triangles one by one ([display modes](#display-modes)); the desktop app's GPU view draws them
  without sorting ([the GPU view](desktop.md#the-gpu-view)).
- Selections are whole patches, surface regions and mesh faces (`faces: true`), one at a time, by
  a rectangle (`select.rect`) or with a brush (`select.brush`). Picking cells is not there yet.
