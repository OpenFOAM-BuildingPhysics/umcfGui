use super::*;
use crate::script::parse;

const PREPARE: &str = "#!/bin/sh\n\
cd ${0%/*} || exit 1\n\
. $WM_PROJECT_DIR/bin/tools/RunFunctions\n\
rm -rf log\n\
surfaceFeatures\n\
cp system/blockMeshDict.air system/blockMeshDict\n\
blockMesh\n\
createPatch -overwrite\n\
cp system/snappyHexMeshDict.air system/snappyHexMeshDict\n\
snappyHexMesh -overwrite\n\
rm -rf constant/air/polyMesh\n\
mv constant/polyMesh constant/air/.\n\
rm -rf constant/polyMesh\n\
changeDictionary -region air > log.changeDictionary.air 2>&1\n\
topoSet -region air\n\
rm -f 0/air/LAD*\n\
cp 0/air.bckp/LAD 0/air/.\n\
setFields -region air > log.setFields.air 2>&1\n\
cp system/blockMeshDict.veg system/blockMeshDict\n\
blockMesh\n\
createPatch -overwrite\n\
cp system/snappyHexMeshDict.veg system/snappyHexMeshDict\n\
snappyHexMesh -overwrite\n\
rm -rf constant/vegetation/polyMesh\n\
mv constant/polyMesh constant/vegetation/.\n\
changeDictionary -region vegetation\n\
for i in buildings street roofs\n\
do\n\
    blockMesh -region $i > log.blockMesh.$i 2>&1\n\
    createPatch -region $i -overwrite\n\
    changeDictionary -region $i > log.changeDictionary.$i 2>&1\n\
    setSet -region $i -noVTK -batch system/$i/setset.batch > log.setSet.$i 2>&1\n\
done\n\
cp system/vegetation/viewFactorsDict system/viewFactorsDict\n\
faceAgglomerate -region vegetation > log.faceAgglomerate.vegetation 2>&1\n\
rm -f system/blockMeshDict system/snappyHexMeshDict\n\
mkdir -p log\n\
mv log.* log/.\n";

const RUN: &str = "#!/bin/sh\n\
cd ${0%/*} || exit 1\n\
rm -rf processor*\n\
decomposePar -allRegions\n\
nProcs=$(foamDictionary system/decomposeParDict -entry numberOfSubdomains -value)\n\
for i in $(seq 0 $((nProcs-1))); do\n\
    cp constant/air/g processor${i}/constant/air/\n\
done\n\
mpirun -np 20 faceAgglomerate -region vegetation -parallel\n\
mpirun -np 20 calcLAI -region air -parallel\n\
mpirun -np 20 viewFactorsGen -region vegetation -parallel\n\
mpirun -np 20 solarRayTracingGen -region vegetation -parallel\n\
mpirun -np 20 urbanMicroclimateFoam -parallel > log.UMC 2>&1\n\
reconstructPar -allRegions\n";

fn regions() -> Vec<String> {
    ["air", "vegetation", "street", "roofs", "buildings"].map(String::from).to_vec()
}

fn imported() -> Workflow {
    let mut wf = Workflow::import(
        &regions(),
        Some("urbanMicroclimateFoam"),
        &[parse("Allprepare", PREPARE), parse("Allrun", RUN)],
    );
    wf.constant_files = vec![("air".into(), vec!["g".into(), "vegetationProperties".into()])];
    wf
}

#[test]
fn the_case_scripts_fit_the_pattern() {
    let wf = imported();
    assert!(wf.features);
    let names: Vec<&str> = wf.meshes.iter().map(|m| m.region.as_str()).collect();
    assert_eq!(names, ["air", "vegetation", "buildings", "street", "roofs"]);
    assert_eq!(
        wf.meshes[0],
        RegionMesh {
            region: "air".into(),
            build: MeshBuild::Root(vec![
                RootStep::BlockMesh("system/blockMeshDict.air".into()),
                RootStep::CreatePatch,
                RootStep::SnappyHexMesh("system/snappyHexMeshDict.air".into()),
            ]),
            steps: vec![
                RegionStep::ChangeDictionary,
                RegionStep::Sets { setset_batch: None },
                RegionStep::SetFields { restore: vec![("0/air.bckp/LAD".into(), "LAD".into())] },
            ],
        }
    );
    assert_eq!(wf.meshes[2].build, MeshBuild::Block);
    assert_eq!(
        wf.meshes[2].steps,
        [
            RegionStep::CreatePatch,
            RegionStep::ChangeDictionary,
            RegionStep::Sets { setset_batch: Some("system/buildings/setset.batch".into()) },
        ]
    );
    assert_eq!(
        wf.radiation,
        Some(Radiation {
            region: "vegetation".into(),
            lai_region: Some("air".into()),
            view_factors: true,
            solar_ray_tracing: true,
        })
    );
    assert_eq!(wf.solver.as_deref(), Some("urbanMicroclimateFoam"));
    assert!(wf.reconstruct);
    assert_eq!(wf.np.len(), 5);
    // Fits, but not needed: the serial faceAgglomerate (Allrun runs it in parallel).
    let unmatched: Vec<(&str, bool)> = wf.unmatched.iter().map(|u| (u.script.as_str(), u.redundant)).collect();
    assert_eq!(unmatched, [("Allprepare", true)], "{:?}", wf.unmatched);
}

#[test]
fn generated_scripts_import_to_the_same_workflow() {
    let wf = imported();
    for version in [OfVersion::V8, OfVersion::V12] {
        let scripts: Vec<Script> = wf.generate(version).iter().map(|(n, t)| parse(n, t)).collect();
        let again = Workflow::import(&regions(), Some("urbanMicroclimateFoam"), &scripts);
        // OpenFOAM-12 has no setSet: its scripts use topoSet.
        let mut meshes = wf.meshes.clone();
        if version == OfVersion::V12 {
            for m in &mut meshes {
                for st in &mut m.steps {
                    if let RegionStep::Sets { setset_batch } = st {
                        *setset_batch = None;
                    }
                }
            }
        }
        assert_eq!(again.meshes, meshes, "{version:?}");
        assert_eq!(again.radiation, wf.radiation);
        assert_eq!((again.features, &again.solver, again.reconstruct), (wf.features, &wf.solver, wf.reconstruct));
        assert!(again.unmatched.is_empty(), "{version:?}: {:?}", again.unmatched);
        // Everything resolves, except the copies into each processor directory (a glob loop)
        // and, in OpenFOAM-8, the parallel runs' `-np "$nProcs"`.
        for st in scripts.iter().flat_map(|s| &s.steps).filter(|st| st.unresolved) {
            let ok = match &st.kind {
                StepKind::Copy { to, .. } => to.starts_with("processor*/"),
                StepKind::Run(r) => version == OfVersion::V8 && r.parallel,
                _ => false,
            };
            assert!(ok, "{version:?}: {st:?}");
        }
    }
}

#[test]
fn version_choices() {
    let wf = imported();
    let text = |v| wf.generate(v).into_iter().map(|(n, t)| format!("== {n}\n{t}")).collect::<String>();
    let of8 = text(OfVersion::V8);
    let of12 = text(OfVersion::V12);
    // OpenFOAM-12 has no setSet: the solid regions use topoSet (system/<r>/topoSetDict).
    assert!(of8.contains("setSet -region buildings -batch system/buildings/setset.batch"));
    assert!(of12.contains("topoSet -region buildings") && !of12.contains("setSet"));
    // faceAgglomerate: OpenFOAM-12 is pointed at the region's dictionary.
    assert!(of12.contains("faceAgglomerate -region vegetation -dict system/vegetation/viewFactorsDict"));
    assert!(of8.contains(
        "mpirun -np \"$nProcs\" faceAgglomerate -region vegetation -parallel > log.faceAgglomerate.vegetation 2>&1\n"
    ));
    // No -allRegions (OpenFOAM-12 needs regionSolvers for it), no hard-coded process count.
    assert!(!of12.contains("-allRegions") && !of12.contains("-np"));
    assert!(of12.contains("runParallel -o urbanMicroclimateFoam"));
    // The LAD is restored before setFields, without leaving LAD next to LAD.gz.
    assert!(of12.contains("rm -f 0/air/LAD 0/air/LAD.gz\ncp 0/air.bckp/LAD 0/air/LAD\n"));
    // The region dictionaries go into each processor directory, for solver builds before June 2026.
    assert!(of12.contains(
        "for p in processor*\ndo\n    cp constant/air/g constant/air/vegetationProperties \"$p\"/constant/air/\ndone\n"
    ));
}

#[test]
fn dict_options_order_and_leftovers() {
    let prepare = "#!/bin/sh\n\
blockMesh -dict system/blockMeshDict.air\n\
snappyHexMesh -overwrite -dict system/snappyHexMeshDict.air\n\
createPatch -overwrite\n\
mv constant/polyMesh constant/air/\n\
topoSet -region air -dict system/air/topoSetDict.trees\n\
cp 0.orig/air/U 0/air/\n";
    let wf = Workflow::import(&regions(), None, &[parse("Allprepare", prepare)]);
    // -dict names the dictionary; the order is kept: createPatch after snappyHexMesh.
    assert_eq!(
        wf.meshes[0].build,
        MeshBuild::Root(vec![
            RootStep::BlockMesh("system/blockMeshDict.air".into()),
            RootStep::SnappyHexMesh("system/snappyHexMeshDict.air".into()),
            RootStep::CreatePatch,
        ])
    );
    let text = wf.generate(OfVersion::V12).remove(0).1;
    let snappy = text.find("snappyHexMesh -overwrite -dict system/snappyHexMeshDict.air").unwrap();
    assert!(text.find("createPatch -overwrite").unwrap() > snappy, "{text}");
    // -dict, not a copy over (and removal of) the case's own system/blockMeshDict.
    assert!(text.contains("blockMesh -dict system/blockMeshDict.air\n") && !text.contains("cp "), "{text}");
    // A region step with its own dictionary, and a field restore no setFields uses: reported.
    let what: Vec<&str> = wf.unmatched.iter().map(|u| u.what.as_str()).collect();
    assert!(what.iter().any(|w| w.starts_with("topoSet -region air -dict")), "{what:?}");
    assert!(what.iter().any(|w| w.starts_with("cp 0.orig/air/U 0/air/U")), "{what:?}");
    // Allclean removes only the meshes Allprepare builds.
    let clean = wf.generate(OfVersion::V12).remove(2).1;
    assert!(clean.contains("for r in air\n"), "{clean}");
}

#[test]
fn names_are_quoted_and_openfoam_8_counts_processes_per_region() {
    let mut wf = imported();
    wf.meshes[0].region = "my air".into();
    let text: String = wf.generate(OfVersion::V8).into_iter().map(|(_, t)| t).collect();
    assert!(text.contains("runApplication -o -s 'my air' blockMesh"), "{text}");
    assert!(text.contains("nProcs=$(foamDictionary -entry numberOfSubdomains -value system/air/decomposeParDict)"));
    // runParallel would read system/decomposeParDict, which OpenFOAM-8 cases may not have.
    assert!(text.contains("mpirun -np \"$nProcs\" urbanMicroclimateFoam -parallel > log.urbanMicroclimateFoam 2>&1"));
    assert!(!text.contains("runParallel"));
}

#[test]
fn a_directory_copy_is_not_a_field_restore() {
    let prepare =
        "#!/bin/sh\ncp -r 0.orig/air 0/air\ncp 0/air.bckp/LAD 0/air/\nblockMesh -region air\nsetFields -region air\n";
    let wf = Workflow::import(&regions(), None, &[parse("Allprepare", prepare)]);
    assert_eq!(wf.meshes[0].steps, [RegionStep::SetFields { restore: vec![("0/air.bckp/LAD".into(), "LAD".into())] }]);
    let u = wf.unmatched.iter().find(|u| u.what.contains("0.orig")).unwrap();
    assert_eq!(u.line, 2);
}
