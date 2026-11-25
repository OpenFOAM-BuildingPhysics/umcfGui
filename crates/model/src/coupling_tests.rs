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
    // OpenFOAM-12 reads the older keys too, after the newer ones: the mesh's patch keeps the
    // newer where it has them, so the older ones' can't be confirmed; one that differs is said
    // to. Method and offsets aren't compared.
    let old_keys = "boundary { street { type mappedWall; sampleRegion street; samplePatch street; offsetMode uniform; offset (1 0 0); } }";
    assert_eq!(
        differences(&root(old_keys), &air12, OfVersion::V12),
        [
            "patch street: the region it maps to is given there as sampleRegion only, which OpenFOAM-12 reads after neighbourRegion, so the mesh's neighbourRegion, where it has one, is kept, which can't be confirmed (the description makes it street)",
            "patch street: the patch it maps to is given there as samplePatch only, which OpenFOAM-12 reads after neighbourPatch, so the mesh's neighbourPatch, where it has one, is kept, which can't be confirmed (the description makes it street)",
        ]
    );
    let old_other = "boundary { street { type mappedWall; sampleRegion roof; neighbourPatch street; } }";
    assert_eq!(
        differences(&root(old_other), &air12, OfVersion::V12),
        ["patch street: the region it maps to is roof there, the description makes it street"]
    );
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
    // OpenFOAM-12's keys alone: OpenFOAM-8 reads neither (what it reads is the mesh's, not told).
    assert_eq!(
        differences(&root(new_only8), &air8, OfVersion::V8),
        [
            "patch street: it has neighbourRegion there, which OpenFOAM-8 doesn't read (it reads sampleRegion)",
            "patch street: it has neighbourPatch there, which OpenFOAM-8 doesn't read (it reads samplePatch)",
        ]
    );
    // OpenFOAM-8's canopy with only the newer key: it samples no region there.
    let veg8 = entries(&d, "vegetation", OfVersion::V8).unwrap();
    let canopy8 = "boundary { west { type mappedPatch; sampleMode nearestPatchFace; sampleRegion air; samplePatch west; } street { type mappedWall; sampleMode nearestPatchFace; sampleRegion air; samplePatch street; } air_to_vegetation { type mappedWall; sampleMode nearestCell; neighbourRegion air; } }";
    assert_eq!(
        differences(&root(canopy8), &veg8, OfVersion::V8),
        [
            "patch air_to_vegetation: it has neighbourRegion there, which OpenFOAM-8 doesn't read (it reads sampleRegion)"
        ]
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
    // A mode from a macro: whether it samples a patch isn't told, so none isn't compared.
    let mac = none.replace("sampleMode      nearestCell;", "sampleMode      $mode;");
    assert!(mac.contains("$mode"));
    assert_eq!(differences(&root(&mac), &veg8, OfVersion::V8), Vec::<String>::new());
    // A region or patch left out keeps the mesh's value (changeDictionary merges): not told,
    // so said to be unconfirmed, whether the description's partner is in the same region or
    // another. A type left out isn't compared.
    let inner = |keys: &[(&'static str, &str)]| {
        vec![Entry { patch: "inlet".into(), keys: keys.iter().map(|(k, v)| (*k, v.to_string())).collect() }]
    };
    let in12 = inner(&[("type", "mapped"), ("neighbourRegion", "air"), ("neighbourPatch", "outlet")]);
    let omitted12 = "boundary { inlet { type mapped; neighbourPatch outlet; } }";
    let unconfirmed = |p: &str, w: &str| {
        format!(
            "patch {p}: the region it maps to isn't given there, so the mesh's is kept, which can't be confirmed (the description makes it {w})"
        )
    };
    assert_eq!(differences(&root(omitted12), &in12, OfVersion::V12), [unconfirmed("inlet", "air")]);
    let in8 = inner(&[
        ("type", "mapped"),
        ("sampleMode", "nearestPatchFace"),
        ("sampleRegion", "air"),
        ("samplePatch", "outlet"),
    ]);
    let omitted8 = "boundary { inlet { type mapped; sampleMode nearestPatchFace; samplePatch outlet; } }";
    assert_eq!(differences(&root(omitted8), &in8, OfVersion::V8), [unconfirmed("inlet", "air")]);
    let omitted = "boundary { street { type mappedWall; neighbourPatch street; } }";
    assert_eq!(differences(&root(omitted), &air12, OfVersion::V12), [unconfirmed("street", "street")]);
    // OpenFOAM-12's mappedInternal reads a region only; OpenFOAM-8's cells' modes no patch: the
    // region alone is unconfirmed then.
    let veg12 = entries(&d, "vegetation", OfVersion::V12).unwrap();
    let t = "boundary { west { type mapped; neighbourRegion air; neighbourPatch west; } street { type mappedWall; neighbourRegion air; neighbourPatch street; } air_to_vegetation { type mappedInternal; } }";
    assert_eq!(differences(&root(t), &veg12, OfVersion::V12), [unconfirmed("air_to_vegetation", "air")]);
    let veg8 = entries(&d, "vegetation", OfVersion::V8).unwrap();
    let t = "boundary { west { type mappedPatch; sampleMode nearestPatchFace; sampleRegion air; samplePatch west; } street { type mappedWall; sampleMode nearestPatchFace; sampleRegion air; samplePatch street; } air_to_vegetation { type mappedWall; sampleMode nearestCell; } }";
    assert_eq!(differences(&root(t), &veg8, OfVersion::V8), [unconfirmed("air_to_vegetation", "air")]);
    let typeless = "boundary { street { neighbourRegion street; neighbourPatch street; } }";
    assert_eq!(differences(&root(typeless), &air12, OfVersion::V12), Vec::<String>::new());
    // A type that is a dictionary isn't a word: refused.
    let t = "boundary { street { type { }; neighbourRegion street; neighbourPatch street; } }";
    assert_eq!(
        differences(&root(t), &air12, OfVersion::V12),
        ["patch street: type is a dictionary there, which OpenFOAM refuses (not a word)"]
    );
    // Nor a mode left out (OpenFOAM-8): the samplePatch then isn't compared either.
    let modeless = "boundary { street { type mappedWall; sampleRegion street; samplePatch roof; } }";
    assert_eq!(differences(&root(modeless), &air8, OfVersion::V8), Vec::<String>::new());
    // A region or patch key that holds a dictionary isn't a word: refused, and OpenFOAM-12
    // doesn't fall back to the older key then.
    let t = "boundary { street { type mappedWall; neighbourRegion { }; sampleRegion street; neighbourPatch street; } }";
    assert_eq!(
        differences(&root(t), &air12, OfVersion::V12),
        ["patch street: neighbourRegion is a dictionary there, which OpenFOAM refuses (not a word)"]
    );
    // `~key` takes a key out of the mesh's patch: what OpenFOAM reads then isn't told, so the
    // patch isn't compared, said once.
    let taken = "boundary { street { type mappedPatch; ~neighbourRegion; ~sampleMode; neighbourRegion roof; } }";
    assert_eq!(
        differences(&root(taken), &air12, OfVersion::V12),
        [
            "patch street: it takes neighbourRegion, sampleMode out of the mesh's patch there, after which what OpenFOAM reads isn't told: not compared"
        ]
    );
    // changeDictionary stops on a patch taken out of boundary; said once for a mapped one.
    let out_patch = "boundary { street { type mappedWall; neighbourRegion street; neighbourPatch street; } ~west; }";
    assert_eq!(
        differences(&root(out_patch), &air12, OfVersion::V12),
        [
            "~west is in boundary there: changeDictionary stops where the mesh has patch west (it can't take a patch out)"
        ]
    );
    assert_eq!(
        differences(&root("boundary { ~street; }"), &air12, OfVersion::V12),
        [
            "~street is in boundary there: changeDictionary stops where the mesh has patch street (it can't take a patch out)"
        ]
    );
    let taken8 =
        "boundary { street { type mappedWall; sampleMode nearestPatchFace; ~sampleRegion; samplePatch street; } }";
    assert_eq!(
        differences(&root(taken8), &entries(&d, "air", OfVersion::V8).unwrap(), OfVersion::V8),
        [
            "patch street: it takes sampleRegion out of the mesh's patch there, after which what OpenFOAM reads isn't told: not compared"
        ]
    );
    // Quoted, too (OpenFOAM reads the `~` first); a pattern taken out does nothing.
    let quoted = "boundary { street { type mappedWall; neighbourRegion street; neighbourPatch street; } \"~west\"; \"~air_.*\"; }";
    assert_eq!(
        differences(&root(quoted), &air12, OfVersion::V12),
        [
            "~west is in boundary there: changeDictionary stops where the mesh has patch west (it can't take a patch out)"
        ]
    );
    // With a block for the patch: stops, or writes the block without the faces; not compared.
    let with_block = "boundary { ~street; street { type mappedPatch; neighbourRegion roof; } }";
    assert_eq!(
        differences(&root(with_block), &air12, OfVersion::V12),
        [
            "~street is in boundary there with a block for street: changeDictionary stops where the mesh has patch street (the block first), or writes the block in its place without its faces (the block after)"
        ]
    );
    // A samePatch that isn't a switch is refused; samePatch alone beside the older region key
    // says both.
    for (v, said) in [
        ("maybe", "samePatch is maybe there"),
        ("{ }", "samePatch is a dictionary there"),
        ("\"\"", "samePatch is \"\" there"),
        ("\"true\"", "samePatch is \"true\" there"),
        ("1.0", "samePatch is 1.0 there"),
    ] {
        let t = format!(
            "boundary {{ street {{ type mappedWall; samePatch {v}; neighbourRegion street; neighbourPatch street; }} }}"
        );
        assert_eq!(
            differences(&root(&t), &air12, OfVersion::V12),
            [format!("patch street: {said}, which OpenFOAM-12 refuses (not a switch)")]
        );
    }
    let t = "boundary { street { type mappedWall; samePatch maybe; neighbourRegion street; } }";
    assert_eq!(
        differences(&root(t), &air12, OfVersion::V12),
        ["patch street: samePatch is maybe there, which OpenFOAM-12 refuses (not a switch)"]
    );
    let t = "boundary { street { type mappedWall; samePatch true; sampleRegion street; } }";
    let diff = differences(&root(t), &air12, OfVersion::V12);
    assert_eq!(diff.len(), 2, "{diff:?}");
    assert!(
        diff[0].contains("given there as sampleRegion only") && diff[1].contains("it has samePatch there"),
        "{diff:?}"
    );
    // samePatch alone where the description maps another patch: compared.
    let mut other = canyon();
    other.regions[0].patches[0].partner = Some(("street".into(), "wall".into()));
    other.regions[1].patches[0].name = "wall".into();
    let to_wall = entries(&other, "air", OfVersion::V12).unwrap();
    let t = "boundary { street { type mappedWall; samePatch true; neighbourRegion street; } }";
    assert_eq!(
        differences(&root(t), &to_wall, OfVersion::V12),
        ["patch street: the patch it maps to is street there, the description makes it wall"]
    );
    // OpenFOAM's switches: a number other than 0 is true, 0 false.
    for (v, refused) in [("1", true), ("true", true), ("0", false), ("false", false)] {
        let t = format!(
            "boundary {{ street {{ type mappedWall; samePatch {v}; neighbourRegion street; neighbourPatch street; }} }}"
        );
        let diff = differences(&root(&t), &air12, OfVersion::V12);
        assert_eq!(diff.iter().any(|m| m.contains("it has samePatch and neighbourPatch")), refused, "{v}: {diff:?}");
    }
    // Alone, the mesh's patch may keep a neighbourPatch beside it, which OpenFOAM-12 refuses.
    let t = "boundary { street { type mappedWall; samePatch 1; neighbourRegion street; } }";
    assert_eq!(
        differences(&root(t), &air12, OfVersion::V12),
        [
            "patch street: it has samePatch there, which OpenFOAM-12 refuses beside a neighbourPatch, and the mesh's patch keeps one it has (OpenFOAM-12 writes one on a mapped patch), which can't be confirmed"
        ]
    );
    // In the modes that sample no patch OpenFOAM-8 stops on a patch given (none will do),
    // with a group too (its patch would replace it, refused all the same).
    let veg8 = entries(&d, "vegetation", OfVersion::V8).unwrap();
    let only = |t: &str| differences(&root(t), &veg8, OfVersion::V8);
    for m in ["nearestCell", "nearestOnlyCell", "nearestFace"] {
        for group in ["", "coupleGroup g;"] {
            let t = |keys: &str| {
                format!("boundary {{ air_to_vegetation {{ type mappedWall; sampleMode {m}; {group} {keys} }} }}")
            };
            let refused = format!(
                "patch air_to_vegetation: samplePatch is west there, which OpenFOAM-8 refuses in sampleMode {m}"
            );
            assert!(only(&t("sampleRegion air; samplePatch west;")).contains(&refused), "{m} {group}");
            assert!(only(&t("samplePatch west;")).contains(&refused), "{m} {group}");
            assert!(
                !only(&t("sampleRegion air; samplePatch none;")).iter().any(|d| d.contains("refuses")),
                "{m} {group}"
            );
        }
    }
    // nearestFace asks the group for a patch not given: refused, and the region alone isn't
    // compared then (the group's region comes with it); the cells' modes read the region alone.
    let t =
        "boundary { air_to_vegetation { type mappedWall; sampleMode nearestFace; coupleGroup g; sampleRegion roof; } }";
    let only = |t: &str| {
        let d = differences(&root(t), &veg8, OfVersion::V8);
        d.into_iter().filter(|m| m.contains("air_to_vegetation")).collect::<Vec<_>>()
    };
    assert_eq!(
        only(t),
        [
            "patch air_to_vegetation: its coupleGroup names a patch, which OpenFOAM-8 refuses in sampleMode nearestFace (samplePatch none will do)"
        ]
    );
    for m in ["nearestCell", "nearestOnlyCell"] {
        let t = t.replace("nearestFace", m);
        assert_eq!(
            only(&t),
            ["patch air_to_vegetation: the region it maps to is roof there, the description makes it air"],
            "{m}"
        );
    }
    let face = |keys: &str| {
        format!("boundary {{ air_to_vegetation {{ type mappedWall; sampleMode nearestFace; coupleGroup g; {keys} }} }}")
    };
    // An empty name isn't a word: OpenFOAM stops on it, said once and not compared, with a
    // group or without, in either version.
    let air8 = entries(&d, "air", OfVersion::V8).unwrap();
    for keys in ["sampleRegion \"\"; samplePatch street;", "coupleGroup g; sampleRegion street; samplePatch \"\";"] {
        let t = format!("boundary {{ street {{ type mappedWall; sampleMode nearestPatchFace; {keys} }} }}");
        let k = if keys.starts_with("sampleRegion") { "sampleRegion" } else { "samplePatch" };
        assert_eq!(
            differences(&root(&t), &air8, OfVersion::V8),
            [format!("patch street: {k} is an empty name there, which OpenFOAM refuses (not a word)")],
            "{keys}"
        );
    }
    let nf = only(&face("sampleRegion air; samplePatch \"\";"));
    let blank = "patch air_to_vegetation: samplePatch is an empty name there, which OpenFOAM refuses (not a word)";
    assert_eq!(nf.iter().filter(|m| m.starts_with("patch air_to_vegetation")).collect::<Vec<_>>(), [blank]);
    let t = "boundary { street { type mappedWall; neighbourRegion \"\"; neighbourPatch street; } }";
    assert_eq!(
        differences(&root(t), &air12, OfVersion::V12),
        ["patch street: neighbourRegion is an empty name there, which OpenFOAM refuses (not a word)"]
    );
    // An empty type is another than the description's (OpenFOAM stops on it too).
    let t = "boundary { street { type \"\"; neighbourRegion street; neighbourPatch street; } }";
    assert_eq!(
        differences(&root(t), &air12, OfVersion::V12),
        ["patch street: type is \"\" there, the description makes it mappedWall"]
    );
    let t = "boundary { street { type mappedWall; neighbourRegion street; neighbourPatch street; method \"\"; } }";
    assert_eq!(
        differences(&root(t), &air12, OfVersion::V12),
        [
            "patch street: its method is \"\" there, which OpenFOAM-12 hasn't (it has intersection, inverseDistance, matching, nearest)"
        ]
    );
    let t = "boundary { street { type mappedWall; sampleMode \"\"; sampleRegion street; samplePatch street; } }";
    let diff = differences(&root(t), &air8, OfVersion::V8);
    assert!(diff.iter().any(|m| m.starts_with("patch street: its sampleMode is \"\" there")), "{diff:?}");
    // An empty group's name too, in either version.
    for (v, entries) in [(OfVersion::V8, &air8), (OfVersion::V12, &air12)] {
        let t = "boundary { street { type mappedWall; sampleMode nearestPatchFace; coupleGroup \"\"; } }";
        assert!(
            differences(&root(t), entries, v).contains(
                &"patch street: coupleGroup is an empty name there, which OpenFOAM refuses (not a word)".to_string()
            ),
            "{v}"
        );
    }
    // OpenFOAM-12 doesn't read an older key beside a newer one, nor a patch for mappedInternal.
    let t =
        "boundary { street { type mappedWall; neighbourRegion street; sampleRegion \"\"; neighbourPatch street; } }";
    assert_eq!(differences(&root(t), &air12, OfVersion::V12), Vec::<String>::new());
    let veg12e = entries(&d, "vegetation", OfVersion::V12).unwrap();
    let text = dict(&d, "vegetation", OfVersion::V12).unwrap();
    let t = text.replace("neighbourRegion air;", "neighbourRegion air;\n        neighbourPatch \"\";");
    assert!(t.contains("neighbourPatch \"\""), "{text}");
    assert_eq!(differences(&root(&t), &veg12e, OfVersion::V12), Vec::<String>::new());
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
    // A sampleMode left out is the mesh's: not told, not compared.
    let modeless = "boundary { street { type mappedWall; sampleRegion street; samplePatch street; } }";
    assert_eq!(differences(&root(modeless), &air8, OfVersion::V8), Vec::<String>::new());
    let air12 = entries(&d, "air", OfVersion::V12).unwrap();
    let alone = "boundary { street { type mappedWall; coupleGroup g; } }";
    assert_eq!(differences(&root(alone), &air12, OfVersion::V12), Vec::<String>::new());
    // samePatch maps to the patch of the same name, unconfirmed alone (the mesh's patch may
    // keep a neighbourPatch); refused with a patch named too, which then isn't compared.
    let veg12 = entries(&d, "vegetation", OfVersion::V12).unwrap();
    let text =
        dict(&d, "vegetation", OfVersion::V12).unwrap().replacen("neighbourPatch  west;", "samePatch       true;", 1);
    assert!(text.contains("samePatch       true;"));
    assert_eq!(
        differences(&root(&text), &veg12, OfVersion::V12),
        [
            "patch west: it has samePatch there, which OpenFOAM-12 refuses beside a neighbourPatch, and the mesh's patch keeps one it has (OpenFOAM-12 writes one on a mapped patch), which can't be confirmed"
        ]
    );
    for patch in ["west", "roof"] {
        let t =
            text.replacen("samePatch       true;", &format!("samePatch       y;\n        samplePatch     {patch};"), 1);
        assert_eq!(
            differences(&root(&t), &veg12, OfVersion::V12),
            ["patch west: it has samePatch and samplePatch there, which OpenFOAM-12 refuses (one or the other)"]
        );
    }
    let same = "boundary { street { type mappedWall; coupleGroup g; samePatch false; } }";
    assert_eq!(differences(&root(same), &air12, OfVersion::V12), Vec::<String>::new());
    let both = "boundary { street { type mappedWall; coupleGroup g; neighbourRegion street; } }";
    assert_eq!(
        differences(&root(both), &air12, OfVersion::V12),
        ["patch street: it has a coupleGroup and neighbourRegion there, which OpenFOAM-12 refuses (one or the other)"]
    );
    // The canopy samples the cells in nearestOnlyCell too; without a mode there (the mesh's,
    // not told), whether it samples a patch isn't told: the patch isn't compared.
    let t = canopy.replace("nearestCell;", "nearestOnlyCell;");
    assert_eq!(differences(&root(&t), &veg8, OfVersion::V8), Vec::<String>::new());
    let t = canopy.replace("sampleMode      nearestCell;", "samplePatch     street;");
    assert_eq!(differences(&root(&t), &veg8, OfVersion::V8), Vec::<String>::new());
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
