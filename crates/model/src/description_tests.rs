use umcf_core::MemFs;

use super::*;

fn header(class: &str, object: &str) -> String {
    format!(
        "FoamFile\n{{\n    version     2.0;\n    format      ascii;\n    class       {class};\n    object      {object};\n}}\n"
    )
}

/// A boundary file with one face per patch: `(name, type, extra entries)`.
fn boundary(patches: &[(&str, &str, &str)]) -> String {
    let mut s = format!("{}{}\n(\n", header("polyBoundaryMesh", "boundary"), patches.len());
    for (k, (name, kind, extra)) in patches.iter().enumerate() {
        s.push_str(&format!(
            "    {name}\n    {{\n        type {kind};\n        nFaces 1;\n        startFace {k};\n{extra}    }}\n"
        ));
    }
    s.push_str(")\n");
    s
}

fn velocity(bcs: &[(&str, &str)]) -> String {
    let mut s = format!(
        "{}dimensions [0 1 -1 0 0 0 0];\ninternalField uniform (0 0 0);\nboundaryField\n{{\n",
        header("volVectorField", "U")
    );
    for (p, t) in bcs {
        s.push_str(&format!("    {p} {{ type {t}; }}\n"));
    }
    s.push_str("}\n");
    s
}

const CONTROL: &str = "application urbanMicroclimateFoam;\nstartFrom startTime;\nstartTime 0;\nendTime 7200;\ndeltaT 3600;\n\
regions\n{\n    fluid (air);\n    solid (street);\n    vegetation (vegetation);\n}\n";

const PREPARE: &str = "#!/bin/sh\n\
blockMesh -region air\n\
createPatch -region air -overwrite\n\
changeDictionary -region air\n\
topoSet -region air\n\
cp 0/air.bckp/LAD 0/air/.\n\
setFields -region air\n\
cp system/blockMeshDict.veg system/blockMeshDict\n\
blockMesh\n\
mv constant/polyMesh constant/vegetation/.\n\
changeDictionary -region vegetation\n\
blockMesh -region street\n\
changeDictionary -region street\n\
topoSet -region street\n\
python3 helper.py\n";

const RUN: &str = "#!/bin/sh\n\
. $WM_PROJECT_DIR/bin/tools/RunFunctions\n\
runApplication faceAgglomerate -region vegetation -dict system/vegetation/viewFactorsDict\n\
runApplication calcLAI -region air\n\
runApplication viewFactorsGen -region vegetation\n\
runApplication solarRayTracingGen -region vegetation\n\
runApplication urbanMicroclimateFoam\n";

/// A street canyon: air with an inlet, an outlet, slip sides, a top, the ground and the street
/// coupled to the solid; the solid street; the vegetation region with the air's twins.
fn canyon() -> MemFs {
    let coupled = |r: &str, p: &str| format!("        neighbourRegion {r};\n        neighbourPatch {p};\n");
    MemFs::new()
        .with("system/controlDict", format!("{}{CONTROL}", header("dictionary", "controlDict")))
        .with(
            "constant/air/polyMesh/boundary",
            boundary(&[
                ("west", "patch", ""),
                ("east", "patch", ""),
                ("north", "patch", ""),
                ("top", "patch", ""),
                ("ground", "wall", ""),
                ("air_to_street", "mappedWall", &coupled("street", "street_to_air")),
                ("frontAndBack", "empty", ""),
                ("odd", "patch", ""),
            ]),
        )
        .with(
            "0/air/U",
            velocity(&[
                ("west", "atmBoundaryLayerInletVelocity"),
                ("east", "inletOutlet"),
                ("north", "slip"),
                ("top", "slip"),
                ("ground", "noSlip"),
                ("air_to_street", "noSlip"),
                ("odd", "fixedFluxPressure"),
            ]),
        )
        .with(
            "constant/street/polyMesh/boundary",
            boundary(&[
                ("street_to_air", "mappedWall", &coupled("air", "air_to_street")),
                ("bottom", "patch", ""),
                ("sides", "wall", ""),
            ]),
        )
        .with(
            "constant/vegetation/polyMesh/boundary",
            boundary(&[
                ("west", "mapped", "        sampleRegion air;\n        samplePatch west;\n"),
                ("top", "mapped", "        sampleRegion air;\n        samplePatch top;\n"),
                ("air_to_street", "mappedWall", &coupled("air", "air_to_street")),
                ("air_to_vegetation", "mappedInternal", "        neighbourRegion air;\n"),
                // OpenFOAM-8's canopy: a mapped wall sampling the air's cells.
                ("canopy8", "mappedWall", "        sampleMode nearestCell;\n        sampleRegion air;\n"),
            ]),
        )
        .with("Allprepare", PREPARE)
        .with("Allrun", RUN)
}

/// A patch's name, role and partner.
type Told = (String, Option<&'static str>, Option<(String, String)>);

fn roles(d: &Description, region: &str) -> Vec<Told> {
    d.region(region)
        .unwrap()
        .patches
        .iter()
        .map(|p| (p.name.clone(), p.role.map(Role::name), p.partner.clone()))
        .collect()
}

fn pair(r: &str, p: &str) -> Option<(String, String)> {
    Some((r.to_string(), p.to_string()))
}

#[test]
fn import_tells_kinds_meshes_steps_and_roles() {
    let c = Case::open(&canyon()).unwrap();
    let (d, notes) = Description::import(&c);
    assert_eq!(d.version, Some(OfVersion::V12));
    // In the order the meshes are built.
    let names: Vec<(&str, RegionKind)> = d.regions.iter().map(|r| (r.name.as_str(), r.kind)).collect();
    assert_eq!(
        names,
        [("air", RegionKind::Fluid), ("vegetation", RegionKind::Vegetation), ("street", RegionKind::Solid)]
    );
    let air = d.region("air").unwrap();
    assert_eq!(air.mesh, Some(MeshBuild::Block));
    assert_eq!(
        air.steps,
        [
            RegionStep::CreatePatch,
            RegionStep::ChangeDictionary,
            RegionStep::Sets { setset_batch: None },
            RegionStep::SetFields { restore: vec![("0/air.bckp/LAD".into(), "LAD".into())] },
        ]
    );
    assert_eq!(
        d.region("vegetation").unwrap().mesh,
        Some(MeshBuild::Root(vec![RootStep::BlockMesh("system/blockMeshDict.veg".into())]))
    );
    assert_eq!(
        roles(&d, "air"),
        [
            ("west".into(), Some("inlet"), None),
            ("east".into(), Some("outlet"), None),
            ("north".into(), Some("side"), None),
            ("top".into(), Some("top"), None),
            ("ground".into(), Some("ground"), None),
            ("air_to_street".into(), Some("coupled"), pair("street", "street_to_air")),
            // `empty` is left out; a patch nothing tells is kept without a role.
            ("odd".into(), None, None),
        ]
    );
    assert_eq!(
        roles(&d, "street"),
        [
            ("street_to_air".into(), Some("coupled"), pair("air", "air_to_street")),
            ("bottom".into(), Some("bottom"), None),
            ("sides".into(), Some("side"), None),
        ]
    );
    // The vegetation twins take their air patch's role: the open sides and top are sky.
    assert_eq!(
        roles(&d, "vegetation"),
        [
            ("west".into(), Some("sky"), pair("air", "west")),
            ("top".into(), Some("sky"), pair("air", "top")),
            ("air_to_street".into(), Some("coupled"), pair("air", "air_to_street")),
            // Mapped to the air's cells: no partner patch.
            ("air_to_vegetation".into(), Some("canopy"), None),
            ("canopy8".into(), Some("canopy"), None),
        ]
    );
    let rad = d.radiation.as_ref().unwrap();
    assert_eq!((rad.region.as_str(), rad.lai_region.as_deref()), ("vegetation", Some("air")));
    assert_eq!(d.solver.as_deref(), Some("urbanMicroclimateFoam"));
    assert!(notes.iter().any(|n| n.contains("patch odd: no role")), "{notes:#?}");
    assert!(notes.iter().any(|n| n.contains("Allprepare line 15") && n.contains("helper.py")), "{notes:#?}");
}

#[test]
fn written_and_read_back_it_is_the_same() {
    let c = Case::open(&canyon()).unwrap();
    let (d, _) = Description::import(&c);
    let text = d.write();
    let doc = Document::parse(&text).unwrap();
    assert_eq!(doc.to_string(), text, "a dictionary that round-trips");
    let (back, problems) = Description::read(&doc);
    assert_eq!(problems, []);
    assert_eq!(back, d);
    // Every kind of step and a quoted name too.
    let mut d = d;
    if let Some(r) = d.regions.get_mut(2) {
        r.steps.push(RegionStep::Sets { setset_batch: Some("system/street/setset.batch".into()) });
        r.mesh = Some(MeshBuild::Root(vec![
            RootStep::BlockMesh("system/blockMeshDict".into()),
            RootStep::CreatePatch,
            RootStep::SnappyHexMesh("system/snappyHexMeshDict.street".into()),
        ]));
        r.patches.push(PatchDesc { name: "with space".into(), role: Some(Role::Interior), partner: None });
    }
    d.features = true;
    d.solver = None;
    d.version = Some(OfVersion::V8);
    let (back, problems) = Description::read(&Document::parse(&d.write()).unwrap());
    assert_eq!(problems, []);
    assert_eq!(back, d);
}

#[test]
fn odd_names_come_back_as_they_were() {
    for odd in ["q\"x", "b\\s", "end\\", "\\\"", "with space", "a;b", "{x}", "$v", "#i", "1st", "-x"] {
        let d = Description {
            version: Some(OfVersion::V12),
            regions: vec![RegionDesc {
                name: odd.into(),
                kind: RegionKind::Solid,
                mesh: Some(MeshBuild::Block),
                steps: vec![RegionStep::SetFields { restore: vec![(format!("0/{odd}.bak"), odd.into())] }],
                patches: vec![PatchDesc {
                    name: odd.into(),
                    role: Some(Role::Coupled),
                    partner: Some((odd.into(), odd.into())),
                }],
            }],
            features: false,
            radiation: Some(Radiation {
                region: odd.into(),
                lai_region: Some(odd.into()),
                view_factors: true,
                solar_ray_tracing: false,
            }),
            solver: Some(odd.into()),
            reconstruct: true,
        };
        let (back, problems) = Description::read(&Document::parse(&d.write()).unwrap());
        assert_eq!((back, problems), (d, vec![]), "{odd}");
    }
    let text = format!(
        "{}regions {{ \"\" {{ kind solid; patches {{ \"\" {{ role side; }} }} }} ok {{ kind solid; patches {{ \"\" {{ }} }} }} }}\nsolver a;\nsolver b;\n",
        header("dictionary", "umcfWorkflow")
    );
    let (d, problems) = Description::read(&Document::parse(&text).unwrap());
    let at: Vec<&str> = problems.iter().map(|p| p.at.as_str()).collect();
    assert_eq!(at, ["regions/\"\"", "regions/ok/patches/\"\"", "solver"], "{problems:#?}");
    assert_eq!(d.solver.as_deref(), Some("a"));
}

#[test]
fn the_scripts_from_it_are_the_scripts_from_the_case() {
    let c = Case::open(&canyon()).unwrap();
    let (d, _) = Description::import(&c);
    let (back, _) = Description::read(&Document::parse(&d.write()).unwrap());
    let mut from_file = back.workflow();
    let from_case = c.workflow();
    from_file.constant_files = from_case.constant_files.clone();
    // The description lists the regions in the order their meshes are built: Allrun decomposes
    // them in that order, the case's scripts in the case's (and OpenFOAM-8's Allrun reads the
    // process count from the first region's decomposeParDict: here air in both). Nothing else
    // differs.
    let same = |scripts: Vec<(&'static str, String)>| -> Vec<(&'static str, String)> {
        scripts
            .into_iter()
            .map(|(n, t)| (n, t.lines().filter(|l| !l.starts_with("for r in ")).collect::<Vec<_>>().join("\n")))
            .collect()
    };
    for v in OfVersion::ALL {
        let (a, b) = (from_file.generate(v).unwrap(), from_case.generate(v).unwrap());
        assert!(a.iter().any(|(_, t)| t.contains("for r in air vegetation street")), "{a:?}");
        assert_eq!(same(a), same(b), "{v}");
    }
}

#[test]
fn problems_are_named_and_the_rest_is_read() {
    let text = format!(
        "{}version 9;\ncolour blue;\nregions\n{{\n    air\n    {{\n        kind fluid;\n        mesh ( createPatch );\n        steps ( topoSet paint );\n        patches {{ west {{ role door; }} east {{ role outlet; partner air; }} north {{ role side; extra 1; }} }}\n    }}\n    nokind {{ mesh blocks; }}\n    air {{ kind solid; }}\n    street {{ kind solid; mesh blocks; steps ( setSet setFields ); }}\n}}\nradiation {{ lai air; }}\nreconstruct maybe;\n",
        header("dictionary", "umcfWorkflow")
    );
    let (d, problems) = Description::read(&Document::parse(&text).unwrap());
    let at: Vec<&str> = problems.iter().map(|p| p.at.as_str()).collect();
    assert_eq!(
        at,
        [
            "version",
            "colour",
            "regions/air/mesh",
            "regions/air/steps",
            "regions/air/patches/west/role",
            "regions/air/patches/east/partner",
            "regions/air/patches/north/extra",
            "regions/nokind/kind",
            "regions/air",
            "regions/street/steps",
            "radiation/region",
            "reconstruct",
        ],
        "{problems:#?}"
    );
    assert!(problems.iter().any(|p| p.message.contains("starts with blockMesh")));
    assert!(problems.iter().any(|p| p.message.contains("`paint` is not a step")));
    // What could be read is.
    let names: Vec<&str> = d.regions.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["air", "street"]);
    let air = d.region("air").unwrap();
    assert_eq!(air.kind, RegionKind::Fluid);
    assert_eq!(air.patches.iter().map(|p| p.role).collect::<Vec<_>>(), [None, Some(Role::Outlet), Some(Role::Side)]);
    assert_eq!(d.region("street").unwrap().mesh, Some(MeshBuild::Block));
    assert!(d.radiation.is_none());
}

#[test]
fn radiation_values_that_are_not_one_name_are_problems() {
    let text = format!(
        "{}regions {{ }}\nradiation {{ region vegetation; lai ( air ); }}\n",
        header("dictionary", "umcfWorkflow")
    );
    let (d, problems) = Description::read(&Document::parse(&text).unwrap());
    let at: Vec<&str> = problems.iter().map(|p| p.at.as_str()).collect();
    assert_eq!(at, ["radiation/lai"]);
    assert_eq!(d.radiation.unwrap().lai_region, None);
    let text = format!("{}regions {{ }}\nradiation {{ region ( a b ); }}\n", header("dictionary", "umcfWorkflow"));
    let (_, problems) = Description::read(&Document::parse(&text).unwrap());
    let at: Vec<&str> = problems.iter().map(|p| p.at.as_str()).collect();
    assert_eq!(at, ["radiation/region"], "one problem, not also `is missing`");
}

#[test]
fn a_case_holds_one_description() {
    let fs = canyon();
    let mut c = Case::open(&fs).unwrap();
    assert!(c.description().is_none());
    let (d, _) = Description::import(&c);
    c.put_description(&fs, &d.write(), false).unwrap();
    let (back, problems) = c.description().unwrap();
    assert_eq!((back, problems), (d.clone(), vec![]));
    assert!(c.changed().contains(&PATH.to_string()));
    // There is one now: replacing it is asked for.
    assert!(matches!(c.put_description(&fs, &d.write(), false), Err(crate::Error::Exists(_))));
    c.put_description(&fs, &d.write(), true).unwrap();
    // On disk but not a dictionary: not written over.
    let fs = canyon().with(PATH, "not { a dictionary");
    let mut c = Case::open(&fs).unwrap();
    assert!(c.description().is_none());
    assert!(matches!(c.put_description(&fs, &d.write(), true), Err(crate::Error::NotLoaded { .. })));
}

#[cfg(unix)]
#[test]
fn a_description_linked_to_another_dictionary_is_not_written() {
    use umcf_core::{CaseWrite, DirFs};
    let root = std::env::temp_dir().join(format!("umcf-model-description-alias-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let mut fs = DirFs::new(&root);
    fs.write("system/controlDict", format!("{}{CONTROL}", header("dictionary", "controlDict")).as_bytes()).unwrap();
    std::os::unix::fs::symlink("controlDict", root.join(PATH)).unwrap();
    let mut c = Case::open(&fs).unwrap();
    let r = c.put_description(&fs, "regions { }\n", true);
    std::fs::remove_dir_all(&root).unwrap();
    assert!(matches!(r, Err(crate::Error::IsDict(_))), "{r:?}");
    assert!(c.changed().is_empty());
}
