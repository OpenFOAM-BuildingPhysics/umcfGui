# 3D scene and selection

umcfGui shows a case in 3D as its meshes' boundary patches and its STL surfaces. What is drawn
or picked is always a patch or a surface region, never just a pixel: each triangle knows the
mesh face or STL triangle it came from. Picking selects what later commands act on (boundary
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
relative to its own middle.

## Snapshots

`scene.snapshot` draws the scene with the CPU renderer and returns a PNG image: flat shading
with the light at the eye, both sides of a face lit alike, an outline where patches meet, and
the selection in orange (in front of a patch of another region in the same place).

| Parameter | Default | Does |
|---|---|---|
| `view` | `iso` | `iso` (from the front, the right and above), `top`, `bottom`, `front` (from −y), `back`, `left` (from −x), `right` |
| `azimuth`, `elevation` | the view's | Degrees: azimuth 0 from −y, 90 from +x; elevation from −90 (below) to 90 (above) |
| `zoom` | 1 | 1 frames the whole scene; 2 is twice as close |
| `width`, `height` | 1280, 800 | Pixels, at most 8192 |
| `edges` | `false` | Draws the faces' edges (the cells at the boundary) |
| `colour_by` | `patch` | `patch` (each patch or region its own colour) or `object` (each mesh region or surface) |
| `regions`, `surfaces` | see above | Which mesh regions and surfaces |
| `reload` | `false` | Reads the files again (after OpenFOAM rewrote a mesh) |

The scene is built the first time it is needed and kept while the same things are shown, so a
pick after a snapshot doesn't read the mesh again. It follows the surfaces commands change (a
repair, an undo); a mesh rewritten on disk needs `reload`.

## Picking

`select.pick` casts a ray through a pixel of the image `scene.snapshot` draws with the same
parameters (`pixel: [x, y]`, from the top left), or along a ray in world coordinates
(`ray: {"origin": [x, y, z], "direction": [x, y, z]}`). It lists everything the ray passes
through, nearest first, once per patch or region (`hits`, each with its face or triangle, the
distance and the point), and selects one of them:

| `mode` | Does |
|---|---|
| `replace` (default) | The selection becomes what was picked; nothing under the cursor clears it |
| `add` | Adds it |
| `toggle` | Adds it, or takes it out if it was selected |
| `none` | Only lists what is under the cursor |

`depth` picks the one behind: 0 (default) is the nearest, 1 the next ("select other": a patch
hidden by another, an internal patch).

Picking uses a bounding volume hierarchy of each object's triangles, built the first time a
ray is cast, on the full mesh or surface.

## Selecting by name

`select.set` sets the selection from a list of `items`: patches as
`{"region": "air", "patch": "inlet"}` (region `""` in a single-region case) and surface regions
as `{"surface": "walls.stl", "solid": "roofs"}`. Each must exist. `mode` is `replace`
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

- Drawing a mesh reads all of it, not only its boundary: the 2.8-million-cell air region of
  the 3×3 case takes about 5 s and 1.3 GB, almost all of it reading. Its 573 000 boundary
  triangles are drawn and picked in well under a second.
- No level of detail and no transparency yet; the renderer draws every triangle on one core.
- Selections are whole patches and surface regions. Single faces and cells come with the
  commands that need them.
