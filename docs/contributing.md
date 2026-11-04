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
cargo xtask catalog                                               # regenerate the catalog for the new pin
```

After moving the pin, the diff of `docs/solver-coverage.md` shows what the solver gained or lost:
new types, removed keywords, changed defaults.

Never commit inside `solver/` from this repository: changes to the solver are pull requests to
[urbanMicroclimateFoam](https://github.com/OpenFOAM-BuildingPhysics/urbanMicroclimateFoam).
Moving the pin is a commit of its own; check it with `cargo xtask ci` and the
[tests against real cases](#tests-against-real-cases).

## Quality gates

`cargo xtask ci` runs every gate, stopping at the first failure:

| Gate | Checks |
|---|---|
| `fmt` | `cargo fmt --check` |
| `clippy` | lints as errors, with and without default features |
| `test` | all tests, with and without default features |
| `layers` | the crate layering ([Architecture](architecture.md)) |
| `catalog --check` | the [solver catalog](solver-coverage.md) matches the pinned `solver/` (needs the submodule and its tags) |
| `assets` | every file under `assets/` and `docs/images/` has a row in `ATTRIBUTION.md` |
| `wasm` | the L0–L3 crates build for `wasm32-unknown-unknown` |
| `docs` | the API reference builds without warnings, and the site builds in strict mode |
| `deny` | every dependency's licence is GPL-3-compatible (needs `cargo install cargo-deny`) |

## Reviews

Every pull request gets a CodeRabbit review; `main` requires its check and `cargo xtask ci` to
pass, and every review thread to be resolved. A Claude Code review runs only when asked for,
never on its own: comment `/claude-review` on an open pull request. Only a comment from an
account with write access to the repository counts. It reviews the PR's head as it stands,
posts its findings as inline comments (each a thread to resolve before merging) and a summary
comment, and never runs the PR's code. The comment trigger runs the workflow as it is on
`main`. It needs the repository secret `CLAUDE_CODE_OAUTH_TOKEN` (`claude setup-token`, then
`gh secret set CLAUDE_CODE_OAUTH_TOKEN`).

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
| `UMCF_FIELDS_ALL_TIMES` | with `UMCF_CASES`: read the fields of every time directory, not only the first and last |
| `UMCF_FIELD_PAIR` | `<ascii case>:<binary case>`: a copy converted with `writeFormat binary;` and `foamFormatConvert -region <r>` must read back bit for bit the same, fields and meshes |
| `UMCF_FUZZ_ITERS` | more iterations for the dictionary and field fuzz tests |

```sh
UMCF_DICT_TREES=$HOME/OpenFOAM-8/tutorials:$HOME/OpenFOAM-12/tutorials cargo test -p umcf-foam-dict --test corpus
```

## Commits

One task id per commit (`M0.4: streamed field reader`), only green states.
