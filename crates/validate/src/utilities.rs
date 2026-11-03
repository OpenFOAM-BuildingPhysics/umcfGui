//! Utilities that one OpenFOAM version has and the other doesn't.
//!
//! Generated from the OpenFOAM-8 and OpenFOAM-12 sources: the executables their
//! `applications/**/Make/files` build, plus the scripts in `bin/`. A `bin/` script that only
//! says a utility has been superseded (OpenFOAM-12 `surfaceFeatureExtract`) counts as missing;
//! one that says so and then runs the replacement (`"$@"`) is listed apart. Names in no list
//! are in both versions, or are not OpenFOAM's (`calcLAI`, `sed`).

/// Not in OpenFOAM-12 (no executable, or a script that only says what replaced it).
pub(crate) const NOT_IN_OF12: &[&str] = &[
    // Scripts saying the solver was renamed (rhoParticleFoam, particleFoam).
    "icoUncoupledKinematicParcelFoam",
    "uncoupledKinematicParcelFoam",
    "DPMFoam",
    "MPPICFoam",
    "SRFPimpleFoam",
    "SRFSimpleFoam",
    "XiEngineFoam",
    "adjointShapeOptimizationFoam",
    "attachMesh",
    "buoyantPimpleFoam",
    "buoyantSimpleFoam",
    "cavitatingFoam",
    "cellSizeAndAlignmentGrid",
    "coalChemistryFoam",
    "coldEngineFoam",
    "compressibleInterFilmFoam",
    "dnsFoam",
    "engineFoam",
    "fireFoam",
    "foamCleanPolyMesh",
    "foamyHexMesh",
    "foamyHexMeshBackgroundMesh",
    "foamyHexMeshSurfaceSimplify",
    "foamyQuadMesh",
    "interMixingFoam",
    "interPhaseChangeFoam",
    "mergeOrSplitBaffles",
    "modifyMesh",
    "moveEngineMesh",
    "nonNewtonianIcoFoam",
    "particleFoam",
    "postChannel",
    "potentialFreeSurfaceFoam",
    "reactingMultiphaseEulerFoam",
    "reactingParcelFoam",
    "reactingTwoPhaseEulerFoam",
    "reconstructParMesh",
    "rhoParticleFoam",
    "rhoReactingBuoyantFoam",
    "rhoReactingFoam",
    "rotateMesh",
    "scalarTransportFoam",
    "setSet",
    "simpleReactingParcelFoam",
    "splitMesh",
    "sprayFoam",
    "surfaceFeatureExtract",
    "surfaceMeshConvertTesting",
    "thermoFoam",
    "twoLiquidMixingFoam",
    "twoPhaseEulerFoam",
    "wallFunctionTable",
];

/// Not in OpenFOAM-8.
pub(crate) const NOT_IN_OF8: &[&str] = &[
    // Scripts saying the solver was renamed (rhoParticleFoam, particleFoam).
    "icoUncoupledKinematicParcelFoam",
    "uncoupledKinematicParcelFoam",
    "adjointShapeOptimisationFoam",
    "buoyantFoam",
    "buoyantReactingFoam",
    "createEngineZones",
    "createNonConformalCouples",
    "denseParticleFoam",
    "foamCleanCase",
    "foamFind",
    "foamMultiRun",
    "foamPostProcess",
    "foamRun",
    "foamToC",
    "foamUnits",
    "mergeBaffles",
    "reactingTwoPhaseEulerFoam",
    "reorderPatches",
    "setAtmBoundaryLayer",
    "snappyHexMeshConfig",
    "splitBaffles",
    "surfaceFeatureExtract",
    "twoPhaseEulerFoam",
    "zeroDimensionalMesh",
];

/// OpenFOAM-12 scripts that say the utility is superseded, then run its replacement
/// (`simpleFoam` runs `foamRun -solver incompressibleFluid`, `postProcess` runs
/// `foamPostProcess`).
pub(crate) const FORWARDED_IN_OF12: &[&str] = &[
    "XiFoam",
    "buoyantFoam",
    "buoyantReactingFoam",
    "chtMultiRegionFoam",
    "compressibleInterFoam",
    "compressibleMultiphaseInterFoam",
    "denseParticleFoam",
    "driftFluxFoam",
    "interFoam",
    "moveDynamicMesh",
    "moveMesh",
    "multiphaseEulerFoam",
    "multiphaseInterFoam",
    "pimpleFoam",
    "pisoFoam",
    "postProcess",
    "reactingFoam",
    "rhoCentralFoam",
    "rhoPimpleFoam",
    "rhoSimpleFoam",
    "simpleFoam",
    "solidDisplacementFoam",
    "solidEquilibriumDisplacementFoam",
];

/// The same in OpenFOAM-8: none.
pub(crate) const FORWARDED_IN_OF8: &[&str] = &[];
