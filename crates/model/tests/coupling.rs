//! Each region's `changeDictionaryDict` generated from the description `workflow.import` reads
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
        eprintln!("skip: UMCF_CASES not set");
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
