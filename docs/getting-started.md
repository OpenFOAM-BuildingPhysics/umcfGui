# Getting Started

There are no releases yet. umcfGui builds from source with a recent stable Rust toolchain
(1.90 or newer).

```sh
git clone --recurse-submodules https://github.com/OpenFOAM-BuildingPhysics/umcfGui.git
cd umcfGui
cargo build --release
cargo xtask ci          # every quality gate: formatting, lints, tests, layering, catalog, assets, wasm, docs, licences
```

`--recurse-submodules` also fetches the urbanMicroclimateFoam source into `solver/` (about 9 MB).
umcfGui doesn't build or link it: it is read by `cargo xtask catalog`, which `cargo xtask ci` runs, so
the gates need it. In a clone without it, `git submodule update --init` fetches it.

## What you can use today

The command line checks a case and edits it ([Command line](user-guide/command-line.md)):

```sh
cargo build --release -p umcf-cli
target/release/umcf-cli check path/to/case
```

The desktop app opens a case, shows it in 3D and runs the same commands
([The desktop app](user-guide/desktop.md)). The example cases carry no mesh, so mesh a copy with
OpenFOAM-12 first; the scripts write into the case, and the copy keeps the example as it is:

```sh
cargo build --release -p umcf-gui
run=$(mktemp -d) && cp -r examples/of12/streetCanyon_CFDHAM "$run/"
(cd "$run/streetCanyon_CFDHAM" && . ~/OpenFOAM-12/etc/bashrc && sh ./Allprepare)
target/release/umcf-gui "$run/streetCanyon_CFDHAM"
```

Without OpenFOAM, the app still opens an example as it is (dictionaries, fields, scripts and
checks), with nothing to draw in 3D.

Small example cases for OpenFOAM-8 and -12, made from the solver's street canyon tutorials, are
in `examples/` (see
[`examples/README.md`](https://github.com/OpenFOAM-BuildingPhysics/umcfGui/blob/main/examples/README.md)).

The foundation libraries, from Rust:

| Crate | Does |
|---|---|
| `umcf-foam-dict` | Reads, edits and writes OpenFOAM dictionaries without losing anything ([Dictionaries](user-guide/dictionaries.md)) |
| `umcf-foam-field` | Reads field files: ASCII, binary, gzip; time and processor directories ([Fields](user-guide/fields.md)) |
| `umcf-foam-mesh` | Reads `polyMesh` (points, faces, owner, neighbour, boundary, cell zones) and checks it ([Meshes](user-guide/meshes.md)) |
| `umcf-schema` | Works out a case's OpenFOAM version, its kind and its regions ([Cases](user-guide/cases.md)) |
| `umcf-core` | Case file access for the layers above |

```rust
use umcf_core::DirFs;
use umcf_foam_dict::Document;

// Which OpenFOAM version is this case for, and what are its regions?
let layout = umcf_schema::detect(&DirFs::new("path/to/case"))?;
println!("{:?} {:?}", layout.version, layout.regions);

// Change one scheme, keep everything else in the file as it was.
let mut doc = Document::parse(&std::fs::read_to_string("path/to/case/system/air/fvSchemes")?)?;
doc.set(&["divSchemes", "div(phi,U)"], "bounded Gauss upwind")?;
std::fs::write("path/to/case/system/air/fvSchemes", doc.to_string())?;
```

## API reference

`cargo doc --workspace --no-deps --open` builds the API reference of every crate.
