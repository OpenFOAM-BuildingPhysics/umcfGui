//! The `snappyHexMeshDict`s of real cases read into a region's snappy entry
//! (`workflow.import`), read only, from `UMCF_CASES` (`:`-separated case directories); skipped
//! without it. What is read and noted is printed (`--nocapture`).
//!
//! Each entry read is written as a dictionary again, and that dictionary reads back to the same
//! entry, with nothing noted: what the entry holds survives the trip.

use umcf_model::description::Description;
use umcf_model::snappy_import;

#[test]
fn the_cases_snappy_dictionaries_read_into_entries_that_read_back() {
    let Ok(cases) = std::env::var("UMCF_CASES") else {
        eprintln!("skip: UMCF_CASES not set");
        return;
    };
    let mut read = 0;
    for dir in cases.split(':').filter(|d| !d.is_empty()) {
        let options = umcf_model::OpenOptions { field_bytes: 0, ..Default::default() };
        let Ok(case) = umcf_model::Case::open_with(&umcf_core::DirFs::new(dir), options) else { continue };
        let (desc, notes) = Description::import(&case);
        for n in notes.iter().filter(|n| n.contains("snappy")) {
            eprintln!("{dir}: note: {n}");
        }
        for r in &desc.regions {
            let Some(sn) = &r.snappy else { continue };
            read += 1;
            eprintln!("{dir}: region {}: {} surfaces, {} boxes", r.name, sn.surfaces.len(), sn.boxes.len());
            let text = sn.dict(&r.name, "system", &|_| "wall".to_string());
            let doc = umcf_foam_dict::Document::parse(&text).unwrap();
            let (back, left) = snappy_import::import(&doc.root, "system/snappyHexMeshDict", case.layout.version)
                .map_err(|e| e.0)
                .unwrap();
            assert_eq!(&back, sn, "{dir}: region {}", r.name);
            assert_eq!(left, Vec::<String>::new(), "{dir}: region {}", r.name);
        }
    }
    eprintln!("{read} snappy entries read");
}
