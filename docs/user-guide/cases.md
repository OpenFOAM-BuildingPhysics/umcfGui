# Cases

## Solver cases and meshing steps

A UMCF project is rarely one case. The multi-region mesh is usually built in a chain of
single-region cases before the solver runs, for example:

1. a terrain mesh with `blockMesh`,
2. buildings and trees cut in with `snappyHexMesh`,
3. facades, ground and roofs extruded into solid regions with `extrudeMesh`,
4. a run that initialises the solid regions,
5. the urbanMicroclimateFoam run itself.

umcfGui calls a case that defines its regions a **solver case**, and any other case a
**meshing step**. For both it lists the OpenFOAM utilities the case has dictionaries for
(`blockMesh`, `snappyHexMesh`, `extrudeMesh`, `topoSet`, `createPatch`, `changeDictionary`,
`setFields`, `viewFactorsGen`, `decomposePar`, …). Variants such as `extrudeMeshDict.east` or
`topoSetDict_cellZones` count as the same utility.

## Regions

A solver case has three kinds of region:

| Kind | What the solver does there |
|---|---|
| `fluid` | CFD: turbulent air flow, heat and moisture transport |
| `solid` | Heat and moisture (HAM) transport in porous building materials |
| `vegetation` | Leaf energy balance and its coupling to the air |

Region names are the project's choice: one project has `street roofs buildings`, another
`ground facades roofs`. umcfGui never assumes names. Where the region list lives depends on
the OpenFOAM version:

=== "OpenFOAM-12"

    In `system/controlDict`:

    ```c++
    regions
    {
        fluid           ( air );
        solid           ( street roofs buildings );
        vegetation      ( vegetation );
    }
    ```

=== "OpenFOAM-8"

    In `constant/regionProperties`:

    ```c++
    regions
    (
        fluid       (air)
        solid       (street roofs buildings)
        vegetation  (vegetation)
    );
    ```

umcfGui warns when a listed region has no `constant/<region>` directory, when a name is not
a valid directory name, and when the list sits in the file the case's version doesn't read
(the solver would then not find its regions).

## Opening a case

Opening a case reads what an editor needs, and nothing more:

- the run controls from `system/controlDict` (`application`, `startFrom`, `startTime`,
  `endTime`, `deltaT`, `writeControl`, `writeInterval`), as written, `$macros` included;
- the time directory the run starts from. `startFrom` is followed as OpenFOAM follows it:
  `firstTime`, `latestTime`, or the directory at `startTime`;
- for each region, the patches of its mesh (`constant/<region>/polyMesh/boundary`) and, for
  every field of the start time, its class, its `internalField` and the boundary condition of
  each `boundaryField` entry (patch, group or quoted pattern). Field values are summarised:
  `nonuniform List<scalar> 2400`, not 2400 numbers;
- every dictionary in `system/`, `constant/` and each region's `system/<region>/` and
  `constant/<region>/`, as an editable document.

What can't be read is listed, and the case opens anyway:
- a dictionary that doesn't parse, such as a half-edited backup file;
- a field file too large to summarise;
- a start time without a directory;
- a broken `boundary` file.

## Editing and saving

An edit changes one entry of one dictionary. The rest of the file stays as it was:
comments, blank lines, alignment, `#include`s and `$macros`.

Saving writes only the dictionaries that changed:

1. **All are checked first.** Each file must still be what was opened (or last saved). If
   anyone (or a script) changed one of them on disk in the meantime, nothing is written, not
   even the other files.
2. **Backups.** Each file's original is copied into the session's backup directory under
   `.umcfgui/backup/` before it is overwritten. A backup is never skipped or overwritten: if a
   different copy is already there, the new one gets the next free name (`controlDict.1`, …).
3. **Atomic writes.** Each file is written to a new temporary file next to it, then renamed
   over it, so a crash never leaves half a dictionary. If writing stops partway (a full disk),
   the error names the files that were already saved.

A symlinked dictionary is written through its link, but only if the link (or a linked
directory on the way) points inside the case. A file a command writes whole (a surface, a face
set, the description `workflow.import` writes) is never written through a link: it is refused.
A hard-linked file is replaced, so its other links keep the old text. Undo after a save shows
the restored text as a change, to be saved again.
