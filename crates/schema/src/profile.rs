//! The settings a new case starts from (`case.new`): a profile gives, per OpenFOAM version, the
//! files of each component a case can have (see `docs/common-workflow.md`):
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
//! [`Profile::Tutorial`] is the urbanMicroclimateFoam tutorials, as `examples/` has them (the
//! street canyons, one component after another: `streetCanyon_CFD`, `_CFDHAM`, `_CFDHAM_grass`,
//! `_CFDHAM_veg`). [`Profile::Hk`] is the Hong Kong benchmark's set-up (OpenFOAM-8), its files in
//! `crates/schema/templates/hk/` (`README.md` there says where each comes from).

use umcf_core::OfVersion;

/// A set of settings for each component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Tutorial,
    Hk,
}

/// One file a profile gives.
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

macro_rules! hk {
    ($path:literal) => {
        include_str!(concat!("../templates/hk/", $path))
    };
}

impl Profile {
    pub const ALL: [Profile; 2] = [Profile::Tutorial, Profile::Hk];

    pub fn name(self) -> &'static str {
        match self {
            Profile::Tutorial => "tutorial",
            Profile::Hk => "hk",
        }
    }

    pub fn parse(s: &str) -> Option<Profile> {
        Profile::ALL.into_iter().find(|p| p.name() == s)
    }

    /// The OpenFOAM versions it has files for.
    pub fn versions(self) -> &'static [OfVersion] {
        match self {
            Profile::Tutorial => &[OfVersion::V8, OfVersion::V12],
            Profile::Hk => &[OfVersion::V8],
        }
    }

    /// The temperature limit's bounds, in kelvin, as written. The tutorials have none: theirs
    /// are the 3×3 reference cases'.
    pub fn temperature_limit(self) -> (&'static str, &'static str) {
        match self {
            Profile::Tutorial => ("273.15", "373.15"),
            Profile::Hk => ("283.15", "370.15"),
        }
    }

    /// The blending layer's coefficients, as a dictionary's body. The tutorials have none:
    /// theirs are the Hong Kong benchmark's.
    pub fn blending_coeffs(self) -> &'static str {
        "{ dampingThickness 60; alphaCoeffU 0.3; alphaCoeffT 0.3; }"
    }

    /// The text of `t` for `version`; `None` where the profile has none (no version of it,
    /// or a file that version doesn't have).
    pub fn template(self, version: OfVersion, t: Template) -> Option<&'static str> {
        use Template as T;
        match (self, version) {
            (Profile::Tutorial, OfVersion::V8) => Some(match t {
                T::ControlDict => example!("of8", "CFDHAM", "system/controlDict"),
                T::RegionProperties => example!("of8", "CFDHAM", "constant/regionProperties"),
                T::FluidThermo => example!("of8", "CFDHAM", "constant/air/thermophysicalProperties"),
                T::FluidMomentum => example!("of8", "CFDHAM", "constant/air/momentumTransport"),
                T::FluidG => example!("of8", "CFDHAM", "constant/air/g"),
                T::FluidSchemes => example!("of8", "CFDHAM", "system/air/fvSchemes"),
                T::FluidSolution => example!("of8", "CFDHAM", "system/air/fvSolution"),
                T::FluidConstraints => return None,
                // The tutorials have no temperature limit: the file is the benchmark's.
                T::FluidOptions => hk!("fluid/fvOptions"),
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
            }),
            (Profile::Tutorial, OfVersion::V12) => Some(match t {
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
            }),
            (Profile::Hk, OfVersion::V8) => Some(match t {
                T::ControlDict => hk!("controlDict"),
                T::RegionProperties => hk!("regionProperties"),
                T::FluidThermo => hk!("fluid/thermophysicalProperties"),
                T::FluidMomentum => hk!("fluid/momentumTransport"),
                T::FluidG => hk!("fluid/g"),
                T::FluidSchemes => hk!("fluid/fvSchemes"),
                T::FluidSolution => hk!("fluid/fvSolution"),
                T::FluidConstraints => return None,
                T::FluidOptions => hk!("fluid/fvOptions"),
                T::SolidSchemes => hk!("solid/fvSchemes"),
                T::SolidSolution => hk!("solid/fvSolution"),
                T::SolidG => hk!("solid/g"),
                T::SolidTransport => hk!("solid/transportProperties"),
                T::VegetationSchemes => hk!("vegetation/fvSchemes"),
                T::VegetationSolution => hk!("vegetation/fvSolution"),
                T::Radiation => hk!("radiation/radiationProperties"),
                T::SolarLoad => hk!("radiation/solarLoadProperties"),
                T::Grass => hk!("fluid/grassProperties"),
                T::Vegetation => hk!("fluid/vegetationProperties"),
            }),
            (Profile::Hk, OfVersion::V12) => None,
        }
    }
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
    fn every_template_of_a_profile_s_versions_reads() {
        for p in Profile::ALL {
            for &v in p.versions() {
                for t in ALL {
                    let Some(text) = p.template(v, t) else {
                        // Only what the version keeps elsewhere is missing.
                        let elsewhere = match v {
                            OfVersion::V8 => t == Template::FluidConstraints,
                            OfVersion::V12 => matches!(t, Template::RegionProperties | Template::FluidOptions),
                        };
                        assert!(elsewhere, "{} {v:?}: no {t:?}", p.name());
                        continue;
                    };
                    let doc = umcf_foam_dict::Document::parse(text)
                        .unwrap_or_else(|e| panic!("{} {v:?} {t:?}: {e}", p.name()));
                    assert!(doc.root.problems().is_empty(), "{} {v:?} {t:?}: {:?}", p.name(), doc.root.problems());
                }
            }
        }
    }

    #[test]
    fn a_profile_has_no_files_for_a_version_it_doesn_t_have() {
        assert!(Profile::Hk.template(OfVersion::V12, Template::ControlDict).is_none());
        assert_eq!(Profile::parse("hk"), Some(Profile::Hk));
        assert_eq!(Profile::parse("HK"), None);
    }
}
