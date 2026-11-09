# Example cases

Small urbanMicroclimateFoam cases, for OpenFOAM-8 (`of8/`) and OpenFOAM-12 (`of12/`), that run
in seconds: to try umcfGui without cases of your own, and to test it and the solver.

| Case | Regions | Exercises |
|---|---|---|
| `streetCanyon_CFD` | air | wind, temperature and humidity in a street canyon (RANS, buoyancy, atmospheric boundary layer inlet) |
| `streetCanyon_CFDHAM` | air, leeward, windward, street | heat and moisture transport in the walls and the street, coupled to the air; solar and long-wave radiation with view factors |
| `streetCanyon_CFDHAM_grass` | as above | grass on the street |
| `streetCanyon_CFDHAM_veg` | as above, and vegetation | a tree in the canyon: leaf area density, the vegetation region split from the air for radiation |

## Running one

Copy it into a new directory first: the scripts write the mesh, logs and results into the case,
and `Allrun` skips a step whose log is already there.

```sh
run=$(mktemp -d) && cp -r examples/of12/streetCanyon_CFDHAM "$run/" && cd "$run/streetCanyon_CFDHAM"
. ~/OpenFOAM-12/etc/bashrc
sh ./Allprepare && sh ./Allrun
```

`streetCanyon_CFD` has no `Allprepare`: its `Allrun` makes the mesh too. To run a case again in
the same place, `sh ./Allclean` first.

Each runs serially for two hourly time steps (11:00 to 13:00, sun up), at most 30 fluid
iterations each. `cargo xtask examples` runs them all on temporary copies and checks that each
ended (see [Contributing](../docs/contributing.md#example-cases)).

## Where they come from

They are the street canyon cases of the
[urbanMicroclimateFoam tutorials](https://github.com/OpenFOAM-BuildingPhysics/urbanMicroclimateFoam-tutorials)
(OpenFOAM-BuildingPhysics) at two pinned commits, `64ae5d2` (2024-05-08) for `of8/` and
`b8135ba` (2026-03-23) for `of12/`, made small by `cargo xtask make-examples`. The tags
`of-org_v8.0` and `of-org_v12.0` have moved to later commits since, whose cases differ. The
changes:

- **Meshes:** the air's blocks have 3 to 6 uniform cells (about 2600 cells); the walls and the
  street keep their graded resolution through the material (at most 20 cells) and have the air's
  cells along it, so that each coupled face has one partner.
- **Runs:** serial (OpenFOAM-12's `Allrun` without `decomposePar` and `reconstructPar`,
  `runApplication` for `runParallel`; the OpenFOAM-8 ones were serial already), `endTime 7200`
  with `maxFluidIteration 30`, and lower relaxation for the air (`p_rgh` 0.3, `U` 0.5, `h` 0.5)
  so the coarse mesh's first iterations stay stable. The probes write at every step.
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

## The settings of a new case

`case.new` takes the settings of each component from these cases, as they
are here (`crates/schema/src/defaults.rs`, built in with `include_str!`): `streetCanyon_CFDHAM`'s
air, `street` and radiation files, `_grass`'s `grassProperties`, `_veg`'s vegetation region and
`vegetationProperties` ([A new case](https://github.com/OpenFOAM-BuildingPhysics/umcfGui/blob/main/docs/user-guide/new-case.md)).
A change made here (by `make-examples`) changes what new cases start from: the air's lower
relaxation above is a new case's too.

`cargo xtask make-examples` writes both directories again; `cargo xtask make-examples --check`
fails unless they are what it makes, byte for byte (CI runs it). It reads the tutorials from
`UMCF_TUTORIALS` (a clone that has both commits), else it fetches them from GitHub. It refuses to
write while `of8/` or `of12/` hold files git doesn't have as committed. A change to
the examples is a change to the generator (`xtask/src/make_examples.rs`), never to the files.
