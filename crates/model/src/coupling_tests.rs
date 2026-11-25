use super::*;
use crate::description::PatchDesc;

fn region(name: &str, kind: RegionKind, patches: Vec<PatchDesc>) -> RegionDesc {
    RegionDesc {
        name: name.into(),
        kind,
        mesh: None,
        steps: vec![RegionStep::ChangeDictionary],
        patches,
        materials: Vec::new(),
        blocks: None,
        mesh_box: None,
        snappy: None,
    }
}

fn patch(name: &str, role: Option<Role>, partner: Option<(&str, &str)>) -> PatchDesc {
    PatchDesc { name: name.into(), role, partner: partner.map(|(r, p)| (r.into(), p.into())) }
}

fn canyon() -> Description {
    Description {
        regions: vec![
            region(
                "air",
                RegionKind::Fluid,
                vec![
                    patch("street", Some(Role::Coupled), Some(("street", "street"))),
                    patch("west", Some(Role::Inlet), None),
                ],
            ),
            region("street", RegionKind::Solid, vec![patch("street", Some(Role::Coupled), Some(("air", "street")))]),
            region(
                "vegetation",
                RegionKind::Vegetation,
                vec![
                    patch("west", Some(Role::Sky), Some(("air", "west"))),
                    patch("street", Some(Role::Coupled), Some(("air", "street"))),
                    patch("air_to_vegetation", Some(Role::Canopy), None),
                ],
            ),
        ],
        ..Default::default()
    }
}

fn root(text: &str) -> umcf_foam_dict::Dict {
    umcf_foam_dict::Document::parse(text).unwrap().root
}

#[test]
fn patches_are_mapped_by_role_and_partner_per_version() {
    let d = canyon();
    let types =
        |r: &str, v| entries(&d, r, v).unwrap().into_iter().map(|e| (e.patch, e.keys[0].1.clone())).collect::<Vec<_>>();
    assert_eq!(types("air", OfVersion::V12), [("street".to_string(), "mappedWall".to_string())]);
    assert_eq!(
        types("vegetation", OfVersion::V12),
        [
            ("west".into(), "mapped".into()),
            ("street".into(), "mappedWall".into()),
            ("air_to_vegetation".into(), "mappedInternal".into())
        ]
    );
    assert_eq!(types("vegetation", OfVersion::V8)[0].1, "mappedPatch");
    assert_eq!(types("vegetation", OfVersion::V8)[2].1, "mappedWall");
    // A fluid's and a solid's: by the nearest face; a vegetation twin: none.
    let air = entries(&d, "air", OfVersion::V12).unwrap();
    assert_eq!(air[0].get("method"), Some("nearest"));
    let veg = entries(&d, "vegetation", OfVersion::V12).unwrap();
    assert!(veg.iter().all(|e| e.get("method").is_none()));
    assert_eq!(veg[2].get("neighbourRegion"), Some("air"));
    let veg8 = entries(&d, "vegetation", OfVersion::V8).unwrap();
    assert_eq!((veg8[2].get("sampleMode"), veg8[2].get("sampleRegion")), (Some("nearestCell"), Some("air")));
    // Generated text parses back to the same entries, and is marked so.
    for v in OfVersion::ALL {
        for r in ["air", "street", "vegetation"] {
            let text = dict(&d, r, v).unwrap();
            assert!(crate::blocks::untouched(&text));
            assert_eq!(differences(&root(&text), &entries(&d, r, v).unwrap(), v), Vec::<String>::new(), "{v} {r}");
        }
    }
}

#[test]
fn what_cant_be_mapped_is_refused() {
    let mut d = canyon();
    d.regions[1].patches[0].role = None;
    let e = entries(&d, "street", OfVersion::V12).unwrap_err();
    assert!(e.contains("coupled to air/street but has no role"), "{e}");
    let mut d = canyon();
    d.regions[0].patches[0].partner = Some(("roof".into(), "x".into()));
    assert!(entries(&d, "air", OfVersion::V12).unwrap_err().contains("region roof, which isn't in the description"));
    let mut d = canyon();
    d.regions[2].patches[2].partner = Some(("air".into(), "west".into()));
    assert!(entries(&d, "vegetation", OfVersion::V12).unwrap_err().contains("is the canopy and coupled to air/west"));
    // A name that would change the dictionary's syntax or where it goes: refused, not written.
    let mut d = canyon();
    d.regions[0].patches[0].name = "street { type wall; } extra".into();
    let e = dict(&d, "air", OfVersion::V12).unwrap_err();
    assert!(e.contains("isn't a plain OpenFOAM word"), "{e}");
    let mut d = canyon();
    d.regions[0].patches[0].partner = Some(("street".into(), "a;b".into()));
    assert!(entries(&d, "air", OfVersion::V8).unwrap_err().contains("`a;b` isn't a plain"));
    let mut d = canyon();
    d.regions[1].name = "../street".into();
    assert!(entries(&d, "../street", OfVersion::V12).unwrap_err().contains("region `../street` isn't a plain"));
    for bad in ["..", ".", "a:b", "street.", "CON"] {
        let mut d = canyon();
        d.regions[1].name = bad.into();
        let e = entries(&d, bad, OfVersion::V12).unwrap_err();
        assert!(e.contains(&format!("region `{bad}`")), "{bad}: {e}");
        // As a partner's region too (one the description has).
        let mut d = canyon();
        d.regions[1].name = bad.into();
        d.regions[0].patches[0].partner = Some((bad.into(), "street".into()));
        assert!(entries(&d, "air", OfVersion::V12).unwrap_err().contains(&format!("region `{bad}`")), "{bad}");
    }
    // A partner region that isn't there is said so first, whatever its name.
    let mut d = canyon();
    d.regions[0].patches[0].partner = Some(("../x".into(), "p".into()));
    assert!(entries(&d, "air", OfVersion::V12).unwrap_err().contains("region ../x, which isn't in the description"));
    // The canopy's air, told by the radiation's lai region, is a directory too.
    let mut d = canyon();
    d.regions[2].patches.retain(|p| p.role == Some(Role::Canopy));
    d.regions[0].name = "nul.txt".into();
    let e = entries(&d, "vegetation", OfVersion::V8).unwrap_err();
    assert!(e.contains("region `nul.txt` can't be a directory"), "{e}");
    for fine in ["com0", "air.1", "a-b"] {
        let mut d = canyon();
        d.regions[1].name = fine.into();
        assert!(entries(&d, fine, OfVersion::V12).is_ok(), "{fine}");
    }
    // A patch without a partner keeps any name: it isn't written.
    let mut d = canyon();
    d.regions[0].patches[1].name = "west side".into();
    assert!(entries(&d, "air", OfVersion::V12).is_ok());
    // A canopy alone with two fluids and no lai: which air isn't told.
    let mut d = canyon();
    d.regions[2].patches.retain(|p| p.role == Some(Role::Canopy));
    d.regions.push(region("air2", RegionKind::Fluid, Vec::new()));
    assert!(entries(&d, "vegetation", OfVersion::V12).unwrap_err().contains("which fluid region isn't told"));
    d.radiation = Some(crate::workflow::Radiation {
        region: "vegetation".into(),
        lai_region: Some("air2".into()),
        view_factors: false,
        solar_ray_tracing: false,
    });
    assert_eq!(entries(&d, "vegetation", OfVersion::V12).unwrap()[0].get("neighbourRegion"), Some("air2"));
}

#[test]
fn a_cases_own_dictionary_is_compared_by_what_the_coupling_needs() {
    let d = canyon();
    let air12 = entries(&d, "air", OfVersion::V12).unwrap();
    // OpenFOAM-12 reads the older keys too; method and offsets aren't compared.
    let old_keys = "boundary { street { type mappedWall; sampleRegion street; samplePatch street; offsetMode uniform; offset (1 0 0); } }";
    assert_eq!(differences(&root(old_keys), &air12, OfVersion::V12), Vec::<String>::new());
    // Another type or partner, a patch missing, one the description doesn't map; a pattern is
    // left alone.
    let other = "boundary { street { type mappedPatch; neighbourRegion air; neighbourPatch street; } west { type patch; } \"air_.*\" { type wall; } }";
    assert_eq!(
        differences(&root(other), &air12, OfVersion::V12),
        [
            "patch street: type is mappedPatch there, the description makes it mappedWall",
            "patch street: the region it maps to is air there, the description makes it street",
            "patch west is changed there, the description doesn't map it",
        ]
    );
    // OpenFOAM-12 reads the newer keys first: an older one beside them doesn't count.
    let both = "boundary { street { type mappedWall; neighbourRegion street; neighbourPatch street; sampleRegion roof; samplePatch roof; } }";
    assert_eq!(differences(&root(both), &air12, OfVersion::V12), Vec::<String>::new());
    // OpenFOAM-8 reads only the older keys: newer ones beside them don't hide another mapping.
    let air8 = entries(&d, "air", OfVersion::V8).unwrap();
    let both8 = "boundary { street { type mappedWall; sampleMode nearestPatchFace; neighbourRegion street; neighbourPatch street; sampleRegion roof; samplePatch roof; } }";
    assert_eq!(
        differences(&root(both8), &air8, OfVersion::V8),
        [
            "patch street: the region it maps to is roof there, the description makes it street",
            "patch street: the patch it maps to is roof there, the description makes it street",
        ]
    );
    let new_only8 = "boundary { street { type mappedWall; neighbourRegion street; neighbourPatch street; } }";
    // Its region, its patch, and no sampleMode, which OpenFOAM-8 reads for every mapped patch.
    assert_eq!(differences(&root(new_only8), &air8, OfVersion::V8).len(), 3);
    // OpenFOAM-8's canopy with only the newer key: it samples no region there.
    let veg8 = entries(&d, "vegetation", OfVersion::V8).unwrap();
    let canopy8 = "boundary { west { type mappedPatch; sampleMode nearestPatchFace; sampleRegion air; samplePatch west; } street { type mappedWall; sampleMode nearestPatchFace; sampleRegion air; samplePatch street; } air_to_vegetation { type mappedWall; sampleMode nearestCell; neighbourRegion air; } }";
    assert_eq!(
        differences(&root(canopy8), &veg8, OfVersion::V8),
        ["patch air_to_vegetation: the region it maps to is not given there, the description makes it air"]
    );
    assert_eq!(differences(&root("boundary { }"), &air12, OfVersion::V12), ["patch street isn't there"]);
    assert_eq!(differences(&root("x 1;"), &air12, OfVersion::V12), ["it has no boundary dictionary"]);
    // OpenFOAM-8's canopy samples the air's cells: nearestPatchFace there is another mapping.
    let veg8 = entries(&d, "vegetation", OfVersion::V8).unwrap();
    let text = dict(&d, "vegetation", OfVersion::V8).unwrap().replace("nearestCell", "nearestPatchFace");
    let diff = differences(&root(&text), &veg8, OfVersion::V8);
    assert_eq!(
        diff,
        ["patch air_to_vegetation: sampleMode is nearestPatchFace there, the description makes it nearestCell"]
    );
    // OpenFOAM-8's canopy samples the cells: it stops on a samplePatch (none is fine).
    let text = dict(&d, "vegetation", OfVersion::V8)
        .unwrap()
        .replace("sampleMode      nearestCell;", "sampleMode      nearestCell;\n        samplePatch     nowhere;");
    assert!(text.contains("samplePatch     nowhere;"));
    assert_eq!(
        differences(&root(&text), &veg8, OfVersion::V8),
        ["patch air_to_vegetation: samplePatch is nowhere there, which OpenFOAM-8 refuses in sampleMode nearestCell"]
    );
    let none = text.replace("samplePatch     nowhere;", "samplePatch     none;");
    assert_eq!(differences(&root(&none), &veg8, OfVersion::V8), Vec::<String>::new());
    // A sample mode OpenFOAM-8 hasn't (OpenFOAM-12's method, copied over); a macro isn't told.
    let text = dict(&d, "air", OfVersion::V8).unwrap();
    let air8 = entries(&d, "air", OfVersion::V8).unwrap();
    assert_eq!(
        differences(&root(&text.replace("nearestPatchFace", "nearest")), &air8, OfVersion::V8),
        [
            "patch street: its sampleMode is nearest there, which OpenFOAM-8 hasn't (it has nearestCell, nearestPatchFace, nearestPatchFaceAMI, nearestPatchPoint, nearestFace, nearestOnlyCell)"
        ]
    );
    let mac = text.replace("nearestPatchFace", "$mode");
    assert_eq!(differences(&root(&mac), &air8, OfVersion::V8), Vec::<String>::new());
    // Nor on the canopy, nor a region, patch or type given by one, nor a directive's value.
    let veg8 = entries(&d, "vegetation", OfVersion::V8).unwrap();
    let canopy = dict(&d, "vegetation", OfVersion::V8).unwrap();
    for (from, to) in [
        ("nearestCell", "$mode"),
        ("nearestCell", "#calc \"x\""),
        ("sampleRegion    air;", "sampleRegion    $air;"),
        ("type            mappedPatch;", "type            $open;"),
    ] {
        let t = canopy.replace(from, to);
        assert_eq!(differences(&root(&t), &veg8, OfVersion::V8), Vec::<String>::new(), "{to}");
    }
    let t = text.replace("samplePatch     street;", "samplePatch     $p;");
    assert_eq!(differences(&root(&t), &air8, OfVersion::V8), Vec::<String>::new());
    // OpenFOAM-8 stops on a patch in every mode that samples none, the one there told first.
    let t = text.replace("nearestPatchFace", "nearestFace");
    assert_eq!(
        differences(&root(&t), &air8, OfVersion::V8),
        [
            "patch street: sampleMode is nearestFace there, the description makes it nearestPatchFace",
            "patch street: samplePatch is street there, which OpenFOAM-8 refuses in sampleMode nearestFace",
        ]
    );
    let t = canopy.replace("nearestCell;", "nearestPatchFace;\n        samplePatch     street;");
    assert_eq!(
        differences(&root(&t), &veg8, OfVersion::V8),
        [
            "patch air_to_vegetation: the patch it maps to is street there, the description makes it not given",
            "patch air_to_vegetation: sampleMode is nearestPatchFace there, the description makes it nearestCell",
        ]
    );
    // A patch coupled by its group names no region or patch.
    let grouped = "boundary { street { type mappedWall; sampleMode nearestPatchFace; coupleGroup g; } }";
    assert_eq!(differences(&root(grouped), &air8, OfVersion::V8), Vec::<String>::new());
    // OpenFOAM-8 takes the region and patch given over the group's only when both are;
    // OpenFOAM-12 stops on a group with either.
    let one = "boundary { street { type mappedWall; sampleMode nearestPatchFace; coupleGroup g; sampleRegion roof; } }";
    assert_eq!(differences(&root(one), &air8, OfVersion::V8), Vec::<String>::new());
    let over = "boundary { street { type mappedWall; sampleMode nearestPatchFace; coupleGroup g; sampleRegion roof; samplePatch street; } }";
    assert_eq!(
        differences(&root(over), &air8, OfVersion::V8),
        ["patch street: the region it maps to is roof there, the description makes it street"]
    );
    // OpenFOAM-8 reads a sampleMode for every mapped patch.
    let modeless = "boundary { street { type mappedWall; sampleRegion street; samplePatch street; } }";
    assert_eq!(
        differences(&root(modeless), &air8, OfVersion::V8),
        ["patch street: sampleMode is not given there, the description makes it nearestPatchFace"]
    );
    let air12 = entries(&d, "air", OfVersion::V12).unwrap();
    let alone = "boundary { street { type mappedWall; coupleGroup g; } }";
    assert_eq!(differences(&root(alone), &air12, OfVersion::V12), Vec::<String>::new());
    // samePatch maps to the patch of the same name.
    let veg12 = entries(&d, "vegetation", OfVersion::V12).unwrap();
    let text =
        dict(&d, "vegetation", OfVersion::V12).unwrap().replacen("neighbourPatch  west;", "samePatch       true;", 1);
    assert!(text.contains("samePatch       true;"));
    assert_eq!(differences(&root(&text), &veg12, OfVersion::V12), Vec::<String>::new());
    let t = text.replacen("samePatch       true;", "samePatch       y;\n        samplePatch     west;", 1);
    assert_eq!(
        differences(&root(&t), &veg12, OfVersion::V12),
        ["patch west: it has samePatch and samplePatch there, which OpenFOAM-12 refuses (one or the other)"]
    );
    let same = "boundary { street { type mappedWall; coupleGroup g; samePatch false; } }";
    assert_eq!(differences(&root(same), &air12, OfVersion::V12), Vec::<String>::new());
    let both = "boundary { street { type mappedWall; coupleGroup g; neighbourRegion street; } }";
    assert_eq!(
        differences(&root(both), &air12, OfVersion::V12),
        ["patch street: it has a coupleGroup and neighbourRegion there, which OpenFOAM-12 refuses (one or the other)"]
    );
    // The canopy samples the cells in nearestOnlyCell too; without a mode there, the
    // description's tells it samples no patch.
    let t = canopy.replace("nearestCell;", "nearestOnlyCell;");
    assert_eq!(differences(&root(&t), &veg8, OfVersion::V8), Vec::<String>::new());
    let t = canopy.replace("sampleMode      nearestCell;", "samplePatch     street;");
    assert_eq!(
        differences(&root(&t), &veg8, OfVersion::V8),
        ["patch air_to_vegetation: sampleMode is not given there, the description makes it nearestCell",]
    );
    // OpenFOAM-12 reads sampleMode only as an older method: the OpenFOAM-8 canopy's is fine
    // on a mappedInternal, and one it has no method of is said.
    let veg12 = entries(&d, "vegetation", OfVersion::V12).unwrap();
    let text = dict(&d, "vegetation", OfVersion::V12)
        .unwrap()
        .replace("distance        0;", "distance        0;\n        sampleMode      nearestCell;");
    assert_eq!(differences(&root(&text), &veg12, OfVersion::V12), Vec::<String>::new());
    let left = "boundary { street { type mappedWall; sampleMode nearestPatchFace; neighbourRegion street; neighbourPatch street; } }";
    assert_eq!(
        differences(&root(left), &air12, OfVersion::V12),
        [
            "patch street: its method is nearestPatchFace there, which OpenFOAM-12 hasn't (it has intersection, inverseDistance, matching, nearest)"
        ]
    );
    // An open side's too.
    let veg12o = entries(&d, "vegetation", OfVersion::V12).unwrap();
    let text = dict(&d, "vegetation", OfVersion::V12).unwrap();
    let text = text.replacen(
        "type            mapped;",
        "type            mapped;\n        method          nearestPatchFace;",
        1,
    );
    assert!(
        differences(&root(&text), &veg12o, OfVersion::V12)
            .iter()
            .any(|m| m.starts_with("patch west: its method is nearestPatchFace there")),
        "{text}"
    );
    let fine = "boundary { street { type mappedWall; method matching; sampleMode nearestPatchFace; neighbourRegion street; neighbourPatch street; } }";
    assert_eq!(differences(&root(fine), &air12, OfVersion::V12), Vec::<String>::new());
    // A value in boundary goes into the mesh's boundary file.
    let air8 = entries(&d, "air", OfVersion::V8).unwrap();
    let text = dict(&d, "air", OfVersion::V8).unwrap().replace("boundary\n{", "boundary\n{\n    foo 1;");
    assert_eq!(
        differences(&root(&text), &air8, OfVersion::V8),
        ["foo is a value in boundary there: changeDictionary adds it to the mesh's boundary file"]
    );
    // A patch mapped to the air's cells reads no patch: one named there is fine.
    let veg12 = entries(&d, "vegetation", OfVersion::V12).unwrap();
    let text = dict(&d, "vegetation", OfVersion::V12)
        .unwrap()
        .replace("distance        0;", "distance        0;\n        neighbourPatch  street;");
    assert_eq!(differences(&root(&text), &veg12, OfVersion::V12), Vec::<String>::new());
}
