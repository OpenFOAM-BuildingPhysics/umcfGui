# umcfGui roadmap

**Stage:** M0 — foundation. *Last updated: 2026-11-03.*

| Area | State |
|---|---|
| Workspace, `cargo xtask ci` (fmt, clippy, tests, layers, assets, wasm, deny) | ✅ |
| `foam-dict`: lossless dictionary parser/editor | ✅ round-trips 8,795 OF8/OF12 tutorial and `etc/` dictionaries and 1,003 UMCF case files byte-for-byte |
| OpenFOAM version detection (OF8 / OF12) | ⬜ next |
| `foam-mesh`, `foam-field` (streamed fields, binary, `processor*`) | ⬜ |
| `schema` + `cargo xtask catalog` (solver types from `TypeName`) | ⬜ |
| `model`, `engine` commands, CLI | ⬜ |
| `ui-egui`, control channel, MCP | ⬜ |
| `run.export` (Allrun + SLURM/PBS), optional local `runner` | ⬜ |

## Progress log

- 2026-11-03 — M0.1 workspace and `xtask`; M0.2 `foam-dict`.
