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
    let lines = "if [ -L constant/vegetation ] || [ -L constant/region1 ] || [ -L constant/region2 ] || [ -L constant/trees ] || [ -L 0/vegetation ]\nthen\n    echo 'region vegetation: a directory the split writes to is a symbolic link (constant/vegetation, constant/region1, constant/region2, constant/trees, 0/vegetation): nothing is removed through it' >&2\n    exit 1\nfi\n\
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

/// The part is taken from where the split wrote it: a time directory without `-overwrite`,
/// constant/ with it; a copy from elsewhere (an older mesh) doesn't take it.
#[test]
fn a_split_is_taken_from_where_it_wrote_its_parts() {
    let overwrite = split_prepare("system/air/topoSetDict").replace("-cellZones\n", "-cellZones -overwrite\n");
    let wf = Workflow::import(&split_regions(), None, &[parse("Allprepare", &overwrite)]);
    assert!(wf.meshes.iter().all(|m| m.region != "vegetation"), "{:?}", wf.meshes);
    assert!(wf.unmatched.iter().any(|u| u.what.starts_with("cp 3600/region1/polyMesh")), "{:?}", wf.unmatched);
    let timed = split_prepare("system/air/topoSetDict").replace(
        "cp -r 3600/region1/polyMesh constant/vegetation/\n",
        "mv constant/region1/polyMesh constant/vegetation/polyMesh\n",
    );
    let wf = Workflow::import(&split_regions(), None, &[parse("Allprepare", &timed)]);
    assert!(wf.meshes.iter().all(|m| m.region != "vegetation"), "{:?}", wf.meshes);
}

/// The air meshed by blockMesh and snappyHexMesh, the vegetation and a second fluid from the
/// air's background mesh (the windAroundBuildings chain).
fn background_workflow() -> Workflow {
    let mut wf = Workflow { regions: vec!["air".into(), "vegetation".into(), "street".into()], ..Workflow::default() };
    let snappy = |r: &str| RootStep::SnappyHexMesh(format!("system/snappyHexMeshDict.{r}"));
    wf.meshes = vec![
        RegionMesh {
            region: "air".into(),
            build: MeshBuild::Root(vec![RootStep::BlockMesh("system/air/blockMeshDict".into()), snappy("air")]),
            steps: vec![RegionStep::ChangeDictionary],
        },
        RegionMesh {
            region: "vegetation".into(),
            build: MeshBuild::Root(vec![RootStep::Background("air".into()), snappy("vegetation")]),
            steps: vec![RegionStep::CreatePatch],
        },
        RegionMesh {
            region: "street".into(),
            build: MeshBuild::Root(vec![RootStep::Background("air".into()), RootStep::CreatePatch, snappy("street")]),
            steps: Vec::new(),
        },
    ];
    wf
}

#[test]
fn a_background_mesh_is_kept_for_the_regions_meshed_from_it() {
    let wf = background_workflow();
    for v in [OfVersion::V8, OfVersion::V12] {
        let scripts = wf.generate(v).unwrap();
        let prep = &scripts[0].1;
        // Kept right after the air's blockMesh, before its snappyHexMesh changes it, marked as
        // Allprepare's; one there without the mark stops the script, nothing removed.
        let air = "runApplication -o -s air blockMesh -dict system/air/blockMeshDict\n\
if [ -e constant/polyMesh.background.air ] && [ ! -e constant/polyMesh.background.air/.umcfgui-kept ]\nthen\n    \
echo 'region air: constant/polyMesh.background.air is there but has no .umcfgui-kept, so Allprepare did not keep it: nothing is removed; move it away' >&2\n    \
exit 1\nfi\n\
rm -rf constant/polyMesh.background.air\ncp -r constant/polyMesh constant/polyMesh.background.air\n\
touch constant/polyMesh.background.air/.umcfgui-kept\n\
runApplication -o -s air snappyHexMesh -overwrite -dict system/snappyHexMeshDict.air\n\
rm -rf constant/air/polyMesh\nmv constant/polyMesh constant/air/polyMesh\n";
        assert!(prep.contains(air), "{prep}");
        // Each region from it starts from the copy; the last removes it.
        let veg = "rm -rf constant/polyMesh\ncp -r constant/polyMesh.background.air constant/polyMesh\n\
rm -f constant/polyMesh/.umcfgui-kept\n\
runApplication -o -s vegetation snappyHexMesh -overwrite -dict system/snappyHexMeshDict.vegetation\n\
rm -rf constant/vegetation/polyMesh\nmv constant/polyMesh constant/vegetation/polyMesh\n\
runApplication -o -s vegetation createPatch -region vegetation -overwrite\n";
        assert!(prep.contains(veg), "{prep}");
        let street = "rm -rf constant/polyMesh\ncp -r constant/polyMesh.background.air constant/polyMesh\n\
rm -f constant/polyMesh/.umcfgui-kept\n\
runApplication -o -s street createPatch -overwrite\n\
runApplication -o -s street snappyHexMesh -overwrite -dict system/snappyHexMeshDict.street\n\
rm -rf constant/street/polyMesh\nmv constant/polyMesh constant/street/polyMesh\n\
rm -rf constant/polyMesh.background.air\n";
        assert!(prep.contains(street), "{prep}");
        assert_eq!(prep.matches("rm -rf constant/polyMesh.background.air\n").count(), 2, "{prep}");
        // Allclean removes it too, should Allprepare have stopped before: only when marked.
        assert!(
            scripts[2].1.contains("if [ -e constant/polyMesh.background.air/.umcfgui-kept ]\nthen\n    rm -rf constant/polyMesh.background.air\nfi\n"),
            "{}",
            scripts[2].1
        );
        // The scripts read back into the same meshes.
        let again = Workflow::import(&wf.regions, None, &[parse("Allprepare", prep)]);
        assert_eq!(again.unmatched, [], "{prep}");
        assert_eq!(again.meshes, wf.meshes);
    }
}

#[test]
fn a_background_the_script_cannot_keep_is_refused() {
    let refused = |wf: &Workflow, region: &str, why: &str| match wf.generate(OfVersion::V12) {
        Err(ScriptError::Background { region: r, why: w }) => {
            assert_eq!(r, region);
            assert!(w.contains(why), "{w}");
        }
        other => panic!("{other:?}"),
    };
    // Built after the region meshed from it.
    let mut wf = background_workflow();
    wf.meshes.swap(0, 1);
    refused(&wf, "vegetation", "isn't built before it");
    // A region of no mesh, its own, one from a background itself, one blocks alone.
    let mut wf = background_workflow();
    wf.meshes[1].build = MeshBuild::Root(vec![RootStep::Background("roof".into())]);
    refused(&wf, "vegetation", "isn't built before it");
    let mut wf = background_workflow();
    wf.meshes[1].build = MeshBuild::Root(vec![RootStep::Background("vegetation".into())]);
    refused(&wf, "vegetation", "its own background");
    let mut wf = background_workflow();
    wf.meshes[2].build = MeshBuild::Root(vec![RootStep::Background("vegetation".into())]);
    refused(&wf, "street", "starts from a background too");
    let mut wf = background_workflow();
    wf.meshes[0].build = MeshBuild::Block;
    refused(&wf, "vegetation", "starting with blockMesh");
    // A region named as the directory the background is kept in: the scripts would remove it.
    let mut wf = background_workflow();
    wf.case_regions = vec!["polyMesh.background.air".into()];
    refused(&wf, "vegetation", "a region named polyMesh.background.air");
    // A background that isn't the first step, or a blockMesh after one (a Workflow made
    // directly; the description refuses both).
    let mut wf = background_workflow();
    wf.meshes[1].build =
        MeshBuild::Root(vec![RootStep::BlockMesh("system/blockMeshDict".into()), RootStep::Background("air".into())]);
    refused(&wf, "vegetation", "background is its mesh's first step");
    let mut wf = background_workflow();
    wf.meshes[1].build =
        MeshBuild::Root(vec![RootStep::Background("air".into()), RootStep::BlockMesh("system/blockMeshDict".into())]);
    refused(&wf, "vegetation", "without a blockMesh after it");
    // A name that isn't safe in a script is refused before.
    let mut wf = background_workflow();
    wf.meshes[1].build = MeshBuild::Root(vec![RootStep::Background("-rf".into())]);
    assert_eq!(wf.generate(OfVersion::V12), Err(ScriptError::Unsafe("-rf".into())));
}

#[test]
fn shell_patterns_match_as_the_shell_does() {
    for (p, n, m) in [
        ("polyMesh.*", "polyMesh.bckp", true),
        ("*.bckp", "polyMesh.bckp", true),
        ("polyMesh.?ckp", "polyMesh.bckp", true),
        ("polyMesh.[ab]ckp", "polyMesh.bckp", true),
        ("polyMesh.[!ab]ckp", "polyMesh.bckp", false),
        ("polyMesh.[^a]ckp", "polyMesh.bckp", true),
        ("polyMesh.[a-c]ckp", "polyMesh.bckp", true),
        ("polyMesh[0-9]*", "polyMesh.bckp", false),
        ("polyMesh[.]bckp", "polyMesh.bckp", true),
        ("polyMesh[]bckp", "polyMesh[]bckp", true),
        ("[", "[", true),
        ("**.bckp", "polyMesh.bckp", true),
        ("polyMesh.[[:alpha:]]*", "polyMesh.bckp", true),
        ("polyMesh.[[:alpha:]0-9]ckp", "polyMesh.bckp", true),
        ("*", "", true),
        ("polyMesh", "polyMesh.bckp", false),
        ("*.x", "polyMesh.bckp", false),
        ("a*b*c", "aXbYc", true),
        ("a*b*c", "aXbY", false),
    ] {
        assert_eq!(glob(p, n), m, "{p} {n}");
    }
    // Bounded: a pattern past the limit is taken as matching; many stars stay cheap.
    assert!(glob(&"a".repeat(GLOB_MAX + 1), "b"));
    assert!(!glob(&format!("{}x", "*".repeat(200)), &"a".repeat(200)));
}

#[test]
fn the_tutorials_kept_background_is_read_when_moved_into_place() {
    // A kept copy of any polyMesh.* name, put back for the next region.
    let prepare = "#!/bin/sh\nblockMesh -dict system/air/blockMeshDict\ncp -r constant/polyMesh constant/polyMesh.bckp\n\
snappyHexMesh -overwrite -dict system/snappyHexMeshDict.air\nmv constant/polyMesh constant/air/polyMesh\n\
cp -r constant/polyMesh.bckp constant/polyMesh\nsnappyHexMesh -overwrite -dict system/snappyHexMeshDict.vegetation\n\
mv constant/polyMesh constant/vegetation/polyMesh\nrm -rf constant/polyMesh.bckp\n";
    let wf = Workflow::import(&["air".into(), "vegetation".into()], None, &[parse("Allprepare", prepare)]);
    assert_eq!(wf.unmatched, [], "{:?}", wf.unmatched);
    assert_eq!(
        wf.meshes[1].build,
        MeshBuild::Root(vec![
            RootStep::Background("air".into()),
            RootStep::SnappyHexMesh("system/snappyHexMeshDict.vegetation".into())
        ])
    );
    // A copy taken after snappyHexMesh is not the background: not read as one.
    let late = prepare.replace(
        "cp -r constant/polyMesh constant/polyMesh.bckp\nsnappyHexMesh -overwrite -dict system/snappyHexMeshDict.air\n",
        "snappyHexMesh -overwrite -dict system/snappyHexMeshDict.air\ncp -r constant/polyMesh constant/polyMesh.bckp\n",
    );
    let wf = Workflow::import(&["air".into(), "vegetation".into()], None, &[parse("Allprepare", &late)]);
    assert!(!wf.unmatched.is_empty());
    assert!(wf.meshes.iter().all(|m| m.build.background().is_none()), "{:?}", wf.meshes);
    // Removed before it is used, or after: the keep and the removal listed, a restore not read.
    let lines = |wf: &Workflow| wf.unmatched.iter().map(|u| u.line).collect::<Vec<_>>();
    for (from, to, listed) in [
        (
            "cp -r constant/polyMesh constant/polyMesh.bckp\n",
            "cp -r constant/polyMesh constant/polyMesh.bckp\nrm -rf constant/polyMesh.bckp\n",
            vec![3, 4, 7, 8, 9, 10],
        ),
        (
            "mv constant/polyMesh constant/air/polyMesh\n",
            "mv constant/polyMesh constant/air/polyMesh\nrm -rf constant/polyMesh.bckp\n",
            vec![3, 6, 7, 8, 9, 10],
        ),
    ] {
        let script = prepare.replacen(from, to, 1);
        let wf = Workflow::import(&["air".into(), "vegetation".into()], None, &[parse("Allprepare", &script)]);
        assert_eq!(lines(&wf), listed, "{script}\n{:?}", wf.unmatched);
        assert!(wf.meshes.iter().all(|m| m.build.background().is_none()), "{:?}", wf.meshes);
    }
    // Kept and never used, or cleared with no keep after it: listed.
    let unused = "#!/bin/sh\nblockMesh -dict system/air/blockMeshDict\nrm -rf constant/polyMesh.x\ncp -r constant/polyMesh constant/polyMesh.bckp\n\
mv constant/polyMesh constant/air/polyMesh\n";
    let wf = Workflow::import(&["air".into()], None, &[parse("Allprepare", unused)]);
    assert_eq!(lines(&wf), [3, 4], "{:?}", wf.unmatched);
    let cleared = "#!/bin/sh\nblockMesh -dict system/air/blockMeshDict\nrm -rf constant/polyMesh.x\nmv constant/polyMesh constant/air/polyMesh\n";
    // A second keep into a directory still kept would nest the copy in it: listed, the first
    // one stays the background.
    let twice = "#!/bin/sh\nblockMesh -dict system/air/blockMeshDict\ncp -r constant/polyMesh constant/polyMesh.bckp\n\
mv constant/polyMesh constant/air/polyMesh\nblockMesh -dict system/street/blockMeshDict\ncp -r constant/polyMesh constant/polyMesh.bckp\n\
mv constant/polyMesh constant/street/polyMesh\ncp -r constant/polyMesh.bckp constant/polyMesh\n\
snappyHexMesh -overwrite -dict system/snappyHexMeshDict.vegetation\nmv constant/polyMesh constant/vegetation/polyMesh\n";
    let three: Vec<String> = ["air", "street", "vegetation"].map(String::from).to_vec();
    let wf = Workflow::import(&three, None, &[parse("Allprepare", twice)]);
    assert_eq!(lines(&wf), [6], "{:?}", wf.unmatched);
    assert_eq!(wf.meshes[2].build.background(), Some("air"));
    // A restore a blockMesh then replaces did nothing: listed, and so is the keep, unused.
    let replaced = "#!/bin/sh\nblockMesh -dict system/air/blockMeshDict\ncp -r constant/polyMesh constant/polyMesh.bckp\n\
mv constant/polyMesh constant/air/polyMesh\ncp -r constant/polyMesh.bckp constant/polyMesh\nblockMesh -dict system/vegetation/blockMeshDict\n\
mv constant/polyMesh constant/vegetation/polyMesh\n";
    let wf = Workflow::import(&["air".into(), "vegetation".into()], None, &[parse("Allprepare", replaced)]);
    assert_eq!(lines(&wf), [3, 5], "{:?}", wf.unmatched);
    assert!(wf.meshes.iter().all(|m| m.build.background().is_none()), "{:?}", wf.meshes);
    let wf = Workflow::import(&["air".into()], None, &[parse("Allprepare", cleared)]);
    assert_eq!(lines(&wf), [3], "{:?}", wf.unmatched);
    // Its clean-up of the same directory goes with a keep never used.
    let unused_cleaned = "#!/bin/sh\nblockMesh -dict system/air/blockMeshDict\nrm -rf constant/polyMesh.bckp\ncp -r constant/polyMesh constant/polyMesh.bckp\n\
mv constant/polyMesh constant/air/polyMesh\n";
    let wf = Workflow::import(&["air".into()], None, &[parse("Allprepare", unused_cleaned)]);
    assert_eq!(lines(&wf), [3, 4], "{:?}", wf.unmatched);
    // A blockMesh after the keep replaces the mesh it was kept from: the keep isn't of the mesh
    // built, listed, and no region is read as meshed from it.
    let rebuilt = prepare.replace(
        "cp -r constant/polyMesh constant/polyMesh.bckp\n",
        "cp -r constant/polyMesh constant/polyMesh.bckp\nblockMesh -dict system/other/blockMeshDict\n",
    );
    let wf = Workflow::import(&["air".into(), "vegetation".into()], None, &[parse("Allprepare", &rebuilt)]);
    assert!(lines(&wf).contains(&3), "{:?}", wf.unmatched);
    assert!(wf.meshes.iter().all(|m| m.build.background().is_none()), "{:?}", wf.meshes);
    // Restored, removed, then replaced by a blockMesh: no region was meshed from the copy, so
    // the keep, the restore and the removal are all listed.
    let gone = "#!/bin/sh\nblockMesh -dict system/air/blockMeshDict\ncp -r constant/polyMesh constant/polyMesh.bckp\n\
mv constant/polyMesh constant/air/polyMesh\ncp -r constant/polyMesh.bckp constant/polyMesh\nrm -rf constant/polyMesh.bckp\n\
blockMesh -dict system/vegetation/blockMeshDict\nmv constant/polyMesh constant/vegetation/polyMesh\n";
    let wf = Workflow::import(&["air".into(), "vegetation".into()], None, &[parse("Allprepare", gone)]);
    let mut listed = lines(&wf);
    listed.sort_unstable();
    assert_eq!(listed, [3, 5, 6], "{:?}", wf.unmatched);
    // Removed after the restore, before the region's mesh is moved: still its background.
    let early = "#!/bin/sh\nblockMesh -dict system/air/blockMeshDict\ncp -r constant/polyMesh constant/polyMesh.bckp\n\
mv constant/polyMesh constant/air/polyMesh\ncp -r constant/polyMesh.bckp constant/polyMesh\nrm -rf constant/polyMesh.bckp\n\
snappyHexMesh -overwrite -dict system/snappyHexMeshDict.vegetation\nmv constant/polyMesh constant/vegetation/polyMesh\n";
    let wf = Workflow::import(&["air".into(), "vegetation".into()], None, &[parse("Allprepare", early)]);
    assert_eq!(wf.unmatched, [], "{:?}", wf.unmatched);
    assert_eq!(wf.meshes[1].build.background(), Some("air"));
    // Scripts whose restore doesn't give a region the kept mesh: listed, not read as one.
    let head = "#!/bin/sh\nblockMesh\ncp -r constant/polyMesh constant/polyMesh.bg\nmv constant/polyMesh constant/air/polyMesh\n\
cp -r constant/polyMesh.bg constant/polyMesh\n";
    let three: Vec<String> = ["air", "vegetation"].map(String::from).to_vec();
    for (tail, listed) in [
        // Its mesh never moved to a region (the copy removed meanwhile, or not).
        ("rm -rf constant/polyMesh.bg\n", vec![3, 5, 6]),
        ("", vec![3, 5]),
        // Moved where no region is.
        ("rm -rf constant/polyMesh.bg\nmv constant/polyMesh constant/a/b/polyMesh\n", vec![3, 5, 6, 7]),
        // The region it was kept from meshed from it.
        (
            "snappyHexMesh -overwrite\nrm -rf constant/air/polyMesh\nmv constant/polyMesh constant/air/polyMesh\n",
            vec![3, 5, 8],
        ),
        // The restored mesh removed before it is moved.
        ("rm -rf constant/polyMesh\nmv constant/polyMesh constant/vegetation/polyMesh\n", vec![3, 5, 6, 7]),
    ] {
        let script = format!("{head}{tail}");
        let wf = Workflow::import(&three, None, &[parse("Allprepare", &script)]);
        let mut got = lines(&wf);
        got.sort_unstable();
        assert_eq!(got, listed, "{script}\n{:?}", wf.unmatched);
        assert!(wf.meshes.iter().all(|m| m.build.background().is_none()), "{script}\n{:?}", wf.meshes);
    }
    // The region the background was kept from built again: listed, the earlier reading kept.
    let again = format!(
        "{head}snappyHexMesh -overwrite\nmv constant/polyMesh constant/vegetation/polyMesh\nblockMesh\nmv constant/polyMesh constant/air/polyMesh\n"
    );
    let wf = Workflow::import(&three, None, &[parse("Allprepare", &again)]);
    assert_eq!(lines(&wf), [9], "{:?}", wf.unmatched);
    assert_eq!(wf.meshes.iter().map(|m| m.region.as_str()).collect::<Vec<_>>(), ["air", "vegetation"]);
    assert_eq!(wf.meshes[1].build.background(), Some("air"));
    // A keep whose mesh is removed (or moved nowhere) isn't handed to the next region moved.
    for gone in ["rm -rf constant/polyMesh\n", "mv constant/polyMesh constant/a/b/polyMesh\n"] {
        let script = format!(
            "#!/bin/sh\nblockMesh\ncp -r constant/polyMesh constant/polyMesh.a\nmv constant/polyMesh constant/air/polyMesh\n\
blockMesh -dict system/street/blockMeshDict\ncp -r constant/polyMesh constant/polyMesh.b\n{gone}\
cp -r constant/polyMesh.a constant/polyMesh\nsnappyHexMesh -overwrite\nmv constant/polyMesh constant/vegetation/polyMesh\n\
cp -r constant/polyMesh.b constant/polyMesh\nsnappyHexMesh -overwrite\nmv constant/polyMesh constant/trees/polyMesh\n"
        );
        let regions: Vec<String> = ["air", "vegetation", "trees"].map(String::from).to_vec();
        let wf = Workflow::import(&regions, None, &[parse("Allprepare", &script)]);
        let got = lines(&wf);
        assert!(got.contains(&6) && got.contains(&7) && got.contains(&11), "{script}\n{:?}", wf.unmatched);
        let bg =
            |r: &str| wf.meshes.iter().find(|m| m.region == r).and_then(|m| m.build.background().map(str::to_string));
        assert_eq!(bg("vegetation").as_deref(), Some("air"), "{script}");
        assert_eq!(bg("trees"), None, "{script}");
    }
    // A kept copy taken away with other paths, by a pattern or by a move: a later restore of it
    // isn't read.
    for away in [
        "rm -rf constant/polyMesh.bckp constant/vegetation/polyMesh\n",
        "rm -rf constant/polyMesh.*\n",
        "mv constant/polyMesh.bckp constant/old\n",
        "rm -rf constant/*.bckp\n",
        "rm -rf constant\n",
    ] {
        let script = format!(
            "#!/bin/sh\nblockMesh\ncp -r constant/polyMesh constant/polyMesh.bckp\nmv constant/polyMesh constant/air/polyMesh\n{away}\
cp -r constant/polyMesh.bckp constant/polyMesh\nsnappyHexMesh -overwrite\nmv constant/polyMesh constant/vegetation/polyMesh\n"
        );
        let wf = Workflow::import(&["air".into(), "vegetation".into()], None, &[parse("Allprepare", &script)]);
        assert!(lines(&wf).contains(&3), "{script}\n{:?}", wf.unmatched);
        assert!(wf.meshes.iter().all(|m| m.build.background().is_none()), "{script}\n{:?}", wf.meshes);
    }
    // A pattern inside constant/polyMesh/ leaves the kept copy: still read, the line listed.
    let inside = "#!/bin/sh\nblockMesh\ncp -r constant/polyMesh constant/polyMesh.bckp\nmv constant/polyMesh constant/air/polyMesh\n\
rm -rf constant/polyMesh/*\ncp -r constant/polyMesh.bckp constant/polyMesh\nsnappyHexMesh -overwrite\n\
mv constant/polyMesh constant/vegetation/polyMesh\n";
    let wf = Workflow::import(&["air".into(), "vegetation".into()], None, &[parse("Allprepare", inside)]);
    assert_eq!(lines(&wf), [5], "{:?}", wf.unmatched);
    assert_eq!(wf.meshes[1].build.background(), Some("air"));
    // Taken away by a pattern or a move after its restore, before the mesh is moved: still the
    // region's background, only that step listed.
    for away in ["rm -rf constant/polyMesh.*\n", "mv constant/polyMesh.bckp constant/old\n"] {
        let script = format!(
            "#!/bin/sh\nblockMesh\ncp -r constant/polyMesh constant/polyMesh.bckp\nmv constant/polyMesh constant/air/polyMesh\n\
cp -r constant/polyMesh.bckp constant/polyMesh\n{away}snappyHexMesh -overwrite\nmv constant/polyMesh constant/vegetation/polyMesh\n"
        );
        let wf = Workflow::import(&["air".into(), "vegetation".into()], None, &[parse("Allprepare", &script)]);
        assert_eq!(lines(&wf), [6], "{script}\n{:?}", wf.unmatched);
        assert_eq!(wf.meshes[1].build.background(), Some("air"), "{script}");
    }
    // A copy into a directory made for it puts the mesh inside it: not a keep.
    let made_first = "#!/bin/sh\nblockMesh\nmkdir -p constant/polyMesh.bckp\ncp -r constant/polyMesh constant/polyMesh.bckp\n\
mv constant/polyMesh constant/air/polyMesh\nrm -rf constant/polyMesh\ncp -r constant/polyMesh.bckp constant/polyMesh\n\
snappyHexMesh -overwrite\nmv constant/polyMesh constant/vegetation/polyMesh\n";
    let wf = Workflow::import(&["air".into(), "vegetation".into()], None, &[parse("Allprepare", made_first)]);
    assert!(lines(&wf).contains(&3), "{:?}", wf.unmatched);
    assert!(wf.meshes.iter().all(|m| m.build.background().is_none()), "{:?}", wf.meshes);
    // A restore into a constant/polyMesh that is there puts the copy inside it: not read.
    let inside_existing = "#!/bin/sh\nblockMesh\ncp -r constant/polyMesh constant/polyMesh.bckp\nmv constant/polyMesh constant/air/polyMesh\n\
cp -r constant/air/polyMesh constant/polyMesh\ncp -r constant/polyMesh.bckp constant/polyMesh\nsnappyHexMesh -overwrite\n\
mv constant/polyMesh constant/vegetation/polyMesh\n";
    let wf = Workflow::import(&["air".into(), "vegetation".into()], None, &[parse("Allprepare", inside_existing)]);
    assert!(lines(&wf).contains(&6), "{:?}", wf.unmatched);
    assert!(wf.meshes.iter().all(|m| m.build.background().is_none()), "{:?}", wf.meshes);
    // The region the background was kept from built again by blockMesh -region: listed, the
    // earlier reading (and order) kept.
    let region_again = format!(
        "{head}snappyHexMesh -overwrite\nmv constant/polyMesh constant/vegetation/polyMesh\nblockMesh -region air\n"
    );
    let wf = Workflow::import(&three, None, &[parse("Allprepare", &region_again)]);
    assert_eq!(lines(&wf), [8], "{:?}", wf.unmatched);
    assert_eq!(wf.meshes.iter().map(|m| m.region.as_str()).collect::<Vec<_>>(), ["air", "vegetation"]);
    assert_eq!(wf.meshes[1].build.background(), Some("air"));
    // A blockMesh of another region (-region) writes its own mesh: the keep and the restore
    // stand.
    for at in ["cp -r constant/polyMesh constant/polyMesh.bckp\n", "cp -r constant/polyMesh.bckp constant/polyMesh\n"] {
        let other = prepare.replace(at, &format!("{at}blockMesh -region wall\n"));
        let regions: Vec<String> = ["air", "vegetation", "wall"].map(String::from).to_vec();
        let wf = Workflow::import(&regions, None, &[parse("Allprepare", &other)]);
        assert_eq!(wf.unmatched, [], "{other}");
        let veg = wf.meshes.iter().find(|m| m.region == "vegetation").unwrap();
        assert_eq!(veg.build.background(), Some("air"), "{other}");
    }
}
