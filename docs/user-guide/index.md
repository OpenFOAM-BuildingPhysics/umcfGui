# User Guide

How umcfGui sees urbanMicroclimateFoam cases, and what it does with them.

- [Cases](cases.md): solver cases and meshing steps, regions, and the chain of cases a project
  is made of.
- [OpenFOAM versions](openfoam-versions.md): how umcfGui tells OpenFOAM-8 and OpenFOAM-12 cases
  apart, and what it reports about partly ported cases.
- [Dictionaries](dictionaries.md): how umcfGui reads and edits OpenFOAM dictionaries without
  losing your formatting.
- [Surfaces](surfaces.md): the STL geometry in `constant/triSurface`: what would break meshing,
  importing and repairing it, and tree crowns from a table of trees.
- [Meshes](meshes.md): the mesh of each region, and the checks it passes before use.
- [3D scene and selection](scene.md): drawing the meshes and surfaces, and picking patches and
  surface regions in them.
- [Checking a case](checks.md): what the case check finds, code by code.
- [The case workflow](workflow.md): the scripts read, checked and written again, and the
  workflow description `system/umcfWorkflow`.
- [A new case](new-case.md): a case made from its regions and components, with one
  configuration's settings (the tutorials').
- [Fields and results](fields.md): reading the solver's field files, ASCII or binary,
  compressed or not.
- [The desktop app](desktop.md): `umcf-gui`, the window and what each part of it does.
- [Command line](command-line.md): `umcf-cli check`, `mesh`, `surface`, `render`, `run` and
  `commands`, without the GUI.
