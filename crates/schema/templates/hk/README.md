# The `hk` profile

The settings of the Hong Kong benchmark (OpenFOAM-8), `3_analysis/3_umcFoam_vegLAD1`, that a new
case starts from with `case.new`'s `"profile": "hk"` (`crates/schema/src/profile.rs`). Settings
only: no meshes, fields, geometry or weather tables. Line ends made LF; nothing else changed.

| File | From the benchmark's |
|---|---|
| `controlDict` | `system/controlDict` (`case.new` takes out its function objects) |
| `regionProperties` | `constant/regionProperties` (`case.new` writes the region list) |
| `fluid/thermophysicalProperties`, `momentumTransport`, `g`, `grassProperties`, `vegetationProperties` | `constant/air/` (`grassPatches` is written by `case.new`) |
| `fluid/fvSchemes`, `fvSolution`, `fvOptions` | `system/air/` (the `fvOptions` header says `location "constant"`; `case.new` writes `"system"`) |
| `solid/fvSchemes`, `fvSolution` | `system/facades/` (the same as `ground`'s and `roofs`') |
| `solid/g` | `constant/facades/g` (the same as the others') |
| `solid/transportProperties` | `constant/ground/transportProperties` (its `Soil` and `HamstadConcrete` coefficients are what new materials of those models start from) |
| `vegetation/fvSchemes`, `fvSolution` | `system/vegetation/` |
| `radiation/radiationProperties`, `solarLoadProperties` | `constant/vegetation/` |
