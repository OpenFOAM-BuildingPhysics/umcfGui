# The desktop app

`umcf-gui` shows a case in 3D and runs the same commands as `umcf-cli`: every button, menu entry
and click in the view is a command of the [registry](command-line.md#the-commands), so what it
does is in the edit history and can be undone, and a script can do the same.

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
| `--dark` | The dark theme |
| `--size <w>x<h>` | The window's size (default 1440x900) |
| `--screenshot <file.png>` | Writes a PNG of the window once it has drawn, then quits |

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
  done; a click on a chip undoes or redoes up to it.

Shortcuts come from the commands: Ctrl+O open, Ctrl+S save, Ctrl+Z undo, Ctrl+Shift+Z redo, F7
check. Escape closes a card or the search.

## Limits

- The 3D view is drawn by the CPU renderer, at the window's resolution (at half of it while the
  view turns). It keeps up with surfaces and meshes of a few hundred thousand boundary faces;
  a GPU view comes later.
- A command's parameters are typed as JSON; forms for them come with the commands that need
  them most (boundary conditions, meshing).
- Opening a large case, reading a mesh and running a check happen while the window waits.
