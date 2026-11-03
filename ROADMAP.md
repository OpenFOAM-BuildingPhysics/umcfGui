# umcfGui roadmap

**Stage:** M0 — foundation. *Last updated: 2026-11-03.*

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
| `engine` commands, CLI (`umcf-cli check`) | ⬜ |
| Mesh quality (`checkMesh` metrics and sets) | ⬜ |
| STL import, diagnostics and repair | ⬜ |
| 3D scene, picking, boundary conditions by picking | ⬜ |
| `ui-egui` (SolidCraft-style shell and viewport), control channel, MCP | ⬜ |
| Meshing set-up (blockMesh, snappyHexMesh) and its checks | ⬜ |
| Batch job scripts (SLURM/PBS), optional local `runner` | ⬜ |

Ranked work list: [`docs/gaps.md`](docs/gaps.md).

## Progress log

- 2026-11-03 — M0.1 workspace and `xtask`; M0.2 `foam-dict`; M0.3 `core` (case file access) and `schema::layout`; M0.4 docs site, CI, protected `main`; M0.5 `foam-field`; M0.6 solver catalog; M0.7 `foam-mesh`; M0.8 `model`; M0.9 `validate`; M0.10 case workflow (scripts read, checked and written). Scope extended: geometry import, meshing set-up, mesh-problem handling and boundary conditions by picking in 3D (`docs/architecture.md`).
