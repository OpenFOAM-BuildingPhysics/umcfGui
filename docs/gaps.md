# Gaps

What umcfGui can't do yet, most important first. Work is picked from the top. A row is removed
in the same pull request that closes it.

## Next

| # | Gap | Why it matters | Where |
|---|---|---|---|
| 1 | Commands, session, undo and journal; the headless CLI with `umcf-cli check` | The GUI, CLI and MCP all act through commands; the case checks need a front end | `engine`, `umcf-cli` |
| 2 | Required fields per region and model (which fields the solver reads for a fluid, solid or vegetation region, and for the selected models), and the keywords each selected type needs | A missing `w` or `Ts` file stops the solver at the start; the checks know the patches but not yet the solver's field list | `xtask catalog`, `validate` |
| 3 | Mesh quality from `foam-mesh`: non-orthogonality, skewness, aspect ratio, volumes, open cells, with `checkMesh`'s thresholds; reading `checkMesh -writeSets` | Bad cells are the most common reason a run diverges; each problem needs a location to show | `validate` |
| 4 | STL import and diagnostics: open and non-manifold edges, normals, degenerate triangles, units, extent; repair commands (weld, drop degenerate, orient, scale) | Urban geometry is rarely clean, and snappyHexMesh fails late on it | `geom`, `engine` |
| 5 | 3D scene of meshes and surfaces, CPU rasteriser, BVH picking (triangle → patch / face) | Seeing the case, and picking what a command acts on | `render`, `geom` |
| 6 | GUI shell: outliner (regions, patches, surfaces), inspector, toolbar and palette from the command registry, wgpu viewport, theme tokens | The application people use | `ui-egui`, `umcf-gui` |
| 7 | Boundary conditions by picking: `bc.set` on the selected patches, with the types the catalog offers for the field and version | Assigning BCs patch by patch in text files is slow and error-prone | `engine`, `ui-egui` |
| 8 | Meshing set-up: typed `blockMeshDict` / `snappyHexMeshDict` / `surfaceFeaturesDict`, checks before running (inside point in the fluid, closed surfaces, background box covers the geometry) | Creating cases from geometry, not only editing existing ones | `model`, `validate` |
| 9 | Pipeline export (`Allprepare`/`Allrun`, SLURM/PBS) and the optional local runner | Running the meshing and the solver | `model`, `runner`, `engine` |

## Known limits

| Limit | Effect | Possible fix |
|---|---|---|
| `Case::open` reads each start-time field file whole to list its boundary conditions | Opening a case at a late time with gzipped multi-GB fields takes 20 s (Hong Kong project: 30 fields, 4 GB); files over 256 MB are listed without their BCs | Read fields in the background after the case opens, and only when they are needed |
| Fields are read, not edited | Boundary conditions can't be changed yet | `bc.set` edits the field file losslessly, splicing list data back unchanged |
| Standard OpenFOAM boundary-condition and model types are not in the catalog | A misspelt `zeroGradient` is not caught before the run | Extend `xtask catalog` to the OpenFOAM sources the solver builds against |
