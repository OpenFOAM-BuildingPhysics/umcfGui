//! Each region's `changeDictionaryDict` (and each `createPatchDict`, below) generated from the description `workflow.import` reads
//! from a real case (`umcf_model::coupling`), compared with the case's own, read only, from
//! `UMCF_CASES` (`:`-separated case directories); skipped without it.
//!
//! For every region whose steps run `changeDictionary` and whose own dictionary is there, the
//! generated entries are the case's in what the coupling needs: each patch's type and the region
//! and patch it maps to (`coupling::differences`), for the same patches. The generated text
//! parses to the same entries and is marked as generated. A case whose OpenFOAM version isn't
//! told is skipped; a case that doesn't open fails it, and so does a run that compares no
//! dictionary.

use umcf_model::coupling;
use umcf_model::description::Description;

#[test]
fn the_cases_change_dictionaries_are_generated_from_their_descriptions() {
    let Ok(cases) = std::env::var("UMCF_CASES") else {
        eprintln!(
            "SKIP the_cases_change_dictionaries_are_generated_from_their_descriptions: set UMCF_CASES=<case>:<case>… to run it"
        );
        return;
    };
    let mut compared = 0;
    for dir in cases.split(':').filter(|d| !d.is_empty()) {
        // With the fields: the air's inlet, outlet and sides are told by their velocity conditions.
        let case = match umcf_model::Case::open_with(&umcf_core::DirFs::new(dir), umcf_model::OpenOptions::default()) {
            Ok(c) => c,
            Err(e) => panic!("{dir}: doesn't open: {e}"),
        };
        let (desc, _) = Description::import(&case);
        // As workflow.dictionaries and the check take it.
        let Some(version) = desc.version.or(case.layout.version) else {
            eprintln!("{dir}: its OpenFOAM version isn't told: not compared");
            continue;
        };
        for r in desc.regions.iter().filter(|r| coupling::runs(r)) {
            let path = coupling::dict_path(&r.name);
            let Some(own) = case.dict(&path) else { continue };
            let entries = coupling::entries(&desc, &r.name, version).unwrap_or_else(|e| panic!("{dir}: {path}: {e}"));
            let diff = coupling::differences(&own.root, &entries, version);
            assert_eq!(diff, Vec::<String>::new(), "{dir}: {path}");
            // And no patch the case's dictionary changes is left out.
            let theirs: Vec<&str> = own
                .root
                .get("boundary")
                .and_then(umcf_foam_dict::Entry::dict)
                // A pattern (quoted) isn't a patch: `differences` leaves it alone too.
                .map(|b| {
                    b.entries()
                        .filter(|e| !e.key.text.starts_with('"') && e.dict().is_some())
                        .map(|e| e.key())
                        .collect()
                })
                .unwrap_or_default();
            for n in theirs {
                assert!(entries.iter().any(|e| e.patch == n), "{dir}: {path}: patch {n} not generated");
            }
            let text = coupling::dict(&desc, &r.name, version).unwrap();
            let doc = umcf_foam_dict::Document::parse(&text).unwrap();
            assert!(umcf_model::blocks::untouched(&text), "{dir}: {path}");
            assert_eq!(coupling::differences(&doc.root, &entries, version), Vec::<String>::new(), "{dir}: {path}");
            eprintln!("{dir}: {path}: {} patches as the case's", entries.len());
            compared += 1;
        }
    }
    eprintln!("{compared} changeDictionaryDicts compared");
    assert!(compared > 0, "UMCF_CASES gives no case with a changeDictionaryDict");
}

/// Each `createPatchDict` the description's meshes and steps read, generated from what
/// `workflow.import` reads of the case's own (`umcf_model::create_patch`), against the case's
/// own: every patch it makes of others, as it makes it (`create_patch::differences`, the same
/// patches, sources and types). A patch whose role the import can't tell (a case without a
/// mesh) leaves its case uncompared, noted; a run that compares none fails.
#[test]
fn the_cases_create_patch_dictionaries_are_generated_from_their_descriptions() {
    use umcf_model::create_patch;
    let Ok(cases) = std::env::var("UMCF_CASES") else {
        eprintln!(
            "SKIP the_cases_create_patch_dictionaries_are_generated_from_their_descriptions: set UMCF_CASES=<case>:<case>… to run it"
        );
        return;
    };
    let mut compared = 0;
    for dir in cases.split(':').filter(|d| !d.is_empty()) {
        let case = match umcf_model::Case::open_with(&umcf_core::DirFs::new(dir), umcf_model::OpenOptions::default()) {
            Ok(c) => c,
            Err(e) => panic!("{dir}: doesn't open: {e}"),
        };
        let (desc, _) = Description::import(&case);
        let Some(version) = desc.version.or(case.layout.version) else {
            eprintln!("{dir}: its OpenFOAM version isn't told: not compared");
            continue;
        };
        let targets = match create_patch::targets(&desc, None) {
            Ok(t) => t,
            Err(e) if e.contains("neither a role nor a type") => {
                eprintln!("{dir}: {e}: not compared");
                continue;
            }
            Err(e) => panic!("{dir}: {e}"),
        };
        for t in &targets {
            let Some(own) = case.dict(&t.path) else {
                assert_ne!(case.exists(&t.path), Some(true), "{dir}: {} is there but doesn't load", t.path);
                continue;
            };
            assert_eq!(
                create_patch::differences(&own.root, &t.made, version),
                Vec::<String>::new(),
                "{dir}: {}",
                t.path
            );
            let text = create_patch::dict(t, version);
            let doc = umcf_foam_dict::Document::parse(&text).unwrap();
            assert!(umcf_model::blocks::untouched(&text), "{dir}: {}", t.path);
            assert_eq!(
                create_patch::differences(&doc.root, &t.made, version),
                Vec::<String>::new(),
                "{dir}: {}",
                t.path
            );
            eprintln!("{dir}: {}: {} patches as the case's", t.path, t.made.len());
            compared += 1;
        }
    }
    eprintln!("{compared} createPatchDicts compared");
    assert!(compared > 0, "UMCF_CASES gives no case with a createPatchDict");
}
