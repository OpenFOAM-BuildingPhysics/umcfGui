# A new case

`case.new` makes an urbanMicroclimateFoam case in a new (or empty) directory and opens it: its
regions, the components it uses, and the settings of each from [one configuration](#the-configuration).
It is the start of
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

And for every case: `system/controlDict` (the examples' run controls without their function
objects, `application urbanMicroclimateFoam`, and OpenFOAM-12's `regions { fluid (…); solid (…);
vegetation (…); }`, a kind without regions left out), OpenFOAM-8's `constant/regionProperties`
(every kind, an empty one too: OpenFOAM-8's solver and `solarRayTracingGen` look each one up and
stop without it), and `system/umcfWorkflow` with the components:

```c++
components  ( grass temperatureLimit );
```

The grass and the vegetation's model are `air`'s, the region the solver looks them up in.
The patches `grassPatches` names are kept in `grassProperties` only, not in the description.
A component is switched on or off later with [`workflow.component`](workflow.md#components).
Gravity points down the axis `up` names (`x`, `y` or `z`, the default: every case at hand but
the OpenFOAM-8 tutorials has z up); a solid's `g` is none, and the vegetation region has none
(the solver doesn't read it).

A solid's materials (`[{"name": "soil", "model": "Soil"}]`, each the cell zone built of it) go
into its `buildingMaterials`, each with the coefficients (`rho`, `cap`, `lambda1`, …) of its model
in the [material library](workflow.md#settings) chosen (`settings`' `materials`), else of the
tutorials' first material of that model (`Soil`, `Hamstad5Brick`); another model gets none, and
a note says to add them. A model the solver doesn't have for the version is refused, and so is
a library entry without a coefficient the solver reads of every material (`rho`, `cap`,
`lambda1`, `lambda2`).

## The configuration

There is one: every new case starts from the same settings, for OpenFOAM-8 and -12. Some are
chosen with `settings` (as [`workflow.settings`](workflow.md#settings) takes them: run times,
turbulence model, temperature-limit bounds, blending coefficients, material library, solver
numerics; not `from`, which is `workflow.settings`' own), and kept in the description;
the case is then changed with the commands that edit it (`workflow.settings`, `control.set`,
`dict.set`, `bc.set`, …).
The settings are those of the urbanMicroclimateFoam tutorials, as [the
examples](https://github.com/OpenFOAM-BuildingPhysics/umcfGui/blob/main/examples/README.md) have
them: `streetCanyon_CFDHAM` (the air, the solids, the radiation), `_grass`, `_veg`, byte for byte
with line ends made LF (`crates/schema/src/defaults.rs`). The examples change the tutorials'
run controls (`endTime 7200`, `maxFluidIteration 30`) and lower the air's relaxation (`p_rgh`
0.3, `U` 0.5, `h` 0.5) so that their coarse meshes' first iterations stay stable, and a new
case starts from that too. The tutorials have no temperature limit: its bounds (273.15 to
373.15 K) are the 3×3 reference cases', and its OpenFOAM-8 `fvOptions` is written here
(`crates/schema/templates/of8/`); nor a blending layer: its coefficients are the Hong Kong
benchmark's.

A `profile` entry that an earlier `case.new` wrote into `system/umcfWorkflow` is read and left
out.

The tutorials' OpenFOAM-12 `fvConstraints` has a `limitPressure` entry, which the case check
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
  `discard`);
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
line of its own. It then writes the settings of the region's kind, as `case.new` does, the
[configuration](#the-configuration)'s but for these:

- a fluid gets the gravity of the case's fluids (when it is a vector written out), the
  temperature limit when the case has one, and the RAS model and limit bounds the description's
  [settings](workflow.md#settings) choose, else those of the case's fluids when they agree (a
  note says when they don't, and the configuration's are taken);
- a solid's materials take the coefficients of their model in the description's material
  library, else of the case's own first material of it, else of the tutorials'; a library entry
  written by hand that the solver can't read (a model it doesn't have, a coefficient it reads of
  every material missing) is left out with a note;
- a solid given no materials takes those the description gives it, when it is described already
  (one without a model is left out, with a note; one whose model the version doesn't have is
  refused, saying it is the description's);
- for the vegetation, `air` gets `vegetationProperties`. A note says when the radiation isn't on
  the vegetation yet.

A file that is there already is kept as it is. The region has no mesh yet. The workflow
description names it, with its materials, when the case has one. The whole addition is one step
in the history, and `case.save` writes it. A material model the case's solids have already
is taken even when the solver's catalog lacks it (one of the case's own library). Otherwise
what `case.new` refuses of a region is refused here too, and so are:

- a name the case has, or one differing from it only by case;
- a case without a known version or a region list (a meshing step);
- a link or a directory where a file would go.

In the desktop app it is *Add a region…* in the *More* group of the *Case* tab.

## Tested

- `case.new`: the street canyon's regions made for each version, with the example's meshes,
  fields and tables copied in, run with the solver: OpenFOAM-12 and -8.
- `region.add`: the same, with the case made with `air` and `street` only, and `leeward` and
  `windward` added with `region.add`.
