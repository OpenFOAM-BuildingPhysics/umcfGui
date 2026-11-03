# umcfGui roadmap

**Stage:** M0 — foundation. *Last updated: 2026-11-03.*

| Area | State |
|---|---|
| Workspace, `cargo xtask ci` (fmt, clippy, tests, layers, assets, wasm, deny) | ✅ |
| `foam-dict`: lossless dictionary parser/editor | ✅ round-trips 8,795 OF8/OF12 tutorial and `etc/` dictionaries and 1,003 UMCF case files byte-for-byte |
| Case layout: OpenFOAM version (OF8 / OF12) and regions | ✅ scored detection; correct on all 11 local UMCF cases, flags porting leftovers |
| Docs site (Zensical) + `docs` gate (rustdoc `-D warnings`, nav, strict build) | ✅ |
| `foam-field`: fields (ASCII, binary, gzip), time and processor directories | ✅ matches the mesh on every field of 6 projects; bit-identical to OpenFOAM-8/12 binary output |
| `foam-mesh` (polyMesh) | ⬜ |
| `schema` + `cargo xtask catalog` (solver types from `TypeName`) | ⬜ |
| `model`, `engine` commands, CLI | ⬜ |
| `ui-egui`, control channel, MCP | ⬜ |
| `run.export` (Allrun + SLURM/PBS), optional local `runner` | ⬜ |

## Progress log

- 2026-11-03 — M0.1 workspace and `xtask`; M0.2 `foam-dict`; M0.3 `core` (case file access) and `schema::layout`; M0.4 docs site, CI, protected `main`; M0.5 `foam-field`.
