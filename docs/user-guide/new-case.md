# A new case

`case.new` makes an urbanMicroclimateFoam case in a new (or empty) directory and opens it: its
regions, the components it uses, and the settings of each from a *profile*. It is the start of
[the common workflow](../common-workflow.md): the case's [workflow
description](workflow.md#the-workflow-description), `system/umcfWorkflow`, names what was made.

```sh
umcf-cli run --cmd 'case.new={"path": "canyon", "version": 12,
  "regions": [{"name": "air", "kind": "fluid"},
              {"name": "street", "kind": "solid", "materials": [{"name": "soil", "model": "Soil"}]},
              {"name": "vegetation", "kind": "vegetation"}],
  "radiation": "vegetation", "grass": ["air_to_street"], "temperatureLimit": true}'
```

In the desktop app it is *New case…* in the *More* group of the *Case* tab: its card starts with
the air alone for OpenFOAM-12.

## Components

A case is the air and whichever components it uses. The solver switches each one on by a file or
an entry of its own, so a case has a component's files only when it uses it:

| Component | Switched on in the solver by | What `case.new` writes |
|---|---|---|
| the air (always) | the `fluid` region list | per fluid: `constant/<r>/thermophysicalProperties`, `momentumTransport`, `g`; `system/<r>/fvSchemes`, `fvSolution`, and OpenFOAM-12's `fvConstraints` (the pressure limit, which OpenFOAM-8 has in `fvSolution`) |
| heat and moisture in solids | the `solid` region list | per solid: `constant/<r>/g` (no gravity), `transportProperties` (its materials); `system/<r>/fvSchemes`, `fvSolution` |
| vegetation | the `vegetation` region list, `vegetationProperties` in the air | the vegetation region's `system/<r>/fvSchemes`, `fvSolution`; the air's `vegetationProperties` |
| radiation (`radiation`: the region it is computed on, a fluid or the vegetation) | `radiationProperties` in that region | its `radiationProperties` and `solarLoadProperties` |
| grass (`grass`: `true`, or the patches it grows on) | `grassProperties` in the air | the air's `grassProperties`, with `grassPatches` |
| a temperature limit (`temperatureLimit`) | a `limitTemperature` entry | OpenFOAM-8: `system/<air>/fvOptions`; OpenFOAM-12: `limitT` in `fvConstraints` (`select all`) |
| the blending layer (`blending`) | `blending` in `controlDict` | `blending true` and `blendingCoeffs` |

And for every case: `system/controlDict` (the profile's run controls without its function
objects, `application urbanMicroclimateFoam`, and OpenFOAM-12's `regions { fluid (…); solid (…);
vegetation (…); }`, a kind without regions left out), OpenFOAM-8's `constant/regionProperties`
(every kind, an empty one too: OpenFOAM-8's solver and `solarRayTracingGen` look each one up and
stop without it), and `system/umcfWorkflow` with the profile and the components:

```c++
profile     tutorial;
components  ( grass temperatureLimit );
```

The grass and the vegetation's model are `air`'s, the region the solver looks them up in.
The patches `grassPatches` names are kept in `grassProperties` only, not in the description.
Gravity points down the axis `up` names (`x`, `y` or `z`, the default: every case at hand but
the OpenFOAM-8 tutorials has z up); a solid's `g` is none, and the vegetation region has none
(the solver doesn't read it).

A solid's materials (`[{"name": "soil", "model": "Soil"}]`, each the cell zone built of it) go
into its `buildingMaterials`, each with the coefficients (`rho`, `cap`, `lambda1`, …) of the
profile's first material of that model; a model the profile has no material of gets none, and a
note says to add them. A model the solver doesn't have for the version is refused.

## Profiles

The settings each component's files start from:

| Profile | Versions | From |
|---|---|---|
| `tutorial` (the default) | 8, 12 | the urbanMicroclimateFoam tutorials, as [the examples](https://github.com/OpenFOAM-BuildingPhysics/umcfGui/blob/main/examples/README.md) have them: `streetCanyon_CFDHAM` (the air, the solids, the radiation), `_grass`, `_veg`. Byte for byte, line ends made LF. The tutorials have no temperature limit: its bounds (273.15 to 373.15 K) are the 3×3 reference cases', its OpenFOAM-8 file the benchmark's; nor a blending layer: its coefficients are the benchmark's |
| `hk` | 8 | the Hong Kong benchmark (`3_umcFoam_vegLAD1`), its settings in `crates/schema/templates/hk/`: `realizableKE`, its `fvSchemes` and `fvSolution`, one solid iteration per step, `limitTemperature` 283.15 to 370.15 K, the blending layer, Picard tolerances. OpenFOAM-12 comes later |

The tutorial's OpenFOAM-12 `fvConstraints` has a `limitPressure` entry, which the case check
points out has no effect in urbanMicroclimateFoam (`fv.limit-pressure`): it is the tutorials'
file, kept as it is.

## What it leaves to later steps

The meshes and their dictionaries, the patches, the fields of `0/`, the process counts
(`decomposeParDict`), the weather and sun tables (`Tambient`, `sunPosVector`, …), the view
factor dictionary and the scripts. The case check says what is missing for a run (`case.table`
for the tables the radiation reads, `region.no-mesh`). The blending layer takes its inflow from
the air's patches `west`, `east`, `north` and `south` and crashes without one: the check says so
(`control.blending`).

## What is refused

Before anything is written:

- a directory that isn't empty, a path with `..`, an open case with unsaved changes (unless
  `discard`), a profile without the version;
- a region name that isn't a plain word (letters, digits, `_`, `.`, `-`, not starting with a
  digit or `-`, not ending with `.`), the name of a file or directory directly in `constant/` or
  `system/` (`polyMesh`, `controlDict`, `fvSchemes`, `g`, `IDN`, …, in any case), a name Windows
  keeps for a device (`con`, `nul`, `com1`, …), two names that differ only by case (they can't
  live together on Windows or macOS), a name given twice;
- what the solver can't run, as it looks regions up by name: no fluid region, a vegetation
  region not named `vegetation` or more than one, vegetation without the radiation on it (the
  trees' model reads `qr` on `vegetation`'s `air_to_vegetation`), radiation without a fluid
  named `air` (`viewFactorSky` looks it up), grass without the radiation (it reads `qr` and `qs`
  on its patches), radiation on a solid or a region that isn't there,
  the blending layer without z up (it takes west and east along x, north and south along y);
- materials of a region that isn't a solid, without a model, of a model the solver doesn't have
  for the version, or given twice; grass patches without grass, given twice, or not plain names.

The files are written into a directory of their own inside `path` (`.umcfgui-new-<process>`)
and moved up once all are written. A directory that was there stays (its owner, its mode); one
made for the case, and the parents made with it, are removed again when it fails, as is
everything written: a failure leaves things as they were (what couldn't be removed, a file
held open on Windows say, is named). A case that was made but didn't open (cancelled) is said so:
`case.open` opens it.

## Adding a region

`region.add` adds a region to a case, made by `case.new` or not:

```sh
umcf-cli run --case canyon \
  --cmd 'region.add={"region": "roofs", "kind": "solid", "materials": [{"name": "brick", "model": "Hamstad5Brick"}]}' \
  --cmd 'case.save={}'
```

It names the region in the list the case's version reads: OpenFOAM-12's `controlDict`
(`solid ( street roofs );`) or OpenFOAM-8's `constant/regionProperties` (`solid (street roofs)`).
The list keeps its layout and comments; a name that would follow a `// …` comment goes on a
line of its own. It then writes the settings of the region's kind, as
`case.new` does:

- the settings are the tutorials', or those of the description's `profile` for a case made with
  one;
- a fluid gets the gravity of the case's fluids (when it is a vector written out), and the
  temperature limit when the case has one;
- a solid given no materials takes those the description gives it, when it is described already
  (one without a model is left out, with a note; one whose model the version doesn't have is
  refused, saying it is the description's);
- for the vegetation, `air` gets `vegetationProperties`. A note says when the radiation isn't on
  the vegetation yet.

A file that is there already is kept as it is. The region has no mesh yet. The workflow
description names it, with its materials, when the case has one. The whole addition is one step
in the history, and `case.save` writes it. What `case.new` refuses of a region is refused here
too, and so are:

- a name the case has, or one differing from it only by case;
- a case without a known version or a region list (a meshing step);
- a link or a directory where a file would go.

In the desktop app it is *Add a region…* in the *More* group of the *Case* tab.

## Tested

- `case.new`: the street canyon's regions made with each profile and version, with the example's
  meshes, fields and tables copied in, run with the solver: OpenFOAM-12 and -8 with `tutorial`,
  OpenFOAM-8 with `hk` (without the blending layer, which needs the four sides the canyon doesn't
  have).
- `region.add`: the same, with the case made with `air` and `street` only, and `leeward` and
  `windward` added with `region.add`.
