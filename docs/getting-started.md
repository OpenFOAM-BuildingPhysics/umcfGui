# Getting Started

There are no releases yet. umcfGui builds from source with a recent stable Rust toolchain
(1.90 or newer).

```sh
git clone https://github.com/OpenFOAM-BuildingPhysics/umcfGui.git
cd umcfGui
cargo build --release
cargo xtask ci          # every quality gate: formatting, lints, tests, layering, wasm, licences
```

## What you can use today

The foundation libraries, from Rust:

| Crate | Does |
|---|---|
| `umcf-foam-dict` | Reads, edits and writes OpenFOAM dictionaries without losing anything ([Dictionaries](user-guide/dictionaries.md)) |
| `umcf-foam-field` | Reads field files: ASCII, binary, gzip; time and processor directories ([Fields](user-guide/fields.md)) |
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
