# User Guide

How umcfGui sees urbanMicroclimateFoam cases, and what it does with them.

- [Cases](cases.md): solver cases and meshing steps, regions, and the chain of cases a project
  is made of.
- [OpenFOAM versions](openfoam-versions.md): how umcfGui tells OpenFOAM-8 and OpenFOAM-12 cases
  apart, and what it reports about partly ported cases.
- [Dictionaries](dictionaries.md): how umcfGui reads and edits OpenFOAM dictionaries without
  losing your formatting.
- [Surfaces](surfaces.md): the STL geometry in `constant/triSurface`: what would break meshing,
  importing and repairing it.
- [Meshes](meshes.md): the mesh of each region, and the checks it passes before use.
- [3D scene and selection](scene.md): drawing the meshes and surfaces, and picking patches and
  surface regions in them.
- [Fields and results](fields.md): reading the solver's field files, ASCII or binary,
  compressed or not.
- [Command line](command-line.md): `umcf-cli check`, `render` and `run`, without the GUI.
