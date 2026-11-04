# Gaps

What umcfGui can't do yet, most important first. Work is picked from the top. A row is removed
in the same pull request that closes it.

## Next

| # | Gap | Why it matters | Where |
|---|---|---|---|
| 1 | Required fields per region and model (which fields the solver reads for a fluid, solid or vegetation region, and for the selected models), and the keywords each selected type needs | A missing `w` or `Ts` file stops the solver at the start; the checks know the patches but not yet the solver's field list | `xtask catalog`, `validate` |
| 2 | Mesh quality from `foam-mesh`: non-orthogonality, skewness, aspect ratio, volumes, open cells, with `checkMesh`'s thresholds; reading `checkMesh -writeSets` | Bad cells are the most common reason a run diverges; each problem needs a location to show | `validate` |
| 3 | STL import and diagnostics: open and non-manifold edges, normals, degenerate triangles, units, extent; repair commands (weld, drop degenerate, orient, scale) | Urban geometry is rarely clean, and snappyHexMesh fails late on it | `geom`, `engine` |
| 4 | 3D scene of meshes and surfaces, CPU rasteriser, BVH picking (triangle → patch / face) | Seeing the case, and picking what a command acts on | `render`, `geom` |
| 5 | GUI shell: outliner (regions, patches, surfaces), inspector, toolbar and palette from the command registry, wgpu viewport, theme tokens | The application people use | `ui-egui`, `umcf-gui` |
| 6 | Boundary conditions by picking: `bc.set` on the selected patches, with the types the catalog offers for the field and version | Assigning BCs patch by patch in text files is slow and error-prone | `engine`, `ui-egui` |
| 7 | Meshing set-up: typed `blockMeshDict` / `snappyHexMeshDict` / `surfaceFeaturesDict`, checks before running (inside point in the fluid, closed surfaces, background box covers the geometry) | Creating cases from geometry, not only editing existing ones | `model`, `validate` |
| 8 | `run.export` (the workflow's scripts as a command), batch job scripts (SLURM/PBS), and the optional local runner | `Allprepare`/`Allrun`/`Allclean` are written from the workflow only by a development example (`cargo run -p umcf-model --example workflow`); the job wrappers and running them are not there | `model`, `runner`, `engine` |

## Known limits

| Limit | Effect | Possible fix |
|---|---|---|
| `Case::open` reads each start-time field file whole to list its boundary conditions | Opening a case at a late time with gzipped multi-GB fields takes 20 s (Hong Kong project: 30 fields, 4 GB); files over 256 MB are listed without their BCs | Read fields in the background after the case opens, and only when they are needed |
| Fields are read, not edited | Boundary conditions can't be changed yet | `bc.set` edits the field file losslessly, splicing list data back unchanged |
| The workflow covers the 3×3 pattern: snappyHexMesh and blockMesh regions in one case | Meshing chains spread over several cases (the Hong Kong project's extrusions, `sourceCase`) are checked step by step but not read into a workflow | Workflow steps for extrusion and for meshes taken from another case |
| A script's effect is followed only for commands the check knows | After a Python helper or an unknown utility, the rest of that script is not checked for missing files | Declare a helper's outputs in the workflow |
| `dict.set` quotes a new key only when it starts with `(` | A new pattern key that doesn't (`"U.*"`) is written as a plain word, which OpenFOAM doesn't match as a pattern; existing quoted keys are found and kept | A parameter that marks a key as a pattern |
| Standard OpenFOAM boundary-condition and model types are not in the catalog | A misspelt `zeroGradient` is not caught before the run | Extend `xtask catalog` to the OpenFOAM sources the solver builds against |
