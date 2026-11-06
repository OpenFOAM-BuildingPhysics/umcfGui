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
  (`{"endTime": 7200}`); *OK* runs it. The checks and *Summary* run at once. Commands without a
  place of their own are in a *More* group of their tab: *Workflow description* and *Write workflow
  description* on *Case*, *Tree crowns* on *Geometry*.
- **Search commands** (Ctrl+P or F1): every command, filtered as you type; Enter runs the first
  that can run.
- **The browser** (left): the case's regions and their patches (type and number of faces), and
  its STL surfaces and their regions. The eye says whether the 3D view shows a region or a
  surface. A click selects a patch or a surface region; Ctrl+click adds it or takes it out. A
  right click on a region, a patch, a surface or a surface region shows it as surfaces,
  wireframe or points, or hides it, and sets its opacity ([display modes](scene.md#display-modes)).
  A patch's menu has its patch groups too. The row says how it is drawn when that isn't the
  default, and *Reset* draws it as what it is part of again. What is hidden is not picked.
- **The 3D view**: the meshes' boundary patches or the surfaces, each patch in its own colour,
  the selection in orange (see [3D scene and selection](scene.md)). Drag to turn the view (or
  pan, with the navigation bar's *Pan*); a right or middle drag pans; the wheel zooms. A click
  selects what is under the cursor (`select.pick`), Ctrl+click toggles it, Shift+click adds it.
  The name of what is under the cursor shows in the history bar.
- **The axis triad** (top right): click an axis to look along it.
- **The navigation bar** (bottom): orbit, pan or *Box select*, zoom, fit, the standard views, the
  faces' edges, *Pick faces* and *Select through*.
  - *Pick faces*: a click then picks the mesh face under it rather than its patch (Ctrl+click
    adds or takes away one, Shift+click adds one), drawn in the selection colour.
  - *Box select*: a drag draws a rectangle that selects what is in it (`select.rect`): the
    patches, or with *Pick faces* the mesh faces. Shift adds to the selection, Ctrl takes away.
    A click still picks, and a right or middle drag still pans.
  - *Select through*: the rectangle takes everything in it, hidden too (the faces whose middle is
    in it), not only what is seen.
- **Properties** (right): the selected patches with their type, size and each field's boundary
  condition for them (by name or patch group; one given by a pattern is matched by the
  [case check](checks.md), not shown here), and the picked faces, counted by region.
- **Results** (right): the last command's message and its diagnostics, and for
  `workflow.show` and `workflow.import` the description (its problems and the import's notes
  first, then the settings and the regions); it opens when a command reports some.
- **History** (bottom): one chip per edit that can be undone, the marker after the last one
  done; a click on a chip undoes or redoes up to it. When there are more than fit, the ones
  nearest the marker are shown, with the number left out on each side.

*Open* (Ctrl+O) asks for the case's folder (the one holding `system/`, `constant/` and the time
directories) in the system's own dialog, over the app's window: the Windows file dialog, the
macOS open panel, and on Linux the desktop portal (`xdg-desktop-portal`, which GNOME, KDE and
most desktops run) or else `zenity`. A folder that doesn't open (not a case, or the open case
has unsaved changes) brings up the *Open* card with the path and why, to change it or to open
anyway (`"discard": true`). On a Linux system with neither dialog (a window manager without a
desktop, and no `zenity`) the card asks for the path instead; a desktop whose portal service isn't
running shows no dialog at all, so install `zenity` or `xdg-desktop-portal` there.
`umcf-gui <case>` opens a case from the command line on every system.

Shortcuts come from the commands: Ctrl+O open, Ctrl+S save, Ctrl+Z undo, Ctrl+Shift+Z redo, F7
check. Escape closes the parameter card, the condition card, the results and the search (not a card
whose command waits or runs: its outcome closes it).

## Long work

Long work runs beside the window, which stays in use: you can turn the 3D view, browse the case,
open cards and look at the results while it runs. The status bar shows what runs, how far it is
and *Cancel*: a bar that fills as the files are done where the job knows how many (reading the
field files: "21 of 30"), and the megabytes read so far and the seconds it has run. This holds
for the ribbon's commands, a command card's *OK*, the *Set* of the boundary condition, patch and
new patch cards, going back or forward in the history bar, reading the field files, and reading
the meshes and surfaces the 3D view shows.

What you ask for while a job runs waits, and runs after it, in order: a command, a card's *Set*
(the card shows its values as sent, *Setting…*, and closes when the command worked or shows the
error; it acts on the patches or faces the card shows, whatever is selected by the time it
runs), a pick in the view, an undo. The status bar says how much waits. A reading of the 3D view
not started yet gives way to other work and follows it.

Work that only reads (the checks, the summary, reading the fields or what the view shows) has
*Cancel*: the case's files are read no more, the work stops, and the case is as it was. Work
that changes the case (an edit, an undo) runs to its end.

Opening and saving a case take the whole window from the moment they are asked for (behind a
job, too): what it would show is the case before, and what was asked meanwhile would act on the
case opened. Opening has *Cancel* (the case open stays); a save runs to its end, and the window
doesn't close before it has.

A case opens quickly, with its field files listed only: the meshes and surfaces the view shows
are read first, so the case shows at once, then the field files, as a job of their own. Until
they are read, their boundary conditions aren't known (a check says so: `fields.pending`), and
conditions and patches can't be changed (`bc.set`, `patch.set` and `patch.create` say to read
them first): the field files change with them. *Read fields* (`case.fields`) reads them again
after a *Cancel*. A case named on the command line (`umcf-gui <case>`) opens the same way; with
`--cmd`, its fields are read as it opens, for the commands to act on.

## Patches

*Edit patch* (on the *Mesh* and *Boundary conditions* tabs) opens a card for the selected
patches, filled from the first: the name (one patch at a time), the type, typed or picked from the
suggestions under it, the groups, other entries (`neighbourRegion air`), and what each patch is
now. *Set* sends only what was changed, as [`patch.set`](meshes.md#editing-patches): a renamed
patch stays selected, and its field files follow.

*New patch* (on the *Mesh* tab) opens a card that prepares a patch, as
[`patch.create`](meshes.md#new-patches): the name, the type, the groups, and the faces: those
picked in the view (chosen when faces are picked), or, of the selected patches, those whose centre
is in a box (from x y z, to x y z) or within a distance of a surface of the case (one of its
solids, or all of it). *Prepare* writes the face set and adds the
patch to the dictionary of the patches prepared on this mesh; the mesh gets them when
`createPatch` runs once with that dictionary.

## Boundary conditions

Select the patches in the view or the browser (Ctrl+click adds one), then *Set condition* on the
*Boundary conditions* tab. The card shows:

- the **field**: the fields every selected patch's region has at the start time, `U`, `T`,
  `p_rgh`, `p` and the turbulence fields first;
- the **type**, typed or picked from the suggestions under it: OpenFOAM's common types and the
  solver's own for the field (scalar or vector) and the case's OpenFOAM version, from the
  [catalog](../solver-coverage.md). Any other type can be typed; the case check judges it;
- the **value** (`uniform 300`, `uniform (2 0 0)`), filled in from the first patch. Left as it
  is, it goes only to patches that get a new entry, and a patch keeping its entry keeps its own
  value; changed, it goes to every selected patch;
- other entries, one `key value` per line (`inletValue uniform 0`);
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
- *Cancel* stops reading a mesh's ASCII `points` or `faces` and field files within a megabyte,
  but a file read whole (binary meshes, the mesh check's) only once it is read. The progress bar
  counts the files of the field reading; other work shows the megabytes it has read and the
  seconds it has run, not how far it is.
- The window reads the case's files when it needs them, not when they change. An STL copied
  into `constant/triSurface` shows in the browser after the next command; a file changed outside
  the window shows when the case is opened again. A scene that failed to build (a mesh not
  written yet) is read again by the next command or *Fit*.
