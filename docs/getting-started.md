# Getting Started

There are no releases yet. umcfGui builds from source with a recent stable Rust toolchain
(1.90 or newer).

```sh
git clone --recurse-submodules https://github.com/OpenFOAM-BuildingPhysics/umcfGui.git
cd umcfGui
cargo build --release
cargo xtask ci          # every quality gate: formatting, lints, tests, layering, wasm, licences
```

`--recurse-submodules` also fetches the urbanMicroclimateFoam source into `solver/` (about 9 MB).
umcfGui doesn't build or link it: it is there to read, and later for the catalog check. In a clone
without it, `git submodule update --init` fetches it.

## What you can use today

The foundation libraries, from Rust:

| Crate | Does |
|---|---|
| `umcf-foam-dict` | Reads, edits and writes OpenFOAM dictionaries without losing anything ([Dictionaries](user-guide/dictionaries.md)) |
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
