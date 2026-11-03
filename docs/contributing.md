# Contributing

The binding rules for every contributor, human or AI agent, are in
[`AGENTS.md`](https://github.com/OpenFOAM-BuildingPhysics/umcfGui/blob/main/AGENTS.md).
The short version:

- **Never lose a case:** dictionary edits are lossless; nothing is deleted without an explicit
  command.
- **Never crash:** no `unwrap`, `expect`, `panic!` or `unsafe` outside tests; case files and
  solver processes are hostile input.
- **Everything is a command**, and the [layering](architecture.md) is enforced.
- **The solver is the spec.** Read the urbanMicroclimateFoam source freely; ported code keeps
  its copyright line and names its source file and commit.

## The solver source

`solver/` is a git submodule: the urbanMicroclimateFoam repository, branch `master` (the
OpenFOAM-12 port), pinned to the commit umcfGui was last checked against. Read it freely, it is
the specification. Every version is a tag in the same clone:

```sh
git submodule update --init                                       # after a plain clone
git -C solver show of-org_v8.0:urbanMicroclimateFoam.C            # the OpenFOAM-8 version of a file
git submodule update --remote solver && git add solver            # move the pin to the solver's latest master
```

Never commit inside `solver/` from this repository: changes to the solver are pull requests to
[urbanMicroclimateFoam](https://github.com/OpenFOAM-BuildingPhysics/urbanMicroclimateFoam).
Moving the pin is a commit of its own.

## Quality gates

`cargo xtask ci` runs every gate, stopping at the first failure:

| Gate | Checks |
|---|---|
| `fmt` | `cargo fmt --check` |
| `clippy` | lints as errors, with and without default features |
| `test` | all tests, with and without default features |
| `layers` | the crate layering ([Architecture](architecture.md)) |
| `assets` | every file under `assets/` and `docs/images/` has a row in `ATTRIBUTION.md` |
| `wasm` | the L0–L3 crates build for `wasm32-unknown-unknown` |
| `docs` | the API reference builds without warnings, and the site builds in strict mode |
| `deny` | every dependency's licence is GPL-3-compatible (needs `cargo install cargo-deny`) |

## This site

The site is built with [Zensical](https://zensical.org) from `docs/` and `zensical.toml`:

```sh
uv venv --python 3.12
uv pip install -r requirements_docs.txt
.venv/bin/zensical serve      # live preview on http://localhost:8000
```

`cargo xtask docs` builds it in strict mode (broken links fail) when Zensical is installed,
either on the `PATH` or in `.venv/`. Every page under `docs/` must be in the `nav` of
`zensical.toml`.

## Tests against real cases

Real case files are never committed. Tests that read them are opt-in through environment
variables, and report a skip when they are unset:

| Variable | Test |
|---|---|
| `UMCF_CASES` | case directories (`:`-separated): every dictionary round-trips; every solver case has a version and regions |
| `UMCF_CASES_EXPECT` | pins versions: `<case path>=8,<case path>=12,…` |
| `UMCF_DICT_TREES` | directory trees, e.g. the OpenFOAM-8 and OpenFOAM-12 `tutorials/` and `etc/`: every file with a `FoamFile` header round-trips |
| `UMCF_OF8_TUTORIALS`, `UMCF_OF12_TUTORIALS` | the snappyHexMesh tutorials get their version |
| `UMCF_FUZZ_ITERS` | more iterations for the dictionary fuzz test |

```sh
UMCF_DICT_TREES=$HOME/OpenFOAM-8/tutorials:$HOME/OpenFOAM-12/tutorials cargo test -p umcf-foam-dict --test corpus
```

## Commits

One task id per commit (`M0.4: streamed field reader`), only green states.
