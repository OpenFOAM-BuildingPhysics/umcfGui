# umcfGui

**An open-source GUI for [urbanMicroclimateFoam](https://github.com/OpenFOAM-BuildingPhysics/urbanMicroclimateFoam):
set up, run and inspect urban microclimate simulations.**

urbanMicroclimateFoam (UMCF) is a multi-region OpenFOAM solver for urban microclimate: CFD in
the air, heat and moisture transport in porous building materials, vegetation, and view-factor
radiation. umcfGui edits UMCF cases, runs their pipeline (locally or as an exported cluster
job) and shows the results. It never re-implements the physics: results always come from a
solver run.

!!! warning "Early development"
    umcfGui is at milestone M0, the foundation. The libraries that read and edit cases exist
    and are tested against real cases. The desktop app, the command line and the run pipeline
    don't exist yet. See the [roadmap](https://github.com/OpenFOAM-BuildingPhysics/umcfGui/blob/main/ROADMAP.md).

## Design goals

<div class="grid cards" markdown>

-   **Never lose a case**

    People hand-edit OpenFOAM dictionaries. umcfGui keeps every comment, alignment, `#include`
    and `$macro`: a file it hasn't changed is byte-for-byte the same, and an edit touches only
    the entry it changes. [Dictionaries](user-guide/dictionaries.md)

-   **OpenFOAM-8 and OpenFOAM-12**

    UMCF exists for both, and real projects mix them. umcfGui works out each case's version from
    its files and reports what's left over from porting. [OpenFOAM versions](user-guide/openfoam-versions.md)

-   **Cluster first**

    Most UMCF cases are too big for a workstation. Running locally is optional; exporting a case
    with its run scripts for a cluster scheduler is always there.

-   **Scriptable**

    Every action is a command with an id, usable from the GUI, the command line, a JSON control
    channel and an MCP server for AI agents. [Architecture](architecture.md)

</div>

## Licence

umcfGui is free software under the GNU General Public License v3.0 or later, the same as
urbanMicroclimateFoam and OpenFOAM.
