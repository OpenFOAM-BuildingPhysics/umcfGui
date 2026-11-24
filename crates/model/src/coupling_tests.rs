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
            assert_eq!(differences(&root(&text), &entries(&d, r, v).unwrap()), Vec::<String>::new(), "{v} {r}");
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
    assert_eq!(differences(&root(old_keys), &air12), Vec::<String>::new());
    // Another type or partner, a patch missing, one the description doesn't map; a pattern is
    // left alone.
    let other = "boundary { street { type mappedPatch; neighbourRegion air; neighbourPatch street; } west { type patch; } \"air_.*\" { type wall; } }";
    assert_eq!(
        differences(&root(other), &air12),
        [
            "patch street: type is mappedPatch there, the description makes it mappedWall",
            "patch street: the region it maps to is air there, the description makes it street",
            "patch west is changed there, the description doesn't map it",
        ]
    );
    assert_eq!(differences(&root("boundary { }"), &air12), ["patch street isn't there"]);
    assert_eq!(differences(&root("x 1;"), &air12), ["it has no boundary dictionary"]);
    // OpenFOAM-8's canopy samples the air's cells: nearestPatchFace there is another mapping.
    let veg8 = entries(&d, "vegetation", OfVersion::V8).unwrap();
    let text = dict(&d, "vegetation", OfVersion::V8).unwrap().replace("nearestCell", "nearestPatchFace");
    let diff = differences(&root(&text), &veg8);
    assert_eq!(
        diff,
        ["patch air_to_vegetation: sampleMode is nearestPatchFace there, the description makes it nearestCell"]
    );
    // A patch mapped to the air's cells reads no patch: one named there is fine.
    let veg12 = entries(&d, "vegetation", OfVersion::V12).unwrap();
    let text = dict(&d, "vegetation", OfVersion::V12)
        .unwrap()
        .replace("distance        0;", "distance        0;\n        neighbourPatch  street;");
    assert_eq!(differences(&root(&text), &veg12), Vec::<String>::new());
}
