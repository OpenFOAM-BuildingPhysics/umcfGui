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
        let scripts: Vec<Script> = wf.generate(version).unwrap().iter().map(|(n, t)| parse(n, t)).collect();
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
    let text = |v| wf.generate(v).unwrap().into_iter().map(|(n, t)| format!("== {n}\n{t}")).collect::<String>();
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
    let text = wf.generate(OfVersion::V12).unwrap().remove(0).1;
    let snappy = text.find("snappyHexMesh -overwrite -dict system/snappyHexMeshDict.air").unwrap();
    assert!(text.find("createPatch -overwrite").unwrap() > snappy, "{text}");
    // -dict, not a copy over (and removal of) the case's own system/blockMeshDict.
    assert!(text.contains("blockMesh -dict system/blockMeshDict.air\n") && !text.contains("cp "), "{text}");
    // A region step with its own dictionary, and a field restore no setFields uses: reported.
    let what: Vec<&str> = wf.unmatched.iter().map(|u| u.what.as_str()).collect();
    assert!(what.iter().any(|w| w.starts_with("topoSet -region air -dict")), "{what:?}");
    assert!(what.iter().any(|w| w.starts_with("cp 0.orig/air/U 0/air/U")), "{what:?}");
    // Allclean removes only the meshes Allprepare builds.
    let clean = wf.generate(OfVersion::V12).unwrap().remove(2).1;
    assert!(clean.contains("for r in air\n"), "{clean}");
}

#[test]
fn names_are_quoted_and_openfoam_8_counts_processes_per_region() {
    let mut wf = imported();
    wf.meshes[0].region = "my air".into();
    let text: String = wf.generate(OfVersion::V8).unwrap().into_iter().map(|(_, t)| t).collect();
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

#[test]
fn names_with_control_characters_are_refused() {
    // A region name read from `blockMesh -region 'x<newline>rm -rf ~'` must not become a command.
    let prepare = "#!/bin/sh\nblockMesh -region 'x\nrm -rf ~'\n";
    let wf = Workflow::import(&regions(), None, &[parse("Allprepare", prepare)]);
    assert_eq!(wf.meshes[0].region, "x\nrm -rf ~");
    assert_eq!(wf.generate(OfVersion::V12), Err(ScriptError::Unsafe("x\nrm -rf ~".into())));
}

#[test]
fn names_starting_with_a_dash_are_refused() {
    let wf = Workflow::import(&regions(), None, &[parse("Allprepare", "#!/bin/sh\nblockMesh -region -rf\n")]);
    if !wf.meshes.is_empty() {
        assert!(wf.generate(OfVersion::V12).is_err());
    }
    let mut wf = imported();
    wf.regions.push("-rf".into());
    assert_eq!(wf.generate(OfVersion::V8), Err(ScriptError::Unsafe("-rf".into())));
}

#[test]
fn names_and_paths_must_stay_in_the_case() {
    for region in ["../../x", "a/b", "..", "."] {
        let mut wf = imported();
        wf.meshes[2].region = region.into();
        assert_eq!(wf.generate(OfVersion::V12), Err(ScriptError::Unsafe(region.into())), "{region}");
    }
    for dict in ["/etc/passwd", "system/../../x"] {
        let mut wf = imported();
        wf.meshes[0].build = MeshBuild::Root(vec![RootStep::BlockMesh(dict.into())]);
        assert_eq!(wf.generate(OfVersion::V12), Err(ScriptError::Unsafe(dict.into())), "{dict}");
    }
    // Paths inside the case are fine.
    let mut wf = imported();
    wf.meshes[0].build = MeshBuild::Root(vec![RootStep::BlockMesh("system/air/blockMeshDict".into())]);
    assert!(wf.generate(OfVersion::V12).is_ok());
}

/// The street canyon's vegetation, as its scripts split it from the air (OpenFOAM-12; the
/// OpenFOAM-8 branch reads `system/../air/topoSetDict`).
fn split_prepare(dict: &str) -> String {
    format!(
        "#!/bin/sh\n\
blockMesh -region air\n\
createPatch -region air -overwrite\n\
cp -r constant/air/polyMesh/ constant/vegetation/polyMesh\n\
topoSet -region vegetation -dict {dict}\n\
splitMeshRegions -region vegetation -cellZones\n\
rm -r constant/vegetation/polyMesh\n\
cp -r 3600/region1/polyMesh constant/vegetation/\n\
rm -r 3600\n\
rm -rf system/vegetation/trees system/vegetation/region*\n\
rm 0/vegetation/cellToRegion*\n\
createPatch -region vegetation -overwrite\n\
changeDictionary -region vegetation\n"
    )
}

fn split_regions() -> Vec<String> {
    ["air", "vegetation"].map(String::from).to_vec()
}

#[test]
fn a_region_split_by_a_cell_zone_is_read_from_the_scripts() {
    for dict in ["system/air/topoSetDict", "system/../air/topoSetDict"] {
        let wf = Workflow::import(&split_regions(), None, &[parse("Allprepare", &split_prepare(dict))]);
        assert_eq!(wf.unmatched, [], "{dict}");
        let veg = wf.meshes.iter().find(|m| m.region == "vegetation").unwrap();
        assert_eq!(
            veg.build,
            MeshBuild::Split(Split {
                of: "air".into(),
                zone: Some("trees".into()),
                dict: "system/air/topoSetDict".into()
            }),
            "{dict}"
        );
        assert_eq!(veg.steps, [RegionStep::CreatePatch, RegionStep::ChangeDictionary]);
    }
    // Without a topoSet on the copy, the default dictionary of the region split is its own.
    let prepare = split_prepare("x").replace("topoSet -region vegetation -dict x\n", "topoSet -region vegetation\n");
    let wf = Workflow::import(&split_regions(), None, &[parse("Allprepare", &prepare)]);
    let MeshBuild::Split(sp) = &wf.meshes[1].build else { panic!("{:?}", wf.meshes) };
    assert_eq!(sp.dict, "system/vegetation/topoSetDict");
    // Another way to split (a point inside, the largest part) doesn't fit: the copy, the
    // topoSet and the split are all named.
    let prepare = split_prepare("system/air/topoSetDict").replace("-cellZones", "-cellZones -largestOnly");
    let wf = Workflow::import(&split_regions(), None, &[parse("Allprepare", &prepare)]);
    assert_eq!(wf.meshes.len(), 1);
    let lines: Vec<usize> = wf.unmatched.iter().map(|u| u.line).collect();
    assert!(lines.starts_with(&[4, 5, 6]), "{:?}", wf.unmatched);
    // A copy no split follows is named too.
    let wf = Workflow::import(
        &split_regions(),
        None,
        &[parse("Allprepare", "#!/bin/sh\nblockMesh -region air\ncp -r constant/air/polyMesh constant/vegetation/\n")],
    );
    assert_eq!(wf.unmatched.len(), 1, "{:?}", wf.unmatched);
}

#[test]
fn a_split_region_is_written_for_each_version() {
    let wf = Workflow::import(&split_regions(), None, &[parse("Allprepare", &split_prepare("system/air/topoSetDict"))]);
    let of12 = wf.generate(OfVersion::V12).unwrap();
    let prep = &of12[0].1;
    let lines = "if [ -L constant/vegetation ] || [ -L constant/region1 ] || [ -L constant/region2 ] || [ -L constant/trees ]\nthen\n    echo 'region vegetation: a directory the split writes to is a symbolic link (constant/vegetation, constant/region1, constant/region2, constant/trees): nothing is removed through it' >&2\n    exit 1\nfi\n\
rm -rf constant/vegetation/polyMesh\n\
mkdir -p constant/vegetation\n\
cp -r constant/air/polyMesh constant/vegetation/polyMesh\n\
rm -rf constant/vegetation/polyMesh/cellZones constant/vegetation/polyMesh/cellZones.gz constant/vegetation/polyMesh/sets\n\
rm -rf constant/region1/polyMesh constant/region2/polyMesh\n\
runApplication -o -s vegetation topoSet -region vegetation -dict system/air/topoSetDict\n\
runApplication -o -s vegetation splitMeshRegions -region vegetation -cellZones -overwrite -noFields\n\
if [ ! -d constant/region1/polyMesh ] || [ -d constant/region2/polyMesh ]\n\
then\n    echo 'region vegetation: the split of air by the cell zone trees left no single part outside it: the zone has no cells, or cuts the rest in pieces (log.splitMeshRegions.vegetation)' >&2\n    exit 1\nfi\n\
rm -rf constant/vegetation/polyMesh\n\
mv constant/region1/polyMesh constant/vegetation/polyMesh\n\
rm -rf constant/trees/polyMesh\n\
rmdir constant/region1 constant/trees || true\n\
rm -f constant/vegetation/cellToRegion constant/vegetation/cellToRegion.gz 0/vegetation/cellToRegion 0/vegetation/cellToRegion.gz\n\
runApplication -o -s vegetation createPatch -region vegetation -overwrite\n";
    assert!(prep.contains(lines), "{prep}");
    // Nothing in 0/ but the maps, and no whole directory of the zone's name, is removed.
    assert!(!prep.contains("0/trees") && !prep.contains("rm -rf constant/trees\n"), "{prep}");
    // The air is built first, its mesh then copied.
    assert!(prep.find("blockMesh -region air").unwrap() < prep.find("cp -r constant/air").unwrap());
    let of8 = wf.generate(OfVersion::V8).unwrap();
    let prep8 = &of8[0].1;
    assert!(prep8.contains("topoSet -region vegetation -dict system/../air/topoSetDict\n"), "{prep8}");
    assert!(
        prep8.contains("[ -L system/vegetation ] || [ -L system/vegetation/region1 ] || [ -L system/vegetation/region2 ] || [ -L system/vegetation/trees ]"),
        "{prep8}"
    );
    assert!(
        prep8.contains("rm -f system/vegetation/region1/fvSchemes system/vegetation/region1/fvSchemes.gz system/vegetation/region1/fvSolution system/vegetation/region1/fvSolution.gz system/vegetation/trees/fvSchemes ")
            && prep8.contains("rmdir constant/region1 constant/trees system/vegetation/region1 system/vegetation/trees || true\n"),
        "{prep8}"
    );
    // The scripts of either version read back into the same workflow.
    for p in [prep, prep8] {
        let again = Workflow::import(&split_regions(), None, &[parse("Allprepare", p)]);
        assert_eq!(again.unmatched, [], "{p}");
        assert_eq!(again.meshes, wf.meshes);
    }
    assert!(of12[2].1.contains("for r in air vegetation\ndo\n    rm -rf \"constant/$r/polyMesh\""), "{}", of12[2].1);
}

#[test]
fn only_what_a_split_leaves_is_taken_for_its_clean_up() {
    let read = |extra: &str| {
        let prepare = split_prepare("system/air/topoSetDict")
            .replace("rm 0/vegetation/cellToRegion*\n", &format!("rm 0/vegetation/cellToRegion*\n{extra}"));
        Workflow::import(&split_regions(), None, &[parse("Allprepare", &prepare)])
    };
    // Dictionaries of the region, a whole system directory, another region's maps: reported.
    for extra in [
        "rm system/vegetation/changeDictionaryDict\n",
        "rm -rf system/vegetation/*\n",
        "rm -rf constant/geometry\n",
        "rm -rf 0/air\n",
        "mkdir -p constant/extra\n",
    ] {
        let wf = read(extra);
        assert_eq!(wf.unmatched.len(), 1, "{extra}: {:?}", wf.unmatched);
    }
    // After a split with -overwrite, a time directory isn't its.
    let prepare = split_prepare("system/air/topoSetDict").replace("-cellZones\n", "-cellZones -overwrite\n").replace(
        "cp -r 3600/region1/polyMesh constant/vegetation/\n",
        "mv constant/region1/polyMesh constant/vegetation/polyMesh\n",
    );
    let wf = Workflow::import(&split_regions(), None, &[parse("Allprepare", &prepare)]);
    assert_eq!(wf.unmatched.iter().map(|u| u.what.as_str()).collect::<Vec<_>>(), ["rm 3600"]);
    // A dictionary's name in the clean-up is not taken for the zone.
    let prepare = split_prepare("system/air/topoSetDict")
        .replace("rm -rf system/vegetation/trees system/vegetation/region*\n", "rm -rf system/vegetation/region*\n");
    let wf = Workflow::import(&split_regions(), None, &[parse("Allprepare", &prepare)]);
    let MeshBuild::Split(sp) = &wf.meshes[1].build else { panic!() };
    assert_eq!(sp.zone, None);
    // The part outside the zone put in is not a copy to split.
    let wf = read("cp -r constant/region1/polyMesh constant/other/\n");
    assert_eq!(wf.meshes.len(), 2);
    // topoSet with other options on the copy doesn't fit.
    let prepare = split_prepare("system/air/topoSetDict")
        .replace("-dict system/air/topoSetDict", "-dict system/air/topoSetDict -noSync");
    let wf = Workflow::import(&split_regions(), None, &[parse("Allprepare", &prepare)]);
    assert_eq!(wf.meshes.len(), 1, "{:?}", wf.meshes);
}

#[test]
fn a_split_whose_clean_up_would_remove_a_region_is_refused() {
    let base =
        Workflow::import(&split_regions(), None, &[parse("Allprepare", &split_prepare("system/air/topoSetDict"))]);
    let with = |f: &dyn Fn(&mut Workflow, &mut Split)| {
        let mut wf = base.clone();
        let MeshBuild::Split(mut sp) = wf.meshes[1].build.clone() else { panic!() };
        f(&mut wf, &mut sp);
        wf.meshes[1].build = MeshBuild::Split(sp);
        wf.generate(OfVersion::V12)
    };
    let refused = |r: Result<Vec<(&'static str, String)>, ScriptError>, says: &str| match r {
        Err(ScriptError::Split { region, why }) => {
            assert_eq!(region, "vegetation");
            assert!(why.contains(says), "{why}");
        }
        other => panic!("{other:?}"),
    };
    // The zone named as a region: `rm -rf constant/air`.
    refused(with(&|_, sp| sp.zone = Some("air".into())), "the zone is named air");
    // A region of the case the workflow leaves out, named as the zone.
    refused(
        with(&|wf, sp| {
            wf.case_regions.push("trees2".into());
            sp.zone = Some("trees2".into());
        }),
        "the zone is named trees2",
    );
    // A region named as the part outside the zone.
    refused(with(&|wf, _| wf.regions.push("region1".into())), "the part outside the zone is named region1");
    refused(with(&|_, sp| sp.zone = None), "isn't known");
    refused(with(&|_, sp| sp.zone = Some("region1".into())), "region1");
    refused(with(&|_, sp| sp.zone = Some("region2".into())), "the zone is named region2");
    // A region only starting so is fine, and its mesh isn't touched by the split.
    let mut wf = base.clone();
    wf.case_regions.push("region1Solid".into());
    let prep = wf.generate(OfVersion::V12).unwrap().remove(0).1;
    assert!(!prep.contains("region1Solid") && !prep.contains("region["), "{prep}");
    refused(with(&|_, sp| sp.of = "vegetation".into()), "its own mesh");
    // The region copied must be built first.
    refused(with(&|wf, _| wf.meshes.swap(0, 1)), "isn't built before it");
    refused(with(&|_, sp| sp.dict = "topoSetDict".into()), "not in a directory");
    // A dictionary that makes other zones too, or whose zones can't be told.
    for (zones, says) in [
        (DictZones::Made(vec!["trees".into(), "street".into()]), "other cell zones too (street)"),
        (DictZones::Unclear, "can't be told"),
        (DictZones::Missing, "isn't there"),
        (DictZones::Made(Vec::new()), "doesn't make the cell zone trees"),
        (DictZones::Made(vec!["bushes".into()]), "doesn't make the cell zone trees"),
    ] {
        refused(with(&|wf, _| wf.split_zones.push(("vegetation".into(), zones.clone()))), says);
    }
    assert!(with(&|wf, _| wf.split_zones.push(("vegetation".into(), DictZones::Made(vec!["trees".into()])))).is_ok());
    // A region named as splitMeshRegions names the parts outside the zone.
    refused(with(&|wf, _| wf.case_regions.push("region2".into())), "region named region2");
    // Unsafe words are refused as everywhere.
    assert_eq!(with(&|_, sp| sp.zone = Some("-rf".into())), Err(ScriptError::Unsafe("-rf".into())));
}

/// Names from the case are quoted in every line, the stop's message too: nothing in them runs.
#[test]
fn a_splits_names_are_quoted_in_its_lines() {
    let mut wf =
        Workflow::import(&split_regions(), None, &[parse("Allprepare", &split_prepare("system/air/topoSetDict"))]);
    let MeshBuild::Split(mut sp) = wf.meshes[1].build.clone() else { panic!() };
    sp.zone = Some("$(touch x)".into());
    wf.meshes[1].build = MeshBuild::Split(sp);
    let prep = wf.generate(OfVersion::V12).unwrap().remove(0).1;
    for line in prep.lines().filter(|l| l.contains("touch x")) {
        let outside: String = line.split('\'').step_by(2).collect();
        assert!(!outside.contains("$("), "{line}");
    }
    assert!(prep.contains("echo 'region vegetation: the split of air by the cell zone $(touch x) left"), "{prep}");
}

/// A region's name in the split's clean-up is not taken for its zone.
#[test]
fn a_regions_name_is_not_taken_for_the_zone() {
    let prepare = split_prepare("system/air/topoSetDict")
        .replace("rm -rf system/vegetation/trees system/vegetation/region*\n", "rm -rf constant/air/polyMesh\n");
    let wf = Workflow::import(&split_regions(), None, &[parse("Allprepare", &prepare)]);
    let MeshBuild::Split(sp) = &wf.meshes[1].build else { panic!() };
    assert_eq!(sp.zone, None);
}

/// The zones a topoSetDict makes are told only from plain actions.
#[test]
fn the_zones_a_dictionary_makes_are_told_from_plain_actions() {
    let told = |text: &str| zones_told(&umcf_foam_dict::Document::parse(text).unwrap());
    let action = |name: &str, kind: &str| format!("{{ name {name}; type {kind}; action new; source setToCellZone; }}");
    let plain = format!(
        "actions ( {} {} {} );",
        action("trees", "cellSet"),
        action("trees", "cellZoneSet"),
        action("bushes", "cellZoneSet")
    );
    assert_eq!(told(&plain), Some(vec!["trees".to_string(), "bushes".to_string()]));
    assert_eq!(told("other 1;"), Some(Vec::new()));
    // Actions in order: a remove takes the zone away.
    let removed = format!(
        "actions ( {} {} {{ name other; type cellZoneSet; action remove; }} );",
        action("trees", "cellZoneSet"),
        action("other", "cellZoneSet"),
    );
    assert_eq!(told(&removed), Some(vec!["trees".to_string()]));
    assert_eq!(zones_made(&umcf_foam_dict::Document::parse(&removed).unwrap()), ["trees"]);
    for unclear in [
        format!("#include \"more\"\n{plain}"),
        format!("actions ( {} $more );", action("trees", "cellZoneSet")),
        "actions ( { #include \"a\" } );".to_string(),
        "actions ( { name $zone; type cellZoneSet; } );".to_string(),
        "actions $list;".to_string(),
        format!("{plain}\nactions ( );"),
        "actions ( { name a; name trees; type cellZoneSet; } );".to_string(),
        "actions ( { name trees; type cellZoneSet; } );".to_string(),
        // add on a zone not made yet: topoSet would stop.
        "actions ( { name trees; type cellZoneSet; action add; source setToCellZone; } );".to_string(),
    ] {
        assert_eq!(told(&unclear), None, "{unclear}");
    }
}

/// A split whose part outside the zone is never put in place builds no mesh: its lines are
/// listed.
#[test]
fn a_split_never_put_in_place_builds_no_mesh() {
    let prepare =
        split_prepare("system/air/topoSetDict").replace("cp -r 3600/region1/polyMesh constant/vegetation/\n", "");
    let wf = Workflow::import(&split_regions(), None, &[parse("Allprepare", &prepare)]);
    assert!(wf.meshes.iter().all(|m| m.region != "vegetation"), "{:?}", wf.meshes);
    let said: Vec<&str> = wf.unmatched.iter().map(|u| u.what.as_str()).collect();
    assert!(said.iter().any(|w| w.starts_with("splitMeshRegions") && w.contains("never put in")), "{said:?}");
    assert!(said.iter().any(|w| w.starts_with("cp constant/air/polyMesh")), "{said:?}");
    // The region's steps after it found no mesh: listed.
    assert!(said.iter().any(|w| w.starts_with("changeDictionary -region vegetation")), "{said:?}");
    // A region built before keeps that build, with its steps.
    let earlier = prepare.replace(
        "cp -r constant/air/polyMesh/ constant/vegetation/polyMesh\n",
        "blockMesh -region vegetation\ncp -r constant/air/polyMesh/ constant/vegetation/polyMesh\n",
    );
    let wf = Workflow::import(&split_regions(), None, &[parse("Allprepare", &earlier)]);
    let veg = wf.meshes.iter().find(|m| m.region == "vegetation").unwrap();
    assert_eq!(veg.build, MeshBuild::Block);
    assert_eq!(veg.steps, [RegionStep::CreatePatch, RegionStep::ChangeDictionary]);
}

/// A zone the clean-up names before the part is put in is the split's.
#[test]
fn a_zone_named_before_the_part_is_put_in_is_kept() {
    let prepare = split_prepare("system/air/topoSetDict")
        .replace("rm -rf system/vegetation/trees system/vegetation/region*\n", "")
        .replace(
            "cp -r 3600/region1/polyMesh constant/vegetation/\n",
            "rm -rf system/vegetation/trees system/vegetation/region*\ncp -r 3600/region1/polyMesh constant/vegetation/\n",
        );
    let wf = Workflow::import(&split_regions(), None, &[parse("Allprepare", &prepare)]);
    assert_eq!(wf.unmatched, []);
    let MeshBuild::Split(sp) = &wf.meshes[1].build else { panic!("{:?}", wf.meshes) };
    assert_eq!(sp.zone.as_deref(), Some("trees"));
}
