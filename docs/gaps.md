# Gaps

What umcfGui can't do yet, most important first. Work is picked from the top. A row is removed
in the same pull request that closes it.

## Next

| # | Gap | Why it matters | Where |
|---|---|---|---|
| 1 | Reading `checkMesh -writeSets` (the face and cell sets, and its log), `checkMesh -allGeometry`'s extra checks, and layer coverage | `checkMesh` is the authority on mesh quality; the native check covers its default checks only | `validate`, `foam-mesh` |
| 2 | STL import and diagnostics: open and non-manifold edges, normals, degenerate triangles, units, extent; repair commands (weld, drop degenerate, orient, scale) | Urban geometry is rarely clean, and snappyHexMesh fails late on it | `geom`, `engine` |
| 3 | 3D scene of meshes and surfaces, CPU rasteriser, BVH picking (triangle → patch / face) | Seeing the case, and picking what a command acts on | `render`, `geom` |
| 4 | GUI shell: outliner (regions, patches, surfaces), inspector, toolbar and palette from the command registry, wgpu viewport, theme tokens | The application people use | `ui-egui`, `umcf-gui` |
| 5 | Boundary conditions by picking: `bc.set` on the selected patches, with the types the catalog offers for the field and version | Assigning BCs patch by patch in text files is slow and error-prone | `engine`, `ui-egui` |
| 6 | Meshing set-up: typed `blockMeshDict` / `snappyHexMeshDict` / `surfaceFeaturesDict`, checks before running (inside point in the fluid, closed surfaces, background box covers the geometry) | Creating cases from geometry, not only editing existing ones | `model`, `validate` |
| 7 | `run.export` (the workflow's scripts as a command), batch job scripts (SLURM/PBS), and the optional local runner | `Allprepare`/`Allrun`/`Allclean` are written from the workflow only by a development example (`cargo run -p umcf-model --example workflow`); the job wrappers and running them are not there | `model`, `runner`, `engine` |

## Known limits

| Limit | Effect | Possible fix |
|---|---|---|
| `Case::open` reads each start-time field file whole to list its boundary conditions | Opening a case at a late time with gzipped multi-GB fields takes 20 s (Hong Kong project: 30 fields, 4 GB); files over 256 MB are listed without their BCs | Read fields in the background after the case opens, and only when they are needed |
| Fields are read, not edited | Boundary conditions can't be changed yet | `bc.set` edits the field file losslessly, splicing list data back unchanged |
| The workflow covers the 3×3 pattern: snappyHexMesh and blockMesh regions in one case | Meshing chains spread over several cases (the Hong Kong project's extrusions, `sourceCase`) are checked step by step but not read into a workflow | Workflow steps for extrusion and for meshes taken from another case |
| A script's effect is followed only for commands the check knows | After a Python helper or an unknown utility, the rest of that script is not checked for missing files | Declare a helper's outputs in the workflow |
| `dict.set` quotes a new key only when it starts with `(` | A new pattern key that doesn't (`"U.*"`) is written as a plain word, which OpenFOAM doesn't match as a pattern; existing quoted keys are found and kept | A parameter that marks a key as a pattern |
| Building-material coefficients are not checked for their keywords | Each entry of `buildingMaterials` reads its coefficients through its own dictionary (`VanGenuchtenCoeffs_`); a missing one is found only when the run starts | Map each material type's coefficient dictionary to its place in `transportProperties` |
| Keywords read under a condition or after the start are not required | A keyword read only in some branch, a loop or a later time step is found only when the run reaches it | Follow the conditions the solver can decide from the case (a flag in the same dictionary) |
| The mesh quality check runs on one core and can't be cancelled | 40 s for a 16-million-cell mesh (13 s of it reading), during which the GUI would wait | Split the face loop over threads, as the reader does; run it as a job with a cancel flag |
| Standard OpenFOAM boundary-condition and model types are not in the catalog | A misspelt `zeroGradient` is not caught before the run | Extend `xtask catalog` to the OpenFOAM sources the solver builds against |
