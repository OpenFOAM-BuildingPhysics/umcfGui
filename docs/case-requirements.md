# What a case needs

Everything an urbanMicroclimateFoam case needs to run: its regions and their names, the
dictionaries and fields of each region, the coupling between them, the meshing chain from STL
surfaces, and the radiation and weather inputs. This is the specification umcfGui builds new
cases against ([gaps](gaps.md) lists what it can build and check so far).

It is drawn from the solver source (`solver/`, the pinned commit and the `of-org_v8.0` tag), the
UMCF tutorials (`windAroundBuildings_CFDHAM_veg` for the chain from STL surfaces,
`streetCanyon_CFDHAM` for radiation without trees) and the reference cases. Source lines are
given as `file:line` in `solver/`. **R** marks what a run or utility stops without; *8* and
*12* mark differences between OpenFOAM-8 and OpenFOAM-12. Points not confirmed by a run are
marked *unconfirmed*.

## Regions

| Kind | Name | Notes |
|---|---|---|
| fluid | **`air`** when radiation is used | The sky temperature is read from `0/air/Tambient` and `viewFactorSky` looks up the mesh `air` (`viewFactorSky.C:1149`, `:1195`). Exactly one fluid region. *Unconfirmed*: whether another name works without radiation. |
| solid | any (`buildings`, `street`, `ground`, …) | Heat and moisture in porous materials; one `buildingMaterialModel` per material. |
| vegetation | **`vegetation`** | The radiation region when there are trees: a second mesh of the air domain with the tree volume cut out. The name is fixed in the solid temperature condition, `simplifiedVegetation`, `calcLAI` and `simpleGrass`. Only one is supported. |

The regions are listed in `system/controlDict` *12* (`regions { fluid (air); solid (…);
vegetation (…); }`, plus `regionSolvers` with the same regions for the `-allRegions` utilities)
or in `constant/regionProperties` *8* (`regions ( fluid (air) solid (…) vegetation (…) );`).

Radiation runs in one region:

- **without trees**, in `air` itself: `constant/air/radiationProperties`,
  `constant/air/solarLoadProperties`, the view factor dictionary, and `0/air/qr`, `0/air/qs`;
- **with trees**, in `vegetation`, whose patch **`air_to_vegetation`** (type `mappedInternal`) is
  the canopy surface (`simplifiedVegetation.C:323`). Radiation is not solved in `air` then.

## Dictionaries per region

| File | `air` | solid | `vegetation` |
|---|---|---|---|
| `constant/<r>/polyMesh` | R | R | R |
| `constant/<r>/g` | R | R | not read |
| `constant/air/thermophysicalProperties` | R: `heRhoThermo`, `pureMixture`, `const`, `hConst`, `Boussinesq`, `sensibleInternalEnergy` (*12* also reads `physicalProperties`) | – | – |
| `constant/air/momentumTransport` | R: `simulationType RAS;` with `RAS { model realizableKE; }` (*8* `RASModel`) | – | – |
| `constant/air/vegetationProperties` | with trees: `vegetationModel simplifiedVegetation;` and its `simplifiedVegetationCoeffs`; without, the model is `none`. `calcLAI` needs the file and the coefficients. | – | – |
| `constant/air/grassProperties` | optional (`simpleGrass` with `grassPatches`) | – | – |
| `constant/<r>/radiationProperties` | without trees: `radiationModel viewFactorSky;` with `viewFactorSkyCoeffs { smoothing; constantEmissivity; }` (both R) | – | with trees: the same |
| `constant/<r>/solarLoadProperties` | without trees: `solarLoadModel directAndDiffuse;` with `directAndDiffuseCoeffs { smoothing; constantAlbedo; }` | – | with trees: the same |
| `constant/<solid>/transportProperties` | – | R: `buildingMaterials ( { name; buildingMaterialModel; rho; cap; lambda1; lambda2; <Type>Coeffs {…} } … );` | – |
| `system/<r>/fvSchemes`, `fvSolution` | R | R | R *8*; a minimal one *12* |
| `system/decomposeParDict` | for parallel runs: the same number of subdomains in every region (*8* reads `system/<r>/decomposeParDict`) | | |

A region without `radiationProperties` has no radiation, and one without `solarLoadProperties`
no solar load; the solver doesn't stop.

**Materials and cell zones.** Every `buildingMaterials` `name` must be a cell zone of that solid
region, and the zones must cover every cell exactly once (`solid/updatebuildingMaterials.H`).
The zones come from `topoSet` *12* or `setSet -batch` *8*.

**What the equations read.**

- Air `fvSolution`: solvers for `p_rgh` and `(U|k|epsilon|w|h|e)`; `SIMPLE` with
  `nNonOrthogonalCorrectors` and `residualControl`; `relaxationFactors` for fields and equations.
  With `simplifiedVegetation`, `relaxationFactors` and `SIMPLE/residualControl` are R
  (`simplifiedVegetation.C:303-306`).
- Air `fvSchemes`: `ddt steadyState`, the `div` schemes for `U`, `e` (or `h`), `K`, `Ekp` *12*,
  `k`, `epsilon` and `w`, and the stress term.
- Solid `fvSolution`: solvers for `pc` and `Ts`. Solid `fvSchemes`: `ddt Euler` and
  `laplacian(Krel,pc)` (every laplacian of the solid equations has that name).

**Run controls** (`system/controlDict`, all with defaults): the solid sub-stepping
(`initialSolidTimestepFactor`, `minDeltaT`, `maxDeltaT`, the Picard tolerances), the fluid
iterations (`minFluidIteration`, `maxFluidIteration`), `viewFactorWorkflow` (`sparse` or `dense`;
read from `controlDict` before the model's coefficients), and `blending` with `blendingCoeffs` for
meso-scale nudging. `deltaT` is the fluid and radiation step; the solids step inside it.

## Fields

| Region | Fields |
|---|---|
| `air` | `U`, `p_rgh`, `p`, `T`, `w` (humidity ratio), `gcr` (catch ratio), `k`, `epsilon`, `nut`, `alphat`; `LAD` with trees; `qr`, `qs` when radiation runs in `air` |
| solid | `Ts`, `pc` (the primary variable), `ws` |
| `vegetation` | `T`, `qr`, `qs` |

Tables that are not fields: `0/air/Tambient` and `0/air/wambient` (outdoor temperature and
humidity ratio), `0/air/cloudCover`, `0/<solid>/T_room.dat` (indoor temperature). See
[time and weather](#time-and-weather).

### Boundary conditions by patch role

`air`:

| Role | `U` | `p_rgh` | `T` | `w` | `k` | `epsilon` | `nut` | `alphat` |
|---|---|---|---|---|---|---|---|---|
| inlet | `atmBoundaryLayerInletVelocity` | `fixedFluxPressure` | `uniformFixedValue` from `Tambient` | `uniformFixedValue` from `wambient` | from \|U\|² | `atmBoundaryLayerInletEpsilon` | `calculated` | `calculated` |
| outlet | `inletOutlet` | `fixedValue` | `inletOutlet` | `inletOutlet` | `zeroGradient` | `zeroGradient` | `calculated` | `calculated` |
| sides, top | `slip` | `slip` | `slip` (top: from `Tambient`) | `slip` | `slip` | `slip` | `calculated` | `calculated` |
| ground, not coupled | `fixedValue` 0 | `fixedFluxPressure` | `zeroGradient` | `zeroGradient` | `kqRWallFunction` | `epsilonWallFunction` | `nutkAtmRoughWallFunction` | `compressible::alphatWallFunction` |
| wall coupled to a solid | `fixedValue` 0 | `fixedFluxPressure` | `compressible::CFDHAMfluidTemperatureCoupledMixed` | `compressible::CFDHAMfluidMoistureCoupledImpermeable` (or `…CoupledMixed`) | `kqRWallFunction` | `epsilonWallFunction` | `nutkWallFunction` | `compressible::alphatWallFunction` |

`p` is `calculated` and `gcr` `fixedValue` everywhere. The inlet profile parameters (`Uref`,
`Zref`, `zDir`, `flowDir`, `z0`, `zGround`) usually come from an included `ABLConditions`.

Solid:

| Role | `Ts` | `pc` | `ws` |
|---|---|---|---|
| coupled to `air` | `compressible::CFDHAMsolidTemperatureCoupledMixed` with `qrNbr qr; qsNbr qs;` (`none` without radiation) | `compressible::CFDHAMsolidMoistureCoupledImpermeable` (or `…CoupledMixed`) | `zeroGradient` |
| building interior | `compressible::CFDHAMsolidTemperatureTransferCoeffInterior` with `hcoeff` and `Tamb` (the `T_room.dat` table) | `zeroGradient` | `zeroGradient` |
| ground bottom | `fixedValue` | `zeroGradient` or `fixedValue` | `zeroGradient` |
| cut sides | `zeroGradient` | `zeroGradient` | `zeroGradient` |

The radiation region (`vegetation`, or `air` without trees):

| Role | `T` (vegetation only) | `qr` | `qs` |
|---|---|---|---|
| every patch of `air`, by the same name | `mappedValue` *12* (`mapped` with `field`, `setAverage`, `average` *8*) | `greyDiffusiveRadiationViewFactor` with `emissivity` | `solarLoadRadiationViewFactor` with `albedo` |
| `air_to_vegetation` | `mappedLeafTemperature` | the same | the same |

### Coupling rules

1. **Patch types.** The CFDHAM conditions and `mappedValue` need a mapped patch;
   `mappedLeafTemperature` needs `mappedInternal`.
2. **Pairs.** An air wall coupled to a solid is `mappedWall` with `neighbourRegion <solid>;
   neighbourPatch <solid patch>;` and the solid patch is `mappedWall` back to `air` (*8*:
   `sampleMode nearestPatchFace; sampleRegion; samplePatch;`). Faces are matched by nearest, so
   the two sides need not match face for face.
3. **Radiation reaches the solids by name.** With a `vegetation` region, the solid temperature
   condition reads `qr` and `qs` from the **vegetation patch named like the air patch** it is
   coupled to (`CFDHAMsolidTemperatureCoupledMixed…C:351`). So every air patch coupled to a solid
   needs a patch of the same name in `vegetation`. Without trees, it reads them from the air patch.
4. **Sky or surface.** In the radiation region, every patch that is neither `wall` nor
   `mappedInternal` is **sky** (`viewFactorSky.C:1290-1296`): the domain sides and top are `patch`
   or `mapped`, the ground and buildings `wall` or `mappedWall`. Every patch whose `qr` is
   `greyDiffusiveRadiationViewFactor` needs an entry in the view factor dictionary.
5. **The vegetation region's patches** are `mapped` (sides and top) or `mappedWall` (ground and
   buildings) to the same-named air patch, and `air_to_vegetation` is `mappedInternal`
   (`offsetMode normal; distance 0;`).

## Meshing from STL surfaces

Real cases mesh in one of three ways:

- **block regions** (the 3×3 reference cases): the buildings are missing blocks of a multi-block
  `blockMeshDict`, and each solid is a `blockMesh` of its own;
- **from STL surfaces** (the `windAroundBuildings_CFDHAM_veg` tutorial): one background box,
  snappyHexMesh cuts the buildings out of the air, and the solids are extruded from the building
  surface and the ground;
- **conformal solids**: as above, but each solid is extruded from the snapped air patch (written
  out as a surface), so solid and air meet face for face.

The chain from STL surfaces, *12*:

| # | Step | Dictionary | What matters |
|---|---|---|---|
| 1 | `blockMesh` | `system/blockMeshDict` | One box covering every surface, ground at its lowest plane. Patches: `west`, `east`, `north`, `south`, `top` (`patch`), `ground` (`wall`). Variables as `$!backgroundMesh/xMin` *12*, `$:backgroundMesh.xMin` *8*. |
| 2 | `surfaceFeatures` | `system/surfaceFeaturesDict` | The building and tree surfaces. |
| 3 | keep a copy of the background mesh | | snappyHexMesh runs twice on it. |
| 4 | `snappyHexMesh` for `air` | `system/snappyHexMeshDict.air` | Buildings a refinement surface with `patchInfo { type wall; }` and layers; trees a distance refinement only; `locationInMesh` (*12* also `insidePoint`) in the air, outside every building and tree. A multi-region STL makes patches `<geometry>_<region>` unless renamed. |
| 5 | `changeDictionary -region air` | `system/air/changeDictionaryDict` | The coupled walls become `mappedWall` to their solids. |
| 6 | restore the background; `snappyHexMesh` for `vegetation` | `system/snappyHexMeshDict.veg` | As for `air`, with the trees also a wall surface, so their volume is cut out; no layers. |
| 7 | `createPatch -region vegetation` | `system/vegetation/createPatchDict` | The tree patch becomes `air_to_vegetation`. |
| 8 | `changeDictionary -region vegetation` | `system/vegetation/changeDictionaryDict` | Patch types per the coupling rules above. |
| 9 | copy the vegetation fields in | | The fields are kept aside (`0/vegetation.bckp`) while meshing. |
| 10–11 | `extrudeMesh`, two layers | `extrudeMeshDict` per layer | Layer 1 from the building surface, extruded inward (`flipNormals true`); layer 2 from layer 1's `otherSide` (`constructFrom mesh`). `nLayers`, `thickness`, `expansionRatio`. |
| 12 | move to `constant/buildings`; `createPatch` | `system/buildings/createPatchDict` | `originalPatch` becomes `buildings_to_air`, `otherSide` `interior`. |
| 13 | `changeDictionary -region buildings` | | `buildings_to_air` becomes `mappedWall` to the air's `buildings`. |
| 14 | `topoSet -region buildings` | `system/buildings/topoSetDict` | One cell zone per layer, **named as the materials**. |
| 15 | the ground region | | The same from a ground surface: patches `ground_to_air`, `bottom`, `sides`. |
| 16 | leaf area density | `topoSet -region air`, then `setFields` | The cells inside the tree surface become a zone (`trees`), which gets `LAD`; `setFieldsDict` names the zone with `zone` *12*, `name` *8*. |

*8* (the tutorial's `of-org_v8.0` tag): one extrusion layer, `setSet -batch`, `sampleMode
nearestPatchFace`.

**Names that must agree across the steps:**

| Name | Where | Rule |
|---|---|---|
| region | the region list, `constant/<r>`, `system/<r>`, `0/<r>`, `neighbourRegion`, the scripts | the same everywhere; `air` and `vegetation` fixed |
| air wall patch | the snappyHexMesh surface or blockMesh patch, `createPatch`, `changeDictionary`, every `0/air/*`, the **vegetation patch**, the **view factor dictionary**, the solid's `neighbourPatch` | the same everywhere |
| solid coupled patch | `createPatch`, the solid's `changeDictionary`, `0/<solid>/*`, the air's `neighbourPatch` | the same |
| tree patch | snappyHexMesh, `createPatch` | ends as `air_to_vegetation` |
| material zone | `topoSet`, `buildingMaterials name` | equal, covering every cell once |
| LAD zone | `topoSet`, `setFieldsDict` | equal |
| `locationInMesh` | both snappyHexMesh runs | in the air, outside every building and tree |
| background box | `blockMeshDict`, the surfaces | covers them; the ground plane is its lowest face |
| north | the sun table, the mesh axes | the mesh north-aligned (+y north, inferred), or the sun vectors rotated |

## Radiation

### Inputs

| Input | Where | Format | Read by |
|---|---|---|---|
| sun position | `constant/sunPosVector` | `( (t (x y z)) … )`: a unit vector **towards the sun** in mesh axes, z up | `solarRayTracingGen`, `calcLAI`, `directAndDiffuse`, `simplifiedVegetation` |
| direct normal irradiance | `constant/IDN` | `( (t W/m²) … )` | `solarRayTracingGen`, `calcLAI` |
| diffuse horizontal irradiance | `constant/Idif` | `( (t W/m²) … )` | `solarRayTracingGen` |
| sky long-wave | `constant/LW` (optional) | `( (t W/m²) … )` | `viewFactorSky`; without it the sky follows `Tambient` (Swinbank) and `cloudCover` |
| outdoor temperature | `0/air/Tambient` (path fixed) | `( (t K) … )` | `viewFactorSky`, the `T` conditions |
| cloud cover | `0/air/cloudCover` (path fixed; optional, 0) | `( (t 0..1) … )` | `viewFactorSky` |
| view factor dictionary | `system/<radiation region>/viewFactorsDict` *12* (`faceAgglomerate` reads `system/viewFactorsDict` unless given `-dict`); `constant/<radiation region>/viewFactorsDict` *8* | `writeFacesAgglomeration` (R), `skyPosVector (0 0 1)` (R), and per patch `{ nFacesInCoarsestLevel N; featureAngle a; }` (`nFacesInCoarsestLevel` R) | `faceAgglomerate`, `viewFactorsGen`, `solarRayTracingGen` |
| emissivity, albedo | the `qr` and `qs` conditions of the radiation region | per patch | the solver |
| leaf area density | `0/air/LAD` | field | `calcLAI`, `simplifiedVegetation` |

These tables are read through a file path, not as OpenFOAM objects, so `cargo xtask catalog`
doesn't list them in [solver coverage](solver-coverage.md).

### The chain

Run per processor once the case is decomposed, in this order:

| Utility | Writes | Re-run after |
|---|---|---|
| `faceAgglomerate -region <rad>` (OpenFOAM) | `finalAgglom` | a mesh, patch, view factor dictionary or decomposition change |
| `calcLAI -region air` (with trees) | `kcLAIboundary`, `divqrsw`, `LAI` | the above, the sun or irradiance tables, `LAD` |
| `viewFactorsGen -region <rad>` (OpenFOAM) | `F`, `globalFaceFaces`, `constructMap`, `constructMapDim`, `subMap` | a mesh, patch, view factor dictionary or decomposition change |
| `solarRayTracingGen -region <rad>` | `sunViewCoeff`, `skyViewCoeff`, `sunskyMap`, `solarLoadFineFaces` | any of the above |

`solarRayTracingGen` needs `kcLAIboundary` whenever the region list has a vegetation region, with
one entry per sun position. *Unconfirmed*: whether a vegetation region without trees then needs
`calcLAI` with `vegetationModel none` and a zero `LAD`.

## Time and weather

The solver has no calendar: every table is keyed by simulation time in seconds. In the reference
cases t = 0 is local midnight of the simulated day, `endTime 86400` and `deltaT 3600` give an
hourly day. The date, the site (latitude, longitude, time zone) and the mesh's north exist only
where the tables were made, so a GUI that builds a case keeps them itself and writes
`sunPosVector`, `IDN`, `Idif` (and `Tambient`, `wambient`, `cloudCover`, `LW`) from them.

| Table | Content | Referenced from |
|---|---|---|
| `0/air/Tambient`, `0/air/wambient` | outdoor temperature, humidity ratio | the `T` and `w` conditions (`uniformFixedValue` with `uniformValue tableFile; uniformValueCoeffs { file "$FOAM_CASE/0/air/Tambient"; }`); `Tambient` also by `viewFactorSky` |
| `0/<solid>/T_room.dat` | indoor temperature | `Tamb` of the interior condition |
| `0/air/cloudCover` | cloud cover | `viewFactorSky` |
| `constant/sunPosVector`, `IDN`, `Idif`, `LW` | sun and sky | [radiation](#radiation) |
| `constant/air/Utarget_<patch>/…` | meso-scale wind | `blending` |

The tables interpolate linearly and hold their last value past the end: a table that ends before
`endTime` goes on silently with its last value. Every table should cover the run and share its
time base.

## OpenFOAM-8 and OpenFOAM-12

| | OpenFOAM-8 | OpenFOAM-12 |
|---|---|---|
| region list | `constant/regionProperties` | `controlDict` `regions` and `regionSolvers` |
| mapped patches | `sampleMode`, `sampleRegion`, `samplePatch` | `neighbourRegion`, `neighbourPatch`, `method` |
| mapped value condition | `mapped` | `mappedValue` |
| sets and zones | `setSet -batch` | `topoSet` |
| `setFieldsDict` zone | `name` | `zone` |
| fv options | `system/<r>/fvOptions` | `system/<r>/fvConstraints`, `constant/<r>/fvModels` |
| turbulence model keyword | `RASModel` | `model` |
| view factor dictionary | `constant/<r>/` | `system/<r>/` |
| `blockMeshDict` variables | `$:dict.key` | `$!dict/key` |
| snappyHexMesh point | `locationInMesh` | `insidePoint` (or `locationInMesh`) |

More on telling and porting the two: [OpenFOAM versions](user-guide/openfoam-versions.md).
