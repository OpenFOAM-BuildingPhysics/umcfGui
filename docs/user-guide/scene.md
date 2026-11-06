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
the selection in orange (in front of a patch of another region in the same place).

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

The scene is built the first time it is needed: each region's mesh (only its boundary: its
points, boundary faces and patches) and each surface is read once, the missing ones in parallel,
and kept, also while it is hidden. So a pick after a snapshot, or showing a region again, reads
nothing, while what is shown stays near where the scene is centred. Points are kept in single
precision relative to the middle of what is shown; when that moves more than 10 km (a mesh in
local and a surface in projected coordinates, say), what is shown is read again around the new
middle and what was kept of hidden ones is dropped, so that nothing loses precision. The scene
follows the surfaces and patch lists commands change (a repair, a rename, an undo). A mesh
rewritten on disk (by OpenFOAM) is read again only with `reload`, which drops what was kept of
every region and surface and reads what is shown again.

## Display modes

`display` (in `scene.snapshot`, `select.pick` and `select.rect`) says how each layer is drawn: a
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
and transparent surfaces last, the farthest first, each blended over what is behind it.

What is hidden is not picked, and doesn't hide anything. Everything else is picked, nearest
first: a click on a transparent surface picks it, and `depth` picks what is behind it. A
wireframe or points layer is picked where its faces are, between the edges or points drawn. A
rectangle (below) sees through what isn't an opaque surface. It takes a transparent surface, a
wireframe or points where they are seen, and also what is seen through them.

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

## The camera

`scene.snapshot` returns the camera it drew with (`camera`: `target`, `distance`, `azimuth`,
`elevation`, `fov`). `select.pick` and `select.rect` (and `scene.snapshot` itself) take it back
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

## Limits

- Drawing a mesh reads its boundary only, but keeps all of its points. ASCII files are read as a
  stream, compressed or not (the file isn't held, nor the text of its points or internal faces);
  a binary file is held whole while it is read. The 16-million-cell air region of the HK project
  takes 3.8 s and 0.97 GB the first time, all five of its regions 4.5 s and 1.8 GB, read in
  parallel. Drawing and picking boundary triangles is quick: the 573 000 of a 2.8-million-cell
  air region of a 3×3 street-grid case in 0.14 s.
- No level of detail; the renderer draws every triangle on one core, and sorts transparent
  triangles one by one ([display modes](#display-modes)).
- Selections are whole patches, surface regions and mesh faces (`faces: true`), one at a time or
  by a rectangle (`select.rect`). A brush and picking cells are not there yet.
