# umcfGui — rules for every agent and contributor

umcfGui is an open-source, pure-Rust desktop GUI (and headless CLI/MCP) for
**urbanMicroclimateFoam** (UMCF), the multi-region OpenFOAM solver for urban microclimate:
CFD in the air region, heat-and-moisture (HAM) transport in porous building materials,
vegetation, and view-factor radiation. The GUI sets up cases, runs the solver pipeline
and shows the results. It never re-implements the physics.

**This file is the only binding rule file.** Every rule for agents and contributors is here;
`docs/contributing.md` sums it up for people, and where the two differ this file wins. `CLAUDE.md`
only imports it, and an agent's own memory holds no project rules (§8).

---

## 1. The solver is the spec

- The solver source (GPL-3.0) is the authoritative reference for dictionary keywords,
  run-time-selectable types (`TypeName("…")`), defaults, region names and file layout. **Read it
  freely.** It is the git submodule `solver/` (upstream urbanMicroclimateFoam, branch `master`,
  the OpenFOAM-12 port), pinned to one commit. Other versions are tags in the same clone:
  `git -C solver show of-org_v8.0:urbanMicroclimateFoam.C`.
- **`solver/` is read-only from here.** Never commit inside it from this repo: solver changes
  are pull requests to urbanMicroclimateFoam. Moving the pin (`git submodule update --remote
  solver`) is its own commit: run `cargo xtask catalog` to regenerate the catalog, review the
  diff of `docs/solver-coverage.md` (it shows what the solver gained or lost), then
  `cargo xtask ci` and the real-case tests (`docs/contributing.md`).
- **Licence: GPL-3.0-or-later, the same as UMCF and OpenFOAM** (`LICENSE`, copied from the
  solver). Every crate sets `license = "GPL-3.0-or-later"` through `[workspace.package]`.
  Dependencies must be GPL-3-compatible: MIT, Apache-2.0, BSD, ISC, Zlib, MPL-2.0 and
  LGPL/GPL-3 are fine; GPL-2.0-only and proprietary licences are not. `cargo deny` enforces
  this in `cargo xtask ci`.
- **Porting solver code is allowed.** A ported function keeps the original copyright line and
  names its source (`// Ported from _LIB/buildingMaterialModel/HamstadBrick/HamstadBrick.C`,
  plus the solver commit). It also gets a test that compares its output with the solver's
  output on the same input.
- **The solver stays the authority on physics.** Port only what a preview or the validation
  needs (material curves for a plot, say), and never present a ported calculation as a result.
  Results always come from a solver run. Where it can, the GUI runs the solver's own utilities
  rather than re-implementing them, so the two don't drift apart.
- **Catalog:** `cargo xtask catalog` scans `solver/` (the pinned commit and the tag
  `of-org_v8.0`) for every model family, selectable type and keyword, and writes
  `crates/schema/src/catalog/generated.rs` and `docs/solver-coverage.md`. Never edit either by
  hand; `cargo xtask ci` fails when they don't match the pinned solver. The GUI offers solver
  types only from this catalog: a type the solver has but the GUI doesn't expose is a gap
  (`docs/gaps.md`), and the GUI cannot offer one the solver doesn't have.
- Target OpenFOAM version: **OpenFOAM-org v12** (the version the reference cases run on). Older
  versions (v6/v7/v8, per solver tags) are a `schema` profile, never `if`s scattered through the
  code.

## 2. Never lose a user's case

People hand-edit OpenFOAM dictionaries. A GUI that rewrites a case and drops their comments,
`#include`s or `$macros` is worse than no GUI.
- **Lossless round-trip:** `foam-dict` keeps comments, ordering, whitespace style, `#include`,
  `#includeEtc`, `$var`, `#calc`, `#codeStream` and unknown keys. Editing one entry rewrites
  only that entry. Tests: parse → write is byte-identical for every file in the reference
  cases.
- **Unknown is preserved, not deleted.** A keyword or type the schema doesn't know shows up as
  "raw" in the UI and survives saving unchanged.
- Saves are atomic (write to a temp file, then rename). Keep a `.umcfgui/backup/` copy of the
  files that were touched before the first save of each session.
- Never delete time directories, `processor*` or `postProcessing` without an explicit command
  (`case.clean`), and that command lists what it will remove before it runs.

## 3. Never crash (outranks feature work)

- **No panics in non-test code:** no `unwrap()`, `expect()`, `panic!`, `unreachable!`, `todo!`,
  `unimplemented!`. `unsafe_code = "forbid"`. Every production crate root (`lib.rs`, each
  `main.rs`) carries
  `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]`.
  Root `clippy.toml` allows them only in tests.
- Errors are `Result<T, E>` through each crate's `thiserror` error type and `?`.
- **Case files are hostile input:** malformed dictionaries, truncated binary `polyMesh` files,
  list sizes that don't match their data, 50-million-cell meshes, NaN fields, missing regions.
  Use `get()` not `[i]`, checked arithmetic, and capped allocations. Recursion is bounded
  (`#include` cycles, nested dictionaries).
- **Solver processes are hostile too:** they crash, hang, fill the disk and print gigabytes of
  logs. Parse logs as streams, stay within a memory bound, and make every running job
  cancellable.
- **Last-resort guard:** `Session::execute` wraps each command in `catch_unwind` and restores
  the case model on a panic. It's a safety net, not a licence.
- Every crash fix lands with a regression test that panicked before the fix.

## 4. Architecture

Full detail: [`docs/architecture.md`](docs/architecture.md). Enforced by `cargo xtask layers`.

| Layer | Crates |
|---|---|
| L0 | `core` (ids, units, errors, paths), `geom` (vectors, bbox, STL read/write, repair and diagnostics, ray-cast BVH) |
| L1 | `foam-dict` (lossless dictionary parser/writer), `foam-mesh` (polyMesh), `foam-field` (fields, time dirs, processor dirs) |
| L2 | `schema` (catalog of solver types and keywords), `model` (typed case: regions, materials, BCs, controls) |
| L3 | `validate` (case checks, meshing set-up checks, mesh quality), `post` (probes, time series, slices, log/residual parsing), `render` (renderer-agnostic 3D scene, CPU rasteriser, picking) |
| L4 | `runner` (**optional**, cargo feature `runner`: runs the pipeline locally, with processes, MPI and cancel; native only) |
| L5 | `engine` (session, commands, history, journal, guard) |
| L6 | `ui-egui`, `mcp` |
| L7 | apps `umcf-gui` (desktop), `umcf-cli` (headless, MCP) |

- **Nothing below L6 depends on egui/eframe/winit/rfd/wgpu. The UI crate is swappable.**
- **Running locally is optional.** Most cases are too big for a workstation, so the main
  workflow is set up → export → run on a cluster → bring the results back → view them.
  `engine` always has the pipeline *description* (`Step`s, in `model`) and `run.export`, which
  writes `Allprepare`/`Allrun` plus a job script for SLURM or PBS. Only local execution
  (`run.start`, `run.stop`, `run.status`) sits behind the `runner` feature. Without it those
  commands still exist, report as disabled ("built without local runner") and never panic. Every
  feature apart from local execution works the same with or without `runner`, and
  `cargo xtask ci` builds and tests both configurations.
- **Everything is a command** (`crates/engine/src/cmd/*`): id, label, menu path, shortcut,
  params doc, `enabled`, `run`, plus tests. Ids look like `case.open`, `region.add`,
  `material.assign`, `bc.set`, `control.set`, `mesh.block`, `run.prepare`, `run.start`,
  `run.stop`, `post.probe`. The UI, CLI, control channel and MCP all dispatch by id.
- **Programmatic calls never open dialogs.** A command called with empty params runs with
  defaults or returns an error. Only menu-style invocation opens a dialog (`UiRequest::Dialog`).
- **The UI is thin:** panels read engine state and act through `app.run(id, params)`. Colours
  come from `theme::Tokens`. **egui only, never Tauri.**
- **From geometry to a case, in the GUI.** People import STL geometry (buildings, terrain,
  trees), set up and run the meshing (blockMesh background, snappyHexMesh refinement and layers,
  the solid-region extrusion steps), look at the mesh in 3D and assign boundary conditions by
  picking patches there. Every one of these is a command first (`geometry.import`,
  `mesh.setup`, `bc.set` with the picked patches as parameters); the 3D view only produces the
  selection. Meshing writes the same dictionaries and runs the same OpenFOAM utilities a user
  would, never a mesher of our own.
- **Common mesh problems are caught and explained, not left to a failed run.** STL defects
  (open or non-manifold edges, flipped normals, degenerate triangles, wrong units), meshing
  set-up mistakes (insidePoint inside a building, a leak through an open surface, missing
  feature edges) and mesh-quality problems (non-orthogonality, skewness, negative volumes,
  failed layers) are reported with a location that is highlighted in the 3D view and, where it
  is safe, a command that fixes it. Mesh quality follows `checkMesh` (its thresholds and, when
  available, its `-writeSets` output); a native check is a preview, never a replacement.
- **Time goes through `chrono`** (simulation start dates, weather files, solar time). Never
  hand-roll calendar math.
- **Rust only** (no handwritten JS/TS, no Python in the product). L0–L3 stay wasm-clean
  (`cargo xtask wasm`) so a web results viewer stays possible.

## 5. Assets

- Every asset is original work (prefer icons drawn in code in `crates/ui-egui/src/icons.rs`),
  CC0/public domain, CC-BY(-SA), OFL or permissive OSS. Nothing from ParaView/Kitware, ESI,
  Ansys or any other vendor's GUI.
- Every asset file has a row in `ATTRIBUTION.md` (author, source, licence). `cargo xtask assets`
  enforces this. Never weaken the check to make a commit pass.
- **Never commit meshes, field data or results.** Test cases are generated in code (a tiny
  blockMesh canyon) or fetched pinned and sha256-verified from a corpus outside the repo.

## 6. Quality gates (before every commit)

`cargo xtask ci` runs fmt check, clippy `-D warnings`, tests, `layers`, `assets`, `catalog` and
`wasm`. Only green states get committed. One task id per commit (`M1.2: material picker`).

## 7. Running and looking at the app

- `cargo run --release -p umcf-gui -- --case ../testcases/Mesh_3x3_small --control 7995`
- Drive it with JSON lines on `127.0.0.1:7995`, e.g.
  `{"id":1,"method":"engine.execute","params":{"command":"bc.set","params":{"region":"air","patch":"inlet","field":"U","type":"fixedValue","value":[2,0,0]}}}`,
  then `{"id":2,"method":"ui.screenshot","params":{"path":"/tmp/shot.png"}}`. Protocol:
  `docs/control-protocol.md`.
- **For UI work, look at the result** (a headless snapshot PNG). Never send OS-level synthetic
  input.
- Headless: `umcf-cli check <case>`, `umcf-cli run --case <case> --cmd 'control.set={"endTime":3600}'`,
  `umcf-cli mcp`.
- Solver runs in tests are opt-in (`UMCF_SOLVER=1`, OpenFOAM sourced). Without that they call
  `skip!`; they never fail silently.
- Parallel agents get a separate `CARGO_TARGET_DIR`, their own control port and a scratch case
  copy. They never run the solver in a shared case directory.
- **Both OpenFOAM versions.** Real cases mix OpenFOAM-8 and -12, so the version is detected per
  case, and every change that reads or writes case files is tested on cases of both (the
  examples, generated cases, or the corpus through `UMCF_CASES`). OpenFOAM runs only on scratch
  copies; real cases are read, never written.

## 8. Planning and progress

`plan/` (gitignored) holds `STATUS.md` (start every session there) and `execution-plan.md`.
Committed: `ROADMAP.md` (stage, headline numbers, progress log), `docs/gaps.md` (the ranked work
list, pick from the top), and `docs/solver-coverage.md` (one row per solver type, keyword and
utility: exposed / partial / missing, with the summary regenerated by `cargo xtask catalog`).
Update the affected rows in the same commit as the feature.

**Several machines.** Work moves between machines through git only. `plan/` stays on its machine
and holds what is true only there: case paths and `UMCF_CASES`, toolchain workarounds, the name
of each remote (an agent's memory on that machine may keep the same facts, never rules). None of
that goes into a committed file. A rule that holds everywhere goes into
this file, never into one machine's `plan/` or an agent's memory, where the other machines would
not see it. The next task id is the next one free in `ROADMAP.md` and the open pull requests on
GitHub, not in one machine's `STATUS.md`.

## 9. Pull requests

- **An independent review before every push.** Before each push to a pull request branch,
  someone other than the author reviews the diff (for an agent: a separate agent, given the code
  and never the author's reasoning). (Since #5,
  where each CodeRabbit round found new problems.)
- **Branches go to `OpenFOAM-BuildingPhysics/umcfGui`**, never to a fork. The remote's name
  differs between machines: check `git remote -v`. Before replying "fixed in <commit>", check
  the pull request's head (`headRefOid`) on GitHub. (Since #14, merged without its fixes.)
- **Every pull request targets `main`**, never another pull request's branch; at most 300 files;
  squash-merged. It is ready only when a CodeRabbit review covers its head: every thread is
  answered and resolved, and its outside-diff comments are read. (Since #16.)
- **Merges are manual.** No auto-merge: the maintainer merges. `--admin` only for the one pull
  request the maintainer names, never carried over to the next. Remote branches are deleted
  only when the maintainer says so. (Since #43.)
- **No attribution lines** (`Co-Authored-By:`, "Generated with …") in commit messages or pull
  request descriptions, whatever an agent's defaults say.
- **The docs come with the change** (§6, §8).
