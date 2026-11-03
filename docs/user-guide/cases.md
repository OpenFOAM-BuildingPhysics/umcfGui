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
