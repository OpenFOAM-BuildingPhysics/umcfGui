# The desktop app

`umcf-gui` shows a case in 3D and runs the same commands as `umcf-cli`: every change it makes to
the case is a command of the [registry](command-line.md#the-commands), so it is in the edit
history and can be undone, and a script can do the same. Selecting is a command too (in the
journal, not in the undo history); what the view shows, the camera and the theme are the
window's own.

![umcfGui with the 3×3 case: the browser on the left, the buildings surface in the view with its roofs selected, the properties and the results of a case check on the right](../images/desktop.png)

```sh
umcf-gui path/to/case
umcf-gui path/to/case --show walls.stl --cmd 'select.set={"items": [{"surface": "walls.stl", "solid": "roofs"}]}'
umcf-gui path/to/case --size 1440x900 --screenshot shot.png
```

| Option | Does |
|---|---|
| `<case>` | Opens the case (`case.open`) |
| `--cmd '<id>=<json>'` | Runs a command once the case is open, as `umcf-cli run --cmd` does; repeat it for more |
| `--show <name>` | What the 3D view shows first: a mesh region or a surface (`walls.stl`); repeat it for more |
| `--open <id>` | Opens a command's card at the start, as its ribbon button does (`--open bc.set`) |
| `--dark` | The dark theme |
| `--size <w>x<h>` | The window's size (default 1440x900) |
| `--screenshot <file.png>` | Writes a PNG of the window once it has drawn, then quits; fails without one when the case, a `--cmd` or a `--show` name does |

## The window

- **The bar along the top**: open, save, undo and redo, the open case (`•` when it has unsaved
  changes), and the theme.
- **The workspace** (on the left of the ribbon): *Setup* holds what there is today. *Run* and
  *Results* are planned. Every workspace works on the same case and selection; only the ribbon
  changes.
- **The ribbon**: the commands in tabs and groups (Case, Geometry, Mesh, Boundary conditions,
  View). A command that can't run now is grey, and its tooltip says why. A command that needs
  parameters opens its card on the right: what it takes, and its parameters as JSON
  (`{"endTime": 7200}`); *OK* runs it. The checks and *Summary* run at once.
- **Search commands** (Ctrl+P or F1): every command, filtered as you type; Enter runs the first
  that can run.
- **The browser** (left): the case's regions and their patches (type and number of faces), and
  its STL surfaces and their regions. The eye says whether the 3D view shows a region or a
  surface. A click selects a patch or a surface region; Ctrl+click adds it or takes it out.
- **The 3D view**: the meshes' boundary patches or the surfaces, each patch in its own colour,
  the selection in orange (see [3D scene and selection](scene.md)). Drag to turn the view (or
  pan, with the navigation bar's *Pan*); a right or middle drag pans; the wheel zooms. A click
  selects what is under the cursor (`select.pick`), Ctrl+click toggles it, Shift+click adds it.
  The name of what is under the cursor shows in the history bar.
- **The axis triad** (top right): click an axis to look along it.
- **The navigation bar** (bottom): orbit or pan, zoom, fit, the standard views, the faces' edges.
- **Properties** (right): the selected patches with their type, size and each field's boundary
  condition for them (by name or patch group; one given by a pattern is matched by the
  [case check](checks.md), not shown here).
- **Results** (right): the last command's message and its diagnostics; it opens when a command
  reports some.
- **History** (bottom): one chip per edit that can be undone, the marker after the last one
  done; a click on a chip undoes or redoes up to it. When there are more than fit, the ones
  nearest the marker are shown, with the number left out on each side.

Shortcuts come from the commands: Ctrl+O open, Ctrl+S save, Ctrl+Z undo, Ctrl+Shift+Z redo, F7
check. Escape closes the parameter card, the condition card, the results and the search.

## Boundary conditions

Select the patches in the view or the browser (Ctrl+click adds one), then *Set condition* on the
*Boundary conditions* tab. The card shows:

- the **field**: the fields every selected patch's region has at the start time, `U`, `T`,
  `p_rgh`, `p` and the turbulence fields first;
- the **type**, typed or picked from the suggestions under it: OpenFOAM's common types and the
  solver's own for the field (scalar or vector) and the case's OpenFOAM version, from the
  [catalog](../solver-coverage.md). Any other type can be typed; the case check judges it;
- the **value** (`uniform 300`, `uniform (2 0 0)`), and other entries, one `key value` per line
  (`inletValue uniform 0`);
- what each patch has **now**, and whether the entries already there stay (the same type) or the
  patch gets a new entry.

*Set* runs [`bc.set`](fields.md#setting-boundary-conditions) for the selected patches. It is one
edit in the history, undone with Ctrl+Z, and the results card lists what the case check finds
about the new conditions.

![The condition card for two patches of the 3×3 case's air region](../images/bc-card.png)

## Limits

- The 3D view is drawn by the CPU renderer, at the window's resolution (at half of it while the
  view turns). It keeps up with surfaces and meshes of a few hundred thousand boundary faces;
  a GPU view comes later.
- A command's parameters are typed as JSON, apart from boundary conditions, which have a card
  of their own; forms for the others come with meshing.
- Opening a large case, reading a mesh and running a check happen while the window waits.
- The window reads the case's files when it needs them, not when they change. An STL copied
  into `constant/triSurface` shows in the browser after the next command; a file changed outside
  the window shows when the case is opened again. A scene that failed to build (a mesh not
  written yet) is read again by the next command or *Fit*.
