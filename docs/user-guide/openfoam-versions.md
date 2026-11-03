# OpenFOAM versions

urbanMicroclimateFoam is maintained for OpenFOAM-org v8 and v12, and projects contain cases of
both, often partly ported. umcfGui works out which version each case is written for.

## The banner can't be trusted

The header comment of OpenFOAM files says which version wrote the file, but copied files keep
their banner: cases ported to OpenFOAM-12 still say `Version: 2.2.2`. umcfGui uses the banner
only as a weak hint and looks at what the files are instead.

## Evidence

Each signal adds weight to one version; the higher score wins.

| Signal | Points to | Weight |
|---|---|---|
| `regions { … }` in `system/controlDict` (read by the OpenFOAM-12 port of UMCF) | OF12 | 5 |
| `constant/regionProperties` with `regions` (read by the OpenFOAM-8 solver) | OF8 | 5 |
| `fvModels` or `fvConstraints` in a region's `system/` or `constant/` | OF12 | 2 each |
| `fvOptions` in a region's `system/` or `constant/` | OF8 | 2 each |
| `constant/<region>/physicalProperties` (OpenFOAM-10 and newer) | OF12 | 2 each |
| `insidePoint` in `snappyHexMeshDict` (OpenFOAM-8 doesn't know it) | OF12 | 2 |
| `locationInMesh` in `snappyHexMeshDict` (OpenFOAM-12 still accepts it) | OF8 | 1 |
| `Version: 8`, or `9` to `12`, in the `controlDict` banner | OF8 / OF12 | 1 |

A meshing step with nothing but, say, an `extrudeMeshDict` carries no evidence. umcfGui then
reports the version as unknown rather than guessing.

## Leftovers from porting

When both versions have evidence, the case is **mixed**. umcfGui keeps the winner and lists
the files the winning version doesn't read, for example:

- `constant/regionProperties` in an OpenFOAM-12 case (the regions come from `controlDict`),
- `system/air/fvOptions` in an OpenFOAM-12 case (replaced by `fvModels` and `fvConstraints`).

These are notes, not errors: the solver runs. They are worth cleaning up, because they
suggest settings that have no effect.

## How it is checked

The detection is tested against every snappyHexMesh tutorial of OpenFOAM-8 and OpenFOAM-12,
and against real UMCF projects of both versions, including partly ported ones. See
[Contributing](../contributing.md#tests-against-real-cases).
