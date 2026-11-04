# Gaps

What umcfGui can't do yet, most important first. Work is picked from the top. A row is removed
in the same pull request that closes it.

## Next

| # | Gap | Why it matters | Where |
|---|---|---|---|
| 1 | Patches of an existing mesh: change a patch's type (`patch`, `wall`, `symmetry`, `empty`, …), rename it (in every field file too) and set its groups (`patch.set`); new patches from picked faces, a box or an STL region, through OpenFOAM's `topoSet` and `createPatch` (`patch.create`) | Real cases need their patches split and typed before conditions can be set; today that is hand-written dictionaries and utilities | `model`, `engine`, `ui-egui` |
| 2 | Patches of a new case on its STL surfaces: name and type the patches snappyHexMesh and blockMesh make from the surfaces' regions and the background box | A new case comes out of meshing with the right patches | `model`, `validate`, `ui-egui` |
| 3 | Reset the mesh and mesh again (`mesh.reset`): put back the case's meshing state (no `polyMesh`, no meshing results) and run the meshing steps of the workflow again | After a change to the geometry or the patches the mesh is out of date | `model`, `runner`, `engine` |
| 4 | Meshing set-up: typed `blockMeshDict` / `snappyHexMeshDict` / `surfaceFeaturesDict`, checks before running (inside point in the fluid, closed surfaces where they must be, inconsistent normals where the inside matters, background box covers the geometry) | Creating cases from geometry, not only editing existing ones | `model`, `validate` |
| 5 | `run.export` (the workflow's scripts as a command), batch job scripts (SLURM/PBS), and the optional local runner | `Allprepare`/`Allrun`/`Allclean` are written from the workflow only by a development example (`cargo run -p umcf-model --example workflow`); the job wrappers and running them are not there | `model`, `runner`, `engine` |

## Known limits

| Limit | Effect | Possible fix |
|---|---|---|
| `Case::open` reads each start-time field file whole to list its boundary conditions | Opening a case at a late time with gzipped multi-GB fields takes 20 s (Hong Kong project: 30 fields, 4 GB); files over 256 MB are listed without their BCs | Read fields in the background after the case opens, and only when they are needed |
| `bc.set` sets the patch's own entry only | A condition given by a group or a pattern (`"(wall\|roof).*"`) isn't changed: the patch gets an entry of its own, which takes precedence; setting the group's entry needs `dict.set`-style editing of field files | A `bc.set` target for a group or pattern entry |
| A same-type `bc.set` can add or change keys, not remove them | Removing a key from a condition means giving the patch another type and back, or editing the file | A `remove` list in `bc.set` |
| Field files over the open limit (256 MB) are not edited | Their conditions can't be set here (they are listed without them already) | Rewrite the `boundaryField` part of a big file without reading its lists into numbers |
| An edited field file is kept whole per change, for undo | Ten condition changes on a 500 MB field file hold ten copies until the undo history drops them | Store the edits as steps to replay, or cap undo by memory |
| The workflow covers the 3×3 pattern: snappyHexMesh and blockMesh regions in one case | Meshing chains spread over several cases (the Hong Kong project's extrusions, `sourceCase`) are checked step by step but not read into a workflow | Workflow steps for extrusion and for meshes taken from another case |
| A script's effect is followed only for commands the check knows | After a Python helper or an unknown utility, the rest of that script is not checked for missing files | Declare a helper's outputs in the workflow |
| `dict.set` quotes a new key only when it starts with `(` | A new pattern key that doesn't (`"U.*"`) is written as a plain word, which OpenFOAM doesn't match as a pattern; existing quoted keys are found and kept | A parameter that marks a key as a pattern |
| Building-material coefficients are not checked for their keywords | Each entry of `buildingMaterials` reads its coefficients through its own dictionary (`VanGenuchtenCoeffs_`); a missing one is found only when the run starts | Map each material type's coefficient dictionary to its place in `transportProperties` |
| Keywords read under a condition or after the start are not required | A keyword read only in some branch, a loop or a later time step is found only when the run reaches it | Follow the conditions the solver can decide from the case (a flag in the same dictionary) |
| The mesh quality check runs on one core and can't be cancelled | 40 s for a 16-million-cell mesh (13 s of it reading), during which the GUI would wait | Split the face loop over threads, as the reader does; run it as a job with a cancel flag |
| The 3D scene reads a region's whole mesh to draw its boundary | Drawing or picking in the 2.8-million-cell air region of the 3×3 case takes 5 s and 1.3 GB the first time, almost all of it reading the internal faces and owners | Read only the boundary faces and the points they use |
| The desktop app's 3D view is drawn by the CPU renderer, on one core | Turning a mesh of millions of boundary faces is slow (the view draws at half resolution while it turns) | A wgpu viewport drawing the same scene; simplified display meshes built off the UI thread |
| The desktop app runs commands on the window's thread | Opening a large case or reading a big mesh freezes the window for seconds | Run commands as jobs with progress and a cancel flag |
| Command parameters are typed as JSON in the desktop app | Usable, not friendly | Forms from typed parameter descriptions in the registry |
| No control channel yet | `umcf-gui --control` (AGENTS.md §7) and `ui.screenshot` are not there; screenshots come from `--screenshot` | A JSON-lines server in `ui-egui` over the engine |
| Only STL surfaces are checked | OBJ, VTK and the other surface formats OpenFOAM reads in `constant/triSurface` are skipped by `geometry.check` | Readers for OBJ and VTK in `geom` |
| A surface command keeps a copy of the file per change, for undo | Repairing a 500 MB surface ten times holds ten copies until the undo history (100 steps) drops them | Keep one copy per surface and store the repairs as steps to replay, or cap undo by memory |
| Standard OpenFOAM boundary-condition and model types are not in the catalog | A misspelt `zeroGradient` is not caught before the run; the condition card offers a short hand-written list of OpenFOAM's common types | Extend `xtask catalog` to the OpenFOAM sources the solver builds against |
