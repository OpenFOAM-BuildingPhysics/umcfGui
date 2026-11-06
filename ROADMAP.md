# umcfGui roadmap

**Stage:** M0 — foundation. *Last updated: 2026-11-06.*

| Area | State |
|---|---|
| Workspace, `cargo xtask ci` (fmt, clippy, tests, layers, assets, wasm, deny) | ✅ |
| `foam-dict`: lossless dictionary parser/editor | ✅ round-trips 8,795 OF8/OF12 tutorial and `etc/` dictionaries and 1,003 UMCF case files byte-for-byte |
| Case layout: OpenFOAM version (OF8 / OF12) and regions | ✅ scored detection; correct on all 11 local UMCF cases, flags porting leftovers |
| Docs site (Zensical) + `docs` gate (rustdoc `-D warnings`, nav, strict build) | ✅ |
| `foam-field`: fields (ASCII, binary, gzip), time and processor directories | ✅ matches the mesh on every field of 6 projects; bit-identical to OpenFOAM-8/12 binary output |
| `foam-mesh`: polyMesh (ASCII, binary `faceCompactList`, gzip), boundary, cell zones, consistency checks | ✅ 32 meshes up to 16 M cells pass; identical to OpenFOAM-8/12 binary; 16 M cells in 13 s |
| Solver catalog: `schema::catalog` + `cargo xtask catalog` | ✅ 7 families, 41 types (13 BCs), 176 keyword reads, OF8 and OF12; checked in CI |
| `model`: typed case (regions, controls, start time, patches, boundary conditions), lossless edits, atomic save with backup and conflict check | ✅ opens all 18 local cases (OF8 and OF12, solver and meshing steps); an edit changes exactly one entry |
| `validate`: case checks (controls, regions, boundary conditions against the mesh as OpenFOAM matches them, region coupling, solver types per version) | ✅ no false errors on the 18 local cases; one real one found (a restart that would fail) |
| Case workflow: scripts read into the Allprepare/Allrun/Allclean pattern, checked (utilities per version, `-allRegions`, process counts, files at each step, overwritten dictionaries), and written back per version | ✅ all 10 local 3×3 cases fit the pattern; written scripts mesh, pre-process and run the 3×3 test case; 4 real script errors found |
| Required keywords of the selected vegetation, grass, solar load and radiation models (`<Type>Coeffs`, from the solver source, per version) | ✅ `keyword.missing`: no false errors on the 18 local cases |
| Required fields: what the solver reads per kind of region and per selected model (from the solver source, per version), plus OpenFOAM's thermophysics and turbulence fields | ✅ `field.missing` on all 18 local cases: no false errors; each removed field found on a test copy |
| `engine` (commands, session, undo and redo, journal, panic guard) and `umcf-cli` (`check`, `run`, `commands`) | ✅ 25 commands; `umcf-cli check` matches the expected result on all 18 local cases, in under 0.5 s each without fields |
| Mesh quality: `checkMesh`'s default geometric checks, ported from OpenFOAM-12, with its thresholds (`umcf-cli mesh`, `mesh.check`) | ✅ same numbers as `checkMesh` on the 3×3 case (OpenFOAM-12) and the 16 M-cell Hong Kong mesh (OpenFOAM-8); `checkMesh`'s logs and `-writeSets` sets read; `-allGeometry`'s checks match on the 3×3 case and a 5.4 M-cell mesh, 12× faster; layer coverage from `snappyHexMesh`'s log (OpenFOAM-8 and 12) |
| STL import, diagnostics and repair, tree crowns from a table (`geom`, `geometry.*`, `umcf-cli surface`) | ✅ vertices, open and non-manifold edges, regions and parts as `surfaceCheck` counts them on the local cases and OpenFOAM's tutorial surfaces; repairs checked with `surfaceCheck` and `surfaceFeatures` |
| 3D scene and picking (`render`, `scene.snapshot`, `select.*`, `umcf-cli render`) | ✅ meshes' boundary patches and STL surfaces drawn by a CPU rasteriser; picks a patch or surface region (and what lies behind it) through a BVH; 573 000 boundary triangles drawn and picked in 0.14 s |
| Boundary conditions and patches by picking (`bc.set`, `patch.set`, `patch.create`, `select.rect`) | ✅ conditions set on picked patches; patches retyped and renamed, and new ones prepared for `createPatch` from picked faces, a box or a surface |
| Example cases (`examples/of8`, `examples/of12`, `cargo xtask examples`, `cargo xtask make-examples`) | ✅ the four street canyon tutorials per version, small enough to run in seconds, made from pinned tutorial commits; checked on every test run, and in CI that they are what the generator makes |
| Workflow description `system/umcfWorkflow` (`workflow.import`, `workflow.show`, the `workflow.description` check) | ✅ written from a case's scripts and meshes; every patch of the 3×3 test case gets its role |
| Desktop app (`ui-egui`, `umcf-gui`): SolidCraft-style shell (workspaces, ribbon and command search from the registry, browser, properties, results, navigation bar, axis triad, edit history), 3D view with picking | ✅ CPU-drawn view; every action a command; light and dark themes |
| Control channel, MCP, wgpu viewport | ⬜ |
| Meshing set-up (blockMesh, snappyHexMesh) and its checks | ⬜ |
| Batch job scripts (SLURM/PBS), optional local `runner` | ⬜ |

Ranked work list: [`docs/gaps.md`](docs/gaps.md).

## Progress log

- 2026-11-03 — M0.1 workspace and `xtask`; M0.2 `foam-dict`; M0.3 `core` (case file access) and `schema::layout`; M0.4 docs site, CI, protected `main`; M0.5 `foam-field`; M0.6 solver catalog; M0.7 `foam-mesh`; M0.8 `model`; M0.9 `validate`; M0.10 case workflow (scripts read, checked and written). Scope extended: geometry import, meshing set-up, mesh-problem handling and boundary conditions by picking in 3D (`docs/architecture.md`).
- 2026-11-04 — M0.11 `engine` (command registry, session, undo, journal) and `umcf-cli` (`check`, `run`, `commands`).
- 2026-11-04 — M0.12 required fields per region and selected model (`xtask catalog` field reads, `field.missing`).
- 2026-11-04 — M0.13 required keywords of the selected models (`xtask catalog` keywords read at start, `keyword.missing`).
- 2026-11-04 — M0.14 mesh quality (`checkMesh`'s default checks, `umcf-cli mesh`).
- 2026-11-04 — M0.15 `checkMesh`'s own results: its logs and `-writeSets` sets, per region.
- 2026-11-04 — M0.16 `checkMesh -allGeometry`'s checks (`umcf-cli mesh --all-geometry`).
- 2026-11-04 — M0.17 layer coverage from `snappyHexMesh`'s log; logs read from `log/` too.
- 2026-11-04 — M0.18 STL surfaces: the `geom` crate (reading, checks, repairs), `geometry.import`, `geometry.check`, `geometry.repair`, `geometry.scale`, `umcf-cli surface`; surfaces saved with the case.
- 2026-11-04 — M0.19 3D scene: the `render` crate (scene, CPU rasteriser, PNG), a triangle BVH in `geom`, `scene.snapshot`, `select.pick`, `select.set`, `select.clear`, the session's selection, `umcf-cli render`.
- 2026-11-04 — M0.20 desktop app: `ui-egui` and `umcf-gui` with a SolidCraft-style layout; the engine names the edits in its undo history.
- 2026-11-04 — M0.21 the Claude review workflow, run on a pull request by `/claude-review`.
- 2026-11-04 — M0.22 the desktop app's review fixes: the scene and the surface list are not read again on every frame, and a panic while building the scene is caught; `--screenshot` fails when the case or a `--cmd` does.
- 2026-11-04 — M0.23 boundary conditions by picking: `bc.set` (a patch's entry set, every other byte of the field file kept, binary and gzip too), the condition card in the desktop app, `umcf-gui --open`.
- 2026-11-04 — M0.24 `bc.set` `defaults`: the condition card no longer writes the first patch's value over patches that keep their own.
- 2026-11-04 — M0.25 what a case needs (`docs/case-requirements.md`), from the solver, the tutorials and the reference cases; the gaps ordered towards building a case from STL surfaces.
- 2026-11-04 — M0.26 patch editing: `patch.set` (a patch's type, groups and entries set and its name changed in the mesh, the start-time fields and the partner patches, every other byte kept), the patch card in the desktop app.
- 2026-11-05 — M0.27 new patches: `patch.create` (faces by label, in a box or near an STL surface, into a face set and a dictionary of the patches prepared on the mesh, for one run of OpenFOAM's `createPatch`, the start-time fields given the source patch's entry), the new patch card in the desktop app.
- 2026-11-05 — M0.28 faces picked in the view: `select.pick` with `faces`, faces in `select.set` and the selection, drawn in the selection colour, `patch.create` from them, *Pick faces* in the desktop app.
- 2026-11-05 — M0.29 *Open* asks for the case's folder in the system's own dialog over the app's window (Windows, macOS, and the desktop portal or zenity on Linux, through `rfd`); the desktop app built and tested on Windows and macOS in CI.
- 2026-11-05 — M0.30 selecting in a rectangle: `select.rect` (what is seen in it, or everything through it; patches or faces), the camera given back to `scene.snapshot`, `select.pick` and `select.rect`, *Box select* and *Select through* in the desktop app.
- 2026-11-05 — M0.31 display modes: each region, patch, patch group, surface or surface region drawn as surfaces, wireframe or points, or hidden, with its own opacity (`display` in `scene.snapshot`, `select.pick` and `select.rect`; transparency sorted back to front in the CPU renderer), set from the browser's context menu in the desktop app.
- 2026-11-05 — M0.32 the ribbon is tall enough for its group labels (File, Inspect, Configure), which ran over its bottom edge.
- 2026-11-05 — M0.33 a renamed patch's new name follows into `0.orig` and the region's `viewFactorsDict`; `patch.set` lists the result time directories and each file and line that still has the old name.
- 2026-11-05 — M0.34 the common workflow designed (`docs/common-workflow.md`): `system/umcfWorkflow` as the case's description, the dictionaries, fields and scripts generated from it, running OpenFOAM natively, through WSL, in a VM or container, or by ssh; the gaps rows 4–9 and 12 ordered towards it.
- 2026-11-05 — M0.35 a mesh header's `nCells: 400` (as topoSet and changeDictionary write it) is read; the check follows `-dict` paths as each version reads them: OpenFOAM-8 through the region's directory for the applications that read through the mesh, a directory, a bare name, `..`.
- 2026-11-05 — M0.36 example cases: the street canyon tutorials (air only, walls and street, grass, a tree) made small enough to run in seconds, for OpenFOAM-8 and -12 (`examples/of8`, `examples/of12`), checked on every test run and run with OpenFOAM by `cargo xtask examples`.
- 2026-11-05 — M0.37 the workflow description, `system/umcfWorkflow`: regions with their kinds, meshing steps and steps after, patches with roles and partners, radiation and the solver; written from a case's scripts and meshes (`workflow.import`), shown (`workflow.show`), read with its problems named and checked against the case (`workflow.description`).
- 2026-11-05 — M0.38 tree crowns from a table of trees (`geometry.trees`): a rhombicuboctahedron per tree of its crown diameter and height, standing on its z, closed and wound outward, written to `constant/triSurface`.
- 2026-11-06 — M0.39 a file a command writes whole (a surface, a face set, the description `workflow.import` writes) is refused when it is a symbolic link, to a field, another file or nothing: the save would have written the file it points to.
- 2026-11-06 — M0.40 AGENTS.md is the only rule file (`CLAUDE.md` imports it): the pull request rules (§8) and work on several machines (§9) are written there.
- 2026-11-06 — M0.41 the example cases' generator in Rust (`cargo xtask make-examples`): the street canyon tutorials at pinned commits (the tags have moved), made small; `--check` in CI fails unless `examples/` is what it makes, byte for byte.
