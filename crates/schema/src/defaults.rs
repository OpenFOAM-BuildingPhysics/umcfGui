//! The settings a new case starts from (`case.new`): per OpenFOAM version, the files
//! of each component a case can have (see `docs/common-workflow.md`):
//!
//! | Component | Switched on in the solver by | Files |
//! |---|---|---|
//! | the air (always) | the `fluid` region list | a fluid's `thermophysicalProperties`, `momentumTransport`, `g`, `fvSchemes`, `fvSolution` (and OpenFOAM-12's `fvConstraints`); `controlDict`; OpenFOAM-8's `regionProperties` |
//! | heat and moisture in solids | the `solid` region list | a solid's `g`, `fvSchemes`, `fvSolution`, `transportProperties` |
//! | radiation | `radiationProperties` in a region | `radiationProperties`, `solarLoadProperties` |
//! | grass | `grassProperties` in a fluid | `grassProperties` |
//! | vegetation | the `vegetation` region list, `vegetationProperties` in a fluid | the vegetation region's `fvSchemes`, `fvSolution`; `vegetationProperties` |
//! | a temperature limit | a `limitTemperature` entry | OpenFOAM-8's `fvOptions`, an entry of OpenFOAM-12's `fvConstraints` |
//! | the blending layer | `blending` in `controlDict` | `blending`, `blendingCoeffs` |
//!
//! There is one configuration: the urbanMicroclimateFoam tutorials' settings, as `examples/` has
//! them (the street canyons, one component after another: `streetCanyon_CFD`, `_CFDHAM`,
//! `_CFDHAM_grass`, `_CFDHAM_veg`). Where the tutorials have no file (OpenFOAM-8's `fvOptions`)
//! it is in `crates/schema/templates/`, written here. A case's own settings are then changed with
//! the commands that edit it (`control.set`, `dict.set`, …).

use umcf_core::OfVersion;

/// One file of the configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Template {
    ControlDict,
    /// OpenFOAM-8's region list; OpenFOAM-12 has it in `controlDict`.
    RegionProperties,
    FluidThermo,
    FluidMomentum,
    FluidG,
    FluidSchemes,
    FluidSolution,
    /// OpenFOAM-12's pressure limit (`limitPressure`); OpenFOAM-8 has it in `fvSolution`.
    FluidConstraints,
    /// OpenFOAM-8's temperature limit (`limitTemperature`); OpenFOAM-12 has it in
    /// `fvConstraints`.
    FluidOptions,
    SolidSchemes,
    SolidSolution,
    SolidG,
    /// A solid's materials: the coefficients of each material model it has are what a new
    /// material of that model starts from.
    SolidTransport,
    VegetationSchemes,
    VegetationSolution,
    Radiation,
    SolarLoad,
    Grass,
    Vegetation,
}

macro_rules! example {
    ($v:literal, $case:literal, $path:literal) => {
        include_str!(concat!("../../../examples/", $v, "/streetCanyon_", $case, "/", $path))
    };
}

/// The temperature limit's bounds, in kelvin, as written. The tutorials have none: these are
/// the 3×3 reference cases'.
pub const TEMPERATURE_LIMIT: (&str, &str) = ("273.15", "373.15");

/// The blending layer's coefficients, as a dictionary's body. The tutorials have none: these
/// are the Hong Kong benchmark's.
pub const BLENDING_COEFFS: &str = "{ dampingThickness 60; alphaCoeffU 0.3; alphaCoeffT 0.3; }";

/// The text of `t` for `version`; `None` for a file that version keeps elsewhere
/// (OpenFOAM-8's `fvConstraints`, OpenFOAM-12's `regionProperties` and `fvOptions`).
pub fn template(version: OfVersion, t: Template) -> Option<&'static str> {
    use Template as T;
    Some(match version {
        OfVersion::V8 => match t {
            T::ControlDict => example!("of8", "CFDHAM", "system/controlDict"),
            T::RegionProperties => example!("of8", "CFDHAM", "constant/regionProperties"),
            T::FluidThermo => example!("of8", "CFDHAM", "constant/air/thermophysicalProperties"),
            T::FluidMomentum => example!("of8", "CFDHAM", "constant/air/momentumTransport"),
            T::FluidG => example!("of8", "CFDHAM", "constant/air/g"),
            T::FluidSchemes => example!("of8", "CFDHAM", "system/air/fvSchemes"),
            T::FluidSolution => example!("of8", "CFDHAM", "system/air/fvSolution"),
            T::FluidConstraints => return None,
            // The tutorials have no temperature limit: the file is written here.
            T::FluidOptions => include_str!("../templates/of8/fvOptions"),
            T::SolidSchemes => example!("of8", "CFDHAM", "system/street/fvSchemes"),
            T::SolidSolution => example!("of8", "CFDHAM", "system/street/fvSolution"),
            T::SolidG => example!("of8", "CFDHAM", "constant/street/g"),
            T::SolidTransport => example!("of8", "CFDHAM", "constant/street/transportProperties"),
            T::VegetationSchemes => example!("of8", "CFDHAM_veg", "system/vegetation/fvSchemes"),
            T::VegetationSolution => example!("of8", "CFDHAM_veg", "system/vegetation/fvSolution"),
            T::Radiation => example!("of8", "CFDHAM", "constant/air/radiationProperties"),
            T::SolarLoad => example!("of8", "CFDHAM", "constant/air/solarLoadProperties"),
            T::Grass => example!("of8", "CFDHAM_grass", "constant/air/grassProperties"),
            T::Vegetation => example!("of8", "CFDHAM_veg", "constant/air/vegetationProperties"),
        },
        OfVersion::V12 => match t {
            T::ControlDict => example!("of12", "CFDHAM", "system/controlDict"),
            T::RegionProperties | T::FluidOptions => return None,
            T::FluidThermo => example!("of12", "CFDHAM", "constant/air/thermophysicalProperties"),
            T::FluidMomentum => example!("of12", "CFDHAM", "constant/air/momentumTransport"),
            T::FluidG => example!("of12", "CFDHAM", "constant/air/g"),
            T::FluidSchemes => example!("of12", "CFDHAM", "system/air/fvSchemes"),
            T::FluidSolution => example!("of12", "CFDHAM", "system/air/fvSolution"),
            T::FluidConstraints => example!("of12", "CFDHAM", "system/air/fvConstraints"),
            T::SolidSchemes => example!("of12", "CFDHAM", "system/street/fvSchemes"),
            T::SolidSolution => example!("of12", "CFDHAM", "system/street/fvSolution"),
            T::SolidG => example!("of12", "CFDHAM", "constant/street/g"),
            T::SolidTransport => example!("of12", "CFDHAM", "constant/street/transportProperties"),
            T::VegetationSchemes => example!("of12", "CFDHAM_veg", "system/vegetation/fvSchemes"),
            T::VegetationSolution => example!("of12", "CFDHAM_veg", "system/vegetation/fvSolution"),
            T::Radiation => example!("of12", "CFDHAM", "constant/air/radiationProperties"),
            T::SolarLoad => example!("of12", "CFDHAM", "constant/air/solarLoadProperties"),
            T::Grass => example!("of12", "CFDHAM_grass", "constant/air/grassProperties"),
            T::Vegetation => example!("of12", "CFDHAM_veg", "constant/air/vegetationProperties"),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Template; 19] = [
        Template::ControlDict,
        Template::RegionProperties,
        Template::FluidThermo,
        Template::FluidMomentum,
        Template::FluidG,
        Template::FluidSchemes,
        Template::FluidSolution,
        Template::FluidConstraints,
        Template::FluidOptions,
        Template::SolidSchemes,
        Template::SolidSolution,
        Template::SolidG,
        Template::SolidTransport,
        Template::VegetationSchemes,
        Template::VegetationSolution,
        Template::Radiation,
        Template::SolarLoad,
        Template::Grass,
        Template::Vegetation,
    ];

    #[test]
    fn every_file_of_each_version_reads() {
        for v in [OfVersion::V8, OfVersion::V12] {
            for t in ALL {
                let Some(text) = template(v, t) else {
                    // Only what the version keeps elsewhere is missing.
                    let elsewhere = match v {
                        OfVersion::V8 => t == Template::FluidConstraints,
                        OfVersion::V12 => matches!(t, Template::RegionProperties | Template::FluidOptions),
                    };
                    assert!(elsewhere, "{v:?}: no {t:?}");
                    continue;
                };
                let doc = umcf_foam_dict::Document::parse(text).unwrap_or_else(|e| panic!("{v:?} {t:?}: {e}"));
                assert!(doc.root.problems().is_empty(), "{v:?} {t:?}: {:?}", doc.root.problems());
            }
        }
    }
}
