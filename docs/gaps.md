# Gaps

What umcfGui can't do yet, most important first. Work is picked from the top. A row is removed
in the same pull request that closes it.

## Next

| # | Gap | Why it matters | Where |
|---|---|---|---|
| 1 | GUI shell: outliner (regions, patches, surfaces), inspector, toolbar and palette from the command registry, wgpu viewport, theme tokens | The application people use | `ui-egui`, `umcf-gui` |
| 2 | Boundary conditions by picking: `bc.set` on the selected patches, with the types the catalog offers for the field and version | Assigning BCs patch by patch in text files is slow and error-prone | `engine`, `ui-egui` |
| 3 | Meshing set-up: typed `blockMeshDict` / `snappyHexMeshDict` / `surfaceFeaturesDict`, checks before running (inside point in the fluid, closed surfaces where they must be, inconsistent normals where the inside matters, background box covers the geometry) | Creating cases from geometry, not only editing existing ones | `model`, `validate` |
| 4 | `run.export` (the workflow's scripts as a command), batch job scripts (SLURM/PBS), and the optional local runner | `Allprepare`/`Allrun`/`Allclean` are written from the workflow only by a development example (`cargo run -p umcf-model --example workflow`); the job wrappers and running them are not there | `model`, `runner`, `engine` |

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
| The 3D scene reads a region's whole mesh to draw its boundary | Drawing or picking in the 2.8-million-cell air region of the 3×3 case takes 5 s and 1.3 GB the first time, almost all of it reading the internal faces and owners | Read only the boundary faces and the points they use |
| The CPU renderer has no level of detail and draws on one core | Fine for snapshots of a few million triangles; too slow for a viewport that turns a large mesh | The wgpu viewport (GUI shell) draws the same scene; simplified display meshes built off the UI thread |
| Only STL surfaces are checked | OBJ, VTK and the other surface formats OpenFOAM reads in `constant/triSurface` are skipped by `geometry.check` | Readers for OBJ and VTK in `geom` |
| A surface command keeps a copy of the file per change, for undo | Repairing a 500 MB surface ten times holds ten copies until the undo history (100 steps) drops them | Keep one copy per surface and store the repairs as steps to replay, or cap undo by memory |
| Standard OpenFOAM boundary-condition and model types are not in the catalog | A misspelt `zeroGradient` is not caught before the run | Extend `xtask catalog` to the OpenFOAM sources the solver builds against |
