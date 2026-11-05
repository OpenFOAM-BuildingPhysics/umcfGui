# Example cases

Small urbanMicroclimateFoam cases, for OpenFOAM-12 (`of12/`; the OpenFOAM-8 ones, `of8/`, follow),
that run in seconds: to try umcfGui without cases of your own, and to test it and the solver.

| Case | Regions | Exercises |
|---|---|---|
| `streetCanyon_CFD` | air | wind, temperature and humidity in a street canyon (RANS, buoyancy, atmospheric boundary layer inlet) |
| `streetCanyon_CFDHAM` | air, leeward, windward, street | heat and moisture transport in the walls and the street, coupled to the air; solar and long-wave radiation with view factors |
| `streetCanyon_CFDHAM_grass` | as above | grass on the street |
| `streetCanyon_CFDHAM_veg` | as above, and vegetation | a tree in the canyon: leaf area density, the vegetation region split from the air for radiation |

## Running one

Copy it first: the scripts write the mesh, logs and results into the case.

```sh
cp -r examples/of12/streetCanyon_CFDHAM /tmp/ && cd /tmp/streetCanyon_CFDHAM
. ~/OpenFOAM-12/etc/bashrc
sh ./Allprepare && sh ./Allrun
```

Each runs serially for two hourly time steps (11:00 to 13:00, sun up), at most 30 fluid
iterations each. `cargo xtask examples` runs them all on temporary copies and checks that each
ended (see [Contributing](../docs/contributing.md#example-cases)).

## Where they come from

They are the street canyon cases of the
[urbanMicroclimateFoam tutorials](https://github.com/OpenFOAM-BuildingPhysics/urbanMicroclimateFoam-tutorials)
(OpenFOAM-BuildingPhysics), tag `of-org_v8.0` for `of8/` and branch `of-org_v12.0` for `of12/`,
made small:

- **Meshes:** the air's blocks have 3 to 6 uniform cells (about 2600 cells); the walls and the
  street keep their graded resolution through the material (at most 20 cells) and have the air's
  cells along it, so that each coupled face has one partner.
- **Runs:** serial (`Allrun` without `decomposePar` and `reconstructPar`, `runApplication` for
  `runParallel`), `endTime 7200` with `maxFluidIteration 30`, and lower relaxation for the air
  (`p_rgh` 0.3, `U` 0.5, `h` 0.5) so the coarse mesh's first iterations stay stable. The probes
  write at every step.
- **Scripts:** `Allprepare` and `Allrun` stop at the first failing step (`set -e`); the
  vegetation case's removal of what splitMeshRegions may not write is `rm -rf`.
- **Inputs:** the weather tables (`Tambient`, `wambient`) and the solar ones (`IDN`, `Idif`,
  `sunPosVector`) start at 11:00, so that the short run has the sun up. Each wall's `Tamb` reads
  its own region's `T_room.dat` (the tutorials cross them; the two tables are the same).
- **OpenFOAM-12:** `faceAgglomerate` is given the region's `viewFactorsDict` (without `-dict` it
  reads `system/viewFactorsDict`); the copies nothing reads are gone (`system/viewFactorsDict`,
  `constant/<region>/viewFactorsDict`, the `setset.batch` files of setSet, which OpenFOAM-12 doesn't
  have, and `constant/regionProperties`: the regions are in `system/controlDict`).
- **Both:** `reconstructScript` is gone (no parallel run).

The files these change are written again: the `blockMeshDict`s and the weather tables with LF
line endings, and all five tables with spaces instead of tabs. Everything else is as in the
tutorials, byte for byte.
